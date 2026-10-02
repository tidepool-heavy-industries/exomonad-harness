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
    item::{ToolInput, ToolKind},
    model::{OperationId, RequestId},
    provider::{
        CallContext, NonValueTerminal, Provider, ProviderError, RetainedOutput, WaitReplayBarrier,
    },
    store::{RecordedReplayTurn, Store, StoreError, TerminalOutcome},
    transport::{ResponsesRequest, ResponsesTurn, TransportError, sse::StreamEvent},
    turn::JobOutput,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Notify;

type ReplayObserver = dyn Fn(&ResponsesRequest, &mut ResponsesTurn, usize) + Send + Sync;

/// Replays recorded model requests and tool outputs from a durable Store.
///
/// Turns are loaded from the selected request branch when constructed. Each
/// transport request must exactly match the next recorded request before its
/// response is consumed; a mismatch leaves the turn available for a corrected
/// request. Provider calls are resolved by the recorded operation and the
/// matched local request.
pub struct ReplayProvider {
    store: Arc<Store>,
    turns: Arc<[RecordedReplayTurn]>,
    progress: Arc<ReplayProgress>,
    tool_schemas: crate::transport::ToolManifest,
    calls: HashMap<OperationId, RecordedCall>,
}

struct RecordedCall {
    name: String,
    kind: ToolKind,
    input: serde_json::Value,
    issued: usize,
}

struct ReplayProgress {
    cursor: Mutex<usize>,
    local_requests: Mutex<HashMap<RequestId, LocalReplayRequest>>,
    changed: Notify,
}

struct LocalReplayRequest {
    original: RequestId,
    committed: HashSet<crate::model::CallId>,
}

/// Authority to publish a recorded wait boundary after its exact history Item.
/// Only this replay owner constructs the token.
pub(crate) struct ReplayWaitCommit {
    operation: OperationId,
    item: crate::item::Item,
    progress: Arc<ReplayProgress>,
}

impl std::fmt::Debug for ReplayWaitCommit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayWaitCommit")
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

impl ReplayWaitCommit {
    pub(crate) fn commit(
        self,
        operation: &OperationId,
        item: &crate::item::Item,
    ) -> Result<(), ProviderError> {
        if operation != &self.operation || item != &self.item {
            return Err(ProviderError::Tool(
                "recorded wait history commit belongs to different evidence".into(),
            ));
        }
        let mut mappings = lock(&self.progress.local_requests);
        let mapped = mappings.get_mut(&operation.request).ok_or_else(|| {
            ProviderError::Tool("recorded wait history has no admitted local request".into())
        })?;
        mapped.committed.insert(operation.call.clone());
        self.progress.changed.notify_waiters();
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("ambiguous recorded call id `{0}`")]
    DuplicateCall(String),
    #[error("recorded call `{0}` is not the persisted invocation")]
    MissingInvocation(String),
    #[error("malformed recorded tool call: {0}")]
    MalformedCall(&'static str),
}

impl ReplayProvider {
    /// Load all recorded turns on `root`'s branch from the Store.
    pub fn new(store: Arc<Store>, root: &RequestId) -> Result<Self, ReplayError> {
        let turns = store.replay_turns(root)?;
        let tool_schemas = turns
            .first()
            .map(|turn| turn.model_request.tools.clone())
            .unwrap_or_default();
        let mut calls = HashMap::new();
        for (issued, turn) in turns.iter().enumerate() {
            let persisted = store.items(&turn.request)?;
            for item in &turn.model_response.items {
                if let Some(call) = item.tool_call().map_err(ReplayError::MalformedCall)? {
                    if !persisted.iter().any(|saved| saved == item) {
                        return Err(ReplayError::MissingInvocation(call.call_id.0));
                    }
                    let (kind, input) = match call.input {
                        ToolInput::Function(arguments) => (ToolKind::Function, arguments),
                        ToolInput::Custom(text) => {
                            (ToolKind::Custom, serde_json::Value::String(text))
                        }
                    };
                    let Some(id) =
                        store.recorded_operation_for_request(&turn.request, &call.call_id)?
                    else {
                        if call.name == crate::finalize::FINALIZE_TOOL_NAME {
                            continue;
                        }
                        return Err(ReplayError::MissingInvocation(call.call_id.0));
                    };
                    if calls
                        .insert(
                            id.clone(),
                            RecordedCall {
                                name: call.name,
                                kind,
                                input,
                                issued,
                            },
                        )
                        .is_some()
                    {
                        return Err(ReplayError::DuplicateCall(id.call.0));
                    }
                }
            }
        }
        Ok(Self {
            store,
            turns: turns.into(),
            progress: Arc::new(ReplayProgress {
                cursor: Mutex::new(0),
                local_requests: Mutex::new(HashMap::new()),
                changed: Notify::new(),
            }),
            tool_schemas,
            calls,
        })
    }

