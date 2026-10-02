//! Asynchronous tool jobs and request scheduling primitives.
//!
//! A tool invocation is admitted independently of the model request which
//! created it. Its output is retained against the call id, and claimants can
//! replay that output (notably after a fork) without invoking the provider a
//! second time.
use crate::{
    item::{Item, ToolInput},
    mailbox::{Envelope, MailboxSignal},
    model::{AgentPath, CallId, ConversationIdentity, OperationId, RequestId},
    provider::{CallContext, JobHandle, Provider, ToolFailure},
};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};
use thiserror::Error;
use tokio::{
    sync::{Mutex, Semaphore, broadcast},
    task::JoinHandle,
};

const JOB_CANCELLATION_GRACE: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, PartialEq)]
pub enum JobOutput {
    Completed(Result<Value, ToolFailure>),
    Cancelled,
    CancelledWithReceipt(Result<Value, ToolFailure>),
    Interrupted,
    CancellationUnconfirmed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct JobSettlement {
    pub operation: OperationId,
    pub call_id: CallId,
    pub output: JobOutput,
    /// Conversations whose outstanding call is made ready by this settlement.
    pub claimants: Vec<ConversationIdentity>,
}

#[derive(Debug, Error)]
pub enum JobError {
    #[error("job capacity must be greater than zero")]
    ZeroCapacity,
    #[error("call id is already registered")]
    DuplicateCall,
    #[error("unknown call id")]
    UnknownCall,
    #[error("operation origin does not match scheduler admission context")]
    OperationContextMismatch,
    #[error("completed tool call item has invalid fields")]
    InvalidCallItem,
    #[error("agent verbs require function-call input")]
    AgentVerbRequiresFunction,
    #[error("finalize requires function-call input")]
    FinalizeRequiresFunction,
}

/// Inputs to the deterministic request priority policy: roots first, then
/// nodes with an operator waiting, then warm prefixes, preserving FIFO ties.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestPriority {
    pub depth: u16,
    pub operator_waiting: bool,
    pub prefix_warm: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestTicket {
    pub id: RequestId,
    pub agent: AgentPath,
    pub priority: RequestPriority,
}

/// A small admission queue for active model requests (separate from tool-job
/// capacity). Call `finish` when a response ends to free a slot.
pub struct RequestScheduler {
    capacity: usize,
    active: HashSet<RequestId>,
    queued: Vec<(u64, RequestTicket)>,
    sequence: u64,
}

impl RequestScheduler {
    pub fn new(capacity: usize) -> Result<Self, JobError> {
        if capacity == 0 {
            return Err(JobError::ZeroCapacity);
        }
        Ok(Self {
            capacity,
            active: HashSet::new(),
            queued: Vec::new(),
            sequence: 0,
        })
    }

    pub fn enqueue(&mut self, ticket: RequestTicket) -> Result<(), JobError> {
        if self.active.contains(&ticket.id)
            || self.queued.iter().any(|(_, queued)| queued.id == ticket.id)
        {
            return Err(JobError::DuplicateCall);
        }
        self.sequence += 1;
        self.queued.push((self.sequence, ticket));
        Ok(())
    }

    /// Admit the highest-priority waiting request if capacity is available.
    pub fn next_ready(&mut self) -> Option<RequestTicket> {
        if self.active.len() >= self.capacity || self.queued.is_empty() {
            return None;
        }
        let best = self
            .queued
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| compare_priority(a, b))?
            .0;
        let (_, ticket) = self.queued.swap_remove(best);
        self.active.insert(ticket.id.clone());
        Some(ticket)
    }

    pub fn finish(&mut self, request_id: &RequestId) -> bool {
        self.active.remove(request_id)
    }
}

fn compare_priority(a: &(u64, RequestTicket), b: &(u64, RequestTicket)) -> Ordering {
    a.1.priority
        .depth
        .cmp(&b.1.priority.depth)
        .then_with(|| {
            b.1.priority
                .operator_waiting
                .cmp(&a.1.priority.operator_waiting)
        })
        .then_with(|| b.1.priority.prefix_warm.cmp(&a.1.priority.prefix_warm))
        .then_with(|| a.0.cmp(&b.0))
}

struct Job {
    handle: JobHandle,
    started: bool,
    requires_publication: bool,
    cancellation_owner: Option<Arc<dyn crate::provider::CancellationOwner>>,
    cancellation_gate: Arc<Mutex<()>>,
    cancellation_ack: Option<crate::provider::CancellationAcknowledgment>,
    claimants: HashSet<ConversationIdentity>,
    settled_claimants: Vec<ConversationIdentity>,
    output: Option<JobOutput>,
    /// The actual provider-future result, retained independently from the
    /// terminal output that was already published to claimants.
    provider_completion: Option<Result<Value, ToolFailure>>,
    completion: Option<crate::provider::ProviderCompletion>,
    completion_authority: crate::provider::InvocationCompletionAuthority,
    context_completion_required: bool,
    progress: Vec<Value>,
    cancel: tokio_util::sync::CancellationToken,
    settled: tokio::sync::watch::Sender<Option<JobOutput>>,
    published: tokio::sync::watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    launch: Option<tokio::sync::oneshot::Sender<Option<crate::context::ContextSnapshot>>>,
}

fn retain_progress(progress: &mut Vec<Value>, event: Value) {
    let capacity = crate::provider::JOB_PROGRESS_CAPACITY;
    if progress.len() == capacity {
        progress.remove(0);
    }
    progress.push(event);
}

/// Owns in-flight provider work and makes settlement replayable.
///
/// The semaphore bounds concurrent calls. The same registry can be shared by
/// the request loop and the agent tree; it does not own durable persistence.
pub struct JobScheduler {
    capacity: Arc<Semaphore>,
    jobs: Arc<Mutex<HashMap<OperationId, Job>>>,
    events: broadcast::Sender<OperationId>,
    legacy_events: broadcast::Sender<CallId>,
    detached_store: String,
}

/// A bare provider ID addresses only the scheduler's detached local test API.
/// Shared Engine calls always pass their full `OperationId`.
pub trait JobKey {
    fn operation(&self, scheduler: &JobScheduler) -> OperationId;
}
impl JobKey for OperationId {
    fn operation(&self, _scheduler: &JobScheduler) -> OperationId {
        self.clone()
    }
}
impl JobKey for CallId {
    fn operation(&self, scheduler: &JobScheduler) -> OperationId {
        scheduler.detached_operation(self)
    }
}

// Existing JSON callers remain function calls at the typed dispatch boundary.
impl From<Value> for ToolInput {
    fn from(args: Value) -> Self {
        Self::Function(args)
    }
}

impl JobScheduler {
    pub fn new(capacity: usize) -> Result<Self, JobError> {
        if capacity == 0 {
            return Err(JobError::ZeroCapacity);
        }
        let (events, _) = broadcast::channel(64);
        let (legacy_events, _) = broadcast::channel(64);
        Ok(Self {
            capacity: Arc::new(Semaphore::new(capacity)),
            jobs: Arc::new(Mutex::new(HashMap::new())),
            events,
            legacy_events,
            detached_store: format!("scheduler:{}", uuid::Uuid::new_v4()),
        })
    }
    fn detached_operation(&self, call: &CallId) -> OperationId {
        OperationId {
            origin: ConversationIdentity::Standalone {
                store: self.detached_store.clone(),
                actor: AgentPath("/root".into()),
            },
            request: RequestId("detached".into()),
            call: call.clone(),
        }
    }

    /// Register and launch a tool call, returning immediately with its stable
    /// call id. A concurrency slot is acquired by the spawned work, so callers
    /// need not block their turn waiting for tool completion.
    pub async fn start<I: Into<ToolInput>>(
        &self,
        provider: Arc<dyn Provider>,
        call_id: CallId,
        name: String,
        input: I,
    ) -> Result<JobHandle, JobError> {
        self.start_for_agent(
            provider,
            AgentPath("/root".into()),
            None,
            call_id,
            name,
            input,
        )
        .await
    }

