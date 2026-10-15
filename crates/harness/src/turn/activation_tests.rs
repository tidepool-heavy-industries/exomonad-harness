use super::*;
use crate::provider::{
    InvocationAdmissionError as AdmissionError, InvocationRole, InvocationState,
    ProviderCompletion, ProviderError, RequestActivation,
};
use serde_json::json;
use tokio::sync::{Notify, mpsc};

struct Probe {
    activation: RequestActivation,
    contexts: mpsc::UnboundedSender<CallContext>,
    release: std::sync::Mutex<HashMap<CallId, Arc<Notify>>>,
    uncertain: bool,
}
struct UncertainOwner;
#[async_trait::async_trait]
impl crate::provider::CancellationOwner for UncertainOwner {
    async fn cancel(
        &self,
        _: &OperationId,
        _: &JobHandle,
        _: crate::provider::CancellationCause,
    ) -> crate::provider::CancellationAcknowledgment {
        crate::provider::CancellationAcknowledgment::Unconfirmed("native cleanup unknown".into())
    }
}
#[async_trait::async_trait]
impl Provider for Probe {
    fn request_activation(&self) -> Option<RequestActivation> {
        Some(self.activation.clone())
    }
    fn invocation_role(&self, name: &str) -> InvocationRole {
        match name {
            "reply" => InvocationRole::Settlement,
            "status" => InvocationRole::Observation,
            _ => InvocationRole::Domain,
        }
    }
    fn cancellation_owner(&self) -> Option<Arc<dyn crate::provider::CancellationOwner>> {
        self.uncertain
            .then(|| Arc::new(UncertainOwner) as Arc<dyn crate::provider::CancellationOwner>)
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        unreachable!()
    }
    async fn complete_call(
        &self,
        name: &str,
        _: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        let release = Arc::new(Notify::new());
        self.release
            .lock()
            .unwrap()
            .insert(context.call_id.clone(), release.clone());
        let cancel = context.cancel.clone();
        self.contexts.send(context).unwrap();
        tokio::select! { _ = release.notified() => {}, _ = cancel.cancelled() => {} }
        ProviderCompletion::unedited(Ok(json!({"done":name})))
    }
}
fn probe() -> (Arc<Probe>, mpsc::UnboundedReceiver<CallContext>) {
    probe_with_owner(false)
}
fn probe_with_owner(uncertain: bool) -> (Arc<Probe>, mpsc::UnboundedReceiver<CallContext>) {
    let (contexts, rx) = mpsc::unbounded_channel();
    (
        Arc::new(Probe {
            activation: RequestActivation::new(),
            contexts,
            release: std::sync::Mutex::new(HashMap::new()),
            uncertain,
        }),
        rx,
    )
}
async fn cleanup(s: &JobScheduler) {
    let operations = s.jobs.lock().await.keys().cloned().collect::<Vec<_>>();
    for operation in operations {
        s.cancel(&operation).await.unwrap();
    }
}
async fn start(s: &JobScheduler, p: Arc<Probe>, id: &str, name: &str) {
    s.start(p, CallId(id.into()), name.into(), json!({}))
        .await
        .unwrap();
}
async fn next(rx: &mut mpsc::UnboundedReceiver<CallContext>) -> CallContext {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap()
        .unwrap()
}
async fn queued(
    s: &JobScheduler,
    p: Arc<Probe>,
    id: &str,
    name: &str,
    request: &str,
) -> OperationId {
    let op = OperationId {
        origin: ConversationIdentity::Standalone {
            store: "activation-tests".into(),
            actor: AgentPath("/root".into()),
        },
        request: RequestId(request.into()),
        call: CallId(id.into()),
    };
    s.queue_operation(
        p,
        op.clone(),
        AgentPath("/root".into()),
        Some(op.request.clone()),
        name.into(),
        json!({}),
    )
    .await
    .unwrap();
    op
}

#[tokio::test]
async fn queued_siblings_prior_requests_and_roles_are_exact() {
    let s = JobScheduler::new(8).unwrap();
    let (a, mut rx) = probe();
    let (b, _brx) = probe();
    start(&s, a.clone(), "reply", "reply").await;
    let reply = next(&mut rx).await;
    let later = queued(&s, a.clone(), "later", "domain", "same-response").await;
    let prior = queued(&s, a.clone(), "prior", "domain", "prior-response").await;
    queued(&s, a.clone(), "observation", "status", "same-response").await;
    queued(&s, a.clone(), "settlement", "reply", "same-response").await;
    queued(&s, b, "successor", "domain", "same-response").await;
    let refused = reply.invocations.unwrap().try_admit_terminal(|inventory| {
        assert_eq!(inventory.len(), 2);
        for op in [&later, &prior] {
            assert!(inventory.iter().any(|pending| &pending.operation == op
                && pending.name == "domain"
                && pending.state == InvocationState::Queued));
        }
        Err::<(), _>("pending")
    });
    assert_eq!(refused, Err(AdmissionError::Claim("pending")));
    assert!(!a.activation.is_closed(), "refusal must leave starts open");
    queued(&s, a, "after-refusal", "domain", "same-response").await;
    cleanup(&s).await;
}

