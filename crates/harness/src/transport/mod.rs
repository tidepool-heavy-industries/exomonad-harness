//! Streaming Codex Responses transport. Authentication never persists secrets.
//!
//! Stateless requests, read-only authentication, and SSE item assembly.

pub mod auth;
pub mod client;
pub mod sse;

use crate::item::Item;
use crate::model::Effort;
use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug)]
pub struct ResponsesRequest {
    pub input: Vec<Item>,
    pub instructions: String,
    pub tools: Vec<Value>,
    pub model: String,
    pub pinned_effort: Effort,
    pub session_id: String,
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub cache_write_tokens: u64,
}

#[derive(Clone, Debug)]
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
