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
    secondary_stages: Vec<RoundFailureStage>,
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
    pub fn secondary_failure_stages(&self) -> &[RoundFailureStage] {
        &self.secondary_stages
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

/// A flat staged view borrows original typed sources from the execution owner.
/// Sources may retain composite errors. Consumers decide from the canonical
/// assessment and obligations; these source references are diagnostic evidence.
#[derive(Clone, Copy, Debug)]
pub enum RoundFailureSourceRef<'a> {
    Engine(&'a EngineError),
    Store(&'a StoreError),
    Cleanup(&'a super::EngineCleanupError),
}
#[derive(Clone, Copy, Debug)]
pub struct RoundSecondaryFailureRef<'a> {
    stage: RoundFailureStage,
    source: RoundFailureSourceRef<'a>,
}
impl<'a> RoundSecondaryFailureRef<'a> {
    pub fn stage(&self) -> RoundFailureStage {
        self.stage
    }
    pub fn source(&self) -> RoundFailureSourceRef<'a> {
        self.source
    }
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
        completion: super::EngineCompletion,
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
    head_settlements: Vec<crate::store::EmbeddedRoundSettlement>,
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
    driver: std::sync::Arc<()>,
    generation: std::sync::Arc<()>,
    identity: crate::embedding::HostIdentity,
    snapshot: crate::store::EmbeddedRoundSnapshot,
}

pub enum EmbeddedRoundReadiness {
    Selected(EmbeddedRoundSelection),
    Waiting(EmbeddedRoundWait),
}

struct ErrorFacts<'a> {
    execution: RoundExecution,
    cleanup_intent: crate::provider::CancellationCause,
    head: Option<RequestId>,
    inference: Vec<InferenceObligation>,
    reopen: bool,
    cleanup_uncertain: bool,
    outstanding: Vec<OperationId>,
    continuity_refusal: bool,
    settlements: Vec<crate::store::EmbeddedRoundSettlement>,
    primary: Option<&'a EngineError>,
    completion: Option<&'a super::EngineCompletion>,
    secondary: Vec<RoundSecondaryFailureRef<'a>>,
}
fn retain_unique<T: PartialEq>(items: &mut Vec<T>, item: T) {
    if !items.contains(&item) {
        items.push(item);
    }
}
impl<'a> ErrorFacts<'a> {
    fn failed(head: Option<RequestId>) -> Self {
        Self {
            execution: RoundExecution::Failed,
            cleanup_intent: crate::provider::CancellationCause::RoundFailureCleanup,
            head,
            inference: Vec::new(),
            reopen: false,
            cleanup_uncertain: false,
            outstanding: Vec::new(),
            continuity_refusal: false,
            settlements: Vec::new(),
            primary: None,
            completion: None,
            secondary: Vec::new(),
        }
    }
    fn accumulate(&mut self, other: Self) {
        self.secondary.extend(other.secondary);
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
    pub(super) fn cleanup_cancellation_intent(&self) -> crate::provider::CancellationCause {
        self.round_facts(None).cleanup_intent
    }
    /// This is the sole Engine error fold. Secondary causes add obligations but
    /// cannot replace the action's execution fact or improve cleanup certainty.
    fn round_facts(&self, head: Option<RequestId>) -> ErrorFacts<'_> {
        use super::EngineCleanupError;
        let mut facts = ErrorFacts::failed(head);
        facts.primary = Some(self);
        match self {
            Self::Secondary(failure) => {
                facts = failure
                    .primary
                    .round_facts(failure.head_request.clone().or(facts.head));
                facts.secondary.push(RoundSecondaryFailureRef {
                    stage: failure.stage,
                    source: RoundFailureSourceRef::Engine(&failure.source),
                });
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
                facts.primary = None;
                facts.completion = Some(&failure.completion);
                facts.secondary.push(RoundSecondaryFailureRef {
                    stage: failure.stage,
                    source: RoundFailureSourceRef::Engine(&failure.source),
                });
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
                facts.secondary.push(RoundSecondaryFailureRef {
                    stage: RoundFailureStage::InferenceBlockRecording,
                    source: RoundFailureSourceRef::Store(persistence),
                });
            }
            Self::Cleanup { primary, cleanup } => {
                facts = primary.round_facts(facts.head);
                facts.cleanup_uncertain = true;
                facts.secondary.push(RoundSecondaryFailureRef {
                    stage: RoundFailureStage::NativeCleanup,
                    source: RoundFailureSourceRef::Cleanup(cleanup),
                });
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
                facts.cleanup_intent = crate::provider::CancellationCause::Unknown;
                facts.head = head_request.clone().or(facts.head);
                retain_unique(
                    &mut facts.inference,
                    InferenceObligation::Reconcile {
                        head: facts.head.clone(),
                    },
                );
            }
            Self::RequestPreparationRefused { head_request, .. } => {
                facts.head = Some(head_request.clone());
                facts.cleanup_intent = crate::provider::CancellationCause::ProviderFailureCleanup;
            }
            Self::RequestRejected { head_request, .. } => {
                facts.execution = RoundExecution::Rejected;
                facts.cleanup_intent = crate::provider::CancellationCause::ProviderFailureCleanup;
                facts.head = Some(head_request.clone());
            }
            Self::InterruptedModelRound { head_request, .. } => {
                facts.execution = RoundExecution::Interrupted;
                facts.cleanup_intent = crate::provider::CancellationCause::ProviderFailureCleanup;
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
                    crate::embedding::InferenceBlockReason::RequestPreparationRefused {
                        ..
                    }
                    | crate::embedding::InferenceBlockReason::ProviderStreamFailure { .. }
                    | crate::embedding::InferenceBlockReason::ResponseIncomplete { .. }
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
                    facts.cleanup_intent =
                        crate::provider::CancellationCause::ProviderFailureCleanup;
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
                    facts.cleanup_intent =
                        crate::provider::CancellationCause::ProviderFailureCleanup;
                    facts.execution = RoundExecution::Rejected
                }
                crate::transport::TransportFailureKind::ProviderFailed { .. }
                | crate::transport::TransportFailureKind::ProviderIncomplete { .. } => {
                    facts.cleanup_intent =
                        crate::provider::CancellationCause::ProviderFailureCleanup;
                }
                crate::transport::TransportFailureKind::Other => {}
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
    fn action_facts(&self) -> ErrorFacts<'_> {
        match &self.action {
            RoundAction::Completed { completion } => completion_facts(completion),
            RoundAction::Failed { head, cause } => cause.round_facts(head.clone()),
        }
    }
    pub fn assessment(&self) -> RoundAssessment {
        assess_evidence(
            self.action_facts(),
            &self.secondary,
            &self.cleanup,
            &self.inference,
            &self.retained,
            &self.head_settlements,
        )
    }
    pub fn primary_error(&self) -> Option<&EngineError> {
        self.action_facts().primary
    }
    pub fn original_completion(&self) -> Option<&super::EngineCompletion> {
        self.action_facts().completion
    }
    pub fn secondary_failures(&self) -> Vec<RoundSecondaryFailureRef<'_>> {
        with_secondary_evidence(self.action_facts(), &self.secondary).secondary
    }
}