#[tokio::test]
async fn finished_unread_is_absent_and_native_finished_waiter_can_refine_running() {
    let s = JobScheduler::new(8).unwrap();
    let (p, mut rx) = probe();
    start(&s, p.clone(), "done", "domain").await;
    let done = next(&mut rx).await;
    p.release
        .lock()
        .unwrap()
        .get(&done.call_id)
        .unwrap()
        .notify_one();
    s.wait(done.operation.as_ref().unwrap()).await.unwrap();
    // No output publication or consumption occurred.
    start(&s, p.clone(), "native-done", "domain").await;
    let native_done = next(&mut rx).await;
    start(&s, p.clone(), "reply", "reply").await;
    let reply = next(&mut rx).await;
    let (_, terminal) = reply
        .invocations
        .unwrap()
        .try_admit_terminal(|inventory| {
            assert_eq!(inventory.len(), 1);
            assert_eq!(
                inventory[0].operation,
                *native_done.operation.as_ref().unwrap()
            );
            assert_eq!(inventory[0].state, InvocationState::Running);
            // Only exact native terminal evidence permits this override.
            Ok::<_, ()>(())
        })
        .unwrap();
    assert!(terminal.matches(reply.operation.as_ref().unwrap(), &p.activation));
    cleanup(&s).await;
}

