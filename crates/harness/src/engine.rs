//! Single-agent, stateless Responses request loop.
//!
//! Each model request receives the full accumulated item history. Function
//! calls are started immediately and their settled outputs are appended under
//! their original call ids before the next request.

use crate::{
    compaction::{
        CompactContext, CompactError, Compactor, Server, ServerCompactFuture, ToolName,
        TypedTurnFuture,
    },
    item::Item,
    mailbox::{Envelope, MessageChannel},
    model::{AgentPath, CallId, Effort, RequestId},
    provider::Provider,
    store::{Store, StoreError, Usage as StoredUsage},
    transport::{
        Auth, ResponsesClient, ResponsesRequest, ResponsesTurn, TransportError, Usage,
        sse::StreamEvent,
    },
    turn::{
        JobError, JobScheduler, WaitAgentResult, WaitResume, outputs_in_call_order,
        wait_agent_and_drain,
    },
};
use serde_json::json;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::watch;

#[path = "engine/items.rs"]
mod items;

#[derive(Clone, Debug)]
struct PendingCall {
    call_id: CallId,
    claim_request: RequestId,
    is_wait_agent: bool,
    cancel_job_on_cleanup: bool,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Job(#[from] JobError),
    #[error("blocking store task failed")]
    StoreTask,
    #[error("engine cancelled")]
    Cancelled,
    #[error("Responses turn did not contain a terminal assistant answer")]
    MissingFinal,
    #[error("malformed Responses function_call item")]
    InvalidFunctionCall,
    #[error("request history has no harness-authored configuration_update")]
    MissingEffortPin,
    #[error("a model response contained more than one wait_agent call")]
    MultipleWaitAgents,
    #[error(transparent)]
    Compact(#[from] CompactError),
    #[error("cannot resume a forked wait_agent call before its parent settles it")]
    UnresumableForkedWaitAgent,
    #[error("inherited settled call {0} has no durable output item")]
    MissingInheritedOutput(String),
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
    compact_at_input_tokens: Option<u64>,
    _auth: std::marker::PhantomData<fn() -> A>,
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

impl<A: Auth, P: Provider + 'static, C: ResponsesTransport> Engine<A, P, C> {
    /// Alternate transport constructor, primarily for deterministic replay.
    pub fn with_transport(
        client: C,
        store: Arc<Store>,
        scheduler: Arc<JobScheduler>,
        provider: Arc<P>,
        config: EngineConfig,
    ) -> Self {
        Self {
            client,
            store,
            scheduler,
            provider,
            config,
            compact_at_input_tokens: None,
            _auth: std::marker::PhantomData,
        }
    }

    /// Opt into explicit server compaction after a turn whose input usage
    /// reaches this threshold. No automatic API compaction is enabled.
    pub fn with_compaction_threshold(mut self, input_tokens: u64) -> Self {
        self.compact_at_input_tokens = (input_tokens > 0).then_some(input_tokens);
        self
    }

    /// Run from a durable request head, appending only new items and admitting
    /// persisted mailbox envelopes before each model request. `incoming` is a
    /// wake-hint stream; hosts must persist each envelope before signaling it.
    pub async fn run(
        &self,
        head: Option<RequestId>,
        new_items: Vec<Item>,
        cancellation: watch::Receiver<bool>,
        mut incoming: tokio::sync::mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<EngineCompletion, EngineError> {
        let (envelope_tx, envelopes) = tokio::sync::mpsc::unbounded_channel();
        let keepalive = envelope_tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(envelope) = incoming.recv().await {
                if envelope_tx.send(envelope).is_err() {
                    break;
                }
            }
        });
        let result = self
            .run_loop(head, new_items, cancellation, envelopes, true)
            .await;
        drop(keepalive);
        forwarder.abort();
        result
    }

    async fn run_loop(
        &self,
        head: Option<RequestId>,
        initial: Vec<Item>,
        mut cancellation: watch::Receiver<bool>,
        mut envelopes: tokio::sync::mpsc::UnboundedReceiver<Envelope>,
        admit_inbox: bool,
    ) -> Result<EngineCompletion, EngineError> {
        let inherited_history = match &head {
            Some(head) => self.read_history(head).await?,
            None => Vec::new(),
        };
        let inherited_claims = match &head {
            Some(head) => {
                let store = self.store.clone();
                let request = head.clone();
                blocking(move || store.claims_on(&request)).await?
            }
            None => Vec::new(),
        };
        let mut pending = Vec::<PendingCall>::new();
        let mut replay_items = Vec::<Item>::new();
        let mut replay_outputs = Vec::<(CallId, crate::turn::JobOutput)>::new();
        let mut attachable = Vec::new();
        for claim in inherited_claims {
            if inherited_history.iter().any(|item| {
                item.0["type"] == "function_call_output"
                    && item.0["call_id"].as_str() == Some(&claim.call_id.0)
            }) {
                continue;
            }
            if claim.state == crate::store::ClaimState::Settled {
                let Some(hash) = claim.output else {
                    return Err(EngineError::MissingInheritedOutput(claim.call_id.0.clone()));
                };
                let store = self.store.clone();
                let item = blocking(move || store.get_item(&hash)).await?;
                let Some(item) = item else {
                    return Err(EngineError::MissingInheritedOutput(claim.call_id.0));
                };
                replay_items.push(item);
                continue;
            }
            if claim.state == crate::store::ClaimState::Interrupted {
                replay_items.push(items::function_output(
                    &claim.call_id,
                    &crate::turn::JobOutput::Interrupted,
                ));
                continue;
            }
            if inherited_history.iter().any(|item| {
                items::function_call(item)
                    .is_some_and(|(call, name, _)| call == claim.call_id && name == "wait_agent")
            }) {
                return Err(EngineError::UnresumableForkedWaitAgent);
            }
            // Validate every pending job before claiming any of them. The
            // scheduler retains jobs for the lifetime of this shared runtime.
            let call_id = claim.call_id.clone();
            self.scheduler.output(&call_id).await?;
            attachable.push(claim);
        }
        for claim in attachable {
            match self
                .scheduler
                .fork_claim(&claim.call_id, self.config.agent.clone(), true)
                .await?
            {
                Some(output) => replay_outputs.push((claim.call_id, output)),
                None => pending.push(PendingCall {
                    call_id: claim.call_id,
                    claim_request: claim.request,
                    is_wait_agent: false,
                    cancel_job_on_cleanup: false,
                }),
            }
        }
        let id = RequestId(uuid::Uuid::new_v4().to_string());
        let store = self.store.clone();
        let request = id.clone();
        let branch = self.config.agent.0.clone();
        let parent = head.clone();
        blocking(move || {
            store.write_request(
                &request,
                parent.as_ref(),
                &branch,
                &initial,
                StoredUsage::default(),
            )
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
        for (call_id, output) in replay_outputs {
            self.persist_output(&call_id, &output, &id).await?;
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
        let mut pending = Vec::<PendingCall>::new();
        let mut compact_due = false;
        let mut previous_usage = Usage::default();
        loop {
            if *cancellation.borrow() {
                return Err(self.cleanup_pending(EngineError::Cancelled, &pending).await);
            }
            let history = match self.read_history(&parent).await {
                Ok(history) => history,
                Err(error) => return Err(self.cleanup_pending(error, &pending).await),
            };
            let did_compact = compact_due;
            if did_compact {
                let compact = self.compact_window(&parent, &history, &pending, &previous_usage);
                parent = match tokio::select! {
                    result = compact => result,
                    changed = cancellation.changed() => {
                        if changed.is_err() || *cancellation.borrow() {
                            Err(EngineError::Cancelled)
                        } else {
                            continue;
                        }
                    }
                } {
                    Ok(request) => request,
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                };
            }
            let history = if did_compact {
                self.read_history(&parent).await?
            } else {
                history
            };
            let pinned_effort = history
                .iter()
                .find_map(Item::configuration_effort)
                .ok_or(EngineError::MissingEffortPin)?;
            let req = ResponsesRequest {
                input: history,
                instructions: self.config.instructions.clone(),
                tools: self.tools(),
                model: self.config.model.clone(),
                // The request-level field is only the cache-preserving mirror
                // of the first positional update in the exact history sent.
                pinned_effort,
                session_id: self.config.session_id.clone(),
            };
            let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);
            let create = self.client.create_streaming(req, event_tx);
            tokio::pin!(create);
            let mut turn_call_ids = Vec::<CallId>::new();
            let mut wait_call = None;
            let mut event_stream_open = true;
            let turn = loop {
                tokio::select! {
                    result = &mut create => match result {
                        Ok(turn) => break turn,
                        Err(error) => {
                            let primary = EngineError::Transport(error);
                            return Err(self.cleanup_pending(primary, &pending).await);
                        }
                    },
                    changed = cancellation.changed() => {
                        if changed.is_err() || *cancellation.borrow() {
                            return Err(self.cleanup_pending(EngineError::Cancelled, &pending).await);
                        }
                    }
                    event = event_rx.recv(), if event_stream_open => {
                        match event {
                            Some(StreamEvent::ItemDone(item)) => {
                                match self.dispatch_completed_item(item, &parent).await {
                                    Ok(Some(call)) => {
                                        if !pending.iter().any(|current| current.call_id == call.call_id) {
                                            if call.is_wait_agent && wait_call.is_some() {
                                                return Err(self.cleanup_pending(
                                                    EngineError::MultipleWaitAgents,
                                                    &pending
                                                ).await);
                                            }
                                            if call.is_wait_agent { wait_call = Some(call.call_id.clone()); }
                                            turn_call_ids.push(call.call_id.clone());
                                            pending.push(call);
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                                }
                            }
                            Some(StreamEvent::Delta(_)) => {}
                            None => event_stream_open = false,
                        }
                    }
                }
            };
            // A completed response can race the buffered final item events.
            while let Ok(event) = event_rx.try_recv() {
                if let StreamEvent::ItemDone(item) = event {
                    match self.dispatch_completed_item(item, &parent).await {
                        Ok(Some(call)) => {
                            if !pending
                                .iter()
                                .any(|current| current.call_id == call.call_id)
                            {
                                if call.is_wait_agent && wait_call.is_some() {
                                    return Err(self
                                        .cleanup_pending(EngineError::MultipleWaitAgents, &pending)
                                        .await);
                                }
                                if call.is_wait_agent {
                                    wait_call = Some(call.call_id.clone());
                                }
                                turn_call_ids.push(call.call_id.clone());
                                pending.push(call);
                            }
                        }
                        Ok(_) => {}
                        Err(error) => {
                            return Err(self.cleanup_pending(error, &pending).await);
                        }
                    }
                }
            }
            // Some injected transports may only return a turn, without emitting
            // item events. The production client emits every completed item.
            for item in &turn.items {
                if item.0["type"] == "function_call" {
                    let Some((call_id, _, _)) = items::function_call(item) else {
                        return Err(self
                            .cleanup_pending(EngineError::InvalidFunctionCall, &pending)
                            .await);
                    };
                    if !pending.iter().any(|current| current.call_id == call_id) {
                        match self.dispatch_completed_item(item.clone(), &parent).await {
                            Ok(Some(call)) => {
                                if call.is_wait_agent && wait_call.is_some() {
                                    return Err(self
                                        .cleanup_pending(EngineError::MultipleWaitAgents, &pending)
                                        .await);
                                }
                                if call.is_wait_agent {
                                    wait_call = Some(call_id.clone());
                                }
                                turn_call_ids.push(call_id.clone());
                                pending.push(call);
                            }
                            Ok(None) => {
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
            if let Err(error) = self.append(&parent, turn.items.clone()).await {
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
                "cache_write_tokens":turn.usage.cache_write_tokens
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

            let settled_this_turn = match self.persist_settled(&mut pending, &parent).await {
                Ok(settled) => settled,
                Err(error) => return Err(self.cleanup_pending(error, &pending).await),
            };

            if let Some(wait_call_id) = wait_call {
                let result = if let Some(call_id) = settled_this_turn.first() {
                    WaitAgentResult {
                        call_outputs: Vec::new(),
                        resumed_by: WaitResume::Job(call_id.clone()),
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
                if matches!(&result.resumed_by, WaitResume::Cancelled) {
                    return Err(self.cleanup_pending(EngineError::Cancelled, &pending).await);
                }
                if let Err(error) = self
                    .persist_wait_result(
                        &mut pending,
                        &parent,
                        Some(&wait_call_id),
                        result,
                        admit_inbox,
                    )
                    .await
                {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            } else if is_final(&turn) && !pending.is_empty() && settled_this_turn.is_empty() {
                let result = match self
                    .wait_for_resume(&pending, &mut envelopes, &mut cancellation)
                    .await
                {
                    Ok(result) => result,
                    Err(error) => return Err(self.cleanup_pending(error, &pending).await),
                };
                if matches!(&result.resumed_by, WaitResume::Cancelled) {
                    return Err(self.cleanup_pending(EngineError::Cancelled, &pending).await);
                }
                if let Err(error) = self
                    .persist_wait_result(&mut pending, &parent, None, result, admit_inbox)
                    .await
                {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            } else if is_final(&turn) && pending.is_empty() {
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
            if let Err(error) = blocking(move || {
                store.write_request(
                    &request,
                    Some(&parent_for_write),
                    &branch,
                    &[],
                    StoredUsage::default(),
                )
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
        let store = self.store.clone();
        let id = id.clone();
        blocking(move || store.append_items(&id, &items)).await?;
        Ok(())
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

    fn tools(&self) -> Vec<serde_json::Value> {
        let mut tools = self.config.tools.clone();
        for tool in self.provider.all_tools() {
            let name = tool.get("name").and_then(serde_json::Value::as_str);
            if name.is_none_or(|name| {
                !tools.iter().any(|existing| {
                    existing.get("name").and_then(serde_json::Value::as_str) == Some(name)
                })
            }) {
                tools.push(tool);
            }
        }
        tools
    }

    // FIXME(correction-wave b): when the model continues past a pending call,
    // the next request resends its `function_call` with no output. The API
    // accepts that only for tools declared `async: true`; no tool or verb
    // schema sets it yet (see agents.rs `function` and the demo `tools`). This
    // path has only run against the mock transport. Prove item 2 live.
    async fn dispatch_completed_item(
        &self,
        item: Item,
        request: &RequestId,
    ) -> Result<Option<PendingCall>, EngineError> {
        if item.0["type"] != "function_call" {
            return Ok(None);
        }
        let Some((call_id, name, args)) = items::function_call(&item) else {
            return Err(EngineError::InvalidFunctionCall);
        };
        let is_wait_agent = name == "wait_agent";
        if is_wait_agent {
            let store = self.store.clone();
            let call = call_id.clone();
            let request_id = request.clone();
            blocking(move || store.claim(&call, &request_id)).await?;
            return Ok(Some(PendingCall {
                call_id,
                claim_request: request.clone(),
                is_wait_agent,
                cancel_job_on_cleanup: false,
            }));
        }
        let provider: Arc<dyn Provider> = self.provider.clone();
        self.scheduler
            .start_for_agent(
                provider,
                self.config.agent.clone(),
                Some(request.clone()),
                call_id.clone(),
                name,
                args,
            )
            .await?;
        if let Err(error) = self
            .scheduler
            .claim(&call_id, self.config.agent.clone())
            .await
        {
            let _ = self.scheduler.cancel(&call_id).await;
            return Err(EngineError::Job(error));
        }
        let store = self.store.clone();
        let call = call_id.clone();
        let request_id = request.clone();
        if let Err(error) = blocking(move || store.claim(&call, &request_id)).await {
            let _ = self.scheduler.cancel(&call_id).await;
            return Err(error);
        }
        Ok(Some(PendingCall {
            call_id,
            claim_request: request.clone(),
            is_wait_agent,
            cancel_job_on_cleanup: true,
        }))
    }

    async fn cancel_pending(&self, pending: &[PendingCall]) -> Result<(), EngineError> {
        let mut first_error = None;
        for call in pending {
            if call.cancel_job_on_cleanup && !call.is_wait_agent {
                if let Err(error) = self.scheduler.cancel(&call.call_id).await {
                    first_error.get_or_insert_with(|| EngineError::Job(error));
                }
            }
            let store = self.store.clone();
            let call_id = call.call_id.clone();
            let request = call.claim_request.clone();
            if let Err(error) = blocking(move || store.interrupt_claim(&call_id, &request)).await {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
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
    ) -> Result<Vec<CallId>, EngineError> {
        let calls: Vec<_> = pending
            .iter()
            .filter(|call| !call.is_wait_agent)
            .map(|call| call.call_id.clone())
            .collect();
        let outputs = outputs_in_call_order(&self.scheduler, &calls).await?;
        let mut settled = Vec::with_capacity(outputs.len());
        for (call_id, output) in outputs {
            self.persist_output(&call_id, &output, request).await?;
            pending.retain(|call| call.call_id != call_id);
            settled.push(call_id);
        }
        Ok(settled)
    }

    async fn persist_output(
        &self,
        call_id: &CallId,
        output: &crate::turn::JobOutput,
        request: &RequestId,
    ) -> Result<(), EngineError> {
        let item = items::function_output(call_id, output);
        let store = self.store.clone();
        let call = call_id.clone();
        let request = request.clone();
        blocking(move || {
            store.write_output(&call, &item)?;
            store.append_items(&request, &[item])?;
            Ok(())
        })
        .await
    }

    async fn wait_for_resume(
        &self,
        pending: &[PendingCall],
        envelopes: &mut tokio::sync::mpsc::UnboundedReceiver<Envelope>,
        cancellation: &mut watch::Receiver<bool>,
    ) -> Result<WaitAgentResult, EngineError> {
        let calls: Vec<_> = pending
            .iter()
            .filter(|call| !call.is_wait_agent)
            .map(|call| call.call_id.clone())
            .collect();
        loop {
            let result =
                wait_agent_and_drain(envelopes, &self.scheduler, cancellation, &calls).await?;
            match &result.resumed_by {
                WaitResume::Job(call_id) if calls.contains(call_id) => return Ok(result),
                WaitResume::Envelope(_) | WaitResume::Cancelled => return Ok(result),
                WaitResume::Job(_) => continue,
            }
        }
    }

    async fn persist_wait_result(
        &self,
        pending: &mut Vec<PendingCall>,
        request: &RequestId,
        wait_call: Option<&CallId>,
        result: WaitAgentResult,
        durable_mailbox: bool,
    ) -> Result<(), EngineError> {
        for (call_id, output) in result.call_outputs {
            self.persist_output(&call_id, &output, request).await?;
            pending.retain(|call| call.call_id != call_id);
        }
        let (agent_envelope, user_envelope, output) = match result.resumed_by {
            WaitResume::Job(call_id) => (None, None, json!({"resumed_by":{"job":call_id.0}})),
            WaitResume::Envelope(envelope) => {
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
            WaitResume::Cancelled => return Err(EngineError::Cancelled),
        };
        if let Some(wait_call) = wait_call {
            self.persist_output(
                wait_call,
                &crate::turn::JobOutput::Completed(Ok(output)),
                request,
            )
            .await?;
            pending.retain(|call| call.call_id != *wait_call);
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

    async fn compact_window(
        &self,
        source: &RequestId,
        history: &[Item],
        pending: &[PendingCall],
        usage: &Usage,
    ) -> Result<RequestId, EngineError> {
        let pending_items: Vec<Item> = pending
            .iter()
            .map(|call| {
                history
                    .iter()
                    .find(|item| {
                        item.0["type"] == "function_call"
                            && item.0["call_id"].as_str() == Some(call.call_id.0.as_str())
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
                        tools: self.tools(),
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
        let window = Server
            .compact(CompactContext {
                items: history,
                usage,
                pending_calls: &pending_items,
                effort: effective_effort,
                server_compact: &server_compact,
                typed_turn: &typed_turn,
            })
            .await?;
        let request = RequestId(uuid::Uuid::new_v4().to_string());
        let store = self.store.clone();
        let source = source.clone();
        let successor = request.clone();
        let branch = self.config.agent.0.clone();
        blocking(move || {
            store.write_compaction_request(&successor, &source, &branch, &window.items)
        })
        .await?;
        Ok(request)
    }
}

async fn load_history(store: Arc<Store>, id: RequestId) -> Result<Vec<Item>, EngineError> {
    blocking(move || {
        let mut cursor = Some(id);
        let mut chain = Vec::new();
        while let Some(request_id) = cursor {
            let Some(request) = store.request(&request_id)? else {
                return Err(StoreError::MissingRequest(request_id.0));
            };
            chain.push(store.items(&request_id)?);
            if store.is_compaction_boundary(&request_id)? {
                break;
            }
            cursor = request.parent;
        }
        chain.reverse();
        Ok(chain.into_iter().flatten().collect())
    })
    .await
}

fn is_final(turn: &ResponsesTurn) -> bool {
    turn.items
        .iter()
        .any(|item| item.0["phase"] == "final_answer")
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
    use crate::agents::AgentToolService;

    fn empty_mailbox() -> tokio::sync::mpsc::UnboundedReceiver<Envelope> {
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        receiver
    }
    use crate::provider::ProviderError;
    use crate::transport::{TransportError, Usage};
    use async_trait::async_trait;
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

    struct SetEffortProvider {
        service: crate::agent_runtime::StoreAgentToolService,
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
                return Err(ProviderError::Tool(format!(
                    "unexpected agent verb: {name}"
                )));
            }
            let effort = match args["effort"].as_str() {
                Some("low") => Effort::Low,
                Some("medium") => Effort::Medium,
                Some("high") => Effort::High,
                _ => return Err(ProviderError::Tool("invalid effort".into())),
            };
            self.service
                .set_effort(&context.agent, effort)
                .await
                .map_err(|error| ProviderError::Tool(error.to_string()))
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
            .start_for_agent(
                provider.clone(),
                AgentPath("/root".into()),
                call_id.clone(),
                "slow".into(),
                json!({}),
            )
            .await
            .unwrap();
        scheduler
            .claim(&call_id, AgentPath("/root".into()))
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
            scheduler.output(&call_id).await.unwrap(),
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
        let settled = scheduler.settled_claimants(&call_id).await.unwrap();
        assert!(settled.contains(&AgentPath("/root".into())));
        assert!(settled.contains(&AgentPath("/root/child".into())));
    }

    #[tokio::test]
    async fn settled_inherited_claim_is_replayed_from_store_before_child_first_request() {
        let (store, _source_head, snapshot, call_id) = inherited_claim_fixture().await;
        let output = Item(json!({
            "type":"function_call_output",
            "call_id":call_id.0,
            "output":"{\"settled_before_start\":true}"
        }));
        store.write_output(&call_id, &output).unwrap();
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
            .start_for_agent(
                provider.clone(),
                AgentPath("/root".into()),
                call_id.clone(),
                "slow".into(),
                json!({}),
            )
            .await
            .unwrap();
        scheduler
            .claim(&call_id, AgentPath("/root".into()))
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
            Err(EngineError::Cancelled)
        ));
        assert_eq!(scheduler.output(&call_id).await.unwrap(), None);
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
                input_tokens: 3,
                output_tokens: 2,
                cached_tokens: 1,
                cache_write_tokens: 0,
            },
        }
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
        assert_eq!(claims[0].state, crate::store::ClaimState::Interrupted);
    }

    #[tokio::test]
    async fn pending_set_effort_follows_tool_output_before_next_model_request() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let replay = Replay {
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
        let provider = Arc::new(SetEffortProvider {
            service: crate::agent_runtime::StoreAgentToolService::new(
                store.clone(),
                AgentPath("/root".into()),
            ),
        });

        let engine = Engine::<FakeAuth, SetEffortProvider, _>::with_transport(
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
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        engine
            .run(Some(head), vec![], cancel_rx, empty_mailbox())
            .await
            .unwrap();

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
        assert!(matches!(run.await.unwrap(), Err(EngineError::Cancelled)));
        assert_eq!(
            scheduler.output(&first).await.unwrap(),
            Some(JobOutput::Cancelled)
        );
        assert_eq!(
            scheduler.output(&second).await.unwrap(),
            Some(JobOutput::Cancelled)
        );
        assert_eq!(
            store.claims(&first).unwrap()[0].state,
            ClaimState::Interrupted
        );
        assert_eq!(
            store.claims(&second).unwrap()[0].state,
            ClaimState::Interrupted
        );
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
            scheduler.output(&call).await.unwrap(),
            Some(JobOutput::Cancelled)
        );
        assert_eq!(
            store.claims(&call).unwrap()[0].state,
            ClaimState::Interrupted
        );
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
            scheduler.output(&call).await.unwrap(),
            Some(JobOutput::Cancelled)
        );
        assert_eq!(
            store.claims(&call).unwrap()[0].state,
            ClaimState::Interrupted
        );
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
        let store = Arc::new(Store::memory().unwrap());
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
