//! Offline replay fixtures for the resident adapter slice.
//!
//! `ReplayTransport` supplies a deterministic sequence of model turns and
//! retains every request it receives. `FakeResidentCell` is a controllable
//! asynchronous evaluator: each call remains pending until the test releases
//! a complete output, and its typed state is persisted in the ordinary
//! `session_state` table.

use crate::{
    cell_job::{CellInput, CellJob, CellOutput},
    engine::ResponsesTransport,
    model::RequestId,
    provider::{CallContext, Provider, ProviderError},
    store::{RecordedReplayTurn, Store},
    transport::{ResponsesRequest, ResponsesTurn, TransportError, sse::StreamEvent},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

/// Replays recorded model requests and tool outputs from a durable Store.
///
/// Turns are loaded from the selected request branch when constructed. Each
/// transport request must exactly match the next recorded request before its
/// response is consumed; a mismatch leaves the turn available for a corrected
/// request. Provider calls are resolved by their durable call id.
pub struct ReplayProvider {
    store: Arc<Store>,
    turns: Mutex<VecDeque<RecordedReplayTurn>>,
    tool_schemas: Vec<serde_json::Value>,
}

impl ReplayProvider {
    /// Load all recorded turns on `root`'s branch from the Store.
    pub fn new(store: Arc<Store>, root: &RequestId) -> crate::store::Result<Self> {
        let turns = store.replay_turns(root)?;
        let tool_schemas = turns
            .first()
            .map(|turn| turn.model_request.tools.clone())
            .unwrap_or_default();
        Ok(Self {
            store,
            turns: Mutex::new(turns.into()),
            tool_schemas,
        })
    }

    /// Number of recorded model turns not yet consumed.
    pub fn turns_remaining(&self) -> usize {
        lock(&self.turns).len()
    }
}

#[async_trait]
impl ResponsesTransport for ReplayProvider {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        let mut turns = lock(&self.turns);
        let Some(next) = turns.front() else {
            return Err(TransportError::Stream(
                "replay provider has no recorded model turns remaining".into(),
            ));
        };
        if let Some(field) = request_mismatch(&next.model_request, &request) {
            return Err(TransportError::Stream(format!(
                "replay request does not match the next recorded request ({field})"
            )));
        }
        Ok(turns.pop_front().expect("front was present").model_response)
    }
}

#[async_trait]
impl Provider for ReplayProvider {
    async fn call(
        &self,
        name: &str,
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        Err(ProviderError::Tool(format!(
            "replay provider requires call context for tool `{name}`"
        )))
    }

    async fn call_with_context(
        &self,
        _name: &str,
        _args: serde_json::Value,
        context: CallContext,
    ) -> Result<serde_json::Value, ProviderError> {
        let output = self
            .store
            .replay_output(&context.call_id)
            .map_err(|error| ProviderError::Tool(format!("loading replay output: {error}")))?
            .ok_or_else(|| {
                ProviderError::Tool(format!(
                    "no settled replay output for call `{}`",
                    context.call_id.0
                ))
            })?;
        if output.0["type"] != "function_call_output" || output.0["call_id"] != context.call_id.0 {
            return Err(ProviderError::Tool(format!(
                "stored replay output does not match call `{}`",
                context.call_id.0
            )));
        }
        let value = output.0.get("output").cloned().ok_or_else(|| {
            ProviderError::Tool(format!(
                "stored replay output for call `{}` has no output value",
                context.call_id.0
            ))
        })?;
        match value {
            serde_json::Value::String(serialized) => {
                serde_json::from_str(&serialized).map_err(|error| {
                    ProviderError::Tool(format!(
                        "decoding replay output for call `{}`: {error}",
                        context.call_id.0
                    ))
                })
            }
            value => Ok(value),
        }
    }

    fn tools(&self) -> Vec<serde_json::Value> {
        self.tool_schemas.clone()
    }
}