fn completion_facts(completion: &super::EngineCompletion) -> ErrorFacts<'_> {
    let mut facts = ErrorFacts::failed(Some(completion.head_request.clone()));
    facts.execution = RoundExecution::Completed;
    facts.completion = Some(completion);
    facts
}

/// One execution owner's private receipt crosses into both standalone rendering
/// and embedded settlement. No global or separately mutable classifier exists.
#[derive(Debug)]
pub(super) struct EngineRunReceipt {
    pub(super) result: Result<super::EngineCompletion, EngineError>,
    pub(super) head: Option<RequestId>,
    pub(super) cleanup: CleanupReceipt,
    pub(super) secondary: Vec<RoundSecondaryFailure>,
    pub(super) inference: Vec<InferenceObligation>,
    pub(super) retained: Vec<crate::turn::ForeignPublicationObservation>,
    pub(super) head_settlements: Vec<crate::store::EmbeddedRoundSettlement>,
}
impl EngineRunReceipt {
    pub(super) fn cleanup_cancellation_intent(&self) -> crate::provider::CancellationCause {
        match &self.result {
            Ok(_) => crate::provider::CancellationCause::Unknown,
            Err(error) => error.round_facts(self.head.clone()).cleanup_intent,
        }
    }

    fn action_facts(&self) -> ErrorFacts<'_> {
        match &self.result {
            Ok(completion) => completion_facts(completion),
            Err(cause) => cause.round_facts(self.head.clone()),
        }
    }
    pub(super) fn primary_error(&self) -> Option<&EngineError> {
        self.action_facts().primary
    }
    pub(super) fn original_completion(&self) -> Option<&super::EngineCompletion> {
        self.action_facts().completion
    }
    pub(super) fn secondary_failures(&self) -> Vec<RoundSecondaryFailureRef<'_>> {
        with_secondary_evidence(self.action_facts(), &self.secondary).secondary
    }
    pub(super) fn assessment(&self) -> RoundAssessment {
        let facts = self.action_facts();
        assess_evidence(
            facts,
            &self.secondary,
            &self.cleanup,
            &self.inference,
            &self.retained,
            &self.head_settlements,
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
    pub(super) fn into_failure(self) -> EmbeddedRoundFailure {
        EmbeddedRoundFailure {
            action: match self.result {
                Ok(completion) => RoundAction::Completed { completion },
                Err(cause) => RoundAction::Failed {
                    head: self.head,
                    cause,
                },
            },
            secondary: self.secondary,
            cleanup: self.cleanup,
            inference: self.inference,
            retained: self.retained,
            head_settlements: self.head_settlements,
        }
    }
}

fn with_secondary_evidence<'a>(
    mut facts: ErrorFacts<'a>,
    secondary: &'a [RoundSecondaryFailure],
) -> ErrorFacts<'a> {
    for failure in secondary {
        facts.secondary.push(RoundSecondaryFailureRef {
            stage: failure.stage,
            source: match &failure.source {
                RoundFailureSource::Engine(source) => RoundFailureSourceRef::Engine(source),
                RoundFailureSource::Store(source) => RoundFailureSourceRef::Store(source),
            },
        });
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
    facts
}

