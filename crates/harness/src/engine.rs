//! Single-agent, stateless Responses request loop.
//!
//! Each model request receives the full accumulated item history. Function
//! calls are started immediately and their settled outputs are appended under
//! their original call ids before the next request.

use crate::{
    item::Item,
    model::{AgentPath, CallId, Effort, RequestId},
    provider::Provider,
    store::{Store, StoreError, Usage as StoredUsage},
    transport::{
        Auth, ResponsesClient, ResponsesRequest, ResponsesTurn, TransportError, sse::StreamEvent,
    },
    turn::{JobError, JobScheduler},
};
use serde_json::json;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::watch;

#[path = "engine/items.rs"]
mod items;

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
        mut cancellation: watch::Receiver<bool>,
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
        loop {
            if *cancellation.borrow() {
                return Err(EngineError::Cancelled);
            }
            let history = self.read_history(&parent).await?;
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
            let mut calls = Vec::<CallId>::new();
            let mut event_stream_open = true;
            let turn = loop {
                tokio::select! {
                    result = &mut create => match result {
                        Ok(turn) => break turn,
                        Err(error) => {
                            let primary = EngineError::Transport(error);
                            return Err(self.cleanup_after(primary, &calls, &parent).await);
                        }
                    },
                    changed = cancellation.changed() => {
                        if changed.is_err() || *cancellation.borrow() {
                            return Err(self.cleanup_after(EngineError::Cancelled, &calls, &parent).await);
                        }
                    }
                    event = event_rx.recv(), if event_stream_open => {
                        match event {
                            Some(StreamEvent::ItemDone(item)) => {
                                match self.dispatch_completed_item(item, &parent).await {
                                    Ok(Some(call_id)) if !calls.contains(&call_id) => calls.push(call_id),
                                    Ok(_) => {}
                                    Err(error) => return Err(self.cleanup_after(error, &calls, &parent).await),
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
                        Ok(Some(call_id)) if !calls.contains(&call_id) => calls.push(call_id),
                        Ok(_) => {}
                        Err(error) => {
                            return Err(self.cleanup_after(error, &calls, &parent).await);
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
                            .cleanup_after(EngineError::InvalidFunctionCall, &calls, &parent)
                            .await);
                    };
                    if !calls.contains(&call_id) {
                        match self.dispatch_completed_item(item.clone(), &parent).await {
                            Ok(Some(_)) => {}
                            Ok(None) => {
                                return Err(self
                                    .cleanup_after(
                                        EngineError::InvalidFunctionCall,
                                        &calls,
                                        &parent,
                                    )
                                    .await);
                            }
                            Err(error) => {
                                return Err(self.cleanup_after(error, &calls, &parent).await);
                            }
                        }
                        calls.push(call_id);
                    }
                }
            }
            if let Err(error) = self.append(&parent, turn.items.clone()).await {
                return Err(self.cleanup_after(error, &calls, &parent).await);
            }
            let usage_for_store = StoredUsage {
                input_tokens: i64::try_from(turn.usage.input_tokens).unwrap_or(i64::MAX),
                output_tokens: i64::try_from(turn.usage.output_tokens).unwrap_or(i64::MAX),
                cost_micros: 0,
            };
            let store = self.store.clone();
            let request = parent.clone();
            if let Err(error) = blocking(move || store.set_usage(&request, usage_for_store)).await {
                return Err(self.cleanup_after(error, &calls, &parent).await);
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
                return Err(self.cleanup_after(error, &calls, &parent).await);
            }
            if calls.is_empty() {
                if turn
                    .items
                    .iter()
                    .any(|item| item.0["phase"] == "final_answer")
                {
                    return Ok(turn);
                }
                return Err(EngineError::MissingFinal);
            }
            for call_id in &calls {
                let call_id = call_id.clone();
                let output = loop {
                    tokio::select! {
                        output = self.scheduler.wait(&call_id) => match output {
                            Ok(output) => break output,
                            Err(error) => return Err(self.cleanup_after(
                                EngineError::Job(error), &calls, &parent
                            ).await),
                        },
                        changed = cancellation.changed() => {
                            if changed.is_err() || *cancellation.borrow() {
                                return Err(self.cleanup_after(EngineError::Cancelled, &calls, &parent).await);
                            }
                        }
                    }
                };
                let item = items::function_output(&call_id, &output);
                let c = call_id.clone();
                let request = parent.clone();
                let saved = item.clone();
                let store = self.store.clone();
                let saved_result = blocking(move || {
                    store.write_output(&c, &saved)?;
                    store.append_items(&request, &[saved])?;
                    Ok::<_, StoreError>(())
                })
                .await;
                if let Err(error) = saved_result {
                    return Err(self.cleanup_after(error, &calls, &parent).await);
                }
            }
            let next_id = RequestId(uuid::Uuid::new_v4().to_string());
            let store = self.store.clone();
            let branch = self.config.agent.0.clone();
            let parent_for_write = parent.clone();
            let request = next_id.clone();
            blocking(move || {
                store.write_request(
                    &request,
                    Some(&parent_for_write),
                    &branch,
                    &[],
                    StoredUsage::default(),
                )
            })
            .await?;
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
    ) -> Result<Option<CallId>, EngineError> {
        if item.0["type"] != "function_call" {
            return Ok(None);
        }
        if items::function_call(&item).is_none() {
            return Err(EngineError::InvalidFunctionCall);
        }
        let provider: Arc<dyn Provider> = self.provider.clone();
        let handle = self
            .scheduler
            .start_completed_item_for_agent(provider, self.config.agent.clone(), &item)
            .await?;
        let Some(handle) = handle else {
            return Ok(None);
        };
        let call_id = CallId(handle.0);
        if let Err(error) = self
            .scheduler
            .claim(&call_id, self.config.agent.clone())
            .await
        {
            return Err(self
                .cleanup_after(
                    EngineError::Job(error),
                    std::slice::from_ref(&call_id),
                    request,
                )
                .await);
        }
        let store = self.store.clone();
        let call = call_id.clone();
        let request_id = request.clone();
        if let Err(error) = blocking(move || store.claim(&call, &request_id)).await {
            return Err(self
                .cleanup_after(error, std::slice::from_ref(&call_id), request)
                .await);
        }
        Ok(Some(call_id))
    }

    async fn cancel_calls(&self, calls: &[CallId], request: &RequestId) -> Result<(), EngineError> {
        let mut first_error = None;
        for call_id in calls {
            if let Err(error) = self.scheduler.cancel(call_id).await {
                first_error.get_or_insert_with(|| EngineError::Job(error));
            }
            let store = self.store.clone();
            let call = call_id.clone();
            let request = request.clone();
            if let Err(error) = blocking(move || store.interrupt_claim(&call, &request)).await {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn cleanup_after(
        &self,
        primary: EngineError,
        calls: &[CallId],
        request: &RequestId,
    ) -> EngineError {
        match self.cancel_calls(calls, request).await {
            Ok(()) => primary,
            Err(cleanup) => EngineError::Cleanup {
                primary: Box::new(primary),
                cleanup: cleanup.to_string(),
            },
        }
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
}