    fn recorded_job_output(
        &self,
        name: &str,
        input: &ToolInput,
        local: &OperationId,
    ) -> Result<(OperationId, JobOutput), ProviderError> {
        let recorded_request = lock(&self.progress.local_requests)
            .get(&local.request)
            .map(|request| request.original.clone())
            .ok_or_else(|| {
                ProviderError::Tool("replay call has no matched recorded model request".into())
            })?;
        let recorded_operation = self
            .store
            .recorded_operation_for_request(&recorded_request, &local.call)
            .map_err(|error| {
                ProviderError::Tool(format!("loading replay invocation: {error}").into())
            })?
            .ok_or_else(|| {
                ProviderError::Tool(format!("no recorded tool call for `{}`", local.call.0).into())
            })?;
        let recorded_call = self.calls.get(&recorded_operation).ok_or_else(|| {
            ProviderError::Tool(format!("no recorded tool call for `{}`", local.call.0).into())
        })?;
        let (input_kind, args) = match input {
            ToolInput::Function(args) => (ToolKind::Function, args.clone()),
            ToolInput::Custom(text) => (ToolKind::Custom, serde_json::Value::String(text.clone())),
        };
        let kind = &recorded_call.kind;
        if recorded_call.name != name || *kind != input_kind || recorded_call.input != args {
            return Err(ProviderError::Tool(
                format!(
                    "replay call `{}` does not match its recorded identity or input",
                    local.call.0
                )
                .into(),
            ));
        }
        let output = self
            .store
            .replay_tool_output_operation(&recorded_operation)
            .map_err(|error| ProviderError::Tool(format!("loading replay output: {error}").into()))?
            .ok_or_else(|| {
                ProviderError::Tool(
                    format!("no settled replay output for call `{}`", local.call.0).into(),
                )
            })?;
        let recorded_item = output.item;
        let terminal = output.terminal;
        let output = &recorded_item;
        let expected_type = match kind {
            ToolKind::Function => "function_call_output",
            ToolKind::Custom => "custom_tool_call_output",
        };
        if output.0["type"] != expected_type || output.0["call_id"] != local.call.0 {
            return Err(ProviderError::Tool(
                format!(
                    "stored replay output kind or identity does not match call `{}`",
                    local.call.0
                )
                .into(),
            ));
        }
        let value = output.0.get("output").cloned().ok_or_else(|| {
            ProviderError::Tool(
                format!(
                    "stored replay output for call `{}` has no output value",
                    local.call.0
                )
                .into(),
            )
        })?;
        let restored = match terminal {
            TerminalOutcome::Failure(failure) => JobOutput::Completed(Err(failure)),
            TerminalOutcome::Cancelled => JobOutput::Cancelled,
            TerminalOutcome::CancelledWithReceipt(receipt) => {
                JobOutput::CancelledWithReceipt(receipt)
            }
            TerminalOutcome::Interrupted => JobOutput::Interrupted,
            TerminalOutcome::CancellationUnconfirmed(detail) => {
                JobOutput::CancellationUnconfirmed(detail)
            }
            TerminalOutcome::Success => {
                let decoded = match (kind, value) {
                    (ToolKind::Custom, serde_json::Value::String(text)) => {
                        Ok(serde_json::Value::String(text))
                    }
                    (ToolKind::Function, serde_json::Value::String(serialized)) => {
                        serde_json::from_str(&serialized).map_err(|error| {
                            ProviderError::Tool(
                                format!(
                                    "decoding replay output for call `{}`: {error}",
                                    local.call.0
                                )
                                .into(),
                            )
                        })
                    }
                    (_, value) => Ok(value),
                }?;
                JobOutput::Completed(Ok(decoded))
            }
        };
        if crate::item::Item::tool_output(&local.call, *kind, &restored) != recorded_item {
            return Err(ProviderError::Tool(
                "recorded terminal does not match its immutable output item".into(),
            ));
        }
        Ok((recorded_operation, restored))
    }

    // Find the first immutable issued input containing this output after its
    // invocation. Count prior identical items so reused wire call ids cannot
    // borrow visibility from an older operation.
    fn output_cut_unchecked(
        &self,
        original: &OperationId,
        output: &JobOutput,
    ) -> Option<(usize, usize)> {
        let call = self.calls.get(original)?;
        let item = crate::item::Item::tool_output(&original.call, call.kind, output);
        let prior = self.turns[call.issued]
            .model_request
            .input
            .iter()
            .filter(|saved| **saved == item)
            .count();
        for (index, turn) in self.turns.iter().enumerate().skip(call.issued + 1) {
            if let Some((position, _)) = turn
                .model_request
                .input
                .iter()
                .enumerate()
                .filter(|(_, saved)| **saved == item)
                .nth(prior)
            {
                return Some((index, position));
            }
        }
        None
    }

    fn output_cut(
        &self,
        original: &OperationId,
        output: &JobOutput,
    ) -> Result<Option<(usize, usize)>, ProviderError> {
        let cut = self.output_cut_unchecked(original, output);
        if let Some(cut) = cut {
            let item =
                crate::item::Item::tool_output(&original.call, self.calls[original].kind, output);
            for (other, call) in &self.calls {
                if other == original || other.call != original.call || call.issued >= cut.0 {
                    continue;
                }
                let saved = self
                    .store
                    .replay_tool_output_operation(other)
                    .map_err(|error| {
                        ProviderError::Tool(format!("validating replay occurrence: {error}").into())
                    })?;
                if let Some(saved) = saved {
                    if saved.item == item && self.output_cut_unchecked(other, output) == Some(cut) {
                        return Err(ProviderError::Tool(
                            "recorded output occurrence belongs to multiple exact operations"
                                .into(),
                        ));
                    }
                }
            }
        }
        Ok(cut)
    }

    fn local_operation(&self, original: &OperationId, local: &OperationId) -> Option<OperationId> {
        lock(&self.progress.local_requests)
            .iter()
            .find_map(|(request, mapped)| {
                (mapped.original == original.request).then(|| OperationId {
                    origin: local.origin.clone(),
                    request: request.clone(),
                    call: original.call.clone(),
                })
            })
    }

    fn outputs_before(
        &self,
        cut: usize,
        position: usize,
        local: &OperationId,
    ) -> Result<Vec<OperationId>, ProviderError> {
        let mut waits = vec![];
        for (original, call) in &self.calls {
            if call.issued >= cut || call.name != "wait_agent" {
                continue;
            }
            let Some(operation) = self.local_operation(original, local) else {
                continue;
            };
            let (_, output) = self.recorded_job_output(
                &call.name,
                &ToolInput::Function(call.input.clone()),
                &operation,
            )?;
            if let Some((output_cut, output_position)) = self.output_cut(original, &output)? {
                if output_cut == cut && output_position < position {
                    waits.push(operation);
                }
            }
        }
        Ok(waits)
    }