    pub async fn start_for_agent<I: Into<ToolInput>>(
        &self,
        provider: Arc<dyn Provider>,
        agent: AgentPath,
        request: Option<crate::model::RequestId>,
        call_id: CallId,
        name: String,
        input: I,
    ) -> Result<JobHandle, JobError> {
        self.start_input_for_agent(provider, agent, request, call_id, name, input.into())
            .await
    }

    /// Start a freeform custom-tool call without converting its input to JSON.
    pub async fn start_custom(
        &self,
        provider: Arc<dyn Provider>,
        call_id: CallId,
        name: String,
        input: String,
    ) -> Result<JobHandle, JobError> {
        self.start_input(provider, call_id, name, ToolInput::Custom(input))
            .await
    }

    pub async fn start_input(
        &self,
        provider: Arc<dyn Provider>,
        call_id: CallId,
        name: String,
        input: ToolInput,
    ) -> Result<JobHandle, JobError> {
        self.start_input_for_agent(
            provider,
            AgentPath("/root".into()),
            None,
            call_id,
            name,
            input,
        )
        .await
    }

    /// Typed dispatch retains the function path and raw custom bytes.
    pub async fn start_input_for_agent(
        &self,
        provider: Arc<dyn Provider>,
        agent: AgentPath,
        request: Option<RequestId>,
        call_id: CallId,
        name: String,
        input: ToolInput,
    ) -> Result<JobHandle, JobError> {
        let operation = self.detached_operation(&call_id);
        self.start_operation(provider, operation, agent, request, name, input)
            .await
    }

    /// Start an exact original operation; this is the shared Engine path.
    pub async fn start_operation<I: Into<ToolInput>>(
        &self,
        provider: Arc<dyn Provider>,
        operation: OperationId,
        agent: AgentPath,
        request: Option<RequestId>,
        name: String,
        input: I,
    ) -> Result<JobHandle, JobError> {
        self.start_operation_with_context(provider, operation, agent, request, name, input, None)
            .await
    }

    pub async fn start_operation_with_context<I: Into<ToolInput>>(
        &self,
        provider: Arc<dyn Provider>,
        operation: OperationId,
        agent: AgentPath,
        request: Option<RequestId>,
        name: String,
        input: I,
        context_snapshot: Option<crate::context::ContextSnapshot>,
    ) -> Result<JobHandle, JobError> {
        self.admit_operation(
            provider,
            operation,
            agent,
            request,
            name,
            input.into(),
            context_snapshot,
            false,
        )
        .await
    }

    /// Register the original identity without entering provider code. Claims
    /// can attach while the issuing response is still being retained.
    pub async fn queue_operation<I: Into<ToolInput>>(
        &self,
        provider: Arc<dyn Provider>,
        operation: OperationId,
        agent: AgentPath,
        request: Option<RequestId>,
        name: String,
        input: I,
    ) -> Result<JobHandle, JobError> {
        self.admit_operation(
            provider,
            operation,
            agent,
            request,
            name,
            input.into(),
            None,
            true,
        )
        .await
    }