fn request_mismatch(
    expected: &ResponsesRequest,
    actual: &ResponsesRequest,
) -> Option<&'static str> {
    if expected.input != actual.input {
        Some("input")
    } else if expected.instructions != actual.instructions {
        Some("instructions")
    } else if expected.tools != actual.tools {
        Some("tools")
    } else if expected.model != actual.model {
        Some("model")
    } else if expected.pinned_effort != actual.pinned_effort {
        Some("pinned_effort")
    } else if expected.session_id != actual.session_id {
        Some("session_id")
    } else {
        None
    }
}

/// A FIFO transport for offline tests. Requests are recorded before the next
/// turn is removed from the queue, including requests whose queue is exhausted.
pub struct ReplayTransport {
    turns: Mutex<VecDeque<ResponsesTurn>>,
    requests: Mutex<Vec<ResponsesRequest>>,
    request_count: Mutex<usize>,
    observer: Option<Arc<dyn Fn(&ResponsesRequest, &mut ResponsesTurn, usize) + Send + Sync>>,
    gated: bool,
    response_permits: Mutex<usize>,
    changed: Notify,
}

impl ReplayTransport {
    pub fn new(turns: impl IntoIterator<Item = ResponsesTurn>) -> Self {
        Self::with_gate(turns, false)
    }

    /// Create a replay transport which pauses before returning each queued
    /// turn. Tests coordinate boundaries using `wait_requested` and
    /// `release_next`, without sleeps or polling.
    pub fn gated(turns: impl IntoIterator<Item = ResponsesTurn>) -> Self {
        Self::with_gate(turns, true)
    }

    fn with_gate(turns: impl IntoIterator<Item = ResponsesTurn>, gated: bool) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
            request_count: Mutex::new(0),
            observer: None,
            gated,
            response_permits: Mutex::new(0),
            changed: Notify::new(),
        }
    }

    /// Observe and mutate each successful replay response before it is
    /// returned (or released by a gated transport). The ordinal is 1-based
    /// and local to this transport, so factory tests can capture shared
    /// per-agent request logs and synthesize usage deterministically.
    pub fn with_observer(
        mut self,
        observer: Arc<dyn Fn(&ResponsesRequest, &mut ResponsesTurn, usize) + Send + Sync>,
    ) -> Self {
        self.observer = Some(observer);
        self
    }

    /// All requests in arrival order, including their full input envelopes.
    pub fn recorded_requests(&self) -> Vec<ResponsesRequest> {
        lock(&self.requests).clone()
    }

    pub fn queue_remaining(&self) -> usize {
        lock(&self.turns).len()
    }

    /// Wait until at least `count` model requests have crossed the transport
    /// boundary and been recorded.
    pub async fn wait_requested(&self, count: usize) {
        loop {
            let changed = self.changed.notified();
            if self.recorded_requests().len() >= count {
                return;
            }
            changed.await;
        }
    }

    /// Permit exactly one gated model response to return to the caller.
    pub fn release_next(&self) {
        *lock(&self.response_permits) += 1;
        self.changed.notify_waiters();
    }

    async fn wait_for_response_release(&self) {
        if !self.gated {
            return;
        }
        loop {
            let changed = self.changed.notified();
            {
                let mut permits = lock(&self.response_permits);
                if *permits > 0 {
                    *permits -= 1;
                    return;
                }
            }
            changed.await;
        }
    }
}

#[async_trait]
impl ResponsesTransport for ReplayTransport {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        lock(&self.requests).push(request.clone());
        let mut turn = lock(&self.turns)
            .pop_front()
            .ok_or_else(|| TransportError::Stream("replay turn queue exhausted".into()))?;
        let ordinal = {
            let mut count = lock(&self.request_count);
            *count += 1;
            *count
        };
        if let Some(observer) = &self.observer {
            observer(&request, &mut turn, ordinal);
        }
        self.changed.notify_waiters();
        self.wait_for_response_release().await;
        Ok(turn)
    }

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

