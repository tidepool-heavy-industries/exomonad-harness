//! Single-agent, stateless Responses request loop.
//!
//! Each model request receives the full accumulated item history. Function
//! calls are started immediately and their settled outputs are appended under
//! their original call ids before the next request.

mod output;
pub use output::{ModelOutput, ModelOutputObserver, ModelOutputUpdate};

use crate::{
    compaction::{
        CompactContext, CompactError, Compactor, PlainText, Server, ServerCompactFuture, ToolName,
        TypedTurnFuture,
    },
    finalize::{FINALIZE_TOOL_NAME, FinalizeError, FinalizeParser},
    item::{Item, ItemHash, ToolInput, ToolKind},
    mailbox::{DurableMailboxWake, Envelope, MailboxSignal, MessageChannel},
    model::{AgentPath, CallId, ConversationIdentity, Effort, OperationId, RequestId},
    provider::Provider,
    store::{Store, StoreError, Usage as StoredUsage},
    transport::{
        Auth, ResponsesClient, ResponsesRequest, ResponsesTurn, TransportError, Usage,
        sse::{OutputChannel, StreamEvent},
    },
    turn::{
        JobError, JobScheduler, WaitAgentResultExact, WaitResumeExact, outputs_in_operation_order,
        wait_agent_and_drain_exact,
    },
};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::{collections::HashSet, num::NonZeroU64, sync::Arc};
use thiserror::Error;
use tokio::sync::watch;

#[path = "engine/items.rs"]
mod items;

struct HistoryWindow {
    items: Vec<Item>,
    provenance: Vec<(RequestId, ItemHash)>,
}

#[derive(Clone, Debug)]
struct PendingCall {
    operation: OperationId,
    call_id: CallId,
    claim_request: RequestId,
    is_wait_agent: bool,
    tool_kind: ToolKind,
    persist_here_invocation_output: bool,
    cancel_job_on_cleanup: bool,
}

enum DispatchResult {
    NotCall,
    Pending(PendingCall),
    Settled {
        operation: OperationId,
        continuation: Option<crate::store::RecordedWaitContinuation>,
        barrier: crate::provider::WaitReplayBarrier,
        commit: Option<crate::replay::ReplayWaitCommit>,
    },
}

enum ReplayBarrierStage {
    BeforeWait,
    AfterWait,
}

#[cfg(test)]
#[path = "engine/recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
#[path = "engine/completion_tests.rs"]
mod completion_tests;

#[cfg(test)]
mod embedded_restart_tests;
#[cfg(test)]
#[path = "engine/rejection_tests.rs"]
mod rejection_tests;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// A provider refused this exact request before starting a response, and
    /// all outstanding call cleanup and failure persistence succeeded.
    #[error("provider rejected request {head_request:?}: {error}")]
    RequestRejected {
        head_request: RequestId,
        error: TransportError,
    },
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Job(#[from] JobError),
    #[error("cancellation of operation {operation:?} remains unconfirmed")]
    UnconfirmedCancellation { operation: OperationId },
    #[error("rejected request lost its embedded agent head fence")]
    RejectedHeadMismatch,
    #[error("blocking store task failed")]
    StoreTask,
    #[error("engine cancelled")]
    Cancelled { head_request: Option<RequestId> },
    #[error("Responses turn did not contain a terminal assistant answer")]
    MissingFinal,
    #[error(transparent)]
    Finalize(#[from] FinalizeError),
    #[error("typed completion requires exactly one finalize call")]
    InvalidFinalizeCount,
    #[error("invalid restricted tool selection: {0}")]
    InvalidToolSelection(String),
    #[error("invalid before-request injected item")]
    InvalidInjectedItem,
    #[error("call refused by issuing provider: {0}")]
    ProviderCall(#[from] crate::provider::ProviderError),
    #[error("malformed Responses tool call item")]
    InvalidFunctionCall,
    #[error("durable output kind or call id does not match invocation {0}")]
    MismatchedToolOutput(String),
    #[error("request history has no harness-authored configuration_update")]
    MissingEffortPin,
    #[error("a model response contained more than one wait_agent call")]
    MultipleWaitAgents,
    #[error(transparent)]
    Compact(#[from] CompactError),
    #[error("cannot resume a forked wait_agent call before its parent settles it")]
    UnresumableForkedWaitAgent,
    #[error("cannot recover unclaimed tool call {call:?} in request {request:?}")]
    UnclaimedInheritedCall { request: RequestId, call: CallId },
    #[error("recorded provider response is not fully present in request {0:?}")]
    IncompleteRecordedResponse(RequestId),
    #[error("inherited settled call {0} has no durable output item")]
    MissingInheritedOutput(String),
    #[error("claim {0} changed during missing-job recovery and could not be reconciled")]
    ClaimRecoveryConflict(String),
    #[error("Here child has no durable snapshot request")]
    MissingHereSnapshot,
    #[error(
        "engine operation failed: {primary}; additionally failed to clean up outstanding calls: {cleanup}"
    )]
    Cleanup {
        primary: Box<EngineError>,
        cleanup: String,
    },
}

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub instructions: String,
    pub tools: Vec<serde_json::Value>,
    pub model: String,
    pub effort: Effort,
    /// Stable key shared for the conversation's request cache.
    pub session_id: String,
    /// Store branch and job claimant identity.
    pub agent: AgentPath,
}

/// Successful engine result, including the durable transcript through the
/// final model response.
#[derive(Clone, Debug)]
pub struct EngineCompletion {
    pub turn: ResponsesTurn,
    pub transcript: Vec<Item>,
    /// Durable request row whose history was supplied to the final response.
    pub head_request: RequestId,
}

/// Narrow transport seam: production uses `ResponsesClient`; tests can replay
/// recorded turns without credentials or live network access.
#[async_trait::async_trait]
pub trait ResponsesTransport: Send + Sync {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError>;

    async fn create_streaming(
        &self,
        request: ResponsesRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        let turn = self.create(request).await?;
        for item in &turn.items {
            let _ = sink.send(StreamEvent::ItemDone(item.clone())).await;
        }
        Ok(turn)
    }
    /// The local durable request is out-of-band and never enters provider wire JSON.
    async fn create_streaming_for_request(
        &self,
        _request_id: &RequestId,
        request: ResponsesRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        self.create_streaming(request, sink).await
    }
}

#[async_trait::async_trait]
impl<A: Auth + Clone + 'static> ResponsesTransport for ResponsesClient<A> {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        ResponsesClient::create(self, request).await
    }

    async fn create_streaming(
        &self,
        request: ResponsesRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        ResponsesClient::create_streaming(self, request, sink).await
    }
}

/// Owns one agent's model loop; clones share durable store and scheduler state.
pub struct Engine<A: Auth, P: Provider, C: ResponsesTransport = ResponsesClient<A>> {
    client: C,
    store: Arc<Store>,
    scheduler: Arc<JobScheduler>,
    provider: Arc<P>,
    config: EngineConfig,
    origin: ConversationIdentity,
    compact_at_input_tokens: Option<u64>,
    bounded_invocation: bool,
    output_observer: Option<Arc<dyn ModelOutputObserver>>,
    compaction_strategy: CompactionStrategy,
    _auth: std::marker::PhantomData<fn() -> A>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompactionStrategy {
    Server,
    PlainText,
}

impl<A: Auth + Clone + 'static, P: Provider + 'static> Engine<A, P, ResponsesClient<A>> {
    pub fn new(
        auth: A,
        store: Arc<Store>,
        scheduler: Arc<JobScheduler>,
        provider: Arc<P>,
        config: EngineConfig,
    ) -> Self {
        Self::with_transport(
            ResponsesClient::new(auth),
            store,
            scheduler,
            provider,
            config,
        )
    }
}

fn parsed_call_id(item: &Item) -> Result<Option<CallId>, EngineError> {
    item.tool_call()
        .map(|call| call.map(|call| call.call_id))
        .map_err(|_| EngineError::InvalidFunctionCall)
}

fn conflicting_call(seen: &[(CallId, Item)], call_id: &CallId, item: &Item) -> bool {
    seen.iter()
        .find(|(seen_id, _)| seen_id == call_id)
        .is_some_and(|(_, previous)| previous.tool_call() != item.tool_call())
}

fn validate_tool_output(call_id: &CallId, kind: ToolKind, item: &Item) -> Result<(), EngineError> {
    let expected = match kind {
        ToolKind::Function => "function_call_output",
        ToolKind::Custom => "custom_tool_call_output",
    };
    if item.0["type"] == expected && item.0["call_id"].as_str() == Some(&call_id.0) {
        Ok(())
    } else {
        Err(EngineError::MismatchedToolOutput(call_id.0.clone()))
    }
}

impl<A: Auth, P: Provider + 'static, C: ResponsesTransport> Engine<A, P, C> {
    /// Alternate transport constructor, primarily for deterministic replay.
    pub fn with_transport(
        client: C,
        store: Arc<Store>,
        scheduler: Arc<JobScheduler>,
        provider: Arc<P>,
        config: EngineConfig,
    ) -> Self {
        let origin = store.standalone_identity(config.agent.clone());
        Self {
            client,
            store,
            scheduler,
            provider,
            config,
            origin,
            compact_at_input_tokens: None,
            bounded_invocation: false,
            output_observer: None,
            compaction_strategy: CompactionStrategy::Server,
            _auth: std::marker::PhantomData,
        }
    }
    pub fn with_output_observer(mut self, observer: Arc<dyn ModelOutputObserver>) -> Self {
        self.output_observer = Some(observer);
        self
    }
    fn observe_delta(
        &self,
        request: &RequestId,
        item_id: String,
        channel: OutputChannel,
        index: u64,
        text: String,
    ) {
        if let Some(observer) = &self.output_observer {
            observer.observe(ModelOutput {
                origin: self.origin.clone(),
                request_id: request.clone(),
                update: ModelOutputUpdate::Delta {
                    item_id,
                    channel,
                    index,
                    text,
                },
            });
        }
    }
    pub(crate) fn with_origin(mut self, origin: ConversationIdentity) -> Self {
        self.origin = origin;
        self
    }

    fn embedded_identity(&self) -> Option<crate::embedding::HostIdentity> {
        match &self.origin {
            ConversationIdentity::Embedded {
                run,
                actor,
                incarnation,
            } => Some(crate::embedding::HostIdentity {
                run: run.clone(),
                actor: actor.clone(),
                incarnation: incarnation.clone(),
            }),
            ConversationIdentity::Standalone { .. } => None,
        }
    }

    /// Private program-driven profile: no mailbox, sequential cooperative callbacks.
    pub(crate) fn bounded_invocation(mut self) -> Self {
        self.bounded_invocation = true;
        self
    }

    pub(crate) async fn run_invocation(
        &self,
        initial: Vec<Item>,
        result_schema: Option<serde_json::Value>,
    ) -> Result<EngineCompletion, EngineError> {
        let schema = result_schema
            .map(crate::finalize::tool_schema_from_result_schema)
            .transpose()?;
        let (_cancel, cancellation) = watch::channel(false);
        let (_incoming, envelopes) = tokio::sync::mpsc::unbounded_channel();
        self.run_loop(
            None,
            initial,
            cancellation,
            envelopes,
            false,
            schema.as_ref(),
            false,
        )
        .await
    }

    /// Opt into explicit server compaction after a turn whose input usage
    /// reaches this threshold. No automatic API compaction is enabled.
    pub fn with_compaction_threshold(mut self, input_tokens: u64) -> Self {
        self.compact_at_input_tokens = (input_tokens > 0).then_some(input_tokens);
        self.compaction_strategy = CompactionStrategy::Server;
        self
    }

    /// Opt into plain-text handoffs near half the configured context capacity.
    pub fn with_plain_text_compaction(mut self, context_capacity_tokens: NonZeroU64) -> Self {
        self.compact_at_input_tokens = Some(context_capacity_tokens.get().div_ceil(2));
        self.compaction_strategy = CompactionStrategy::PlainText;
        self
    }