fn assess_evidence<'a>(
    mut facts: ErrorFacts<'a>,
    secondary: &'a [RoundSecondaryFailure],
    cleanup_receipt: &CleanupReceipt,
    inference: &[InferenceObligation],
    retained: &[crate::turn::ForeignPublicationObservation],
    head_settlements: &[crate::store::EmbeddedRoundSettlement],
) -> RoundAssessment {
    facts = with_secondary_evidence(facts, secondary);
    for settlement in head_settlements {
        retain_unique(&mut facts.settlements, settlement.clone());
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
        secondary_stages: facts
            .secondary
            .iter()
            .map(|failure| failure.stage)
            .collect(),
    }
}

pub(super) struct OrderedProviderFailure {
    pub(super) cause: EngineError,
    pub(super) cleanup: CleanupReceipt,
    pub(super) retained: Vec<crate::turn::ForeignPublicationObservation>,
}

/// Issued after the execution owner has retained its action and durable evidence.
/// Cleanup confirmation covers this round's own custody; foreign observations
/// remain separate and do not certify global producer quiescence.
#[derive(Debug)]
pub struct EmbeddedRoundSettlement {
    receipt: EngineRunReceipt,
}
impl EmbeddedRoundSettlement {
    pub fn assessment(&self) -> RoundAssessment {
        self.receipt.assessment()
    }
    pub fn issue(&self) -> Option<&EngineError> {
        self.receipt.primary_error()
    }
    pub fn original_completion(&self) -> Option<&super::EngineCompletion> {
        self.receipt.original_completion()
    }
    pub fn secondary_failures(&self) -> Vec<RoundSecondaryFailureRef<'_>> {
        self.receipt.secondary_failures()
    }
}

#[derive(Clone, Copy)]
enum HistoryMode {
    Recent,
    Reconcile,
}

