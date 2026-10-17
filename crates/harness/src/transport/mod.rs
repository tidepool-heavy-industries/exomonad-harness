//! Streaming Responses transport with provider-bound credential sources.
//!
//! Stateless requests, protected native authentication, and SSE item assembly.
//! Explicit ChatGPT-plan and DeepSeek credentials under Standard Responses.

mod admission;
pub use admission::{
    CredentialReference, InferenceLimits, NativeTransportAdmission, NativeTransportContract,
};
pub mod auth;
pub mod client;
mod http_error;
mod identity;
pub use identity::SessionIdentity;
mod observation;
pub use observation::{
    EventKind, IncompleteReason, ResponseEnd, ResponseTerminal, TransportFailureSummary,
};
mod request_failure;
pub mod sse;
mod usage;
pub use crate::store::PreparedProviderRequest;
pub use request_failure::RequestFailure;
pub(crate) use request_failure::TransportFailureKind;
pub use usage::{RawUsage, Usage};

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

/// Requests best-effort cache diagnostics against one retained provider response.
/// This selection does not load history or change cache behavior.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct PromptCacheComparison {
    pub comparison_response_id: String,
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
    pub session_id: SessionIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_cache_comparison: Option<PromptCacheComparison>,
}

/// The selected native contract or an exact step issued by the replay owner.
/// This capability is never reconstructed from recorded JSON.
#[derive(Clone)]
pub struct PreparedTransportSelection(PreparedTransportEvidence);

#[derive(Clone)]
enum PreparedTransportEvidence {
    Component(ResponsesRoute, ResponsesProtocol),
    Native(NativeTransportAdmission),
    Replay(crate::replay::PreparedReplayStep),
}

