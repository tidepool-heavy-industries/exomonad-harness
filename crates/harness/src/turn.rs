//! Asynchronous tool jobs and request scheduling primitives.
//!
//! A tool invocation is admitted independently of the model request which
//! created it. Its output is retained against the call id, and claimants can
//! replay that output (notably after a fork) without invoking the provider a
//! second time.
use crate::{
    item::Item,
    mailbox::Envelope,
    model::{AgentPath, CallId, RequestId},
    provider::{CallContext, JobHandle, Provider, ProviderError},
};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::Arc,
};
use thiserror::Error;
use tokio::{
    sync::{Mutex, Semaphore, broadcast},
    task::JoinHandle,
};

#[derive(Clone, Debug, PartialEq)]
pub enum JobOutput {
    Completed(Result<Value, String>),
    Cancelled,
    Interrupted,
}

#[derive(Clone, Debug, PartialEq)]
pub struct JobSettlement {
    pub call_id: CallId,
    pub output: JobOutput,
    /// Conversations whose outstanding call is made ready by this settlement.
    pub claimants: Vec<AgentPath>,
}

#[derive(Debug, Error)]
pub enum JobError {
    #[error("job capacity must be greater than zero")]
    ZeroCapacity,
    #[error("call id is already registered")]
    DuplicateCall,
    #[error("unknown call id")]
    UnknownCall,
    #[error("completed function call item has invalid fields")]
    InvalidCallItem,
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
    claimants: HashSet<AgentPath>,
    settled_claimants: Vec<AgentPath>,
    output: Option<JobOutput>,
    progress: Vec<Value>,
    settled: tokio::sync::watch::Sender<Option<JobOutput>>,
    task: Option<JoinHandle<()>>,
}

/// Owns in-flight provider work and makes settlement replayable.
///
/// The semaphore bounds concurrent calls. The same registry can be shared by
/// the request loop and the agent tree; it does not own durable persistence.
pub struct JobScheduler {
    capacity: Arc<Semaphore>,
    jobs: Arc<Mutex<HashMap<CallId, Job>>>,
    events: broadcast::Sender<CallId>,
}

impl JobScheduler {
    pub fn new(capacity: usize) -> Result<Self, JobError> {
        if capacity == 0 {
            return Err(JobError::ZeroCapacity);
        }
        let (events, _) = broadcast::channel(64);
        Ok(Self {
            capacity: Arc::new(Semaphore::new(capacity)),
            jobs: Arc::new(Mutex::new(HashMap::new())),
            events,
        })
    }

    /// Register and launch a tool call, returning immediately with its stable
    /// call id. A concurrency slot is acquired by the spawned work, so callers
    /// need not block their turn waiting for tool completion.
    pub async fn start(
        &self,
        provider: Arc<dyn Provider>,
        call_id: CallId,
        name: String,
        args: Value,
    ) -> Result<JobHandle, JobError> {
        self.start_for_agent(provider, AgentPath("/root".into()), call_id, name, args)
            .await
    }