    /// Open one admitted call after its current prefix lease has been resolved.
    pub async fn release_operation(
        &self,
        operation: &OperationId,
        context_snapshot: Option<crate::context::ContextSnapshot>,
    ) -> Result<(), JobError> {
        if context_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.operation != *operation)
        {
            return Err(JobError::OperationContextMismatch);
        }
        let mut jobs = self.jobs.lock().await;
        let job = jobs.get_mut(operation).ok_or(JobError::UnknownCall)?;
        if job.output.is_some() {
            return Ok(());
        }
        let launch = job.launch.take().ok_or(JobError::DuplicateCall)?;
        job.context_completion_required = context_snapshot.is_some();
        launch
            .send(context_snapshot)
            .map_err(|_| JobError::UnknownCall)
    }

    async fn admit_operation(
        &self,
        provider: Arc<dyn Provider>,
        operation: OperationId,
        agent: AgentPath,
        request: Option<RequestId>,
        name: String,
        input: ToolInput,
        context_snapshot: Option<crate::context::ContextSnapshot>,
        deferred: bool,
    ) -> Result<JobHandle, JobError> {
        if context_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.operation != operation)
        {
            return Err(JobError::OperationContextMismatch);
        }
        if !matches!(&operation.origin, ConversationIdentity::Standalone { store, .. } if store == &self.detached_store)
            && (operation.origin.actor() != &agent || request.as_ref() != Some(&operation.request))
        {
            return Err(JobError::OperationContextMismatch);
        }
        let call_id = operation.call.clone();
        if matches!(&input, ToolInput::Custom(_)) && crate::provider::is_harness_tool(&name) {
            return Err(JobError::AgentVerbRequiresFunction);
        }
        if matches!(&input, ToolInput::Custom(_)) && name == crate::finalize::FINALIZE_TOOL_NAME {
            return Err(JobError::FinalizeRequiresFunction);
        }
        let mut registry = self.jobs.lock().await;
        if registry.contains_key(&operation) {
            return Err(JobError::DuplicateCall);
        }
        let (settled, _) = tokio::sync::watch::channel(None);
        let handle = JobHandle(format!("job:{}", uuid::Uuid::new_v4()));
        registry.insert(
            operation.clone(),
            Job {
                handle: handle.clone(),
                started: false,
                requires_publication: deferred,
                cancellation_owner: provider.cancellation_owner(),
                cancellation_gate: Arc::new(Mutex::new(())),
                cancellation_ack: None,
                claimants: HashSet::new(),
                settled_claimants: Vec::new(),
                output: None,
                provider_completion: None,
                completion: None,
                completion_authority: crate::provider::InvocationCompletionAuthority::new(Some(
                    operation.clone(),
                )),
                context_completion_required: false,
                progress: Vec::new(),
                cancel: tokio_util::sync::CancellationToken::new(),
                settled,
                published: tokio::sync::watch::channel(false).0,
                task: None,
                launch: None,
            },
        );
        let jobs = self.jobs.clone();
        let capacity = self.capacity.clone();
        let events = self.events.clone();
        let legacy_events = self.legacy_events.clone();
        let task_operation = operation.clone();
        let task_call_id = call_id.clone();
        let task_handle = handle.clone();
        let task_agent = agent;
        let task_request = request;
        let task_cancel = registry
            .get(&operation)
            .expect("job inserted before launch")
            .cancel
            .clone();
        let task_completion = registry
            .get(&operation)
            .expect("admitted job")
            .completion_authority
            .clone();
        let verb_backend = provider.job_agent_service();
        let holds_capacity = provider.holds_job_capacity();
        let (launch, launch_gate) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            // Do not enter provider code until its JoinHandle is installed in
            // the registry. Cancellation racing registration can then always
            // abort the actual work before side effects begin.
            let context_snapshot = match launch_gate.await {
                Ok(snapshot) => snapshot,
                Err(_) => return,
            };
            let retained = provider
                .retained_output(&name, &input, &task_operation)
                .await;
            match retained {
                Ok(Some(retained)) => {
                    let output = match retained.into_parts(&task_operation) {
                        Ok((output, None, barrier, None)) if barrier.is_empty() => output,
                        Ok(_) => JobOutput::Completed(Err(
                            "retained builtin continuation requires Engine dispatch".into(),
                        )),
                        Err(error) => JobOutput::Completed(Err(error.into_tool_failure())),
                    };
                    let completion = match &output {
                        JobOutput::Completed(result) => Some(result.clone()),
                        _ => None,
                    };
                    settle(&jobs, task_operation.clone(), output, completion).await;
                    let _ = events.send(task_operation);
                    let _ = legacy_events.send(task_call_id);
                    return;
                }
                Err(error) => {
                    let result = Err(error.into_tool_failure());
                    settle(
                        &jobs,
                        task_operation.clone(),
                        JobOutput::Completed(result.clone()),
                        Some(result),
                    )
                    .await;
                    let _ = events.send(task_operation);
                    let _ = legacy_events.send(task_call_id);
                    return;
                }
                Ok(None) => {}
            }
            let permit = if holds_capacity {
                match capacity.acquire_owned().await {
                    Ok(p) => Some(p),
                    Err(_) => return,
                }
            } else {
                None
            };
            let cancelled_before_start = {
                let mut registry = jobs.lock().await;
                let job = registry.get_mut(&task_operation).expect("registered job");
                if job.output.is_some() || task_cancel.is_cancelled() {
                    true
                } else {
                    job.started = true;
                    false
                }
            };
            if cancelled_before_start {
                drop(permit);
                settle(&jobs, task_operation.clone(), JobOutput::Cancelled, None).await;
                let _ = events.send(task_operation);
                let _ = legacy_events.send(task_call_id);
                return;
            }
            let (progress, mut progress_rx) =
                tokio::sync::mpsc::channel(crate::provider::JOB_PROGRESS_CAPACITY);
            let verbs = crate::agents::JobVerbs::from_scheduler(
                verb_backend,
                task_agent.clone(),
                task_request
                    .as_ref()
                    .map(|request| crate::agents::AgentInvocation {
                        request: request.clone(),
                        call_id: task_call_id.clone(),
                    }),
                task_cancel.clone(),
                progress.clone(),
            );
            let context = CallContext {
                handle: task_handle,
                operation: Some(task_operation.clone()),
                call_id: task_call_id.clone(),
                agent: task_agent,
                request: task_request,
                cancel: task_cancel,
                verbs,
                progress,
                context: context_snapshot,
                completion: task_completion,
            };
            let call = provider.complete_call(&name, input, context);
            tokio::pin!(call);
            let mut result = loop {
                tokio::select! {
                    event = progress_rx.recv() => {
                        if let Some(event) = event {
                            if let Some(job) = jobs.lock().await.get_mut(&task_operation) {
                                retain_progress(&mut job.progress, event);
                            }
                        }
                    }
                    result = &mut call => break result,
                }
            };
            while let Ok(event) = progress_rx.try_recv() {
                if let Some(job) = jobs.lock().await.get_mut(&task_operation) {
                    retain_progress(&mut job.progress, event);
                }
            }
            drop(permit);
            result.full_success &= matches!(&result.output, JobOutput::Completed(Ok(_)));
            let output = result.output.clone();
            let provider_completion = match &output {
                JobOutput::Completed(value) => Some(value.clone()),
                _ => None,
            };
            if let Some(job) = jobs.lock().await.get_mut(&task_operation) {
                if job.completion.is_none() {
                    result.full_success &= job
                        .output
                        .as_ref()
                        .is_none_or(|output| matches!(output, JobOutput::Completed(Ok(_))));
                    job.completion = Some(result);
                }
            }
            settle(&jobs, task_operation.clone(), output, provider_completion).await;
            let _ = events.send(task_operation);
            let _ = legacy_events.send(task_call_id);
        });
        if let Some(job) = registry.get_mut(&operation) {
            job.task = Some(task);
            job.launch = Some(launch);
        } else {
            task.abort();
            return Err(JobError::UnknownCall);
        }
        drop(registry);
        if !deferred {
            self.release_operation(&operation, context_snapshot).await?;
        }
        Ok(handle)
    }

    /// Admit a tool as soon as a complete streamed function or custom call item is
    /// observed (e.g. from `response.output_item.done`). Transport readers
    /// call this from the item-done callback, not after response completion.
    /// Non-call items return `Ok(None)`; malformed calls are rejected.
    pub async fn start_completed_item(
        &self,
        provider: Arc<dyn Provider>,
        item: &Item,
    ) -> Result<Option<JobHandle>, JobError> {
        self.start_completed_item_for_agent(provider, AgentPath("/root".into()), item)
            .await
    }

    pub async fn start_completed_item_for_agent(
        &self,
        provider: Arc<dyn Provider>,
        agent: AgentPath,
        item: &Item,
    ) -> Result<Option<JobHandle>, JobError> {
        let Some(call) = item.tool_call().map_err(|_| JobError::InvalidCallItem)? else {
            return Ok(None);
        };
        self.start_input_for_agent(provider, agent, None, call.call_id, call.name, call.input)
            .await
            .map(Some)
    }

    /// Add a conversation's claim. A claim made after settlement replays the
    /// retained output immediately.
    pub async fn claim(
        &self,
        call_id: &CallId,
        claimant: AgentPath,
    ) -> Result<Option<JobOutput>, JobError> {
        let operation = self.detached_operation(call_id);
        self.claim_exact(
            &operation,
            ConversationIdentity::Standalone {
                store: self.detached_store.clone(),
                actor: claimant,
            },
        )
        .await
    }

    pub async fn claim_exact(
        &self,
        operation: &OperationId,
        claimant: ConversationIdentity,
    ) -> Result<Option<JobOutput>, JobError> {
        let mut jobs = self.jobs.lock().await;
        let job = jobs.get_mut(operation).ok_or(JobError::UnknownCall)?;
        if !job.requires_publication || *job.published.borrow() {
            if let Some(output) = &job.output {
                return Ok(Some(output.clone()));
            }
        }
        job.claimants.insert(claimant);
        Ok(None)
    }

    /// Fork a claimant. Inheriting registers the same call id; opting out
    /// yields a local typed interruption and does not affect sibling claims.
    pub async fn fork_claim(
        &self,
        call_id: &CallId,
        claimant: AgentPath,
        inherit: bool,
    ) -> Result<Option<JobOutput>, JobError> {
        let operation = self.detached_operation(call_id);
        self.fork_claim_exact(
            &operation,
            ConversationIdentity::Standalone {
                store: self.detached_store.clone(),
                actor: claimant,
            },
            inherit,
        )
        .await
    }
    pub async fn fork_claim_exact(
        &self,
        operation: &OperationId,
        claimant: ConversationIdentity,
        inherit: bool,
    ) -> Result<Option<JobOutput>, JobError> {
        if inherit {
            self.claim_exact(operation, claimant).await
        } else {
            if !self.jobs.lock().await.contains_key(operation) {
                return Err(JobError::UnknownCall);
            }
            Ok(Some(JobOutput::Interrupted))
        }
    }

    pub async fn settled_claimants(&self, call_id: &CallId) -> Result<Vec<AgentPath>, JobError> {
        Ok(self
            .settled_claimants_exact(&self.detached_operation(call_id))
            .await?
            .into_iter()
            .map(|c| c.actor().clone())
            .collect())
    }
    pub async fn settled_claimants_exact(
        &self,
        operation: &OperationId,
    ) -> Result<Vec<ConversationIdentity>, JobError> {
        Ok(self
            .jobs
            .lock()
            .await
            .get(operation)
            .ok_or(JobError::UnknownCall)?
            .settled_claimants
            .clone())
    }

    pub async fn output<K: JobKey>(&self, call_id: &K) -> Result<Option<JobOutput>, JobError> {
        let operation = call_id.operation(self);
        let jobs = self.jobs.lock().await;
        let job = jobs.get(&operation).ok_or(JobError::UnknownCall)?;
        if job.requires_publication && !*job.published.borrow() {
            return Ok(None);
        }
        Ok(job.output.clone())
    }

    /// Synchronous invocation results become visible to attached conversations
    /// only after their owning Engine publishes the terminal in Store.
    pub(crate) async fn requires_store_publication(
        &self,
        operation: &OperationId,
    ) -> Result<bool, JobError> {
        Ok(self
            .jobs
            .lock()
            .await
            .get(operation)
            .ok_or(JobError::UnknownCall)?
            .requires_publication)
    }

    pub(crate) async fn unpublished_output<K: JobKey>(
        &self,
        key: &K,
    ) -> Result<Option<JobOutput>, JobError> {
        let operation = key.operation(self);
        Ok(self
            .jobs
            .lock()
            .await
            .get(&operation)
            .ok_or(JobError::UnknownCall)?
            .output
            .clone())
    }

    /// Inspect the provider future's actual completion, even when a prior
    /// cancellation outcome remains the immutable result delivered to callers.
    pub async fn provider_completion<K: JobKey>(
        &self,
        call_id: &K,
    ) -> Result<Option<Result<Value, ToolFailure>>, JobError> {
        let call_id = call_id.operation(self);
        Ok(self
            .jobs
            .lock()
            .await
            .get(&call_id)
            .ok_or(JobError::UnknownCall)?
            .provider_completion
            .clone())
    }

    pub async fn invocation_completion<K: JobKey>(
        &self,
        key: &K,
    ) -> Result<Option<crate::provider::ProviderCompletion>, JobError> {
        let operation = key.operation(self);
        Ok(self
            .jobs
            .lock()
            .await
            .get(&operation)
            .ok_or(JobError::UnknownCall)?
            .completion
            .clone())
    }

    pub async fn progress<K: JobKey>(&self, call_id: &K) -> Result<Vec<Value>, JobError> {
        let call_id = call_id.operation(self);
        Ok(self
            .jobs
            .lock()
            .await
            .get(&call_id)
            .ok_or(JobError::UnknownCall)?
            .progress
            .clone())
    }

    /// Wait for this job's retained call output. The output remains readable
    /// after this future resolves and can therefore be delivered to every
    /// claimant on the same call id.
    pub async fn wait<K: JobKey>(&self, call_id: &K) -> Result<JobOutput, JobError> {
        let call_id = call_id.operation(self);
        loop {
            let mut settled = {
                let jobs = self.jobs.lock().await;
                let job = jobs.get(&call_id).ok_or(JobError::UnknownCall)?;
                if let Some(output) = &job.output {
                    return Ok(output.clone());
                }
                job.settled.subscribe()
            };
            if settled.changed().await.is_err() {
                return Err(JobError::UnknownCall);
            }
            if let Some(output) = settled.borrow().clone() {
                return Ok(output);
            }
        }
    }

    /// The owning Engine retained this terminal in Store. Provider completion
    /// alone cannot release an inherited synchronous invocation for inference.
    pub async fn mark_output_committed(&self, operation: &OperationId) -> Result<(), JobError> {
        let mut jobs = self.jobs.lock().await;
        let job = jobs.get_mut(operation).ok_or(JobError::UnknownCall)?;
        if job.output.is_none() {
            return Err(JobError::UnknownCall);
        }
        if !*job.published.borrow() {
            job.settled_claimants.extend(job.claimants.drain());
            job.published.send_replace(true);
            if job.requires_publication {
                let _ = self.events.send(operation.clone());
                let _ = self.legacy_events.send(operation.call.clone());
            }
        }
        Ok(())
    }

    /// Cancel an in-flight job. Cancellation is a typed terminal output; it
    /// is retained and delivered through the same claim mechanism.
    pub async fn cancel<K: JobKey>(&self, call_id: &K) -> Result<Option<JobSettlement>, JobError> {
        let call_id = call_id.operation(self);
        let gate = {
            let jobs = self.jobs.lock().await;
            jobs.get(&call_id)
                .ok_or(JobError::UnknownCall)?
                .cancellation_gate
                .clone()
        };
        let _guard = gate.lock().await;
        let handle = self
            .jobs
            .lock()
            .await
            .get(&call_id)
            .ok_or(JobError::UnknownCall)?
            .handle
            .clone();
        let owner = {
            let jobs = self.jobs.lock().await;
            let job = jobs.get(&call_id).ok_or(JobError::UnknownCall)?;
            if job.output.is_some() {
                return Ok(None);
            }
            job.cancel.cancel();
            if job.started {
                job.cancellation_owner.clone()
            } else {
                None
            }
        };
        if let Some(owner) = owner {
            {
                let jobs = self.jobs.lock().await;
                let job = jobs.get(&call_id).ok_or(JobError::UnknownCall)?;
                if job.output.is_some() {
                    return Ok(None);
                }
                job.cancel.cancel();
            }
            let ack = tokio::time::timeout(JOB_CANCELLATION_GRACE, owner.cancel(&call_id, &handle))
                .await
                .unwrap_or_else(|_| {
                    crate::provider::CancellationAcknowledgment::Unconfirmed(
                        "owner acknowledgment timed out".into(),
                    )
                });
            // The native terminal can precede the ordinary result waiter. Its
            // publication metadata must be retained before waking the Engine.
            // Projection calls the invocation owner outside the scheduler lock.
            let completion = if let crate::provider::CancellationAcknowledgment::Completed(value) =
                &ack
            {
                let (authority, required, retained) = {
                    let jobs = self.jobs.lock().await;
                    let job = jobs.get(&call_id).ok_or(JobError::UnknownCall)?;
                    (
                        job.completion_authority.clone(),
                        job.context_completion_required,
                        job.completion.clone(),
                    )
                };
                let output = JobOutput::Completed(value.clone());
                let mut completion = retained
                    .or_else(|| authority.project(output.clone()))
                    .unwrap_or_else(|| {
                        let mut completion =
                            crate::provider::ProviderCompletion::unedited(value.clone());
                        if required {
                            completion.full_success = false;
                            completion.context = crate::provider::ContextDisposition::Unavailable;
                        }
                        completion
                    });
                completion.output = output;
                completion.full_success &= value.is_ok();
                Some(completion)
            } else {
                None
            };
            let (task, settlement) = {
                let mut jobs = self.jobs.lock().await;
                let job = jobs.get_mut(&call_id).ok_or(JobError::UnknownCall)?;
                job.cancellation_ack = Some(ack.clone());
                // Completion while the owner was answering wins unchanged.
                if job.output.is_some() {
                    return Ok(None);
                }
                if let Some(completion) = completion {
                    job.completion = Some(completion);
                }
                let output = match &ack {
                    crate::provider::CancellationAcknowledgment::Stopped => JobOutput::Cancelled,
                    crate::provider::CancellationAcknowledgment::StoppedWithReceipt(receipt) => {
                        JobOutput::CancelledWithReceipt(receipt.clone())
                    }
                    crate::provider::CancellationAcknowledgment::Completed(result) => {
                        JobOutput::Completed(result.clone())
                    }
                    crate::provider::CancellationAcknowledgment::Unconfirmed(detail) => {
                        JobOutput::CancellationUnconfirmed(detail.clone())
                    }
                };
                job.output = Some(output.clone());
                job.settled_claimants = job.claimants.drain().collect();
                job.settled.send_replace(Some(output.clone()));
                let task = if matches!(
                    ack,
                    crate::provider::CancellationAcknowledgment::Stopped
                        | crate::provider::CancellationAcknowledgment::StoppedWithReceipt(_)
                ) {
                    job.task.take()
                } else {
                    None
                };
                (
                    task,
                    JobSettlement {
                        operation: call_id.clone(),
                        call_id: call_id.call.clone(),
                        output,
                        claimants: job.settled_claimants.clone(),
                    },
                )
            };
            if let Some(task) = task {
                task.abort();
                let _ = task.await;
            }
            let _ = self.events.send(call_id.clone());
            let _ = self.legacy_events.send(call_id.call.clone());
            return Ok(Some(settlement));
        }
        let (mut task, cancel, settlement) = {
            let mut jobs = self.jobs.lock().await;
            let job = jobs.get_mut(&call_id).ok_or(JobError::UnknownCall)?;
            if job.output.is_some() {
                return Ok(None);
            }
            let output = JobOutput::Cancelled;
            job.output = Some(output.clone());
            job.settled_claimants = job.claimants.drain().collect();
            job.settled.send_replace(Some(output.clone()));
            (
                job.task.take(),
                job.cancel.clone(),
                JobSettlement {
                    operation: call_id.clone(),
                    call_id: call_id.call.clone(),
                    output,
                    claimants: job.settled_claimants.clone(),
                },
            )
        };
        cancel.cancel();
        let mut publish_event = task.is_none();
        if let Some(task) = task.as_mut() {
            match tokio::time::timeout(JOB_CANCELLATION_GRACE, &mut *task).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => publish_event = true,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    publish_event = true;
                }
            }
        }
        if publish_event {
            let _ = self.events.send(call_id.clone());
            let _ = self.legacy_events.send(call_id.call.clone());
        }
        Ok(Some(settlement))
    }

    /// Ask the retained external owner again without rewriting an emitted result.
    pub async fn retry_cancellation<K: JobKey>(
        &self,
        call_id: &K,
    ) -> Result<Option<crate::provider::CancellationAcknowledgment>, JobError> {
        let call_id = call_id.operation(self);
        let (owner, gate) = {
            let jobs = self.jobs.lock().await;
            let job = jobs.get(&call_id).ok_or(JobError::UnknownCall)?;
            (
                if job.started {
                    job.cancellation_owner.clone()
                } else {
                    None
                },
                job.cancellation_gate.clone(),
            )
        };
        let Some(owner) = owner else {
            return Ok(None);
        };
        let _guard = gate.lock().await;
        let handle = self
            .jobs
            .lock()
            .await
            .get(&call_id)
            .ok_or(JobError::UnknownCall)?
            .handle
            .clone();
        {
            let jobs = self.jobs.lock().await;
            let job = jobs.get(&call_id).ok_or(JobError::UnknownCall)?;
            if matches!(job.provider_completion, Some(Ok(_))) {
                return Ok(job.cancellation_ack.clone());
            }
            // A failed result waiter is not proof the external operation ended.
            // Its owner remains available to acknowledge cleanup.
            match &job.cancellation_ack {
                None => return Ok(None),
                Some(
                    crate::provider::CancellationAcknowledgment::Stopped
                    | crate::provider::CancellationAcknowledgment::StoppedWithReceipt(_),
                ) => {
                    return Ok(job.cancellation_ack.clone());
                }
                Some(crate::provider::CancellationAcknowledgment::Completed(_)) => {
                    return Ok(job.cancellation_ack.clone());
                }
                Some(crate::provider::CancellationAcknowledgment::Unconfirmed(_)) => {}
            }
        }
        let ack = tokio::time::timeout(JOB_CANCELLATION_GRACE, owner.cancel(&call_id, &handle))
            .await
            .unwrap_or_else(|_| {
                crate::provider::CancellationAcknowledgment::Unconfirmed(
                    "owner acknowledgment timed out".into(),
                )
            });
        let task = {
            let mut jobs = self.jobs.lock().await;
            let job = jobs.get_mut(&call_id).ok_or(JobError::UnknownCall)?;
            job.cancellation_ack = Some(ack.clone());
            if matches!(
                ack,
                crate::provider::CancellationAcknowledgment::Stopped
                    | crate::provider::CancellationAcknowledgment::StoppedWithReceipt(_)
            ) && job.provider_completion.is_none()
            {
                job.task.take()
            } else {
                None
            }
        };
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        Ok(Some(ack))
    }

    /// Inspect cleanup evidence without sending another cancellation request.
    pub async fn cancellation_acknowledgment<K: JobKey>(
        &self,
        call_id: &K,
    ) -> Result<Option<crate::provider::CancellationAcknowledgment>, JobError> {
        let call_id = call_id.operation(self);
        let jobs = self.jobs.lock().await;
        Ok(jobs
            .get(&call_id)
            .ok_or(JobError::UnknownCall)?
            .cancellation_ack
            .clone())
    }

    /// Subscribe to exact operation-settlement signals.
    pub fn operation_settlements(&self) -> broadcast::Receiver<OperationId> {
        self.events.subscribe()
    }
    /// Detached local API events; shared Engine consumers use `operation_settlements`.
    pub fn settlements(&self) -> broadcast::Receiver<CallId> {
        self.legacy_events.subscribe()
    }
}

