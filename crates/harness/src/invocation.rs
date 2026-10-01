//! Program-driven, bounded turns on the existing Engine and durable Store.
use crate::{
    engine::{Engine, EngineConfig, ResponsesTransport},
    item::{Item, ToolKind},
    model::{AgentPath, RequestId},
    provider::{Provider, ProviderError},
    store::Store,
    transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, Usage, sse::StreamEvent},
    turn::JobScheduler,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Limits {
    pub requests: u64,
    pub tools: u64,
    pub reported_tokens: u64,
    pub seconds: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            requests: 16,
            tools: 64,
            reported_tokens: 128_000,
            seconds: 300,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Exhausted {
    Requests,
    Tools,
    ReportedTokens,
    Deadline,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Counts {
    pub requests: u64,
    pub tools: u64,
    pub reported_tokens: u64,
    pub unknown_usage_requests: u64,
}
impl Counts {
    fn record_usage(&mut self, usage: Option<&Usage>) {
        if let Some(usage) = usage {
            self.reported_tokens = self
                .reported_tokens
                .saturating_add(usage.input_tokens.saturating_add(usage.output_tokens));
            if !usage.reported {
                self.unknown_usage_requests += 1;
            }
        } else {
            self.unknown_usage_requests += 1;
        }
    }
}

struct Scope {
    limits: Limits,
    deadline: Instant,
    counts: Mutex<Counts>,
}
struct Gate {
    reason: Option<Exhausted>,
}
/// One host-created budget is shared by every Model invocation in a parent cell.
/// Child scopes may narrow limits and deadlines; exhaustion latches the whole cell scope.
#[derive(Clone)]
pub struct CellBudget {
    gate: Arc<Mutex<Gate>>,
    scopes: Vec<Arc<Scope>>,
    cancelled: tokio_util::sync::CancellationToken,
}
impl CellBudget {
    pub fn new(limits: Limits) -> Self {
        Self {
            gate: Arc::new(Mutex::new(Gate { reason: None })),
            cancelled: tokio_util::sync::CancellationToken::new(),
            scopes: vec![Arc::new(Scope {
                limits,
                deadline: Instant::now()
                    .checked_add(Duration::from_secs(limits.seconds))
                    .unwrap_or_else(Instant::now),
                counts: Mutex::new(Counts::default()),
            })],
        }
    }
    pub fn narrow(&self, limits: Limits) -> Self {
        let parent = self.scopes.last().unwrap();
        let limits = Limits {
            requests: limits.requests.min(parent.limits.requests),
            tools: limits.tools.min(parent.limits.tools),
            reported_tokens: limits.reported_tokens.min(parent.limits.reported_tokens),
            seconds: limits.seconds.min(parent.limits.seconds),
        };
        let mut scopes = self.scopes.clone();
        scopes.push(Arc::new(Scope {
            limits,
            deadline: parent.deadline.min(
                Instant::now()
                    .checked_add(Duration::from_secs(limits.seconds))
                    .unwrap_or_else(Instant::now),
            ),
            counts: Mutex::new(Counts::default()),
        }));
        Self {
            gate: self.gate.clone(),
            cancelled: self.cancelled.clone(),
            scopes,
        }
    }
    pub fn counts(&self) -> Counts {
        self.scopes.last().unwrap().counts.lock().unwrap().clone()
    }
    pub fn exhausted(&self) -> Option<Exhausted> {
        let mut gate = self.gate.lock().unwrap();
        self.check(&mut gate).err()
    }
    fn check(&self, gate: &mut Gate) -> Result<(), Exhausted> {
        if let Some(reason) = &gate.reason {
            return Err(reason.clone());
        }
        for scope in &self.scopes {
            let counts = scope.counts.lock().unwrap();
            let reason = if Instant::now() >= scope.deadline {
                Some(Exhausted::Deadline)
            } else if counts.reported_tokens >= scope.limits.reported_tokens {
                Some(Exhausted::ReportedTokens)
            } else {
                None
            };
            if let Some(reason) = reason {
                gate.reason = Some(reason.clone());
                self.cancelled.cancel();
                return Err(reason);
            }
        }
        Ok(())
    }
    fn admit(&self, tool: bool) -> Result<(), Exhausted> {
        let mut gate = self.gate.lock().unwrap();
        self.check(&mut gate)?;
        for scope in &self.scopes {
            let counts = scope.counts.lock().unwrap();
            if if tool {
                counts.tools >= scope.limits.tools
            } else {
                counts.requests >= scope.limits.requests
            } {
                let reason = if tool {
                    Exhausted::Tools
                } else {
                    Exhausted::Requests
                };
                gate.reason = Some(reason.clone());
                self.cancelled.cancel();
                return Err(reason);
            }
        }
        for scope in &self.scopes {
            let mut counts = scope.counts.lock().unwrap();
            if tool {
                counts.tools += 1;
            } else {
                counts.requests += 1;
            }
        }
        Ok(())
    }
    fn usage(&self, usage: Option<&Usage>) {
        let mut gate = self.gate.lock().unwrap();
        for scope in &self.scopes {
            let mut counts = scope.counts.lock().unwrap();
            counts.record_usage(usage);
        }
        let _ = self.check(&mut gate);
    }
    fn deadline(&self) -> tokio::time::Instant {
        self.scopes.last().unwrap().deadline.into()
    }
}

/// A callback request stays owned by the original Haskell continuation.
/// Completing it is cooperative even after the budget deadline has latched.
pub struct Callback {
    /// Exact original operation; provider call IDs may recur in later requests.
    pub operation: crate::model::OperationId,
    pub name: String,
    pub arguments: Value,
    pub call_id: String,
    admission: Arc<AtomicU8>,
    reply: oneshot::Sender<Result<Value, String>>,
}
impl Callback {
    pub fn complete(self, result: Result<Value, String>) -> Result<(), Result<Value, String>> {
        self.reply.send(result)
    }
}
struct CallbackProvider {
    tools: Vec<Value>,
    budget: CellBudget,
    sender: mpsc::Sender<BridgeEvent>,
    store: Arc<Store>,
    after_tool: bool,
    projections: Mutex<std::collections::HashMap<(RequestId, String), Item>>,
    acknowledged: Mutex<std::collections::HashSet<crate::model::OperationId>>,
    closed: Arc<AtomicBool>,
    closed_signal: tokio_util::sync::CancellationToken,
    invocation_counts: Arc<Mutex<Counts>>,
    failure: Arc<Mutex<Option<String>>>,
}
const QUEUED: u8 = 0;
const ADMITTED: u8 = 1;
const REFUSED: u8 = 2;
impl CallbackProvider {
    async fn continuation<T>(
        &self,
        mut receive: oneshot::Receiver<T>,
        admission: &AtomicU8,
    ) -> Result<T, ProviderError> {
        tokio::select! {
            result = &mut receive => return result.map_err(|_| ProviderError::Tool("continuation dropped".into())),
            _ = self.closed_signal.cancelled() => {},
            _ = self.budget.cancelled.cancelled() => {},
            _ = tokio::time::sleep_until(self.budget.deadline()) => {
                self.budget.exhausted();
            },
        }
        // Delivery and refusal compete at one admission boundary. Cancellation
        // may discard queued work, but cannot revoke a handed-out continuation.
        if admission
            .compare_exchange(QUEUED, REFUSED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            Err(ProviderError::Tool("queued continuation refused".into()))
        } else {
            receive
                .await
                .map_err(|_| ProviderError::Tool("continuation dropped".into()))
        }
    }
}
#[async_trait]
impl Provider for CallbackProvider {
    fn model_visible_item(&self, request: &RequestId, item: &Item) -> Option<Item> {
        if item.0["type"] != "function_call_output" {
            return None;
        }
        self.projections
            .lock()
            .unwrap()
            .get(&(request.clone(), item.0["call_id"].as_str()?.to_owned()))
            .cloned()
    }
    async fn output_committed(
        &self,
        operation: &crate::model::OperationId,
    ) -> Result<(), ProviderError> {
        if !self.after_tool
            || self.failure.lock().unwrap().is_some()
            || self.closed.load(Ordering::Acquire)
            || self.acknowledged.lock().unwrap().contains(operation)
        {
            return Ok(());
        }
        let output = self
            .store
            .replay_output_operation(operation)
            .map_err(|error| ProviderError::Tool(error.to_string().into()))?
            .ok_or_else(|| ProviderError::Tool("missing retained callback output".into()))?;
        let ordinal = self
            .store
            .record_event(
                Some(&operation.request),
                "model_tool_result",
                &json!({"operation":operation,"output":output}),
            )
            .map_err(|error| ProviderError::Tool(error.to_string().into()))?;
        let (reply, receive) = oneshot::channel();
        let admission = Arc::new(AtomicU8::new(QUEUED));
        self.sender
            .send(BridgeEvent::Hook(Hook {
                operation: operation.clone(),
                output,
                ordinal,
                admission: admission.clone(),
                reply,
            }))
            .await
            .map_err(|_| ProviderError::Tool("hook continuation closed".into()))?;
        let annotation = self.continuation(receive, &admission).await?;
        self.store
            .record_event(
                Some(&operation.request),
                "model_tool_annotation",
                &json!({"ordinal":ordinal,"annotation":annotation}),
            )
            .map_err(|error| ProviderError::Tool(error.to_string().into()))?;
        let text = match annotation {
            HookAnnotation::Annotated(text) => Some(text),
            HookAnnotation::Pruned {
                replacement,
                annotation,
            } => {
                let projected = Item::tool_output(
                    &operation.call,
                    ToolKind::Function,
                    &crate::turn::JobOutput::Completed(Ok(replacement)),
                );
                self.projections.lock().unwrap().insert(
                    (operation.request.clone(), operation.call.0.clone()),
                    projected,
                );
                Some(annotation)
            }
            HookAnnotation::NoAnnotation | HookAnnotation::Abstained(_) => None,
        };
        if let Some(text) = text.filter(|text| !text.is_empty()) {
            self.store.append_items(&operation.request,&[Item(json!({"type":"message","role":"developer","content":[{"type":"input_text","text":text}]}))])
                .map_err(|error|ProviderError::Tool(error.to_string().into()))?;
        }
        self.acknowledged.lock().unwrap().insert(operation.clone());
        Ok(())
    }
    fn holds_job_capacity(&self) -> bool {
        false
    }
    fn all_tools(&self) -> Vec<Value> {
        self.tools.clone()
    }
    fn tools(&self) -> Vec<Value> {
        self.tools.clone()
    }
    fn validate_call(&self, name: &str, kind: ToolKind) -> Result<(), ProviderError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ProviderError::Tool("invocation closed".into()));
        }
        self.budget.admit(true).map_err(|reason| {
            ProviderError::Tool(format!("budget exhausted: {reason:?}").into())
        })?;
        self.invocation_counts.lock().unwrap().tools += 1;
        if kind != ToolKind::Function
            || !self
                .tools
                .iter()
                .any(|tool| tool["name"].as_str() == Some(name))
            || crate::provider::is_harness_tool(name)
        {
            return Err(ProviderError::Tool(
                format!("undeclared callback {name}").into(),
            ));
        }
        self.budget.exhausted().map_or(Ok(()), |reason| {
            Err(ProviderError::Tool(
                format!("budget exhausted: {reason:?}").into(),
            ))
        })
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool(
            "callback needs scheduled identity".into(),
        ))
    }
    async fn call_with_context(
        &self,
        name: &str,
        args: Value,
        context: crate::provider::CallContext,
    ) -> Result<Value, ProviderError> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(name))
            .ok_or_else(|| ProviderError::Tool(format!("undeclared callback {name}").into()))?;
        let schema = &tool["parameters"];
        if let Err(error) = crate::finalize::validate_result(&args, schema, "$") {
            *self.failure.lock().unwrap() = Some(error.to_string());
            return Err(ProviderError::Tool(error.to_string().into()));
        }
        let (reply, receive) = oneshot::channel();
        let admission = Arc::new(AtomicU8::new(QUEUED));
        self.sender
            .send(BridgeEvent::Callback(Callback {
                operation: context.operation.ok_or_else(|| {
                    ProviderError::Tool("callback needs exact scheduled operation".into())
                })?,
                name: name.into(),
                arguments: args,
                call_id: context.call_id.0,
                admission: admission.clone(),
                reply,
            }))
            .await
            .map_err(|_| ProviderError::Tool("callback continuation closed".into()))?;
        self.continuation(receive, &admission)
            .await?
            .map_err(|error| ProviderError::Tool(error.into()))
    }
}
struct BudgetTransport<C> {
    inner: C,
    budget: CellBudget,
    requests: Arc<Mutex<Vec<RequestId>>>,
    closed: Arc<AtomicBool>,
    closed_signal: tokio_util::sync::CancellationToken,
    invocation_counts: Arc<Mutex<Counts>>,
    failure: Arc<Mutex<Option<String>>>,
}
#[async_trait]
impl<C: ResponsesTransport> ResponsesTransport for BudgetTransport<C> {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Stream("invocation closed".into()));
        }
        if let Some(failure) = self.failure.lock().unwrap().clone() {
            return Err(TransportError::Stream(failure));
        }
        self.budget
            .admit(false)
            .map_err(|reason| TransportError::Stream(format!("budget exhausted: {reason:?}")))?;
        self.invocation_counts.lock().unwrap().requests += 1;
        let result = tokio::select! {
            result = tokio::time::timeout_at(self.budget.deadline(), self.inner.create(request)) => result,
            _ = self.closed_signal.cancelled() => Ok(Err(TransportError::Stream("invocation closed".into()))),
            _ = self.budget.cancelled.cancelled() => Ok(Err(TransportError::Stream("shared cell budget exhausted".into()))),
        };
        let result =
            result.unwrap_or_else(|_| Err(TransportError::Stream("budget deadline".into())));
        let usage = result.as_ref().ok().map(|turn| &turn.usage);
        self.budget.usage(usage);
        self.invocation_counts.lock().unwrap().record_usage(usage);
        result
    }
    async fn create_streaming_for_request(
        &self,
        id: &RequestId,
        request: ResponsesRequest,
        sink: mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        self.requests.lock().unwrap().push(id.clone());
        if self.closed.load(Ordering::Acquire) {
            return Err(TransportError::Stream("invocation closed".into()));
        }
        if let Some(failure) = self.failure.lock().unwrap().clone() {
            return Err(TransportError::Stream(failure));
        }
        self.budget
            .admit(false)
            .map_err(|reason| TransportError::Stream(format!("budget exhausted: {reason:?}")))?;
        self.invocation_counts.lock().unwrap().requests += 1;
        let result = tokio::select! {
            result = tokio::time::timeout_at(self.budget.deadline(), self.inner.create_streaming_for_request(id, request, sink)) => result,
            _ = self.closed_signal.cancelled() => Ok(Err(TransportError::Stream("invocation closed".into()))),
            _ = self.budget.cancelled.cancelled() => Ok(Err(TransportError::Stream("shared cell budget exhausted".into()))),
        };
        let result =
            result.unwrap_or_else(|_| Err(TransportError::Stream("budget deadline".into())));
        let usage = result.as_ref().ok().map(|turn| &turn.usage);
        self.budget.usage(usage);
        self.invocation_counts.lock().unwrap().record_usage(usage);
        result
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Outcome {
    Text(String),
    Typed(Value),
    Exhausted(Exhausted),
    Cancelled,
    Failed(String),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub invocation_id: String,
    pub parent_cell: String,
    pub requests: Vec<RequestId>,
    pub counts: Counts,
    pub cell_counts: Counts,
    /// Items remain durable in Store; wire receipts carry request evidence references.
    #[serde(skip)]
    pub transcript: Vec<Item>,
    pub outcome: Outcome,
}
pub struct Hook {
    pub operation: crate::model::OperationId,
    pub output: Item,
    pub ordinal: i64,
    admission: Arc<AtomicU8>,
    reply: oneshot::Sender<HookAnnotation>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum HookAnnotation {
    #[default]
    NoAnnotation,
    Abstained(String),
    Annotated(String),
    Pruned {
        replacement: Value,
        annotation: String,
    },
}
impl Hook {
    pub fn complete(self, annotation: HookAnnotation) -> Result<(), HookAnnotation> {
        self.reply.send(annotation)
    }
}
enum BridgeEvent {
    Callback(Callback),
    Hook(Hook),
}
pub enum Step {
    Callback(Callback),
    HookRequested(Hook),
    Finished(Receipt),
}
#[derive(Clone, Debug, Default)]
pub struct InvocationOptions {
    pub result_schema: Option<Value>,
    pub after_tool: bool,
}
/// Host control that can signal provider cancellation without acquiring the
/// invocation step lock. An admitted callback retains its completion sender.
#[derive(Clone)]
pub struct InvocationCloseHandle {
    closed: Arc<AtomicBool>,
    signal: tokio_util::sync::CancellationToken,
}
impl InvocationCloseHandle {
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.signal.cancel();
    }
}
/// A single invocation has no authored conversation handle, mailbox, actor or checkout.
pub struct Invocation {
    callbacks: mpsc::Receiver<BridgeEvent>,
    completion: oneshot::Receiver<Receipt>,
    closed: Arc<AtomicBool>,
    closed_signal: tokio_util::sync::CancellationToken,
    budget: CellBudget,
    identity: (String, String),
    finished: Option<Receipt>,
}
impl Invocation {
    pub fn start<A: Auth + 'static, C: ResponsesTransport + 'static>(
        transport: C,
        store: Arc<Store>,
        scheduler: Arc<JobScheduler>,
        mut config: EngineConfig,
        parent_cell: String,
        budget: CellBudget,
        initial: Vec<Item>,
        options: InvocationOptions,
    ) -> Result<Self, crate::finalize::FinalizeError> {
        let mut names = std::collections::HashSet::new();
        for tool in &mut config.tools {
            let name = tool["name"]
                .as_str()
                .ok_or_else(|| {
                    crate::finalize::FinalizeError::MalformedArguments("tool name missing".into())
                })?
                .to_owned();
            if crate::provider::is_harness_tool(&name)
                || name == "finalize"
                || !names.insert(name.clone())
                || tool["type"] != "function"
            {
                return Err(crate::finalize::FinalizeError::MalformedArguments(format!(
                    "duplicate, reserved or unsupported tool {name}"
                )));
            }
            tool["parameters"] =
                crate::finalize::normalize_schema(tool["parameters"].clone(), "$input")?;
            tool["strict"] = Value::Bool(true);
        }
        if let Some(schema) = &options.result_schema {
            crate::finalize::tool_schema_from_result_schema(schema.clone())?;
        }
        let invocation_id = uuid::Uuid::new_v4().to_string();
        config.agent = AgentPath(format!("model/{parent_cell}/{invocation_id}"));
        config.session_id = invocation_id.clone();
        let tools = std::mem::take(&mut config.tools);
        let (sender, callbacks) = mpsc::channel(1);
        let closed = Arc::new(AtomicBool::new(false));
        let closed_signal = tokio_util::sync::CancellationToken::new();
        let invocation_counts = Arc::new(Mutex::new(Counts::default()));
        let failure = Arc::new(Mutex::new(None));
        let provider = Arc::new(CallbackProvider {
            tools,
            budget: budget.clone(),
            sender,
            store: store.clone(),
            after_tool: options.after_tool,
            projections: Mutex::new(std::collections::HashMap::new()),
            acknowledged: Mutex::new(std::collections::HashSet::new()),
            closed: closed.clone(),
            closed_signal: closed_signal.clone(),
            invocation_counts: invocation_counts.clone(),
            failure: failure.clone(),
        });
        let result_schema = options.result_schema;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let transport = BudgetTransport {
            inner: transport,
            budget: budget.clone(),
            requests: requests.clone(),
            closed: closed.clone(),
            closed_signal: closed_signal.clone(),
            invocation_counts: invocation_counts.clone(),
            failure: failure.clone(),
        };
        let engine = Engine::<A, _, _>::with_transport(
            transport,
            store.clone(),
            scheduler,
            provider,
            config,
        )
        .bounded_invocation();
        let (complete, completion) = oneshot::channel();
        let worker_closed = closed.clone();
        let identity = (invocation_id.clone(), parent_cell.clone());
        let step_budget = budget.clone();
        tokio::spawn(async move {
            let result = engine.run_invocation(initial, result_schema.clone()).await;
            let requests = requests.lock().unwrap().clone();
            let mut transcript = Vec::new();
            // Each invocation starts a fresh branch; retained request items are disjoint.
            for request in &requests {
                if let Ok(items) = store.items(request) {
                    transcript.extend(items);
                }
            }
            let outcome = if let Some(failure) = failure.lock().unwrap().clone() {
                Outcome::Failed(failure)
            } else if worker_closed.load(Ordering::Acquire) {
                Outcome::Cancelled
            } else if let Some(reason) = budget.exhausted() {
                Outcome::Exhausted(reason)
            } else {
                match result {
                    Ok(completion) => {
                        transcript = completion.transcript;
                        if result_schema.is_some() {
                            let item = completion
                                .turn
                                .items
                                .iter()
                                .find(|item| item.0["name"] == "finalize");
                            match item
                                .ok_or(crate::finalize::FinalizeError::WrongTool)
                                .and_then(|item| {
                                    crate::finalize::FinalizeParser::new()
                                        .parse_completed::<Value>(item)
                                }) {
                                Ok(value) => Outcome::Typed(value),
                                Err(error) => Outcome::Failed(error.to_string()),
                            }
                        } else {
                            Outcome::Text(
                                completion
                                    .turn
                                    .items
                                    .iter()
                                    .flat_map(|item| {
                                        item.0["content"].as_array().into_iter().flatten()
                                    })
                                    .filter_map(|part| part["text"].as_str())
                                    .collect::<Vec<_>>()
                                    .join(""),
                            )
                        }
                    }
                    Err(error) => Outcome::Failed(error.to_string()),
                }
            };
            let receipt = Receipt {
                invocation_id,
                parent_cell,
                requests,
                counts: invocation_counts.lock().unwrap().clone(),
                cell_counts: budget
                    .scopes
                    .first()
                    .unwrap()
                    .counts
                    .lock()
                    .unwrap()
                    .clone(),
                transcript,
                outcome,
            };
            let payload = json!(receipt);
            if let Err(error) = store.record_event(
                receipt.requests.last(),
                "model_invocation_receipt",
                &payload,
            ) {
                let mut receipt = receipt;
                receipt.outcome = Outcome::Failed(format!("receipt retention failed: {error}"));
                let _ = complete.send(receipt);
            } else {
                let _ = complete.send(receipt);
            }
        });
        Ok(Self {
            callbacks,
            completion,
            closed,
            closed_signal,
            budget: step_budget,
            identity,
            finished: None,
        })
    }
    pub fn close_handle(&self) -> InvocationCloseHandle {
        InvocationCloseHandle {
            closed: self.closed.clone(),
            signal: self.closed_signal.clone(),
        }
    }
    /// Refuse new work; an already handed-out callback may finish cooperatively.
    pub fn close(&mut self) {
        self.closed.store(true, Ordering::Release);
        self.closed_signal.cancel();
        self.callbacks.close();
    }
    pub async fn next(&mut self) -> Step {
        if let Some(receipt) = &self.finished {
            return Step::Finished(receipt.clone());
        }
        let result = loop {
            tokio::select! { biased;
                callback = self.callbacks.recv() => match callback {
                    Some(event) => {
                        // Delivery admits the program continuation. A close or
                        // exhausted budget refuses queued work; handed-out work
                        // retains its independent completion sender.
                        let admission = match &event {
                            BridgeEvent::Callback(callback) => &callback.admission,
                            BridgeEvent::Hook(hook) => &hook.admission,
                        };
                        if self.closed.load(Ordering::Acquire)
                            || self.budget.exhausted().is_some()
                            || admission.compare_exchange(QUEUED, ADMITTED, Ordering::AcqRel, Ordering::Acquire).is_err()
                        {
                            drop(event);
                            continue;
                        }
                        match event {
                            BridgeEvent::Callback(callback) => return Step::Callback(callback),
                            BridgeEvent::Hook(hook) => return Step::HookRequested(hook),
                        }
                    }
                    None => break (&mut self.completion).await,
                },
                receipt = &mut self.completion => break receipt,
            }
        };
        let receipt = result.unwrap_or_else(|error| Receipt {
            invocation_id: self.identity.0.clone(),
            parent_cell: self.identity.1.clone(),
            requests: Vec::new(),
            counts: Counts::default(),
            cell_counts: Counts::default(),
            transcript: Vec::new(),
            outcome: Outcome::Failed(error.to_string()),
        });
        self.finished = Some(receipt.clone());
        Step::Finished(receipt)
    }
}

impl Drop for Invocation {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Release);
        self.closed_signal.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Effort;
    use std::collections::VecDeque;
    #[derive(Clone)]
    struct TestAuth;
    impl Auth for TestAuth {
        fn access(&self) -> Result<(String, String), TransportError> {
            unreachable!()
        }
    }
    struct Script(Mutex<VecDeque<ResponsesTurn>>);
    #[async_trait]
    impl ResponsesTransport for Script {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| TransportError::Stream("script exhausted".into()))
        }
    }
    fn call(id: &str) -> Item {
        Item(json!({"type":"function_call","call_id":id,"name":"echo","arguments":"{}"}))
    }
    fn turn(items: Vec<Item>) -> ResponsesTurn {
        ResponsesTurn {
            response_id: uuid::Uuid::new_v4().to_string(),
            items,
            usage: Usage {
                reported: true,
                input_tokens: 2,
                output_tokens: 1,
                ..Usage::default()
            },
        }
    }
    fn final_turn() -> ResponsesTurn {
        turn(vec![Item(
            json!({"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"done"}]}),
        )])
    }
    fn config() -> EngineConfig {
        EngineConfig {
            instructions: "bounded".into(),
            tools: vec![
                json!({"type":"function","name":"echo","parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false},"strict":true}),
            ],
            model: "offline".into(),
            effort: Effort::Low,
            session_id: String::new(),
            agent: AgentPath(String::new()),
        }
    }
    fn start(
        store: Arc<Store>,
        scheduler: Arc<JobScheduler>,
        budget: CellBudget,
        turns: Vec<ResponsesTurn>,
        hook: bool,
    ) -> Invocation {
        Invocation::start::<TestAuth, _>(
            Script(Mutex::new(turns.into())),
            store,
            scheduler,
            config(),
            "cell1".into(),
            budget,
            vec![],
            InvocationOptions {
                after_tool: hook,
                ..InvocationOptions::default()
            },
        )
        .unwrap()
    }
    async fn finish(invocation: &mut Invocation) -> Receipt {
        loop {
            match invocation.next().await {
                Step::Callback(callback) => callback.complete(Ok(json!("result"))).unwrap(),
                Step::HookRequested(hook) => hook
                    .complete(HookAnnotation::Annotated("checked".into()))
                    .unwrap(),
                Step::Finished(receipt) => return receipt,
            }
        }
    }
    #[tokio::test]
    async fn sequential_callbacks_and_hooks_are_retained_by_real_engine() {
        let store = Arc::new(Store::memory().unwrap());
        let mut invocation = start(
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            CellBudget::new(Limits::default()),
            vec![turn(vec![call("a"), call("b")]), final_turn()],
            true,
        );
        let first = match invocation.next().await {
            Step::Callback(callback) => callback,
            _ => panic!("callback"),
        };
        assert_eq!(first.call_id, "a");
        first.complete(Ok(json!(1))).unwrap();
        let first_hook = match invocation.next().await {
            Step::HookRequested(hook) => hook,
            _ => panic!("first hook before next callback"),
        };
        assert_eq!(first_hook.operation.call.0, "a");
        first_hook
            .complete(HookAnnotation::Annotated("checked".into()))
            .unwrap();
        let second = loop {
            match invocation.next().await {
                Step::Callback(callback) => break callback,
                Step::HookRequested(hook) => hook
                    .complete(HookAnnotation::Annotated("checked".into()))
                    .unwrap(),
                _ => panic!("callback"),
            }
        };
        assert_eq!(second.call_id, "b");
        second.complete(Ok(json!(2))).unwrap();
        let second_hook = match invocation.next().await {
            Step::HookRequested(hook) => hook,
            _ => panic!("second hook before finish"),
        };
        assert_eq!(second_hook.operation.call.0, "b");
        second_hook
            .complete(HookAnnotation::Annotated("checked".into()))
            .unwrap();
        let receipt = finish(&mut invocation).await;
        assert!(matches!(receipt.outcome,Outcome::Text(ref text) if text=="done"));
        assert_eq!(receipt.counts.tools, 2);
        assert_eq!(receipt.counts.requests, 2);
        let events = store.events(None).unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "model_tool_result")
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == "model_invocation_receipt")
                .count(),
            1
        );
        assert_eq!(
            receipt
                .transcript
                .iter()
                .filter(|item| item.0["type"] == "function_call_output")
                .count(),
            2
        );
    }
    #[tokio::test]
    async fn nested_invocation_uses_one_scheduler_and_shared_request_budget() {
        let store = Arc::new(Store::memory().unwrap());
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let budget = CellBudget::new(Limits {
            requests: 3,
            ..Limits::default()
        });
        let mut parent = start(
            store.clone(),
            scheduler.clone(),
            budget.clone(),
            vec![turn(vec![call("parent")]), final_turn()],
            false,
        );
        let callback = match parent.next().await {
            Step::Callback(callback) => callback,
            _ => panic!("callback"),
        };
        let mut child = start(
            store.clone(),
            scheduler,
            budget.narrow(Limits::default()),
            vec![turn(vec![call("nested")]), final_turn()],
            false,
        );
        let child_receipt = tokio::time::timeout(Duration::from_secs(2), finish(&mut child))
            .await
            .unwrap();
        assert!(matches!(child_receipt.outcome, Outcome::Text(_)));
        callback.complete(Ok(json!("nested finished"))).unwrap();
        let receipt = finish(&mut parent).await;
        assert!(matches!(
            receipt.outcome,
            Outcome::Exhausted(Exhausted::Requests)
        ));
        assert_eq!(budget.counts().requests, 3);
        assert!(
            receipt
                .transcript
                .iter()
                .any(|item| item.0["type"] == "function_call_output")
        );
    }
    #[tokio::test]
    async fn tool_exhaustion_refuses_the_next_callback_and_retains_failure() {
        let store = Arc::new(Store::memory().unwrap());
        let budget = CellBudget::new(Limits {
            tools: 1,
            ..Limits::default()
        });
        let mut invocation = start(
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            budget,
            vec![turn(vec![call("a"), call("b")]), final_turn()],
            false,
        );
        let receipt = finish(&mut invocation).await;
        assert_eq!(receipt.counts.tools, 1);
        assert!(matches!(
            receipt.outcome,
            Outcome::Exhausted(Exhausted::Tools)
        ));
        assert!(
            store
                .events(None)
                .unwrap()
                .iter()
                .any(|event| event.kind == "model_invocation_receipt")
        );
    }
    #[test]
    fn narrowing_cannot_broaden_or_reset_the_cell_budget() {
        let root = CellBudget::new(Limits {
            requests: 2,
            ..Limits::default()
        });
        root.admit(false).unwrap();
        let child = root.narrow(Limits {
            requests: 1,
            ..Limits::default()
        });
        child.admit(false).unwrap();
        assert_eq!(child.admit(false), Err(Exhausted::Requests));
        assert_eq!(root.exhausted(), Some(Exhausted::Requests));
        assert_eq!(root.counts().requests, 2);
    }
    #[tokio::test]
    async fn invalid_arguments_never_enter_haskell_callback() {
        let store = Arc::new(Store::memory().unwrap());
        let bad = Item(
            json!({"type":"function_call","call_id":"bad","name":"echo","arguments":"{\"extra\":true}"}),
        );
        let mut invocation = start(
            store,
            Arc::new(JobScheduler::new(1).unwrap()),
            CellBudget::new(Limits::default()),
            vec![turn(vec![bad]), final_turn()],
            false,
        );
        match invocation.next().await {
            Step::Finished(receipt) => assert!(matches!(receipt.outcome, Outcome::Failed(_))),
            _ => panic!("invalid input was dispatched"),
        }
    }
    #[tokio::test]
    async fn close_preserves_already_admitted_callback_output() {
        let store = Arc::new(Store::memory().unwrap());
        let mut invocation = start(
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            CellBudget::new(Limits::default()),
            vec![turn(vec![call("a")]), final_turn()],
            false,
        );
        let callback = match invocation.next().await {
            Step::Callback(callback) => callback,
            _ => panic!("callback"),
        };
        invocation.close_handle().close();
        callback
            .complete(Ok(json!("finished cooperatively")))
            .unwrap();
        let receipt = finish(&mut invocation).await;
        assert!(matches!(receipt.outcome, Outcome::Cancelled));
        assert_eq!(receipt.counts.requests, 1);
        assert!(
            receipt
                .transcript
                .iter()
                .any(|item| item.0["type"] == "function_call_output")
        );
        assert!(
            !serde_json::to_value(&receipt)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("transcript")
        );
        assert!(matches!(invocation.next().await, Step::Finished(_)));
    }
    #[tokio::test]
    async fn deadline_allows_admitted_callback_to_finish_cooperatively() {
        let store = Arc::new(Store::memory().unwrap());
        let budget = CellBudget::new(Limits {
            seconds: 1,
            ..Limits::default()
        });
        let mut invocation = start(
            store,
            Arc::new(JobScheduler::new(1).unwrap()),
            budget,
            vec![turn(vec![call("a")]), final_turn()],
            false,
        );
        let callback = match invocation.next().await {
            Step::Callback(callback) => callback,
            _ => panic!("callback"),
        };
        tokio::time::sleep(Duration::from_millis(1100)).await;
        callback
            .complete(Ok(json!("finished after deadline")))
            .unwrap();
        let receipt = finish(&mut invocation).await;
        assert!(matches!(
            receipt.outcome,
            Outcome::Exhausted(Exhausted::Deadline)
        ));
        assert!(
            receipt
                .transcript
                .iter()
                .any(|item| item.0["type"] == "function_call_output")
        );
    }
    #[tokio::test]
    async fn reported_token_cutoff_and_unknown_usage_are_explicit() {
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let mut exhausted = start(
            Arc::new(Store::memory().unwrap()),
            scheduler.clone(),
            CellBudget::new(Limits {
                reported_tokens: 3,
                ..Limits::default()
            }),
            vec![final_turn()],
            false,
        );
        let receipt = finish(&mut exhausted).await;
        assert!(matches!(
            receipt.outcome,
            Outcome::Exhausted(Exhausted::ReportedTokens)
        ));
        assert_eq!(receipt.counts.reported_tokens, 3);
        let mut unknown = final_turn();
        unknown.usage = Usage::default();
        let mut invocation = start(
            Arc::new(Store::memory().unwrap()),
            scheduler,
            CellBudget::new(Limits::default()),
            vec![unknown],
            false,
        );
        let receipt = finish(&mut invocation).await;
        assert!(matches!(receipt.outcome, Outcome::Text(_)));
        assert_eq!(receipt.counts.unknown_usage_requests, 1);
        assert_eq!(receipt.counts.reported_tokens, 0);
        let mut partial = final_turn();
        partial.usage.reported = false;
        partial.usage.input_tokens = 7;
        partial.usage.output_tokens = 0;
        let mut invocation = start(
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            CellBudget::new(Limits::default()),
            vec![partial],
            false,
        );
        let receipt = finish(&mut invocation).await;
        assert_eq!(receipt.counts.reported_tokens, 7);
        assert_eq!(receipt.counts.unknown_usage_requests, 1);
    }
    #[tokio::test]
    async fn pruning_projects_only_model_input_and_retains_original_output() {
        struct Inspect {
            turns: Mutex<VecDeque<ResponsesTurn>>,
            inputs: Arc<Mutex<Vec<ResponsesRequest>>>,
        }
        #[async_trait]
        impl ResponsesTransport for Inspect {
            async fn create(
                &self,
                request: ResponsesRequest,
            ) -> Result<ResponsesTurn, TransportError> {
                self.inputs.lock().unwrap().push(request);
                Ok(self.turns.lock().unwrap().pop_front().unwrap())
            }
        }
        let store = Arc::new(Store::memory().unwrap());
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let config = EngineConfig {
            instructions: String::new(),
            tools: vec![
                json!({"type":"function","name":"echo","parameters":{"type":"object","properties":{}}}),
            ],
            model: "offline".into(),
            effort: Effort::Low,
            session_id: String::new(),
            agent: AgentPath(String::new()),
        };
        let mut invocation = Invocation::start::<TestAuth, _>(
            Inspect {
                turns: Mutex::new(vec![turn(vec![call("a")]), final_turn()].into()),
                inputs: inputs.clone(),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            config,
            "cell".into(),
            CellBudget::new(Limits::default()),
            vec![],
            InvocationOptions {
                after_tool: true,
                ..InvocationOptions::default()
            },
        )
        .unwrap();
        let callback = match invocation.next().await {
            Step::Callback(callback) => callback,
            _ => panic!("callback"),
        };
        callback.complete(Ok(json!({"full":"original"}))).unwrap();
        let hook = match invocation.next().await {
            Step::HookRequested(hook) => hook,
            _ => panic!("hook"),
        };
        assert!(
            store
                .events(Some(&hook.operation.request))
                .unwrap()
                .iter()
                .any(|event| event.id == hook.ordinal)
        );
        hook.complete(HookAnnotation::Pruned {
            replacement: json!("short"),
            annotation: "retained full output".into(),
        })
        .unwrap();
        let receipt = finish(&mut invocation).await;
        let inputs = inputs.lock().unwrap();
        let projected = inputs[1]
            .input
            .iter()
            .find(|item| item.0["type"] == "function_call_output")
            .unwrap();
        assert_eq!(projected.0["output"], "\"short\"");
        let original = receipt
            .transcript
            .iter()
            .find(|item| item.0["type"] == "function_call_output")
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(original.0["output"].as_str().unwrap()).unwrap(),
            json!({"full":"original"})
        );
    }
    #[test]
    fn duplicate_and_reserved_manifests_fail_before_any_provider_request() {
        for names in [vec!["echo", "echo"], vec!["spawn_agent"], vec!["finalize"]] {
            let config=EngineConfig { instructions:String::new(),tools:names.into_iter().map(|name|json!({"type":"function","name":name,"parameters":{"type":"object","properties":{}}})).collect(),model:"offline".into(),effort:Effort::Low,session_id:String::new(),agent:AgentPath(String::new()) };
            let result = Invocation::start::<TestAuth, _>(
                Script(Mutex::new(VecDeque::new())),
                Arc::new(Store::memory().unwrap()),
                Arc::new(JobScheduler::new(1).unwrap()),
                config,
                "cell".into(),
                CellBudget::new(Limits::default()),
                vec![],
                InvocationOptions::default(),
            );
            assert!(result.is_err());
        }
    }
    #[tokio::test]
    async fn shared_exhaustion_cancels_inflight_provider_work() {
        struct Pending {
            entered: Arc<tokio::sync::Notify>,
            dropped: Arc<AtomicBool>,
        }
        struct Guard(Arc<AtomicBool>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        #[async_trait]
        impl ResponsesTransport for Pending {
            async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
                let _guard = Guard(self.dropped.clone());
                self.entered.notify_one();
                std::future::pending().await
            }
        }
        let store = Arc::new(Store::memory().unwrap());
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let budget = CellBudget::new(Limits {
            requests: 1,
            ..Limits::default()
        });
        let entered = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(AtomicBool::new(false));
        let mut active = Invocation::start::<TestAuth, _>(
            Pending {
                entered: entered.clone(),
                dropped: dropped.clone(),
            },
            store.clone(),
            scheduler.clone(),
            config(),
            "cell1".into(),
            budget.clone(),
            vec![],
            InvocationOptions::default(),
        )
        .unwrap();
        entered.notified().await;
        let mut rejected = start(store, scheduler, budget, vec![final_turn()], false);
        let rejected = finish(&mut rejected).await;
        assert!(matches!(
            rejected.outcome,
            Outcome::Exhausted(Exhausted::Requests)
        ));
        let active = tokio::time::timeout(Duration::from_secs(2), finish(&mut active))
            .await
            .unwrap();
        assert!(matches!(
            active.outcome,
            Outcome::Exhausted(Exhausted::Requests)
        ));
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(active.counts.requests, 1);
        assert_eq!(rejected.counts.requests, 0);
    }
    #[tokio::test]
    async fn typed_completion_uses_existing_finalize_owner() {
        let schema = json!({"type":"object","properties":{"maybe":{"type":"integer"},"number":{"type":"number"}},"required":["number"],"additionalProperties":false});
        let result = json!({"maybe":null,"number":2.5});
        let final_call = Item(
            json!({"type":"function_call","name":"finalize","call_id":"final","arguments":serde_json::to_string(&json!({"result":result})).unwrap()}),
        );
        let mut invocation = Invocation::start::<TestAuth, _>(
            Script(Mutex::new(vec![turn(vec![final_call])].into())),
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            config(),
            "cell1".into(),
            CellBudget::new(Limits::default()),
            vec![],
            InvocationOptions {
                result_schema: Some(schema),
                after_tool: false,
            },
        )
        .unwrap();
        let receipt = finish(&mut invocation).await;
        assert!(matches!(receipt.outcome,Outcome::Typed(value) if value==result));
        assert_eq!(receipt.counts.tools, 0);
    }
    #[tokio::test]
    async fn bounded_callbacks_do_not_stall_provider_stream_polling() {
        struct Streaming {
            turns: Mutex<VecDeque<ResponsesTurn>>,
            completed: Arc<AtomicBool>,
        }
        #[async_trait]
        impl ResponsesTransport for Streaming {
            async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
                unreachable!()
            }
            async fn create_streaming_for_request(
                &self,
                _: &RequestId,
                _: ResponsesRequest,
                sink: mpsc::Sender<StreamEvent>,
            ) -> Result<ResponsesTurn, TransportError> {
                let turn = self.turns.lock().unwrap().pop_front().unwrap();
                for item in &turn.items {
                    sink.send(StreamEvent::ItemDone(item.clone()))
                        .await
                        .unwrap();
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
                self.completed.store(true, Ordering::Release);
                Ok(turn)
            }
        }
        let completed = Arc::new(AtomicBool::new(false));
        let mut invocation = Invocation::start::<TestAuth, _>(
            Streaming {
                turns: Mutex::new(vec![turn(vec![call("a"), call("b")]), final_turn()].into()),
                completed: completed.clone(),
            },
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            config(),
            "cell1".into(),
            CellBudget::new(Limits::default()),
            vec![],
            InvocationOptions::default(),
        )
        .unwrap();
        let callback = tokio::time::timeout(Duration::from_secs(2), invocation.next())
            .await
            .unwrap();
        assert!(completed.load(Ordering::Acquire));
        match callback {
            Step::Callback(callback) => {
                assert_eq!(callback.call_id, "a");
                callback.complete(Ok(json!("done"))).unwrap();
            }
            _ => panic!("callback"),
        }
        match invocation.next().await {
            Step::Callback(callback) => {
                assert_eq!(callback.call_id, "b");
                callback.complete(Ok(json!("done"))).unwrap();
            }
            _ => panic!("second callback"),
        }
        assert!(matches!(
            finish(&mut invocation).await.outcome,
            Outcome::Text(_)
        ));
    }
    #[tokio::test]
    async fn text_finalize_is_refused_without_waiting_for_callback() {
        let mut invocation = start(
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            CellBudget::new(Limits::default()),
            vec![turn(vec![Item(json!({
                "type": "function_call", "name": "finalize",
                "call_id": "final", "arguments": "{}"
            }))])],
            false,
        );
        let step = tokio::time::timeout(Duration::from_secs(2), invocation.next())
            .await
            .expect("undeclared finalize must settle promptly");
        match step {
            Step::Finished(receipt) => {
                assert!(matches!(receipt.outcome, Outcome::Failed(_)));
            }
            _ => panic!("undeclared finalize must never reach the callback"),
        }
    }
    #[tokio::test]
    async fn conflicting_streamed_call_is_refused_before_callback() {
        struct Conflict;
        #[async_trait]
        impl ResponsesTransport for Conflict {
            async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
                unreachable!()
            }
            async fn create_streaming_for_request(
                &self,
                _: &RequestId,
                _: ResponsesRequest,
                sink: mpsc::Sender<StreamEvent>,
            ) -> Result<ResponsesTurn, TransportError> {
                sink.send(StreamEvent::ItemDone(call("a"))).await.unwrap();
                let mut changed = call("a");
                changed.0["arguments"] = json!("{\"unexpected\":true}");
                Ok(turn(vec![changed]))
            }
        }
        let mut invocation = Invocation::start::<TestAuth, _>(
            Conflict,
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            config(),
            "cell1".into(),
            CellBudget::new(Limits::default()),
            vec![],
            InvocationOptions::default(),
        )
        .unwrap();
        let receipt = match invocation.next().await {
            Step::Finished(receipt) => receipt,
            _ => panic!("conflicting provider call must not reach callback"),
        };
        assert!(matches!(receipt.outcome, Outcome::Failed(_)));
        assert_eq!(receipt.counts.tools, 0);
    }

    #[tokio::test]
    async fn queued_callback_is_refused_after_close_or_shared_exhaustion() {
        for close in [true, false] {
            let store = Arc::new(Store::memory().unwrap());
            let budget = CellBudget::new(Limits {
                requests: 1,
                ..Limits::default()
            });
            let mut invocation = start(
                store.clone(),
                Arc::new(JobScheduler::new(1).unwrap()),
                budget.clone(),
                vec![turn(vec![call("queued")])],
                false,
            );
            tokio::time::timeout(Duration::from_secs(2), async {
                while invocation.callbacks.is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            if close {
                invocation.close_handle().close();
            } else {
                assert_eq!(budget.admit(false), Err(Exhausted::Requests));
            }
            // Cancellation settles queued work even when the host never asks
            // for another step. The queue still holds the undelivered callback.
            let receipt = tokio::time::timeout(Duration::from_secs(2), &mut invocation.completion)
                .await
                .expect("queued work must settle without entering the continuation")
                .unwrap();
            assert!(!invocation.callbacks.is_empty());
            if close {
                assert!(matches!(receipt.outcome, Outcome::Cancelled));
            } else {
                assert!(matches!(
                    receipt.outcome,
                    Outcome::Exhausted(Exhausted::Requests)
                ));
            }
            assert!(
                receipt
                    .transcript
                    .iter()
                    .any(|item| item.0["type"] == "function_call_output")
            );
        }
    }

    #[tokio::test]
    async fn repeated_provider_call_ids_keep_exact_callback_and_hook_identity() {
        let mut invocation = start(
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(1).unwrap()),
            CellBudget::new(Limits::default()),
            vec![
                turn(vec![call("repeat")]),
                turn(vec![call("repeat")]),
                final_turn(),
            ],
            true,
        );
        let mut operations = Vec::new();
        for value in [1, 2] {
            let Step::Callback(callback) = invocation.next().await else {
                panic!("callback");
            };
            let operation = callback.operation.clone();
            assert_eq!(operation.call.0, callback.call_id);
            callback.complete(Ok(json!(value))).unwrap();
            let Step::HookRequested(hook) = invocation.next().await else {
                panic!("hook");
            };
            assert_eq!(hook.operation, operation);
            hook.complete(HookAnnotation::NoAnnotation).unwrap();
            operations.push(operation);
        }
        assert_eq!(operations[0].call, operations[1].call);
        assert_ne!(operations[0].request, operations[1].request);
        assert!(matches!(
            finish(&mut invocation).await.outcome,
            Outcome::Text(_)
        ));
    }

    #[tokio::test]
    async fn queued_hook_close_retains_output_without_waiting_for_next_step() {
        let store = Arc::new(Store::memory().unwrap());
        let mut invocation = start(
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            CellBudget::new(Limits::default()),
            vec![turn(vec![call("a")])],
            true,
        );
        let Step::Callback(callback) = invocation.next().await else {
            panic!("callback");
        };
        callback.complete(Ok(json!("original"))).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while invocation.callbacks.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        invocation.close_handle().close();
        let receipt = tokio::time::timeout(Duration::from_secs(2), &mut invocation.completion)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(receipt.outcome, Outcome::Cancelled));
        assert!(
            receipt
                .transcript
                .iter()
                .any(|item| item.0["type"] == "function_call_output")
        );
        assert!(
            store
                .events(None)
                .unwrap()
                .iter()
                .any(|event| event.kind == "model_tool_result")
        );
    }
}
