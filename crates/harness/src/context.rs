//! Editable conversation content. Native envelopes remain Store-owned evidence.
use crate::{
    item::{Item, ItemHash},
    model::{Effort, OperationId, RequestId},
    turn::JobOutput,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The resolved provider contract selected by the transport owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextProjectionTarget {
    pub model: String,
    pub route: crate::transport::ResponsesRoute,
    pub protocol: crate::transport::ResponsesProtocol,
}

/// Resolved current inference choices observed together at the owning head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveInferenceSelection {
    pub model: String,
    pub effort: Effort,
    pub head: Option<RequestId>,
}

/// Exact continuity losses from an explicitly selected visible-history recovery.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryProjectionReceipt {
    pub omitted: Vec<ContextReference>,
    pub unknown_response_completeness: Vec<RequestId>,
}

/// Structural replay refusals retain identities, never provider payloads.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ContextReplayRejection {
    #[error("provider response provenance is unavailable for request {request:?}")]
    MissingProvenance { request: RequestId },
    #[error("provider response membership is not an exact ordered selection")]
    InvalidMembership,
    #[error("captured context is not an authenticated provider response prefix")]
    InvalidPrefix,
    #[error("opaque provider continuity has no established route or protocol compatibility")]
    IncompatibleTransport,
    #[error("native invocation has no exact owning operation at {request:?}:{position}")]
    UnownedInvocation { request: RequestId, position: i64 },
}

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
/// reasoning need provider-qualified authority.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextTextSelector {
    MessageText {
        part: u32,
    },
    ToolResultText,
    /// Plaintext provider reasoning in one exact content part.
    ReasoningText {
        part: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextVisibleText {
    pub reference: ContextReference,
    pub selector: ContextTextSelector,
    pub text: String,
    pub editable: bool,
    /// Only independently authored occurrences can be removed from native groups.
    #[serde(default)]
    pub removable: bool,
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
    pub(crate) prefix_origins: Option<Vec<Origin>>,
    pub(crate) retained_exchanges: Vec<RetainedExchangeReference>,
    pub(crate) output: Item,
    pub(crate) invocation: Item,
    pub(crate) model: Option<String>,
    pub(crate) next_effort: Option<Effort>,
}

impl ContextCommitEvidence {
    pub fn original_operation(&self) -> &OperationId {
        &self.original_operation
    }

    /// A copied result does not reissue the original native operation's format.
    /// Validate the sealed marker even when the receiving identity requires demotion.
    pub(crate) fn replay_output_format(
        &self,
        operation: &OperationId,
    ) -> Result<crate::provider::OutputFormat, serde_json::Error> {
        let format = self.output.output_format()?;
        Ok(if operation == &self.original_operation {
            format
        } else {
            crate::provider::OutputFormat::LegacyOpaque
        })
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

/// Structural portability refusals contain item/field names, never item values.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ContextPortabilityRejection {
    #[error("response group has no visible content")]
    NoVisibleContent,
    #[error("unsupported response item kind")]
    UnsupportedItem,
    #[error("unsupported {item} field: {field}")]
    UnsupportedField { item: &'static str, field: String },
    #[error("invalid {item} field: {field}")]
    InvalidField {
        item: &'static str,
        field: &'static str,
    },
    #[error("encrypted payload outside reasoning continuity")]
    EncryptedPayload,
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
    #[error("context replay refused: {0}")]
    Replay(ContextReplayRejection),
    #[error("context publication requires a successful complete invocation")]
    Ineligible,
    #[error("context publication was cancelled before admission")]
    Cancelled,
    #[error("context state has an unsupported format")]
    UnsupportedState,
    #[error("context portability refused: {0}")]
    Portability(ContextPortabilityRejection),
    #[error("context model must be a nonempty resolved model name")]
    InvalidModel,
}

/// Content versions describe visible content, independently of snapshot generation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTextVersion {
    pub reference: ContextReference,
    pub selector: ContextTextSelector,
    pub digest: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUnitVersion {
    pub reference: ContextReference,
    pub digest: String,
}
/// Canonical authorship comes from Store's retained occurrence, independently
/// of its current visible overlay or editing authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextTextAuthor {
    User,
    Assistant,
    Tool,
    System,
    Developer,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTextAuthorship {
    pub reference: ContextReference,
    pub selector: ContextTextSelector,
    pub author: ContextTextAuthor,
    pub authored_note: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextInspection {
    pub document: ContextDocument,
    pub text_versions: Vec<ContextTextVersion>,
    pub unit_versions: Vec<ContextUnitVersion>,
    pub authorship: Vec<ContextTextAuthorship>,
}
/// An exact retained exchange identity. Store revalidates authority at use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RetainedExchangeReference(pub(crate) String);

impl RetainedExchangeReference {
    pub fn from_raw(raw: String) -> Self {
        Self(raw)
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum RetainedExchangeObservation {
    Available(RetainedExchangeReference),
    Missing,
    Denied,
    Pending,
    Unpublished,
    NotStarted,
    Failure,
    Cancelled,
    CancelledWithReceipt,
    Interrupted,
    CancellationUnconfirmed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextPatch {
    pub changes: Vec<ContextChange>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContextChange {
    ReplaceContextText {
        reference: ContextReference,
        selector: ContextTextSelector,
        expected: String,
        text: String,
    },
    RemoveContextUnit {
        reference: ContextReference,
        expected: String,
    },
    AppendContextNote {
        text: String,
        sources: Vec<ContextReference>,
    },
    AppendRetainedExchange {
        source: RetainedExchangeReference,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextProblemCode {
    Stale,
    Missing,
    Ambiguous,
    Denied,
    InvalidPatch,
    Ineligible,
    AlreadyStaged,
    UnsupportedSelector,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextProblem {
    pub code: ContextProblemCode,
    pub reference: Option<ContextReference>,
    pub detail: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextTransaction {
    pub raw: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTransactionReceipt {
    pub head: RequestId,
    pub generation: u64,
    pub changed: bool,
    pub appended: Vec<ContextReference>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum ContextTransactionStatus {
    Staged,
    Committed(ContextTransactionReceipt),
    Rejected(Vec<ContextProblem>),
    Cancelled,
    Unknown(String),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextTransactionDraft {
    pub patch: ContextPatch,
    pub next_model: Option<String>,
    pub next_effort: Option<Effort>,
}

pub(crate) fn context_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}

/// Inspect supported editable bodies and removable units without granting authority.
/// Authorship is empty here; only Store can issue it from canonical occurrences.
pub fn inspect_context(document: &ContextDocument) -> ContextInspection {
    let mut text_versions = Vec::new();
    let mut unit_versions = Vec::new();
    for block in &document.blocks {
        match block {
            ContextBlock::Text {
                reference: Some(reference),
                text,
                ..
            } => {
                let digest = context_digest(text.as_bytes());
                text_versions.push(ContextTextVersion {
                    reference: reference.clone(),
                    selector: ContextTextSelector::MessageText { part: 0 },
                    digest: digest.clone(),
                });
                unit_versions.push(ContextUnitVersion {
                    reference: reference.clone(),
                    digest,
                });
            }
            ContextBlock::Native {
                reference,
                protected,
                kind,
                texts,
                ..
            } => {
                if !protected && *kind == ContextNativeKind::CompletedExchange {
                    unit_versions.push(ContextUnitVersion {
                        reference: reference.clone(),
                        digest: context_digest(
                            &serde_json::to_vec(&(
                                reference,
                                texts
                                    .iter()
                                    .map(|field| (&field.reference, &field.selector, &field.text))
                                    .collect::<Vec<_>>(),
                            ))
                            .expect("context serialization"),
                        ),
                    });
                }
                for field in texts {
                    let digest = context_digest(field.text.as_bytes());
                    if field.editable {
                        text_versions.push(ContextTextVersion {
                            reference: field.reference.clone(),
                            selector: field.selector.clone(),
                            digest: digest.clone(),
                        });
                    }
                    if field.removable {
                        unit_versions.push(ContextUnitVersion {
                            reference: field.reference.clone(),
                            digest,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    ContextInspection {
        document: document.clone(),
        text_versions,
        unit_versions,
        authorship: Vec::new(),
    }
}

/// Validate one immutable batch against one document; errors never return partial edits.
/// Store independently checks the issuing cut and current structural authority.
pub fn check_context_patch(
    document: &ContextDocument,
    patch: &ContextPatch,
) -> Result<ContextDocument, Vec<ContextProblem>> {
    use std::collections::HashSet;
    let inspection = inspect_context(document);
    let mut result = document.clone();
    let mut problems = Vec::new();
    let mut texts = HashSet::new();
    let mut removals = HashSet::new();
    for change in &patch.changes {
        let (reference, matches, expected) = match change {
            ContextChange::ReplaceContextText {
                reference,
                selector,
                expected,
                ..
            } => {
                if !texts.insert((reference.clone(), selector.clone())) {
                    problems.push(ContextProblem {
                        code: ContextProblemCode::InvalidPatch,
                        reference: Some(reference.clone()),
                        detail: "text target occurs more than once in the batch".into(),
                    });
                }
                (
                    reference,
                    inspection
                        .text_versions
                        .iter()
                        .filter(|v| &v.reference == reference && &v.selector == selector)
                        .map(|v| &v.digest)
                        .collect::<Vec<_>>(),
                    expected,
                )
            }
            ContextChange::RemoveContextUnit {
                reference,
                expected,
            } => {
                if !removals.insert(reference.clone()) {
                    problems.push(ContextProblem {
                        code: ContextProblemCode::InvalidPatch,
                        reference: Some(reference.clone()),
                        detail: "unit target occurs more than once in the batch".into(),
                    });
                }
                (
                    reference,
                    inspection
                        .unit_versions
                        .iter()
                        .filter(|v| &v.reference == reference)
                        .map(|v| &v.digest)
                        .collect::<Vec<_>>(),
                    expected,
                )
            }
            ContextChange::AppendRetainedExchange { .. } => continue,
            ContextChange::AppendContextNote { sources, .. } => {
                for source in sources {
                    if !context_has_reference(document, source) {
                        problems.push(ContextProblem {
                            code: ContextProblemCode::Denied,
                            reference: Some(source.clone()),
                            detail: "note source is outside the inspected context".into(),
                        });
                    }
                }
                continue;
            }
        };
        if expected.len() != 64
            || !expected
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            problems.push(ContextProblem {
                code: ContextProblemCode::InvalidPatch,
                reference: Some(reference.clone()),
                detail: "expected version must be an owner-issued SHA-256 digest".into(),
            });
            continue;
        }
        let code = match matches.as_slice() {
            [] if context_has_reference(document, reference) => Some(ContextProblemCode::Denied),
            [] => Some(ContextProblemCode::Missing),
            [actual] if *actual != expected => Some(ContextProblemCode::Stale),
            [_] => None,
            _ => Some(ContextProblemCode::Ambiguous),
        };
        if let Some(code) = code {
            problems.push(ContextProblem {
                code,
                reference: Some(reference.clone()),
                detail: "target version or editable authority does not match".into(),
            });
        }
    }
    for (reference, _) in &texts {
        if removals.contains(reference) || document.blocks.iter().any(|b| matches!(b, ContextBlock::Native {reference: group, texts: fields, ..} if removals.contains(group) && fields.iter().any(|f| &f.reference == reference))) {
            problems.push(ContextProblem { code: ContextProblemCode::InvalidPatch, reference: Some(reference.clone()), detail: "batch both edits and removes the same unit".into() });
        }
    }
    for block in &document.blocks {
        if let ContextBlock::Native {
            reference, texts, ..
        } = block
        {
            if removals.contains(reference)
                && texts.iter().any(|field| {
                    field.reference != *reference && removals.contains(&field.reference)
                })
            {
                problems.push(ContextProblem {
                    code: ContextProblemCode::InvalidPatch,
                    reference: Some(reference.clone()),
                    detail: "batch removes both a unit and one of its members".into(),
                });
            }
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    for change in &patch.changes {
        match change {
            ContextChange::ReplaceContextText {
                reference,
                selector,
                text,
                ..
            } => {
                for block in &mut result.blocks {
                    match block {
                        ContextBlock::Text {
                            reference: Some(r),
                            text: body,
                            ..
                        } if r == reference => *body = text.clone(),
                        ContextBlock::Native { texts, .. } => {
                            for field in texts {
                                if field.reference == *reference && field.selector == *selector {
                                    field.text = text.clone();
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            ContextChange::RemoveContextUnit { reference, .. } => {
                result.blocks.retain(|b| !matches!(b, ContextBlock::Text {reference:Some(r),..} | ContextBlock::Native {reference:r,..} if r == reference));
                for block in &mut result.blocks {
                    if let ContextBlock::Native { texts, .. } = block {
                        texts.retain(|field| &field.reference != reference);
                    }
                }
            }
            ContextChange::AppendRetainedExchange { .. } => {}
            ContextChange::AppendContextNote { text, sources } => {
                result.blocks.push(ContextBlock::Text {
                    reference: None,
                    role: ContextRole::Assistant,
                    text: text.clone(),
                    sources: sources.clone(),
                })
            }
        }
    }
    Ok(result)
}

pub(crate) fn context_has_reference(
    document: &ContextDocument,
    reference: &ContextReference,
) -> bool {
    document.blocks.iter().any(|block| match block {
        ContextBlock::Text {
            reference: Some(r),
            sources,
            ..
        } => r == reference || sources.contains(reference),
        ContextBlock::Native {
            reference: r,
            texts,
            ..
        } => r == reference || texts.iter().any(|f| &f.reference == reference),
        _ => false,
    })
}
