//! Streaming Codex Responses transport. Authentication never persists secrets.
//!
//! Stateless requests, read-only authentication, and SSE item assembly.

pub mod auth;
pub mod client;
pub mod sse;

use crate::item::Item;
use crate::model::Effort;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{ops::Deref, sync::Arc};
use thiserror::Error;

/// Immutable advertised tools. Cloning a manifest retains the same schema bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolManifest(Arc<ManifestData>);

#[derive(Debug, PartialEq, Eq)]
struct ManifestData {
    tools: Vec<Value>,
    json: String,
    hash: crate::item::ItemHash,
    duplicate_names: bool,
}

impl Default for ToolManifest {
    fn default() -> Self {
        Vec::new().into()
    }
}

impl ToolManifest {
    pub(crate) fn encoded(&self) -> (&crate::item::ItemHash, &str) {
        (&self.0.hash, &self.0.json)
    }

    pub(crate) fn has_duplicate_names(&self) -> bool {
        self.0.duplicate_names
    }
}

impl From<Vec<Value>> for ToolManifest {
    fn from(tools: Vec<Value>) -> Self {
        let json = serde_json::to_string(&tools).expect("tool Values serialize");
        let hash = crate::item::ItemHash(blake3::hash(json.as_bytes()).to_hex().to_string());
        let mut names = std::collections::HashSet::new();
        let duplicate_names = tools.iter().any(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| !names.insert(name))
        });
        Self(Arc::new(ManifestData {
            tools,
            json,
            hash,
            duplicate_names,
        }))
    }
}

impl Deref for ToolManifest {
    type Target = [Value];
    fn deref(&self) -> &[Value] {
        &self.0.tools
    }
}

impl Serialize for ToolManifest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.tools.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ToolManifest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Vec::<Value>::deserialize(deserializer)?.into())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResponsesRequest {
    pub input: Vec<Item>,
    pub instructions: String,
    pub tools: ToolManifest,
    /// None keeps automatic choice; Some selects a subset, including empty.
    #[serde(default)]
    pub tools_allowed: Option<Vec<String>>,
    pub model: String,
    pub pinned_effort: Effort,
    pub session_id: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Usage {
    /// True only when both input and output counters were reported.
    #[serde(default)]
    pub reported: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResponsesTurn {
    pub response_id: String,
    pub items: Vec<Item>,
    pub usage: Usage,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("authentication expired; operator action required")]
    Authentication,
    #[error("terminal HTTP status {0}")]
    Http(u16),
    #[error("stream failed: {0}")]
    Stream(String),
    #[error("replay request {request} already has a recorded owner")]
    ReplayRequestReuse { request: crate::model::RequestId },
}

/// Read-only credential source. An implementation must never refresh Codex's
/// credential file; a 401 is surfaced as Authentication.
pub trait Auth: Send + Sync {
    fn access(&self) -> Result<(String, String), TransportError>;
}

pub struct ResponsesClient<A: Auth> {
    pub auth: A,
}

impl<A: Auth + Clone + 'static> ResponsesClient<A> {
    pub fn new(auth: A) -> Self {
        Self { auth }
    }

    pub async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        client::execute(self.auth.clone(), request, None).await
    }

    /// Sends completed items as they arrive; text deltas are best-effort.
    /// The returned turn remains authoritative even if the receiver closes.
    pub async fn create_streaming(
        &self,
        request: ResponsesRequest,
        sink: tokio::sync::mpsc::Sender<sse::StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        client::execute(self.auth.clone(), request, Some(sink)).await
    }
}