#[tokio::test]
async fn unavailable_inventory_and_domain_role_never_grant() {
    let s = JobScheduler::new(4).unwrap();
    let (p, mut rx) = probe();
    start(&s, p.clone(), "reply", "reply").await;
    let admission = next(&mut rx).await.invocations.unwrap();
    {
        let _held = s.jobs.lock().await;
        assert_eq!(
            admission.try_admit_terminal(|_| Ok::<_, ()>(())),
            Err(AdmissionError::Unavailable)
        );
    }
    start(&s, p.clone(), "domain", "domain").await;
    assert_eq!(
        next(&mut rx)
            .await
            .invocations
            .unwrap()
            .try_admit_terminal(|_| Ok::<_, ()>(())),
        Err(AdmissionError::NotSettlement)
    );
    assert!(!p.activation.is_closed());
    cleanup(&s).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn claim_fences_concurrent_registration_and_preserves_successor() {
    let s = Arc::new(JobScheduler::new(4).unwrap());
    let (a, mut rx) = probe();
    start(&s, a.clone(), "reply", "reply").await;
    let context = next(&mut rx).await;
    let admission = context.invocations.unwrap();
    let entered = Arc::new(Notify::new());
    let (continue_tx, continue_rx) = std::sync::mpsc::channel();
    let claim = {
        let entered = entered.clone();
        tokio::task::spawn_blocking(move || {
            admission.try_admit_terminal(|inventory| {
                assert!(inventory.is_empty());
                entered.notify_one();
                continue_rx.recv().unwrap();
                Ok::<_, ()>(())
            })
        })
    };
    entered.notified().await;
    let late = {
        let s = s.clone();
        let a = a.clone();
        tokio::spawn(async move {
            s.start(a, CallId("late".into()), "domain".into(), json!({}))
                .await
        })
    };
    continue_tx.send(()).unwrap();
    let (_, terminal) = claim.await.unwrap().unwrap();
    assert!(matches!(
        late.await.unwrap(),
        Err(JobError::RequestActivationClosed)
    ));
    let (b, mut brx) = probe();
    start(&s, b.clone(), "next", "domain").await;
    let next_b = next(&mut brx).await;
    assert!(!terminal.matches(next_b.operation.as_ref().unwrap(), &b.activation));
    assert_ne!(
        a.activation, b.activation,
        "identical schemas do not establish identity"
    );
    cleanup(&s).await;
}

#[tokio::test]
async fn registered_domain_wins_claim_and_runs_once() {
    let s = JobScheduler::new(4).unwrap();
    let (p, mut rx) = probe();
    start(&s, p.clone(), "reply", "reply").await;
    let reply = next(&mut rx).await;
    let domain = queued(&s, p.clone(), "domain", "domain", "r").await;
    assert_eq!(
        reply.invocations.unwrap().try_admit_terminal(|inventory| {
            assert_eq!(inventory[0].operation, domain);
            Err::<(), _>("pending")
        }),
        Err(AdmissionError::Claim("pending"))
    );
    s.release_operation(&domain, None).await.unwrap();
    assert_eq!(next(&mut rx).await.operation.as_ref(), Some(&domain));
    assert!(matches!(
        s.release_operation(&domain, None).await,
        Err(JobError::DuplicateCall)
    ));
    assert!(rx.try_recv().is_err());
    cleanup(&s).await;
}

#[tokio::test]
async fn dropped_waiter_keeps_admission_and_cross_scheduler_refuses() {
    let s = JobScheduler::new(4).unwrap();
    let (p, mut rx) = probe();
    start(&s, p.clone(), "reply", "reply").await;
    let reply = next(&mut rx).await;
    let op = reply.operation.as_ref().unwrap();
    {
        let wait = s.wait(op);
        tokio::pin!(wait);
        assert!(
            tokio::time::timeout(Duration::from_millis(1), &mut wait)
                .await
                .is_err()
        );
    }
    let (_, terminal) = reply
        .invocations
        .unwrap()
        .try_admit_terminal(|_| Ok::<_, ()>(()))
        .unwrap();
    assert!(terminal.matches(op, &p.activation));
    let other = JobScheduler::new(4).unwrap();
    assert!(matches!(
        other
            .start(p, CallId("foreign".into()), "reply".into(), json!({}))
            .await,
        Err(JobError::ForeignRequestActivation)
    ));
    cleanup(&s).await;
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(32))]
    #[test]
    fn generated_inventory_histories_preserve_exact_identity_and_refusal(
        history in proptest::collection::vec((proptest::bool::ANY, 0u8..3, proptest::bool::ANY), 0..12)
    ) {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let s = JobScheduler::new(16).unwrap();
            let (a, mut arx) = probe(); let (b, mut brx) = probe();
            start(&s, a.clone(), "reply", "reply").await;
            let admission = next(&mut arx).await.invocations.unwrap();
            let mut expected = Vec::new();
            for (index, (same_activation, role, finished)) in history.into_iter().enumerate() {
                let name = ["domain", "status", "reply"][usize::from(role)];
                let provider = if same_activation { a.clone() } else { b.clone() };
                let id = format!("generated-{index}");
                let operation = queued(&s, provider.clone(), &id, name, &format!("request-{}", index % 3)).await;
                if finished {
                    s.release_operation(&operation, None).await.unwrap();
                    let context = next(if same_activation { &mut arx } else { &mut brx }).await;
                    assert_eq!(context.operation.as_ref(), Some(&operation));
                    provider.release.lock().unwrap().get(&context.call_id).unwrap().notify_one();
                    s.wait(&operation).await.unwrap();
                } else if same_activation && role == 0 {
                    expected.push(operation);
                }
            }
            let result = admission.try_admit_terminal(|inventory| {
                assert_eq!(inventory.len(), expected.len());
                for operation in &expected {
                    assert!(inventory.iter().any(|entry| &entry.operation == operation && entry.state == InvocationState::Queued));
                }
                if expected.is_empty() { Ok(()) } else { Err("pending") }
            });
            assert_eq!(result.is_ok(), expected.is_empty());
            assert_eq!(a.activation.is_closed(), expected.is_empty());
            assert!(!b.activation.is_closed());
            cleanup(&s).await;
        });
    }
}

#[tokio::test]
async fn unconfirmed_cleanup_remains_pending_after_scheduler_terminal() {
    let s = JobScheduler::new(4).unwrap();
    let (p, mut rx) = probe_with_owner(true);
    start(&s, p.clone(), "uncertain", "domain").await;
    let domain = next(&mut rx).await;
    s.cancel(domain.operation.as_ref().unwrap()).await.unwrap();
    assert!(matches!(
        s.output(domain.operation.as_ref().unwrap()).await.unwrap(),
        Some(JobOutput::CancellationUnconfirmed(_))
    ));
    start(&s, p.clone(), "reply", "reply").await;
    let reply = next(&mut rx).await;
    assert_eq!(
        reply.invocations.unwrap().try_admit_terminal(|inventory| {
            assert_eq!(inventory.len(), 1);
            assert_eq!(inventory[0].operation, *domain.operation.as_ref().unwrap());
            assert_eq!(inventory[0].state, InvocationState::Unconfirmed);
            Err::<(), _>("uncertain_cleanup")
        }),
        Err(AdmissionError::Claim("uncertain_cleanup"))
    );
    assert!(!p.activation.is_closed());
    cleanup(&s).await;
}