    pub async fn start_for_agent(
        &self,
        provider: Arc<dyn Provider>,
        agent: AgentPath,
        call_id: CallId,
        name: String,
        args: Value,
    ) -> Result<JobHandle, JobError> {
        let mut registry = self.jobs.lock().await;
        if registry.contains_key(&call_id) {
            return Err(JobError::DuplicateCall);
        }
        let (settled, _) = tokio::sync::watch::channel(None);
        registry.insert(
            call_id.clone(),
            Job {
                claimants: HashSet::new(),
                settled_claimants: Vec::new(),
                output: None,
                progress: Vec::new(),
                settled,
                task: None,
            },
        );
        let jobs = self.jobs.clone();
        let capacity = self.capacity.clone();
        let events = self.events.clone();
        let task_call_id = call_id.clone();
        let task_agent = agent;
        let is_agent_verb = crate::provider::is_harness_tool(&name);
        let (launch, launch_gate) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            // Do not enter provider code until its JoinHandle is installed in
            // the registry. Cancellation racing registration can then always
            // abort the actual work before side effects begin.
            if launch_gate.await.is_err() {
                return;
            }
            let permit = match capacity.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
            let (progress, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
            let context = CallContext {
                handle: JobHandle(task_call_id.0.clone()),
                call_id: task_call_id.clone(),
                agent: task_agent,
                progress,
            };
            let call = if is_agent_verb {
                provider.call_agent_verb(&name, args, context)
            } else {
                provider.call_with_context(&name, args, context)
            };
            tokio::pin!(call);
            let result = loop {
                tokio::select! {
                    event = progress_rx.recv() => {
                        if let Some(event) = event {
                            if let Some(job) = jobs.lock().await.get_mut(&task_call_id) {
                                job.progress.push(event);
                            }
                        }
                    }
                    result = &mut call => break result.map_err(|e: ProviderError| e.to_string()),
                }
            };
            while let Ok(event) = progress_rx.try_recv() {
                if let Some(job) = jobs.lock().await.get_mut(&task_call_id) {
                    job.progress.push(event);
                }
            }
            drop(permit);
            settle(&jobs, task_call_id.clone(), JobOutput::Completed(result)).await;
            let _ = events.send(task_call_id);
        });
        if let Some(job) = registry.get_mut(&call_id) {
            job.task = Some(task);
        } else {
            task.abort();
            return Err(JobError::UnknownCall);
        }
        drop(registry);
        let _ = launch.send(());
        Ok(JobHandle(call_id.0))
    }

    /// Admit a tool as soon as a complete streamed `function_call` item is
    /// observed (e.g. from `response.output_item.done`). Transport readers
    /// call this from the item-done callback, not after response completion.
    /// Non-function items return `Ok(None)`.
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
        let value = &item.0;
        if value.get("type").and_then(Value::as_str) != Some("function_call") {
            return Ok(None);
        }
        let call_id = value
            .get("call_id")
            .and_then(Value::as_str)
            .ok_or(JobError::InvalidCallItem)?;
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .ok_or(JobError::InvalidCallItem)?;
        let args = match value.get("arguments") {
            Some(Value::String(raw)) => {
                serde_json::from_str(raw).map_err(|_| JobError::InvalidCallItem)?
            }
            Some(value) => value.clone(),
            None => return Err(JobError::InvalidCallItem),
        };
        self.start_for_agent(
            provider,
            agent,
            CallId(call_id.to_owned()),
            name.to_owned(),
            args,
        )
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
        let mut jobs = self.jobs.lock().await;
        let job = jobs.get_mut(call_id).ok_or(JobError::UnknownCall)?;
        if let Some(output) = &job.output {
            return Ok(Some(output.clone()));
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
        if inherit {
            self.claim(call_id, claimant).await
        } else {
            if !self.jobs.lock().await.contains_key(call_id) {
                return Err(JobError::UnknownCall);
            }
            Ok(Some(JobOutput::Interrupted))
        }
    }

    pub async fn settled_claimants(&self, call_id: &CallId) -> Result<Vec<AgentPath>, JobError> {
        Ok(self
            .jobs
            .lock()
            .await
            .get(call_id)
            .ok_or(JobError::UnknownCall)?
            .settled_claimants
            .clone())
    }

    pub async fn output(&self, call_id: &CallId) -> Result<Option<JobOutput>, JobError> {
        Ok(self
            .jobs
            .lock()
            .await
            .get(call_id)
            .ok_or(JobError::UnknownCall)?
            .output
            .clone())
    }

    pub async fn progress(&self, call_id: &CallId) -> Result<Vec<Value>, JobError> {
        Ok(self
            .jobs
            .lock()
            .await
            .get(call_id)
            .ok_or(JobError::UnknownCall)?
            .progress
            .clone())
    }

    /// Wait for this job's retained call output. The output remains readable
    /// after this future resolves and can therefore be delivered to every
    /// claimant on the same call id.
    pub async fn wait(&self, call_id: &CallId) -> Result<JobOutput, JobError> {
        loop {
            let mut settled = {
                let jobs = self.jobs.lock().await;
                let job = jobs.get(call_id).ok_or(JobError::UnknownCall)?;
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

    /// Cancel an in-flight job. Cancellation is a typed terminal output; it
    /// is retained and delivered through the same claim mechanism.
    pub async fn cancel(&self, call_id: &CallId) -> Result<Option<JobSettlement>, JobError> {
        let (task, was_pending) = {
            let mut jobs = self.jobs.lock().await;
            let job = jobs.get_mut(call_id).ok_or(JobError::UnknownCall)?;
            if job.output.is_some() {
                return Ok(None);
            }
            (job.task.take(), true)
        };
        if !was_pending {
            return Ok(None);
        }
        if let Some(task) = task {
            task.abort();
        }
        let settlement = settle(&self.jobs, call_id.clone(), JobOutput::Cancelled).await;
        let _ = self.events.send(call_id.clone());
        Ok(Some(settlement))
    }

    /// Subscribe to job-settlement signals for wait-agent coordination.
    /// Consumers still read the authoritative retained output by call id.
    pub fn settlements(&self) -> broadcast::Receiver<CallId> {
        self.events.subscribe()
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
                Ok(call_id) => return WaitResume::Job(call_id),
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
    jobs: &Mutex<HashMap<CallId, Job>>,
    call_id: CallId,
    output: JobOutput,
) -> JobSettlement {
    let settlement = {
        let mut jobs = jobs.lock().await;
        if let Some(job) = jobs.get_mut(&call_id) {
            let final_output = if let Some(existing) = &job.output {
                existing.clone()
            } else {
                job.output = Some(output.clone());
                job.settled_claimants = job.claimants.drain().collect();
                output
            };
            JobSettlement {
                call_id,
                output: final_output,
                claimants: job.settled_claimants.clone(),
            }
        } else {
            JobSettlement {
                call_id,
                output,
                claimants: Vec::new(),
            }
        }
    };
    if let Some(job) = jobs.lock().await.get(&settlement.call_id) {
        job.settled.send_replace(Some(settlement.output.clone()));
    }
    settlement
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{AgentToolService, Contract, dispatch_agent_verb};
    use crate::provider::Provider;
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

        assert_eq!(start.await.unwrap().unwrap().0, call_id.0);
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
        assert_eq!(handle.0, "slow-call");
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
