use async_trait::async_trait;
use harness::{
    item::ToolInput,
    model::{AgentPath, CallId, ConversationIdentity, OperationId, RequestId},
    provider::{
        CallContext, CancellationAcknowledgment, CancellationOwner, ContextDisposition, JobHandle,
        Provider, ProviderCompletion, ProviderError, ToolFailure,
    },
    turn::{JobOutput, JobScheduler},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio::sync::{Mutex, Notify, oneshot};

struct External {
    started: Notify,
    release: Notify,
    dropped: Arc<AtomicBool>,
    stopped: AtomicBool,
    fail_waiter: AtomicBool,
    cancel_calls: std::sync::atomic::AtomicUsize,
    cancel_entered: Notify,
    cancel_barrier: Mutex<Option<oneshot::Receiver<()>>>,
}
struct DropMark(Arc<AtomicBool>);
impl Drop for DropMark {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
#[async_trait]
impl CancellationOwner for External {
    async fn cancel(&self, _: &OperationId, _: &JobHandle) -> CancellationAcknowledgment {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        self.cancel_entered.notify_one();
        let barrier = self.cancel_barrier.lock().await.take();
        if let Some(barrier) = barrier {
            let _ = barrier.await;
        }
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
        if self.0.fail_waiter.load(Ordering::SeqCst) {
            Err(ProviderError::Tool("result waiter disconnected".into()))
        } else {
            Ok(json!("completed"))
        }
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
        fail_waiter: AtomicBool::new(false),
        cancel_calls: std::sync::atomic::AtomicUsize::new(0),
        cancel_entered: Notify::new(),
        cancel_barrier: Mutex::new(None),
    })
}

struct StoppedWithReceipt {
    started: Notify,
    dropped: Arc<AtomicBool>,
    receipt: Result<Value, ToolFailure>,
}

#[async_trait]
impl CancellationOwner for StoppedWithReceipt {
    async fn cancel(&self, _: &OperationId, _: &JobHandle) -> CancellationAcknowledgment {
        CancellationAcknowledgment::StoppedWithReceipt(self.receipt.clone())
    }
}

struct ReceiptProvider(Arc<StoppedWithReceipt>);

#[async_trait]
impl Provider for ReceiptProvider {
    fn cancellation_owner(&self) -> Option<Arc<dyn CancellationOwner>> {
        Some(self.0.clone())
    }

    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        let _drop = DropMark(self.0.dropped.clone());
        self.0.started.notify_one();
        std::future::pending().await
    }

    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

#[tokio::test]
async fn stopped_owner_receipt_survives_waiter_abort_without_becoming_completion() {
    for receipt in [
        Ok(json!({"items":[{"status":"committed","output":"performed prefix"}],"nextIndex":1})),
        Err(ToolFailure::with_metadata(
            "cancelled after performed prefix",
            json!({"class":"interrupted","phase":"run"}),
        )),
    ] {
        let jobs = JobScheduler::new(1).unwrap();
        let owner = Arc::new(StoppedWithReceipt {
            started: Notify::new(),
            dropped: Arc::new(AtomicBool::new(false)),
            receipt: receipt.clone(),
        });
        let call = CallId("cancelled-with-prefix".into());
        jobs.start(
            Arc::new(ReceiptProvider(owner.clone())),
            call.clone(),
            "run".into(),
            json!({}),
        )
        .await
        .unwrap();
        owner.started.notified().await;
        let settlement = jobs.cancel(&call).await.unwrap().unwrap();
        let expected = JobOutput::CancelledWithReceipt(receipt.clone());
        assert_eq!(settlement.call_id, call);
        assert_eq!(settlement.output, expected);
        assert_eq!(jobs.wait(&call).await.unwrap(), expected);
        assert_eq!(jobs.provider_completion(&call).await.unwrap(), None);
        assert!(owner.dropped.load(Ordering::SeqCst));
        assert_eq!(jobs.cancel(&call).await.unwrap(), None);
        assert_eq!(
            jobs.retry_cancellation(&call).await.unwrap(),
            Some(CancellationAcknowledgment::StoppedWithReceipt(receipt))
        );
    }
}

struct ReplyBeforeCancelAcknowledgment {
    started: Notify,
    release_reply: Notify,
    cancel_entered: Notify,
    release_ack: Notify,
    receipt: Result<Value, ToolFailure>,
}

#[async_trait]
impl CancellationOwner for ReplyBeforeCancelAcknowledgment {
    async fn cancel(&self, _: &OperationId, _: &JobHandle) -> CancellationAcknowledgment {
        self.cancel_entered.notify_one();
        self.release_ack.notified().await;
        CancellationAcknowledgment::StoppedWithReceipt(self.receipt.clone())
    }
}

struct TypedCancelledProvider(Arc<ReplyBeforeCancelAcknowledgment>);

#[async_trait]
impl Provider for TypedCancelledProvider {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        unreachable!("scheduler must use typed complete_call")
    }

    fn cancellation_owner(&self) -> Option<Arc<dyn CancellationOwner>> {
        Some(self.0.clone())
    }

    async fn complete_call(&self, _: &str, _: ToolInput, _: CallContext) -> ProviderCompletion {
        self.0.started.notify_one();
        self.0.release_reply.notified().await;
        ProviderCompletion {
            output: JobOutput::CancelledWithReceipt(self.0.receipt.clone()),
            // Even a misreported success flag cannot make a cancellation eligible.
            full_success: true,
            context: ContextDisposition::Unedited,
        }
    }

    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

#[tokio::test]
async fn typed_cancelled_reply_precedes_owner_ack_without_becoming_completion() {
    for queued in [false, true] {
        for receipt in [
            Ok(json!({"items":[{"status":"committed","output":"performed prefix"}],"nextIndex":1})),
            Err(ToolFailure::with_metadata(
                "partial result",
                json!({"phase":"run"}),
            )),
        ] {
            let jobs = Arc::new(JobScheduler::new(1).unwrap());
            let owner = Arc::new(ReplyBeforeCancelAcknowledgment {
                started: Notify::new(),
                release_reply: Notify::new(),
                cancel_entered: Notify::new(),
                release_ack: Notify::new(),
                receipt: receipt.clone(),
            });
            let agent = AgentPath("/root".into());
            let operation = OperationId {
                origin: ConversationIdentity::Standalone {
                    store: "typed-cancellation-test".into(),
                    actor: agent.clone(),
                },
                request: RequestId("issuing-response".into()),
                call: CallId("reply-before-ack".into()),
            };
            let provider = Arc::new(TypedCancelledProvider(owner.clone()));
            if queued {
                jobs.queue_operation(
                    provider,
                    operation.clone(),
                    agent,
                    Some(operation.request.clone()),
                    "run".into(),
                    json!({}),
                )
                .await
                .unwrap();
                jobs.release_operation(&operation, None).await.unwrap();
            } else {
                jobs.start_operation(
                    provider,
                    operation.clone(),
                    agent,
                    Some(operation.request.clone()),
                    "run".into(),
                    json!({}),
                )
                .await
                .unwrap();
            }
            owner.started.notified().await;
            let mut settlements = jobs.operation_settlements();
            let cancel_jobs = jobs.clone();
            let cancel_operation = operation.clone();
            let cancel = tokio::spawn(async move { cancel_jobs.cancel(&cancel_operation).await });
            owner.cancel_entered.notified().await;
            owner.release_reply.notify_one();
            assert_eq!(settlements.recv().await.unwrap(), operation);
            let expected = JobOutput::CancelledWithReceipt(receipt.clone());
            assert_eq!(jobs.wait(&operation).await.unwrap(), expected);
            assert_eq!(jobs.provider_completion(&operation).await.unwrap(), None);
            let completion = jobs
                .invocation_completion(&operation)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(completion.output, expected);
            assert!(!completion.full_success);
            if queued {
                assert_eq!(jobs.output(&operation).await.unwrap(), None);
                jobs.mark_output_committed(&operation).await.unwrap();
            }
            assert_eq!(
                jobs.output(&operation).await.unwrap(),
                Some(expected.clone())
            );
            owner.release_ack.notify_one();
            assert_eq!(cancel.await.unwrap().unwrap(), None);
            assert_eq!(jobs.output(&operation).await.unwrap(), Some(expected));
            assert_eq!(
                jobs.cancellation_acknowledgment(&operation).await.unwrap(),
                Some(CancellationAcknowledgment::StoppedWithReceipt(receipt)),
            );
        }
    }
}

struct CompletedDuringCancel {
    started: Notify,
    release_waiter: Notify,
}

#[async_trait]
impl CancellationOwner for CompletedDuringCancel {
    async fn cancel(&self, _: &OperationId, _: &JobHandle) -> CancellationAcknowledgment {
        CancellationAcknowledgment::Completed(Ok(json!({"terminal": "success"})))
    }
}

struct CompletedProvider(Arc<CompletedDuringCancel>);

#[async_trait]
impl Provider for CompletedProvider {
    fn cancellation_owner(&self) -> Option<Arc<dyn CancellationOwner>> {
        Some(self.0.clone())
    }

    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        self.0.started.notify_one();
        self.0.release_waiter.notified().await;
        Ok(json!({"waiter": "late"}))
    }

    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

#[tokio::test]
async fn owner_completed_reply_wins_before_provider_waiter_resumes() {
    let jobs = JobScheduler::new(1).unwrap();
    let owner = Arc::new(CompletedDuringCancel {
        started: Notify::new(),
        release_waiter: Notify::new(),
    });
    let call = CallId("completed-at-owner".into());
    jobs.start(
        Arc::new(CompletedProvider(owner.clone())),
        call.clone(),
        "run".into(),
        json!({}),
    )
    .await
    .unwrap();
    owner.started.notified().await;
    let settlement = jobs.cancel(&call).await.unwrap().unwrap();
    let completed = JobOutput::Completed(Ok(json!({"terminal": "success"})));
    assert_eq!(settlement.output, completed);
    assert_eq!(jobs.output(&call).await.unwrap(), Some(completed.clone()));
    owner.release_waiter.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if jobs.provider_completion(&call).await.unwrap().is_some() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(jobs.output(&call).await.unwrap(), Some(completed));
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
async fn late_completion_is_retained_separately_and_stops_cancellation_retries() {
    let jobs = JobScheduler::new(1).unwrap();
    let owner = external();
    let call = CallId("late-completion".into());
    jobs.start(
        Arc::new(ExternalProvider(owner.clone())),
        call.clone(),
        "run".into(),
        json!({}),
    )
    .await
    .unwrap();
    owner.started.notified().await;
    let mut settlements = jobs.settlements();

    let cancellation = jobs.cancel(&call).await.unwrap().unwrap();
    assert!(matches!(
        cancellation.output,
        JobOutput::CancellationUnconfirmed(_)
    ));
    assert_eq!(settlements.recv().await.unwrap(), call);

    owner.release.notify_one();
    assert_eq!(settlements.recv().await.unwrap(), call);
    assert_eq!(
        jobs.provider_completion(&call).await.unwrap(),
        Some(Ok(json!("completed")))
    );
    assert_eq!(
        jobs.output(&call).await.unwrap(),
        Some(cancellation.output.clone()),
        "late completion is evidence, not a rewrite of the emitted result"
    );

    let cancel_calls = owner.cancel_calls.load(Ordering::SeqCst);
    assert!(matches!(
        jobs.retry_cancellation(&call).await.unwrap(),
        Some(CancellationAcknowledgment::Unconfirmed(_))
    ));
    assert_eq!(
        owner.cancel_calls.load(Ordering::SeqCst),
        cancel_calls,
        "known provider completion makes another owner cancellation stale"
    );
}

#[tokio::test]
async fn completion_wins_while_owner_acknowledgment_is_pending() {
    let jobs = Arc::new(JobScheduler::new(1).unwrap());
    let owner = external();
    let (ack_tx, ack_rx) = oneshot::channel();
    *owner.cancel_barrier.lock().await = Some(ack_rx);
    owner.stopped.store(true, Ordering::SeqCst);
    let call = CallId("completion-during-cancel".into());
    jobs.start(
        Arc::new(ExternalProvider(owner.clone())),
        call.clone(),
        "run".into(),
        json!({}),
    )
    .await
    .unwrap();
    owner.started.notified().await;
    let mut settlements = jobs.settlements();

    let cancel_jobs = jobs.clone();
    let cancel_call = call.clone();
    let cancel = tokio::spawn(async move { cancel_jobs.cancel(&cancel_call).await });
    owner.cancel_entered.notified().await;
    owner.release.notify_one();
    assert_eq!(settlements.recv().await.unwrap(), call);
    ack_tx.send(()).unwrap();

    assert!(cancel.await.unwrap().unwrap().is_none());
    assert_eq!(
        jobs.output(&call).await.unwrap(),
        Some(JobOutput::Completed(Ok(json!("completed"))))
    );
    assert_eq!(
        jobs.provider_completion(&call).await.unwrap(),
        Some(Ok(json!("completed")))
    );
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

#[tokio::test]
async fn failed_result_waiter_does_not_remove_external_cleanup_authority() {
    let jobs = JobScheduler::new(1).unwrap();
    let owner = external();
    owner.fail_waiter.store(true, Ordering::SeqCst);
    let call = CallId("lost-waiter".into());
    jobs.start(
        Arc::new(ExternalProvider(owner.clone())),
        call.clone(),
        "run".into(),
        json!({}),
    )
    .await
    .unwrap();
    owner.started.notified().await;
    let result = jobs.cancel(&call).await.unwrap().unwrap();
    assert!(matches!(
        result.output,
        JobOutput::CancellationUnconfirmed(_)
    ));
    let mut events = jobs.settlements();
    owner.release.notify_one();
    assert_eq!(events.recv().await.unwrap(), call);
    assert!(matches!(
        jobs.provider_completion(&call).await.unwrap(),
        Some(Err(_))
    ));
    owner.stopped.store(true, Ordering::SeqCst);
    assert_eq!(
        jobs.retry_cancellation(&call).await.unwrap(),
        Some(CancellationAcknowledgment::Stopped)
    );
    assert_eq!(jobs.output(&call).await.unwrap(), Some(result.output));
}