pub struct EmbeddedRounds<A: super::Auth, P: super::Provider, C: super::ResponsesTransport> {
    driver: std::sync::Arc<()>,
    generation: std::sync::Arc<()>,
    engine: super::Engine<A, P, C>,
    identity: crate::embedding::HostIdentity,
    history: HistoryMode,
}
impl<A: super::Auth, P: super::Provider + 'static, C: super::ResponsesTransport>
    super::Engine<A, P, C>
{
    pub fn into_embedded_rounds(self) -> Result<EmbeddedRounds<A, P, C>, EmbeddedRoundFailure> {
        let identity = self
            .embedded_identity()
            .filter(|identity| identity.actor == self.config.agent)
            .ok_or_else(|| {
                EmbeddedRoundFailure::refused(StoreError::InvalidEmbeddedBinding.into(), None)
            })?;
        Ok(EmbeddedRounds {
            driver: std::sync::Arc::new(()),
            generation: std::sync::Arc::new(()),
            engine: self,
            identity,
            history: HistoryMode::Reconcile,
        })
    }
}
impl<A: super::Auth, P: super::Provider + 'static, C: super::ResponsesTransport>
    EmbeddedRounds<A, P, C>
{
    pub fn next_round(&self) -> Result<EmbeddedRoundReadiness, EmbeddedRoundFailure> {
        let snapshot = self
            .engine
            .store
            .select_embedded_round(&self.identity)
            .map_err(|source| EmbeddedRoundFailure::refused(source.into(), None))?;
        let wait = match &snapshot.inference {
            crate::embedding::HostInferenceState::Blocked { block }
            | crate::embedding::HostInferenceState::AutoRetry { block, .. } => {
                Some(EmbeddedRoundWait::RecoveryRequired {
                    block: block.clone(),
                })
            }
            crate::embedding::HostInferenceState::Running => {
                let head = snapshot
                    .frontier
                    .pending_head
                    .clone()
                    .or(snapshot.frontier.settled_head.clone());
                let mut failure = EmbeddedRoundFailure::refused(
                    StoreError::InferenceRunning.into(),
                    head.clone(),
                );
                failure
                    .inference
                    .push(InferenceObligation::Reconcile { head });
                return Err(failure);
            }
            crate::embedding::HostInferenceState::Recovering { .. } => None,
            crate::embedding::HostInferenceState::Idle => {
                if snapshot.first_envelope.is_some()
                    || (snapshot.frontier.pending_head.is_some()
                        && snapshot.frontier.pending_interruption.is_none())
                {
                    None
                } else {
                    Some(match &snapshot.frontier.pending_head {
                        Some(request) => EmbeddedRoundWait::ExplicitInputRequired {
                            request: request.clone(),
                        },
                        None => EmbeddedRoundWait::NoPendingWork,
                    })
                }
            }
        };
        match wait {
            Some(wait) => Ok(EmbeddedRoundReadiness::Waiting(wait)),
            None => Ok(EmbeddedRoundReadiness::Selected(EmbeddedRoundSelection {
                driver: self.driver.clone(),
                generation: self.generation.clone(),
                identity: self.identity.clone(),
                snapshot,
            })),
        }
    }

    /// Native host admission precedes this call. An observation cannot issue an
    /// inference attempt: the existing Store transaction validates and issues it.
    pub async fn run_round(
        &mut self,
        selection: EmbeddedRoundSelection,
        cancellation: tokio::sync::watch::Receiver<bool>,
        incoming: tokio::sync::mpsc::UnboundedReceiver<crate::mailbox::DurableMailboxWake>,
    ) -> Result<EmbeddedRoundSettlement, EmbeddedRoundFailure> {
        let epoch = std::mem::replace(&mut self.generation, std::sync::Arc::new(()));
        let previous = selection.snapshot.frontier.settled_head.clone();
        if selection.identity != self.identity
            || !std::sync::Arc::ptr_eq(&selection.driver, &self.driver)
            || !std::sync::Arc::ptr_eq(&selection.generation, &epoch)
        {
            return Err(EmbeddedRoundFailure::refused(
                StoreError::InvalidEmbeddedFrontier.into(),
                previous,
            ));
        }
        let recovering = matches!(self.history, HistoryMode::Reconcile)
            || selection.snapshot.frontier.pending_head.is_some();
        let receipt = self
            .engine
            .run_with_selected_receipt(
                previous.clone(),
                Vec::new(),
                cancellation,
                incoming,
                None,
                recovering,
                Some(&selection.snapshot),
            )
            .await
            .map_err(|cause| EmbeddedRoundFailure::refused(cause, previous.clone()))?;
        self.finish(previous, receipt)
    }

    fn head_committed(
        &self,
        assessment: &RoundAssessment,
        previous: Option<&RequestId>,
        outcome: crate::store::EmbeddedRoundOutcome,
    ) -> bool {
        assessment.head_settlements().iter().any(|settlement| {
            self.engine
                .store
                .matches_embedded_settlement_issuer(&self.identity, settlement)
                && settlement.previous() == previous
                && Some(settlement.request()) == assessment.head()
                && settlement.outcome() == outcome
        })
    }

    pub(super) fn finish(
        &mut self,
        previous: Option<RequestId>,
        mut receipt: EngineRunReceipt,
    ) -> Result<EmbeddedRoundSettlement, EmbeddedRoundFailure> {
        let assessment = receipt.assessment();
        match assessment.execution() {
            RoundExecution::Completed | RoundExecution::Cancelled => {
                let outcome = if assessment.execution() == RoundExecution::Completed {
                    crate::store::EmbeddedRoundOutcome::Completed
                } else {
                    crate::store::EmbeddedRoundOutcome::Cancelled
                };
                let recording = match assessment.head() {
                    Some(request) => self.engine.store.settle_embedded_round_receipt(
                        &self.identity,
                        previous.as_ref(),
                        request,
                        outcome,
                    ),
                    None => Err(StoreError::InvalidEmbeddedFrontier),
                };
                match recording {
                    Ok(crate::store::EmbeddedRoundSettlementResult::Committed(settlement)) => {
                        receipt.head_settlements.push(settlement)
                    }
                    Ok(
                        crate::store::EmbeddedRoundSettlementResult::AlreadySettled
                        | crate::store::EmbeddedRoundSettlementResult::StaleFrontier,
                    ) => receipt.add_failure(
                        RoundFailureStage::DurableHeadSettlement,
                        StoreError::InvalidEmbeddedFrontier.into(),
                    ),
                    Err(source) => {
                        receipt.add_failure(RoundFailureStage::DurableHeadSettlement, source.into())
                    }
                }
            }
            RoundExecution::Rejected => {
                let proved = self.head_committed(
                    &assessment,
                    previous.as_ref(),
                    crate::store::EmbeddedRoundOutcome::Rejected,
                );
                // A failed recording already carries its actual typed stage. A
                // clean rejection must have the same transaction's head receipt.
                if !proved && assessment.secondary_failure_stages().is_empty() {
                    receipt.add_failure(
                        RoundFailureStage::DurableHeadSettlement,
                        StoreError::InvalidEmbeddedFrontier.into(),
                    );
                }
            }
            RoundExecution::Interrupted | RoundExecution::Failed => {}
        }
        let assessment = receipt.assessment();
        self.history = if assessment.execution() == RoundExecution::Completed
            && assessment.inference_obligations().is_empty()
            && assessment.secondary_failure_stages().is_empty()
        {
            HistoryMode::Recent
        } else {
            HistoryMode::Reconcile
        };
        let outcome = match assessment.execution() {
            RoundExecution::Completed => Some(crate::store::EmbeddedRoundOutcome::Completed),
            RoundExecution::Cancelled => Some(crate::store::EmbeddedRoundOutcome::Cancelled),
            RoundExecution::Rejected => Some(crate::store::EmbeddedRoundOutcome::Rejected),
            RoundExecution::Interrupted | RoundExecution::Failed => None,
        };
        let durable = match outcome {
            Some(outcome) => self.head_committed(&assessment, previous.as_ref(), outcome),
            None => assessment.inference_obligations().iter().any(|obligation|
                matches!(obligation, InferenceObligation::RecoverBlock { block } if Some(&block.request) == assessment.head())),
        };
        if durable
            && assessment.store_continuation() == StoreContinuation::NoReopenRequired
            && matches!(
                assessment.cleanup(),
                RoundCleanup::Confirmed | RoundCleanup::ReleasedWithFollowup
            )
            && assessment.secondary_failure_stages().is_empty()
        {
            Ok(EmbeddedRoundSettlement { receipt })
        } else {
            Err(receipt.into_failure())
        }
    }
}

impl EmbeddedRoundFailure {
    fn refused(cause: EngineError, head: Option<RequestId>) -> Self {
        Self {
            action: RoundAction::Failed { head, cause },
            secondary: Vec::new(),
            cleanup: CleanupReceipt::Unobserved,
            inference: Vec::new(),
            retained: Vec::new(),
            head_settlements: Vec::new(),
        }
    }
}
impl std::fmt::Display for RoundFailureSourceRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Engine(source) => std::fmt::Display::fmt(source, f),
            Self::Store(source) => std::fmt::Display::fmt(source, f),
            Self::Cleanup(source) => std::fmt::Display::fmt(source, f),
        }
    }
}
impl std::fmt::Display for EmbeddedRoundFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "embedded round {:?}", self.assessment().execution())?;
        if let Some(primary) = self.primary_error() {
            write!(f, ": {primary}")?;
        }
        for secondary in self.secondary_failures() {
            write!(f, "; {:?}: {}", secondary.stage(), secondary.source())?;
        }
        Ok(())
    }
}
impl std::error::Error for EmbeddedRoundFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.primary_error().map(|source| source as _)
    }
}