/// Read all settled tool outputs in original call order (pending calls are
/// omitted). This is the required first phase before a `wait_agent` status
/// item is appended.
pub async fn outputs_in_call_order(
    scheduler: &JobScheduler,
    calls: &[CallId],
) -> Result<Vec<(CallId, JobOutput)>, JobError> {
    let mut outputs = Vec::with_capacity(calls.len());
    for call in calls {
        if let Some(output) = scheduler.output(call).await? {
            outputs.push((call.clone(), output));
        }
    }
    Ok(outputs)
}

pub async fn outputs_in_operation_order(
    scheduler: &JobScheduler,
    operations: &[OperationId],
) -> Result<Vec<(OperationId, JobOutput)>, JobError> {
    let mut outputs = Vec::new();
    for operation in operations {
        if let Some(output) = scheduler.output(operation).await? {
            outputs.push((operation.clone(), output));
        }
    }
    Ok(outputs)
}

#[derive(Clone, Debug, PartialEq)]
pub enum WaitResumeExact {
    Job(OperationId),
    Envelope(Envelope),
    DurableWake(i64),
    TimedOut,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaitAgentResultExact {
    pub call_outputs: Vec<(OperationId, JobOutput)>,
    pub resumed_by: WaitResumeExact,
}

pub async fn wait_agent_and_drain_exact<I: Into<MailboxSignal>>(
    envelopes: &mut tokio::sync::mpsc::UnboundedReceiver<I>,
    jobs: &JobScheduler,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
    outstanding: &[OperationId],
) -> Result<WaitAgentResultExact, JobError> {
    wait_agent_and_drain_until_exact(envelopes, jobs, cancelled, outstanding, None).await
}

/// One event wait shared by indefinite waits and bounded yields. A timeout
/// only settles this wait; ownership of outstanding jobs is unchanged.
pub async fn wait_agent_and_drain_until_exact<I: Into<MailboxSignal>>(
    envelopes: &mut tokio::sync::mpsc::UnboundedReceiver<I>,
    jobs: &JobScheduler,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
    outstanding: &[OperationId],
    deadline: Option<tokio::time::Instant>,
) -> Result<WaitAgentResultExact, JobError> {
    let resumed_by = wait_until_exact(envelopes, jobs, cancelled, outstanding, deadline).await;
    let call_outputs = outputs_in_operation_order(jobs, outstanding).await?;
    // A result ready at the timer boundary wins over the timer and is drained
    // before the wait status. Exact operation identity fences other actors.
    let resumed_by = if matches!(resumed_by, WaitResumeExact::TimedOut) {
        call_outputs.first().map_or(resumed_by, |(operation, _)| {
            WaitResumeExact::Job(operation.clone())
        })
    } else {
        resumed_by
    };
    Ok(WaitAgentResultExact {
        call_outputs,
        resumed_by,
    })
}

pub async fn wait_agent_exact<I: Into<MailboxSignal>>(
    envelopes: &mut tokio::sync::mpsc::UnboundedReceiver<I>,
    jobs: &JobScheduler,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
    outstanding: &[OperationId],
) -> WaitResumeExact {
    wait_until_exact(envelopes, jobs, cancelled, outstanding, None).await
}

async fn wait_until_exact<I: Into<MailboxSignal>>(
    envelopes: &mut tokio::sync::mpsc::UnboundedReceiver<I>,
    jobs: &JobScheduler,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
    outstanding: &[OperationId],
    deadline: Option<tokio::time::Instant>,
) -> WaitResumeExact {
    if *cancelled.borrow() || cancelled.has_changed().is_err() {
        return WaitResumeExact::Cancelled;
    }
    let mut settlements = jobs.operation_settlements();
    let timer = async {
        match deadline {
            Some(deadline) => tokio::time::sleep_until(deadline).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(timer);
    // Subscribe before reading retained results to close the check/await race.
    // Recheck after a lagged broadcast because its lost event may be ours.
    let mut mailbox_open = true;
    loop {
        for operation in outstanding {
            if jobs.output(operation).await.ok().flatten().is_some() {
                return WaitResumeExact::Job(operation.clone());
            }
        }
        if *cancelled.borrow() || cancelled.has_changed().is_err() {
            return WaitResumeExact::Cancelled;
        }
        // Ready unrelated events must not postpone an expired bound. This
        // check also fences re-entry after Engine rejects a stale wake hint.
        if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
            return WaitResumeExact::TimedOut;
        }
        tokio::select! {
            biased;
            changed = cancelled.changed() => if changed.is_err() || *cancelled.borrow() {
                return WaitResumeExact::Cancelled;
            },
            event = settlements.recv() => match event {
                Ok(operation) if outstanding.contains(&operation)
                    && jobs.output(&operation).await.ok().flatten().is_some() => {
                    return WaitResumeExact::Job(operation);
                },
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return WaitResumeExact::Cancelled,
            },
            envelope = envelopes.recv(), if mailbox_open => match envelope {
                Some(envelope) => return match envelope.into() {
                    MailboxSignal::Direct(envelope) => WaitResumeExact::Envelope(envelope),
                    MailboxSignal::Durable(wake) => WaitResumeExact::DurableWake(wake.envelope_id),
                },
                None => mailbox_open = false,
            },
            _ = &mut timer => return WaitResumeExact::TimedOut,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum WaitResume {
    Job(CallId),
    Envelope(Envelope),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaitAgentResult {
    /// Must be appended as call outputs before `resumed_by`.
    pub call_outputs: Vec<(CallId, JobOutput)>,
    pub resumed_by: WaitResume,
}

pub async fn wait_agent_and_drain(
    envelopes: &mut tokio::sync::mpsc::UnboundedReceiver<Envelope>,
    jobs: &JobScheduler,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
    outstanding_calls_in_order: &[CallId],
) -> Result<WaitAgentResult, JobError> {
    let resumed_by = wait_agent(envelopes, jobs, cancelled, outstanding_calls_in_order).await;
    let call_outputs = outputs_in_call_order(jobs, outstanding_calls_in_order).await?;
    Ok(WaitAgentResult {
        call_outputs,
        resumed_by,
    })
}

/// Wait for the first asynchronous event without consuming its payload from
/// model history. After it returns, callers append `outputs_in_call_order`
/// results first and then render the returned resume status/envelope.
pub async fn wait_agent(
    envelopes: &mut tokio::sync::mpsc::UnboundedReceiver<Envelope>,
    jobs: &JobScheduler,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
    outstanding_calls_in_order: &[CallId],
) -> WaitResume {
    if *cancelled.borrow() {
        return WaitResume::Cancelled;
    }
    let mut settlements = jobs.settlements();
    // Subscribe before checking retained state so a concurrent settlement
    // cannot fall into the check/await gap.
    for call_id in outstanding_calls_in_order {
        if jobs.output(call_id).await.ok().flatten().is_some() {
            return WaitResume::Job(call_id.clone());
        }
    }
    loop {
        tokio::select! {
            envelope = envelopes.recv() => {
                if let Some(envelope) = envelope {
                    return WaitResume::Envelope(envelope);
                }
            }
            event = settlements.recv() => match event {
                Ok(call_id) if outstanding_calls_in_order.contains(&call_id)
                    && jobs.output(&call_id).await.ok().flatten().is_some() => {
                    return WaitResume::Job(call_id);
                }
                Ok(_) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {}
            },
            changed = cancelled.changed() => {
                if changed.is_err() || *cancelled.borrow() {
                    return WaitResume::Cancelled;
                }
            }
        }
    }
}

async fn settle(
    jobs: &Mutex<HashMap<OperationId, Job>>,
    operation: OperationId,
    output: JobOutput,
    provider_completion: Option<Result<Value, ToolFailure>>,
) -> JobSettlement {
    let settlement = {
        let mut jobs = jobs.lock().await;
        if let Some(job) = jobs.get_mut(&operation) {
            if let Some(completion) = provider_completion {
                job.provider_completion = Some(completion);
            }
            let final_output = if let Some(existing) = &job.output {
                existing.clone()
            } else {
                job.output = Some(output.clone());
                job.settled_claimants = job.claimants.drain().collect();
                output
            };
            JobSettlement {
                call_id: operation.call.clone(),
                operation,
                output: final_output,
                claimants: job.settled_claimants.clone(),
            }
        } else {
            JobSettlement {
                call_id: operation.call.clone(),
                operation,
                output,
                claimants: Vec::new(),
            }
        }
    };
    if let Some(job) = jobs.lock().await.get(&settlement.operation) {
        job.settled.send_replace(Some(settlement.output.clone()));
    }
    settlement
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentInvocation, AgentToolService, Contract, dispatch_agent_verb};
    use crate::provider::Provider;
    use crate::provider::ProviderError;
    use async_trait::async_trait;
    use serde_json::json;
    struct Slow;
    #[async_trait]
    impl Provider for Slow {
        async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            Ok(json!({"ok": true}))
        }
        fn tools(&self) -> Vec<Value> {
            vec![]
        }
    }

    #[tokio::test]
    async fn equal_wire_ids_from_distinct_stores_cancel_independently() {
        let first_store = crate::store::Store::memory().unwrap();
        let second_store = crate::store::Store::memory().unwrap();
        let request = RequestId("same-request".into());
        let call = CallId("same-call".into());
        for store in [&first_store, &second_store] {
            store.create_request(&request, None, "/root").unwrap();
        }
        let first = first_store.operation_for_request(&request, &call).unwrap();
        let second = second_store.operation_for_request(&request, &call).unwrap();
        assert_ne!(first, second);
        let scheduler = JobScheduler::new(2).unwrap();
        let provider: Arc<dyn Provider> = Arc::new(Slow);
        let first_handle = scheduler
            .start_operation(
                provider.clone(),
                first.clone(),
                AgentPath("/root".into()),
                Some(request.clone()),
                "slow".into(),
                json!({}),
            )
            .await
            .unwrap();
        let second_handle = scheduler
            .start_operation(
                provider,
                second.clone(),
                AgentPath("/root".into()),
                Some(request),
                "slow".into(),
                json!({}),
            )
            .await
            .unwrap();
        assert_ne!(first_handle, second_handle);
        assert!(matches!(
            scheduler
                .start_operation(
                    Arc::new(Slow),
                    first.clone(),
                    AgentPath("/root".into()),
                    Some(first.request.clone()),
                    "slow".into(),
                    json!({})
                )
                .await,
            Err(JobError::DuplicateCall)
        ));
        scheduler.cancel(&first).await.unwrap();
        assert_eq!(scheduler.wait(&first).await.unwrap(), JobOutput::Cancelled);
        assert_eq!(
            scheduler.wait(&second).await.unwrap(),
            JobOutput::Completed(Ok(json!({"ok": true})))
        );
        assert!(matches!(
            scheduler.output(&call).await,
            Err(JobError::UnknownCall)
        ));
    }

    struct RawCustomCapture(std::sync::Mutex<Vec<String>>);

    #[async_trait]
    impl Provider for RawCustomCapture {
        async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
            Ok(json!({"function": true}))
        }

        async fn call_custom_with_context(
            &self,
            _: &str,
            input: String,
            _: CallContext,
        ) -> Result<Value, ProviderError> {
            self.0.lock().unwrap().push(input.clone());
            Ok(Value::String(input))
        }

        fn tools(&self) -> Vec<Value> {
            vec![]
        }
    }

    #[tokio::test]
    async fn completed_custom_item_dispatches_raw_input_and_rejects_malformed() {
        let scheduler = JobScheduler::new(1).unwrap();
        let seen = Arc::new(RawCustomCapture(std::sync::Mutex::new(Vec::new())));
        let provider: Arc<dyn Provider> = seen.clone();
        let raw = "say \"hello\" \\\\path\n第二行 — λ 🪼";
        let item = Item(json!({
            "type": "custom_tool_call", "call_id": "custom-raw",
            "name": "cell", "input": raw
        }));
        let handle = scheduler
            .start_completed_item(provider.clone(), &item)
            .await
            .unwrap()
            .unwrap();
        assert!(handle.0.starts_with("job:"));
        assert_ne!(handle.0, "custom-raw");
        assert_eq!(
            scheduler.wait(&CallId("custom-raw".into())).await.unwrap(),
            JobOutput::Completed(Ok(Value::String(raw.into())))
        );
        assert_eq!(*seen.0.lock().unwrap(), vec![raw]);
        let malformed = Item(json!({
            "type": "custom_tool_call", "call_id": "bad", "name": "cell", "input": {"not":"raw"}
        }));
        assert!(matches!(
            scheduler
                .start_completed_item(provider.clone(), &malformed)
                .await,
            Err(JobError::InvalidCallItem)
        ));
        assert!(matches!(
            scheduler
                .start(
                    provider.clone(),
                    CallId("custom-agent-verb".into()),
                    "wait_agent".into(),
                    ToolInput::Custom("{}".into()),
                )
                .await,
            Err(JobError::AgentVerbRequiresFunction)
        ));
        assert!(matches!(
            scheduler
                .start(
                    provider,
                    CallId("custom-finalize".into()),
                    crate::finalize::FINALIZE_TOOL_NAME.into(),
                    ToolInput::Custom("{}".into()),
                )
                .await,
            Err(JobError::FinalizeRequiresFunction)
        ));
        assert!(matches!(
            scheduler.output(&CallId("custom-finalize".into())).await,
            Err(JobError::UnknownCall)
        ));
    }

    struct SpawnOnly;
    #[async_trait]
    impl AgentToolService for SpawnOnly {
        async fn spawn_agent(
            &self,
            parent: &AgentPath,
            task_name: &str,
            _: crate::agents::SpawnSource,
            _: Contract,
        ) -> Result<Value, crate::agents::AgentVerbError> {
            Ok(json!({"task_name": format!("{}/{}", parent.0, task_name)}))
        }
        async fn spawn_agent_from_invocation(
            &self,
            parent: &AgentPath,
            task_name: &str,
            _: crate::agents::SpawnSource,
            _: Contract,
            invocation: Option<&AgentInvocation>,
        ) -> Result<Value, crate::agents::AgentVerbError> {
            Ok(json!({
                "task_name": format!("{}/{}", parent.0, task_name),
                "request": invocation.map(|value| value.request.0.as_str()),
                "call_id": invocation.map(|value| value.call_id.0.as_str())
            }))
        }
        async fn send_message(
            &self,
            _: &AgentPath,
            _: AgentPath,
            _: String,
        ) -> Result<Value, crate::agents::AgentVerbError> {
            unreachable!()
        }
        async fn followup_task(
            &self,
            _: &AgentPath,
            _: AgentPath,
            _: Contract,
        ) -> Result<Value, crate::agents::AgentVerbError> {
            unreachable!()
        }
        async fn wait_agent(&self, _: &AgentPath) -> Result<Value, crate::agents::AgentVerbError> {
            unreachable!()
        }
        async fn checkpoint(
            &self,
            _: &AgentPath,
            _: String,
        ) -> Result<Value, crate::agents::AgentVerbError> {
            unreachable!()
        }
        async fn list_agents(
            &self,
            _: &AgentPath,
            _: Option<AgentPath>,
        ) -> Result<Value, crate::agents::AgentVerbError> {
            unreachable!()
        }
        async fn interrupt_agent(
            &self,
            _: &AgentPath,
            _: AgentPath,
        ) -> Result<Value, crate::agents::AgentVerbError> {
            unreachable!()
        }
    }
    #[tokio::test]
    async fn async_job_settles_for_all_claimants_and_replays() {
        let scheduler = JobScheduler::new(2).unwrap();
        let id = CallId("c1".into());
        scheduler
            .start(Arc::new(Slow), id.clone(), "slow".into(), json!({}))
            .await
            .unwrap();
        let a = AgentPath("/root/a".into());
        let b = AgentPath("/root/b".into());
        assert_eq!(scheduler.claim(&id, a.clone()).await.unwrap(), None);
        assert_eq!(scheduler.claim(&id, b.clone()).await.unwrap(), None);
        let settled = tokio::time::timeout(std::time::Duration::from_secs(1), scheduler.wait(&id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(settled, JobOutput::Completed(Ok(json!({"ok": true}))));
        assert_eq!(scheduler.settled_claimants(&id).await.unwrap().len(), 2);
        let replay = scheduler.claim(&id, a).await.unwrap();
        assert_eq!(replay, Some(JobOutput::Completed(Ok(json!({"ok": true})))));
    }

    #[tokio::test]
    async fn concurrent_start_and_cancel_never_loses_provider_task_handle() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        struct DeferredSideEffect {
            release: Arc<tokio::sync::Notify>,
            effects: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl Provider for DeferredSideEffect {
            async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
                self.release.notified().await;
                self.effects.fetch_add(1, AtomicOrdering::SeqCst);
                Ok(serde_json::json!({"side_effect":true}))
            }
            fn tools(&self) -> Vec<Value> {
                vec![]
            }
        }

        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let call_id = CallId("start-cancel-race".into());
        let release = Arc::new(tokio::sync::Notify::new());
        let effects = Arc::new(AtomicUsize::new(0));
        let provider: Arc<dyn Provider> = Arc::new(DeferredSideEffect {
            release: release.clone(),
            effects: effects.clone(),
        });
        let barrier = Arc::new(tokio::sync::Barrier::new(2));

        let start_scheduler = scheduler.clone();
        let start_barrier = barrier.clone();
        let start_id = call_id.clone();
        let start = tokio::spawn(async move {
            start_barrier.wait().await;
            start_scheduler
                .start(provider, start_id, "never".into(), serde_json::json!({}))
                .await
        });

        let cancel_scheduler = scheduler.clone();
        let cancel_barrier = barrier.clone();
        let cancel_id = call_id.clone();
        let cancel = tokio::spawn(async move {
            cancel_barrier.wait().await;
            loop {
                match cancel_scheduler.cancel(&cancel_id).await {
                    Err(JobError::UnknownCall) => tokio::task::yield_now().await,
                    result => return result,
                }
            }
        });

        assert!(start.await.unwrap().unwrap().0.starts_with("job:"));
        let settlement = cancel.await.unwrap().unwrap().unwrap();
        assert_eq!(settlement.output, JobOutput::Cancelled);
        // Releasing provider code after cancellation must never perform the
        // delayed side effect, whether cancellation caught it at the launch
        // gate or aborted it while awaiting.
        release.notify_one();
        tokio::task::yield_now().await;
        assert_eq!(effects.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(
            scheduler.output(&call_id).await.unwrap(),
            Some(JobOutput::Cancelled)
        );
    }

    #[tokio::test]
    async fn wave18_cancelled_job_rejects_late_settlement() {
        let scheduler = JobScheduler::new(1).unwrap();
        let call_id = CallId("wave18-late-completion".into());
        scheduler
            .start(Arc::new(Slow), call_id.clone(), "slow".into(), json!({}))
            .await
            .unwrap();
        assert_eq!(
            scheduler.cancel(&call_id).await.unwrap().unwrap().output,
            JobOutput::Cancelled
        );
        // Exercise the first-terminal-wins guard at the settlement boundary.
        let late = settle(
            &scheduler.jobs,
            scheduler.detached_operation(&call_id),
            JobOutput::Completed(Ok(json!({"late": true}))),
            Some(Ok(json!({"late": true}))),
        )
        .await;
        assert_eq!(late.output, JobOutput::Cancelled);
        assert_eq!(
            scheduler.output(&call_id).await.unwrap(),
            Some(JobOutput::Cancelled)
        );
    }

    #[tokio::test]
    async fn slow_streamed_tool_does_not_block_spawn_in_same_turn() {
        let scheduler = JobScheduler::new(2).unwrap();
        let provider: Arc<dyn Provider> = Arc::new(Slow);
        let item = Item(json!({
            "type":"function_call",
            "call_id":"slow-call",
            "name":"slow",
            "arguments":"{}"
        }));
        let handle = scheduler
            .start_completed_item(provider, &item)
            .await
            .unwrap()
            .expect("function call item starts a job");

        let contract = json!({
            "clauses":[], "acceptance":[], "owned":[], "must_not":[],
            "introduces":[], "consumes":[], "boundaries":[]
        });
        let spawned = dispatch_agent_verb(
            &SpawnOnly,
            &AgentPath("/root".into()),
            None,
            "spawn_agent",
            json!({
                "task_name":"child-one",
                "from":{"kind":"here","name":null},
                "task":contract
            }),
        )
        .await
        .unwrap();
        assert_eq!(spawned["task_name"], "/root/child_one");
        assert!(spawned["request"].is_null());
        let invocation = AgentInvocation {
            request: crate::model::RequestId("active-request".into()),
            call_id: CallId("spawn-call".into()),
        };
        let from_active = dispatch_agent_verb(
            &SpawnOnly,
            &AgentPath("/root".into()),
            Some(&invocation),
            "spawn_agent",
            json!({
                "task_name":"child-two",
                "from":{"kind":"here","name":null},
                "task":contract
            }),
        )
        .await
        .unwrap();
        assert_eq!(from_active["request"], "active-request");
        assert_eq!(from_active["call_id"], "spawn-call");
        assert!(handle.0.starts_with("job:"));
        assert!(
            scheduler
                .output(&CallId("slow-call".into()))
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            scheduler.wait(&CallId("slow-call".into())).await.unwrap(),
            JobOutput::Completed(Ok(_))
        ));
    }

    #[tokio::test]
    async fn settled_outputs_are_emitted_in_original_call_order() {
        struct Uneven;
        #[async_trait]
        impl Provider for Uneven {
            async fn call(&self, name: &str, _: Value) -> Result<Value, ProviderError> {
                if name == "first" {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Ok(json!({"name":name}))
            }
            fn tools(&self) -> Vec<Value> {
                vec![]
            }
        }
        let scheduler = JobScheduler::new(2).unwrap();
        let provider: Arc<dyn Provider> = Arc::new(Uneven);
        let first = CallId("call-first".into());
        let second = CallId("call-second".into());
        scheduler
            .start(provider.clone(), first.clone(), "first".into(), json!({}))
            .await
            .unwrap();
        scheduler
            .start(provider, second.clone(), "second".into(), json!({}))
            .await
            .unwrap();
        let mut events = scheduler.settlements();
        assert_eq!(events.recv().await.unwrap(), second);
        let outputs = outputs_in_call_order(&scheduler, &[first.clone(), second.clone()])
            .await
            .unwrap();
        assert_eq!(
            outputs.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            vec![&second]
        );
        assert_eq!(events.recv().await.unwrap(), first);
        let outputs = outputs_in_call_order(&scheduler, &[first.clone(), second.clone()])
            .await
            .unwrap();
        assert_eq!(
            outputs.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            vec![&first, &second]
        );
    }

    #[tokio::test]
    async fn wait_agent_resumes_on_envelope_and_preserves_envelope_value() {
        let scheduler = JobScheduler::new(1).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (_cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
        let expected = Envelope::final_answer(
            AgentPath("/root".into()),
            AgentPath("/root/child".into()),
            "done".into(),
            1,
        );
        tx.send(expected.clone()).unwrap();
        assert_eq!(
            wait_agent(&mut rx, &scheduler, &mut cancel_rx, &[]).await,
            WaitResume::Envelope(expected)
        );
    }

    #[tokio::test]
    async fn fork_can_drop_pending_claim_without_cancelling_parent() {
        let scheduler = JobScheduler::new(1).unwrap();
        let id = CallId("fork-call".into());
        scheduler
            .start(Arc::new(Slow), id.clone(), "slow".into(), json!({}))
            .await
            .unwrap();
        assert_eq!(
            scheduler
                .fork_claim(&id, AgentPath("/root/dropped".into()), false)
                .await
                .unwrap(),
            Some(JobOutput::Interrupted)
        );
        assert_eq!(
            scheduler
                .claim(&id, AgentPath("/root/parent".into()))
                .await
                .unwrap(),
            None
        );
        assert!(matches!(
            scheduler.wait(&id).await.unwrap(),
            JobOutput::Completed(Ok(_))
        ));
        assert_eq!(
            scheduler.settled_claimants(&id).await.unwrap(),
            vec![AgentPath("/root/parent".into())]
        );
    }

    #[test]
    fn request_scheduler_caps_active_and_orders_root_before_priority_ties() {
        let mut scheduler = RequestScheduler::new(1).unwrap();
        let ticket = |id: &str, depth, waiting, warm| RequestTicket {
            id: RequestId(id.into()),
            agent: AgentPath(format!("/root/{id}")),
            priority: RequestPriority {
                depth,
                operator_waiting: waiting,
                prefix_warm: warm,
            },
        };
        scheduler.enqueue(ticket("leaf", 1, true, true)).unwrap();
        scheduler.enqueue(ticket("root", 0, false, false)).unwrap();
        let first = scheduler.next_ready().unwrap();
        assert_eq!(first.id.0, "root");
        assert!(scheduler.next_ready().is_none());
        assert!(scheduler.finish(&first.id));
        assert_eq!(scheduler.next_ready().unwrap().id.0, "leaf");
    }
}

#[cfg(test)]
mod bounded_wait_tests {
    use super::*;
    use crate::mailbox::DurableMailboxWake;

    #[tokio::test]
    async fn expired_exact_wait_bound_cannot_be_starved_by_queued_wakes() {
        let jobs = JobScheduler::new(1).unwrap();
        let (_cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let (incoming, mut envelopes) = tokio::sync::mpsc::unbounded_channel();
        for envelope_id in 0..100 {
            incoming.send(DurableMailboxWake { envelope_id }).unwrap();
        }
        let result = wait_agent_and_drain_until_exact(
            &mut envelopes,
            &jobs,
            &mut cancelled,
            &[],
            Some(tokio::time::Instant::now()),
        )
        .await
        .unwrap();
        assert_eq!(result.resumed_by, WaitResumeExact::TimedOut);
        assert_eq!(
            envelopes.len(),
            100,
            "expired bound checked before wake re-entry"
        );
    }

    #[tokio::test]
    async fn bounded_exact_wait_returns_under_continuous_unrelated_settlements() {
        let jobs = JobScheduler::new(1).unwrap();
        let (_cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let (_incoming, mut envelopes) = tokio::sync::mpsc::unbounded_channel::<MailboxSignal>();
        let operation = OperationId {
            origin: ConversationIdentity::Embedded {
                run: "unrelated".into(),
                actor: AgentPath("/other".into()),
                incarnation: "one".into(),
            },
            request: RequestId("other".into()),
            call: CallId("other".into()),
        };
        let events = jobs.events.clone();
        let flood = tokio::spawn(async move {
            loop {
                for _ in 0..128 {
                    let _ = events.send(operation.clone());
                }
                tokio::task::yield_now().await;
            }
        });
        let started = tokio::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_millis(500),
            wait_agent_and_drain_until_exact(
                &mut envelopes,
                &jobs,
                &mut cancelled,
                &[],
                Some(started + Duration::from_millis(10)),
            ),
        )
        .await;
        flood.abort();
        let _ = flood.await;
        let result = result
            .expect("unrelated events must not starve deadline")
            .unwrap();
        assert_eq!(result.resumed_by, WaitResumeExact::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(10));
    }
}
