//! Single-agent, stateless Responses request loop.
//!
//! Each model request receives the full accumulated item history. Function
//! calls are started immediately and their settled outputs are appended under
//! their original call ids before the next request.

use crate::{
    item::Item,
    mailbox::{Envelope, MessageChannel},
    model::{AgentPath, CallId, Effort, RequestId},
    provider::Provider,
    store::{Store, StoreError, Usage as StoredUsage},
    transport::{
        Auth, ResponsesClient, ResponsesRequest, ResponsesTurn, TransportError, sse::StreamEvent,
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
    #[error("a model response contained more than one wait_agent call")]
    MultipleWaitAgents,
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
            _auth: std::marker::PhantomData,
        }
    }

    /// Run until an assistant final answer or cancellation. `initial` may be
    /// an entire prior transcript; it is never truncated by this engine.
    pub async fn run(
        &self,
        initial: Vec<Item>,
        cancellation: watch::Receiver<bool>,
    ) -> Result<ResponsesTurn, EngineError> {
        let (keepalive, envelopes) = tokio::sync::mpsc::unbounded_channel();
        let result = self
            .run_with_envelopes(initial, cancellation, envelopes)
            .await;
        drop(keepalive);
        result
    }

    /// Run with the agent's mailbox receiver. This is required for
    /// `wait_agent` to resume on envelope delivery; `run` remains suitable
    /// when the host has no mailbox source.
    pub async fn run_with_envelopes(
        &self,
        initial: Vec<Item>,
        cancellation: watch::Receiver<bool>,
        mut incoming: tokio::sync::mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<ResponsesTurn, EngineError> {
        let (envelope_tx, envelopes) = tokio::sync::mpsc::unbounded_channel();
        let keepalive = envelope_tx.clone();
        let forwarder = tokio::spawn(async move {
            while let Some(envelope) = incoming.recv().await {
                if envelope_tx.send(envelope).is_err() {
                    break;
                }
            }
        });
        let result = self.run_loop(initial, cancellation, envelopes).await;
        drop(keepalive);
        forwarder.abort();
        result
    }

    async fn run_loop(
        &self,
        initial: Vec<Item>,
        mut cancellation: watch::Receiver<bool>,
        mut envelopes: tokio::sync::mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<ResponsesTurn, EngineError> {
        let id = RequestId(uuid::Uuid::new_v4().to_string());
        let store = self.store.clone();
        let request = id.clone();
        let branch = self.config.agent.0.clone();
        blocking(move || {
            store.write_request(&request, None, &branch, &initial, StoredUsage::default())
        })
        .await?;
        let mut parent = id.clone();
        let mut pending = Vec::<PendingCall>::new();
        loop {
            if *cancellation.borrow() {
                return Err(self.cleanup_pending(EngineError::Cancelled, &pending).await);
            }
            let history = match self.read_history(&parent).await {
                Ok(history) => history,
                Err(error) => return Err(self.cleanup_pending(error, &pending).await),
            };
            let req = ResponsesRequest {
                input: history,
                instructions: self.config.instructions.clone(),
                tools: self.tools(),
                model: self.config.model.clone(),
                pinned_effort: self.config.effort,
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
                    .persist_wait_result(&mut pending, &parent, Some(&wait_call_id), result)
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
                    .persist_wait_result(&mut pending, &parent, None, result)
                    .await
                {
                    return Err(self.cleanup_pending(error, &pending).await);
                }
            } else if is_final(&turn) && pending.is_empty() {
                return Ok(turn);
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
            }));
        }
        let provider: Arc<dyn Provider> = self.provider.clone();
        self.scheduler
            .start_for_agent(
                provider,
                self.config.agent.clone(),
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
        }))
    }

    async fn cancel_pending(&self, pending: &[PendingCall]) -> Result<(), EngineError> {
        let mut first_error = None;
        for call in pending {
            if !call.is_wait_agent {
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
        if let Some(item) = agent_envelope {
            self.append(request, vec![item]).await?;
        }
        if let Some(wait_call) = wait_call {
            self.persist_output(
                wait_call,
                &crate::turn::JobOutput::Completed(Ok(output)),
                request,
            )
            .await?;
            pending.retain(|call| call.call_id != *wait_call);
        }
        if let Some(item) = user_envelope {
            self.append(request, vec![item]).await?;
        }
        Ok(())
    }

    async fn read_history(&self, id: &RequestId) -> Result<Vec<Item>, EngineError> {
        load_history(self.store.clone(), id.clone()).await
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
            .run(vec![Item(json!({"role":"user","content":"go"}))], cancel_rx)
            .await
            .unwrap();
        assert_eq!(result.response_id, "r3");

        let expected_history = {
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            assert_eq!(requests[0].input.len(), 1);
            let second = &requests[1].input;
            assert_eq!(second[0].0["content"], "go");
            assert_eq!(second[1].0["type"], "function_call");
            assert_eq!(second[2].0["type"], "function_call_output");
            assert_eq!(second[2].0["call_id"], "c1");
            let output: Value =
                serde_json::from_str(second[2].0["output"].as_str().unwrap()).unwrap();
            assert_eq!(output, json!({"tool":"echo","args":{"x":1}}));
            let third = &requests[2].input;
            assert_eq!(third[3].0["type"], "function_call");
            assert_eq!(third[3].0["call_id"], "c2");
            assert_eq!(third[4].0["type"], "function_call_output");
            assert_eq!(third[4].0["call_id"], "c2");
            let output: Value =
                serde_json::from_str(third[4].0["output"].as_str().unwrap()).unwrap();
            assert_eq!(output, json!({"tool":"echo","args":{"x":2}}));
            let mut expected_history = third.clone();
            expected_history.push(Item(json!({
                "type":"message","role":"assistant","phase":"final_answer","content":"done"
            })));
            expected_history
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
            .run(vec![Item(json!({"role":"user","content":"go"}))], cancel_rx)
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
                .run(vec![Item(json!({"role":"user","content":"go"}))], cancel_rx)
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
                .run(vec![Item(json!({"role":"user","content":"go"}))], cancel_rx)
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
                .run(vec![Item(json!({"role":"user","content":"go"}))], cancel_rx)
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
                .run(vec![Item(json!({"role":"user","content":"go"}))], cancel_rx)
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
                .run(vec![Item(json!({"role":"user","content":"go"}))], cancel_rx)
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), second_request.notified())
            .await
            .expect("model continued while slow tool remained pending");
        assert!(!released.load(std::sync::atomic::Ordering::SeqCst));
        release_tx.send(()).unwrap();
        let final_result = tokio::time::timeout(std::time::Duration::from_secs(3), run).await;
        assert_eq!(
            final_result.unwrap().unwrap().unwrap().response_id,
            "async-final"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        let resumed_history = &requests[2].input;
        assert_eq!(resumed_history[1].0["call_id"], "slow-call");
        assert_eq!(resumed_history[2].0["call_id"], "wait-call");
        assert_eq!(resumed_history[3].0["call_id"], "slow-call");
        assert_eq!(resumed_history[4].0["call_id"], "wait-call");
        let slow_output: Value =
            serde_json::from_str(resumed_history[3].0["output"].as_str().unwrap()).unwrap();
        assert_eq!(slow_output, json!({"slow_done":true}));
        let wait_output: Value =
            serde_json::from_str(resumed_history[4].0["output"].as_str().unwrap()).unwrap();
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
                .run(vec![Item(json!({"role":"user","content":"go"}))], cancel_rx)
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
        assert_eq!(run.await.unwrap().unwrap().response_id, "late-continued");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].input[1].0["call_id"], "late-call");
        assert_eq!(requests[1].input[3].0["call_id"], "late-call");
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
                .run_with_envelopes(
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
        envelope_tx
            .send(Envelope {
                kind: crate::mailbox::EnvelopeType::Message,
                recipient: AgentPath("/root".into()),
                sender: AgentPath("/root/worker".into()),
                payload: "arrived".into(),
                class: crate::mailbox::DeliveryClass::AtBoundary,
                timestamp_ms: 1,
            })
            .unwrap();
        assert_eq!(run.await.unwrap().unwrap().response_id, "resumed");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let next = &requests[1].input;
        assert_eq!(next[1].0["call_id"], "wait-envelope");
        assert_eq!(next[2].0["type"], "message");
        assert_eq!(
            next[2].0["content"],
            "Message Type: MESSAGE\nTask name: /root\nSender: /root/worker\nPayload:\narrived"
        );
        assert_eq!(next[3].0["call_id"], "wait-envelope");
        let output: Value = serde_json::from_str(next[3].0["output"].as_str().unwrap()).unwrap();
        assert_eq!(output, json!({"resumed_by":{"agent":"/root/worker"}}));
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
                vec![Item(
                    json!({"role":"user","content":"Reply exactly ENGINE_OK"}),
                )],
                cancel_rx,
            )
            .await
            .expect("live engine turn");
        assert!(
            turn.items
                .iter()
                .any(|item| item.0["phase"] == "final_answer")
        );
    }
}