/// Typed key for fake evaluator state in `Store::session_state`.
///
/// The key is intentionally a normal application session key, not a reserved
/// `harness:` key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplaySessionKey(String);

impl ReplaySessionKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn key(&self) -> &str {
        &self.0
    }
}

/// Durable state maintained by the fake resident evaluator.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReplayCellState {
    pub evaluations: u64,
    pub last_source: Option<String>,
}

impl ReplayCellState {
    /// Load the evaluator state through any Store handle, including a reopened
    /// handle to the same database.
    pub fn load(store: &Store, key: &ReplaySessionKey) -> crate::store::Result<Self> {
        store
            .session_state(key.key())?
            .map(|row| serde_json::from_str(&row.state).map_err(Into::into))
            .transpose()
            .map(|state| state.unwrap_or_default())
    }
}

struct CellControl {
    starts: usize,
    cancels: usize,
    released: VecDeque<CellOutput>,
}

/// A fake `CellJob` which waits until explicitly released.
///
/// Dropping a still-pending `run` future (the scheduler's cancellation path)
/// increments `cancel_count`. A released run updates typed durable state and
/// returns the entire `CellOutput` unchanged.
pub struct FakeResidentCell {
    store: Arc<Store>,
    key: ReplaySessionKey,
    control: Mutex<CellControl>,
    changed: Notify,
}

impl FakeResidentCell {
    pub fn new(store: Arc<Store>, key: ReplaySessionKey) -> Self {
        Self {
            store,
            key,
            control: Mutex::new(CellControl {
                starts: 0,
                cancels: 0,
                released: VecDeque::new(),
            }),
            changed: Notify::new(),
        }
    }

    pub fn start_count(&self) -> usize {
        lock(&self.control).starts
    }

    pub fn cancel_count(&self) -> usize {
        lock(&self.control).cancels
    }

    /// Wait until at least `count` cell calls have entered `run`.
    pub async fn wait_started(&self, count: usize) {
        loop {
            let changed = self.changed.notified();
            if self.start_count() >= count {
                return;
            }
            changed.await;
        }
    }

    /// Make one waiting call complete with this exact output.
    pub fn release(&self, output: CellOutput) {
        lock(&self.control).released.push_back(output);
        self.changed.notify_waiters();
    }

    pub fn query_state(&self, store: &Store) -> crate::store::Result<ReplayCellState> {
        ReplayCellState::load(store, &self.key)
    }

    async fn take_release(&self) -> CellOutput {
        loop {
            let changed = self.changed.notified();
            if let Some(output) = lock(&self.control).released.pop_front() {
                return output;
            }
            changed.await;
        }
    }
}

struct PendingRun<'a> {
    cell: &'a FakeResidentCell,
    completed: AtomicBool,
}

impl PendingRun<'_> {
    fn complete(&self) {
        self.completed.store(true, Ordering::Release);
    }
}

impl Drop for PendingRun<'_> {
    fn drop(&mut self) {
        if !self.completed.load(Ordering::Acquire) {
            lock(&self.cell.control).cancels += 1;
            self.cell.changed.notify_waiters();
        }
    }
}

#[async_trait]
impl CellJob for FakeResidentCell {
    async fn run(
        &self,
        input: CellInput,
        _context: CallContext,
    ) -> Result<CellOutput, ProviderError> {
        lock(&self.control).starts += 1;
        self.changed.notify_waiters();
        let pending = PendingRun {
            cell: self,
            completed: AtomicBool::new(false),
        };
        let output = self.take_release().await;
        let state = ReplayCellState::load(&self.store, &self.key)
            .map_err(|error| ProviderError::Tool(format!("loading replay cell state: {error}")))?;
        let state = ReplayCellState {
            evaluations: state.evaluations + 1,
            last_source: Some(input.source),
        };
        let json = serde_json::to_value(state)
            .map_err(|error| ProviderError::Tool(format!("encoding replay cell state: {error}")))?;
        self.store
            .save_session_state(self.key.key(), &json)
            .map_err(|error| ProviderError::Tool(format!("saving replay cell state: {error}")))?;
        pending.complete();
        Ok(output)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::{Engine, EngineConfig},
        item::Item,
        mailbox::Envelope,
        model::{AgentPath, CallId, Effort, RequestId},
        provider::{JobHandle, Provider},
        store::Store,
        transport::{Auth, Usage},
        turn::JobScheduler,
    };
    use serde_json::json;