impl PreparedTransportSelection {
    pub(crate) fn component(contract: (ResponsesRoute, ResponsesProtocol)) -> Self {
        Self(PreparedTransportEvidence::Component(contract.0, contract.1))
    }
    pub(crate) fn native(admission: NativeTransportAdmission) -> Self {
        Self(PreparedTransportEvidence::Native(admission))
    }
    pub(crate) fn native_admission(&self) -> Option<&NativeTransportAdmission> {
        match &self.0 {
            PreparedTransportEvidence::Native(admission) => Some(admission),
            _ => None,
        }
    }
    pub(crate) fn replay(step: crate::replay::PreparedReplayStep) -> Self {
        Self(PreparedTransportEvidence::Replay(step))
    }
    pub fn response_contract(&self) -> (ResponsesRoute, ResponsesProtocol) {
        match &self.0 {
            PreparedTransportEvidence::Component(route, protocol) => (*route, *protocol),
            PreparedTransportEvidence::Native(admission) => {
                (admission.contract().route, admission.contract().protocol)
            }
            PreparedTransportEvidence::Replay(step) => step.response_contract(),
        }
    }
    pub(crate) fn replay_step(&self) -> Option<&crate::replay::PreparedReplayStep> {
        match &self.0 {
            PreparedTransportEvidence::Replay(step) => Some(step),
            PreparedTransportEvidence::Native(..) | PreparedTransportEvidence::Component(..) => {
                None
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ResponsesTurn {
    pub response_id: String,
    pub items: Vec<Item>,
    pub usage: Usage,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("provider response.incomplete ({response_id}, {reason:?})")]
    ResponseIncomplete {
        /// Bounded/redacted diagnostic ID, not exact provider replay provenance.
        response_id: String,
        reason: Option<IncompleteReason>,
    },
    #[error("invalid native transport admission contract")]
    InvalidAdmission,
    #[error("unsupported provider request semantics: {0}")]
    UnsupportedSemantics(UnsupportedSemantics),
    #[error("prepared provider selection does not match its owning transport or step")]
    PreparedSelectionMismatch,
    #[error("prepared transport contract does not match the selected replay turn")]
    ReplayContractMismatch,
    #[error("provider request has no retained transport contract")]
    UnprovenResponseContract,
    #[error("provider completed item has no output index")]
    MissingOutputIndex,
    #[error("provider output index {index} has conflicting items")]
    ConflictingOutputIndex { index: u64 },
    #[error("completed provider response has an output index gap at {index}")]
    IncompleteOutputIndices { index: u64 },
    #[error("native ChatGPT plan sign-in required; private Codex credentials are unsupported")]
    Authentication,
    #[error("{route:?} credentials unavailable: {reason}")]
    CredentialUnavailable {
        route: ResponsesRoute,
        reason: CredentialFailure,
    },
    #[error("{route:?} authentication rejected")]
    ProviderAuthentication { route: ResponsesRoute },
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

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Error, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedSemantics {
    #[error("reasoning effort")]
    ReasoningEffort,
    #[error("tool declaration")]
    ToolDeclaration,
    #[error("input item")]
    InputItem,
    #[error("message role or content")]
    Message,
    #[error("encrypted reasoning or reasoning summary")]
    ReasoningContent,
    #[error("cache comparison")]
    CacheComparison,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Error, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum CredentialFailure {
    #[error("selected source is missing")]
    Missing,
    #[error("selected source cannot be read")]
    Unreadable,
    #[error("selected source is unsafe")]
    Unsafe,
    #[error("selected value is invalid")]
    Invalid,
}

/// An explicit provider failure event, distinct from local framing or schema
/// errors. It gives no proof that escaped tool work did not execute.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Error, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStreamFailureEvent {
    #[error("response.failed")]
    ResponseFailed,
    #[error("error")]
    Error,
}

/// The response ended without completion proof. This does not establish whether
/// completed tool items escaped the stream or their external calls ran.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Error, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum StreamInterruption {
    #[error("missing response.completed")]
    MissingCompletion,
    #[error("SSE read failed")]
    ReadFailed,
}

/// Allowlisted, bounded provider error fields. The transport removes credentials
/// before constructing this value; it never retains the provider's raw body.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
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

/// The tool encoding implemented by an admitted provider route.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEncodingProfile {
    Native,
    FunctionOnlyExplicitDelivery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResponseFinality {
    ExplicitPhase,
    CompletedAssistantText,
}
impl ToolEncodingProfile {
    /// Validate the actual retained call before parsing can erase wire fields.
    pub fn validate_invocation_item(self, item: &Item) -> Result<(), TransportError> {
        if self == Self::Native {
            return Ok(());
        }
        let name = item.0["name"].as_str().unwrap_or("");
        if item.0["type"] != "function_call"
            || item.0.get("namespace").is_some()
            || item.0.get("async").is_some()
            || name.is_empty()
            || name.len() > 128
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        {
            return Err(TransportError::UnsupportedSemantics(
                UnsupportedSemantics::InputItem,
            ));
        }
        Ok(())
    }
}

#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ResponsesRoute {
    #[default]
    ChatGptPlan,
    DeepSeek,
}

impl ResponsesRoute {
    pub const fn response_finality(self) -> ResponseFinality {
        match self {
            Self::ChatGptPlan => ResponseFinality::ExplicitPhase,
            Self::DeepSeek => ResponseFinality::CompletedAssistantText,
        }
    }
    pub const fn tool_encoding_profile(self) -> ToolEncodingProfile {
        match self {
            Self::ChatGptPlan => ToolEncodingProfile::Native,
            Self::DeepSeek => ToolEncodingProfile::FunctionOnlyExplicitDelivery,
        }
    }
    /// The implemented wire profile, independent of requested model spelling.
    pub const fn supports_plaintext_reasoning_edits(self) -> bool {
        matches!(self, Self::DeepSeek)
    }
}

/// Public-plan token source. A 401 requires operator sign-in; tokens never enter
/// retained requests or diagnostic formatting.
pub trait Auth: Send + Sync {
    fn admission(&self) -> Result<NativeTransportAdmission, TransportError>;
    fn access(&self) -> Result<String, TransportError>;
}

/// Selected wire contract, independent of model-name spelling and Engine jobs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponsesProtocol {
    #[default]
    Standard,
}

pub struct ResponsesClient<A: Auth> {
    auth: A,
    admission: Result<NativeTransportAdmission, ()>,
    http: std::sync::OnceLock<Result<reqwest::Client, &'static str>>,
}

impl<A: Auth + Clone + 'static> ResponsesClient<A> {
    pub fn new(auth: A) -> Self {
        Self {
            admission: auth.admission().map_err(|_| ()),
            auth,
            http: std::sync::OnceLock::new(),
        }
    }

    pub fn admission(&self) -> Result<&NativeTransportAdmission, TransportError> {
        self.admission
            .as_ref()
            .map_err(|_| TransportError::InvalidAdmission)
    }

    fn http(&self) -> Result<reqwest::Client, TransportError> {
        self.http
            .get_or_init(|| {
                client::http_client_with_limits(
                    self.admission()
                        .map_err(|_| "invalid native admission")?
                        .contract()
                        .limits,
                )
                .map_err(|_| "HTTP client initialization failed")
            })
            .clone()
            .map_err(|message| TransportError::Stream(message.into()))
    }

    pub async fn create(
        &self,
        request: PreparedProviderRequest,
    ) -> Result<ResponsesTurn, TransportError> {
        client::execute(
            self.auth.clone(),
            self.admission()?.clone(),
            self.http()?,
            request,
            None,
        )
        .await
    }

    /// Sends completed items as they arrive; text deltas are best-effort.
    /// The returned turn remains authoritative even if the receiver closes.
    pub async fn create_streaming(
        &self,
        request: PreparedProviderRequest,
        sink: tokio::sync::mpsc::Sender<sse::StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        client::execute(
            self.auth.clone(),
            self.admission()?.clone(),
            self.http()?,
            request,
            Some(sink),
        )
        .await
    }
}
