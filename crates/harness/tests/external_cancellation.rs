use async_trait::async_trait;
use harness::{
    model::CallId,
    provider::{CancellationAcknowledgment, CancellationOwner, JobHandle, Provider, ProviderError},
    turn::{JobOutput, JobScheduler},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::Notify;

struct External {
    started: Notify,
    release: Notify,
    dropped: Arc<AtomicBool>,
    stopped: AtomicBool,
}
struct DropMark(Arc<AtomicBool>);
impl Drop for DropMark {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
#[async_trait]
impl CancellationOwner for External {
    async fn cancel(&self, _: &JobHandle) -> CancellationAcknowledgment {
        if self.stopped.load(Ordering::SeqCst) {
            CancellationAcknowledgment::Stopped
        } else {
            CancellationAcknowledgment::Unconfirmed("external operation still running".into())
        }
    }
}
struct ExternalProvider(Arc<External>);
#[async_trait]
impl Provider for ExternalProvider {
    fn cancellation_owner(&self) -> Option<Arc<dyn CancellationOwner>> {
        Some(self.0.clone())
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        let _drop = DropMark(self.0.dropped.clone());
        self.0.started.notify_one();
        self.0.release.notified().await;
        Ok(json!("completed"))
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}
fn external() -> Arc<External> {
    Arc::new(External {
        started: Notify::new(),
        release: Notify::new(),
        dropped: Arc::new(AtomicBool::new(false)),
        stopped: AtomicBool::new(false),
    })
}

#[tokio::test]
async fn uncertain_external_cancellation_retains_waiter_until_owner_acknowledges() {
    let jobs = JobScheduler::new(1).unwrap();
    let owner = external();
    let call = CallId("external".into());
    jobs.start(
        Arc::new(ExternalProvider(owner.clone())),
        call.clone(),
        "run".into(),
        json!({}),
    )
    .await
    .unwrap();
    owner.started.notified().await;
    assert_eq!(jobs.retry_cancellation(&call).await.unwrap(), None);
    assert_eq!(jobs.cancellation_acknowledgment(&call).await.unwrap(), None);
    assert!(!owner.dropped.load(Ordering::SeqCst));
    let settlement = jobs.cancel(&call).await.unwrap().unwrap();
    assert!(matches!(
        jobs.cancellation_acknowledgment(&call).await.unwrap(),
        Some(CancellationAcknowledgment::Unconfirmed(_))
    ));
    assert!(matches!(
        settlement.output,
        JobOutput::CancellationUnconfirmed(_)
    ));
    assert!(
        !owner.dropped.load(Ordering::SeqCst),
        "uncertainty cannot destroy the only waiter"
    );
    owner.stopped.store(true, Ordering::SeqCst);
    assert_eq!(
        jobs.retry_cancellation(&call).await.unwrap(),
        Some(CancellationAcknowledgment::Stopped)
    );
    assert!(owner.dropped.load(Ordering::SeqCst));
    assert_eq!(
        jobs.output(&call).await.unwrap(),
        Some(settlement.output),
        "reconciliation does not rewrite an already emitted outcome"
    );
}

#[tokio::test]
async fn completed_external_work_wins_over_later_cancel() {
    let jobs = JobScheduler::new(1).unwrap();
    let owner = external();
    let call = CallId("completed".into());
    jobs.start(
        Arc::new(ExternalProvider(owner.clone())),
        call.clone(),
        "run".into(),
        json!({}),
    )
    .await
    .unwrap();
    owner.started.notified().await;
    owner.release.notify_one();
    assert!(matches!(
        jobs.wait(&call).await.unwrap(),
        JobOutput::Completed(Ok(_))
    ));
    assert!(jobs.cancel(&call).await.unwrap().is_none());
}

#[tokio::test]
async fn queued_external_call_is_cancelled_before_admission() {
    let jobs = JobScheduler::new(1).unwrap();
    let first = external();
    jobs.start(
        Arc::new(ExternalProvider(first.clone())),
        CallId("first".into()),
        "run".into(),
        json!({}),
    )
    .await
    .unwrap();
    first.started.notified().await;
    let second = external();
    let queued = CallId("queued".into());
    jobs.start(
        Arc::new(ExternalProvider(second.clone())),
        queued.clone(),
        "run".into(),
        json!({}),
    )
    .await
    .unwrap();
    assert_eq!(
        jobs.cancel(&queued).await.unwrap().unwrap().output,
        JobOutput::Cancelled
    );
    assert!(
        !second.dropped.load(Ordering::SeqCst),
        "provider was never entered"
    );
    first.release.notify_one();
    jobs.wait(&CallId("first".into())).await.unwrap();
}