    #[derive(Clone)]
    struct OfflineAuth;

    impl Auth for OfflineAuth {
        fn access(&self) -> Result<(String, String), TransportError> {
            Err(TransportError::Authentication)
        }
    }

    struct EchoTool;

    #[async_trait]
    impl Provider for EchoTool {
        async fn call(
            &self,
            name: &str,
            args: serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            if name != "echo" {
                return Err(ProviderError::Tool(format!("unexpected tool `{name}`")));
            }
            Ok(json!({"echo": args["value"]}))
        }

        fn tools(&self) -> Vec<serde_json::Value> {
            vec![json!({
                "type":"function",
                "name":"echo",
                "description":"Return the supplied value.",
                "parameters":{
                    "type":"object",
                    "properties":{"value":{"type":"integer"}},
                    "required":["value"],
                    "additionalProperties":false
                }
            })]
        }
    }

    fn empty_incoming() -> tokio::sync::mpsc::UnboundedReceiver<Envelope> {
        let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        receiver
    }

    fn call_context(call_id: CallId) -> CallContext {
        let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
        CallContext {
            handle: JobHandle(format!("replay-{}", call_id.0)),
            call_id,
            agent: AgentPath("/root".into()),
            request: None,
            progress,
        }
    }

    fn final_turn() -> ResponsesTurn {
        ResponsesTurn {
            response_id: "echo-final".into(),
            items: vec![Item(json!({
                "type":"message",
                "role":"assistant",
                "phase":"final_answer",
                "content":"echo complete"
            }))],
            usage: Usage::default(),
        }
    }

    fn echo_config() -> EngineConfig {
        EngineConfig {
            instructions: "Use echo and then answer.".into(),
            tools: vec![],
            model: "offline-replay-test".into(),
            effort: Effort::Low,
            session_id: "durable-replay-session".into(),
            agent: AgentPath("/root".into()),
        }
    }

    fn echo_input() -> Vec<Item> {
        vec![Item(json!({
            "type":"message",
            "role":"user",
            "content":"echo 42"
        }))]
    }

    fn turn(id: &str) -> ResponsesTurn {
        ResponsesTurn {
            response_id: id.into(),
            items: vec![],
            usage: Usage::default(),
        }
    }

    fn request(session_id: &str) -> ResponsesRequest {
        ResponsesRequest {
            input: vec![Item(json!({"type":"message","role":"user","content":[]}))],
            instructions: "offline".into(),
            tools: vec![],
            model: "test-model".into(),
            pinned_effort: Effort::Medium,
            session_id: session_id.into(),
        }
    }

    #[tokio::test]
    async fn transport_records_requests_and_consumes_turns_in_order() {
        let replay = ReplayTransport::new([turn("r1"), turn("r2")]);
        assert_eq!(
            replay.create(request("s1")).await.unwrap().response_id,
            "r1"
        );
        assert_eq!(
            replay.create(request("s2")).await.unwrap().response_id,
            "r2"
        );
        assert_eq!(replay.queue_remaining(), 0);
        assert_eq!(
            replay
                .recorded_requests()
                .iter()
                .map(|request| request.session_id.as_str())
                .collect::<Vec<_>>(),
            ["s1", "s2"]
        );
        assert!(replay.create(request("s3")).await.is_err());
        assert_eq!(replay.recorded_requests().len(), 3);
    }

