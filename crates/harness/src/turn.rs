//! Asynchronous tool jobs and request scheduling primitives.
//!
//! A tool invocation is admitted independently of the model request which
//! created it. Its output is retained against the call id, and claimants can
//! replay that output (notably after a fork) without invoking the provider a
//! second time.
use crate::{
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
    sync::{Mutex, Semaphore},
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
}

impl JobScheduler {
    pub fn new(capacity: usize) -> Result<Self, JobError> {
        if capacity == 0 {
            return Err(JobError::ZeroCapacity);
        }
        Ok(Self {
            capacity: Arc::new(Semaphore::new(capacity)),
            jobs: Arc::new(Mutex::new(HashMap::new())),
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
        let mut jobs = self.jobs.lock().await;
        if jobs.contains_key(&call_id) {
            return Err(JobError::DuplicateCall);
        }
        let (settled, _) = tokio::sync::watch::channel(None);
        jobs.insert(
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
        drop(jobs);

        let jobs = self.jobs.clone();
        let capacity = self.capacity.clone();
        let task_call_id = call_id.clone();
        let task = tokio::spawn(async move {
            let permit = match capacity.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
            let (progress, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
            let context = CallContext {
                handle: JobHandle(task_call_id.0.clone()),
                call_id: task_call_id.clone(),
                progress,
            };
            let call = provider.call_with_context(&name, args, context);
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
            settle(&jobs, task_call_id, JobOutput::Completed(result)).await;
        });
        self.jobs
            .lock()
            .await
            .get_mut(&call_id)
            .expect("registered above")
            .task = Some(task);
        Ok(JobHandle(call_id.0))
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
        Ok(Some(
            settle(&self.jobs, call_id.clone(), JobOutput::Cancelled).await,
        ))
    }
}

async fn settle(
    jobs: &Mutex<HashMap<CallId, Job>>,
    call_id: CallId,
    output: JobOutput,
) -> JobSettlement {
    let settlement = {
        let mut jobs = jobs.lock().await;
        let job = jobs
            .get_mut(&call_id)
            .expect("job registered before execution");
        if job.output.is_none() {
            job.output = Some(output);
            job.settled_claimants = job.claimants.drain().collect();
        }
        JobSettlement {
            call_id,
            output: job.output.clone().expect("just settled"),
            claimants: job.settled_claimants.clone(),
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