    async fn ready_output(
        &self,
        name: &str,
        input: &ToolInput,
        local: &OperationId,
    ) -> Result<RetainedOutput, ProviderError> {
        let (original, output) = self.recorded_job_output(name, input, local)?;
        let cut = self.output_cut(&original, &output)?;
        if name == "wait_agent"
            && cut.map(|(index, _)| index) != Some(self.calls[&original].issued + 1)
        {
            return Err(ProviderError::Tool(
                "recorded wait has no immediate immutable successor input boundary".into(),
            ));
        }
        loop {
            let changed = self.progress.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let ready_turn = cut.map(|(index, _)| index).unwrap_or(self.turns.len());
            let ready = *lock(&self.progress.cursor) >= ready_turn;
            let preceding_waits = if name == "wait_agent" {
                vec![]
            } else {
                match cut {
                    Some((index, position)) => self.outputs_before(index, position, local)?,
                    None => vec![],
                }
            };
            let waits_committed = {
                let mappings = lock(&self.progress.local_requests);
                preceding_waits.iter().all(|wait| {
                    mappings
                        .get(&wait.request)
                        .is_some_and(|mapped| mapped.committed.contains(&wait.call))
                })
            };
            if ready && waits_committed {
                break;
            }
            changed.await;
        }
        if name != "wait_agent" {
            return Ok(RetainedOutput::terminal(output));
        }
        let next = cut.and_then(|(index, _)| self.turns.get(index));
        let continuation = self
            .store
            .replay_wait_continuation(local, &original, next)
            .map_err(|error| {
                ProviderError::Tool(format!("loading replay wait continuation: {error}").into())
            })?;
        let mut barrier = WaitReplayBarrier::default();
        let mut before = vec![];
        let mut after = vec![];
        if let Some((index, position)) = cut {
            let mut next_wait = self.turns[index].model_request.input.len();
            for (original, call) in &self.calls {
                if call.name != "wait_agent" || call.issued >= index {
                    continue;
                }
                let Some(operation) = self.local_operation(original, local) else {
                    continue;
                };
                let (_, output) = self.recorded_job_output(
                    &call.name,
                    &ToolInput::Function(call.input.clone()),
                    &operation,
                )?;
                if let Some((other_cut, other_position)) = self.output_cut(original, &output)? {
                    if other_cut == index && other_position > position {
                        next_wait = next_wait.min(other_position);
                    }
                }
            }
            for (original, call) in &self.calls {
                if call.name == "wait_agent" {
                    continue;
                }
                let Some(operation) = self.local_operation(original, local) else {
                    continue;
                };
                let input = match call.kind {
                    ToolKind::Function => ToolInput::Function(call.input.clone()),
                    ToolKind::Custom => ToolInput::Custom(
                        call.input
                            .as_str()
                            .ok_or_else(|| {
                                ProviderError::Tool("malformed recorded custom input".into())
                            })?
                            .to_owned(),
                    ),
                };
                let (_, output) = self.recorded_job_output(&call.name, &input, &operation)?;
                if let Some((output_cut, output_position)) = self.output_cut(original, &output)? {
                    if output_cut == index {
                        if output_position < position {
                            before.push((output_position, operation));
                        } else if output_position < next_wait {
                            after.push((output_position, operation));
                        }
                    }
                }
            }
        }
        before.sort_by_key(|(position, _)| *position);
        after.sort_by_key(|(position, _)| *position);
        barrier.before = before.into_iter().map(|(_, operation)| operation).collect();
        barrier.after = after.into_iter().map(|(_, operation)| operation).collect();
        let commit = ReplayWaitCommit {
            operation: local.clone(),
            item: crate::item::Item::tool_output(&local.call, self.calls[&original].kind, &output),
            progress: self.progress.clone(),
        };
        Ok(RetainedOutput::recorded_wait(
            local.clone(),
            output,
            continuation,
            barrier,
            commit,
        ))
    }

    fn next_turn(
        &self,
        local: Option<&RequestId>,
        request: ResponsesRequest,
    ) -> Result<ResponsesTurn, TransportError> {
        let mut cursor = lock(&self.progress.cursor);
        let mut mappings = lock(&self.progress.local_requests);
        if let Some(local) = local {
            if mappings.contains_key(local) {
                return Err(TransportError::ReplayRequestReuse {
                    request: local.clone(),
                });
            }
        }
        let Some(next) = self.turns.get(*cursor) else {
            return Err(TransportError::Stream(
                "replay provider has no recorded model turns remaining".into(),
            ));
        };
        if let Some(field) = request_mismatch(&next.model_request, &request) {
            return Err(TransportError::Stream(format!(
                "replay request does not match the next recorded request ({field})"
            )));
        }
        let recorded = next;
        *cursor += 1;
        if let Some(local) = local {
            mappings.insert(
                local.clone(),
                LocalReplayRequest {
                    original: recorded.request.clone(),
                    committed: HashSet::new(),
                },
            );
        }
        self.progress.changed.notify_waiters();
        Ok(recorded.model_response.clone())
    }

    /// Number of recorded model turns not yet consumed.
    pub fn turns_remaining(&self) -> usize {
        self.turns.len() - *lock(&self.progress.cursor)
    }
}

#[async_trait]
impl ResponsesTransport for ReplayProvider {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.next_turn(None, request)
    }
    async fn create_streaming_for_request(
        &self,
        local: &RequestId,
        request: ResponsesRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        let turn = self.next_turn(Some(local), request)?;
        for item in &turn.items {
            let _ = sink.send(StreamEvent::ItemDone(item.clone())).await;
        }
        Ok(turn)
    }
}

#[async_trait]
impl ResponsesTransport for Arc<ReplayProvider> {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.as_ref().create(request).await
    }
    async fn create_streaming_for_request(
        &self,
        local: &RequestId,
        request: ResponsesRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        self.as_ref()
            .create_streaming_for_request(local, request, sink)
            .await
    }
}

#[async_trait]
impl Provider for ReplayProvider {
    async fn call(
        &self,
        name: &str,
        _args: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        Err(ProviderError::Tool(
            format!("replay provider requires call context for tool `{name}`").into(),
        ))
    }

    async fn retained_output(
        &self,
        name: &str,
        input: &ToolInput,
        operation: &OperationId,
    ) -> Result<Option<RetainedOutput>, ProviderError> {
        self.ready_output(name, input, operation).await.map(Some)
    }

    async fn call_with_context(
        &self,
        name: &str,
        args: serde_json::Value,
        context: CallContext,
    ) -> Result<serde_json::Value, ProviderError> {
        match self
            .recorded_job_output(
                name,
                &ToolInput::Function(args),
                aligned_operation(&context)?,
            )?
            .1
        {
            JobOutput::Completed(result) => result.map_err(ProviderError::Tool),
            JobOutput::Cancelled => {
                Err(ProviderError::NonValueTerminal(NonValueTerminal::Cancelled))
            }
            JobOutput::CancelledWithReceipt(receipt) => Err(ProviderError::NonValueTerminal(
                NonValueTerminal::CancelledWithReceipt(receipt),
            )),
            JobOutput::Interrupted => Err(ProviderError::NonValueTerminal(
                NonValueTerminal::Interrupted,
            )),
            JobOutput::CancellationUnconfirmed(detail) => Err(ProviderError::NonValueTerminal(
                NonValueTerminal::CancellationUnconfirmed(detail),
            )),
        }
    }