    #[tokio::test]
    async fn gated_transport_waits_for_each_boundary_release() {
        let replay = Arc::new(ReplayTransport::gated([turn("r1"), turn("r2")]));
        let running = {
            let replay = replay.clone();
            tokio::spawn(async move {
                let mut ids = Vec::new();
                for index in 1..=2 {
                    let turn = replay
                        .create(request(&format!("session-{index}")))
                        .await
                        .unwrap();
                    ids.push(turn.response_id);
                }
                ids
            })
        };

        replay.wait_requested(1).await;
        assert_eq!(replay.recorded_requests().len(), 1);
        assert!(!running.is_finished(), "first response must be gated");
        replay.release_next();

        // The next request cannot cross the boundary until the first response
        // was released, and its own response needs an independent permit.
        replay.wait_requested(2).await;
        assert_eq!(replay.recorded_requests().len(), 2);
        assert!(!running.is_finished(), "second response must be gated");
        replay.release_next();
        assert_eq!(running.await.unwrap(), ["r1", "r2"]);
    }

    #[tokio::test]
    async fn observer_records_full_requests_and_mutates_turn_before_gate_release() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observer = {
            let observed = observed.clone();
            Arc::new(
                move |request: &ResponsesRequest, turn: &mut ResponsesTurn, ordinal: usize| {
                    observed.lock().unwrap().push((request.clone(), ordinal));
                    turn.usage.input_tokens = request.input.len() as u64 * 10 + ordinal as u64;
                },
            )
        };
        let replay =
            Arc::new(ReplayTransport::gated([turn("r1"), turn("r2")]).with_observer(observer));
        let running = {
            let replay = replay.clone();
            tokio::spawn(async move {
                let first = replay.create(request("first-session")).await.unwrap();
                let mut second_request = request("second-session");
                second_request
                    .input
                    .push(Item(json!({"type":"message","content":"extra"})));
                let second = replay.create(second_request).await.unwrap();
                [first.usage.input_tokens, second.usage.input_tokens]
            })
        };

        replay.wait_requested(1).await;
        assert_eq!(replay.recorded_requests()[0].session_id, "first-session");
        assert_eq!(observed.lock().unwrap()[0].1, 1);
        assert_eq!(
            observed.lock().unwrap()[0].0.input,
            replay.recorded_requests()[0].input
        );
        assert!(!running.is_finished());
        replay.release_next();