    /// Run from a durable request head, appending only new items and admitting
    /// persisted mailbox envelopes before each model request. `incoming`
    /// carries standalone direct envelopes as content.
    pub async fn run(
        &self,
        head: Option<RequestId>,
        new_items: Vec<Item>,
        cancellation: watch::Receiver<bool>,
        incoming: tokio::sync::mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<EngineCompletion, EngineError> {
        self.run_with_finalize(head, new_items, cancellation, incoming, None)
            .await
    }

    /// Run an embedded conversation from Store-backed input. Hints carry only
    /// committed envelope IDs; Store remains the sole content and delivery owner.
    pub async fn run_embedded(
        &self,
        head: Option<RequestId>,
        new_items: Vec<Item>,
        cancellation: watch::Receiver<bool>,
        incoming: tokio::sync::mpsc::UnboundedReceiver<DurableMailboxWake>,
    ) -> Result<EngineCompletion, EngineError> {
        self.run_with_finalize_mode(head, new_items, cancellation, incoming, None, false)
            .await
    }

    /// Resume an embedded conversation without replaying uncertain jobs.
    pub async fn run_recovering_embedded(
        &self,
        head: Option<RequestId>,
        new_items: Vec<Item>,
        cancellation: watch::Receiver<bool>,
        incoming: tokio::sync::mpsc::UnboundedReceiver<DurableMailboxWake>,
    ) -> Result<EngineCompletion, EngineError> {
        self.run_with_finalize_mode(head, new_items, cancellation, incoming, None, true)
            .await
    }

    /// Recover after process restart from a durable request head.
    ///
    /// Unlike `run`, this opts into reconciling outstanding claims on the
    /// contiguous same-agent-branch ancestry of `head`. Missing in-memory jobs
    /// are durably interrupted unless settlement already won; provider calls
    /// are never replayed. Ordinary runs, especially Here-fork starts, retain
    /// direct `claims_on` behavior.
    pub async fn run_recovering(
        &self,
        head: Option<RequestId>,
        new_items: Vec<Item>,
        cancellation: watch::Receiver<bool>,
        incoming: tokio::sync::mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<EngineCompletion, EngineError> {
        self.run_with_finalize_mode(head, new_items, cancellation, incoming, None, true)
            .await
    }

    /// Require a strict typed `finalize` call instead of an assistant final
    /// message. The call is persisted as an item but never scheduled as a Job.
    pub async fn run_finalized<T: JsonSchema + DeserializeOwned>(
        &self,
        head: Option<RequestId>,
        new_items: Vec<Item>,
        cancellation: watch::Receiver<bool>,
        incoming: tokio::sync::mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<(EngineCompletion, T), EngineError> {
        let schema = crate::finalize::tool_schema::<T>()?;
        let completion = self
            .run_with_finalize(head, new_items, cancellation, incoming, Some(schema))
            .await?;
        let calls: Vec<_> = completion
            .turn
            .items
            .iter()
            .filter(|item| is_finalize_call(item))
            .collect();
        if calls.len() != 1 {
            return Err(EngineError::InvalidFinalizeCount);
        }
        let reply = FinalizeParser::new().parse_completed(calls[0])?;
        Ok((completion, reply))
    }

    /// Run an agent whose stored contract supplies a strict result schema.
    /// `result_schema` describes the value inside finalize's `result` field.
    /// Unlike `run_finalized<T>`, the result remains in `completion.turn` for
    /// the host to parse and route using its contract.
    pub async fn run_with_reply_schema(
        &self,
        head: Option<RequestId>,
        new_items: Vec<Item>,
        cancellation: watch::Receiver<bool>,
        incoming: tokio::sync::mpsc::UnboundedReceiver<Envelope>,
        result_schema: serde_json::Value,
    ) -> Result<EngineCompletion, EngineError> {
        let schema = crate::finalize::tool_schema_from_result_schema(result_schema)?;
        self.run_with_finalize(head, new_items, cancellation, incoming, Some(schema))
            .await
    }

    async fn run_with_finalize(
        &self,
        head: Option<RequestId>,
        new_items: Vec<Item>,
        cancellation: watch::Receiver<bool>,
        incoming: tokio::sync::mpsc::UnboundedReceiver<Envelope>,
        finalize_schema: Option<serde_json::Value>,
    ) -> Result<EngineCompletion, EngineError> {
        self.run_with_finalize_mode(
            head,
            new_items,
            cancellation,
            incoming,
            finalize_schema,
            false,
        )
        .await
    }

    async fn run_with_finalize_mode<I: Into<MailboxSignal> + Send + 'static>(
        &self,
        head: Option<RequestId>,
        new_items: Vec<Item>,
        cancellation: watch::Receiver<bool>,
        mut incoming: tokio::sync::mpsc::UnboundedReceiver<I>,
        finalize_schema: Option<serde_json::Value>,
        recovering: bool,
    ) -> Result<EngineCompletion, EngineError> {
        let (envelope_tx, envelopes) = tokio::sync::mpsc::unbounded_channel();
        let keepalive = envelope_tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(envelope) = incoming.recv().await {
                if envelope_tx.send(envelope.into()).is_err() {
                    break;
                }
            }
        });
        let result = self
            .run_loop(
                head,
                new_items,
                cancellation,
                envelopes,
                true,
                finalize_schema.as_ref(),
                recovering,
            )
            .await;
        drop(keepalive);
        forwarder.abort();
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_loop(
        &self,
        head: Option<RequestId>,
        initial: Vec<Item>,
        mut cancellation: watch::Receiver<bool>,
        mut envelopes: tokio::sync::mpsc::UnboundedReceiver<MailboxSignal>,
        admit_inbox: bool,
        finalize_schema: Option<&serde_json::Value>,
        recovering: bool,
    ) -> Result<EngineCompletion, EngineError> {
        let settled_head = head.clone();
        let identity = self.embedded_identity();
        let pending_head = if let Some(identity) = &identity {
            let frontier = self.store.embedded_round_frontier(identity)?;
            if frontier.settled_head != head || (!recovering && frontier.pending_head.is_some()) {
                return Err(StoreError::InvalidEmbeddedFrontier.into());
            }
            frontier.pending_head
        } else {
            None
        };
        let resuming = pending_head.is_some();
        let head = pending_head.or(head);
        let recorded_response = if resuming {
            self.store
                .recorded_response(head.as_ref().expect("pending request"))?
        } else {
            None
        };
        if let Some(response) = &recorded_response {
            let request = head.as_ref().expect("pending request");
            let local = self.store.items(request)?;
            let mut expected = response.items.iter();
            let mut next = expected.next();
            for item in &local {
                if next == Some(item) {
                    next = expected.next();
                }
            }
            if next.is_some() {
                return Err(EngineError::IncompleteRecordedResponse(request.clone()));
            }
        }
        self.await_here_invocation_output(&head, &mut cancellation)
            .await?;
        let inherited_pairs = match &head {
            Some(head) => self.read_history_pairs(head).await?,
            None => Vec::new(),
        };
        let inherited_history = inherited_pairs
            .iter()
            .map(|(_, item)| item.clone())
            .collect::<Vec<_>>();
        let inherited_claims = match &head {
            Some(head) => {
                let store = self.store.clone();
                let request = head.clone();
                let branch = self.config.agent.0.clone();
                blocking(move || {
                    if recovering {
                        store.claims_on_branch_lineage(&request, &branch)
                    } else {
                        store.claims_on(&request)
                    }
                })
                .await?
            }
            None => Vec::new(),
        };
        if resuming {
            for (request, item) in &inherited_pairs {
                let Some(call) = item
                    .tool_call()
                    .map_err(|_| EngineError::InvalidFunctionCall)?
                else {
                    continue;
                };
                if self
                    .store
                    .request(request)?
                    .is_some_and(|row| row.branch != self.config.agent.0)
                {
                    continue;
                }
                if !inherited_claims.iter().any(|claim| {
                    claim.call_id == call.call_id
                        && (&claim.request == request || &claim.operation.request == request)
                }) {
                    return Err(EngineError::UnclaimedInheritedCall {
                        request: request.clone(),
                        call: call.call_id,
                    });
                }
            }
        }
        let mut pending = Vec::<PendingCall>::new();
        let mut replay_items = Vec::<Item>::new();
        let mut replay_outputs =
            Vec::<(OperationId, crate::turn::JobOutput, ToolKind, RequestId)>::new();
        let mut attachable = Vec::new();
        for claim in inherited_claims {
            let tool_kind = self
                .store
                .tool_invocation_kind(&claim.operation.request, &claim.call_id)?
                .ok_or_else(|| EngineError::MissingInheritedOutput(claim.call_id.0.clone()))?;
            let same_call = |item: &Item| {
                item.tool_call()
                    .ok()
                    .flatten()
                    .is_some_and(|call| call.call_id == claim.call_id)
            };
            let claimant_call_count = inherited_pairs
                .iter()
                .filter(|(request, item)| request == &claim.request && same_call(item))
                .count();
            if claimant_call_count > 1 && claim.state != crate::store::ClaimState::Pending {
                return Err(EngineError::MismatchedToolOutput(claim.call_id.0.clone()));
            }
            let call_position = inherited_pairs
                .iter()
                .enumerate()
                .find(|(_, (request, item))| request == &claim.request && same_call(item))
                .map(|(position, _)| position)
                .or_else(|| {
                    inherited_pairs
                        .iter()
                        .enumerate()
                        .find(|(_, (request, item))| {
                            request == &claim.operation.request && same_call(item)
                        })
                        .map(|(position, _)| position)
                })
                .or_else(|| inherited_history.iter().rposition(&same_call))
                .ok_or_else(|| EngineError::MissingInheritedOutput(claim.call_id.0.clone()))?;
            let next_call = inherited_history[call_position + 1..]
                .iter()
                .position(&same_call)
                .map_or(inherited_history.len(), |relative| {
                    call_position + 1 + relative
                });
            let already_output = inherited_history[call_position + 1..next_call]
                .iter()
                .filter(|item| {
                    (item.0["type"] == "function_call_output"
                        || item.0["type"] == "custom_tool_call_output")
                        && item.0["call_id"].as_str() == Some(&claim.call_id.0)
                })
                .collect::<Vec<_>>();
            for item in &already_output {
                validate_tool_output(&claim.call_id, tool_kind, item)?;
            }
            if !already_output.is_empty() {
                if already_output.len() != 1 {
                    return Err(EngineError::MismatchedToolOutput(claim.call_id.0.clone()));
                }
                let expected = match claim.state {
                    crate::store::ClaimState::Pending => {
                        return Err(EngineError::ClaimRecoveryConflict(claim.call_id.0.clone()));
                    }
                    crate::store::ClaimState::Interrupted => Item::tool_output(
                        &claim.call_id,
                        tool_kind,
                        &crate::turn::JobOutput::Interrupted,
                    ),
                    crate::store::ClaimState::Settled => {
                        self.read_settled_output(&claim.operation, &claim.request)
                            .await?
                    }
                };
                if already_output[0] != &expected {
                    return Err(EngineError::MismatchedToolOutput(claim.call_id.0.clone()));
                }
                if claim.state == crate::store::ClaimState::Settled {
                    self.acknowledge_output(&claim.operation).await?;
                }
                continue;
            }
            if claim.state == crate::store::ClaimState::Settled {
                let item = self
                    .read_settled_output(&claim.operation, &claim.request)
                    .await?;
                self.acknowledge_output(&claim.operation).await?;
                replay_items.push(item);
                continue;
            }
            if claim.state == crate::store::ClaimState::Interrupted {
                replay_items.push(Item::tool_output(
                    &claim.call_id,
                    tool_kind,
                    &crate::turn::JobOutput::Interrupted,
                ));
                continue;
            }
            if inherited_history[call_position..next_call]
                .iter()
                .any(|item| {
                    items::function_call(item).is_some_and(|(call, name, _)| {
                        call == claim.call_id && name == "wait_agent"
                    })
                })
            {
                return Err(EngineError::UnresumableForkedWaitAgent);
            }
            // Validate every pending job before claiming any of them. The
            // scheduler retains jobs for the lifetime of this shared runtime.
            match self.scheduler.output(&claim.operation).await {
                Ok(_) => {}
                // A pending durable claim with no in-memory job can only be
                // resumed after process loss by replaying the external call,
                // which is unsafe: it may already have had side effects.
                // Persist an interruption before appending its synthetic output;
                // if settlement won the race, recover that durable output instead.
                Err(JobError::UnknownCall) => {
                    replay_items.push(self.recover_missing_job(&claim).await?);
                    continue;
                }
                Err(error) => return Err(EngineError::Job(error)),
            }
            attachable.push((claim, tool_kind));
        }
        for (claim, tool_kind) in attachable {
            match self
                .scheduler
                .fork_claim_exact(&claim.operation, self.origin.clone(), true)
                .await?
            {
                Some(output) => {
                    replay_outputs.push((claim.operation, output, tool_kind, claim.request))
                }
                None => pending.push(PendingCall {
                    operation: claim.operation,
                    call_id: claim.call_id,
                    claim_request: claim.request,
                    is_wait_agent: false,
                    tool_kind,
                    persist_here_invocation_output: false,
                    cancel_job_on_cleanup: false,
                }),
            }
        }
        if let Some(turn) = &recorded_response {
            if is_final(turn, finalize_schema.is_some())
                && initial.is_empty()
                && pending.is_empty()
                && replay_items.is_empty()
                && replay_outputs.is_empty()
            {
                return Ok(EngineCompletion {
                    turn: turn.clone(),
                    transcript: inherited_history,
                    head_request: head.expect("pending request"),
                });
            }
        }
        let id = RequestId(uuid::Uuid::new_v4().to_string());
        let store = self.store.clone();
        let request = id.clone();
        let branch = self.config.agent.0.clone();
        let parent = head.clone();
        let identity_for_write = identity.clone();
        blocking(move || match identity_for_write {
            Some(identity) => store.write_embedded_request(
                &identity,
                &request,
                parent.as_ref(),
                &initial,
                StoredUsage::default(),
            ),
            None => store.write_request(
                &request,
                parent.as_ref(),
                &branch,
                &initial,
                StoredUsage::default(),
            ),
        })
        .await?;
        if !inherited_history.iter().any(Item::is_configuration_update) {
            let store = self.store.clone();
            let request = id.clone();
            let initial_effort = self.config.effort;
            blocking(move || store.set_effort(&request, initial_effort)).await?;
        }
        {
            let store = self.store.clone();
            let request = id.clone();
            let agent = self.config.agent.clone();
            blocking(move || store.apply_pending_effort(&agent, &request)).await?;
        }
        for (operation, output, kind, claim_request) in replay_outputs {
            self.persist_output(&operation, kind, &output, &id, &claim_request)
                .await?;
        }
        if !replay_items.is_empty() {
            self.append(&id, replay_items).await?;
        }
        if admit_inbox {
            let store = self.store.clone();
            let recipient = self.config.agent.clone();
            let request = id.clone();
            blocking(move || {
                store
                    .append_unread_envelopes(&recipient, &request)
                    .map(|_| ())
            })
            .await?;
        }
        let mut parent = id.clone();
        let mut compact_due = false;
        let mut previous_usage = Usage::default();
        let mut last_text_compaction_attempt_bytes = None;
        let mut request_tools: Option<(
            crate::transport::ToolManifest,
            crate::transport::ToolManifest,
        )> = None;
        let store = self.store.clone();
        let instructions = self.config.instructions.clone();
        let replay_instructions =
            match blocking(move || store.intern_replay_instructions(&instructions)).await {
                Ok(hash) => hash,
                Err(error) => return Err(self.cleanup_pending(error, &pending).await),
            };
        loop {
            if *cancellation.borrow() {
                return Err(self
                    .cleanup_pending(
                        EngineError::Cancelled {
                            head_request: Some(parent.clone()),
                        },
                        &pending,
                    )
                    .await);
            }
            // AtBoundary is a property of *every* model request, not of a
            // completed turn. Admission is a Store transaction and neither
            // polls nor cancels an in-flight provider Job.
            if admit_inbox {
                if let Err(error) = self.append_unread_envelopes(&parent).await {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            }
            let history = match load_history_window(self.store.clone(), parent.clone()).await {
                Ok(history) => history,
                Err(error) => return Err(self.cleanup_pending(error, &pending).await),
            };
            let history_bytes = (compact_due
                && self.compaction_strategy == CompactionStrategy::PlainText)
                .then(|| {
                    serde_json::to_vec(&history.items)
                        .expect("Item serialization is infallible")
                        .len()
                });
            let did_compact = compact_due
                && (self.compaction_strategy == CompactionStrategy::Server
                    || last_text_compaction_attempt_bytes.is_none_or(|previous: usize| {
                        history_bytes
                            .is_some_and(|bytes| bytes >= previous.saturating_add(previous / 4))
                    }));
            if did_compact {
                let compact =
                    self.compact_window(&parent, &history.items, &pending, &previous_usage);
                let successor = match tokio::select! {
                    result = compact => result,
                    changed = cancellation.changed() => {
                        if changed.is_err() || *cancellation.borrow() {
                            Err(EngineError::Cancelled { head_request: Some(parent.clone()) })
                        } else {
                            continue;
                        }
                    }
                } {
                    Ok(request) => request,
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                };
                if self.compaction_strategy == CompactionStrategy::PlainText {
                    last_text_compaction_attempt_bytes = history_bytes;
                }
                if let Some(request) = successor {
                    parent = request;
                }
            }
            if did_compact && admit_inbox {
                if let Err(error) = self.append_unread_envelopes(&parent).await {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            }
            let history = if did_compact {
                match load_history_window(self.store.clone(), parent.clone()).await {
                    Ok(history) => history,
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                }
            } else {
                history
            };
            let pinned_effort = history
                .items
                .iter()
                .find_map(Item::configuration_effort)
                .ok_or(EngineError::MissingEffortPin)?;
            let snapshot = match self.provider.request_snapshot() {
                Ok(snapshot) => snapshot,
                Err(error) => return Err(self.cleanup_pending(error.into(), &pending).await),
            };
            let request_provider: Arc<dyn Provider> =
                snapshot.unwrap_or_else(|| self.provider.clone());
            let provider_tools = request_provider.tool_manifest();
            if request_tools
                .as_ref()
                .is_none_or(|(previous, _)| previous != &provider_tools)
            {
                let tools = self.compose_tools(finalize_schema, &provider_tools);
                request_tools = Some((provider_tools, tools));
            }
            let mut input_hashes = Vec::with_capacity(history.items.len());
            let model_input = history
                .items
                .into_iter()
                .zip(history.provenance)
                .map(|(item, (request, hash))| {
                    if self.bounded_invocation {
                        if let Some(projected) =
                            request_provider.model_visible_item(&request, &item)
                        {
                            input_hashes.push(None);
                            return projected;
                        }
                    }
                    input_hashes.push(Some(hash));
                    item
                })
                .collect();
            let mut req = ResponsesRequest {
                input: model_input,
                instructions: self.config.instructions.clone(),
                tools: request_tools
                    .as_ref()
                    .expect("request tools installed")
                    .1
                    .clone(),
                tools_allowed: None,
                model: self.config.model.clone(),
                // The request-level field is only the cache-preserving mirror
                // of the first positional update in the exact history sent.
                pinned_effort,
                session_id: self.config.session_id.clone(),
            };
            if let Some(version) = request_provider.tool_surface_version() {
                let store = self.store.clone();
                let request = parent.clone();
                let evidence = json!({"version":version});
                if let Err(error) =
                    blocking(move || store.record_event(Some(&request), "tool_surface", &evidence))
                        .await
                {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            }
            let plan = crate::hooks::RequestPlanView {
                items: &req.input,
                tools_allowed: &req.tools,
                effort: req.pinned_effort,
            };
            let started = std::time::Instant::now();
            let before_request = tokio::select! {
                result = request_provider.before_request(&plan) => result,
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow() {
                        return Err(self.cleanup_pending(EngineError::Cancelled { head_request: Some(parent.clone()) }, &pending).await);
                    }
                    continue;
                }
            };
            let event_refs = plan
                .items
                .iter()
                .zip(&input_hashes)
                .map(|(item, hash)| {
                    hash.as_ref().map(|hash| hash.0.clone()).unwrap_or_else(|| {
                        blake3::hash(
                            &serde_json::to_vec(item).expect("Item serialization is infallible"),
                        )
                        .to_hex()
                        .to_string()
                    })
                })
                .collect();
            let selected_tools = match &before_request.decision {
                crate::hooks::BeforeRequestDecision::Send => None,
                crate::hooks::BeforeRequestDecision::SendRestricted { tools_allowed } => {
                    let mut seen = std::collections::HashSet::new();
                    let available = req
                        .tools
                        .iter()
                        .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
                        .collect::<std::collections::HashSet<_>>();
                    let invalid = tools_allowed.iter().find(|name| {
                        !seen.insert(name.as_str()) || !available.contains(name.as_str())
                    });
                    if let Some(name) = invalid {
                        return Err(self
                            .cleanup_pending(
                                EngineError::InvalidToolSelection(name.clone()),
                                &pending,
                            )
                            .await);
                    }
                    if finalize_schema.is_some() && !seen.contains(FINALIZE_TOOL_NAME) {
                        return Err(self
                            .cleanup_pending(
                                EngineError::InvalidToolSelection(
                                    "required typed completion tool excluded".into(),
                                ),
                                &pending,
                            )
                            .await);
                    }
                    Some(tools_allowed.clone())
                }
                crate::hooks::BeforeRequestDecision::Inject {
                    item,
                    tools_allowed,
                } => {
                    let valid = item.0.as_object().is_some_and(|object| {
                        object.len() == 3
                            && object.get("type").and_then(serde_json::Value::as_str)
                                == Some("message")
                            && object.get("role").and_then(serde_json::Value::as_str)
                                == Some("user")
                            && object
                                .get("content")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|content| !content.is_empty())
                    });
                    if !valid {
                        return Err(self
                            .cleanup_pending(EngineError::InvalidInjectedItem, &pending)
                            .await);
                    }
                    if let Some(names) = tools_allowed {
                        let mut seen = std::collections::HashSet::new();
                        let available = req
                            .tools
                            .iter()
                            .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
                            .collect::<std::collections::HashSet<_>>();
                        if let Some(name) = names.iter().find(|name| {
                            !seen.insert(name.as_str()) || !available.contains(name.as_str())
                        }) {
                            return Err(self
                                .cleanup_pending(
                                    EngineError::InvalidToolSelection(name.clone()),
                                    &pending,
                                )
                                .await);
                        }
                        if finalize_schema.is_some() && !seen.contains(FINALIZE_TOOL_NAME) {
                            return Err(self
                                .cleanup_pending(
                                    EngineError::InvalidToolSelection(
                                        "required typed completion tool excluded".into(),
                                    ),
                                    &pending,
                                )
                                .await);
                        }
                    }
                    req.input.push(item.clone());
                    input_hashes.push(None);
                    tools_allowed.clone()
                }
            };
            req.tools_allowed = selected_tools;
            let decision = crate::store::Decision {
                hook: "before-request".into(),
                event_refs,
                decision: serde_json::to_value(&before_request.decision)
                    .expect("typed hook decision serializes"),
                evidence: before_request
                    .evidence
                    .clone()
                    .unwrap_or(serde_json::Value::Null),
                latency_ms: Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64),
            };
            let store = self.store.clone();
            let decision_request = parent.clone();
            if let Err(error) =
                blocking(move || store.record_decision(Some(&decision_request), &decision)).await
            {
                return Err(self.cleanup_pending(error, &pending).await);
            }
            let store = self.store.clone();
            let instructions = replay_instructions.clone();
            let (req, replay_request) = match blocking(move || {
                let issued = store.seal_replay_request_with_hashes(
                    &req,
                    &input_hashes,
                    Some(&instructions),
                )?;
                Ok((req, issued))
            })
            .await
            {
                Ok(sealed) => sealed,
                Err(error) => return Err(self.cleanup_pending(error, &pending).await),
            };
            let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);
            // Retain completed calls while the provider is streaming. Their
            // output enters model history after this response, without changing
            // the already-issued request or delaying the host acknowledgment.
            let mut settlements = self.scheduler.operation_settlements();
            let mut retained_in_stream = HashSet::new();
            let mut settlements_open = true;
            if let Err(error) = self
                .retain_pending_outputs(&pending, &mut retained_in_stream)
                .await
            {
                return Err(self.cleanup_pending(error, &pending).await);
            }
            let model_request_id = parent.clone();
            let output_lifetime = output::OutputLifetime {
                observer: self.output_observer.clone(),
                origin: self.origin.clone(),
                request_id: parent.clone(),
            };
            if let Some(observer) = &self.output_observer {
                observer.observe(ModelOutput {
                    origin: self.origin.clone(),
                    request_id: parent.clone(),
                    update: ModelOutputUpdate::Started,
                });
            }
            let create = self
                .client
                .create_streaming_for_request(&model_request_id, req, event_tx);
            tokio::pin!(create);
            let mut turn_call_ids = Vec::<CallId>::new();
            let mut turn_call_items = Vec::<(CallId, Item)>::new();
            let mut inline_settled = Vec::<(
                OperationId,
                Option<crate::store::RecordedWaitContinuation>,
                crate::provider::WaitReplayBarrier,
                Option<crate::replay::ReplayWaitCommit>,
            )>::new();
            let mut wait_call = None;
            let mut persisted_items = Vec::<Item>::new();
            let mut event_stream_open = true;
            let mut deferred_dispatch_error = None;
            let turn = loop {
                tokio::select! {
                    settlement = settlements.recv(), if settlements_open => {
                        match settlement {
                            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                if let Err(error) = self.retain_pending_outputs(&pending, &mut retained_in_stream).await {
                                    return Err(self.cleanup_pending(error, &pending).await);
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => settlements_open = false,
                        }
                    }
                    result = &mut create => match result {
                        Ok(turn) => break turn,
                        Err(error) => {
                            return Err(self.reject_or_cleanup(error, settled_head.as_ref(), &parent, &pending).await);
                        }
                    },
                    changed = cancellation.changed() => {
                        if changed.is_err() || *cancellation.borrow() {
                            return Err(self.cleanup_pending(EngineError::Cancelled { head_request: Some(parent.clone()) }, &pending).await);
                        }
                    }
                    event = event_rx.recv(), if event_stream_open => {
                        match event {
                            Some(StreamEvent::ItemDone(item)) => {
                                let call_id = match parsed_call_id(&item) {
                                    Ok(call_id) => call_id,
                                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                                };
                                if let Some(call_id) = call_id {
                                    if turn_call_ids.contains(&call_id) {
                                        if conflicting_call(&turn_call_items, &call_id, &item) {
                                            return Err(self.cleanup_pending(EngineError::InvalidFunctionCall, &pending).await);
                                        }
                                        continue;
                                    }
                                }
                                if let Err(error) = self.append(&parent, vec![item.clone()]).await {
                                    return Err(self.cleanup_pending(error, &pending).await);
                                }
                                persisted_items.push(item.clone());
                                // Bounded callbacks are sequential continuations. Finish
                                // receiving the provider round before awaiting one, so
                                // its stream keeps progressing while items are retained.
                                if self.bounded_invocation {
                                    continue;
                                }
                                if deferred_dispatch_error.is_some() {
                                    continue;
                                }
                                if finalize_schema.is_some() && is_finalize_call(&item) {
                                    continue;
                                }
                                let call_item = item.clone();
                                match self.dispatch_with_provider(item, &parent, request_provider.clone(), &mut cancellation).await {
                                    Ok(DispatchResult::Pending(call)) => {
                                        if !pending.iter().any(|current| current.operation == call.operation) {
                                            if call.is_wait_agent && (wait_call.is_some() || !inline_settled.is_empty()) {
                                                deferred_dispatch_error = Some(EngineError::MultipleWaitAgents);
                                                continue;
                                            }
                                            if call.is_wait_agent { wait_call = Some(call.call_id.clone()); }
                                            turn_call_ids.push(call.call_id.clone());
                                            turn_call_items.push((call.call_id.clone(), call_item));
                                            pending.push(call);
                                        }
                                    }
                                    Ok(DispatchResult::Settled { operation, continuation, barrier, commit }) => {
                                        if wait_call.is_some() || !inline_settled.is_empty() {
                                            deferred_dispatch_error = Some(EngineError::MultipleWaitAgents);
                                            continue;
                                        }
                                        turn_call_ids.push(operation.call.clone());
                                        turn_call_items.push((operation.call.clone(), call_item));
                                        inline_settled.push((operation, continuation, barrier, commit));
                                    }
                                    Ok(DispatchResult::NotCall) => {}
                                    Err(error) => deferred_dispatch_error = Some(error),
                                }
                            }
                            Some(StreamEvent::Delta {item_id, channel, index, text}) => {
                                self.observe_delta(&parent, item_id, channel, index, text);
                            }
                            None => event_stream_open = false,
                        }
                    }
                }
            };
            // Once transport completed, its exact request/response pair is
            // durable even if a subsequent dispatch or output step fails.
            let turn = {
                let store = self.store.clone();
                let request = parent.clone();
                match blocking(move || {
                    store.record_issued_replay_turn(&request, replay_request, &turn)?;
                    Ok(turn)
                })
                .await
                {
                    Ok(turn) => turn,
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                }
            };
            // A completed response can race the buffered final item events.
            while let Ok(event) = event_rx.try_recv() {
                if let StreamEvent::Delta {
                    item_id,
                    channel,
                    index,
                    text,
                } = event
                {
                    self.observe_delta(&parent, item_id, channel, index, text);
                    continue;
                }
                if let StreamEvent::ItemDone(item) = event {
                    let call_id = match parsed_call_id(&item) {
                        Ok(call_id) => call_id,
                        Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                    };
                    if let Some(call_id) = call_id {
                        if turn_call_ids.contains(&call_id) {
                            if conflicting_call(&turn_call_items, &call_id, &item) {
                                return Err(self
                                    .cleanup_pending(EngineError::InvalidFunctionCall, &pending)
                                    .await);
                            }
                            continue;
                        }
                    }
                    if let Err(error) = self.append(&parent, vec![item.clone()]).await {
                        return Err(self.cleanup_pending(error, &pending).await);
                    }
                    persisted_items.push(item.clone());
                    if self.bounded_invocation {
                        continue;
                    }
                    if deferred_dispatch_error.is_some() {
                        continue;
                    }
                    if finalize_schema.is_some() && is_finalize_call(&item) {
                        continue;
                    }
                    let call_item = item.clone();
                    match self
                        .dispatch_with_provider(
                            item,
                            &parent,
                            request_provider.clone(),
                            &mut cancellation,
                        )
                        .await
                    {
                        Ok(DispatchResult::Pending(call)) => {
                            if !pending
                                .iter()
                                .any(|current| current.operation == call.operation)
                            {
                                if call.is_wait_agent
                                    && (wait_call.is_some() || !inline_settled.is_empty())
                                {
                                    return Err(self
                                        .cleanup_pending(EngineError::MultipleWaitAgents, &pending)
                                        .await);
                                }
                                if call.is_wait_agent {
                                    wait_call = Some(call.call_id.clone());
                                }
                                turn_call_ids.push(call.call_id.clone());
                                turn_call_items.push((call.call_id.clone(), call_item));
                                pending.push(call);
                            }
                        }
                        Ok(DispatchResult::Settled {
                            operation,
                            continuation,
                            barrier,
                            commit,
                        }) => {
                            if wait_call.is_some() || !inline_settled.is_empty() {
                                return Err(self
                                    .cleanup_pending(EngineError::MultipleWaitAgents, &pending)
                                    .await);
                            }
                            turn_call_ids.push(operation.call.clone());
                            turn_call_items.push((operation.call.clone(), call_item));
                            inline_settled.push((operation, continuation, barrier, commit));
                        }
                        Ok(DispatchResult::NotCall) => {}
                        Err(error) => {
                            return Err(self.cleanup_pending(error, &pending).await);
                        }
                    }
                }
            }
            drop(output_lifetime);
            // Some injected transports may only return a turn, without emitting
            // item events. The production client emits every completed item.
            for item in &turn.items {
                if let Some(call_id) = match parsed_call_id(item) {
                    Ok(call_id) => call_id,
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                } {
                    if conflicting_call(&turn_call_items, &call_id, item)
                        || (self.bounded_invocation
                            && persisted_items.iter().any(|retained| {
                                parsed_call_id(retained).ok().flatten().as_ref() == Some(&call_id)
                                    && retained != item
                            }))
                    {
                        return Err(self
                            .cleanup_pending(EngineError::InvalidFunctionCall, &pending)
                            .await);
                    }
                }
                if !remove_matching_item(&mut persisted_items, item) {
                    if let Err(error) = self.append(&parent, vec![item.clone()]).await {
                        return Err(self.cleanup_pending(error, &pending).await);
                    }
                }
                if deferred_dispatch_error.is_some() {
                    continue;
                }
                if item.0["type"] == "function_call" || item.0["type"] == "custom_tool_call" {
                    if finalize_schema.is_some() && is_finalize_call(item) {
                        continue;
                    }
                    let call_id = match parsed_call_id(item) {
                        Ok(Some(call_id)) => call_id,
                        Ok(None) => {
                            return Err(self
                                .cleanup_pending(EngineError::InvalidFunctionCall, &pending)
                                .await);
                        }
                        Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                    };
                    if !turn_call_ids.contains(&call_id) {
                        match self
                            .dispatch_with_provider(
                                item.clone(),
                                &parent,
                                request_provider.clone(),
                                &mut cancellation,
                            )
                            .await
                        {
                            Ok(DispatchResult::Pending(call)) => {
                                if call.is_wait_agent
                                    && (wait_call.is_some() || !inline_settled.is_empty())
                                {
                                    return Err(self
                                        .cleanup_pending(EngineError::MultipleWaitAgents, &pending)
                                        .await);
                                }
                                if call.is_wait_agent {
                                    wait_call = Some(call_id.clone());
                                }
                                turn_call_ids.push(call_id.clone());
                                turn_call_items.push((call_id.clone(), item.clone()));
                                pending.push(call);
                            }
                            Ok(DispatchResult::Settled {
                                operation,
                                continuation,
                                barrier,
                                commit,
                            }) => {
                                if wait_call.is_some() || !inline_settled.is_empty() {
                                    return Err(self
                                        .cleanup_pending(EngineError::MultipleWaitAgents, &pending)
                                        .await);
                                }
                                turn_call_ids.push(operation.call.clone());
                                turn_call_items.push((operation.call.clone(), item.clone()));
                                inline_settled.push((operation, continuation, barrier, commit));
                            }
                            Ok(DispatchResult::NotCall) => {
                                return Err(self
                                    .cleanup_pending(EngineError::InvalidFunctionCall, &pending)
                                    .await);
                            }
                            Err(error) => {
                                return Err(self.cleanup_pending(error, &pending).await);
                            }
                        }
                    }
                }
            }
            if let Some(error) = deferred_dispatch_error {
                return Err(self.cleanup_pending(error, &pending).await);
            }
            let usage_for_store = StoredUsage {
                input_tokens: i64::try_from(turn.usage.input_tokens).unwrap_or(i64::MAX),
                output_tokens: i64::try_from(turn.usage.output_tokens).unwrap_or(i64::MAX),
                cost_micros: 0,
            };
            let store = self.store.clone();
            let request = parent.clone();
            if let Err(error) = blocking(move || store.set_usage(&request, usage_for_store)).await {
                return Err(self.cleanup_pending(error, &pending).await);
            }
            let usage = json!({
                "response_id":turn.response_id,
                "input_tokens":turn.usage.input_tokens,
                "output_tokens":turn.usage.output_tokens,
                "cached_tokens":turn.usage.cached_tokens,
                "cache_write_tokens":turn.usage.cache_write_tokens,
                "reported":turn.usage.reported
            });
            let store = self.store.clone();
            let request = parent.clone();
            if let Err(error) =
                blocking(move || store.record_event(Some(&request), "responses_usage", &usage))
                    .await
            {
                return Err(self.cleanup_pending(error, &pending).await);
            }
            previous_usage = turn.usage.clone();
            compact_due = self
                .compact_at_input_tokens
                .is_some_and(|threshold| turn.usage.input_tokens >= threshold);

            let had_inline_wait = !inline_settled.is_empty();
            for (_, _, barrier, _) in &inline_settled {
                if let Err(error) = self
                    .await_replay_barrier(
                        barrier,
                        ReplayBarrierStage::BeforeWait,
                        &pending,
                        &parent,
                        &mut cancellation,
                    )
                    .await
                {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            }

            let mut settled_this_turn = match self.persist_settled(&mut pending, &parent).await {
                Ok(settled) => settled,
                Err(error) => return Err(self.cleanup_pending(error, &pending).await),
            };

            // Retained builtin output follows every model Item and the ready
            // job drain, matching the live wait boundary's history order.
            for (operation, continuation, barrier, commit) in inline_settled {
                if commit.is_some() && settled_this_turn != barrier.before {
                    return Err(self
                        .cleanup_pending(
                            EngineError::ClaimRecoveryConflict(operation.call.0.clone()),
                            &pending,
                        )
                        .await);
                }
                let item = match self
                    .read_settled_output(&operation, &operation.request)
                    .await
                {
                    Ok(item) => item,
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                };
                let mut attached = vec![item];
                if let Some(continuation) = continuation {
                    match continuation.into_items(&operation, &attached[0]) {
                        Ok(items) => attached.extend(items),
                        Err(error) => {
                            return Err(self.cleanup_pending(error.into(), &pending).await);
                        }
                    }
                }
                let published = match self.append_retaining_items(&parent, attached).await {
                    Ok(items) => items,
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                };
                let recorded_wait = commit.is_some();
                if let Some(commit) = commit {
                    if let Err(error) = commit.commit(&operation, &published[0]) {
                        return Err(self.cleanup_pending(error.into(), &pending).await);
                    }
                }
                drop(published);
                if let Err(error) = self.acknowledge_output(&operation).await {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
                if let Err(error) = self
                    .await_replay_barrier(
                        &barrier,
                        ReplayBarrierStage::AfterWait,
                        &pending,
                        &parent,
                        &mut cancellation,
                    )
                    .await
                {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
                match self.persist_settled(&mut pending, &parent).await {
                    Ok(settled) => {
                        if recorded_wait && settled != barrier.after {
                            return Err(self
                                .cleanup_pending(
                                    EngineError::ClaimRecoveryConflict(operation.call.0.clone()),
                                    &pending,
                                )
                                .await);
                        }
                        settled_this_turn.extend(settled)
                    }
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                }
            }

            if let Some(wait_call_id) = wait_call {
                let result = if let Some(call_id) = settled_this_turn.first() {
                    WaitAgentResultExact {
                        call_outputs: Vec::new(),
                        resumed_by: WaitResumeExact::Job(call_id.clone()),
                    }
                } else {
                    match self
                        .wait_for_resume(&pending, &mut envelopes, &mut cancellation)
                        .await
                    {
                        Ok(result) => result,
                        Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                    }
                };
                if matches!(&result.resumed_by, WaitResumeExact::Cancelled) {
                    return Err(self
                        .cleanup_pending(
                            EngineError::Cancelled {
                                head_request: Some(parent.clone()),
                            },
                            &pending,
                        )
                        .await);
                }
                let wait_operation = OperationId {
                    origin: self.origin.clone(),
                    request: parent.clone(),
                    call: wait_call_id,
                };
                if let Err(error) = self
                    .persist_wait_result(
                        &mut pending,
                        &parent,
                        Some(&wait_operation),
                        result,
                        admit_inbox,
                    )
                    .await
                {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            } else if !had_inline_wait
                && is_final(&turn, finalize_schema.is_some())
                && !pending.is_empty()
                && settled_this_turn.is_empty()
            {
                let result = match self
                    .wait_for_resume(&pending, &mut envelopes, &mut cancellation)
                    .await
                {
                    Ok(result) => result,
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                };
                if matches!(&result.resumed_by, WaitResumeExact::Cancelled) {
                    return Err(self
                        .cleanup_pending(
                            EngineError::Cancelled {
                                head_request: Some(parent.clone()),
                            },
                            &pending,
                        )
                        .await);
                }
                if let Err(error) = self
                    .persist_wait_result(&mut pending, &parent, None, result, admit_inbox)
                    .await
                {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            } else if !had_inline_wait
                && is_final(&turn, finalize_schema.is_some())
                && pending.is_empty()
            {
                if let Some(schema) = finalize_schema {
                    let calls: Vec<_> = turn
                        .items
                        .iter()
                        .filter(|item| is_finalize_call(item))
                        .collect();
                    if calls.len() != 1 {
                        return Err(EngineError::InvalidFinalizeCount);
                    }
                    FinalizeParser::new().parse_completed_with_result_schema(
                        calls[0],
                        &schema["parameters"]["properties"]["result"],
                    )?;
                }
                // Read the durable parent chain only after every item/output
                // from this final turn has been persisted.
                let transcript = match self.read_history(&parent).await {
                    Ok(history) => history,
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                };
                return Ok(EngineCompletion {
                    turn,
                    transcript,
                    head_request: parent,
                });
            } else if turn_call_ids.is_empty() && pending.is_empty() {
                return Err(EngineError::MissingFinal);
            }

            let next_id = RequestId(uuid::Uuid::new_v4().to_string());
            let store = self.store.clone();
            let branch = self.config.agent.0.clone();
            let parent_for_write = parent.clone();
            let request = next_id.clone();
            let identity = identity.clone();
            if let Err(error) = blocking(move || match identity {
                Some(identity) => store.write_embedded_request(
                    &identity,
                    &request,
                    Some(&parent_for_write),
                    &[],
                    StoredUsage::default(),
                ),
                None => store.write_request(
                    &request,
                    Some(&parent_for_write),
                    &branch,
                    &[],
                    StoredUsage::default(),
                ),
            })
            .await
            {
                return Err(self.cleanup_pending(error, &pending).await);
            }
            {
                let store = self.store.clone();
                let agent = self.config.agent.clone();
                let request = next_id.clone();
                if let Err(error) =
                    blocking(move || store.apply_pending_effort(&agent, &request)).await
                {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            }
            if admit_inbox {
                if let Err(error) = self.append_unread_envelopes(&next_id).await {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            }
            parent = next_id;
            // Outputs were appended individually as their jobs settled; next
            // iteration sends the full ordered transcript to the model.
        }
    }

    async fn append(&self, id: &RequestId, items: Vec<Item>) -> Result<(), EngineError> {
        self.append_retaining_items(id, items).await.map(drop)
    }

    async fn append_retaining_items(
        &self,
        id: &RequestId,
        items: Vec<Item>,
    ) -> Result<Vec<Item>, EngineError> {
        let store = self.store.clone();
        let request = id.clone();
        let observing = self.output_observer.is_some();
        let (items, hashes, origin) = blocking(move || {
            let hashes = store.append_items(&request, &items)?;
            let origin = if observing {
                Some(store.request_output_origin(&request)?)
            } else {
                None
            };
            Ok((items, hashes, origin))
        })
        .await?;
        if let (Some(observer), Some(origin)) = (&self.output_observer, origin) {
            for (item, hash) in items
                .iter()
                .filter(|i| !i.is_configuration_update())
                .zip(hashes)
            {
                observer.observe(ModelOutput {
                    origin: origin.clone(),
                    request_id: id.clone(),
                    update: ModelOutputUpdate::Committed {
                        item_id: item.0["id"].as_str().map(str::to_owned),
                        hash,
                    },
                });
            }
        }
        Ok(items)
    }

    async fn append_unread_envelopes(&self, request: &RequestId) -> Result<(), EngineError> {
        let store = self.store.clone();
        let recipient = self.config.agent.clone();
        let request = request.clone();
        blocking(move || {
            store
                .append_unread_envelopes(&recipient, &request)
                .map(|_| ())
        })
        .await
    }

    /// Turn a missing in-memory job into a durable interruption, unless a
    /// concurrent settlement already won and supplied the actual output.
    async fn recover_missing_job(&self, claim: &crate::store::Claim) -> Result<Item, EngineError> {
        let tool_kind = self
            .store
            .tool_invocation_kind(&claim.operation.request, &claim.call_id)?
            .ok_or_else(|| EngineError::MissingInheritedOutput(claim.call_id.0.clone()))?;
        let store = self.store.clone();
        let call = claim.operation.clone();
        let request = claim.request.clone();
        let interrupted =
            blocking(move || store.interrupt_operation_claim(&call, &request)).await?;
        if interrupted > 0 {
            return Ok(Item::tool_output(
                &claim.call_id,
                tool_kind,
                &crate::turn::JobOutput::Interrupted,
            ));
        }

        // A settlement may race the missing-job check. Never replace its
        // durable output with a synthetic interruption.
        let store = self.store.clone();
        let call = claim.operation.clone();
        let request = claim.request.clone();
        let current = blocking(move || {
            store
                .claims_for_operation(&call)
                .map(|claims| claims.into_iter().find(|c| c.request == request))
        })
        .await?;
        match current {
            Some(current) if current.state == crate::store::ClaimState::Settled => {
                self.read_settled_output(&current.operation, &current.request)
                    .await
            }
            Some(current) if current.state == crate::store::ClaimState::Interrupted => {
                Ok(Item::tool_output(
                    &claim.call_id,
                    tool_kind,
                    &crate::turn::JobOutput::Interrupted,
                ))
            }
            _ => Err(EngineError::ClaimRecoveryConflict(claim.call_id.0.clone())),
        }
    }

    async fn read_settled_output(
        &self,
        operation: &OperationId,
        claimant: &RequestId,
    ) -> Result<Item, EngineError> {
        let store = self.store.clone();
        let operation = operation.clone();
        let call_id = operation.call.0.clone();
        let claimant = claimant.clone();
        blocking(move || store.replay_tool_output_claim(&operation, &claimant))
            .await?
            .map(|recorded| recorded.item)
            .ok_or(EngineError::MissingInheritedOutput(call_id))
    }

    fn tools(&self, finalize_schema: Option<&serde_json::Value>) -> crate::transport::ToolManifest {
        self.compose_tools(finalize_schema, &self.provider.tool_manifest())
    }

    fn compose_tools(
        &self,
        finalize_schema: Option<&serde_json::Value>,
        manifest: &crate::transport::ToolManifest,
    ) -> crate::transport::ToolManifest {
        if self.config.tools.is_empty()
            && finalize_schema.is_none()
            && !manifest.has_duplicate_names()
        {
            return manifest.clone();
        }
        let mut tools = self.config.tools.clone();
        if let Some(schema) = finalize_schema {
            // Typed completion owns this name for this run. A caller may have
            // supplied a non-strict or differently-shaped finalize tool;
            // advertising both would make the reply contract ambiguous.
            tools.retain(|tool| {
                tool.get("name").and_then(serde_json::Value::as_str) != Some(FINALIZE_TOOL_NAME)
            });
            tools.push(schema.clone());
        }
        for tool in manifest.iter() {
            let name = tool.get("name").and_then(serde_json::Value::as_str);
            if name.is_none_or(|name| {
                !tools.iter().any(|existing| {
                    existing.get("name").and_then(serde_json::Value::as_str) == Some(name)
                })
            }) {
                tools.push(tool.clone());
            }
        }
        tools.into()
    }

    #[cfg(test)]
    async fn dispatch_completed_item(
        &self,
        item: Item,
        request: &RequestId,
    ) -> Result<DispatchResult, EngineError> {
        if item.0["type"] != "function_call" && item.0["type"] != "custom_tool_call" {
            return Ok(DispatchResult::NotCall);
        }
        let (_cancel, mut cancellation) = watch::channel(false);
        self.dispatch_with_provider(item, request, self.provider.clone(), &mut cancellation)
            .await
    }

    async fn dispatch_with_provider(
        &self,
        item: Item,
        request: &RequestId,
        provider: Arc<dyn Provider>,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<DispatchResult, EngineError> {
        if item.0["type"] != "function_call" && item.0["type"] != "custom_tool_call" {
            return Ok(DispatchResult::NotCall);
        }
        let call = item
            .tool_call()
            .map_err(|_| EngineError::InvalidFunctionCall)?
            .ok_or(EngineError::InvalidFunctionCall)?;
        let call_id = call.call_id;
        let operation = OperationId {
            origin: self.origin.clone(),
            request: request.clone(),
            call: call_id.clone(),
        };
        let name = call.name;
        let input = call.input;
        let tool_kind = input.kind();
        provider.validate_call(&name, tool_kind)?;
        if tool_kind == ToolKind::Custom
            && (crate::provider::is_harness_tool(&name) || name == FINALIZE_TOOL_NAME)
        {
            return Err(EngineError::InvalidFunctionCall);
        }
        let is_wait_agent = name == "wait_agent";
        let is_here_spawn = name == "spawn_agent"
            && matches!(&input, ToolInput::Function(args) if args["from"]["kind"].as_str() == Some("here"));
        if is_wait_agent {
            let store = self.store.clone();
            let call = operation.clone();
            let request_id = request.clone();
            blocking(move || store.claim_operation(&call, &request_id)).await?;
            let retained = tokio::select! {
                biased;
                _ = await_cancellation(cancellation) => Err(EngineError::Cancelled { head_request: Some(request.clone()) }),
                result = async {
                    provider.retained_output(&name, &input, &operation).await?
                        .map(|retained| retained.into_parts(&operation)).transpose()
                } => result.map_err(EngineError::from),
            };
            let retained = match retained {
                Ok(retained) => retained,
                Err(error) => {
                    let store = self.store.clone();
                    let call = operation.clone();
                    let request = request.clone();
                    blocking(move || store.interrupt_operation_claim(&call, &request)).await?;
                    return Err(error);
                }
            };
            if let Some((output, continuation, barrier, commit)) = retained {
                self.retain_output(&operation, tool_kind, &output, request)
                    .await?;
                return Ok(DispatchResult::Settled {
                    operation,
                    continuation,
                    barrier,
                    commit,
                });
            }
            if *cancellation.borrow() || cancellation.has_changed().is_err() {
                let store = self.store.clone();
                let call = operation.clone();
                let request_id = request.clone();
                blocking(move || store.interrupt_operation_claim(&call, &request_id)).await?;
                return Err(EngineError::Cancelled {
                    head_request: Some(request.clone()),
                });
            }
            return Ok(DispatchResult::Pending(PendingCall {
                operation,
                call_id,
                claim_request: request.clone(),
                is_wait_agent,
                tool_kind,
                persist_here_invocation_output: false,
                cancel_job_on_cleanup: false,
            }));
        }
        // Admission is durable before the provider can execute or capture a
        // checkpoint containing this call. Failure never cancels an unrelated
        // job whose call ID happened to collide with this attempted admission.
        let store = self.store.clone();
        let call = operation.clone();
        let request_id = request.clone();
        blocking(move || store.claim_operation(&call, &request_id)).await?;
        if let Err(error) = self
            .scheduler
            .start_operation(
                provider,
                operation.clone(),
                self.config.agent.clone(),
                Some(request.clone()),
                name,
                input,
            )
            .await
        {
            let store = self.store.clone();
            let call = operation.clone();
            let request_id = request.clone();
            blocking(move || store.interrupt_operation_claim(&call, &request_id)).await?;
            return Err(EngineError::Job(error));
        }
        if let Err(error) = self
            .scheduler
            .claim_exact(&operation, self.origin.clone())
            .await
        {
            let _ = self.scheduler.cancel(&operation).await;
            let store = self.store.clone();
            let call = operation.clone();
            let request_id = request.clone();
            blocking(move || store.interrupt_operation_claim(&call, &request_id)).await?;
            return Err(EngineError::Job(error));
        }
        if self.bounded_invocation {
            let output = self.scheduler.wait(&operation).await?;
            self.persist_output(&operation, tool_kind, &output, request, request)
                .await?;
        }
        Ok(DispatchResult::Pending(PendingCall {
            operation,
            call_id,
            claim_request: request.clone(),
            is_wait_agent,
            tool_kind,
            persist_here_invocation_output: is_here_spawn,
            cancel_job_on_cleanup: true,
        }))
    }

    async fn await_replay_barrier(
        &self,
        barrier: &crate::provider::WaitReplayBarrier,
        stage: ReplayBarrierStage,
        pending: &[PendingCall],
        request: &RequestId,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<(), EngineError> {
        let mut seen = HashSet::new();
        for operation in barrier.before.iter().chain(&barrier.after) {
            if operation.origin != self.origin || !seen.insert(operation) {
                return Err(EngineError::ClaimRecoveryConflict(operation.call.0.clone()));
            }
        }
        let (first, second, wait): (&[OperationId], &[OperationId], &[OperationId]) = match stage {
            ReplayBarrierStage::BeforeWait => (&barrier.before, &barrier.after, &barrier.before),
            ReplayBarrierStage::AfterWait => (&[], &barrier.after, &barrier.after),
        };
        for operation in first.iter().chain(second) {
            if !pending
                .iter()
                .any(|call| call.operation == *operation && !call.is_wait_agent)
            {
                return Err(EngineError::ClaimRecoveryConflict(operation.call.0.clone()));
            }
        }
        for operation in wait {
            tokio::select! {
                biased;
                _ = await_cancellation(cancellation) => return Err(EngineError::Cancelled { head_request: Some(request.clone()) }),
                output = self.scheduler.wait(operation) => { output?; }
            }
        }
        Ok(())
    }

    async fn cancel_pending(&self, pending: &[PendingCall]) -> Result<(), EngineError> {
        let mut first_error = None;
        for call in pending {
            let mut persisted_terminal = false;
            if call.cancel_job_on_cleanup && !call.is_wait_agent {
                if let Err(error) = self.scheduler.cancel(&call.operation).await {
                    if !matches!(error, JobError::UnknownCall) {
                        first_error.get_or_insert_with(|| EngineError::Job(error));
                    }
                }
                match self.scheduler.output(&call.operation).await {
                    Ok(Some(output)) => {
                        if matches!(output, crate::turn::JobOutput::CancellationUnconfirmed(_)) {
                            first_error.get_or_insert_with(|| {
                                EngineError::UnconfirmedCancellation {
                                    operation: call.operation.clone(),
                                }
                            });
                        }
                        match self
                            .retain_settled_output(
                                &call.operation,
                                call.tool_kind,
                                &output,
                                &call.claim_request,
                            )
                            .await
                        {
                            Ok(()) => persisted_terminal = true,
                            Err(error) => {
                                first_error.get_or_insert(error);
                            }
                        }
                    }
                    Err(JobError::UnknownCall) => {}
                    Ok(None) => {}
                    Err(error) => {
                        first_error.get_or_insert(EngineError::Job(error));
                    }
                }
            }
            if !persisted_terminal {
                let store = self.store.clone();
                let operation = call.operation.clone();
                let request = call.claim_request.clone();
                if let Err(error) =
                    blocking(move || store.interrupt_operation_claim(&operation, &request)).await
                {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn reject_or_cleanup(
        &self,
        error: TransportError,
        expected_head: Option<&RequestId>,
        request: &RequestId,
        pending: &[PendingCall],
    ) -> EngineError {
        let Some(failure) = error.request_failure() else {
            return self
                .cleanup_pending(EngineError::Transport(error), pending)
                .await;
        };
        let primary = EngineError::RequestRejected {
            head_request: request.clone(),
            error,
        };
        if let Err(cleanup) = self.cancel_pending(pending).await {
            return EngineError::Cleanup {
                primary: Box::new(primary),
                cleanup: cleanup.to_string(),
            };
        }
        let store = self.store.clone();
        let request = request.clone();
        let expected = expected_head.cloned();
        let identity = match &self.origin {
            ConversationIdentity::Embedded {
                run,
                actor,
                incarnation,
            } => Some(crate::embedding::HostIdentity {
                run: run.clone(),
                actor: actor.clone(),
                incarnation: incarnation.clone(),
            }),
            ConversationIdentity::Standalone { .. } => None,
        };
        match blocking(move || {
            store.record_failed_model_request(
                &request,
                &failure,
                identity
                    .as_ref()
                    .map(|identity| (identity, expected.as_ref())),
            )
        })
        .await
        {
            Ok(true) => primary,
            Ok(false) => EngineError::RejectedHeadMismatch,
            Err(error) => error,
        }
    }

    async fn cleanup_pending(&self, primary: EngineError, pending: &[PendingCall]) -> EngineError {
        match self.cancel_pending(pending).await {
            Ok(()) => primary,
            Err(cleanup) => EngineError::Cleanup {
                primary: Box::new(primary),
                cleanup: cleanup.to_string(),
            },
        }
    }

    async fn persist_settled(
        &self,
        pending: &mut Vec<PendingCall>,
        request: &RequestId,
    ) -> Result<Vec<OperationId>, EngineError> {
        let calls: Vec<_> = pending
            .iter()
            .filter(|call| !call.is_wait_agent)
            .map(|call| call.operation.clone())
            .collect();
        let outputs = outputs_in_operation_order(&self.scheduler, &calls).await?;
        let mut settled = Vec::with_capacity(outputs.len());
        for (operation, output) in outputs {
            let call = pending
                .iter()
                .find(|call| call.operation == operation)
                .expect("scheduler output belongs to a pending call");
            let output_request = if call.persist_here_invocation_output {
                call.claim_request.clone()
            } else {
                request.clone()
            };
            if !self.bounded_invocation {
                self.persist_output(
                    &operation,
                    call.tool_kind,
                    &output,
                    &output_request,
                    &call.claim_request,
                )
                .await?;
            }
            pending.retain(|call| call.operation != operation);
            settled.push(operation);
        }
        Ok(settled)
    }

    async fn retain_pending_outputs(
        &self,
        pending: &[PendingCall],
        retained: &mut HashSet<OperationId>,
    ) -> Result<(), EngineError> {
        let operations = pending
            .iter()
            .filter(|call| !call.is_wait_agent && !retained.contains(&call.operation))
            .map(|call| call.operation.clone())
            .collect::<Vec<_>>();
        for (operation, output) in outputs_in_operation_order(&self.scheduler, &operations).await? {
            let call = pending
                .iter()
                .find(|call| call.operation == operation)
                .expect("scheduler output belongs to a pending call");
            self.retain_settled_output(&operation, call.tool_kind, &output, &call.claim_request)
                .await?;
            retained.insert(operation);
        }
        Ok(())
    }

    async fn persist_output(
        &self,
        operation: &OperationId,
        kind: ToolKind,
        output: &crate::turn::JobOutput,
        request: &RequestId,
        claim_request: &RequestId,
    ) -> Result<(), EngineError> {
        let call_id = &operation.call;
        let item = Item::tool_output(call_id, kind, output);
        let store = self.store.clone();
        let call = operation.clone();
        let request = request.clone();
        let claimed_at = claim_request.clone();
        let terminal_output = output.clone();
        let terminal = blocking(move || {
            store.write_job_output(&call, kind, &terminal_output)?;
            let claim = store
                .claims_for_operation(&call)?
                .into_iter()
                .find(|claim| claim.request == claimed_at);
            let retained = match claim.as_ref().and_then(|claim| claim.output.as_ref()) {
                Some(hash) => store.get_item(hash)?,
                None => None,
            };
            Ok((claim, retained, item))
        })
        .await?;
        let (claim, retained, item) = terminal;
        if !matches!(
            claim.as_ref().map(|claim| claim.state),
            Some(crate::store::ClaimState::Settled)
        ) || retained.as_ref() != Some(&item)
        {
            return Err(EngineError::ClaimRecoveryConflict(call_id.0.clone()));
        }
        self.append(&request, vec![item]).await?;
        if matches!(output, crate::turn::JobOutput::Completed(_)) {
            self.acknowledge_output(operation).await?;
        }
        Ok(())
    }

    /// Record terminal evidence without attaching it to an ancestor request.
    /// A later Engine resume replays this durable claim output into its own
    /// request history, avoiding mutation of a parent branch during cleanup.
    async fn retain_settled_output(
        &self,
        operation: &OperationId,
        kind: ToolKind,
        output: &crate::turn::JobOutput,
        claim_request: &RequestId,
    ) -> Result<(), EngineError> {
        self.retain_output(operation, kind, output, claim_request)
            .await?;
        if matches!(output, crate::turn::JobOutput::Completed(_)) {
            self.acknowledge_output(operation).await?;
        }
        Ok(())
    }

    async fn retain_output(
        &self,
        operation: &OperationId,
        kind: ToolKind,
        output: &crate::turn::JobOutput,
        claim_request: &RequestId,
    ) -> Result<(), EngineError> {
        let call_id = &operation.call;
        let item = Item::tool_output(call_id, kind, output);
        let store = self.store.clone();
        let call = operation.clone();
        let claimed_at = claim_request.clone();
        let terminal_output = output.clone();
        let terminal = blocking(move || {
            store.write_job_output(&call, kind, &terminal_output)?;
            let claim = store
                .claims_for_operation(&call)?
                .into_iter()
                .find(|claim| claim.request == claimed_at);
            let retained = match claim.as_ref().and_then(|claim| claim.output.as_ref()) {
                Some(hash) => store.get_item(hash)?,
                None => None,
            };
            Ok((claim, retained, item))
        })
        .await?;
        let (claim, retained, item) = terminal;
        if !matches!(
            claim.as_ref().map(|claim| claim.state),
            Some(crate::store::ClaimState::Settled)
        ) || retained.as_ref() != Some(&item)
        {
            return Err(EngineError::ClaimRecoveryConflict(call_id.0.clone()));
        }
        Ok(())
    }

    async fn acknowledge_output(&self, operation: &OperationId) -> Result<(), EngineError> {
        // A fork replays its ancestor's result; only the issuing conversation
        // can acknowledge the owner's live execution boundary.
        if operation.origin == self.origin && self.store.has_completed_output(operation)? {
            self.provider.output_committed(operation).await?;
        }
        Ok(())
    }

    async fn wait_for_resume(
        &self,
        pending: &[PendingCall],
        envelopes: &mut tokio::sync::mpsc::UnboundedReceiver<MailboxSignal>,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<WaitAgentResultExact, EngineError> {
        let calls: Vec<_> = pending
            .iter()
            .filter(|call| !call.is_wait_agent)
            .map(|call| call.operation.clone())
            .collect();
        loop {
            let result =
                wait_agent_and_drain_exact(envelopes, &self.scheduler, cancellation, &calls)
                    .await?;
            match &result.resumed_by {
                WaitResumeExact::Job(operation) if calls.contains(operation) => return Ok(result),
                WaitResumeExact::Envelope(_) | WaitResumeExact::Cancelled => return Ok(result),
                WaitResumeExact::DurableWake(envelope_id) => {
                    let store = self.store.clone();
                    let recipient = self.config.agent.0.clone();
                    let envelope_id = *envelope_id;
                    let pending = blocking(move || {
                        Ok(store.envelope(envelope_id)?.is_some_and(|envelope| {
                            envelope.recipient == recipient && envelope.delivered_request.is_none()
                        }))
                    })
                    .await?;
                    if pending {
                        return Ok(result);
                    }
                }
                WaitResumeExact::Job(_) => continue,
            }
        }
    }

    async fn persist_wait_result(
        &self,
        pending: &mut Vec<PendingCall>,
        request: &RequestId,
        wait_call: Option<&OperationId>,
        result: WaitAgentResultExact,
        durable_mailbox: bool,
    ) -> Result<(), EngineError> {
        for (operation, output) in result.call_outputs {
            let call = pending
                .iter()
                .find(|call| call.operation == operation)
                .expect("wait result belongs to a pending call");
            let output_request = if call.persist_here_invocation_output {
                call.claim_request.clone()
            } else {
                request.clone()
            };
            self.persist_output(
                &operation,
                call.tool_kind,
                &output,
                &output_request,
                &call.claim_request,
            )
            .await?;
            pending.retain(|call| call.operation != operation);
        }
        let (agent_envelope, user_envelope, output) = match result.resumed_by {
            WaitResumeExact::Job(operation) => {
                (None, None, json!({"resumed_by":{"job":operation.call.0}}))
            }
            WaitResumeExact::Envelope(envelope) => {
                let (channel, content) = envelope.render();
                let role = match channel {
                    MessageChannel::Assistant => "assistant",
                    MessageChannel::User => "user",
                    MessageChannel::Developer => "developer",
                };
                let item = Item(json!({"type":"message","role":role,"content":content}));
                let resumed = if envelope.sender.0 == "/operator" {
                    json!({"resumed_by":"user_input"})
                } else {
                    json!({"resumed_by":{"agent":envelope.sender.0}})
                };
                if channel == MessageChannel::User {
                    (None, Some(item), resumed)
                } else {
                    (Some(item), None, resumed)
                }
            }
            WaitResumeExact::DurableWake(_) => {
                // The next request atomically attaches the stored envelope.
                // A wake never supplies or appends a second copy of its content.
                (None, None, json!({"resumed_by":"user_input"}))
            }
            WaitResumeExact::Cancelled => {
                return Err(EngineError::Cancelled {
                    head_request: Some(request.clone()),
                });
            }
        };
        if let Some(wait_call) = wait_call {
            self.persist_output(
                wait_call,
                ToolKind::Function,
                &crate::turn::JobOutput::Completed(Ok(output)),
                request,
                request,
            )
            .await?;
            pending.retain(|call| call.operation != *wait_call);
        }
        if durable_mailbox {
            // The wake envelope is only a hint. Its persisted inbox row is
            // attached transactionally after outputs and wait status.
            self.append_unread_envelopes(request).await?;
        } else {
            if let Some(item) = agent_envelope {
                self.append(request, vec![item]).await?;
            }
            if let Some(item) = user_envelope {
                self.append(request, vec![item]).await?;
            }
        }
        Ok(())
    }

    async fn read_history(&self, id: &RequestId) -> Result<Vec<Item>, EngineError> {
        load_history(self.store.clone(), id.clone()).await
    }

    async fn read_history_pairs(
        &self,
        id: &RequestId,
    ) -> Result<Vec<(RequestId, Item)>, EngineError> {
        load_history_pairs(self.store.clone(), id.clone()).await
    }

    /// A model-facing Here child is admitted before its spawn tool returns.
    /// Do not send the child's first model request until the parent's actual
    /// function_call_output Item is durable in request history, then copy that
    /// exact Item (not the earlier claim-settlement record) into the snapshot.
    async fn await_here_invocation_output(
        &self,
        snapshot: &Option<RequestId>,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<(), EngineError> {
        let agent_path = self.config.agent.clone();
        let store = self.store.clone();
        let Some(agent) = blocking(move || store.agent(&agent_path)).await? else {
            return Ok(());
        };
        if agent.fork_source["kind"] != "here" {
            return Ok(());
        }
        let Some(source_request) = agent.fork_source["invocation_request"].as_str() else {
            return Ok(());
        };
        let Some(call_id) = agent.fork_source["invocation_call_id"].as_str() else {
            return Ok(());
        };
        let Some(snapshot) = snapshot.clone().or(agent.head_request) else {
            return Err(EngineError::MissingHereSnapshot);
        };
        let source_request = RequestId(source_request.to_owned());
        let call_id = CallId(call_id.to_owned());
        loop {
            let store = self.store.clone();
            let source = source_request.clone();
            let target = snapshot.clone();
            let call = call_id.clone();
            if blocking(move || store.copy_call_output_if_persisted(&source, &target, &call))
                .await?
            {
                return Ok(());
            }
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => {}
                changed = cancellation.changed() => {
                    if changed.is_err() || *cancellation.borrow() {
                        return Err(EngineError::Cancelled { head_request: Some(snapshot.clone()) });
                    }
                }
            }
        }
    }

    async fn compact_window(
        &self,
        source: &RequestId,
        history: &[Item],
        pending: &[PendingCall],
        usage: &Usage,
    ) -> Result<Option<RequestId>, EngineError> {
        let pending_items: Vec<Item> = pending
            .iter()
            .map(|call| {
                self.store
                    .items(&call.operation.request)
                    .map_err(|e| CompactError::Failed(e.to_string()))?
                    .iter()
                    .find(|item| {
                        item.0["call_id"].as_str() == Some(call.call_id.0.as_str())
                            && matches!(
                                item.0["type"].as_str(),
                                Some("function_call" | "custom_tool_call")
                            )
                    })
                    .cloned()
                    .ok_or_else(|| {
                        CompactError::Failed(format!("missing pending call {}", call.call_id.0))
                    })
            })
            .collect::<Result<_, _>>()?;
        let effective_effort = history
            .iter()
            .rev()
            .find_map(Item::configuration_effort)
            .ok_or(EngineError::MissingEffortPin)?;
        let server_compact = |items: Vec<Item>| -> ServerCompactFuture<'_> {
            Box::pin(async move {
                let turn = self
                    .client
                    .create(ResponsesRequest {
                        input: items,
                        instructions: self.config.instructions.clone(),
                        tools: self.tools(None),
                        tools_allowed: None,
                        model: self.config.model.clone(),
                        pinned_effort: effective_effort,
                        session_id: self.config.session_id.clone(),
                    })
                    .await
                    .map_err(|error| CompactError::Failed(error.to_string()))?;
                if !turn.items.iter().any(|item| item.0["type"] == "compaction") {
                    return Err(CompactError::Failed(
                        "server response lacks compaction item".into(),
                    ));
                }
                Ok(turn.items)
            })
        };
        let summary_response = std::sync::Mutex::new(None::<(String, Usage)>);
        let text_turn = |items: Vec<Item>| -> ServerCompactFuture<'_> {
            let summary_response = &summary_response;
            Box::pin(async move {
                let turn = self
                    .client
                    .create(ResponsesRequest {
                        input: items,
                        instructions: self.config.instructions.clone(),
                        tools: vec![].into(),
                        tools_allowed: Some(vec![]),
                        model: self.config.model.clone(),
                        pinned_effort: effective_effort,
                        session_id: self.config.session_id.clone(),
                    })
                    .await
                    .map_err(|error| CompactError::Failed(error.to_string()))?;
                *summary_response
                    .lock()
                    .expect("summary response lock poisoned") =
                    Some((turn.response_id.clone(), turn.usage.clone()));
                Ok(turn.items)
            })
        };
        // Server does not request typed turns; the capability is reserved for
        // other strategies and must not silently call an unforced model turn.
        let typed_turn = |_instructions: String,
                          _tool: ToolName,
                          _schema: serde_json::Value|
         -> TypedTurnFuture<'_> {
            Box::pin(async {
                Err(CompactError::Failed(
                    "typed turn requires a forced-tool transport".into(),
                ))
            })
        };
        let context = CompactContext {
            items: history,
            usage,
            pending_calls: &pending_items,
            effort: effective_effort,
            server_compact: &server_compact,
            typed_turn: &typed_turn,
            text_turn: &text_turn,
        };
        let window = match self.compaction_strategy {
            CompactionStrategy::Server => Server.compact(context).await?,
            CompactionStrategy::PlainText => match PlainText.compact(context).await {
                Ok(window) => window,
                Err(error) => {
                    let detail: String = error.to_string().chars().take(256).collect();
                    self.record_text_compaction_attempt(
                        source,
                        "failed",
                        history,
                        None,
                        &summary_response,
                        Some(&detail),
                    )
                    .await?;
                    return Ok(None);
                }
            },
        };
        if self.compaction_strategy == CompactionStrategy::PlainText {
            let old_bytes = serde_json::to_vec(history)
                .expect("Item serialization is infallible")
                .len();
            let new_bytes = serde_json::to_vec(&window.items)
                .expect("Item serialization is infallible")
                .len();
            if new_bytes.saturating_mul(5) > old_bytes.saturating_mul(4) {
                self.record_text_compaction_attempt(
                    source,
                    "no_progress",
                    history,
                    Some(new_bytes),
                    &summary_response,
                    None,
                )
                .await?;
                return Ok(None);
            }
        }
        let request = RequestId(uuid::Uuid::new_v4().to_string());
        let window_bytes = (self.compaction_strategy == CompactionStrategy::PlainText).then(|| {
            serde_json::to_vec(&window.items)
                .expect("Item serialization is infallible")
                .len()
        });
        let store = self.store.clone();
        let source_for_write = source.clone();
        let successor = request.clone();
        let branch = self.config.agent.0.clone();
        let pending_operations = pending
            .iter()
            .filter(|call| !call.is_wait_agent)
            .map(|call| call.operation.clone())
            .collect::<Vec<_>>();
        let identity = self.embedded_identity();
        blocking(move || {
            store.write_compaction_request_with_claims(
                &successor,
                &source_for_write,
                &branch,
                &window.items,
                &pending_operations,
                identity.as_ref(),
            )
        })
        .await?;
        if self.compaction_strategy == CompactionStrategy::PlainText {
            self.record_text_compaction_attempt(
                source,
                "applied",
                history,
                window_bytes,
                &summary_response,
                None,
            )
            .await?;
        }
        Ok(Some(request))
    }

    async fn record_text_compaction_attempt(
        &self,
        source: &RequestId,
        outcome: &str,
        history: &[Item],
        window_bytes: Option<usize>,
        summary_response: &std::sync::Mutex<Option<(String, Usage)>>,
        error: Option<&str>,
    ) -> Result<(), EngineError> {
        let source_bytes = serde_json::to_vec(history)
            .expect("Item serialization is infallible")
            .len();
        let response = summary_response
            .lock()
            .expect("summary response lock poisoned")
            .clone();
        let payload = json!({
            "outcome": outcome,
            "source_bytes": source_bytes,
            "window_bytes": window_bytes,
            "summary_response_id": response.as_ref().map(|(id, _)| id),
            "summary_usage": response.as_ref().map(|(_, usage)| usage),
            "error": error,
        });
        let store = self.store.clone();
        let request = source.clone();
        blocking(move || {
            store
                .record_event(Some(&request), "compaction_attempt", &payload)
                .map(|_| ())
        })
        .await
    }
}

async fn load_history(store: Arc<Store>, id: RequestId) -> Result<Vec<Item>, EngineError> {
    Ok(load_history_window(store, id).await?.items)
}

async fn load_history_pairs(
    store: Arc<Store>,
    id: RequestId,
) -> Result<Vec<(RequestId, Item)>, EngineError> {
    let history = load_history_window(store, id).await?;
    Ok(history
        .provenance
        .into_iter()
        .zip(history.items)
        .map(|((request, _), item)| (request, item))
        .collect())
}

async fn load_history_window(
    store: Arc<Store>,
    id: RequestId,
) -> Result<HistoryWindow, EngineError> {
    blocking(move || {
        let mut cursor = Some(id);
        let mut chain = Vec::new();
        while let Some(request_id) = cursor {
            let Some(request) = store.request(&request_id)? else {
                return Err(StoreError::MissingRequest(request_id.0));
            };
            chain.push(
                store
                    .items_with_hashes(&request_id)?
                    .into_iter()
                    .map(|(hash, item)| (request_id.clone(), hash, item))
                    .collect::<Vec<_>>(),
            );
            if store.is_compaction_boundary(&request_id)? {
                break;
            }
            cursor = request.parent;
        }
        chain.reverse();
        let mut items = Vec::new();
        let mut provenance = Vec::new();
        for (request, hash, item) in chain.into_iter().flatten() {
            items.push(item);
            provenance.push((request, hash));
        }
        Ok(HistoryWindow { items, provenance })
    })
    .await
}

fn remove_matching_item(persisted: &mut Vec<Item>, item: &Item) -> bool {
    let Some(index) = persisted.iter().position(|candidate| candidate == item) else {
        return false;
    };
    persisted.remove(index);
    true
}

fn is_finalize_call(item: &Item) -> bool {
    item.0["type"] == "function_call" && item.0["name"] == FINALIZE_TOOL_NAME
}

fn is_final(turn: &ResponsesTurn, finalized: bool) -> bool {
    turn.items.iter().any(|item| {
        if finalized {
            is_finalize_call(item)
        } else {
            item.0["phase"] == "final_answer"
        }
    })
}

async fn await_cancellation(cancellation: &mut watch::Receiver<bool>) {
    loop {
        if *cancellation.borrow() {
            return;
        }
        if cancellation.changed().await.is_err() {
            return;
        }
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, StoreError> + Send + 'static,
) -> Result<T, EngineError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| EngineError::StoreTask)?
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentInvocation, AgentToolService, dispatch_agent_verb};

    fn empty_mailbox() -> tokio::sync::mpsc::UnboundedReceiver<Envelope> {
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        receiver
    }
    use crate::provider::ProviderError;
    use crate::transport::{TransportError, Usage};
    use async_trait::async_trait;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use std::sync::Mutex;
    use tokio::sync::Notify;

    #[derive(Clone)]
    struct FakeAuth;
    impl Auth for FakeAuth {
        fn access(&self) -> Result<(String, String), TransportError> {
            Ok(("unused".into(), "unused".into()))
        }
    }

    struct Replay {
        requests: Arc<Mutex<Vec<ResponsesRequest>>>,
        turns: Mutex<std::collections::VecDeque<ResponsesTurn>>,
    }
    #[async_trait::async_trait]
    impl ResponsesTransport for Replay {
        async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            self.requests.lock().unwrap().push(request);
            self.turns
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| TransportError::Stream("replay exhausted".into()))
        }
    }

    struct EffortSettlementBarrier {
        replay: Replay,
        release_first_turn: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }
    #[async_trait::async_trait]
    impl ResponsesTransport for EffortSettlementBarrier {
        async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            self.replay.create(request).await
        }

        async fn create_streaming(
            &self,
            request: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            let turn = self.replay.create(request).await?;
            for item in &turn.items {
                let _ = sink.send(StreamEvent::ItemDone(item.clone())).await;
            }
            let release = { self.release_first_turn.lock().await.take() };
            if let Some(release) = release {
                release
                    .await
                    .map_err(|_| TransportError::Stream("effort turn gate dropped".into()))?;
            }
            Ok(turn)
        }
    }

    async fn inherited_claim_fixture() -> (Arc<Store>, RequestId, RequestId, CallId) {
        let store = Arc::new(Store::memory().unwrap());
        let parent = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        let source_head = RequestId("inherited-parent-head".into());
        let snapshot = RequestId("inherited-child-snapshot".into());
        let call_id = CallId("inherited-call".into());
        store.create_request(&source_head, None, &parent.0).unwrap();
        store
            .append_items(
                &source_head,
                &[Item(json!({
                    "type":"function_call",
                    "call_id":call_id.0,
                    "name":"slow",
                    "arguments":"{}"
                }))],
            )
            .unwrap();
        store.set_effort(&source_head, Effort::Low).unwrap();
        store
            .admit_agent(
                &parent,
                None,
                Some(&source_head),
                &json!({}),
                &json!({"kind":"root"}),
            )
            .unwrap();
        store.claim(&call_id, &source_head).unwrap();
        store
            .admit_here_agent_with_snapshot(
                &child,
                &parent,
                &snapshot,
                &json!({}),
                &parent.0,
                &child.0,
                "AtBoundary",
                &Item(json!({
                    "type":"message","role":"assistant","content":[{
                        "type":"output_text","text":"NEW_TASK"
                    }]
                })),
            )
            .unwrap();
        let child_claims = store.claims_on(&snapshot).unwrap();
        assert_eq!(child_claims.len(), 1);
        assert_eq!(child_claims[0].call_id, call_id);
        assert_eq!(child_claims[0].request, snapshot);
        (store, source_head, snapshot, call_id)
    }

    #[tokio::test]
    async fn inherited_settled_legacy_outcome_refuses_before_provider_request() {
        for already_in_history in [false, true] {
            let (store, source, snapshot, call) = inherited_claim_fixture().await;
            let operation = store.operation_for_request(&source, &call).unwrap();
            let output = Item::tool_output(
                &call,
                ToolKind::Function,
                &crate::turn::JobOutput::Completed(Ok(json!({"result":"legacy"}))),
            );
            store
                .write_output(&operation, &output, crate::store::TerminalOutcome::Success)
                .unwrap();
            if already_in_history {
                store.append_items(&snapshot, &[output.clone()]).unwrap();
            }
            store
                .lock()
                .execute(
                    "UPDATE claims SET terminal_json=NULL WHERE state='settled'",
                    [],
                )
                .unwrap();
            let before = store.items(&snapshot).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let engine = Engine::<FakeAuth, Echo, _>::with_transport(
                Replay {
                    requests: requests.clone(),
                    turns: Mutex::new(Default::default()),
                },
                store.clone(),
                Arc::new(JobScheduler::new(1).unwrap()),
                Arc::new(Echo),
                EngineConfig {
                    instructions: "instruction".into(),
                    tools: vec![],
                    model: "test".into(),
                    effort: Effort::Low,
                    session_id: "legacy-child".into(),
                    agent: AgentPath("/root/child".into()),
                },
            );
            assert_ne!(operation.origin, engine.origin);
            let (_cancel, cancellation) = watch::channel(false);
            assert!(matches!(
                engine.run(Some(snapshot.clone()), vec![], cancellation, empty_mailbox()).await,
                Err(EngineError::Store(StoreError::UnsupportedReplayOutcome { operation: refused })) if refused == operation
            ));
            assert!(requests.lock().unwrap().is_empty());
            assert_eq!(store.items(&snapshot).unwrap(), before);
            assert_eq!(
                store.replay_output_operation(&operation).unwrap(),
                Some(output)
            );
        }
    }

    #[tokio::test]
    async fn retained_wait_lookup_is_cancel_responsive_and_fences_none() {
        struct Lookup {
            cancel: watch::Sender<bool>,
            block: bool,
        }
        #[async_trait]
        impl Provider for Lookup {
            async fn retained_output(
                &self,
                _: &str,
                _: &ToolInput,
                _: &OperationId,
            ) -> Result<Option<crate::provider::RetainedOutput>, ProviderError> {
                self.cancel.send(true).unwrap();
                if self.block {
                    std::future::pending::<()>().await;
                }
                Ok(None)
            }
            async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
                panic!("cancelled wait cannot execute provider work")
            }
            fn tools(&self) -> Vec<Value> {
                vec![]
            }
        }
        for block in [true, false] {
            let store = Arc::new(Store::memory().unwrap());
            let request = RequestId("cancel-retained-wait".into());
            store.create_request(&request, None, "/root").unwrap();
            let item = Item(
                json!({"type":"function_call","call_id":"wait","name":"wait_agent","arguments":"{}"}),
            );
            store.append_items(&request, &[item.clone()]).unwrap();
            let (cancel, mut cancellation) = watch::channel(false);
            let provider = Arc::new(Lookup { cancel, block });
            let engine = Engine::<FakeAuth, Lookup, _>::with_transport(
                Replay {
                    requests: Arc::new(Mutex::new(vec![])),
                    turns: Mutex::new(Default::default()),
                },
                store.clone(),
                Arc::new(JobScheduler::new(1).unwrap()),
                provider.clone(),
                EngineConfig {
                    instructions: "test".into(),
                    tools: vec![],
                    model: "test".into(),
                    effort: Effort::Low,
                    session_id: "test".into(),
                    agent: AgentPath("/root".into()),
                },
            );
            assert!(matches!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    engine.dispatch_with_provider(item, &request, provider, &mut cancellation)
                )
                .await
                .unwrap(),
                Err(EngineError::Cancelled { .. })
            ));
            assert_eq!(
                store.claims_on(&request).unwrap()[0].state,
                crate::store::ClaimState::Interrupted
            );
        }
    }

    #[tokio::test]
    async fn retained_wait_counts_once_and_attaches_after_full_model_turn() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Retained(AtomicUsize);
        #[async_trait]
        impl Provider for Retained {
            async fn retained_output(
                &self,
                _: &str,
                _: &ToolInput,
                _: &OperationId,
            ) -> Result<Option<crate::provider::RetainedOutput>, ProviderError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(Some(crate::provider::RetainedOutput::terminal(
                    crate::turn::JobOutput::Completed(Ok(json!({"resumed_by":"user_input"}))),
                )))
            }
            async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
                panic!("retained wait cannot execute provider work")
            }
            fn tools(&self) -> Vec<Value> {
                vec![]
            }
        }
        let requests = Arc::new(Mutex::new(vec![]));
        let wait = Item(
            json!({"type":"function_call","call_id":"retained-wait","name":"wait_agent","arguments":"{}"}),
        );
        let following = Item(
            json!({"type":"message","role":"assistant","phase":"final_answer","content":"after wait call"}),
        );
        let provider = Arc::new(Retained(AtomicUsize::new(0)));
        let engine=Engine::<FakeAuth,Retained,_>::with_transport(
            Replay{requests:requests.clone(),turns:Mutex::new([turn("wait",vec![wait.clone(),following.clone()]),turn("final",vec![Item(json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}))])].into())},
            Arc::new(Store::memory().unwrap()),Arc::new(JobScheduler::new(1).unwrap()),provider.clone(),
            EngineConfig{instructions:"test".into(),tools:vec![],model:"test".into(),effort:Effort::Low,session_id:"test".into(),agent:AgentPath("/root".into())},
        );
        let (_cancel, cancellation) = watch::channel(false);
        let completion = engine
            .run(None, vec![], cancellation, empty_mailbox())
            .await
            .unwrap();
        assert_eq!(provider.0.load(Ordering::SeqCst), 1);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let next = &requests[1].input;
        let call = next.iter().position(|item| item == &wait).unwrap();
        let model_item = next.iter().position(|item| item == &following).unwrap();
        let output = next
            .iter()
            .position(|item| {
                item.0["type"] == "function_call_output" && item.0["call_id"] == "retained-wait"
            })
            .unwrap();
        assert!(call < model_item && model_item < output);
        assert_eq!(
            completion
                .transcript
                .iter()
                .filter(|item| item.0["type"] == "function_call_output"
                    && item.0["call_id"] == "retained-wait")
                .count(),
            1
        );
    }

    struct Echo;
    #[async_trait]
    impl Provider for Echo {
        async fn call(&self, name: &str, args: Value) -> Result<Value, ProviderError> {
            Ok(json!({"tool":name,"args":args}))
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn engine_custom_stream_and_final_copy_dispatch_once_with_matching_output() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountRaw {
            calls: Arc<AtomicUsize>,
            inputs: Arc<Mutex<Vec<Value>>>,
        }
        #[async_trait]
        impl Provider for CountRaw {
            async fn call(&self, _name: &str, _input: Value) -> Result<Value, ProviderError> {
                unreachable!("custom input must not enter function dispatch")
            }
            async fn call_custom_with_context(
                &self,
                _name: &str,
                input: String,
                _context: crate::provider::CallContext,
            ) -> Result<Value, ProviderError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.inputs.lock().unwrap().push(Value::String(input));
                Ok(Value::String("raw result".into()))
            }
            fn tools(&self) -> Vec<Value> {
                vec![]
            }
        }

        let raw = "line 1\nquote \" slash \\ λ";
        let call = Item(json!({
            "type":"custom_tool_call", "call_id":"raw-call", "name":"cell", "input":raw
        }));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, CountRaw, _>::with_transport(
            Replay {
                requests: requests.clone(),
                turns: Mutex::new(
                    [
                        turn("raw-start", vec![call]),
                        turn(
                            "raw-finish",
                            vec![Item(json!({
                                "type":"message", "role":"assistant",
                                "phase":"final_answer", "content":"done"
                            }))],
                        ),
                    ]
                    .into(),
                ),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(CountRaw {
                calls: calls.clone(),
                inputs: inputs.clone(),
            }),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "custom-once".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let completion = engine
            .run(None, vec![], cancel_rx, empty_mailbox())
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(*inputs.lock().unwrap(), vec![Value::String(raw.into())]);
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[1]
                .input
                .iter()
                .filter(|item| item.0["type"] == "custom_tool_call_output")
                .count(),
            1
        );
        assert!(
            sent[1]
                .input
                .iter()
                .any(|item| item.0["call_id"] == "raw-call"
                    && item.0["type"] == "custom_tool_call_output"
                    && item.0["output"] == "raw result")
        );
        assert_eq!(store.claims(&CallId("raw-call".into())).unwrap().len(), 1);
        assert_eq!(completion.turn.response_id, "raw-finish");
    }

    #[tokio::test]
    async fn engine_rejects_malformed_and_reserved_custom_calls_before_admission() {
        let store = Arc::new(Store::memory().unwrap());
        let request = RequestId("reject-custom".into());
        store.create_request(&request, None, "/root").unwrap();
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            Replay {
                requests: Arc::new(Mutex::new(Vec::new())),
                turns: Mutex::new(std::collections::VecDeque::new()),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "reject-custom".into(),
                agent: AgentPath("/root".into()),
            },
        );
        for item in [
            Item(json!({"type":"custom_tool_call","call_id":"missing-input","name":"cell"})),
            Item(json!({"type":"custom_tool_call","call_id":"bad-input","name":"cell","input":{}})),
            Item(
                json!({"type":"custom_tool_call","call_id":"reserved","name":"wait_agent","input":"{}"}),
            ),
            Item(
                json!({"type":"custom_tool_call","call_id":"reserved-finalize","name":"finalize","input":"{}"}),
            ),
            Item(
                json!({"type":"function_call","call_id":"bad-function","name":"echo","arguments":"{"}),
            ),
        ] {
            assert!(matches!(
                engine.dispatch_completed_item(item, &request).await,
                Err(EngineError::InvalidFunctionCall)
            ));
        }
        assert!(store.claims_on(&request).unwrap().is_empty());
    }

    #[tokio::test]
    async fn engine_stream_rejects_malformed_custom_call_explicitly() {
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            Replay {
                requests: Arc::new(Mutex::new(Vec::new())),
                turns: Mutex::new(
                    [turn(
                        "malformed-custom",
                        vec![Item(json!({
                            "type":"custom_tool_call", "call_id":"bad", "name":"cell", "input":{}
                        }))],
                    )]
                    .into(),
                ),
            },
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "malformed-custom".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        assert!(matches!(
            engine.run(None, vec![], cancel_rx, empty_mailbox()).await,
            Err(EngineError::InvalidFunctionCall)
        ));
    }

    #[tokio::test]
    async fn engine_rejects_conflicting_stream_and_final_custom_copy() {
        struct MutatingCopy;
        #[async_trait]
        impl ResponsesTransport for MutatingCopy {
            async fn create(
                &self,
                _request: ResponsesRequest,
            ) -> Result<ResponsesTurn, TransportError> {
                unreachable!("streaming entry point is used")
            }
            async fn create_streaming(
                &self,
                _request: ResponsesRequest,
                sink: tokio::sync::mpsc::Sender<StreamEvent>,
            ) -> Result<ResponsesTurn, TransportError> {
                sink.send(StreamEvent::ItemDone(Item(json!({
                    "type":"custom_tool_call","call_id":"same-id","name":"cell","input":"first"
                }))))
                .await
                .unwrap();
                Ok(turn(
                    "conflicting-final",
                    vec![Item(json!({
                        "type":"custom_tool_call","call_id":"same-id","name":"cell","input":"changed"
                    }))],
                ))
            }
        }
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            MutatingCopy,
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "conflicting-copy".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        assert!(matches!(
            engine.run(None, vec![], cancel_rx, empty_mailbox()).await,
            Err(EngineError::InvalidFunctionCall)
        ));
    }

    #[test]
    fn engine_rejects_durable_output_kind_or_identity_mismatch() {
        let call = CallId("custom-call".into());
        let custom = Item(json!({
            "type":"custom_tool_call_output","call_id":"custom-call","output":"raw result"
        }));
        assert!(validate_tool_output(&call, ToolKind::Custom, &custom).is_ok());
        for item in [
            Item(json!({"type":"function_call_output","call_id":"custom-call","output":"{}"})),
            Item(
                json!({"type":"custom_tool_call_output","call_id":"different","output":"raw result"}),
            ),
        ] {
            assert!(matches!(
                validate_tool_output(&call, ToolKind::Custom, &item),
                Err(EngineError::MismatchedToolOutput(_))
            ));
        }
    }

    #[tokio::test]
    async fn engine_terminal_claim_guards_replay_and_cleanup_output() {
        let store = Arc::new(Store::memory().unwrap());
        let origin = RequestId("first-terminal-origin".into());
        let delivery = RequestId("first-terminal-delivery".into());
        store.create_request(&origin, None, "/root").unwrap();
        store
            .create_request(&delivery, Some(&origin), "/root")
            .unwrap();
        let interrupted = CallId("interrupted-call".into());
        store.append_items(&origin, &[
            Item(json!({"type":"function_call","call_id":"interrupted-call","name":"cell","arguments":"{}"})),
            Item(json!({"type":"function_call","call_id":"cancelled-call","name":"cell","arguments":"{}"})),
        ]).unwrap();
        store.claim(&interrupted, &origin).unwrap();
        assert_eq!(store.interrupt_claim(&interrupted, &origin).unwrap(), 1);
        let cancelled = CallId("cancelled-call".into());
        store.claim(&cancelled, &origin).unwrap();
        let cancelled_output = Item::tool_output(
            &cancelled,
            ToolKind::Function,
            &crate::turn::JobOutput::Cancelled,
        );
        assert_eq!(
            store
                .write_output(
                    &store.claims(&cancelled).unwrap()[0].operation,
                    &cancelled_output,
                    crate::store::TerminalOutcome::Cancelled
                )
                .unwrap(),
            1
        );

        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            Replay {
                requests: Arc::new(Mutex::new(Vec::new())),
                turns: Mutex::new(std::collections::VecDeque::new()),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "first-terminal".into(),
                agent: AgentPath("/root".into()),
            },
        );
        for call in [&interrupted, &cancelled] {
            assert!(matches!(
                engine
                    .persist_output(
                        &store.claims(call).unwrap()[0].operation,
                        ToolKind::Function,
                        &crate::turn::JobOutput::Completed(Ok(json!({"late":"success"}))),
                        &delivery,
                        &origin,
                    )
                    .await,
                Err(EngineError::ClaimRecoveryConflict(_))
                    | Err(EngineError::Store(
                        StoreError::ConflictingReplayOutcome { .. }
                    ))
            ));
        }
        assert!(
            store.items(&delivery).unwrap().is_empty(),
            "no late success is appended"
        );
        assert_eq!(
            store.claims(&interrupted).unwrap()[0].state,
            crate::store::ClaimState::Interrupted
        );
        let cancelled_claims = store.claims(&cancelled).unwrap();
        let cancelled_claim = &cancelled_claims[0];
        assert_eq!(cancelled_claim.state, crate::store::ClaimState::Settled);
        assert_eq!(
            store
                .get_item(cancelled_claim.output.as_ref().unwrap())
                .unwrap(),
            Some(cancelled_output)
        );
        assert!(matches!(
            engine
                .retain_settled_output(
                    &store.claims(&interrupted).unwrap()[0].operation,
                    ToolKind::Function,
                    &crate::turn::JobOutput::Cancelled,
                    &origin,
                )
                .await,
            Err(EngineError::ClaimRecoveryConflict(_))
        ));
        assert!(matches!(
            engine
                .retain_settled_output(
                    &store.claims(&cancelled).unwrap()[0].operation,
                    ToolKind::Function,
                    &crate::turn::JobOutput::Completed(Ok(json!({"late":"success"}))),
                    &origin,
                )
                .await,
            Err(EngineError::Store(
                StoreError::ConflictingReplayOutcome { .. }
            ))
        ));
        engine
            .retain_settled_output(
                &store.claims(&cancelled).unwrap()[0].operation,
                ToolKind::Function,
                &crate::turn::JobOutput::Cancelled,
                &origin,
            )
            .await
            .unwrap();
        engine
            .persist_output(
                &store.claims(&cancelled).unwrap()[0].operation,
                ToolKind::Function,
                &crate::turn::JobOutput::Cancelled,
                &delivery,
                &origin,
            )
            .await
            .unwrap();
        assert_eq!(
            store.items(&delivery).unwrap(),
            vec![Item::tool_output(
                &cancelled,
                ToolKind::Function,
                &crate::turn::JobOutput::Cancelled
            )],
            "replay may append the identical first-terminal output"
        );
    }

    #[tokio::test]
    async fn engine_rejects_duplicate_inherited_terminal_outputs() {
        let store = Arc::new(Store::memory().unwrap());
        let head = RequestId("duplicate-inherited-output".into());
        let call = CallId("duplicate-call".into());
        let invocation = Item(json!({
            "type":"custom_tool_call","call_id":call.0,"name":"cell","input":"raw"
        }));
        let output = Item::tool_output(
            &call,
            ToolKind::Custom,
            &crate::turn::JobOutput::Completed(Ok(Value::String("value".into()))),
        );
        store
            .write_request(
                &head,
                None,
                "/root",
                &[invocation, output.clone(), output.clone()],
                StoredUsage::default(),
            )
            .unwrap();
        store.set_effort(&head, Effort::Low).unwrap();
        store.claim(&call, &head).unwrap();
        assert_eq!(
            store
                .write_output(
                    &store.claims(&call).unwrap()[0].operation,
                    &output,
                    crate::store::TerminalOutcome::Success
                )
                .unwrap(),
            1
        );
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            Replay {
                requests: Arc::new(Mutex::new(Vec::new())),
                turns: Mutex::new(std::collections::VecDeque::new()),
            },
            store,
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "duplicate-output".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        assert!(matches!(
            engine
                .run(Some(head), vec![], cancel_rx, empty_mailbox())
                .await,
            Err(EngineError::MismatchedToolOutput(_))
        ));
    }

    struct BeforeRequestRecorder {
        plans: Arc<Mutex<Vec<crate::hooks::RequestPlan>>>,
    }
    struct RestrictedSelection(Vec<String>);
    struct InjectSelection(Item, Option<Vec<String>>);
    #[async_trait]
    impl Provider for InjectSelection {
        async fn before_request(
            &self,
            _plan: &crate::hooks::RequestPlanView<'_>,
        ) -> crate::hooks::BeforeRequestResult {
            crate::hooks::BeforeRequestResult {
                decision: crate::hooks::BeforeRequestDecision::Inject {
                    item: self.0.clone(),
                    tools_allowed: self.1.clone(),
                },
                evidence: Some(json!({"inject":"opaque"})),
            }
        }
        async fn call(&self, _name: &str, _args: Value) -> Result<Value, ProviderError> {
            unreachable!("injection test does not dispatch tools")
        }
        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }
    #[async_trait]
    impl Provider for RestrictedSelection {
        async fn before_request(
            &self,
            _plan: &crate::hooks::RequestPlanView<'_>,
        ) -> crate::hooks::BeforeRequestResult {
            crate::hooks::BeforeRequestResult {
                decision: crate::hooks::BeforeRequestDecision::SendRestricted {
                    tools_allowed: self.0.clone(),
                },
                evidence: None,
            }
        }
        async fn call(&self, name: &str, args: Value) -> Result<Value, ProviderError> {
            Err(ProviderError::Tool(
                format!("unexpected tool: {name} {args}").into(),
            ))
        }
        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }
    #[async_trait]
    impl Provider for BeforeRequestRecorder {
        async fn before_request(
            &self,
            plan: &crate::hooks::RequestPlanView<'_>,
        ) -> crate::hooks::BeforeRequestResult {
            self.plans.lock().unwrap().push(plan.to_owned());
            crate::hooks::BeforeRequestResult {
                decision: crate::hooks::BeforeRequestDecision::Send,
                evidence: Some(json!({"marker":"engine-before-request"})),
            }
        }

        async fn call(&self, name: &str, args: Value) -> Result<Value, ProviderError> {
            Err(ProviderError::Tool(
                format!("unexpected tool: {name} {args}").into(),
            ))
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    struct PausingBeforeRequest {
        started: Arc<Notify>,
    }
    #[async_trait]
    impl Provider for PausingBeforeRequest {
        async fn before_request(
            &self,
            _plan: &crate::hooks::RequestPlanView<'_>,
        ) -> crate::hooks::BeforeRequestResult {
            self.started.notify_one();
            std::future::pending().await
        }

        async fn call(&self, _name: &str, _args: Value) -> Result<Value, ProviderError> {
            unreachable!("no transport request should be made")
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    struct NeverSettlingTool;
    #[async_trait]
    impl Provider for NeverSettlingTool {
        async fn call(&self, _name: &str, _args: Value) -> Result<Value, ProviderError> {
            std::future::pending().await
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn before_request_decision_store_failure_cleans_pending_claim() {
        let db_path = std::env::temp_dir().join(format!(
            "harness-before-request-store-failure-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let store = Arc::new(Store::open(&db_path).unwrap());
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        let source_head = RequestId("decision-failure-parent".into());
        let snapshot = RequestId("decision-failure-child".into());
        let call_id = CallId("decision-failure-pending".into());
        store.create_request(&source_head, None, &root.0).unwrap();
        store
            .append_items(
                &source_head,
                &[Item(json!({
                    "type":"function_call",
                    "call_id":call_id.0,
                    "name":"never",
                    "arguments":"{}"
                }))],
            )
            .unwrap();
        store.set_effort(&source_head, Effort::Low).unwrap();
        store
            .admit_agent(
                &root,
                None,
                Some(&source_head),
                &json!({}),
                &json!({"kind":"root"}),
            )
            .unwrap();
        store.claim(&call_id, &source_head).unwrap();
        store
            .admit_here_agent_with_snapshot(
                &child,
                &root,
                &snapshot,
                &json!({}),
                &root.0,
                &child.0,
                "AtBoundary",
                &Item(json!({
                    "type":"message","role":"assistant","content":[{
                        "type":"output_text","text":"continue"
                    }]
                })),
            )
            .unwrap();

        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        scheduler
            .start_operation(
                Arc::new(NeverSettlingTool),
                store.operation_for_request(&source_head, &call_id).unwrap(),
                AgentPath("/root".into()),
                Some(source_head.clone()),
                "never".into(),
                json!({}),
            )
            .await
            .unwrap();

        let connection = rusqlite::Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER fail_before_request_decision
                 BEFORE INSERT ON decisions
                 BEGIN SELECT RAISE(ABORT, 'injected decision persistence failure'); END;",
            )
            .unwrap();
        drop(connection);

        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            Replay {
                requests: Arc::new(Mutex::new(Vec::new())),
                turns: Mutex::new(std::collections::VecDeque::new()),
            },
            store.clone(),
            scheduler.clone(),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "decision-store-failure".into(),
                agent: child,
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let error = engine
            .run(Some(snapshot.clone()), vec![], cancel_rx, empty_mailbox())
            .await
            .unwrap_err();
        assert!(matches!(error, EngineError::Store(_)));
        assert!(store.decisions(None).unwrap().is_empty());
        let claims = store.claims_on(&snapshot).unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].state, crate::store::ClaimState::Interrupted);
        assert_eq!(
            store.request(&snapshot).unwrap().unwrap().branch,
            "/root/child"
        );

        scheduler
            .cancel(&store.claims(&call_id).unwrap()[0].operation)
            .await
            .unwrap();
        drop(engine);
        drop(store);
        std::fs::remove_file(&db_path).unwrap();
    }

    #[tokio::test]
    async fn before_request_send_persists_on_transport_failure() {
        let db_path = std::env::temp_dir().join(format!(
            "harness-before-request-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let store = Arc::new(Store::open(&db_path).unwrap());
        let plans = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new(std::collections::VecDeque::new()),
        };
        let tools = vec![json!({"type":"function","name":"distinctive"})];
        let engine = Engine::<FakeAuth, BeforeRequestRecorder, _>::with_transport(
            replay,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(BeforeRequestRecorder {
                plans: plans.clone(),
            }),
            EngineConfig {
                instructions: "instruction".into(),
                tools: tools.clone(),
                model: "test".into(),
                effort: Effort::Low,
                session_id: "before-request".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let input = Item(json!({"type":"message","role":"user","content":"hello"}));
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let error = engine
            .run(None, vec![input], cancel_rx, empty_mailbox())
            .await
            .unwrap_err();
        assert!(matches!(error, EngineError::Transport(_)));

        let request_id = {
            let sent = requests.lock().unwrap();
            assert_eq!(sent.len(), 1);
            let sent = &sent[0];
            let observed_plans = plans.lock().unwrap();
            assert_eq!(observed_plans.len(), 1);
            assert_eq!(observed_plans[0].items, sent.input);
            assert_eq!(observed_plans[0].tools_allowed.as_slice(), &*sent.tools);
            assert_eq!(observed_plans[0].effort, sent.pinned_effort);
            assert!(sent.tools.contains(&tools[0]));

            let decisions = store.decisions(None).unwrap();
            assert_eq!(decisions.len(), 1);
            let row = &decisions[0];
            let request_id = row
                .request
                .as_ref()
                .expect("decision correlated to request");
            assert_eq!(store.request(request_id).unwrap().unwrap().branch, "/root");
            assert_eq!(row.decision.hook, "before-request");
            assert_eq!(
                row.decision.event_refs,
                sent.input
                    .iter()
                    .map(|item| blake3::hash(&serde_json::to_vec(item).unwrap())
                        .to_hex()
                        .to_string())
                    .collect::<Vec<_>>()
            );
            assert_eq!(row.decision.decision, json!("Send"));
            assert_eq!(
                row.decision.evidence,
                json!({"marker":"engine-before-request"})
            );
            request_id.clone()
        };

        drop(engine);
        drop(store);
        let reopened = Store::open(&db_path).unwrap();
        assert_eq!(reopened.decisions(Some(&request_id)).unwrap().len(), 1);
        assert_eq!(
            plans.lock().unwrap().len(),
            1,
            "readback does not invoke hook"
        );
        drop(reopened);
        std::fs::remove_file(db_path).unwrap();

        let cancel_store = Arc::new(Store::memory().unwrap());
        let started = Arc::new(Notify::new());
        let engine = Engine::<FakeAuth, PausingBeforeRequest, _>::with_transport(
            Replay {
                requests: Arc::new(Mutex::new(Vec::new())),
                turns: Mutex::new(std::collections::VecDeque::new()),
            },
            cancel_store,
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(PausingBeforeRequest {
                started: started.clone(),
            }),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "cancel-before-request".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let task =
            tokio::spawn(async move { engine.run(None, vec![], cancel_rx, empty_mailbox()).await });
        started.notified().await;
        cancel_tx.send(true).unwrap();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), task)
                .await
                .expect("cancel interrupts hook")
                .unwrap(),
            Err(EngineError::Cancelled { .. })
        ));
    }

    #[tokio::test]
    async fn restricted_invalid_names_and_excluded_finalize_fail_before_transport_and_cleanup_claims()
     {
        for (selection, typed) in [
            (vec!["unknown".to_owned()], false),
            (vec!["slow".to_owned(), "slow".to_owned()], false),
            (vec![], true),
        ] {
            let (store, _source, snapshot, call_id) = inherited_claim_fixture().await;
            let requests = Arc::new(Mutex::new(Vec::new()));
            let replay = Replay {
                requests: requests.clone(),
                turns: Mutex::new(std::collections::VecDeque::new()),
            };
            let engine = Engine::<FakeAuth, RestrictedSelection, _>::with_transport(
                replay,
                store.clone(),
                Arc::new(JobScheduler::new(1).unwrap()),
                Arc::new(RestrictedSelection(selection)),
                EngineConfig {
                    instructions: "instruction".into(),
                    tools: vec![json!({"type":"function","name":"slow","strict":true})],
                    model: "test".into(),
                    effort: Effort::Low,
                    session_id: "invalid-selection".into(),
                    agent: AgentPath("/root/child".into()),
                },
            );
            let (_cancel_tx, cancel_rx) = watch::channel(false);
            let error = if typed {
                engine
                    .run_finalized::<FinalReply>(
                        Some(snapshot.clone()),
                        vec![],
                        cancel_rx,
                        empty_mailbox(),
                    )
                    .await
                    .unwrap_err()
            } else {
                engine
                    .run(Some(snapshot.clone()), vec![], cancel_rx, empty_mailbox())
                    .await
                    .unwrap_err()
            };
            assert!(matches!(error, EngineError::InvalidToolSelection(_)));
            assert!(requests.lock().unwrap().is_empty());
            let claims = store.claims_on(&snapshot).unwrap();
            assert_eq!(claims.len(), 1);
            assert_eq!(claims[0].call_id, call_id);
            assert_eq!(claims[0].state, crate::store::ClaimState::Interrupted);
        }
    }

    #[tokio::test]
    async fn injected_message_is_attempt_only_after_history_and_persists_on_transport_failure() {
        let (store, _source, snapshot, _call_id) = inherited_claim_fixture().await;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let injected = Item(json!({"type":"message","role":"user","content":"injected context"}));
        let engine = Engine::<FakeAuth, InjectSelection, _>::with_transport(
            Replay {
                requests: requests.clone(),
                turns: Mutex::new(Default::default()),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(InjectSelection(injected.clone(), Some(vec!["slow".into()]))),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![json!({"type":"function","name":"slow","strict":true})],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "inject".into(),
                agent: AgentPath("/root/child".into()),
            },
        );
        let (_tx, rx) = watch::channel(false);
        assert!(matches!(
            engine
                .run(Some(snapshot.clone()), vec![], rx, empty_mailbox())
                .await,
            Err(EngineError::Transport(_))
        ));
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].input.last(), Some(&injected));
        assert_eq!(
            sent[0]
                .input
                .iter()
                .filter(|item| **item == injected)
                .count(),
            1
        );
        let body = crate::transport::client::request_body(&sent[0]).unwrap();
        assert_eq!(body["input"].as_array().unwrap().last(), Some(&injected.0));
        assert_eq!(body["input"].as_array().unwrap().len(), sent[0].input.len());
        assert_eq!(body["tool_choice"]["tools"][0]["name"], "slow");
        assert_eq!(sent[0].tools_allowed, Some(vec!["slow".into()]));
        let row = store.decisions(None).unwrap().pop().unwrap();
        assert_eq!(row.decision.event_refs.len(), sent[0].input.len() - 1);
        assert_eq!(row.decision.evidence, json!({"inject":"opaque"}));
        assert!(
            store
                .items(&snapshot)
                .unwrap()
                .iter()
                .all(|item| item != &injected)
        );
    }

    #[tokio::test]
    async fn invalid_injected_item_fails_before_transport_and_cleans_claim() {
        let (store, _source, snapshot, call_id) = inherited_claim_fixture().await;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::<FakeAuth, InjectSelection, _>::with_transport(
            Replay {
                requests: requests.clone(),
                turns: Mutex::new(Default::default()),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(InjectSelection(
                Item(json!({
                    "type":"message","role":"assistant","content":"not user","extra":true
                })),
                None,
            )),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "invalid-inject".into(),
                agent: AgentPath("/root/child".into()),
            },
        );
        let (_tx, rx) = watch::channel(false);
        assert!(matches!(
            engine
                .run(Some(snapshot.clone()), vec![], rx, empty_mailbox())
                .await,
            Err(EngineError::InvalidInjectedItem)
        ));
        assert!(requests.lock().unwrap().is_empty());
        let claims = store.claims_on(&snapshot).unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].call_id, call_id);
        assert_eq!(claims[0].state, crate::store::ClaimState::Interrupted);
    }

    struct SetEffortProvider {
        service: crate::agent_runtime::StoreAgentToolService,
        completed: Arc<Notify>,
    }
    #[async_trait]
    impl Provider for SetEffortProvider {
        async fn call(&self, _name: &str, _args: Value) -> Result<Value, ProviderError> {
            Err(ProviderError::Tool("unexpected provider tool".into()))
        }

        async fn call_agent_verb(
            &self,
            name: &str,
            args: Value,
            context: crate::provider::CallContext,
        ) -> Result<Value, ProviderError> {
            if name != "set_effort" {
                return Err(ProviderError::Tool(
                    format!("unexpected agent verb: {name}").into(),
                ));
            }
            let effort = match args["effort"].as_str() {
                Some("low") => Effort::Low,
                Some("medium") => Effort::Medium,
                Some("high") => Effort::High,
                _ => return Err(ProviderError::Tool("invalid effort".into())),
            };
            let result = self
                .service
                .set_effort(&context.agent, effort)
                .await
                .map_err(|error| ProviderError::Tool(error.to_string().into()))?;
            self.completed.notify_one();
            Ok(result)
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    struct BlockingHereProvider {
        service: crate::agent_runtime::StoreAgentToolService,
        admitted: Arc<Notify>,
        release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }
    #[async_trait]
    impl Provider for BlockingHereProvider {
        async fn call(&self, name: &str, _args: Value) -> Result<Value, ProviderError> {
            Err(ProviderError::Tool(
                format!("unexpected provider tool: {name}").into(),
            ))
        }

        async fn call_agent_verb(
            &self,
            name: &str,
            args: Value,
            context: crate::provider::CallContext,
        ) -> Result<Value, ProviderError> {
            let request = context
                .request
                .clone()
                .ok_or_else(|| ProviderError::Tool("active request is absent".into()))?;
            let invocation = AgentInvocation {
                request,
                call_id: context.call_id,
            };
            let result =
                dispatch_agent_verb(&self.service, &context.agent, Some(&invocation), name, args)
                    .await
                    .map_err(|error| ProviderError::Tool(error.to_string().into()))?;
            self.admitted.notify_one();
            self.release
                .lock()
                .await
                .take()
                .expect("active Here result is released once")
                .await
                .map_err(|_| ProviderError::Tool("active Here release dropped".into()))?;
            Ok(result)
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    struct SlowProvider {
        started: Arc<Notify>,
        release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        released: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait]
    impl Provider for SlowProvider {
        async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
            self.started.notify_one();
            self.release
                .lock()
                .await
                .take()
                .expect("one slow invocation")
                .await
                .expect("test releases job");
            self.released
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(json!({"slow_done":true}))
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn here_fork_inherits_claim_and_delivers_late_output_before_child_resumes() {
        use crate::turn::JobOutput;

        let (store, source_head, snapshot, call_id) = inherited_claim_fixture().await;
        let started = Arc::new(Notify::new());
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let provider = Arc::new(SlowProvider {
            started: started.clone(),
            release: tokio::sync::Mutex::new(Some(release_rx)),
            released: released.clone(),
        });
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        scheduler
            .start_operation(
                provider.clone(),
                store.operation_for_request(&source_head, &call_id).unwrap(),
                AgentPath("/root".into()),
                Some(source_head.clone()),
                "slow".into(),
                json!({}),
            )
            .await
            .unwrap();
        scheduler
            .claim_exact(
                &store.operation_for_request(&source_head, &call_id).unwrap(),
                store.standalone_identity(AgentPath("/root".into())),
            )
            .await
            .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let first_turn = turn(
            "fork-pending-continuation",
            vec![Item(json!({
                "type":"function_call","call_id":"quick-call","name":"quick","arguments":"{}"
            }))],
        );
        let final_turn = turn(
            "fork-final",
            vec![Item(json!({
                "type":"message","role":"assistant","phase":"final_answer","content":"done"
            }))],
        );
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new([first_turn, final_turn].into()),
        };
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            store.clone(),
            scheduler.clone(),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "child-session".into(),
                agent: AgentPath("/root/child".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let run_head = snapshot.clone();
        let run = tokio::spawn(async move {
            engine
                .run(Some(run_head), vec![], cancel_rx, empty_mailbox())
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if requests.lock().unwrap().len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("child sends its initial async request");
        let first_input = requests.lock().unwrap()[0].input.clone();
        assert!(
            first_input.iter().any(|item| {
                item.0["type"] == "function_call" && item.0["call_id"] == call_id.0
            })
        );
        let inherited = store.claims_on(&snapshot).unwrap();
        assert_eq!(inherited.len(), 1, "claim is bound to child snapshot");
        assert_eq!(inherited[0].call_id, call_id);
        assert_eq!(inherited[0].request, snapshot);
        assert_eq!(inherited[0].state, crate::store::ClaimState::Pending);
        assert!(!first_input.iter().any(|item| {
            item.0["type"] == "function_call_output" && item.0["call_id"] == call_id.0
        }));
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .expect("shared provider job has started");
        release_tx.send(()).unwrap();
        let completion = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .expect("child resumes after inherited output")
            .unwrap()
            .unwrap();
        assert!(released.load(std::sync::atomic::Ordering::SeqCst));
        {
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(
                requests[1]
                    .input
                    .iter()
                    .filter(|item| item.0["type"] == "function_call_output"
                        && item.0["call_id"] == call_id.0)
                    .count(),
                1,
                "resumed child request receives exactly one output for the original call"
            );
        }
        assert_eq!(
            completion
                .transcript
                .iter()
                .filter(|item| item.0["type"] == "function_call_output"
                    && item.0["call_id"] == call_id.0)
                .count(),
            1,
            "child history contains the late output exactly once"
        );
        assert_eq!(
            scheduler
                .output(&store.claims(&call_id).unwrap()[0].operation)
                .await
                .unwrap(),
            Some(JobOutput::Completed(Ok(json!({"slow_done":true}))))
        );
        let claims = store.claims(&call_id).unwrap();
        assert_eq!(claims.len(), 2);
        assert!(
            claims
                .iter()
                .all(|claim| claim.state == crate::store::ClaimState::Settled)
        );
        let child_claims = store.claims_on(&snapshot).unwrap();
        assert_eq!(child_claims.len(), 1);
        assert_eq!(child_claims[0].state, crate::store::ClaimState::Settled);
        assert!(claims.iter().any(|claim| claim.request == source_head));
        assert!(claims.iter().any(|claim| claim.request == snapshot));
        let settled = scheduler
            .settled_claimants_exact(&store.claims(&call_id).unwrap()[0].operation)
            .await
            .unwrap();
        assert!(settled.contains(&store.standalone_identity(AgentPath("/root".into()))));
        assert!(settled.contains(&store.standalone_identity(AgentPath("/root/child".into()))));
    }

    #[tokio::test]
    async fn model_facing_here_spawn_waits_for_active_call_output_before_child_first_request() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let stale_head = RequestId("active-here-stale-head".into());
        store
            .write_request(
                &stale_head,
                None,
                &root.0,
                &[
                    Item(json!({"type":"message","role":"user","content":"old prefix"})),
                    Item(json!({"type":"annotation","text":"strip me"})),
                    Item(json!({"type":"dropped_claim","call_id":"old-drop"})),
                ],
                StoredUsage::default(),
            )
            .unwrap();
        let compacted_head = RequestId("active-here-compacted-head".into());
        store
            .write_compaction_request(
                &compacted_head,
                &stale_head,
                &root.0,
                &[
                    Item(json!({
                        "type":"message",
                        "role":"developer",
                        "content":"compacted prefix"
                    })),
                    Item(json!({"type":"annotation","text":"strip after compaction"})),
                    Item(json!({"type":"dropped_claim","call_id":"compact-drop"})),
                    Item::configuration_update(Effort::High),
                ],
            )
            .unwrap();
        store
            .admit_agent(
                &root,
                None,
                Some(&stale_head),
                &json!({}),
                &json!({"kind":"root"}),
            )
            .unwrap();

        let service = crate::agent_runtime::StoreAgentToolService::new(store.clone(), root.clone());
        let admitted = Arc::new(Notify::new());
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let provider = Arc::new(BlockingHereProvider {
            service,
            admitted: admitted.clone(),
            release: tokio::sync::Mutex::new(Some(release_rx)),
        });
        let spawn_call_id = "active-spawn-call";
        let contract = json!({
            "clauses":[],
            "acceptance":[],
            "owned":[],
            "must_not":[],
            "introduces":[],
            "consumes":[],
            "boundaries":[]
        });
        let spawn_item = Item(json!({
            "type":"function_call",
            "call_id":spawn_call_id,
            "name":"spawn_agent",
            "arguments":serde_json::to_string(&json!({
                "task_name":"active-child",
                "from":{"kind":"here","name":null},
                "task":contract
            })).unwrap()
        }));
        let parent_transport = Replay {
            requests: Arc::new(Mutex::new(Vec::new())),
            turns: Mutex::new(
                [
                    turn("active-here-spawn", vec![spawn_item.clone()]),
                    turn(
                        "active-here-parent-final",
                        vec![Item(json!({
                            "type":"message",
                            "role":"assistant",
                            "phase":"final_answer",
                            "content":"spawned"
                        }))],
                    ),
                    turn(
                        "active-here-parent-after-spawn",
                        vec![Item(json!({
                            "type":"message",
                            "role":"assistant",
                            "phase":"final_answer",
                            "content":"spawn output persisted"
                        }))],
                    ),
                ]
                .into(),
            ),
        };
        let parent_engine = Engine::<FakeAuth, BlockingHereProvider, _>::with_transport(
            parent_transport,
            store.clone(),
            Arc::new(JobScheduler::new(2).unwrap()),
            provider,
            EngineConfig {
                instructions: "parent".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Medium,
                session_id: "parent-session".into(),
                agent: root.clone(),
            },
        );
        let (_parent_cancel_tx, parent_cancel_rx) = watch::channel(false);
        let parent_run = tokio::spawn(async move {
            parent_engine
                .run(
                    Some(compacted_head),
                    vec![Item(json!({
                        "type":"message",
                        "role":"user",
                        "content":"active request"
                    }))],
                    parent_cancel_rx,
                    empty_mailbox(),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), admitted.notified())
            .await
            .expect("model-facing Here admission");

        let child_path = AgentPath("/root/active_child".into());
        let child = store.agent(&child_path).unwrap().unwrap();
        let snapshot = child.head_request.clone().unwrap();
        let invocation_request = RequestId(
            child.fork_source["invocation_request"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        let invocation_call = CallId(
            child.fork_source["invocation_call_id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
        assert_ne!(invocation_request.0, "active-here-stale-head");
        assert_eq!(invocation_call.0, spawn_call_id);
        assert_eq!(
            store
                .items(&invocation_request)
                .unwrap()
                .iter()
                .filter(|item| item.0["type"] == "function_call")
                .count(),
            1,
            "Engine persists the active call before routing its model-facing verb"
        );

        let child_requests = Arc::new(Mutex::new(Vec::new()));
        let child_transport = Replay {
            requests: child_requests.clone(),
            turns: Mutex::new(
                [turn(
                    "active-here-child-final",
                    vec![Item(json!({
                        "type":"message",
                        "role":"assistant",
                        "phase":"final_answer",
                        "content":"child ready"
                    }))],
                )]
                .into(),
            ),
        };
        let child_engine = Engine::<FakeAuth, Echo, _>::with_transport(
            child_transport,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "child".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "child-session".into(),
                agent: child_path,
            },
        );
        let (_child_cancel_tx, child_cancel_rx) = watch::channel(false);
        let child_run = tokio::spawn(async move {
            child_engine
                .run(Some(snapshot), vec![], child_cancel_rx, empty_mailbox())
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            child_requests.lock().unwrap().is_empty(),
            "child must not issue its first model request before spawn output is durable"
        );

        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), parent_run)
            .await
            .expect("parent Engine run")
            .unwrap()
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), child_run)
            .await
            .expect("child Engine run")
            .unwrap()
            .unwrap();

        let parent_items = store.items(&invocation_request).unwrap();
        let actual_output = parent_items
            .iter()
            .find(|item| {
                item.0["type"] == "function_call_output"
                    && item.0["call_id"].as_str() == Some(&invocation_call.0)
            })
            .unwrap()
            .clone();
        let child_requests = child_requests.lock().unwrap();
        let child_input = &child_requests[0].input;
        assert!(child_input.contains(&spawn_item));
        assert!(child_input.contains(&actual_output));
        let spawn_position = child_input
            .iter()
            .position(|item| item == &spawn_item)
            .unwrap();
        assert_eq!(child_input[spawn_position + 1], actual_output);
        assert_eq!(
            child_input[spawn_position + 2].configuration_effort(),
            Some(Effort::High),
            "fresh effort pin follows the actual spawn output"
        );
        assert_eq!(child_requests[0].pinned_effort, Effort::High);
        assert_eq!(
            child_input
                .iter()
                .filter(|item| item.is_configuration_update())
                .count(),
            1
        );
        assert_eq!(
            child_input.iter().find_map(Item::configuration_effort),
            Some(Effort::High)
        );
        assert!(!child_input.iter().any(|item| {
            matches!(
                item.0["type"].as_str(),
                Some("annotation" | "watchdog_annotation" | "dropped_claim")
            )
        }));
        assert!(child_input.iter().any(|item| {
            item.0["role"] == "developer" && item.0["content"] == "compacted prefix"
        }));
        assert!(
            !child_input
                .iter()
                .any(|item| item.0["content"] == "old prefix")
        );
        assert_eq!(
            parent_items
                .iter()
                .filter(|item| {
                    item.0["type"] == "function_call"
                        && item.0["call_id"].as_str() == Some(spawn_call_id)
                })
                .count(),
            1,
            "streamed function_call is not duplicated after response completion"
        );
    }

    #[tokio::test]
    async fn here_child_waits_between_claim_settlement_and_persisted_spawn_output() {
        let store = Arc::new(Store::memory().unwrap());
        let parent = AgentPath("/root".into());
        let child = AgentPath("/root/race_child".into());
        let invocation_request = RequestId("here-output-race-parent".into());
        let snapshot = RequestId("here-output-race-snapshot".into());
        let call_id = CallId("here-output-race-call".into());
        let other_call_id = CallId("here-output-race-other-call".into());
        let other_call_item = Item(json!({
            "type":"function_call",
            "call_id":other_call_id.0,
            "name":"slow",
            "arguments":"{}"
        }));
        let spawn_item = Item(json!({
            "type":"function_call",
            "call_id":call_id.0,
            "name":"spawn_agent",
            "arguments":"{}"
        }));
        store
            .create_request(&invocation_request, None, &parent.0)
            .unwrap();
        store.set_effort(&invocation_request, Effort::High).unwrap();
        store
            .append_items(
                &invocation_request,
                &[other_call_item.clone(), spawn_item.clone()],
            )
            .unwrap();
        store
            .admit_agent(
                &parent,
                None,
                Some(&invocation_request),
                &json!({}),
                &json!({"kind":"root"}),
            )
            .unwrap();
        store.claim(&other_call_id, &invocation_request).unwrap();
        store.claim(&call_id, &invocation_request).unwrap();
        store
            .admit_here_agent_from_invocation(
                &child,
                &parent,
                &snapshot,
                &invocation_request,
                &call_id,
                &json!({}),
                &parent.0,
                &child.0,
                "AtBoundary",
                &Item(json!({"type":"message","role":"assistant","content":"NEW_TASK"})),
            )
            .unwrap();

        let started = Arc::new(Notify::new());
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        scheduler
            .start_operation(
                Arc::new(SlowProvider {
                    started: started.clone(),
                    release: tokio::sync::Mutex::new(Some(release_rx)),
                    released: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                }),
                store
                    .operation_for_request(&invocation_request, &other_call_id)
                    .unwrap(),
                parent.clone(),
                Some(invocation_request.clone()),
                "slow".into(),
                json!({}),
            )
            .await
            .unwrap();
        scheduler
            .claim_exact(
                &store
                    .operation_for_request(&invocation_request, &other_call_id)
                    .unwrap(),
                store.standalone_identity(parent.clone()),
            )
            .await
            .unwrap();

        let requests = Arc::new(Mutex::new(Vec::new()));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new(
                [
                    turn(
                        "here-output-race-provisional",
                        vec![Item(json!({
                            "type":"message",
                            "role":"assistant",
                            "phase":"final_answer",
                            "content":"waiting for other pending call"
                        }))],
                    ),
                    turn(
                        "here-output-race-final",
                        vec![Item(json!({
                            "type":"message",
                            "role":"assistant",
                            "phase":"final_answer",
                            "content":"ready"
                        }))],
                    ),
                ]
                .into(),
            ),
        };
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            store.clone(),
            scheduler,
            Arc::new(Echo),
            EngineConfig {
                instructions: "child".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "here-output-race-child".into(),
                agent: child,
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let child_snapshot = snapshot.clone();
        let child_run = tokio::spawn(async move {
            engine
                .run(Some(child_snapshot), vec![], cancel_rx, empty_mailbox())
                .await
        });

        let output = Item(json!({
            "type":"function_call_output",
            "call_id":call_id.0,
            "output":"{\"spawned\":true}"
        }));
        assert_eq!(
            store
                .write_output(
                    &store.claims(&call_id).unwrap()[0].operation,
                    &output,
                    crate::store::TerminalOutcome::Success
                )
                .unwrap(),
            2
        );
        let claims = store.claims(&call_id).unwrap();
        assert_eq!(claims.len(), 2, "parent and child claims both settle");
        assert!(
            claims
                .iter()
                .all(|claim| claim.state == crate::store::ClaimState::Settled)
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            requests.lock().unwrap().is_empty(),
            "settled claim alone must not release the child before its output Item is persisted"
        );

        store
            .append_items(&invocation_request, std::slice::from_ref(&output))
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .expect("second inherited call starts");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if requests.lock().unwrap().len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("Here child starts while the second call remains pending");
        let first_input = requests.lock().unwrap()[0].input.clone();
        assert!(first_input.contains(&other_call_item));
        assert!(!first_input.iter().any(|item| {
            item.0["type"] == "function_call_output"
                && item.0["call_id"].as_str() == Some(&other_call_id.0)
        }));
        assert!(
            store
                .claims(&other_call_id)
                .unwrap()
                .iter()
                .all(|claim| claim.state == crate::store::ClaimState::Pending),
            "the second active-call claim remains pending at the child's first request"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            !child_run.is_finished(),
            "child waits for the second pending call after its first final response"
        );
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), child_run)
            .await
            .expect("child wakes after invocation output persistence")
            .unwrap()
            .unwrap();
        assert!(
            store
                .copy_call_output_if_persisted(&invocation_request, &snapshot, &call_id)
                .unwrap()
        );
        let child_requests = requests.lock().unwrap();
        let child_input = &child_requests[0].input;
        assert_eq!(child_requests.len(), 2);
        assert_eq!(
            child_requests[1]
                .input
                .iter()
                .filter(|item| {
                    item.0["type"] == "function_call_output" && item.0["call_id"] == other_call_id.0
                })
                .count(),
            1,
            "the second active call's output arrives on the resumed request"
        );
        let spawn_position = child_input
            .iter()
            .position(|item| item == &spawn_item)
            .unwrap();
        assert_eq!(child_input[spawn_position + 1], output);
        assert_eq!(
            child_input[spawn_position + 2].configuration_effort(),
            Some(Effort::High)
        );
        assert_eq!(child_requests[0].pinned_effort, Effort::High);
        assert_eq!(
            child_input
                .iter()
                .filter(|item| {
                    item.0["type"] == "function_call_output" && item.0["call_id"] == call_id.0
                })
                .count(),
            1,
            "retrying the copy does not duplicate the output"
        );
    }

    #[tokio::test]
    async fn settled_inherited_claim_is_replayed_from_store_before_child_first_request() {
        let (store, _source_head, snapshot, call_id) = inherited_claim_fixture().await;
        let output = Item(json!({
            "type":"function_call_output",
            "call_id":call_id.0,
            "output":"{\"settled_before_start\":true}"
        }));
        store
            .write_output(
                &store.claims(&call_id).unwrap()[0].operation,
                &output,
                crate::store::TerminalOutcome::Success,
            )
            .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new(
                [turn(
                    "settled-fork-final",
                    vec![Item(json!({
                        "type":"message","role":"assistant","phase":"final_answer","content":"done"
                    }))],
                )]
                .into(),
            ),
        };
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            store,
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "child-session".into(),
                agent: AgentPath("/root/child".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let completion = engine
            .run(Some(snapshot), vec![], cancel_rx, empty_mailbox())
            .await
            .unwrap();
        let requests = requests.lock().unwrap();
        let first_input = &requests[0].input;
        let output_position = first_input.iter().position(|item| {
            item.0["type"] == "function_call_output"
                && item.0["call_id"] == call_id.0
                && item.0["output"] == output.0["output"]
        });
        let task_position = first_input.iter().position(|item| {
            item.0["type"] == "message"
                && item.0["content"].as_array().is_some_and(|content| {
                    content.iter().any(|part| {
                        part["text"]
                            .as_str()
                            .is_some_and(|text| text.contains("NEW_TASK"))
                    })
                })
        });
        assert!(output_position.is_some());
        assert!(task_position.is_some());
        assert!(output_position.unwrap() < task_position.unwrap());
        assert!(completion.transcript.contains(&output));
    }

    #[tokio::test]
    async fn forked_wait_agent_claim_fails_closed_before_model_request() {
        let store = Arc::new(Store::memory().unwrap());
        let parent = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        let source_head = RequestId("wait-parent-head".into());
        let snapshot = RequestId("wait-child-snapshot".into());
        let call_id = CallId("inherited-wait".into());
        store.create_request(&source_head, None, &parent.0).unwrap();
        store
            .append_items(
                &source_head,
                &[Item(json!({
                    "type":"function_call",
                    "call_id":call_id.0,
                    "name":"wait_agent",
                    "arguments":"{}"
                }))],
            )
            .unwrap();
        store.set_effort(&source_head, Effort::Low).unwrap();
        store
            .admit_agent(
                &parent,
                None,
                Some(&source_head),
                &json!({}),
                &json!({"kind":"root"}),
            )
            .unwrap();
        store.claim(&call_id, &source_head).unwrap();
        store
            .admit_here_agent_with_snapshot(
                &child,
                &parent,
                &snapshot,
                &json!({}),
                &parent.0,
                &child.0,
                "AtBoundary",
                &Item(json!({
                    "type":"message","role":"assistant","content":[{
                        "type":"output_text","text":"NEW_TASK"
                    }]
                })),
            )
            .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            Replay {
                requests: requests.clone(),
                turns: Mutex::new([turn(
                    "must-not-run",
                    vec![Item(json!({
                        "type":"message","role":"assistant","phase":"final_answer","content":"unexpected"
                    }))],
                )]
                .into()),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "child-session".into(),
                agent: child,
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        assert!(matches!(
            engine
                .run(Some(snapshot.clone()), vec![], cancel_rx, empty_mailbox())
                .await,
            Err(EngineError::UnresumableForkedWaitAgent)
        ));
        assert!(requests.lock().unwrap().is_empty());
        assert_eq!(store.claims_on(&snapshot).unwrap()[0].request, snapshot);
    }

    #[tokio::test]
    async fn child_cleanup_interrupts_only_its_inherited_claim_not_the_parent_job() {
        use crate::store::ClaimState;

        let (store, source_head, snapshot, call_id) = inherited_claim_fixture().await;
        let provider = Arc::new(PendingProvider);
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        scheduler
            .start_operation(
                provider.clone(),
                store.operation_for_request(&source_head, &call_id).unwrap(),
                AgentPath("/root".into()),
                Some(source_head.clone()),
                "slow".into(),
                json!({}),
            )
            .await
            .unwrap();
        scheduler
            .claim_exact(
                &store.operation_for_request(&source_head, &call_id).unwrap(),
                store.standalone_identity(AgentPath("/root".into())),
            )
            .await
            .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new(
                [turn(
                    "fork-cancel",
                    vec![Item(json!({
                        "type":"message","role":"assistant","phase":"final_answer","content":"done"
                    }))],
                )]
                .into(),
            ),
        };
        let engine = Engine::<FakeAuth, PendingProvider, _>::with_transport(
            replay,
            store.clone(),
            scheduler.clone(),
            provider,
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "child-session".into(),
                agent: AgentPath("/root/child".into()),
            },
        );
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let run_head = snapshot.clone();
        let run = tokio::spawn(async move {
            engine
                .run(Some(run_head), vec![], cancel_rx, empty_mailbox())
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !requests.lock().unwrap().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("child begins with the inherited call");
        cancel_tx.send(true).unwrap();
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), run)
                .await
                .expect("child cleanup returns")
                .unwrap(),
            Err(EngineError::Cancelled { .. })
        ));
        assert_eq!(
            scheduler
                .output(&store.claims(&call_id).unwrap()[0].operation)
                .await
                .unwrap(),
            None
        );
        let claims = store.claims(&call_id).unwrap();
        assert_eq!(claims.len(), 2);
        assert!(
            claims
                .iter()
                .any(|claim| claim.request == source_head && claim.state == ClaimState::Pending)
        );
        assert!(
            claims
                .iter()
                .any(|claim| claim.request == snapshot && claim.state == ClaimState::Interrupted)
        );
    }

    struct AsyncContinuation {
        requests: Arc<Mutex<Vec<ResponsesRequest>>>,
        started: Arc<Notify>,
        second_request: Arc<Notify>,
    }
    #[async_trait::async_trait]
    impl ResponsesTransport for AsyncContinuation {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            unreachable!("streaming path expected")
        }

        async fn create_streaming(
            &self,
            request: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            let index = {
                let mut requests = self.requests.lock().unwrap();
                let index = requests.len();
                requests.push(request);
                index
            };
            match index {
                0 => {
                    let call = Item(json!({
                        "type":"function_call","call_id":"slow-call","name":"slow","arguments":"{}"
                    }));
                    sink.send(StreamEvent::ItemDone(call.clone()))
                        .await
                        .map_err(|_| {
                            TransportError::Stream("engine event receiver closed".into())
                        })?;
                    tokio::time::timeout(
                        std::time::Duration::from_secs(2),
                        self.started.notified(),
                    )
                    .await
                    .map_err(|_| TransportError::Stream("slow provider never began".into()))?;
                    Ok(turn("async-one", vec![call]))
                }
                1 => {
                    self.second_request.notify_one();
                    let wait = Item(json!({
                        "type":"function_call","call_id":"wait-call","name":"wait_agent","arguments":"{}"
                    }));
                    sink.send(StreamEvent::ItemDone(wait.clone()))
                        .await
                        .map_err(|_| {
                            TransportError::Stream("engine event receiver closed".into())
                        })?;
                    Ok(turn("async-wait", vec![wait]))
                }
                2 => {
                    let final_item = Item(json!({
                        "type":"message","role":"assistant","phase":"final_answer","content":"done"
                    }));
                    sink.send(StreamEvent::ItemDone(final_item.clone()))
                        .await
                        .map_err(|_| {
                            TransportError::Stream("engine event receiver closed".into())
                        })?;
                    Ok(turn("async-final", vec![final_item]))
                }
                _ => Err(TransportError::Stream("unexpected replay request".into())),
            }
        }
    }

    struct LateFinal {
        requests: Arc<Mutex<Vec<ResponsesRequest>>>,
        started: Arc<Notify>,
        response_finished: Arc<Notify>,
        second_request: Arc<Notify>,
    }
    #[async_trait::async_trait]
    impl ResponsesTransport for LateFinal {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            unreachable!("streaming path expected")
        }

        async fn create_streaming(
            &self,
            request: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            let index = {
                let mut requests = self.requests.lock().unwrap();
                let index = requests.len();
                requests.push(request);
                index
            };
            if index == 0 {
                let call = Item(json!({
                    "type":"function_call","call_id":"late-call","name":"slow","arguments":"{}"
                }));
                let final_item = Item(json!({
                    "type":"message","role":"assistant","phase":"final_answer","content":"provisional"
                }));
                sink.send(StreamEvent::ItemDone(call.clone()))
                    .await
                    .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
                sink.send(StreamEvent::ItemDone(final_item.clone()))
                    .await
                    .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
                tokio::time::timeout(std::time::Duration::from_secs(2), self.started.notified())
                    .await
                    .map_err(|_| TransportError::Stream("slow provider never began".into()))?;
                self.response_finished.notify_one();
                Ok(turn("late-provisional", vec![call, final_item]))
            } else if index == 1 {
                self.second_request.notify_one();
                let final_item = Item(json!({
                    "type":"message","role":"assistant","phase":"final_answer","content":"continued"
                }));
                sink.send(StreamEvent::ItemDone(final_item.clone()))
                    .await
                    .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
                Ok(turn("late-continued", vec![final_item]))
            } else {
                Err(TransportError::Stream("unexpected replay request".into()))
            }
        }
    }

    struct WaitEnvelopeTransport {
        requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    }
    #[async_trait::async_trait]
    impl ResponsesTransport for WaitEnvelopeTransport {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            unreachable!("streaming path expected")
        }

        async fn create_streaming(
            &self,
            request: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            let index = {
                let mut requests = self.requests.lock().unwrap();
                let index = requests.len();
                requests.push(request);
                index
            };
            let item = if index == 0 {
                Item(json!({
                    "type":"function_call","call_id":"wait-envelope","name":"wait_agent","arguments":"{}"
                }))
            } else {
                Item(json!({
                    "type":"message","role":"assistant","phase":"final_answer","content":"resumed"
                }))
            };
            sink.send(StreamEvent::ItemDone(item.clone()))
                .await
                .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
            Ok(turn(
                if index == 0 { "wait" } else { "resumed" },
                vec![item],
            ))
        }
    }

    struct QueueInboxBeforeContinuation {
        requests: Arc<Mutex<Vec<ResponsesRequest>>>,
        store: Arc<Store>,
        recipient: AgentPath,
        inbox_item: Item,
    }
    #[async_trait::async_trait]
    impl ResponsesTransport for QueueInboxBeforeContinuation {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            unreachable!("streaming path expected")
        }

        async fn create_streaming(
            &self,
            request: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            let index = {
                let mut requests = self.requests.lock().unwrap();
                let index = requests.len();
                requests.push(request);
                index
            };
            let items = if index == 0 {
                self.store
                    .add_envelope(
                        "/root/child",
                        &self.recipient.0,
                        "AtBoundary",
                        &self.inbox_item,
                        None,
                    )
                    .map_err(|error| TransportError::Stream(error.to_string()))?;
                vec![Item(json!({
                    "type":"function_call","call_id":"queued-continuation","name":"echo","arguments":"{}"
                }))]
            } else {
                vec![Item(json!({
                    "type":"message","role":"assistant","phase":"final_answer","content":"done"
                }))]
            };
            for item in &items {
                sink.send(StreamEvent::ItemDone(item.clone()))
                    .await
                    .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
            }
            Ok(turn(
                if index == 0 {
                    "queued-before-next"
                } else {
                    "queued-next"
                },
                items,
            ))
        }
    }

    struct GateProvider {
        started: Arc<Notify>,
        response_completed: Arc<std::sync::atomic::AtomicBool>,
        started_early: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait]
    impl Provider for GateProvider {
        async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
            self.started_early.store(
                !self
                    .response_completed
                    .load(std::sync::atomic::Ordering::SeqCst),
                std::sync::atomic::Ordering::SeqCst,
            );
            self.started.notify_one();
            Ok(json!({"ok":true}))
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    struct ControlledStream {
        requests: Arc<Mutex<Vec<ResponsesRequest>>>,
        started: Arc<Notify>,
        response_completed: Arc<std::sync::atomic::AtomicBool>,
    }
    #[async_trait::async_trait]
    impl ResponsesTransport for ControlledStream {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            Err(TransportError::Stream(
                "controlled transport must use streaming".into(),
            ))
        }

        async fn create_streaming(
            &self,
            request: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            let turn_index = {
                let mut requests = self.requests.lock().unwrap();
                let index = requests.len();
                requests.push(request);
                index
            };
            if turn_index == 0 {
                let call = Item(json!({
                    "type":"function_call","call_id":"gate-call","name":"echo","arguments":"{}"
                }));
                sink.send(StreamEvent::ItemDone(call.clone()))
                    .await
                    .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
                tokio::time::timeout(std::time::Duration::from_secs(2), self.started.notified())
                    .await
                    .map_err(|_| {
                        TransportError::Stream("tool did not start before completion".into())
                    })?;
                self.response_completed
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(turn("gated-call", vec![call]))
            } else {
                let final_item = Item(json!({
                    "type":"message","role":"assistant","phase":"final_answer","content":"done"
                }));
                sink.send(StreamEvent::ItemDone(final_item.clone()))
                    .await
                    .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
                Ok(turn("gated-final", vec![final_item]))
            }
        }
    }

    fn turn(id: &str, items: Vec<Item>) -> ResponsesTurn {
        ResponsesTurn {
            response_id: id.into(),
            items,
            usage: Usage {
                reported: true,
                input_tokens: 3,
                output_tokens: 2,
                cached_tokens: 1,
                cache_write_tokens: 0,
            },
        }
    }

    struct PendingAProvider {
        a_started: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        b_started: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release_b: tokio::sync::Mutex<tokio::sync::oneshot::Receiver<()>>,
    }

    #[async_trait]
    impl Provider for PendingAProvider {
        async fn call(&self, name: &str, _args: Value) -> Result<Value, ProviderError> {
            match name {
                "A" => {
                    if let Some(started) = self.a_started.lock().await.take() {
                        let _ = started.send(());
                    }
                    std::future::pending().await
                }
                "B" => {
                    if let Some(started) = self.b_started.lock().await.take() {
                        let _ = started.send(());
                    }
                    let mut release = self.release_b.lock().await;
                    (&mut *release)
                        .await
                        .map_err(|_| ProviderError::Tool("B release dropped".into()))?;
                    Ok(json!({"original":"B-result"}))
                }
                _ => Err(ProviderError::Tool(
                    format!("unexpected tool {name}").into(),
                )),
            }
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    struct DeliverBWhileAPending {
        requests: Arc<Mutex<Vec<ResponsesRequest>>>,
        scheduler: Arc<JobScheduler>,
        store: Arc<Store>,
        a_started: tokio::sync::Mutex<tokio::sync::oneshot::Receiver<()>>,
        b_started: tokio::sync::Mutex<tokio::sync::oneshot::Receiver<()>>,
        release_b: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        third_request: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl ResponsesTransport for DeliverBWhileAPending {
        async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            let index = {
                let mut requests = self.requests.lock().unwrap();
                let index = requests.len();
                requests.push(request);
                index
            };
            match index {
                0 => Ok(turn(
                    "a-and-b",
                    vec![
                        Item(json!({
                            "type":"function_call","call_id":"call-A","name":"A","arguments":"{}"
                        })),
                        Item(json!({
                            "type":"function_call","call_id":"call-B","name":"B","arguments":"{}"
                        })),
                    ],
                )),
                1 => {
                    let mut a_started = self.a_started.lock().await;
                    tokio::time::timeout(std::time::Duration::from_secs(2), &mut *a_started)
                        .await
                        .map_err(|_| TransportError::Stream("A did not start".into()))?
                        .map_err(|_| TransportError::Stream("A start signal dropped".into()))?;
                    let mut b_started = self.b_started.lock().await;
                    tokio::time::timeout(std::time::Duration::from_secs(2), &mut *b_started)
                        .await
                        .map_err(|_| TransportError::Stream("B did not start".into()))?
                        .map_err(|_| TransportError::Stream("B start signal dropped".into()))?;
                    self.release_b
                        .lock()
                        .await
                        .take()
                        .ok_or_else(|| TransportError::Stream("B already released".into()))?
                        .send(())
                        .map_err(|_| TransportError::Stream("B job stopped".into()))?;
                    tokio::time::timeout(std::time::Duration::from_secs(2), async {
                        loop {
                            if self
                                .scheduler
                                .output(
                                    &self.store.claims(&CallId("call-B".into())).unwrap()[0]
                                        .operation,
                                )
                                .await
                                .ok()
                                .flatten()
                                .is_some()
                            {
                                break;
                            }
                            tokio::task::yield_now().await;
                        }
                    })
                    .await
                    .map_err(|_| TransportError::Stream("B did not settle".into()))?;
                    Ok(turn(
                        "intermediate",
                        vec![Item(json!({
                            "type":"message","role":"assistant","content":"continue"
                        }))],
                    ))
                }
                2 => {
                    self.third_request.notify_one();
                    Ok(turn(
                        "final",
                        vec![Item(json!({
                            "type":"message","role":"assistant","phase":"final_answer","content":"done"
                        }))],
                    ))
                }
                _ => Err(TransportError::Stream("unexpected extra request".into())),
            }
        }
    }

    #[tokio::test]
    async fn wave18_async_b_delivered_while_a_pending() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let scheduler = Arc::new(JobScheduler::new(2).unwrap());
        let (a_started_tx, a_started_rx) = tokio::sync::oneshot::channel();
        let (b_started_tx, b_started_rx) = tokio::sync::oneshot::channel();
        let (release_b_tx, release_b_rx) = tokio::sync::oneshot::channel();
        let third_request = Arc::new(Notify::new());
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, PendingAProvider, _>::with_transport(
            DeliverBWhileAPending {
                requests: requests.clone(),
                scheduler: scheduler.clone(),
                store: store.clone(),
                a_started: tokio::sync::Mutex::new(a_started_rx),
                b_started: tokio::sync::Mutex::new(b_started_rx),
                release_b: tokio::sync::Mutex::new(Some(release_b_tx)),
                third_request: third_request.clone(),
            },
            store.clone(),
            scheduler.clone(),
            Arc::new(PendingAProvider {
                a_started: tokio::sync::Mutex::new(Some(a_started_tx)),
                b_started: tokio::sync::Mutex::new(Some(b_started_tx)),
                release_b: tokio::sync::Mutex::new(release_b_rx),
            }),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "wave18-async-b".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let run = tokio::spawn(async move {
            engine
                .run(
                    None,
                    vec![Item(json!({"role":"user","content":"go"}))],
                    cancel_rx,
                    empty_mailbox(),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), third_request.notified())
            .await
            .expect("model received another request with B settled and A pending");
        assert_eq!(
            scheduler
                .output(&store.claims(&CallId("call-A".into())).unwrap()[0].operation)
                .await
                .unwrap(),
            None,
            "A is still pending when B is supplied"
        );
        cancel_tx.send(true).unwrap();
        assert!(matches!(
            run.await.unwrap(),
            Err(EngineError::Cancelled { .. })
        ));
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        let delivered = requests[2]
            .input
            .iter()
            .filter(|item| {
                item.0["type"] == "function_call_output" && item.0["call_id"] == "call-B"
            })
            .collect::<Vec<_>>();
        assert_eq!(
            delivered.len(),
            1,
            "B's original output is delivered exactly once"
        );
        assert_eq!(
            serde_json::from_str::<Value>(delivered[0].0["output"].as_str().unwrap()).unwrap()["original"],
            "B-result"
        );
        assert!(
            !requests[2].input.iter().any(|item| {
                item.0["type"] == "function_call_output" && item.0["call_id"] == "call-A"
            }),
            "A remains unanswered while B is delivered"
        );
    }

    struct OutputRecorder(tokio::sync::mpsc::UnboundedSender<ModelOutput>);
    impl ModelOutputObserver for OutputRecorder {
        fn observe(&self, output: ModelOutput) {
            self.0.send(output).unwrap();
        }
    }
    struct PartialForever;
    #[async_trait::async_trait]
    impl ResponsesTransport for PartialForever {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            unreachable!()
        }
        async fn create_streaming(
            &self,
            _: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            sink.send(StreamEvent::Delta {
                item_id: "partial".into(),
                channel: OutputChannel::Assistant,
                index: 0,
                text: "incomplete".into(),
            })
            .await
            .unwrap();
            std::future::pending().await
        }
    }
    #[tokio::test]
    async fn live_output_provider_cancellation_stops_incomplete_partial() {
        let (outputs, mut received) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            PartialForever,
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "partial-test".into(),
                agent: AgentPath("/root".into()),
            },
        )
        .with_output_observer(Arc::new(OutputRecorder(outputs)));
        let (cancel, cancellation) = watch::channel(false);
        let running = tokio::spawn(async move {
            engine
                .run(None, vec![], cancellation, empty_mailbox())
                .await
        });
        let scope = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let output = received.recv().await.unwrap();
                if matches!(output.update, ModelOutputUpdate::Delta { .. }) {
                    break output.request_id;
                }
            }
        })
        .await
        .unwrap();
        cancel.send_replace(true);
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(2), running)
                .await
                .unwrap()
                .unwrap(),
            Err(EngineError::Cancelled { .. })
        ));
        assert!(
            matches!(received.recv().await.unwrap(),ModelOutput {request_id,update:ModelOutputUpdate::Stopped,..} if request_id == scope)
        );
    }
    struct OutputBeforeTool {
        stream_release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }
    #[async_trait::async_trait]
    impl ResponsesTransport for OutputBeforeTool {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            unreachable!()
        }
        async fn create_streaming(
            &self,
            _: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            let release = self.stream_release.lock().await.take();
            if let Some(release) = release {
                sink.send(StreamEvent::Delta {
                    item_id: "assistant-live".into(),
                    channel: OutputChannel::Assistant,
                    index: 0,
                    text: "Before tool".into(),
                })
                .await
                .unwrap();
                let message = Item(
                    json!({"id":"assistant-live","type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"Before tool"}]}),
                );
                let call = Item(
                    json!({"id":"tool-live","type":"function_call","call_id":"slow-live","name":"slow","arguments":"{}"}),
                );
                sink.send(StreamEvent::ItemDone(message.clone()))
                    .await
                    .unwrap();
                sink.send(StreamEvent::ItemDone(call.clone()))
                    .await
                    .unwrap();
                release.await.unwrap();
                Ok(turn("streamed", vec![message, call]))
            } else {
                Ok(turn(
                    "finished",
                    vec![Item(
                        json!({"id":"final-live","type":"message","role":"assistant","phase":"final_answer","content":"After tool"}),
                    )],
                ))
            }
        }
    }
    #[tokio::test]
    async fn live_output_is_visible_before_provider_and_tool_complete() {
        let (stream_release, stream_wait) = tokio::sync::oneshot::channel();
        let (tool_release, tool_wait) = tokio::sync::oneshot::channel();
        let started = Arc::new(Notify::new());
        let provider = Arc::new(SlowProvider {
            started: started.clone(),
            release: tokio::sync::Mutex::new(Some(tool_wait)),
            released: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let (outputs, mut received) = tokio::sync::mpsc::unbounded_channel();
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, SlowProvider, _>::with_transport(
            OutputBeforeTool {
                stream_release: tokio::sync::Mutex::new(Some(stream_wait)),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            provider,
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "live-test".into(),
                agent: AgentPath("/root".into()),
            },
        )
        .with_output_observer(Arc::new(OutputRecorder(outputs)));
        let (_cancel, cancellation) = watch::channel(false);
        let running = tokio::spawn(async move {
            engine
                .run(None, vec![], cancellation, empty_mailbox())
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        assert!(
            !running.is_finished(),
            "provider and tool gates remain closed"
        );
        let mut delta = None;
        let committed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let output = received.recv().await.unwrap();
                match &output.update {
                    ModelOutputUpdate::Delta { text, item_id, .. } => {
                        assert_eq!(text, "Before tool");
                        assert_eq!(item_id, "assistant-live");
                        delta = Some(output.request_id);
                    }
                    ModelOutputUpdate::Committed {
                        item_id: Some(id),
                        hash,
                    } if id == "assistant-live" => break (output.request_id, hash.clone()),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(delta.as_ref(), Some(&committed.0));
        assert_eq!(
            store.get_item(&committed.1).unwrap().unwrap().0["id"],
            "assistant-live"
        );
        assert!(
            store
                .items(&committed.0)
                .unwrap()
                .iter()
                .any(|i| i.0["id"] == "assistant-live")
        );
        tool_release.send(()).unwrap();
        stream_release.send(()).unwrap();
        let completion = tokio::time::timeout(std::time::Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            completion
                .transcript
                .iter()
                .any(|i| i.0["type"] == "function_call_output")
        );
        let tool_output = completion
            .transcript
            .iter()
            .find(|i| i.0["type"] == "function_call_output")
            .unwrap();
        let tool_hash = blake3::hash(&serde_json::to_vec(tool_output).unwrap())
            .to_hex()
            .to_string();
        let mut saw_tool_commit = false;
        while let Ok(output) = received.try_recv() {
            if matches!(output.update, ModelOutputUpdate::Committed {hash, ..} if hash.0 == tool_hash)
            {
                saw_tool_commit = true;
            }
        }
        assert!(
            saw_tool_commit,
            "tool output storage also invalidates retained history"
        );
    }

    #[tokio::test]
    async fn engine_compaction_installs_new_window_before_next_response() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let call = Item(json!({
            "type":"function_call","call_id":"compact-call","name":"echo","arguments":"{}"
        }));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new(
                [
                    turn("before-compact", vec![call]),
                    turn(
                        "server-compact",
                        vec![Item(json!({"type":"compaction","encrypted_content":"opaque"}))],
                    ),
                    turn(
                        "after-compact",
                        vec![Item(json!({
                            "type":"message","role":"assistant","phase":"final_answer","content":"done"
                        }))],
                    ),
                ]
                .into(),
            ),
        };
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Medium,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        )
        .with_compaction_threshold(3);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let completion = engine
            .run(
                None,
                vec![Item(
                    json!({"type":"message","role":"user","content":"keep"}),
                )],
                cancel_rx,
                empty_mailbox(),
            )
            .await
            .unwrap();
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 3);
        assert_eq!(
            sent[1].input.last().unwrap().0["type"],
            "compaction_trigger"
        );
        assert!(!sent[1].input.iter().any(Item::is_configuration_update));
        assert_eq!(sent[2].input[0].0["role"], "developer");
        assert!(
            sent[2]
                .input
                .iter()
                .any(|item| item.0["type"] == "compaction")
        );
        assert_eq!(
            sent[2].input.last().unwrap().configuration_effort(),
            Some(Effort::Medium)
        );
        assert!(
            sent[2]
                .input
                .iter()
                .any(|item| item.0["role"] == "user" && item.0["content"] == "keep")
        );
        assert!(
            !sent[2]
                .input
                .iter()
                .any(|item| item.0["call_id"] == "compact-call")
        );
        assert_eq!(
            completion.transcript,
            sent[2]
                .input
                .iter()
                .cloned()
                .chain(completion.turn.items)
                .collect::<Vec<_>>()
        );
        let compact_request = completion.head_request;
        assert!(store.is_compaction_boundary(&compact_request).unwrap());
        assert!(
            store
                .request(&compact_request)
                .unwrap()
                .unwrap()
                .parent
                .is_some()
        );
    }

    #[tokio::test]
    async fn engine_compaction_carries_unanswered_call_and_late_output() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let provider = Arc::new(SlowProvider {
            started: Arc::new(Notify::new()),
            release: tokio::sync::Mutex::new(Some(release_rx)),
            released: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let call = Item(json!({
            "type":"function_call","call_id":"carried-slow","name":"slow","arguments":"{}"
        }));
        let mut provisional = turn(
            "provisional",
            vec![Item(json!({
                "type":"message","role":"assistant","phase":"final_answer","content":"before output"
            }))],
        );
        provisional.usage.input_tokens = 0;
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new(
                [
                    turn("pending", vec![call.clone()]),
                    turn(
                        "compact",
                        vec![Item(json!({"type":"compaction","encrypted_content":"opaque"}))],
                    ),
                    provisional,
                    turn(
                        "final",
                        vec![Item(json!({
                            "type":"message","role":"assistant","phase":"final_answer","content":"after output"
                        }))],
                    ),
                ]
                .into(),
            ),
        };
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, SlowProvider, _>::with_transport(
            replay,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            provider,
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        )
        .with_compaction_threshold(3);
        let release_requests = requests.clone();
        tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    if release_requests.lock().unwrap().len() >= 3 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("successor request after compaction");
            release_tx.send(()).expect("slow job still pending");
        });
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let completion = engine
            .run(None, vec![], cancel_rx, empty_mailbox())
            .await
            .unwrap();
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 3);
        assert!(sent[2].input.iter().any(|item| item == &call));
        assert!(completion.transcript.iter().any(|item| {
            item.0["type"] == "function_call_output" && item.0["call_id"] == "carried-slow"
        }));
        assert!(completion.transcript.iter().any(|item| item == &call));
        assert_eq!(store.pending_at(&completion.head_request).unwrap().len(), 0);
    }

    #[tokio::test]
    async fn engine_compaction_failure_cleans_pending_claim_without_installing_window() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (_release_tx, release_rx) = tokio::sync::oneshot::channel();
        let provider = Arc::new(SlowProvider {
            started: Arc::new(Notify::new()),
            release: tokio::sync::Mutex::new(Some(release_rx)),
            released: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new(
                [
                    turn(
                        "pending",
                        vec![Item(json!({
                            "type":"function_call","call_id":"failed-compact","name":"slow","arguments":"{}"
                        }))],
                    ),
                    turn(
                        "not-a-compaction",
                        vec![Item(json!({"type":"message","role":"assistant","content":"wrong"}))],
                    ),
                ]
                .into(),
            ),
        };
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, SlowProvider, _>::with_transport(
            replay,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            provider,
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        )
        .with_compaction_threshold(3);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let error = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            engine.run(None, vec![], cancel_rx, empty_mailbox()),
        )
        .await
        .expect("compaction failure must not wait on job")
        .unwrap_err();
        assert!(matches!(error, EngineError::Compact(_)));
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert!(store.recover_pending().unwrap().is_empty());
        let claims = store.claims(&CallId("failed-compact".into())).unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].state, crate::store::ClaimState::Settled);
        let output = store
            .get_item(&claims[0].output.clone().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(output.0["output"].as_str().unwrap()).unwrap(),
            json!({"error":"job cancelled"})
        );
    }

    #[tokio::test]
    async fn pending_set_effort_follows_tool_output_before_next_model_request() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (release_first_turn, release_first_turn_rx) = tokio::sync::oneshot::channel();
        let replay = EffortSettlementBarrier {
            replay: Replay {
                requests: requests.clone(),
                turns: Mutex::new(
                    [
                        turn(
                            "effort-call",
                            vec![Item(json!({
                                "type":"function_call",
                                "call_id":"set-effort-call",
                                "name":"set_effort",
                                "arguments":"{\"effort\":\"high\"}"
                            }))],
                        ),
                        turn(
                            "effort-final",
                            vec![Item(json!({
                                "type":"message","role":"assistant","phase":"final_answer","content":"done"
                            }))],
                        ),
                    ]
                    .into(),
                ),
            },
            release_first_turn: tokio::sync::Mutex::new(Some(release_first_turn_rx)),
        };
        let store = Arc::new(Store::memory().unwrap());
        let head = RequestId("effort-tool-output-head".into());
        store
            .write_request(&head, None, "/root", &[], StoredUsage::default())
            .unwrap();
        store.set_effort(&head, Effort::Low).unwrap();
        store
            .admit_agent(
                &AgentPath("/root".into()),
                None,
                Some(&head),
                &json!({}),
                &json!({"kind":"root"}),
            )
            .unwrap();
        let completed = Arc::new(Notify::new());
        let provider = Arc::new(SetEffortProvider {
            service: crate::agent_runtime::StoreAgentToolService::new(
                store.clone(),
                AgentPath("/root".into()),
            ),
            completed: completed.clone(),
        });

        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let engine = Engine::<FakeAuth, SetEffortProvider, _>::with_transport(
            replay,
            store.clone(),
            scheduler.clone(),
            provider,
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let run = tokio::spawn(async move {
            engine
                .run(Some(head), vec![], cancel_rx, empty_mailbox())
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), completed.notified())
            .await
            .expect("set_effort updated the pending request boundary");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if scheduler
                    .output(&store.claims(&CallId("set-effort-call".into())).unwrap()[0].operation)
                    .await
                    .unwrap()
                    .is_some()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("set_effort output settled before the model response completes");
        release_first_turn
            .send(())
            .expect("release the response only after tool settlement");
        run.await.unwrap().unwrap();

        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].input.len(), 1);
        let history = &sent[1].input;
        assert_eq!(history.len(), 4);
        assert_eq!(history[0].configuration_effort(), Some(Effort::Low));
        assert_eq!(history[1].0["type"], "function_call");
        assert_eq!(history[2].0["type"], "function_call_output");
        assert_eq!(history[2].0["output"], "{\"effective\":\"high\"}");
        assert_eq!(history[3].configuration_effort(), Some(Effort::High));
        assert_eq!(sent[0].pinned_effort, Effort::Low);
        assert_eq!(sent[1].pinned_effort, Effort::Low);
        assert_eq!(
            history
                .iter()
                .filter(|item| item.configuration_effort() == Some(Effort::High))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn replay_sends_full_history_after_immediate_tool_dispatch() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new([
                turn("r1", vec![Item(json!({
                    "type":"function_call","call_id":"c1","name":"echo","arguments":"{\"x\":1}"
                }))]),
                turn("r2", vec![Item(json!({
                    "type":"function_call","call_id":"c2","name":"echo","arguments":"{\"x\":2}"
                }))]),
                turn("r3", vec![Item(json!({
                    "type":"message","role":"assistant","phase":"final_answer","content":"done"
                }))]),
            ].into()),
        };
        let path = std::env::temp_dir().join(format!("harness-engine-{}.db", uuid::Uuid::new_v4()));
        let store = Arc::new(Store::open(&path).unwrap());
        let scheduler = Arc::new(JobScheduler::new(2).unwrap());
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            store.clone(),
            scheduler,
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let result = engine
            .run(
                None,
                vec![Item(json!({"role":"user","content":"go"}))],
                cancel_rx,
                empty_mailbox(),
            )
            .await
            .unwrap();
        assert_eq!(result.turn.response_id, "r3");

        let expected_history = {
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            for request in requests.iter() {
                assert_eq!(
                    request.pinned_effort,
                    request
                        .input
                        .iter()
                        .find_map(Item::configuration_effort)
                        .expect("every sent history has an effort pin")
                );
            }
            assert_eq!(requests[0].input.len(), 2);
            assert_eq!(requests[0].input[1].0["type"], "configuration_update");
            let second = &requests[1].input;
            assert_eq!(second[0].0["content"], "go");
            assert_eq!(second[1].0["type"], "configuration_update");
            assert_eq!(second[2].0["type"], "function_call");
            assert_eq!(second[3].0["type"], "function_call_output");
            assert_eq!(second[3].0["call_id"], "c1");
            let output: Value =
                serde_json::from_str(second[3].0["output"].as_str().unwrap()).unwrap();
            assert_eq!(output, json!({"tool":"echo","args":{"x":1}}));
            let third = &requests[2].input;
            assert_eq!(third[4].0["type"], "function_call");
            assert_eq!(third[4].0["call_id"], "c2");
            assert_eq!(third[5].0["type"], "function_call_output");
            assert_eq!(third[5].0["call_id"], "c2");
            let output: Value =
                serde_json::from_str(third[5].0["output"].as_str().unwrap()).unwrap();
            assert_eq!(output, json!({"tool":"echo","args":{"x":2}}));
            let mut history = third.clone();
            history.push(Item(json!({
                "type":"message","role":"assistant","phase":"final_answer","content":"done"
            })));
            history
        };
        let usage_events: Vec<_> = store
            .events(None)
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "responses_usage")
            .collect();
        assert_eq!(usage_events.len(), 3);
        let request_ids: Vec<_> = usage_events
            .iter()
            .map(|event| event.request.clone().unwrap())
            .collect();
        assert_ne!(request_ids[0], request_ids[1]);
        assert_ne!(request_ids[1], request_ids[2]);
        let first = store.request(&request_ids[0]).unwrap().unwrap();
        let second = store.request(&request_ids[1]).unwrap().unwrap();
        let third = store.request(&request_ids[2]).unwrap().unwrap();
        assert_eq!(second.parent, Some(first.id.clone()));
        assert_eq!(third.parent, Some(second.id.clone()));
        let aggregate = store.usage_subtree(&first.id).unwrap();
        assert_eq!(aggregate.input_tokens, 9);
        assert_eq!(aggregate.output_tokens, 6);
        drop(engine);
        drop(store);
        let reopened = Arc::new(Store::open(&path).unwrap());
        let history = load_history(reopened.clone(), third.id.clone())
            .await
            .unwrap();
        assert_eq!(history, expected_history);
        let recorded = reopened.replay_turns(&first.id).unwrap();
        let sent = requests.lock().unwrap();
        assert_eq!(recorded.len(), sent.len());
        for (recorded, sent) in recorded.iter().zip(sent.iter()) {
            assert_eq!(
                crate::transport::client::request_body(&recorded.model_request).unwrap(),
                crate::transport::client::request_body(sent).unwrap(),
            );
        }
        drop(reopened);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn successful_run_returns_complete_ordered_transcript_once() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let initial = Item(json!({"role":"user","content":"start"}));
        let call = Item(json!({
            "type":"function_call","call_id":"transcript-call","name":"echo","arguments":"{\"n\":7}"
        }));
        let final_item = Item(json!({
            "type":"message","role":"assistant","phase":"final_answer","content":"finished"
        }));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new(
                [
                    turn("transcript-one", vec![call.clone()]),
                    turn("transcript-two", vec![final_item.clone()]),
                ]
                .into(),
            ),
        };
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let completion = engine
            .run(None, vec![initial.clone()], cancel_rx, empty_mailbox())
            .await
            .unwrap();

        assert_eq!(completion.turn.response_id, "transcript-two");
        assert_eq!(
            completion.transcript,
            vec![
                initial.clone(),
                Item::configuration_update(Effort::Low),
                call,
                items::function_output(
                    &CallId("transcript-call".into()),
                    &crate::turn::JobOutput::Completed(Ok(json!({"tool":"echo","args":{"n":7}})))
                ),
                final_item,
            ]
        );
        assert_eq!(
            completion
                .transcript
                .iter()
                .filter(|item| **item == initial)
                .count(),
            1,
            "initial input is included exactly once"
        );
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert_eq!(
            load_history(store, completion.head_request).await.unwrap(),
            completion.transcript
        );
    }

    #[tokio::test]
    async fn run_from_head_uses_parent_once_and_chains_sequential_runs() {
        let path =
            std::env::temp_dir().join(format!("harness-engine-head-{}.db", uuid::Uuid::new_v4()));
        let store = Arc::new(Store::open(&path).unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let prefix = Item(json!({"role":"user","content":"old prefix"}));
        let parent = RequestId("existing-head".into());
        store
            .write_request(
                &parent,
                None,
                "/root",
                std::slice::from_ref(&prefix),
                StoredUsage::default(),
            )
            .unwrap();
        let first_new = Item(json!({"role":"user","content":"first new"}));
        let first_answer = Item(json!({
            "type":"message","role":"assistant","phase":"final_answer","content":"answer one"
        }));
        let second_new = Item(json!({"role":"user","content":"second new"}));
        let second_answer = Item(json!({
            "type":"message","role":"assistant","phase":"final_answer","content":"answer two"
        }));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new(
                [
                    turn("head-one", vec![first_answer.clone()]),
                    turn("head-two", vec![second_answer.clone()]),
                ]
                .into(),
            ),
        };
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "stable-session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let first = engine
            .run(
                Some(parent.clone()),
                vec![first_new.clone()],
                cancel_rx,
                empty_mailbox(),
            )
            .await
            .unwrap();
        let first_req = store.request(&first.head_request).unwrap().unwrap();
        assert_eq!(first_req.parent, Some(parent.clone()));
        assert_eq!(
            requests.lock().unwrap()[0].input,
            vec![
                prefix.clone(),
                first_new.clone(),
                Item::configuration_update(Effort::Low)
            ]
        );

        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let second = engine
            .run(
                Some(first.head_request.clone()),
                vec![second_new.clone()],
                cancel_rx,
                empty_mailbox(),
            )
            .await
            .unwrap();
        let second_req = store.request(&second.head_request).unwrap().unwrap();
        assert_eq!(second_req.parent, Some(first.head_request.clone()));
        let recorded = requests.lock().unwrap();
        assert_eq!(
            recorded[1].input,
            vec![
                prefix.clone(),
                first_new.clone(),
                Item::configuration_update(Effort::Low),
                first_answer.clone(),
                second_new.clone(),
            ]
        );
        assert_eq!(recorded[0].session_id, "stable-session");
        assert_eq!(recorded[1].session_id, "stable-session");
        assert_eq!(
            second.transcript,
            vec![
                prefix.clone(),
                first_new,
                Item::configuration_update(Effort::Low),
                first_answer,
                second_new,
                second_answer,
            ]
        );
        assert_eq!(
            second.transcript.iter().filter(|i| **i == prefix).count(),
            1
        );
        drop(recorded);
        drop(engine);
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn run_from_none_starts_with_only_new_items() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new(
                [turn(
                    "from-none",
                    vec![Item(json!({
                        "type":"message","role":"assistant","phase":"final_answer","content":"done"
                    }))],
                )]
                .into(),
            ),
        };
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "new-session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let new_item = Item(json!({"role":"user","content":"only this"}));
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        engine
            .run(None, vec![new_item.clone()], cancel_rx, empty_mailbox())
            .await
            .unwrap();
        let recorded = requests.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(
            recorded[0].input,
            vec![new_item, Item::configuration_update(Effort::Low)]
        );
        assert_eq!(recorded[0].session_id, "new-session");
    }

    #[tokio::test]
    async fn run_from_missing_head_fails_without_creating_a_fresh_root() {
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            Replay {
                requests: Arc::new(Mutex::new(Vec::new())),
                turns: Mutex::new([].into()),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        assert!(matches!(
            engine
                .run(
                    Some(RequestId("no-such-head".into())),
                    vec![Item(json!({"role":"user","content":"new"}))],
                    cancel_rx,
                    empty_mailbox(),
                )
                .await,
            Err(EngineError::Store(StoreError::MissingRequest(id))) if id == "no-such-head"
        ));
        assert!(
            store
                .children_of(&RequestId("no-such-head".into()))
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn run_from_head_admits_unread_inbox_once_before_first_prompt() {
        let store = Arc::new(Store::memory().unwrap());
        let recipient = AgentPath("/root/worker".into());
        let task_item = Item(json!({
            "type":"message","role":"user","content":"NEW_TASK: do this"
        }));
        store
            .add_envelope("/root", &recipient.0, "task", &task_item, None)
            .unwrap();

        let requests = Arc::new(Mutex::new(Vec::new()));
        let first_answer = Item(json!({
            "type":"message","role":"assistant","phase":"final_answer","content":"ack"
        }));
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            Replay {
                requests: requests.clone(),
                turns: Mutex::new([
                    turn("inbox-first", vec![first_answer.clone()]),
                    turn(
                        "inbox-second",
                        vec![Item(json!({
                            "type":"message","role":"assistant","phase":"final_answer","content":"next"
                        }))],
                    ),
                ]
                .into()),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: recipient.clone(),
            },
        );

        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let first = engine
            .run(None, vec![], cancel_rx, empty_mailbox())
            .await
            .unwrap();
        assert_eq!(
            requests.lock().unwrap()[0].input,
            vec![Item::configuration_update(Effort::Low), task_item.clone()]
        );
        assert_eq!(
            store.items(&first.head_request).unwrap(),
            vec![
                Item::configuration_update(Effort::Low),
                task_item.clone(),
                first_answer.clone()
            ]
        );
        assert!(store.unread(&recipient.0).unwrap().is_empty());

        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let second = engine
            .run(
                Some(first.head_request.clone()),
                vec![],
                cancel_rx,
                empty_mailbox(),
            )
            .await
            .unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].input,
            vec![
                Item::configuration_update(Effort::Low),
                task_item.clone(),
                first_answer
            ]
        );
        assert_eq!(
            second
                .transcript
                .iter()
                .filter(|item| **item == task_item)
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn run_from_head_orders_new_input_before_inbox_items() {
        let store = Arc::new(Store::memory().unwrap());
        let recipient = AgentPath("/root/worker".into());
        let inbox_item = Item(json!({
            "type":"message","role":"user","content":"NEW_TASK: queued"
        }));
        store
            .add_envelope("/root", &recipient.0, "task", &inbox_item, None)
            .unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            Replay {
                requests: requests.clone(),
                turns: Mutex::new([turn(
                    "input-before-inbox",
                    vec![Item(json!({
                        "type":"message","role":"assistant","phase":"final_answer","content":"done"
                    }))],
                )]
                .into()),
            },
            store,
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: recipient,
            },
        );
        let user_item = Item(json!({"type":"message","role":"user","content":"hello"}));
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        engine
            .run(None, vec![user_item.clone()], cancel_rx, empty_mailbox())
            .await
            .unwrap();
        assert_eq!(
            requests.lock().unwrap()[0].input,
            vec![
                user_item,
                Item::configuration_update(Effort::Low),
                inbox_item
            ]
        );
    }

    #[tokio::test]
    async fn starts_tool_from_item_done_before_response_completes() {
        let started = Arc::new(Notify::new());
        let response_completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let started_early = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(Store::memory().unwrap());
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let transport = ControlledStream {
            requests,
            started: started.clone(),
            response_completed: response_completed.clone(),
        };
        let provider = Arc::new(GateProvider {
            started,
            response_completed,
            started_early: started_early.clone(),
        });
        let engine = Engine::<FakeAuth, GateProvider, _>::with_transport(
            transport,
            store,
            scheduler,
            provider,
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        engine
            .run(
                None,
                vec![Item(json!({"role":"user","content":"go"}))],
                cancel_rx,
                empty_mailbox(),
            )
            .await
            .unwrap();
        assert!(started_early.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn malformed_function_call_returns_typed_engine_error() {
        let replay = Replay {
            requests: Arc::new(Mutex::new(Vec::new())),
            turns: Mutex::new(
                [turn(
                    "malformed",
                    vec![Item(json!({
                        "type":"function_call","call_id":"bad","name":"echo","arguments":"["
                    }))],
                )]
                .into(),
            ),
        };
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        assert!(matches!(
            engine
                .run(
                    None,
                    vec![Item(json!({"role":"user","content":"go"}))],
                    cancel_rx,
                    empty_mailbox(),
                )
                .await,
            Err(EngineError::InvalidFunctionCall)
        ));
    }

    struct PendingProvider;
    #[async_trait]
    impl Provider for PendingProvider {
        async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
            std::future::pending().await
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    struct NeverComplete;
    #[async_trait::async_trait]
    impl ResponsesTransport for NeverComplete {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            std::future::pending().await
        }

        async fn create_streaming(
            &self,
            _: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            for (call_id, name) in [("cancel-one", "one"), ("cancel-two", "two")] {
                sink.send(StreamEvent::ItemDone(Item(json!({
                    "type":"function_call","call_id":call_id,"name":name,"arguments":"{}"
                }))))
                .await
                .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
            }
            std::future::pending().await
        }
    }

    struct SignaledPendingProvider {
        started: Arc<Notify>,
    }
    #[async_trait]
    impl Provider for SignaledPendingProvider {
        async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
            self.started.notify_one();
            std::future::pending().await
        }

        fn tools(&self) -> Vec<Value> {
            Vec::new()
        }
    }

    struct FailAfterStart {
        started: Arc<Notify>,
    }
    #[async_trait::async_trait]
    impl ResponsesTransport for FailAfterStart {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            unreachable!("streaming path expected")
        }

        async fn create_streaming(
            &self,
            _: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            sink.send(StreamEvent::ItemDone(Item(json!({
                "type":"function_call","call_id":"transport-fail","name":"pending","arguments":"{}"
            }))))
            .await
            .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
            self.started.notified().await;
            Err(TransportError::Stream("scripted response failure".into()))
        }
    }

    struct MalformedAfterStart {
        started: Arc<Notify>,
    }
    #[async_trait::async_trait]
    impl ResponsesTransport for MalformedAfterStart {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            unreachable!("streaming path expected")
        }

        async fn create_streaming(
            &self,
            _: ResponsesRequest,
            sink: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            let first = Item(json!({
                "type":"function_call","call_id":"before-malformed","name":"pending","arguments":"{}"
            }));
            let malformed = Item(json!({
                "type":"function_call","call_id":"malformed-later","name":"pending","arguments":"["
            }));
            sink.send(StreamEvent::ItemDone(first.clone()))
                .await
                .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
            self.started.notified().await;
            sink.send(StreamEvent::ItemDone(malformed.clone()))
                .await
                .map_err(|_| TransportError::Stream("engine event receiver closed".into()))?;
            Ok(turn("malformed-after-start", vec![first, malformed]))
        }
    }

    #[tokio::test]
    async fn cancellation_cancels_all_dispatched_calls() {
        use crate::store::ClaimState;
        use crate::turn::JobOutput;

        let store = Arc::new(Store::memory().unwrap());
        let scheduler = Arc::new(JobScheduler::new(2).unwrap());
        let engine = Engine::<FakeAuth, PendingProvider, _>::with_transport(
            NeverComplete,
            store.clone(),
            scheduler.clone(),
            Arc::new(PendingProvider),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let run = tokio::spawn(async move {
            engine
                .run(
                    None,
                    vec![Item(json!({"role":"user","content":"go"}))],
                    cancel_rx,
                    empty_mailbox(),
                )
                .await
        });
        let first = CallId("cancel-one".into());
        let second = CallId("cancel-two".into());
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !store.claims(&first).unwrap().is_empty()
                    && !store.claims(&second).unwrap().is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both durable claims registered");
        cancel_tx.send(true).unwrap();
        assert!(matches!(
            run.await.unwrap(),
            Err(EngineError::Cancelled { .. })
        ));
        assert_eq!(
            scheduler
                .output(&store.claims(&first).unwrap()[0].operation)
                .await
                .unwrap(),
            Some(JobOutput::Cancelled)
        );
        assert_eq!(
            scheduler
                .output(&store.claims(&second).unwrap()[0].operation)
                .await
                .unwrap(),
            Some(JobOutput::Cancelled)
        );
        assert_eq!(store.claims(&first).unwrap()[0].state, ClaimState::Settled);
        assert_eq!(store.claims(&second).unwrap()[0].state, ClaimState::Settled);
        for call in [&first, &second] {
            let claim = store.claims(call).unwrap().remove(0);
            let output = store.get_item(&claim.output.unwrap()).unwrap().unwrap();
            assert_eq!(output.0["type"], "function_call_output");
            assert_eq!(
                serde_json::from_str::<Value>(output.0["output"].as_str().unwrap()).unwrap(),
                json!({"error":"job cancelled"})
            );
        }
    }

    #[tokio::test]
    async fn transport_failure_cancels_started_job_and_interrupts_claim() {
        use crate::store::ClaimState;
        use crate::turn::JobOutput;

        let started = Arc::new(Notify::new());
        let store = Arc::new(Store::memory().unwrap());
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let engine = Engine::<FakeAuth, SignaledPendingProvider, _>::with_transport(
            FailAfterStart {
                started: started.clone(),
            },
            store.clone(),
            scheduler.clone(),
            Arc::new(SignaledPendingProvider {
                started: started.clone(),
            }),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        assert!(matches!(
            engine
                .run(
                    None,
                    vec![Item(json!({"role":"user","content":"go"}))],
                    cancel_rx,
                    empty_mailbox(),
                )
                .await,
            Err(EngineError::Transport(TransportError::Stream(_)))
        ));
        let call = CallId("transport-fail".into());
        assert_eq!(
            scheduler
                .output(&store.claims(&call).unwrap()[0].operation)
                .await
                .unwrap(),
            Some(JobOutput::Cancelled)
        );
        assert_eq!(store.claims(&call).unwrap()[0].state, ClaimState::Settled);
    }

    #[tokio::test]
    async fn malformed_later_stream_item_cleans_up_earlier_job() {
        use crate::store::ClaimState;
        use crate::turn::JobOutput;

        let started = Arc::new(Notify::new());
        let store = Arc::new(Store::memory().unwrap());
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let engine = Engine::<FakeAuth, SignaledPendingProvider, _>::with_transport(
            MalformedAfterStart {
                started: started.clone(),
            },
            store.clone(),
            scheduler.clone(),
            Arc::new(SignaledPendingProvider {
                started: started.clone(),
            }),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        assert!(matches!(
            engine
                .run(
                    None,
                    vec![Item(json!({"role":"user","content":"go"}))],
                    cancel_rx,
                    empty_mailbox(),
                )
                .await,
            Err(EngineError::InvalidFunctionCall)
        ));
        let call = CallId("before-malformed".into());
        assert_eq!(
            scheduler
                .output(&store.claims(&call).unwrap()[0].operation)
                .await
                .unwrap(),
            Some(JobOutput::Cancelled)
        );
        assert_eq!(store.claims(&call).unwrap()[0].state, ClaimState::Settled);
        let request = store.claims(&call).unwrap()[0].request.clone();
        let recorded = store.replay_turns(&request).unwrap();
        assert_eq!(
            recorded.len(),
            1,
            "completed model turn survives dispatch failure"
        );
        assert_eq!(recorded[0].request, request);
        assert_eq!(recorded[0].model_response.items.len(), 2);
    }

    #[tokio::test]
    async fn missing_parent_is_not_treated_as_a_truncated_history() {
        let path = std::env::temp_dir().join(format!(
            "harness-engine-missing-{}.db",
            uuid::Uuid::new_v4()
        ));
        let parent = RequestId("missing-parent".into());
        let child = RequestId("history-child".into());
        {
            let store = Store::open(&path).unwrap();
            store
                .write_request(&parent, None, "root", &[], StoredUsage::default())
                .unwrap();
            store
                .write_request(&child, Some(&parent), "root", &[], StoredUsage::default())
                .unwrap();
        }
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute("PRAGMA foreign_keys=OFF", []).unwrap();
            conn.execute("DELETE FROM requests WHERE id=?1", [&parent.0])
                .unwrap();
        }
        let store = Arc::new(Store::open(&path).unwrap());
        assert!(matches!(
            load_history(store, child).await,
            Err(EngineError::Store(StoreError::MissingRequest(id))) if id == "missing-parent"
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn slow_job_does_not_block_model_continuation_and_wait_resumes_in_call_order() {
        let started = Arc::new(Notify::new());
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(Store::memory().unwrap());
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let second_request = Arc::new(Notify::new());
        let engine = Engine::<FakeAuth, SlowProvider, _>::with_transport(
            AsyncContinuation {
                requests: requests.clone(),
                started: started.clone(),
                second_request: second_request.clone(),
            },
            store,
            scheduler,
            Arc::new(SlowProvider {
                started,
                release: tokio::sync::Mutex::new(Some(release_rx)),
                released: released.clone(),
            }),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let run = tokio::spawn(async move {
            engine
                .run(
                    None,
                    vec![Item(json!({"role":"user","content":"go"}))],
                    cancel_rx,
                    empty_mailbox(),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), second_request.notified())
            .await
            .expect("model continued while slow tool remained pending");
        assert!(!released.load(std::sync::atomic::Ordering::SeqCst));
        release_tx.send(()).unwrap();
        let final_result = tokio::time::timeout(std::time::Duration::from_secs(3), run).await;
        assert_eq!(
            final_result.unwrap().unwrap().unwrap().turn.response_id,
            "async-final"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        let resumed_history = &requests[2].input;
        assert_eq!(resumed_history[2].0["call_id"], "slow-call");
        assert_eq!(resumed_history[3].0["call_id"], "wait-call");
        assert_eq!(resumed_history[4].0["call_id"], "slow-call");
        assert_eq!(resumed_history[5].0["call_id"], "wait-call");
        let slow_output: Value =
            serde_json::from_str(resumed_history[4].0["output"].as_str().unwrap()).unwrap();
        assert_eq!(slow_output, json!({"slow_done":true}));
        let wait_output: Value =
            serde_json::from_str(resumed_history[5].0["output"].as_str().unwrap()).unwrap();
        assert_eq!(wait_output, json!({"resumed_by":{"job":"slow-call"}}));
    }

    #[tokio::test]
    async fn late_settlement_after_final_response_wakes_continuation() {
        let started = Arc::new(Notify::new());
        let response_finished = Arc::new(Notify::new());
        let second_request = Arc::new(Notify::new());
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, SlowProvider, _>::with_transport(
            LateFinal {
                requests: requests.clone(),
                started: started.clone(),
                response_finished: response_finished.clone(),
                second_request: second_request.clone(),
            },
            store,
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(SlowProvider {
                started,
                release: tokio::sync::Mutex::new(Some(release_rx)),
                released: released.clone(),
            }),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let run = tokio::spawn(async move {
            engine
                .run(
                    None,
                    vec![Item(json!({"role":"user","content":"go"}))],
                    cancel_rx,
                    empty_mailbox(),
                )
                .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            response_finished.notified(),
        )
        .await
        .expect("model produced provisional final");
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                second_request.notified()
            )
            .await
            .is_err(),
            "engine must pause while job is pending"
        );
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), second_request.notified())
            .await
            .expect("late settlement triggered next model request");
        assert_eq!(
            run.await.unwrap().unwrap().turn.response_id,
            "late-continued"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].input[2].0["call_id"], "late-call");
        assert_eq!(requests[1].input[4].0["call_id"], "late-call");
    }

    #[tokio::test]
    async fn wait_agent_resumes_on_envelope_after_any_ready_job_outputs() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            WaitEnvelopeTransport {
                requests: requests.clone(),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (envelope_tx, envelope_rx) = tokio::sync::mpsc::unbounded_channel();
        let run = tokio::spawn(async move {
            engine
                .run(
                    None,
                    vec![Item(json!({"role":"user","content":"go"}))],
                    cancel_rx,
                    envelope_rx,
                )
                .await
        });
        let wait_call = CallId("wait-envelope".into());
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !store.claims(&wait_call).unwrap().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("wait call claim persisted");
        let envelope = Envelope {
            kind: crate::mailbox::EnvelopeType::Message,
            recipient: AgentPath("/root".into()),
            sender: AgentPath("/root/worker".into()),
            payload: "arrived".into(),
            class: crate::mailbox::DeliveryClass::AtBoundary,
            timestamp_ms: 1,
        };
        let stored_item = Item(json!({
            "type":"message",
            "role":"assistant",
            "content":"Message Type: MESSAGE\nTask name: /root\nSender: /root/worker\nPayload:\narrived"
        }));
        store
            .add_envelope(
                &envelope.sender.0,
                &envelope.recipient.0,
                "AtBoundary",
                &stored_item,
                None,
            )
            .unwrap();
        envelope_tx.send(envelope).unwrap();
        assert_eq!(run.await.unwrap().unwrap().turn.response_id, "resumed");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let next = &requests[1].input;
        assert_eq!(next[1], Item::configuration_update(Effort::Low));
        assert_eq!(next[2].0["call_id"], "wait-envelope");
        assert_eq!(next[3].0["type"], "function_call_output");
        assert_eq!(next[3].0["call_id"], "wait-envelope");
        let output: Value = serde_json::from_str(next[3].0["output"].as_str().unwrap()).unwrap();
        assert_eq!(output, json!({"resumed_by":{"agent":"/root/worker"}}));
        assert_eq!(next[4].0["type"], "message");
        assert_eq!(
            next[4].0["content"],
            "Message Type: MESSAGE\nTask name: /root\nSender: /root/worker\nPayload:\narrived"
        );
    }

    #[tokio::test]
    async fn durable_envelope_wake_appends_store_item_after_wait_output_once() {
        let path = std::env::temp_dir().join(format!(
            "harness-wait-replay-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let store = Arc::new(Store::open(&path).unwrap());
        let recipient = AgentPath("/root".into());
        let sender = AgentPath("/root/worker".into());
        let envelope = Envelope {
            kind: crate::mailbox::EnvelopeType::Message,
            recipient: recipient.clone(),
            sender: sender.clone(),
            payload: "persisted reply".into(),
            class: crate::mailbox::DeliveryClass::AtBoundary,
            timestamp_ms: 5,
        };
        let stored_item = Item(json!({
            "type":"message",
            "role":"assistant",
            "content":[{
                "type":"output_text",
                "text":"Message Type: MESSAGE\nTask name: /root\nSender: /root/worker\nPayload:\npersisted reply"
            }]
        }));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            WaitEnvelopeTransport {
                requests: requests.clone(),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "durable-session".into(),
                agent: recipient.clone(),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (envelope_tx, envelope_rx) = tokio::sync::mpsc::unbounded_channel();
        let run =
            tokio::spawn(async move { engine.run(None, vec![], cancel_rx, envelope_rx).await });
        let wait_call = CallId("wait-envelope".into());
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !store.claims(&wait_call).unwrap().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("wait call is active before inbox arrival");
        store
            .add_envelope(&sender.0, &recipient.0, "AtBoundary", &stored_item, None)
            .unwrap();
        envelope_tx.send(envelope).unwrap();
        let completion = run.await.unwrap().unwrap();

        let recorded = requests.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        let resumed = &recorded[1].input;
        assert_eq!(resumed.len(), 4);
        assert_eq!(resumed[0], Item::configuration_update(Effort::Low));
        assert_eq!(resumed[1].0["call_id"], "wait-envelope");
        assert_eq!(resumed[2].0["type"], "function_call_output");
        assert_eq!(resumed[2].0["call_id"], "wait-envelope");
        let status: Value = serde_json::from_str(resumed[2].0["output"].as_str().unwrap()).unwrap();
        assert_eq!(status, json!({"resumed_by":{"agent":"/root/worker"}}));
        assert_eq!(resumed[3], stored_item);
        assert_eq!(
            completion
                .transcript
                .iter()
                .filter(|item| **item == stored_item)
                .count(),
            1,
            "the wake hint must not be rendered/appended a second time"
        );
        assert!(store.unread(&recipient.0).unwrap().is_empty());
        let root = store.claims(&wait_call).unwrap()[0]
            .operation
            .request
            .clone();
        drop(recorded);
        drop(store);
        let reopened = Arc::new(Store::open(&path).unwrap());
        let replay = Arc::new(crate::replay::ReplayProvider::new(reopened.clone(), &root).unwrap());
        assert_eq!(replay.turns_remaining(), 2);
        let engine = Engine::<FakeAuth, crate::replay::ReplayProvider, _>::with_transport(
            replay.clone(),
            reopened.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            replay.clone(),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "durable-session".into(),
                agent: AgentPath("/offline-replay".into()),
            },
        );
        let (_cancel, cancellation) = watch::channel(false);
        let replayed = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            engine.run(None, vec![], cancellation, empty_mailbox()),
        )
        .await
        .expect("retained wait must continue without a fresh envelope")
        .unwrap();
        assert_eq!(replay.turns_remaining(), 0);
        assert_eq!(replayed.transcript, completion.transcript);
        assert_eq!(
            replayed
                .transcript
                .iter()
                .filter(|item| **item == stored_item)
                .count(),
            1
        );
        assert_eq!(
            replayed
                .transcript
                .iter()
                .filter(|item| item.0["type"] == "function_call_output"
                    && item.0["call_id"] == "wait-envelope")
                .count(),
            1
        );
        drop(engine);
        drop(replay);
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn durable_mode_admits_inbox_arriving_before_next_model_request() {
        let store = Arc::new(Store::memory().unwrap());
        let recipient = AgentPath("/root/worker".into());
        let inbox_item = Item(json!({
            "type":"message","role":"assistant","content":[{
                "type":"output_text","text":"queued while model was running"
            }]
        }));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            QueueInboxBeforeContinuation {
                requests: requests.clone(),
                store: store.clone(),
                recipient: recipient.clone(),
                inbox_item: inbox_item.clone(),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "durable-session".into(),
                agent: recipient.clone(),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_envelope_tx, envelope_rx) = tokio::sync::mpsc::unbounded_channel();
        let completion = engine
            .run(None, vec![], cancel_rx, envelope_rx)
            .await
            .unwrap();

        let recorded = requests.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        let next = &recorded[1].input;
        assert_eq!(next.last(), Some(&inbox_item));
        assert_eq!(next.iter().filter(|item| **item == inbox_item).count(), 1);
        assert_eq!(
            completion
                .transcript
                .iter()
                .filter(|item| **item == inbox_item)
                .count(),
            1
        );
        assert!(store.unread(&recipient.0).unwrap().is_empty());
    }

    #[derive(Debug, Deserialize, JsonSchema, PartialEq)]
    struct FinalReply {
        answer: String,
    }

    #[tokio::test]
    async fn typed_finalize_is_the_only_terminal_path_and_is_not_a_job() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let final_call = Item(json!({
            "type":"function_call", "call_id":"final-1", "name":"finalize",
            "arguments":"{\"result\":{\"answer\":\"ready\"}}"
        }));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new([
                turn("typed-tool", vec![Item(json!({
                    "type":"function_call","call_id":"before-final","name":"echo","arguments":"{}"
                }))]),
                turn("typed-final", vec![final_call.clone()]),
            ].into()),
        };
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "finalize".into(),
                tools: vec![json!({
                    "type":"function", "name":"finalize", "strict":false,
                    "parameters":{"type":"object","properties":{}}
                })],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "typed-session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (completion, reply) = engine
            .run_finalized::<FinalReply>(
                None,
                vec![Item(json!({"role":"user","content":"go"}))],
                cancel_rx,
                empty_mailbox(),
            )
            .await
            .unwrap();
        assert_eq!(
            reply,
            FinalReply {
                answer: "ready".into()
            }
        );
        assert_eq!(
            FinalizeParser::new()
                .parse_completed::<FinalReply>(&completion.turn.items[0])
                .unwrap(),
            FinalReply {
                answer: "ready".into()
            }
        );
        assert!(completion.transcript.contains(&final_call));
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(std::ptr::eq(
            requests[0].tools.as_ptr(),
            requests[1].tools.as_ptr()
        ));
        assert_eq!(
            requests[0]
                .tools
                .iter()
                .filter(|t| t["name"] == "finalize")
                .count(),
            1
        );
        assert_eq!(
            requests[0]
                .tools
                .iter()
                .find(|t| t["name"] == "finalize")
                .unwrap()["strict"],
            true
        );
        assert!(
            store
                .claims_on(&completion.head_request)
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn dynamic_reply_schema_preserves_strict_result_without_job() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let final_call = Item(json!({
            "type":"function_call", "call_id":"final-dynamic", "name":"finalize",
            "arguments":{"result":{"answer":"ready"}}
        }));
        let replay = Replay {
            requests: requests.clone(),
            turns: Mutex::new([turn("dynamic-final", vec![final_call.clone()])].into()),
        };
        let store = Arc::new(Store::memory().unwrap());
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            replay,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "finalize".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "dynamic-session".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let completion = engine
            .run_with_reply_schema(
                None,
                vec![Item(json!({"role":"user","content":"go"}))],
                cancel_rx,
                empty_mailbox(),
                json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}),
            )
            .await
            .unwrap();
        assert_eq!(
            FinalizeParser::new()
                .parse_completed::<serde_json::Value>(&completion.turn.items[0])
                .unwrap(),
            json!({"answer":"ready"})
        );
        assert!(completion.transcript.contains(&final_call));
        assert!(
            store
                .claims_on(&completion.head_request)
                .unwrap()
                .is_empty()
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]
                .tools
                .iter()
                .filter(|t| t["name"] == "finalize")
                .count(),
            1
        );
        assert_eq!(
            requests[0]
                .tools
                .iter()
                .find(|t| t["name"] == "finalize")
                .unwrap()["strict"],
            true
        );
    }

    #[tokio::test]
    async fn dynamic_reply_schema_rejects_multiple_and_malformed_finalize() {
        let valid = Item(json!({
            "type":"function_call", "call_id":"final-1", "name":"finalize",
            "arguments":{"result":{"answer":"ready"}}
        }));
        let second = Item(json!({
            "type":"function_call", "call_id":"final-2", "name":"finalize",
            "arguments":{"result":{"answer":"again"}}
        }));
        let malformed = Item(json!({
            "type":"function_call", "call_id":"final-1", "name":"finalize",
            "arguments":{"wrong":{"answer":"ready"}}
        }));
        let wrong_shape = Item(json!({
            "type":"function_call", "call_id":"final-1", "name":"finalize",
            "arguments":{"result":{"answer":7}}
        }));
        for (name, items) in [
            ("multiple", vec![valid.clone(), second]),
            ("missing-result", vec![malformed]),
            ("wrong-result-shape", vec![wrong_shape]),
        ] {
            let replay = Replay {
                requests: Arc::new(Mutex::new(Vec::new())),
                turns: Mutex::new([turn(name, items)].into()),
            };
            let engine = Engine::<FakeAuth, Echo, _>::with_transport(
                replay,
                Arc::new(Store::memory().unwrap()),
                Arc::new(JobScheduler::new(1).unwrap()),
                Arc::new(Echo),
                EngineConfig {
                    instructions: "finalize".into(),
                    tools: vec![],
                    model: "test".into(),
                    effort: Effort::Low,
                    session_id: format!("bad-{name}"),
                    agent: AgentPath("/root".into()),
                },
            );
            let (_cancel_tx, cancel_rx) = watch::channel(false);
            let error = engine
                .run_with_reply_schema(
                    None,
                    vec![Item(json!({"role":"user","content":"go"}))],
                    cancel_rx,
                    empty_mailbox(),
                    json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}),
                )
                .await
                .expect_err(name);
            match name {
                "multiple" => assert!(matches!(error, EngineError::InvalidFinalizeCount)),
                "missing-result" => assert!(matches!(
                    error,
                    EngineError::Finalize(FinalizeError::InvalidEnvelope)
                )),
                _ => assert!(matches!(
                    error,
                    EngineError::Finalize(FinalizeError::ResultSchemaMismatch(_))
                )),
            }
        }
    }

    /// Explicit subscription smoke: opt in locally, never in ordinary CI.
    #[tokio::test]
    #[ignore]
    async fn live_subscription_engine_final_answer() {
        use crate::transport::auth::CodexFileAuth;
        let auth = CodexFileAuth::new(CodexFileAuth::default_path().expect("auth path"));
        let engine = Engine::new(
            auth,
            Arc::new(Store::memory().expect("store")),
            Arc::new(JobScheduler::new(2).expect("jobs")),
            Arc::new(Echo),
            EngineConfig {
                instructions: "Reply briefly and do not call tools.".into(),
                tools: Vec::new(),
                model: "gpt-6-sol".into(),
                effort: Effort::Low,
                session_id: format!("harness-engine-smoke-{}", uuid::Uuid::new_v4()),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let turn = engine
            .run(
                None,
                vec![Item(
                    json!({"role":"user","content":"Reply exactly ENGINE_OK"}),
                )],
                cancel_rx,
                empty_mailbox(),
            )
            .await
            .expect("live engine turn");
        assert!(
            turn.turn
                .items
                .iter()
                .any(|item| item.0["phase"] == "final_answer")
        );
    }
}
