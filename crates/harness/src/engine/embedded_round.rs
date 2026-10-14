//! Engine-owned embedded-round observations. Construction stays with execution,
//! cleanup and Store settlement; a host observes these facts without rebuilding
//! policy from errors or issuing recovery authority.
use super::{EngineError, RequestId, StoreError};
use crate::{embedding::InferenceBlock, model::OperationId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundExecution {
    Completed,
    Rejected,
    Interrupted,
    Cancelled,
    Failed,
}

/// Obligations accumulate independently. An observation names retained state;
/// only the Store's existing journaled recovery grant authorizes continuation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InferenceObligation {
    ExplicitInput { request: RequestId },
    RecoverBlock { block: InferenceBlock },
    Reconcile { head: Option<RequestId> },
    PreserveUnsettled { request: RequestId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreContinuation {
    NoReopenRequired,
    ReopenRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundCleanup {
    /// This round's obligations have affirmative release evidence.
    Confirmed,
    /// Owned obligations are released while exact foreign work is observed.
    ReleasedWithFollowup,
    Unconfirmed,
}

#[derive(Debug)]
pub struct RetainedFollowup(crate::turn::ForeignPublicationObservation);
impl RetainedFollowup {
    pub fn operation(&self) -> &OperationId {
        self.0.operation()
    }
    pub fn publication(&self) -> crate::turn::FollowupPublicationState {
        self.0.state()
    }
}

/// One immutable assessment of the owner's retained action and receipts.
/// Consumers cannot construct or independently update these observations.
#[derive(Debug)]
pub struct RoundAssessment {
    execution: RoundExecution,
    head: Option<RequestId>,
    inference: Vec<InferenceObligation>,
    storage: StoreContinuation,
    cleanup: RoundCleanup,
    outstanding: Vec<OperationId>,
    retained: Vec<RetainedFollowup>,
    continuity_refusal: bool,
    settlements: Vec<crate::store::EmbeddedRoundSettlement>,
}
impl RoundAssessment {
    pub fn execution(&self) -> RoundExecution {
        self.execution
    }
    pub fn head(&self) -> Option<&RequestId> {
        self.head.as_ref()
    }
    pub fn inference_obligations(&self) -> &[InferenceObligation] {
        &self.inference
    }
    pub fn store_continuation(&self) -> StoreContinuation {
        self.storage
    }
    pub fn cleanup(&self) -> RoundCleanup {
        self.cleanup
    }
    pub fn outstanding_operations(&self) -> &[OperationId] {
        &self.outstanding
    }
    pub fn retained_followups(&self) -> &[RetainedFollowup] {
        &self.retained
    }
    pub(super) fn head_settlements(&self) -> &[crate::store::EmbeddedRoundSettlement] {
        &self.settlements
    }
    pub(super) fn continuity_refused(&self) -> bool {
        self.continuity_refusal
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundFailureStage {
    NativeCleanup,
    OperationClaimSettlement,
    ProviderTailRecording,
    ProviderResponseSettlement,
    FollowupObservation,
    InterruptedRequestRecording,
    RejectedRequestRecording,
    InferenceBlockRecording,
    InferenceAttemptSettlement,
    DurableHeadSettlement,
}

/// Sources retain their owning error types; labels and rendering never classify.
#[derive(Debug, thiserror::Error)]
pub enum RoundFailureSource {
    #[error(transparent)]
    Engine(EngineError),
    #[error(transparent)]
    Store(StoreError),
}

#[derive(Debug)]
pub struct RoundSecondaryFailure {
    stage: RoundFailureStage,
    source: RoundFailureSource,
}
impl RoundSecondaryFailure {
    pub fn stage(&self) -> RoundFailureStage {
        self.stage
    }
    pub fn source(&self) -> &RoundFailureSource {
        &self.source
    }
}

/// Private receipts retain actual issued operations until affirmative owner
/// observation confirms cleanup. An absent error wrapper issues no receipt.
#[derive(Debug)]
pub(super) enum CleanupReceipt {
    Unobserved,
    Confirmed,
    Outstanding(Vec<OperationId>),
}

pub(super) struct ConsumedCleanup {
    pub(super) receipt: CleanupReceipt,
    pub(super) retained: Vec<crate::turn::ForeignPublicationObservation>,
}

#[derive(Debug)]
enum RoundAction {
    Completed {
        head: RequestId,
    },
    Failed {
        head: Option<RequestId>,
        cause: EngineError,
    },
}

/// Original action, staged failures and cleanup receipts share one owner.
/// The assessment is derived from this state, never cached as mutable policy.
#[derive(Debug)]
pub struct EmbeddedRoundFailure {
    action: RoundAction,
    secondary: Vec<RoundSecondaryFailure>,
    cleanup: CleanupReceipt,
    inference: Vec<InferenceObligation>,
    retained: Vec<crate::turn::ForeignPublicationObservation>,
}

/// Waiting preserves why a notification cannot start a provider request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmbeddedRoundWait {
    NoPendingWork,
    ExplicitInputRequired { request: RequestId },
    RecoveryRequired { block: InferenceBlock },
}

/// This opaque selection is tied to one driver and an exact Store observation.
/// Store revalidation and attempt issuance follow native host admission.
pub struct EmbeddedRoundSelection {
    generation: std::sync::Arc<()>,
    identity: crate::embedding::HostIdentity,
    snapshot: crate::store::EmbeddedRoundSnapshot,
}

pub enum EmbeddedRoundReadiness {
    Selected(EmbeddedRoundSelection),
    Waiting(EmbeddedRoundWait),
}

struct ErrorFacts {
    execution: RoundExecution,
    head: Option<RequestId>,
    inference: Vec<InferenceObligation>,
    reopen: bool,
    cleanup_uncertain: bool,
    outstanding: Vec<OperationId>,
    continuity_refusal: bool,
    settlements: Vec<crate::store::EmbeddedRoundSettlement>,
}
fn retain_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}
impl ErrorFacts {
    fn failed(head: Option<RequestId>) -> Self {
        Self {
            execution: RoundExecution::Failed,
            head,
            inference: Vec::new(),
            reopen: false,
            cleanup_uncertain: false,
            outstanding: Vec::new(),
            continuity_refusal: false,
            settlements: Vec::new(),
        }
    }
    fn accumulate(&mut self, other: Self) {
        self.reopen |= other.reopen;
        self.cleanup_uncertain |= other.cleanup_uncertain;
        self.continuity_refusal |= other.continuity_refusal;
        for obligation in other.inference {
            retain_unique(&mut self.inference, obligation);
        }
        for settlement in other.settlements {
            retain_unique(&mut self.settlements, settlement);
        }
        for operation in other.outstanding {
            retain_unique(&mut self.outstanding, operation);
        }
    }
    fn store(&mut self, source: &StoreError) {
        self.continuity_refusal |= source.is_inference_projection_refusal();
        let requirements = source.recovery_requirements();
        self.reopen |= requirements.reopen;
        if let Some(operation) = requirements.uncertain {
            self.cleanup_uncertain = true;
            retain_unique(&mut self.outstanding, operation.clone());
        }
        if let Some(block) = requirements.block {
            retain_unique(
                &mut self.inference,
                InferenceObligation::RecoverBlock {
                    block: block.clone(),
                },
            );
        }
    }
}

impl EngineError {
    /// This is the sole Engine error fold. Secondary causes add obligations but
    /// cannot replace the action's execution fact or improve cleanup certainty.
    fn round_facts(&self, head: Option<RequestId>) -> ErrorFacts {
        use super::EngineCleanupError;
        let mut facts = ErrorFacts::failed(head);
        match self {
            Self::Secondary(failure) => {
                facts = failure
                    .primary
                    .round_facts(failure.head_request.clone().or(facts.head));
                facts.accumulate(failure.source.round_facts(failure.head_request.clone()));
            }
            Self::BlockedAction(failure) => {
                facts = failure
                    .primary
                    .round_facts(Some(failure.block.request.clone()));
                if let Some(settlement) = &failure.head_settlement {
                    retain_unique(&mut facts.settlements, settlement.clone());
                }
                facts.inference.retain(|obligation| !matches!(obligation,
                    InferenceObligation::ExplicitInput { request } if request == &failure.block.request));
                retain_unique(
                    &mut facts.inference,
                    InferenceObligation::RecoverBlock {
                        block: failure.block.clone(),
                    },
                );
            }
            Self::CompletionSettlement(failure) => {
                facts.execution = RoundExecution::Completed;
                facts.head = Some(failure.completion.head_request.clone());
                facts.accumulate(failure.source.round_facts(facts.head.clone()));
            }
            Self::InferenceBlockPersistence {
                primary,
                persistence,
            } => {
                facts = primary.round_facts(facts.head);
                facts.store(persistence);
            }
            Self::Cleanup { primary, cleanup } => {
                facts = primary.round_facts(facts.head);
                facts.cleanup_uncertain = true;
                match cleanup {
                    EngineCleanupError::Engine(source) => {
                        facts.accumulate(source.round_facts(facts.head.clone()))
                    }
                    EngineCleanupError::External(_) => {}
                }
            }
            Self::CleanupFailures(failures) => {
                facts.cleanup_uncertain = true;
                for failure in failures {
                    facts.accumulate(failure.round_facts(facts.head.clone()));
                }
            }
            Self::Store(source) => facts.store(source),
            Self::Cancelled { head_request } => {
                facts.execution = RoundExecution::Cancelled;
                facts.head = head_request.clone().or(facts.head);
                retain_unique(
                    &mut facts.inference,
                    InferenceObligation::Reconcile {
                        head: facts.head.clone(),
                    },
                );
            }
            Self::RequestRejected { head_request, .. } => {
                facts.execution = RoundExecution::Rejected;
                facts.head = Some(head_request.clone());
            }
            Self::InterruptedModelRound { head_request, .. } => {
                facts.execution = RoundExecution::Interrupted;
                facts.head = Some(head_request.clone());
                retain_unique(
                    &mut facts.inference,
                    InferenceObligation::ExplicitInput {
                        request: head_request.clone(),
                    },
                );
            }
            Self::InferenceBlocked { block } => {
                facts.head = Some(block.request.clone());
                facts.execution = match &block.reason {
                    crate::embedding::InferenceBlockReason::StreamInterrupted { .. } => {
                        RoundExecution::Interrupted
                    }
                    crate::embedding::InferenceBlockReason::RequestRejected { .. } => {
                        RoundExecution::Rejected
                    }
                    crate::embedding::InferenceBlockReason::ProviderStreamFailure { .. }
                    | crate::embedding::InferenceBlockReason::ContinuityUnproven
                    | crate::embedding::InferenceBlockReason::HostInterrupted => {
                        RoundExecution::Failed
                    }
                };
                retain_unique(
                    &mut facts.inference,
                    InferenceObligation::RecoverBlock {
                        block: block.clone(),
                    },
                );
            }
            Self::Transport(source) => match source.failure_kind() {
                crate::transport::TransportFailureKind::Interrupted(_) => {
                    facts.execution = RoundExecution::Interrupted;
                    if let Some(request) = &facts.head {
                        retain_unique(
                            &mut facts.inference,
                            InferenceObligation::ExplicitInput {
                                request: request.clone(),
                            },
                        );
                    }
                }
                crate::transport::TransportFailureKind::Rejected(_) => {
                    facts.execution = RoundExecution::Rejected
                }
                crate::transport::TransportFailureKind::ProviderFailed { .. }
                | crate::transport::TransportFailureKind::Other => {}
            },
            Self::UnconfirmedCancellation { operation, .. } => {
                facts.cleanup_uncertain = true;
                retain_unique(&mut facts.outstanding, operation.clone());
            }
            Self::InterruptedHeadMismatch
            | Self::RejectedHeadMismatch
            | Self::StoreTask
            | Self::Job(_)
            | Self::MissingFinal
            | Self::Finalize(_)
            | Self::InvalidFinalizeCount
            | Self::InvalidToolSelection(_)
            | Self::InvalidInjectedItem
            | Self::ProviderCall(_)
            | Self::InvalidFunctionCall
            | Self::MismatchedToolOutput(_)
            | Self::MissingEffortPin
            | Self::MultipleWaitAgents
            | Self::ReservedYieldTool
            | Self::InvalidYieldArguments
            | Self::Compact(_)
            | Self::UnresumableForkedWaitAgent
            | Self::UnclaimedInheritedCall { .. }
            | Self::IncompleteRecordedResponse(_)
            | Self::MissingInheritedOutput(_)
            | Self::MissingOperationInvocation(_)
            | Self::ClaimRecoveryConflict(_)
            | Self::MissingHereSnapshot => {}
        }
        facts
    }
}
impl EmbeddedRoundFailure {
    pub fn assessment(&self) -> RoundAssessment {
        let facts = match &self.action {
            RoundAction::Completed { head } => {
                let mut facts = ErrorFacts::failed(Some(head.clone()));
                facts.execution = RoundExecution::Completed;
                facts
            }
            RoundAction::Failed { head, cause } => cause.round_facts(head.clone()),
        };
        assess_evidence(
            facts,
            &self.secondary,
            &self.cleanup,
            &self.inference,
            &self.retained,
        )
    }
    pub fn primary_error(&self) -> Option<&EngineError> {
        match &self.action {
            RoundAction::Failed { cause, .. } => Some(cause),
            RoundAction::Completed { .. } => None,
        }
    }
    pub fn secondary_failures(&self) -> &[RoundSecondaryFailure] {
        &self.secondary
    }
}

/// One execution owner's private receipt crosses into both standalone rendering
/// and embedded settlement. No global or separately mutable classifier exists.
pub(super) struct EngineRunReceipt {
    pub(super) result: Result<super::EngineCompletion, EngineError>,
    pub(super) head: Option<RequestId>,
    pub(super) cleanup: CleanupReceipt,
    pub(super) secondary: Vec<RoundSecondaryFailure>,
    pub(super) inference: Vec<InferenceObligation>,
    pub(super) retained: Vec<crate::turn::ForeignPublicationObservation>,
}
impl EngineRunReceipt {
    pub(super) fn assessment(&self) -> RoundAssessment {
        let facts = match &self.result {
            Ok(completion) => {
                let mut facts = ErrorFacts::failed(Some(completion.head_request.clone()));
                facts.execution = RoundExecution::Completed;
                facts
            }
            Err(cause) => cause.round_facts(self.head.clone()),
        };
        assess_evidence(
            facts,
            &self.secondary,
            &self.cleanup,
            &self.inference,
            &self.retained,
        )
    }
    pub(super) fn add_failure(&mut self, stage: RoundFailureStage, source: EngineError) {
        self.secondary.push(RoundSecondaryFailure {
            stage,
            source: RoundFailureSource::Engine(source),
        });
    }
    pub(super) fn into_result(self) -> Result<super::EngineCompletion, EngineError> {
        let mut result = self.result;
        for failure in self.secondary {
            let source = match failure.source {
                RoundFailureSource::Engine(source) => source,
                RoundFailureSource::Store(source) => source.into(),
            };
            result = if failure.stage == RoundFailureStage::NativeCleanup {
                match result {
                    Err(primary) => Err(EngineError::Cleanup {
                        primary: Box::new(primary),
                        cleanup: source.into(),
                    }),
                    Ok(completion) => {
                        super::retain_post_action_failure(Ok(completion), failure.stage, source)
                    }
                }
            } else {
                super::retain_post_action_failure(result, failure.stage, source)
            };
        }
        for obligation in self.inference {
            if let InferenceObligation::RecoverBlock { block } = obligation {
                result = match result {
                    Err(primary) => Err(EngineError::BlockedAction(super::EngineBlockedAction {
                        primary: Box::new(primary),
                        block,
                        head_settlement: None,
                    })),
                    // The completed action remains valid, but continuation is
                    // refused by the issued block. This is settlement evidence,
                    // not a failure to persist the block.
                    Ok(completion) => super::retain_post_action_failure(
                        Ok(completion),
                        RoundFailureStage::InferenceAttemptSettlement,
                        EngineError::InferenceBlocked { block },
                    ),
                };
            }
        }
        result
    }
    fn into_failure(self) -> EmbeddedRoundFailure {
        EmbeddedRoundFailure {
            action: match self.result {
                Ok(completion) => RoundAction::Completed {
                    head: completion.head_request,
                },
                Err(cause) => RoundAction::Failed {
                    head: self.head,
                    cause,
                },
            },
            secondary: self.secondary,
            cleanup: self.cleanup,
            inference: self.inference,
            retained: self.retained,
        }
    }
}

fn assess_evidence(
    mut facts: ErrorFacts,
    secondary: &[RoundSecondaryFailure],
    cleanup_receipt: &CleanupReceipt,
    inference: &[InferenceObligation],
    retained: &[crate::turn::ForeignPublicationObservation],
) -> RoundAssessment {
    for failure in secondary {
        match &failure.source {
            RoundFailureSource::Engine(source) => {
                facts.accumulate(source.round_facts(facts.head.clone()))
            }
            RoundFailureSource::Store(source) => facts.store(source),
        }
        if failure.stage == RoundFailureStage::NativeCleanup {
            facts.cleanup_uncertain = true;
        }
    }
    for obligation in inference {
        if let InferenceObligation::RecoverBlock { block } = obligation {
            facts.inference.retain(|existing| {
                !matches!(existing, InferenceObligation::ExplicitInput { request } if request == &block.request)
            });
        }
        retain_unique(&mut facts.inference, obligation.clone());
    }
    let cleanup = match cleanup_receipt {
        CleanupReceipt::Unobserved => RoundCleanup::Unconfirmed,
        CleanupReceipt::Confirmed if !facts.cleanup_uncertain => {
            if retained.is_empty() {
                RoundCleanup::Confirmed
            } else {
                RoundCleanup::ReleasedWithFollowup
            }
        }
        CleanupReceipt::Confirmed => RoundCleanup::Unconfirmed,
        CleanupReceipt::Outstanding(operations) => {
            for operation in operations {
                retain_unique(&mut facts.outstanding, operation.clone());
            }
            RoundCleanup::Unconfirmed
        }
    };
    RoundAssessment {
        execution: facts.execution,
        head: facts.head,
        inference: facts.inference,
        storage: if facts.reopen {
            StoreContinuation::ReopenRequired
        } else {
            StoreContinuation::NoReopenRequired
        },
        cleanup,
        outstanding: facts.outstanding,
        retained: retained.iter().cloned().map(RetainedFollowup).collect(),
        continuity_refusal: facts.continuity_refusal,
        settlements: facts.settlements,
    }
}

pub(super) struct OrderedProviderFailure {
    pub(super) cause: EngineError,
    pub(super) cleanup: CleanupReceipt,
    pub(super) retained: Vec<crate::turn::ForeignPublicationObservation>,
}