        replay.wait_requested(2).await;
        assert_eq!(replay.recorded_requests()[1].session_id, "second-session");
        assert_eq!(replay.recorded_requests()[1].input.len(), 2);
        assert_eq!(observed.lock().unwrap()[1].1, 2);
        assert!(!running.is_finished());
        replay.release_next();
        assert_eq!(running.await.unwrap(), [11, 22]);
    }

    #[tokio::test]
    async fn replay_provider_reopens_store_and_replays_engine_tool_call_offline() {
        let path = std::env::temp_dir().join(format!(
            "harness-replay-provider-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let call_id = CallId("durable-echo-call".into());
        let call = Item(json!({
            "type":"function_call",
            "call_id":call_id.0,
            "name":"echo",
            "arguments":"{\"value\":42}"
        }));
        let source_store = Arc::new(Store::open(&path).unwrap());
        let source_scheduler = Arc::new(JobScheduler::new(2).unwrap());
        let source_engine = Engine::<OfflineAuth, EchoTool, _>::with_transport(
            ReplayTransport::new([
                ResponsesTurn {
                    response_id: "echo-call".into(),
                    items: vec![call.clone()],
                    usage: Usage::default(),
                },
                final_turn(),
            ]),
            source_store.clone(),
            source_scheduler.clone(),
            Arc::new(EchoTool),
            echo_config(),
        );
        let (_source_cancel, source_cancel_rx) = tokio::sync::watch::channel(false);
        let source_completion = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            source_engine.run(None, echo_input(), source_cancel_rx, empty_incoming()),
        )
        .await
        .expect("recording engine completes without network access")
        .expect("recording engine succeeds");
        let source_root = {
            let mut request = source_completion.head_request.clone();
            loop {
                let record = source_store
                    .request(&request)
                    .unwrap()
                    .expect("engine request remains in Store");
                match record.parent {
                    Some(parent) => request = parent,
                    None => break request,
                }
            }
        };
        let original_output = source_store
            .replay_output(&call_id)
            .unwrap()
            .expect("recording engine settles its provider call");
        assert_eq!(original_output.0["type"], "function_call_output");
        assert_eq!(original_output.0["call_id"], call_id.0);
        drop(source_engine);
        drop(source_scheduler);
        drop(source_store);

        // Reopen the durable recording independently; neither model turns nor
        // settled tool output may depend on the original Store handle.
        let reopened_store = Arc::new(Store::open(&path).unwrap());
        let recorded_turns = reopened_store.replay_turns(&source_root).unwrap();
        assert_eq!(recorded_turns.len(), 2);
        assert_eq!(recorded_turns[0].model_response.items, vec![call]);
        assert_eq!(
            recorded_turns[0].model_request.input,
            vec![
                echo_input()[0].clone(),
                Item::configuration_update(Effort::Low)
            ],
            "the recorded envelope retains exact model input"
        );
        assert_eq!(
            reopened_store.replay_output(&call_id).unwrap(),
            Some(original_output.clone())
        );

        // A near-match must not consume a turn; matching includes every
        // ResponsesRequest field, not just the FIFO position.
        let match_probe = ReplayProvider::new(reopened_store.clone(), &source_root).unwrap();
        let mut mismatched_input = recorded_turns[0].model_request.clone();
        mismatched_input.input.push(Item(
            json!({"type":"message","role":"user","content":"extra"}),
        ));
        let mut mismatched_instructions = recorded_turns[0].model_request.clone();
        mismatched_instructions.instructions.push_str(" different");
        let mut mismatched_tools = recorded_turns[0].model_request.clone();
        mismatched_tools.tools.push(json!({"name":"unexpected"}));
        let mut mismatched_model = recorded_turns[0].model_request.clone();
        mismatched_model.model.push_str("-different");
        let mut mismatched_effort = recorded_turns[0].model_request.clone();
        mismatched_effort.pinned_effort = Effort::High;
        let mut mismatched_session = recorded_turns[0].model_request.clone();
        mismatched_session.session_id.push_str("-different");
        for mismatched in [
            mismatched_input,
            mismatched_instructions,
            mismatched_tools,
            mismatched_model,
            mismatched_effort,
            mismatched_session,
        ] {
            assert!(match_probe.create(mismatched).await.is_err());
            assert_eq!(match_probe.turns_remaining(), 2);
        }

        let replay_transport = ReplayProvider::new(reopened_store.clone(), &source_root).unwrap();
        let replay_provider =
            Arc::new(ReplayProvider::new(reopened_store.clone(), &source_root).unwrap());
        let replay_destination = Arc::new(Store::memory().unwrap());
        let replay_scheduler = Arc::new(JobScheduler::new(2).unwrap());
        let replay_engine = Engine::<OfflineAuth, ReplayProvider, _>::with_transport(
            replay_transport,
            replay_destination,
            replay_scheduler,
            replay_provider,
            echo_config(),
        );
        let (_replay_cancel, replay_cancel_rx) = tokio::sync::watch::channel(false);
        let replay_completion = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            replay_engine.run(None, echo_input(), replay_cancel_rx, empty_incoming()),
        )
        .await
        .expect("ReplayProvider completes without API access")
        .expect("replay engine succeeds");
        assert_eq!(replay_completion.turn.response_id, "echo-final");
        assert!(replay_completion.transcript.iter().any(|item| {
            item.0["type"] == "function_call_output"
                && item.0["call_id"] == call_id.0
                && serde_json::from_str::<serde_json::Value>(
                    item.0["output"].as_str().unwrap_or_default(),
                )
                .is_ok_and(|value| value == json!({"echo":42}))
        }));
        let after_replay = reopened_store.replay_turns(&source_root).unwrap();
        assert_eq!(after_replay.len(), recorded_turns.len());
        assert_eq!(
            after_replay[0].model_response.response_id,
            recorded_turns[0].model_response.response_id
        );

        drop(reopened_store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[tokio::test]
    async fn replay_provider_rejects_missing_and_malformed_tool_outputs() {
        let store = Arc::new(Store::memory().unwrap());
        let root = RequestId("replay-output-root".into());
        store.create_request(&root, None, "/root").unwrap();
        let provider = ReplayProvider::new(store.clone(), &root).unwrap();
        let missing = provider
            .call_with_context("echo", json!({}), call_context(CallId("missing".into())))
            .await
            .unwrap_err();
        assert!(missing.to_string().contains("no settled replay output"));

        let malformed = CallId("malformed".into());
        store.claim(&malformed, &root).unwrap();
        store
            .write_output(
                &malformed,
                &Item(json!({
                    "type":"function_call_output",
                    "call_id":malformed.0,
                    "output":"not valid json"
                })),
            )
            .unwrap();
        let invalid = provider
            .call_with_context("echo", json!({}), call_context(malformed))
            .await
            .unwrap_err();
        assert!(invalid.to_string().contains("decoding replay output"));
    }

    #[tokio::test]
    async fn fake_cell_waits_releases_full_output_and_persists_state() {
        let path = std::env::temp_dir().join(format!(
            "harness-replay-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Arc::new(Store::open(&path).unwrap());
        let key = ReplaySessionKey::new("demo:evaluator:session-7");
        let cell = Arc::new(FakeResidentCell::new(store, key.clone()));
        let input = CellInput {
            source: "42".into(),
        };
        let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let context = CallContext {
            handle: JobHandle("replay-cell".into()),
            call_id: CallId("replay-cell".into()),
            agent: AgentPath("/root".into()),
            request: Some(RequestId("request".into())),
            progress,
        };
        let running = {
            let cell = cell.clone();
            tokio::spawn(async move { cell.run(input, context).await })
        };

        cell.wait_started(1).await;
        assert!(!running.is_finished());
        let expected = CellOutput {
            value: json!({"answer": 42}),
            stdout: "all stdout is retained\n".into(),
            stderr: "diagnostic".into(),
        };
        cell.release(expected.clone());
        assert_eq!(running.await.unwrap().unwrap(), expected);
        assert_eq!(cell.start_count(), 1);
        assert_eq!(cell.cancel_count(), 0);

        // Open an independent SQLite handle to prove this is durable state,
        // rather than only the evaluator's in-memory copy.
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            ReplayCellState::load(&reopened, &key).unwrap(),
            ReplayCellState {
                evaluations: 1,
                last_source: Some("42".into()),
            }
        );
        assert!(!key.key().starts_with("harness:"));
        drop(reopened);
        drop(cell);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn dropping_pending_cell_counts_a_cancellation() {
        let store = Arc::new(Store::memory().unwrap());
        let cell = Arc::new(FakeResidentCell::new(
            store,
            ReplaySessionKey::new("demo:evaluator:cancel"),
        ));
        let input = CellInput {
            source: "pending".into(),
        };
        let (progress, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let context = CallContext {
            handle: JobHandle("cancel-cell".into()),
            call_id: CallId("cancel-cell".into()),
            agent: AgentPath("/root".into()),
            request: None,
            progress,
        };
        let running = {
            let cell = cell.clone();
            tokio::spawn(async move { cell.run(input, context).await })
        };
        cell.wait_started(1).await;
        running.abort();
        let _ = running.await;
        assert_eq!(cell.start_count(), 1);
        assert_eq!(cell.cancel_count(), 1);
    }
}
