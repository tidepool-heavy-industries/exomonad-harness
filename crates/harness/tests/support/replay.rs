//! Controllable transport and resident evaluator for offline tests.
use async_trait::async_trait;
use harness::{
    cell_job::{CellInput, CellJob, CellOutput},
    engine::ResponsesTransport,
    provider::{CallContext, ProviderError},
    store::Store,
    transport::{ResponsesRequest, ResponsesTurn, TransportError, sse::StreamEvent},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

type ReplayObserver = dyn Fn(&ResponsesRequest, &mut ResponsesTurn, usize) + Send + Sync;

/// A FIFO transport for offline tests. Requests are recorded before the next
/// turn is removed from the queue, including requests whose queue is exhausted.
pub struct ReplayTransport {
    turns: Mutex<VecDeque<ResponsesTurn>>,
    requests: Mutex<Vec<ResponsesRequest>>,
    request_count: Mutex<usize>,
    observer: Option<Arc<ReplayObserver>>,
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
    pub fn with_observer(mut self, observer: Arc<ReplayObserver>) -> Self {
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
    pub fn load(store: &Store, key: &ReplaySessionKey) -> harness::store::Result<Self> {
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

    pub fn query_state(&self, store: &Store) -> harness::store::Result<ReplayCellState> {
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
        let state = ReplayCellState::load(&self.store, &self.key).map_err(|error| {
            ProviderError::Tool(format!("loading replay cell state: {error}").into())
        })?;
        let state = ReplayCellState {
            evaluations: state.evaluations + 1,
            last_source: Some(input.source),
        };
        let json = serde_json::to_value(state).map_err(|error| {
            ProviderError::Tool(format!("encoding replay cell state: {error}").into())
        })?;
        self.store
            .save_session_state(self.key.key(), &json)
            .map_err(|error| {
                ProviderError::Tool(format!("saving replay cell state: {error}").into())
            })?;
        pending.complete();
        Ok(output)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
