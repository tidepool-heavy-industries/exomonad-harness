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
    provider::{CallContext, ProviderError},
    store::Store,
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

/// A FIFO transport for offline tests. Requests are recorded before the next
/// turn is removed from the queue, including requests whose queue is exhausted.
pub struct ReplayTransport {
    turns: Mutex<VecDeque<ResponsesTurn>>,
    requests: Mutex<Vec<ResponsesRequest>>,
}

impl ReplayTransport {
    pub fn new(turns: impl IntoIterator<Item = ResponsesTurn>) -> Self {
        Self {
            turns: Mutex::new(turns.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// All requests in arrival order, including their full input envelopes.
    pub fn recorded_requests(&self) -> Vec<ResponsesRequest> {
        lock(&self.requests).clone()
    }

    pub fn queue_remaining(&self) -> usize {
        lock(&self.turns).len()
    }
}

#[async_trait]
impl ResponsesTransport for ReplayTransport {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        lock(&self.requests).push(request);
        lock(&self.turns)
            .pop_front()
            .ok_or_else(|| TransportError::Stream("replay turn queue exhausted".into()))
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
        item::Item,
        model::{AgentPath, CallId, Effort, RequestId},
        provider::JobHandle,
        store::Store,
        transport::Usage,
    };
    use serde_json::json;

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
