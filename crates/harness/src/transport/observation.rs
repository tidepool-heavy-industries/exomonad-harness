//! Bounded native response metadata, delivered through the issued request receipt.
use super::http_error;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// Diagnostic retention budgets, independent of the provider protocol limits.
const KIND_TAIL: usize = 8;
const METADATA_BYTES: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IncompleteReason {
    MaxOutputTokens,
    ContentFilter,
    Steered,
    Unknown,
}
impl IncompleteReason {
    pub(super) fn from_provider(value: &str) -> Self {
        match value {
            "max_output_tokens" => Self::MaxOutputTokens,
            "content_filter" => Self::ContentFilter,
            "steered" => Self::Steered,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    Created,
    Completed,
    Incomplete,
    Failed,
    Error,
    OutputItemAdded,
    OutputItemDone,
    OutputTextDelta,
    ReasoningSummaryDelta,
    ReasoningDelta,
    FunctionArgumentsDelta,
    ToolInputDelta,
    RefusalDelta,
    Missing,
    Unknown { sha256: String },
}
impl EventKind {
    pub(super) fn from_provider(value: Option<&str>) -> Self {
        match value {
            Some("response.created") => Self::Created,
            Some("response.completed") => Self::Completed,
            Some("response.incomplete") => Self::Incomplete,
            Some("response.failed") => Self::Failed,
            Some("error") => Self::Error,
            Some("response.output_item.added") => Self::OutputItemAdded,
            Some("response.output_item.done") => Self::OutputItemDone,
            Some("response.output_text.delta") => Self::OutputTextDelta,
            Some("response.reasoning_summary_text.delta") => Self::ReasoningSummaryDelta,
            Some("response.reasoning_text.delta") => Self::ReasoningDelta,
            Some("response.function_call_arguments.delta") => Self::FunctionArgumentsDelta,
            Some("response.custom_tool_call_input.delta") => Self::ToolInputDelta,
            Some("response.refusal.delta") => Self::RefusalDelta,
            Some(value) => Self::Unknown {
                sha256: format!("{:x}", Sha256::digest(value.as_bytes())),
            },
            None => Self::Missing,
        }
    }
    fn recognized(&self) -> bool {
        !matches!(self, Self::Unknown { .. } | Self::Missing)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseTerminal {
    Completed,
    ResponseFailed,
    Error,
    Incomplete {
        response_id: String,
        reason: Option<IncompleteReason>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseEnd {
    CleanEof,
    ReadFailed,
    ProviderTerminal,
    ProtocolError,
    HttpRejected,
    SendFailed,
    AuthenticationFailed,
    AdmissionFailed,
    /// The producer future was dropped; the caller owns why and effect cleanup.
    Abandoned,
}

/// Metadata is not response provenance or permission to retry. The Engine's
/// retained provider items, operations and failure assessment own those facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportObservation {
    pub version: u8,
    #[serde(default)]
    pub requested_model: Option<String>,
    #[serde(default)]
    pub backend: Option<super::ResponsesRoute>,
    #[serde(default)]
    pub effective_reasoning: Option<crate::model::Effort>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub started_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub completed_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub queue_ms: Option<u64>,
    #[serde(default)]
    /// Monotonic elapsed from request observation start, including queue/auth.
    pub first_event_ms: Option<u64>,
    #[serde(default)]
    /// Monotonic elapsed from request observation start, including queue/auth.
    pub first_text_ms: Option<u64>,
    #[serde(default)]
    /// Monotonic elapsed from request observation start, including queue/auth.
    pub first_reasoning_ms: Option<u64>,
    #[serde(default)]
    pub total_ms: Option<u64>,
    #[serde(default)]
    pub dispatch_attempted: Option<bool>,
    #[serde(default)]
    pub usage: Option<super::Usage>,
    #[serde(default)]
    pub returned_model: Option<String>,
    #[serde(default)]
    pub returned_reasoning_effort: Option<String>,
    #[serde(default)]
    pub reported_max_output_tokens: Option<u64>,
    pub http_status: Option<u16>,
    pub server_request_id: Option<String>,
    pub content_type: Option<String>,
    pub response_created_id: Option<String>,
    pub body_elapsed_ms: u64,
    /// Monotonic elapsed from response-body reading start, excluding queue/auth.
    pub first_byte_ms: Option<u64>,
    pub received_bytes: Option<u64>,
    pub received_chunks: Option<u64>,
    pub framed_events: Option<u64>,
    pub event_kind_tail: Vec<EventKind>,
    pub sse_event_kind_tail: Vec<EventKind>,
    pub last_recognized_event: Option<EventKind>,
    pub unknown_event_count: u64,
    pub unframed_line_bytes: u64,
    pub unframed_data_bytes: u64,
    pub unframed_event_name: bool,
    pub metadata_truncated: bool,
    pub terminal: Option<ResponseTerminal>,
    pub end: ResponseEnd,
}

pub(super) fn metadata(value: &str, token: &str, account: &str) -> (String, bool) {
    let mut value = http_error::redact(value, token, account);
    let truncated = value.len() > METADATA_BYTES;
    let mut end = value.len().min(METADATA_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    (value, truncated)
}

fn push_kind(tail: &mut Vec<EventKind>, value: EventKind) {
    if tail.len() == KIND_TAIL {
        tail.remove(0);
    }
    tail.push(value);
}

impl TransportObservation {
    pub(super) fn new(end: ResponseEnd) -> Self {
        Self {
            version: 3,
            requested_model: None,
            backend: None,
            effective_reasoning: None,
            max_output_tokens: None,
            started_at_unix_ms: None,
            completed_at_unix_ms: None,
            queue_ms: None,
            first_event_ms: None,
            first_text_ms: None,
            first_reasoning_ms: None,
            total_ms: None,
            dispatch_attempted: None,
            usage: None,
            returned_model: None,
            returned_reasoning_effort: None,
            reported_max_output_tokens: None,
            http_status: None,
            server_request_id: None,
            content_type: None,
            response_created_id: None,
            body_elapsed_ms: 0,
            first_byte_ms: None,
            received_bytes: None,
            received_chunks: None,
            framed_events: None,
            event_kind_tail: Vec::new(),
            sse_event_kind_tail: Vec::new(),
            last_recognized_event: None,
            unknown_event_count: 0,
            unframed_line_bytes: 0,
            unframed_data_bytes: 0,
            unframed_event_name: false,
            metadata_truncated: false,
            terminal: None,
            end,
        }
    }
    pub(super) fn observe_response_metadata(
        &mut self,
        response: &serde_json::Value,
        token: &str,
        account: &str,
    ) {
        self.usage = Some(super::Usage::from_response_usage(response));
        for (value, destination) in [
            (response.get("model"), &mut self.returned_model),
            (
                response
                    .get("reasoning")
                    .and_then(|reasoning| reasoning.get("effort")),
                &mut self.returned_reasoning_effort,
            ),
        ] {
            if let Some(value) = value.and_then(serde_json::Value::as_str) {
                let (value, truncated) = metadata(value, token, account);
                self.metadata_truncated |= truncated;
                *destination = Some(value);
            }
        }
        self.reported_max_output_tokens = response
            .get("max_output_tokens")
            .and_then(serde_json::Value::as_u64);
    }
    pub(super) fn response(response: &reqwest::Response, token: &str, account: &str) -> Self {
        let mut value = Self::new(ResponseEnd::HttpRejected);
        value.http_status = Some(response.status().as_u16());
        for (name, destination) in [
            ("x-request-id", &mut value.server_request_id),
            ("content-type", &mut value.content_type),
        ] {
            if let Some(text) = response
                .headers()
                .get(name)
                .and_then(|header| header.to_str().ok())
            {
                let (text, truncated) = metadata(text, token, account);
                value.metadata_truncated |= truncated;
                *destination = Some(text);
            }
        }
        value
    }
    pub(super) fn event(&mut self, kind: EventKind) {
        if kind.recognized() {
            self.last_recognized_event = Some(kind.clone());
        } else {
            self.unknown_event_count = self.unknown_event_count.saturating_add(1);
        }
        push_kind(&mut self.event_kind_tail, kind);
    }
    pub(super) fn sse_event(&mut self, kind: EventKind) {
        push_kind(&mut self.sse_event_kind_tail, kind);
    }
}
