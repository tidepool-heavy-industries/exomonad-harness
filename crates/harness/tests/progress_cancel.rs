use async_trait::async_trait;
use harness::{
    model::{AgentPath, CallId},
    provider::{CallContext, JOB_PROGRESS_CAPACITY, Provider, ProviderError},
    turn::{JobOutput, JobScheduler},
};
use serde_json::{Value, json};
use tokio::sync::oneshot;

struct BarrierProvider {
    token_sender: std::sync::Mutex<Option<oneshot::Sender<tokio_util::sync::CancellationToken>>>,
    release_receiver: std::sync::Mutex<Option<oneshot::Receiver<()>>>,
}

struct CooperativeProvider {
    entered: std::sync::Mutex<Option<oneshot::Sender<()>>>,
    cleanup_started: std::sync::Mutex<Option<oneshot::Sender<()>>>,
    release_cleanup: std::sync::Mutex<Option<oneshot::Receiver<()>>>,
}

#[async_trait]
impl Provider for CooperativeProvider {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool("unexpected plain call".into()))
    }

    async fn call_with_context(
        &self,
        _: &str,
        _: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        let entered = { self.entered.lock().unwrap().take() };
        if let Some(entered) = entered {
            let _ = entered.send(());
        }
        context.cancel.cancelled().await;
        let cleanup_started = { self.cleanup_started.lock().unwrap().take() };
        if let Some(cleanup_started) = cleanup_started {
            let _ = cleanup_started.send(());
        }
        let release_cleanup = { self.release_cleanup.lock().unwrap().take() };
        if let Some(release_cleanup) = release_cleanup {
            let _ = release_cleanup.await;
        }
        Ok(json!({"cleaned_up":true}))
    }

    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

#[async_trait]
impl Provider for BarrierProvider {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool("unexpected plain call".into()))
    }
    async fn call_with_context(
        &self,
        name: &str,
        _: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        if name == "progress" {
            let mut accepted = 0usize;
            let mut rejected = 0usize;
            for n in 0..(JOB_PROGRESS_CAPACITY * 2) {
                match context.verbs.envelope(json!({"n": n})) {
                    Ok(()) => accepted += 1,
                    Err(_) => rejected += 1,
                }
            }
            return Ok(json!({"accepted":accepted,"rejected":rejected}));
        }
        if let Some(sender) = self.token_sender.lock().unwrap().take() {
            let _ = sender.send(context.cancel.clone());
        }
        let receiver = self
            .release_receiver
            .lock()
            .unwrap()
            .take()
            .expect("release barrier installed");
        let _ = receiver.await;
        Ok(json!({"late":"must not settle"}))
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

#[tokio::test]
async fn job_progress_is_bounded_and_refuses_overflow_without_blocking() {
    let jobs = JobScheduler::new(1).unwrap();
    let call_id = CallId("progress-call".into());
    jobs.start_for_agent(
        std::sync::Arc::new(BarrierProvider {
            token_sender: std::sync::Mutex::new(None),
            release_receiver: std::sync::Mutex::new(None),
        }),
        AgentPath("/root".into()),
        None,
        call_id.clone(),
        "progress".into(),
        Value::Null,
    )
    .await
    .unwrap();
    let output = jobs.wait(&call_id).await.unwrap();
    let (accepted, rejected) = match output {
        JobOutput::Completed(Ok(value)) => (
            value["accepted"].as_u64().unwrap() as usize,
            value["rejected"].as_u64().unwrap() as usize,
        ),
        other => panic!("unexpected progress job output: {other:?}"),
    };
    assert_eq!(accepted + rejected, JOB_PROGRESS_CAPACITY * 2);
    assert!(accepted <= JOB_PROGRESS_CAPACITY);
    assert!(rejected >= JOB_PROGRESS_CAPACITY);
    let retained = jobs.progress(&call_id).await.unwrap();
    assert!(retained.len() <= JOB_PROGRESS_CAPACITY);
}

#[tokio::test]
async fn cancellation_signals_provider_and_late_result_cannot_replace_cancelled() {
    let jobs = JobScheduler::new(1).unwrap();
    let call_id = CallId("cancel-call".into());
    let (token_tx, token_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    jobs.start_for_agent(
        std::sync::Arc::new(BarrierProvider {
            token_sender: std::sync::Mutex::new(Some(token_tx)),
            release_receiver: std::sync::Mutex::new(Some(release_rx)),
        }),
        AgentPath("/root".into()),
        None,
        call_id.clone(),
        "wait".into(),
        Value::Null,
    )
    .await
    .unwrap();
    let token = token_rx.await.expect("provider entered barrier");
    let settlement = jobs
        .cancel(&call_id)
        .await
        .unwrap()
        .expect("pending job cancelled");
    assert_eq!(settlement.output, JobOutput::Cancelled);
    assert!(token.is_cancelled());
    // Release the operation after cancellation (the receiver may not have
    // observed task abort yet); its late completion must never replace the
    // scheduler's retained terminal outcome.
    let _ = release_tx.send(());
    tokio::task::yield_now().await;
    assert_eq!(
        jobs.output(&call_id).await.unwrap(),
        Some(JobOutput::Cancelled)
    );
    assert!(jobs.cancel(&call_id).await.unwrap().is_none());
}

#[tokio::test]
async fn cancellation_waits_for_cooperative_provider_cleanup_before_returning() {
    let jobs = std::sync::Arc::new(JobScheduler::new(1).unwrap());
    let call_id = CallId("cooperative-cancel-call".into());
    let (entered_tx, entered_rx) = oneshot::channel();
    let (cleanup_started_tx, cleanup_started_rx) = oneshot::channel();
    let (release_cleanup_tx, release_cleanup_rx) = oneshot::channel();
    jobs.start_for_agent(
        std::sync::Arc::new(CooperativeProvider {
            entered: std::sync::Mutex::new(Some(entered_tx)),
            cleanup_started: std::sync::Mutex::new(Some(cleanup_started_tx)),
            release_cleanup: std::sync::Mutex::new(Some(release_cleanup_rx)),
        }),
        AgentPath("/root".into()),
        None,
        call_id.clone(),
        "cooperative".into(),
        Value::Null,
    )
    .await
    .unwrap();
    entered_rx.await.expect("provider entered");
    let cancel_jobs = jobs.clone();
    let cancel_id = call_id.clone();
    let cancelling = tokio::spawn(async move { cancel_jobs.cancel(&cancel_id).await });
    cleanup_started_rx
        .await
        .expect("provider observed cancellation and started cleanup");
    assert!(
        !cancelling.is_finished(),
        "cancellation must retain the provider task during cooperative cleanup"
    );
    release_cleanup_tx
        .send(())
        .expect("provider remains alive through cleanup");
    let settlement = cancelling
        .await
        .expect("cancellation task")
        .unwrap()
        .expect("pending job cancelled");
    assert_eq!(settlement.output, JobOutput::Cancelled);
    assert_eq!(
        jobs.output(&call_id).await.unwrap(),
        Some(JobOutput::Cancelled),
        "the provider's post-cleanup success is not allowed to replace cancellation"
    );
}
