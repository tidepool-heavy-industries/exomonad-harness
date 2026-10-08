//! Editable conversation content. Native envelopes remain Store-owned evidence.
use crate::{
    item::{Item, ItemHash},
    model::{Effort, OperationId, RequestId},
    turn::JobOutput,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContextReference(pub(crate) String);

impl ContextReference {
    /// Preserve an opaque reference across a typed wire boundary. Possession
    /// grants no authority; Store validates it against the invocation's cut.
    pub fn from_raw(value: String) -> Self {
        Self(value)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    User,
    Assistant,
}

impl ContextRole {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextNativeKind {
    CompletedExchange,
    Opaque,
    Pending,
}

/// Closed selectors for provider-visible bodies. Invocation inputs and opaque
/// reasoning never grant editable text authority.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextTextSelector {
    MessageText { part: u32 },
    ToolResultText,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextVisibleText {
    pub reference: ContextReference,
    pub selector: ContextTextSelector,
    pub text: String,
    pub editable: bool,
}

/// Occurrence-local request projection; canonical item bytes stay unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ContextTextOverlay {
    pub selector: ContextTextSelector,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextBlock {
    Text {
        reference: Option<ContextReference>,
        role: ContextRole,
        text: String,
        sources: Vec<ContextReference>,
    },
    Native {
        reference: ContextReference,
        kind: ContextNativeKind,
        preview: String,
        protected: bool,
        #[serde(default)]
        texts: Vec<ContextVisibleText>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextDocument {
    pub blocks: Vec<ContextBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextDraft {
    pub document: ContextDocument,
    pub next_model: Option<String>,
    pub next_effort: Option<Effort>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContextSnapshot {
    pub document: ContextDocument,
    pub generation: u64,
    pub head: RequestId,
    pub operation: OperationId,
    pub(crate) prefix: Vec<Occurrence>,
    pub(crate) blocks: Vec<StoredBlock>,
    pub(crate) store_id: String,
    pub(crate) seal: SnapshotSeal,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SnapshotSeal {
    pub operation: OperationId,
    pub head: RequestId,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Occurrence {
    pub request: RequestId,
    pub position: i64,
    pub hash: ItemHash,
    pub item: Item,
    pub output_operation: Option<OperationId>,
    pub origin: Origin,
    pub sources: Vec<ContextReference>,
    pub note: bool,
    pub overlays: Vec<ContextTextOverlay>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Origin {
    pub request: RequestId,
    pub position: i64,
    pub hash: ItemHash,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoredBlock {
    pub block: ContextBlock,
    pub items: Vec<Occurrence>,
    pub opaque: bool,
    pub mandatory: bool,
}

pub struct ContextCommit<'a> {
    pub snapshot: &'a ContextSnapshot,
    pub draft: &'a ContextDraft,
    pub output: &'a JobOutput,
    pub pending: &'a [OperationId],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCommitReceipt {
    pub head: RequestId,
    pub generation: u64,
    pub changed: bool,
    pub model: Option<String>,
}

/// Hydrated by Store from a versioned, terminal-bound commit event. Its private
/// fields prevent a provider from manufacturing replay authority.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextCommitEvidence {
    pub(crate) version: u32,
    pub(crate) original_operation: OperationId,
    pub(crate) prefix: Vec<(Item, Vec<ContextReference>, bool, Vec<ContextTextOverlay>)>,
    pub(crate) output: Item,
    pub(crate) invocation: Item,
    pub(crate) model: Option<String>,
    pub(crate) next_effort: Option<Effort>,
}

impl ContextCommitEvidence {
    pub fn original_operation(&self) -> &OperationId {
        &self.original_operation
    }
}

#[derive(Clone, Debug)]
pub struct ContextRequestState {
    /// Provider-facing projections and their exact interned hashes. Native
    /// occurrences retain canonical request identity; editable references are
    /// resolved independently against raw Store provenance.
    pub history: Vec<(RequestId, ItemHash, Item)>,
    pub(crate) occurrences: Vec<Option<Occurrence>>,
    pub model: Option<String>,
    pub generation: u64,
}

#[derive(Debug, Error)]
pub enum ContextError {
    #[error("context changed since this synchronous invocation began")]
    Conflict,
    #[error("the exact issuing call is unavailable in this conversation")]
    MissingCall,
    #[error("context reference is not part of this editable prefix")]
    InvalidReference,
    #[error("native context identity and text authority are read-only")]
    NativeEdit,
    #[error("a pending, opaque or cross-boundary native group cannot be removed")]
    ProtectedGroup,
    #[error("retained native groups must keep their original order")]
    NativeOrder,
    #[error("opaque provider continuity has no established compatibility with the requested model")]
    OpaqueModel,
    #[error("context publication requires a successful complete invocation")]
    Ineligible,
    #[error("context publication was cancelled before admission")]
    Cancelled,
    #[error("context state has an unsupported format")]
    UnsupportedState,
    #[error("context model must be a nonempty resolved model name")]
    InvalidModel,
}