    async fn call_custom_with_context(
        &self,
        name: &str,
        input: String,
        context: CallContext,
    ) -> Result<serde_json::Value, ProviderError> {
        match self
            .recorded_job_output(
                name,
                &ToolInput::Custom(input),
                aligned_operation(&context)?,
            )?
            .1
        {
            JobOutput::Completed(result) => result.map_err(ProviderError::Tool),
            JobOutput::Cancelled => {
                Err(ProviderError::NonValueTerminal(NonValueTerminal::Cancelled))
            }
            JobOutput::CancelledWithReceipt(receipt) => Err(ProviderError::NonValueTerminal(
                NonValueTerminal::CancelledWithReceipt(receipt),
            )),
            JobOutput::Interrupted => Err(ProviderError::NonValueTerminal(
                NonValueTerminal::Interrupted,
            )),
            JobOutput::CancellationUnconfirmed(detail) => Err(ProviderError::NonValueTerminal(
                NonValueTerminal::CancellationUnconfirmed(detail),
            )),
        }
    }

    fn tools(&self) -> Vec<serde_json::Value> {
        self.tool_schemas.to_vec()
    }

    fn tool_manifest(&self) -> crate::transport::ToolManifest {
        self.tool_schemas.clone()
    }
}

fn aligned_operation(context: &CallContext) -> Result<&OperationId, ProviderError> {
    let operation = context
        .operation
        .as_ref()
        .ok_or_else(|| ProviderError::Tool("replay call has no operation identity".into()))?;
    if operation.call != context.call_id
        || context.request.as_ref() != Some(&operation.request)
        || operation.origin.actor() != &context.agent
    {
        return Err(ProviderError::Tool(
            "replay call context does not match its operation".into(),
        ));
    }
    Ok(operation)
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
    } else if expected.tools_allowed != actual.tools_allowed {
        Some("tools_allowed")
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
                return Err(ProviderError::Tool(
                    format!("unexpected tool `{name}`").into(),
                ));
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
        let agent = AgentPath("/root".into());
        let request = Some(RequestId("replay-direct".into()));
        let cancel = tokio_util::sync::CancellationToken::new();
        let (progress, _receiver) =
            tokio::sync::mpsc::channel(crate::provider::JOB_PROGRESS_CAPACITY);
        let verbs = crate::agents::JobVerbs::from_scheduler(
            None,
            agent.clone(),
            request
                .clone()
                .map(|request| crate::agents::AgentInvocation {
                    request,
                    call_id: call_id.clone(),
                }),
            cancel.clone(),
            progress.clone(),
        );
        CallContext {
            handle: JobHandle(format!("replay-{}", call_id.0)),
            operation: Some(crate::model::OperationId {
                origin: crate::model::ConversationIdentity::Standalone {
                    store: "direct-replay-test".into(),
                    actor: agent.clone(),
                },
                request: RequestId("replay-direct".into()),
                call: call_id.clone(),
            }),
            call_id,
            agent,
            request,
            cancel,
            verbs,
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
            tools: vec![].into(),
            tools_allowed: None,
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
        let mut tools = mismatched_tools.tools.to_vec();
        tools.push(json!({"name":"unexpected"}));
        mismatched_tools.tools = tools.into();
        let mut mismatched_model = recorded_turns[0].model_request.clone();
        mismatched_model.model.push_str("-different");
        let mut mismatched_effort = recorded_turns[0].model_request.clone();
        mismatched_effort.pinned_effort = Effort::High;
        let mut mismatched_session = recorded_turns[0].model_request.clone();
        mismatched_session.session_id.push_str("-different");
        let mut mismatched_allowed = recorded_turns[0].model_request.clone();
        mismatched_allowed.tools_allowed = Some(vec![]);
        for mismatched in [
            mismatched_input,
            mismatched_instructions,
            mismatched_tools,
            mismatched_model,
            mismatched_effort,
            mismatched_session,
            mismatched_allowed,
        ] {
            assert!(match_probe.create(mismatched).await.is_err());
            assert_eq!(match_probe.turns_remaining(), 2);
        }

        let replay_provider =
            Arc::new(ReplayProvider::new(reopened_store.clone(), &source_root).unwrap());
        let replay_destination = Arc::new(Store::memory().unwrap());
        let replay_scheduler = Arc::new(JobScheduler::new(2).unwrap());
        let replay_engine = Engine::<OfflineAuth, ReplayProvider, _>::with_transport(
            replay_provider.clone(),
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
    async fn tool_failure_metadata_survives_scheduler_and_durable_replay() {
        use crate::provider::ToolFailure;
        struct FailingTool(Arc<std::sync::atomic::AtomicUsize>);
        #[async_trait]
        impl Provider for FailingTool {
            fn tools(&self) -> Vec<serde_json::Value> {
                EchoTool.tools()
            }

            async fn call(
                &self,
                _: &str,
                _: serde_json::Value,
            ) -> Result<serde_json::Value, ProviderError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::Tool(ToolFailure::with_metadata(
                    "retained owner missing",
                    json!({
                        "class": "version-skew", "phase": "compile",
                        "cause": { "kind": "artifact_inventory", "required": { "unit": "package", "module": "Original" } }
                    }),
                )))
            }
        }
        let path = std::env::temp_dir().join(format!(
            "harness-replay-failure-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Arc::new(Store::open(&path).unwrap());
        let root = RequestId("failure-root".into());
        let call_id = CallId("failure-call".into());
        let call = Item(
            json!({ "type": "function_call", "call_id": call_id.0, "name": "echo", "arguments": "{}" }),
        );
        store.create_request(&root, None, "/root").unwrap();
        store
            .append_items(&root, std::slice::from_ref(&call))
            .unwrap();
        let operation = store.claim(&call_id, &root).unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let scheduler = JobScheduler::new(1).unwrap();
        scheduler
            .start_operation(
                Arc::new(FailingTool(calls.clone())),
                operation.clone(),
                AgentPath("/root".into()),
                Some(root.clone()),
                "echo".into(),
                json!({}),
            )
            .await
            .unwrap();
        let output = scheduler.wait(&operation).await.unwrap();
        let JobOutput::Completed(Err(failure)) = &output else {
            panic!("failed provider became successful: {output:?}")
        };
        assert_eq!(failure.message(), "tool failed: retained owner missing");
        assert_eq!(
            failure.metadata().unwrap()["cause"]["required"]["module"],
            "Original"
        );
        store
            .write_job_output(&operation, ToolKind::Function, &output)
            .unwrap();
        store
            .record_replay_turn(
                &root,
                &request("failure-session"),
                &ResponsesTurn {
                    response_id: "failure-response".into(),
                    items: vec![call],
                    usage: Usage::default(),
                },
            )
            .unwrap();
        let original = store.replay_output_operation(&operation).unwrap().unwrap();
        let expected: serde_json::Value =
            serde_json::from_str(original.0["output"].as_str().unwrap()).unwrap();
        assert_eq!(expected, failure.output_value());
        drop(scheduler);
        drop(store);

        let reopened = Arc::new(Store::open(&path).unwrap());
        assert_eq!(
            reopened.replay_output_operation(&operation).unwrap(),
            Some(original)
        );
        let provider = Arc::new(ReplayProvider::new(reopened, &root).unwrap());
        let local = RequestId("failure-local".into());
        let (sink, _stream) = tokio::sync::mpsc::channel(4);
        provider
            .create_streaming_for_request(&local, request("failure-session"), sink)
            .await
            .unwrap();
        let mut replay_operation = operation.clone();
        replay_operation.request = local.clone();
        let scheduler = JobScheduler::new(1).unwrap();
        scheduler
            .start_operation(
                provider.clone(),
                replay_operation.clone(),
                AgentPath("/root".into()),
                Some(local),
                "echo".into(),
                json!({}),
            )
            .await
            .unwrap();
        let replayed = scheduler.wait(&replay_operation).await.unwrap();
        assert_eq!(replayed, output);
        let JobOutput::Completed(Err(replayed_failure)) = replayed else {
            panic!("failed replay became a success")
        };
        assert_eq!(
            replayed_failure.metadata().unwrap()["class"],
            "version-skew"
        );
        assert_eq!(replayed_failure.metadata().unwrap()["phase"], "compile");
        assert_eq!(
            replayed_failure.message(),
            "tool failed: retained owner missing"
        );
        drop(scheduler);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "replay executed the failing provider again"
        );
        drop(provider);
        std::fs::remove_file(&path).unwrap();
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    fn recorded_terminal(output: &JobOutput, tombstone: bool) -> (Arc<Store>, RequestId, CallId) {
        recorded_terminal_in(Arc::new(Store::memory().unwrap()), output, tombstone)
    }

    fn recorded_terminal_in(
        store: Arc<Store>,
        output: &JobOutput,
        tombstone: bool,
    ) -> (Arc<Store>, RequestId, CallId) {
        recorded_terminal_kind_in(store, output, tombstone, ToolKind::Function)
    }

    fn recorded_terminal_kind_in(
        store: Arc<Store>,
        output: &JobOutput,
        tombstone: bool,
        kind: ToolKind,
    ) -> (Arc<Store>, RequestId, CallId) {
        let root = RequestId("terminal-root".into());
        let call_id = CallId("terminal-call".into());
        let call = Item(match kind {
            ToolKind::Function => {
                json!({"type":"function_call", "call_id":call_id.0, "name":"echo", "arguments":"{}"})
            }
            ToolKind::Custom => {
                json!({"type":"custom_tool_call", "call_id":call_id.0, "name":"echo", "input":"raw prefix"})
            }
        });
        store.create_request(&root, None, "/root").unwrap();
        store
            .append_items(&root, std::slice::from_ref(&call))
            .unwrap();
        let operation = store.claim(&call_id, &root).unwrap();
        if tombstone {
            store.interrupt_operation_claim(&operation, &root).unwrap();
        } else {
            store.write_job_output(&operation, kind, output).unwrap();
        }
        store
            .record_replay_turn(
                &root,
                &request("terminal-session"),
                &ResponsesTurn {
                    response_id: "terminal-response".into(),
                    items: vec![call],
                    usage: Usage::default(),
                },
            )
            .unwrap();
        (store, root, call_id)
    }

    #[tokio::test]
    async fn cancelled_custom_receipt_survives_reopen_and_refuses_changed_payload() {
        let path = std::env::temp_dir().join(format!(
            "harness-cancelled-receipt-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let expected =
            JobOutput::CancelledWithReceipt(Err(crate::provider::ToolFailure::with_metadata(
                "input unit 2 cancelled; input unit 1 committed: prefix",
                json!({"class":"interrupted","phase":"run"}),
            )));
        let (store, root, call) = recorded_terminal_kind_in(
            Arc::new(Store::open(&path).unwrap()),
            &expected,
            false,
            ToolKind::Custom,
        );
        let original = store.claims_on(&root).unwrap()[0].operation.clone();
        drop(store);
        let store = Arc::new(Store::open(&path).unwrap());
        let provider = ReplayProvider::new(store.clone(), &root).unwrap();
        let local = RequestId("replay-direct".into());
        let (sink, _stream) = tokio::sync::mpsc::channel(4);
        provider
            .create_streaming_for_request(&local, request("terminal-session"), sink)
            .await
            .unwrap();
        let context = call_context(call);
        let operation = context.operation.as_ref().unwrap();
        let restored = provider
            .retained_output("echo", &ToolInput::Custom("raw prefix".into()), operation)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.output(), &expected);
        let recorded = store
            .replay_tool_output_operation(&original)
            .unwrap()
            .unwrap();
        assert_eq!(recorded.terminal, TerminalOutcome::from(&expected));
        assert_eq!(
            recorded.item,
            Item::tool_output(&operation.call, ToolKind::Custom, &expected)
        );
        assert!(!store.has_completed_output(&original).unwrap());
        let hash = store.claims_on(&root).unwrap()[0].output.clone().unwrap();
        let changed = Item::tool_output(&operation.call, ToolKind::Custom, &JobOutput::Cancelled);
        store
            .lock()
            .execute(
                "UPDATE items SET json=?1 WHERE hash=?2",
                rusqlite::params![serde_json::to_string(&changed).unwrap(), hash.0],
            )
            .unwrap();
        let refusal = provider
            .retained_output("echo", &ToolInput::Custom("raw prefix".into()), operation)
            .await
            .unwrap_err();
        assert!(
            matches!(refusal, ProviderError::Tool(failure) if failure.message() == "recorded terminal does not match its immutable output item")
        );
        drop(provider);
        drop(store);
        std::fs::remove_file(&path).unwrap();
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[tokio::test]
    async fn replay_restores_each_typed_terminal_without_interpreting_success_payload() {
        for expected in [
            JobOutput::Completed(Ok(
                json!({"error":"legitimate data", "failure":{"class":"user"}}),
            )),
            JobOutput::Completed(Err("old plain failure".into())),
            JobOutput::Cancelled,
            JobOutput::CancelledWithReceipt(Ok(json!({
                "items": [{"status":"committed","output":"prefix"}], "nextIndex": 1
            }))),
            JobOutput::CancelledWithReceipt(Err(crate::provider::ToolFailure::with_metadata(
                "cancelled after prefix",
                json!({"class":"interrupted","phase":"run"}),
            ))),
            JobOutput::Interrupted,
            JobOutput::CancellationUnconfirmed("owner unavailable".into()),
        ] {
            let (store, root, call) = recorded_terminal(&expected, false);
            let provider = Arc::new(ReplayProvider::new(store, &root).unwrap());
            let local = RequestId("replay-direct".into());
            let (sink, _stream) = tokio::sync::mpsc::channel(4);
            provider
                .create_streaming_for_request(&local, request("terminal-session"), sink)
                .await
                .unwrap();
            let context = call_context(call.clone());
            let operation = context.operation.clone().unwrap();
            let retained = provider
                .retained_output("echo", &ToolInput::Function(json!({})), &operation)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retained.output(), &expected);
            let direct = provider.call_with_context("echo", json!({}), context).await;
            match expected {
                JobOutput::Completed(Ok(value)) => assert_eq!(direct.unwrap(), value),
                JobOutput::Completed(Err(failure)) => {
                    assert!(matches!(direct, Err(ProviderError::Tool(saved)) if saved == failure))
                }
                JobOutput::Cancelled => assert!(matches!(
                    direct,
                    Err(ProviderError::NonValueTerminal(NonValueTerminal::Cancelled))
                )),
                JobOutput::CancelledWithReceipt(receipt) => assert!(matches!(
                    direct,
                    Err(ProviderError::NonValueTerminal(NonValueTerminal::CancelledWithReceipt(saved))) if saved == receipt
                )),
                JobOutput::Interrupted => assert!(matches!(
                    direct,
                    Err(ProviderError::NonValueTerminal(
                        NonValueTerminal::Interrupted
                    ))
                )),
                JobOutput::CancellationUnconfirmed(detail) => assert!(
                    matches!(direct, Err(ProviderError::NonValueTerminal(NonValueTerminal::CancellationUnconfirmed(saved))) if saved == detail)
                ),
            }
        }
    }

    #[tokio::test]
    async fn replay_restores_original_process_loss_claim_without_borrowing_child_success() {
        let path = std::env::temp_dir().join(format!(
            "harness-interrupted-replay-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let (store, root, call) = recorded_terminal_in(
            Arc::new(Store::open(&path).unwrap()),
            &JobOutput::Interrupted,
            true,
        );
        let child = RequestId("terminal-child".into());
        store.create_request(&child, Some(&root), "/root").unwrap();
        let parent_op = store.operation_for_request(&root, &call).unwrap();
        store.lock().execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state) SELECT origin,origin_request_id,call_id,?1,'pending' FROM claims WHERE request_id=?2", rusqlite::params![child.0, root.0]).unwrap();
        store
            .write_job_output(
                &parent_op,
                ToolKind::Function,
                &JobOutput::Completed(Ok(json!({"child":true}))),
            )
            .unwrap();
        assert_eq!(
            store
                .replay_tool_output_claim(&parent_op, &child)
                .unwrap()
                .unwrap()
                .terminal,
            TerminalOutcome::Success
        );
        drop(store);
        let store = Arc::new(Store::open(&path).unwrap());
        assert_eq!(
            store
                .replay_tool_output_claim(&parent_op, &root)
                .unwrap()
                .unwrap()
                .terminal,
            TerminalOutcome::Interrupted
        );
        assert_eq!(
            store
                .replay_tool_output_claim(&parent_op, &child)
                .unwrap()
                .unwrap()
                .terminal,
            TerminalOutcome::Success
        );
        let provider = ReplayProvider::new(store, &root).unwrap();
        let (sink, _stream) = tokio::sync::mpsc::channel(4);
        provider
            .create_streaming_for_request(
                &RequestId("replay-direct".into()),
                request("terminal-session"),
                sink,
            )
            .await
            .unwrap();
        let result = provider
            .retained_output(
                "echo",
                &ToolInput::Function(json!({})),
                call_context(call).operation.as_ref().unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.output(), &JobOutput::Interrupted);
        drop(provider);
        std::fs::remove_file(&path).unwrap();
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[tokio::test]
    async fn replay_refuses_context_aliases_and_local_request_remapping() {
        let (store, root, call) = recorded_terminal(&JobOutput::Completed(Ok(json!({}))), false);
        let provider = ReplayProvider::new(store, &root).unwrap();
        let local = RequestId("replay-direct".into());
        let (sink, _stream) = tokio::sync::mpsc::channel(4);
        provider
            .create_streaming_for_request(&local, request("terminal-session"), sink)
            .await
            .unwrap();
        let mut context = call_context(call);
        context.request = Some(RequestId("foreign-request".into()));
        assert!(
            provider
                .call_with_context("echo", json!({}), context)
                .await
                .is_err()
        );
        let (sink, _stream) = tokio::sync::mpsc::channel(4);
        assert!(matches!(
            provider
                .create_streaming_for_request(&local, request("terminal-session"), sink)
                .await,
            Err(TransportError::ReplayRequestReuse { .. })
        ));
        assert_eq!(provider.turns_remaining(), 0);
    }

    #[tokio::test]
    async fn replay_wait_barriers_preserve_history_order_for_every_terminal() {
        for wait_output in [
            JobOutput::Completed(Ok(json!({"wait":true}))),
            JobOutput::Cancelled,
            JobOutput::Interrupted,
            JobOutput::CancellationUnconfirmed("lost".into()),
        ] {
            let store = Arc::new(Store::memory().unwrap());
            let root = RequestId("wait-source".into());
            let next = RequestId("wait-next".into());
            store.create_request(&root, None, "/root").unwrap();
            let mut first = request("wait-session");
            first.input.clear();
            let calls: Vec<Item> = [
                ("before", "echo"),
                ("wait", "wait_agent"),
                ("after", "echo"),
            ]
            .into_iter()
            .map(|(id, name)| {
                Item(json!({"type":"function_call", "call_id":id, "name":name, "arguments":"{}"}))
            })
            .collect();
            store.append_items(&root, &calls).unwrap();
            let mut outputs = vec![];
            for (id, output) in [
                ("before", JobOutput::Completed(Ok(json!(1)))),
                ("wait", wait_output.clone()),
                ("after", JobOutput::Completed(Ok(json!(2)))),
            ] {
                let operation = store.claim(&CallId(id.into()), &root).unwrap();
                store
                    .write_job_output(&operation, ToolKind::Function, &output)
                    .unwrap();
                outputs.push(Item::tool_output(
                    &operation.call,
                    ToolKind::Function,
                    &output,
                ));
            }
            let message = Item(
                json!({"type":"message", "role":"user", "content":[{"type":"input_text", "text":"delivered while waiting"}]}),
            );
            store
                .append_items(
                    &root,
                    &[
                        outputs[0].clone(),
                        outputs[1].clone(),
                        message.clone(),
                        outputs[2].clone(),
                    ],
                )
                .unwrap();
            store
                .record_replay_turn(
                    &root,
                    &first,
                    &ResponsesTurn {
                        response_id: "wait-issued".into(),
                        items: calls,
                        usage: Usage::default(),
                    },
                )
                .unwrap();
            store.create_request(&next, Some(&root), "/root").unwrap();
            let mut second = first.clone();
            second.input = store.items(&root).unwrap();
            store
                .record_replay_turn(&next, &second, &final_turn())
                .unwrap();
            let provider = ReplayProvider::new(store, &root).unwrap();
            let local = RequestId("replay-direct".into());
            let (sink, _stream) = tokio::sync::mpsc::channel(4);
            provider
                .create_streaming_for_request(&local, first, sink)
                .await
                .unwrap();
            let before = call_context(CallId("before".into())).operation.unwrap();
            let wait = call_context(CallId("wait".into())).operation.unwrap();
            let after = call_context(CallId("after".into())).operation.unwrap();
            let args = ToolInput::Function(json!({}));
            let mut after_lookup = Box::pin(provider.retained_output("echo", &args, &after));
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(10), &mut after_lookup)
                    .await
                    .is_err()
            );
            provider.output_committed(&wait).await.unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(10), &mut after_lookup)
                    .await
                    .is_err(),
                "durable acknowledgment published wait history"
            );
            let retained = provider
                .retained_output("wait_agent", &args, &wait)
                .await
                .unwrap()
                .unwrap();
            let (saved, continuation, barrier, commit) = retained.into_parts(&wait).unwrap();
            assert_eq!(saved, wait_output);
            assert_eq!(barrier.before, vec![before.clone()]);
            assert_eq!(barrier.after, vec![after.clone()]);
            let messages = continuation
                .unwrap()
                .into_items(&wait, &outputs[1])
                .unwrap();
            assert_eq!(messages, vec![message]);
            assert_eq!(
                provider
                    .retained_output("echo", &args, &before)
                    .await
                    .unwrap()
                    .unwrap()
                    .output(),
                &JobOutput::Completed(Ok(json!(1)))
            );
            commit.unwrap().commit(&wait, &outputs[1]).unwrap();
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(1), after_lookup)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .output(),
                &JobOutput::Completed(Ok(json!(2)))
            );
        }
    }

    #[tokio::test]
    async fn replay_refuses_wait_without_an_immediate_immutable_input_cut() {
        let (store, root, call) = recorded_terminal(&JobOutput::Completed(Ok(json!({}))), false);
        let mut provider = ReplayProvider::new(store, &root).unwrap();
        let original = provider.calls.keys().next().unwrap().clone();
        provider.calls.get_mut(&original).unwrap().name = "wait_agent".into();
        let (sink, _stream) = tokio::sync::mpsc::channel(4);
        provider
            .create_streaming_for_request(
                &RequestId("replay-direct".into()),
                request("terminal-session"),
                sink,
            )
            .await
            .unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            provider.retained_output(
                "wait_agent",
                &ToolInput::Function(json!({})),
                call_context(call).operation.as_ref().unwrap(),
            ),
        )
        .await
        .unwrap();
        assert!(result.is_err());
    }
    #[tokio::test]
    async fn cancelled_read_only_lookup_never_dispatches_or_claims_live_cleanup() {
        use crate::provider::{CancellationAcknowledgment, CancellationOwner};
        struct Owner(Arc<std::sync::atomic::AtomicUsize>);
        #[async_trait]
        impl CancellationOwner for Owner {
            async fn cancel(&self, _: &OperationId, _: &JobHandle) -> CancellationAcknowledgment {
                self.0.fetch_add(1, Ordering::SeqCst);
                CancellationAcknowledgment::Stopped
            }
        }
        struct Lookup {
            entered: Notify,
            release: Notify,
            live: Arc<std::sync::atomic::AtomicUsize>,
            owner: Arc<Owner>,
            retained: bool,
        }
        #[async_trait]
        impl Provider for Lookup {
            async fn retained_output(
                &self,
                _: &str,
                _: &ToolInput,
                _: &OperationId,
            ) -> Result<Option<RetainedOutput>, ProviderError> {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(self
                    .retained
                    .then(|| RetainedOutput::terminal(JobOutput::Interrupted)))
            }
            async fn call(
                &self,
                _: &str,
                _: serde_json::Value,
            ) -> Result<serde_json::Value, ProviderError> {
                self.live.fetch_add(1, Ordering::SeqCst);
                Ok(json!({}))
            }
            fn cancellation_owner(&self) -> Option<Arc<dyn CancellationOwner>> {
                Some(self.owner.clone())
            }
            fn tools(&self) -> Vec<serde_json::Value> {
                vec![]
            }
        }
        for retained in [false, true] {
            let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let cleanups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let provider = Arc::new(Lookup {
                entered: Notify::new(),
                release: Notify::new(),
                live: live.clone(),
                owner: Arc::new(Owner(cleanups.clone())),
                retained,
            });
            let scheduler = Arc::new(JobScheduler::new(1).unwrap());
            let operation = call_context(CallId("lookup-race".into()))
                .operation
                .unwrap();
            scheduler
                .start_operation(
                    provider.clone(),
                    operation.clone(),
                    AgentPath("/root".into()),
                    Some(operation.request.clone()),
                    "echo".into(),
                    json!({}),
                )
                .await
                .unwrap();
            provider.entered.notified().await;
            let cancelling = {
                let scheduler = scheduler.clone();
                let operation = operation.clone();
                tokio::spawn(async move { scheduler.cancel(&operation).await.unwrap() })
            };
            assert_eq!(
                scheduler.wait(&operation).await.unwrap(),
                JobOutput::Cancelled
            );
            provider.release.notify_one();
            assert!(cancelling.await.unwrap().is_some());
            assert_eq!(
                scheduler.output(&operation).await.unwrap(),
                Some(JobOutput::Cancelled)
            );
            assert_eq!(
                scheduler.provider_completion(&operation).await.unwrap(),
                None
            );
            assert_eq!(
                scheduler.retry_cancellation(&operation).await.unwrap(),
                None
            );
            assert_eq!(live.load(Ordering::SeqCst), 0);
            assert_eq!(
                cleanups.load(Ordering::SeqCst),
                0,
                "read-only replay was treated as external execution"
            );
        }
    }
    #[tokio::test]
    async fn replay_provider_rejects_missing_and_malformed_tool_outputs() {
        let store = Arc::new(Store::memory().unwrap());
        let root = RequestId("replay-output-root".into());
        store.create_request(&root, None, "/root").unwrap();
        let provider = ReplayProvider::new(store.clone(), &root).unwrap();
        lock(&provider.progress.local_requests).insert(
            RequestId("replay-direct".into()),
            LocalReplayRequest {
                original: root.clone(),
                committed: HashSet::new(),
            },
        );
        let missing = provider
            .call_with_context("echo", json!({}), call_context(CallId("missing".into())))
            .await
            .unwrap_err();
        assert!(missing.to_string().contains("no recorded tool call"));

        let malformed = CallId("malformed".into());
        store.append_items(&root, &[Item(json!({"type":"function_call","call_id":malformed.0,"name":"echo","arguments":"{}"}))]).unwrap();
        store.claim(&malformed, &root).unwrap();
        store
            .write_output(
                &store.claims(&malformed).unwrap()[0].operation,
                &Item(json!({
                    "type":"function_call_output",
                    "call_id":malformed.0,
                    "output":"not valid json"
                })),
                crate::store::TerminalOutcome::Success,
            )
            .unwrap();
        let invalid = provider
            .call_with_context("echo", json!({}), call_context(malformed))
            .await
            .unwrap_err();
        assert!(invalid.to_string().contains("no recorded tool call"));
    }

    #[tokio::test]
    async fn replay_provider_distinguishes_equal_call_ids_across_descendant_turns() {
        let store = Arc::new(Store::memory().unwrap());
        let root = RequestId("ambiguous-replay-root".into());
        let child = RequestId("ambiguous-replay-child".into());
        let call = Item(json!({
            "type": "custom_tool_call",
            "call_id": "ambiguous-id",
            "name": "cell",
            "input": "raw input"
        }));
        store.create_request(&root, None, "/root").unwrap();
        store.create_request(&child, Some(&root), "/root").unwrap();
        store
            .append_items(&root, std::slice::from_ref(&call))
            .unwrap();
        store
            .append_items(&child, std::slice::from_ref(&call))
            .unwrap();
        for (request, value) in [(&root, "first"), (&child, "second")] {
            let operation = store
                .claim(&CallId("ambiguous-id".into()), request)
                .unwrap();
            store
                .write_output(
                    &operation,
                    &Item(json!({
                        "type":"custom_tool_call_output", "call_id":"ambiguous-id", "output":value
                    })),
                    crate::store::TerminalOutcome::Success,
                )
                .unwrap();
        }
        store
            .record_replay_turn(
                &root,
                &request("ambiguous-session"),
                &ResponsesTurn {
                    response_id: "ambiguous-response".into(),
                    items: vec![call.clone()],
                    usage: Usage::default(),
                },
            )
            .unwrap();
        store
            .record_replay_turn(
                &child,
                &request("ambiguous-session-child"),
                &ResponsesTurn {
                    response_id: "ambiguous-response-child".into(),
                    items: vec![call],
                    usage: Usage::default(),
                },
            )
            .unwrap();

        let provider = ReplayProvider::new(store, &root).unwrap();
        assert_eq!(provider.calls.len(), 2);
        for (local, session, expected) in [
            ("local-first", "ambiguous-session", "first"),
            ("local-second", "ambiguous-session-child", "second"),
        ] {
            let local = RequestId(local.into());
            let (sink, _stream) = tokio::sync::mpsc::channel(4);
            provider
                .create_streaming_for_request(&local, request(session), sink)
                .await
                .unwrap();
            let mut context = call_context(CallId("ambiguous-id".into()));
            context.operation.as_mut().unwrap().request = local.clone();
            context.request = Some(local);
            assert_eq!(
                provider
                    .call_custom_with_context("cell", "raw input".into(), context)
                    .await
                    .unwrap(),
                serde_json::Value::String(expected.into())
            );
        }
    }

    #[test]
    fn replay_provider_refuses_unpersisted_turn_invocation() {
        let store = Arc::new(Store::memory().unwrap());
        let root = RequestId("unpersisted-replay-root".into());
        store.create_request(&root, None, "/root").unwrap();
        store
            .record_replay_turn(
                &root,
                &request("unpersisted-session"),
                &ResponsesTurn {
                    response_id: "unpersisted-response".into(),
                    items: vec![Item(json!({
                        "type": "custom_tool_call",
                        "call_id": "unpersisted-call",
                        "name": "cell",
                        "input": "raw input"
                    }))],
                    usage: Usage::default(),
                },
            )
            .unwrap();
        assert!(matches!(
            ReplayProvider::new(store, &root),
            Err(ReplayError::MissingInvocation(id)) if id == "unpersisted-call"
        ));
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
        let agent = AgentPath("/root".into());
        let request = Some(RequestId("request".into()));
        let cancel = tokio_util::sync::CancellationToken::new();
        let (progress, _receiver) =
            tokio::sync::mpsc::channel(crate::provider::JOB_PROGRESS_CAPACITY);
        let call_id = CallId("replay-cell".into());
        let verbs = crate::agents::JobVerbs::from_scheduler(
            None,
            agent.clone(),
            request
                .as_ref()
                .map(|request| crate::agents::AgentInvocation {
                    request: request.clone(),
                    call_id: call_id.clone(),
                }),
            cancel.clone(),
            progress.clone(),
        );
        let context = CallContext {
            handle: JobHandle("replay-cell".into()),
            operation: None,
            call_id,
            agent,
            request,
            cancel,
            verbs,
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
        let agent = AgentPath("/root".into());
        let request = None;
        let cancel = tokio_util::sync::CancellationToken::new();
        let (progress, _receiver) =
            tokio::sync::mpsc::channel(crate::provider::JOB_PROGRESS_CAPACITY);
        let call_id = CallId("cancel-cell".into());
        let verbs = crate::agents::JobVerbs::from_scheduler(
            None,
            agent.clone(),
            request
                .clone()
                .map(|request| crate::agents::AgentInvocation {
                    request,
                    call_id: call_id.clone(),
                }),
            cancel.clone(),
            progress.clone(),
        );
        let context = CallContext {
            handle: JobHandle("cancel-cell".into()),
            operation: None,
            call_id,
            agent,
            request,
            cancel,
            verbs,
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
