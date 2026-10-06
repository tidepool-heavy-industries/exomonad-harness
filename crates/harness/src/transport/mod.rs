//! Streaming Responses transport with provider-bound credential sources.
//!
//! Stateless requests, protected native authentication, and SSE item assembly.
//! Codex-owned credentials remain a separate read-only source.

pub mod auth;
pub mod client;
mod http_error;
mod request_failure;
pub mod sse;
pub use request_failure::RequestFailure;

use crate::item::Item;
use crate::model::Effort;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{ops::Deref, sync::Arc};
use thiserror::Error;

/// Immutable advertised tools and their cached strict-schema admission result.
/// Cloning retains the same schema bytes; HTTP serialization requires admission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolManifest(Arc<ManifestData>);

#[derive(Debug, PartialEq, Eq)]
struct ManifestData {
    tools: Vec<Value>,
    json: String,
    hash: crate::item::ItemHash,
    duplicate_names: bool,
    strict_validation: Result<(), String>,
}

impl Default for ToolManifest {
    fn default() -> Self {
        Vec::new().into()
    }
}

impl ToolManifest {
    pub(crate) fn strict_tools(&self) -> Result<StrictToolManifest<'_>, TransportError> {
        self.0
            .strict_validation
            .as_ref()
            .map_err(|error| TransportError::Stream(error.clone()))?;
        Ok(StrictToolManifest(self))
    }
    pub(crate) fn encoded(&self) -> (&crate::item::ItemHash, &str) {
        (&self.0.hash, &self.0.json)
    }

    pub(crate) fn has_duplicate_names(&self) -> bool {
        self.0.duplicate_names
    }
}

/// Only the immutable manifest's cached admission can construct this transport
/// serialization token. Raw JSON declarations cannot enter an HTTP request.
pub(crate) struct StrictToolManifest<'a>(&'a ToolManifest);

impl Serialize for StrictToolManifest<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl From<Vec<Value>> for ToolManifest {
    fn from(tools: Vec<Value>) -> Self {
        let strict_validation = tools.iter().try_for_each(|tool| {
            if tool["type"] == "function" {
                if tool["strict"] != true {
                    return Err("all function tools must be strict".to_owned());
                }
                crate::finalize::validate_function_parameters(&tool["parameters"])
                    .map_err(|error| format!("invalid function parameters: {error}"))?;
            }
            Ok(())
        });
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
            strict_validation,
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
    #[error("terminal HTTP status {status}{suffix}", suffix = diagnostic_suffix(.diagnostic.as_ref()))]
    Http {
        status: u16,
        diagnostic: Option<HttpDiagnostic>,
    },
    #[error("provider stream failed ({event}){suffix}", suffix = diagnostic_suffix(.diagnostic.as_ref()))]
    ProviderStreamFailure {
        event: ProviderStreamFailureEvent,
        diagnostic: Option<HttpDiagnostic>,
    },
    #[error("stream failed: {0}")]
    Stream(String),
    #[error("stream failed: {0}")]
    IncompleteResponse(StreamInterruption),
    #[error("replay request {request:?} already has a recorded owner")]
    ReplayRequestReuse { request: crate::model::RequestId },
}

/// An explicit provider failure event, distinct from local framing or schema
/// errors. It gives no proof that escaped tool work did not execute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStreamFailureEvent {
    #[error("response.failed")]
    ResponseFailed,
    #[error("error")]
    Error,
}

/// The response ended without completion proof. This does not establish whether
/// completed tool items escaped the stream or their external calls ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum StreamInterruption {
    #[error("missing response.completed")]
    MissingCompletion,
    #[error("SSE read failed")]
    ReadFailed,
}

/// Allowlisted, bounded provider error fields. The transport removes credentials
/// before constructing this value; it never retains the provider's raw body.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct HttpDiagnostic {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn diagnostic_suffix(diagnostic: Option<&HttpDiagnostic>) -> String {
    diagnostic.map_or_else(String::new, |diagnostic| {
        format!(
            ": {}",
            serde_json::to_string(diagnostic).expect("diagnostic strings serialize")
        )
    })
}

/// Request credentials bound to the provider that issued them. Deliberately
/// has no Debug implementation: diagnostic formatting must not expose tokens.
pub enum AuthCredentials {
    Codex {
        access_token: String,
        account_id: String,
    },
    ChatGptPlan {
        access_token: String,
    },
}

/// The credential authority determines the fixed provider endpoint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResponsesRoute {
    #[default]
    Codex,
    ChatGptPlan,
}

/// Credential source. Codex-owned files remain read-only; a native source may
/// refresh only its own registration. A 401 is surfaced as Authentication.
pub trait Auth: Send + Sync {
    fn access(&self) -> Result<(String, String), TransportError>;

    fn route(&self) -> ResponsesRoute {
        ResponsesRoute::Codex
    }

    fn credentials(&self) -> Result<AuthCredentials, TransportError> {
        let (access_token, account_id) = self.access()?;
        Ok(AuthCredentials::Codex {
            access_token,
            account_id,
        })
    }
}

/// Selected wire contract, independent of model-name spelling and Engine jobs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ResponsesProtocol {
    #[default]
    Standard,
    Lite,
}

pub struct ResponsesClient<A: Auth> {
    pub auth: A,
    protocol: ResponsesProtocol,
}

impl<A: Auth + Clone + 'static> ResponsesClient<A> {
    pub fn new(auth: A) -> Self {
        Self {
            auth,
            protocol: ResponsesProtocol::Standard,
        }
    }

    pub fn with_protocol(mut self, protocol: ResponsesProtocol) -> Self {
        self.protocol = protocol;
        self
    }

    pub async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        client::execute(self.auth.clone(), self.protocol, request, None).await
    }

    /// Sends completed items as they arrive; text deltas are best-effort.
    /// The returned turn remains authoritative even if the receiver closes.
    pub async fn create_streaming(
        &self,
        request: ResponsesRequest,
        sink: tokio::sync::mpsc::Sender<sse::StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        client::execute(self.auth.clone(), self.protocol, request, Some(sink)).await
    }
}
