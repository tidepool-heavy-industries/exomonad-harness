use super::*;
use crate::provider::{CancellationAcknowledgment, CancellationOwner, ProviderError};
use async_trait::async_trait;
use serde_json::Value;
use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use tokio::sync::{Mutex, Notify};

#[derive(Clone)]
struct TestAuth;
impl Auth for TestAuth {
    fn access(&self) -> Result<(String, String), TransportError> {
        unreachable!("offline transport")
    }
}

struct FinalTransport;
#[async_trait]
impl ResponsesTransport for FinalTransport {
    async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        Ok(ResponsesTurn {
            response_id: "final".into(),
            items: vec![Item(json!({
                "type":"message", "role":"assistant", "phase":"final_answer",
                "content":[{"type":"output_text","text":"done"}]
            }))],
            usage: Usage::default(),
        })
    }
}

struct CompletedOwner;
#[async_trait]
impl CancellationOwner for CompletedOwner {
    async fn cancel(
        &self,
        _: &OperationId,
        _: &crate::provider::JobHandle,
    ) -> CancellationAcknowledgment {
        CancellationAcknowledgment::Completed(Ok(json!("owner result")))
    }
}

struct CompletionProvider {
    store: Arc<Store>,
    calls: AtomicUsize,
    acknowledgments: AtomicUsize,
    committed: Mutex<HashSet<OperationId>>,
    reject: AtomicBool,
    external: bool,
    entered: Notify,
    paused: AtomicBool,
    release: Notify,
    acknowledged: Notify,
}

impl CompletionProvider {
    fn new(store: Arc<Store>, reject: bool, external: bool) -> Arc<Self> {
        Arc::new(Self {
            store,
            calls: AtomicUsize::new(0),
            acknowledgments: AtomicUsize::new(0),
            committed: Mutex::new(HashSet::new()),
            reject: AtomicBool::new(reject),
            external,
            entered: Notify::new(),
            paused: AtomicBool::new(false),
            release: Notify::new(),
            acknowledged: Notify::new(),
        })
    }
}

#[async_trait]
impl Provider for CompletionProvider {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.paused.load(Ordering::SeqCst) {
            self.release.notified().await;
        }
        if self.external {
            std::future::pending().await
        } else {
            Ok(json!("result"))
        }
    }
    fn cancellation_owner(&self) -> Option<Arc<dyn CancellationOwner>> {
        self.external
            .then(|| Arc::new(CompletedOwner) as Arc<dyn CancellationOwner>)
    }
    async fn output_committed(&self, operation: &OperationId) -> Result<(), ProviderError> {
        let claims = self.store.claims_for_operation(operation).unwrap();
        assert!(!claims.is_empty());
        assert!(
            claims
                .iter()
                .all(|claim| claim.state == crate::store::ClaimState::Settled)
        );
        assert!(claims.iter().all(|claim| {
            self.store
                .get_item(claim.output.as_ref().unwrap())
                .unwrap()
                .is_some()
        }));
        self.acknowledgments.fetch_add(1, Ordering::SeqCst);
        if self.reject.load(Ordering::SeqCst) {
            return Err(ProviderError::Tool("host attachment unavailable".into()));
        }
        self.committed.lock().await.insert(operation.clone());
        self.acknowledged.notify_one();
        Ok(())
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

fn engine(
    store: Arc<Store>,
    provider: Arc<CompletionProvider>,
    agent: &str,
) -> Engine<TestAuth, CompletionProvider, FinalTransport> {
    Engine::with_transport(
        FinalTransport,
        store,
        Arc::new(JobScheduler::new(1).unwrap()),
        provider,
        EngineConfig {
            instructions: "test".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "completion".into(),
            agent: AgentPath(agent.into()),
        },
    )
}

async fn admit(
    engine: &Engine<TestAuth, CompletionProvider, FinalTransport>,
) -> (RequestId, PendingCall) {
    let head = RequestId("completed-head".into());
    let item =
        Item(json!({"type":"function_call", "call_id":"work", "name":"work", "arguments":"{}"}));
    engine
        .store
        .write_request(
            &head,
            None,
            &engine.config.agent.0,
            std::slice::from_ref(&item),
            StoredUsage::default(),
        )
        .unwrap();
    engine.store.set_effort(&head, Effort::Low).unwrap();
    let DispatchResult::Pending(pending) =
        engine.dispatch_completed_item(item, &head).await.unwrap()
    else {
        panic!("live ordinary call must retain its pending scheduler job");
    };
    (head, pending)
}

fn empty_mailbox() -> tokio::sync::mpsc::UnboundedReceiver<Envelope> {
    tokio::sync::mpsc::unbounded_channel().1
}

#[tokio::test]
async fn output_acknowledgment_failure_recovers_without_replaying_work() {
    let store = Arc::new(Store::memory().unwrap());
    let provider = CompletionProvider::new(store.clone(), true, false);
    let runtime = engine(store.clone(), provider.clone(), "/root");
    let (head, call) = admit(&runtime).await;
    runtime.scheduler.wait(&call.operation).await.unwrap();
    let mut pending = vec![call.clone()];
    assert!(matches!(
        runtime.persist_settled(&mut pending, &head).await,
        Err(EngineError::ProviderCall(_))
    ));
    assert_eq!(pending.len(), 1);
    assert_eq!(
        store
            .items(&head)
            .unwrap()
            .iter()
            .filter(|item| item.0["type"] == "function_call_output")
            .count(),
        1
    );
    provider.reject.store(false, Ordering::SeqCst);
    let (_cancel, cancellation) = watch::channel(false);
    engine(store.clone(), provider.clone(), "/root")
        .run_recovering(Some(head), vec![], cancellation, empty_mailbox())
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.acknowledgments.load(Ordering::SeqCst), 2);
    assert_eq!(provider.committed.lock().await.len(), 1);
}

#[tokio::test]
async fn inherited_output_cannot_acknowledge_its_parent_operation() {
    let store = Arc::new(Store::memory().unwrap());
    let parent = CompletionProvider::new(store.clone(), false, false);
    let runtime = engine(store.clone(), parent.clone(), "/root");
    let (head, call) = admit(&runtime).await;
    runtime.scheduler.wait(&call.operation).await.unwrap();
    runtime
        .persist_settled(&mut vec![call], &head)
        .await
        .unwrap();
    let child = CompletionProvider::new(store.clone(), true, false);
    let (_cancel, cancellation) = watch::channel(false);
    engine(store, child.clone(), "/child")
        .run(Some(head), vec![], cancellation, empty_mailbox())
        .await
        .unwrap();
    assert_eq!(child.calls.load(Ordering::SeqCst), 0);
    assert_eq!(child.acknowledgments.load(Ordering::SeqCst), 0);
    assert_eq!(parent.committed.lock().await.len(), 1);
}

#[tokio::test]
async fn cancellation_completed_output_is_acknowledged_and_retries_are_idempotent() {
    let store = Arc::new(Store::memory().unwrap());
    let provider = CompletionProvider::new(store.clone(), false, true);
    let runtime = engine(store.clone(), provider.clone(), "/root");
    let (head, call) = admit(&runtime).await;
    provider.entered.notified().await;
    runtime
        .cancel_pending(std::slice::from_ref(&call))
        .await
        .unwrap();
    assert_eq!(
        runtime.scheduler.output(&call.operation).await.unwrap(),
        Some(crate::turn::JobOutput::Completed(Ok(json!("owner result"))))
    );
    assert_eq!(provider.committed.lock().await.len(), 1);
    let (_cancel, cancellation) = watch::channel(false);
    engine(store, provider.clone(), "/root")
        .run_recovering(Some(head), vec![], cancellation, empty_mailbox())
        .await
        .unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.committed.lock().await.len(), 1);
    assert_eq!(provider.acknowledgments.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn unavailable_attachment_keeps_real_output_and_refuses_completion() {
    let store = Arc::new(Store::memory().unwrap());
    let provider = CompletionProvider::new(store.clone(), true, false);
    let runtime = engine(store.clone(), provider.clone(), "/root");
    let (head, call) = admit(&runtime).await;
    runtime.scheduler.wait(&call.operation).await.unwrap();
    assert!(
        runtime
            .persist_settled(&mut vec![call.clone()], &head)
            .await
            .is_err()
    );
    let (_cancel, cancellation) = watch::channel(false);
    assert!(matches!(
        engine(store.clone(), provider.clone(), "/root")
            .run_recovering(Some(head), vec![], cancellation, empty_mailbox())
            .await,
        Err(EngineError::ProviderCall(_))
    ));
    assert_eq!(
        store.claims_for_operation(&call.operation).unwrap()[0].state,
        crate::store::ClaimState::Settled
    );
    assert!(provider.committed.lock().await.is_empty());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelled_round_retains_its_latest_durable_head() {
    let store = Arc::new(Store::memory().unwrap());
    let provider = CompletionProvider::new(store.clone(), false, false);
    let runtime = engine(store.clone(), provider, "/root");
    let previous = RequestId("before-interrupt".into());
    store.create_request(&previous, None, "/root").unwrap();
    store.set_effort(&previous, Effort::Low).unwrap();
    let (_cancel, cancellation) = watch::channel(true);
    let error = runtime
        .run(
            Some(previous.clone()),
            vec![],
            cancellation,
            empty_mailbox(),
        )
        .await
        .unwrap_err();
    let EngineError::Cancelled {
        head_request: Some(head),
    } = error
    else {
        panic!("missing cancelled head: {error:?}");
    };
    assert_eq!(store.request(&head).unwrap().unwrap().branch, "/root");
    assert_ne!(head, previous);
    assert_eq!(
        store.request(&head).unwrap().unwrap().parent,
        Some(previous)
    );
}

#[tokio::test]
async fn synthetic_settlement_never_acknowledges_completed_execution() {
    for output in [
        crate::turn::JobOutput::Cancelled,
        crate::turn::JobOutput::CancellationUnconfirmed("owner still running".into()),
    ] {
        let store = Arc::new(Store::memory().unwrap());
        let provider = CompletionProvider::new(store.clone(), true, false);
        let runtime = engine(store.clone(), provider.clone(), "/root");
        let head = RequestId("synthetic-head".into());
        let call = CallId("not-completed".into());
        store.write_request(&head, None, "/root", &[Item(json!({"type":"function_call", "call_id":call.0, "name":"work", "arguments":"{}"}))], StoredUsage::default()).unwrap();
        store.set_effort(&head, Effort::Low).unwrap();
        let operation = OperationId {
            origin: runtime.origin.clone(),
            request: head.clone(),
            call,
        };
        store.claim_operation(&operation, &head).unwrap();
        runtime
            .persist_output(&operation, ToolKind::Function, &output, &head, &head)
            .await
            .unwrap();
        let (_cancel, cancellation) = watch::channel(false);
        runtime
            .run_recovering(Some(head), vec![], cancellation, empty_mailbox())
            .await
            .unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.acknowledgments.load(Ordering::SeqCst), 0);
    }
}

#[derive(Clone)]
struct StalledTransport {
    reject_successor: Option<bool>,
    round: Arc<AtomicUsize>,
    stalled: Arc<Notify>,
    release: Arc<Notify>,
    inputs: Arc<Mutex<Vec<Vec<Item>>>>,
}

#[async_trait]
impl ResponsesTransport for StalledTransport {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.inputs.lock().await.push(request.input);
        let round = self.round.fetch_add(1, Ordering::SeqCst);
        let items = if round == 0 {
            vec![Item(
                json!({"type":"function_call", "call_id":"stalled-work", "name":"work", "async":true, "arguments":"{}"}),
            )]
        } else {
            if round == 1 {
                self.stalled.notify_one();
                self.release.notified().await;
                if let Some(authentication) = self.reject_successor {
                    return Err(if authentication {
                        TransportError::Authentication
                    } else {
                        TransportError::Http {
                            status: 400,
                            diagnostic: None,
                        }
                    });
                }
            }
            vec![Item(json!({
                "type":"message", "role":"assistant", "phase":"final_answer",
                "content":[{"type":"output_text","text":"done"}]
            }))]
        };
        Ok(ResponsesTurn {
            response_id: format!("stalled-{round}"),
            items,
            usage: Usage::default(),
        })
    }
}

#[tokio::test]
async fn completed_tool_is_acknowledged_while_successor_provider_response_is_stalled() {
    let store = Arc::new(Store::memory().unwrap());
    let provider = CompletionProvider::new(store.clone(), false, false);
    provider.paused.store(true, Ordering::SeqCst);
    let transport = StalledTransport {
        reject_successor: None,
        round: Arc::new(AtomicUsize::new(0)),
        stalled: Arc::new(Notify::new()),
        release: Arc::new(Notify::new()),
        inputs: Arc::new(Mutex::new(Vec::new())),
    };
    let runtime = engine(store.clone(), provider.clone(), "/root");
    let runtime = Engine::<TestAuth, _, _>::with_transport(
        transport.clone(),
        store.clone(),
        runtime.scheduler.clone(),
        provider.clone(),
        runtime.config.clone(),
    );
    let (_cancel, cancellation) = watch::channel(false);
    let task = tokio::spawn(async move {
        runtime
            .run(None, vec![], cancellation, empty_mailbox())
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        transport.stalled.notified(),
    )
    .await
    .unwrap();
    provider.release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        provider.acknowledged.notified(),
    )
    .await
    .expect("durable tool completion must not wait for the stalled provider");
    let claim = store
        .claims(&CallId("stalled-work".into()))
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(claim.state, crate::store::ClaimState::Settled);
    assert!(
        store
            .completed_finalization(&claim.operation)
            .unwrap()
            .is_some()
    );
    assert!(
        !transport.inputs.lock().await[1]
            .iter()
            .any(|item| item.0["type"] == "function_call_output")
    );
    assert!(
        !store
            .items(&claim.request)
            .unwrap()
            .iter()
            .any(|item| item.0["type"] == "function_call_output")
    );
    transport.release.notify_one();
    let completed = tokio::time::timeout(std::time::Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        completed
            .transcript
            .iter()
            .filter(|item| item.0["type"] == "function_call_output"
                && item.0["call_id"] == "stalled-work")
            .count(),
        1
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.committed.lock().await.len(), 1);
}

#[tokio::test]
async fn rejection_recovery_replays_settled_ancestor_output_once_without_reexecuting_tool() {
    for authentication in [false, true] {
        let store = Arc::new(Store::memory().unwrap());
        let provider = CompletionProvider::new(store.clone(), false, false);
        provider.paused.store(true, Ordering::SeqCst);
        let transport = StalledTransport {
            reject_successor: Some(authentication),
            round: Arc::new(AtomicUsize::new(0)),
            stalled: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            inputs: Arc::new(Mutex::new(Vec::new())),
        };
        let base = engine(store.clone(), provider.clone(), "/root");
        let runtime = Arc::new(Engine::<TestAuth, _, _>::with_transport(
            transport.clone(),
            store.clone(),
            base.scheduler.clone(),
            provider.clone(),
            base.config.clone(),
        ));
        let (_cancel, cancellation) = watch::channel(false);
        let running = runtime.clone();
        let first_cancellation = cancellation.clone();
        let first_input = Item(json!({"type":"message","role":"user","content":"first input"}));
        let first_input_clone = first_input.clone();
        let task = tokio::spawn(async move {
            running
                .run(
                    None,
                    vec![first_input_clone],
                    first_cancellation,
                    empty_mailbox(),
                )
                .await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            transport.stalled.notified(),
        )
        .await
        .unwrap();
        provider.release.notify_one();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            provider.acknowledged.notified(),
        )
        .await
        .unwrap();
        let claim = store
            .claims(&CallId("stalled-work".into()))
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(claim.state, crate::store::ClaimState::Settled);
        assert!(
            !transport.inputs.lock().await[1]
                .iter()
                .any(|item| item.0["type"] == "function_call_output")
        );
        assert!(
            !store
                .items(&claim.request)
                .unwrap()
                .iter()
                .any(|item| item.0["type"] == "function_call_output")
        );
        transport.release.notify_one();
        let error = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        let EngineError::RequestRejected { head_request, .. } = error else {
            panic!("unexpected failure: {error}")
        };
        assert_ne!(head_request, claim.request);
        assert_eq!(
            store.request(&head_request).unwrap().unwrap().parent,
            Some(claim.request)
        );
        assert_eq!(transport.round.load(Ordering::SeqCst), 2);
        let followup = Item(json!({"type":"message","role":"user","content":"explicit followup"}));
        let completion = runtime
            .run_recovering(
                Some(head_request.clone()),
                vec![followup.clone()],
                cancellation,
                empty_mailbox(),
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .request(&completion.head_request)
                .unwrap()
                .unwrap()
                .parent,
            Some(head_request)
        );
        let inputs = transport.inputs.lock().await;
        assert_eq!(inputs.len(), 3);
        assert_eq!(
            inputs[2]
                .iter()
                .filter(|item| item.0["type"] == "function_call_output"
                    && item.0["call_id"] == "stalled-work"
                    && item.0["output"] == "\"result\"")
                .count(),
            1
        );
        for user_input in [first_input, followup] {
            assert_eq!(
                inputs[2].iter().filter(|item| **item == user_input).count(),
                1
            );
        }
        assert_eq!(
            completion
                .transcript
                .iter()
                .filter(|item| item.0["type"] == "function_call_output"
                    && item.0["call_id"] == "stalled-work")
                .count(),
            1
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.committed.lock().await.len(), 1);
        assert_eq!(
            store.claims(&CallId("stalled-work".into())).unwrap().len(),
            1
        );
    }
}

struct AdmissionHost {
    identity: crate::embedding::HostIdentity,
    open: AtomicBool,
    acknowledgments: AtomicUsize,
    surface: Arc<crate::embedding::ToolSurface>,
}
struct AdmissionLease;
impl crate::embedding::AdmissionGuard for AdmissionLease {}
#[async_trait]
impl crate::embedding::HostActor for AdmissionHost {
    fn identity(&self) -> &crate::embedding::HostIdentity {
        &self.identity
    }
    fn admit(
        &self,
    ) -> Result<Box<dyn crate::embedding::AdmissionGuard>, crate::embedding::EmbeddedError> {
        if self.open.load(Ordering::SeqCst) {
            Ok(Box::new(AdmissionLease))
        } else {
            Err(crate::embedding::EmbeddedError::Host(
                "admission closed".into(),
            ))
        }
    }
    fn tool_surface(
        &self,
    ) -> Result<Arc<crate::embedding::ToolSurface>, crate::embedding::EmbeddedError> {
        Ok(self.surface.clone())
    }
    async fn wake(&self, _: i64) -> Result<(), String> {
        Ok(())
    }
    async fn control(
        &self,
        _: crate::embedding::HostControl,
    ) -> Result<Value, crate::embedding::HostControlError> {
        panic!("test does not issue control")
    }
    async fn output_committed(&self, _: &OperationId) -> Result<(), String> {
        self.acknowledgments.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn output_aborted(&self, _: &OperationId) -> Result<(), String> {
        panic!("unedited failure does not abort context")
    }
}
struct FailingDispatcher {
    calls: AtomicUsize,
}
#[async_trait]
impl Provider for FailingDispatcher {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(ProviderError::Tool("postdispatch provider failure".into()))
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

#[tokio::test]
async fn pinned_admission_refusal_skips_callback_but_dispatched_error_keeps_it_on_recovery() {
    for close_before_launch in [false, true] {
        let store = Arc::new(Store::memory().unwrap());
        let dispatcher = Arc::new(FailingDispatcher {
            calls: AtomicUsize::new(0),
        });
        let manifest = crate::embedding::EmbeddedToolManifest::with_scheduling(
            vec![json!({"type":"function","name":"work","description":"Perform owned work","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}})],
            std::collections::HashMap::from([("work".into(), ToolScheduling::BeforeNextInference)]),
        ).unwrap();
        let surface = Arc::new(
            crate::embedding::ToolSurface::from_manifest(
                "owner-version".into(),
                Arc::new(manifest),
                dispatcher.clone(),
            )
            .unwrap(),
        );
        let host = Arc::new(AdmissionHost {
            identity: crate::embedding::HostIdentity {
                run: "owner-run".into(),
                actor: AgentPath("/root".into()),
                incarnation: "same-incarnation".into(),
            },
            open: AtomicBool::new(true),
            acknowledgments: AtomicUsize::new(0),
            surface,
        });
        let conversation =
            crate::embedding::Conversation::attach(store.clone(), host.clone(), None).unwrap();
        let config = EngineConfig {
            instructions: "owned completion".into(),
            tools: vec![],
            model: "offline".into(),
            effort: Effort::Low,
            session_id: "responsibility".into(),
            agent: host.identity.actor.clone(),
        };
        let runtime = conversation
            .engine::<TestAuth, _>(
                FinalTransport,
                Arc::new(JobScheduler::new(1).unwrap()),
                config.clone(),
                std::num::NonZeroU64::new(10000).unwrap(),
            )
            .unwrap();
        let pinned = conversation.provider().request_snapshot().unwrap().unwrap();
        let head = RequestId("pinned-completion-head".into());
        let item = Item(
            json!({"type":"function_call","call_id":"owned-call","name":"work","arguments":"{}"}),
        );
        store
            .write_embedded_request(
                &host.identity,
                &head,
                None,
                std::slice::from_ref(&item),
                StoredUsage::default(),
            )
            .unwrap();
        store.set_effort(&head, Effort::Low).unwrap();
        store
            .record_event(
                Some(&head),
                "tool_surface",
                &json!({"version":"owner-version"}),
            )
            .unwrap();
        let (_cancel, mut cancellation) = watch::channel(false);
        let DispatchResult::Pending(call) = runtime
            .dispatch_with_provider(item, &head, pinned, None, &mut cancellation)
            .await
            .unwrap()
        else {
            panic!("native request is queued before launch")
        };
        assert!(call.queued);
        assert_eq!(dispatcher.calls.load(Ordering::SeqCst), 0);
        if close_before_launch {
            host.open.store(false, Ordering::SeqCst);
        }
        runtime
            .scheduler
            .release_operation(&call.operation, None)
            .await
            .unwrap();
        let output = runtime.scheduler.wait(&call.operation).await.unwrap();
        assert!(matches!(output, crate::turn::JobOutput::Completed(Err(_))));
        runtime
            .persist_output(&call.operation, call.tool_kind, &output, &head, &head)
            .await
            .unwrap();
        let expected = usize::from(!close_before_launch);
        assert_eq!(dispatcher.calls.load(Ordering::SeqCst), expected);
        assert_eq!(host.acknowledgments.load(Ordering::SeqCst), expected);
        assert_eq!(
            store
                .completed_finalization(&call.operation)
                .unwrap()
                .unwrap()
                .requires_provider(),
            !close_before_launch
        );
        drop(runtime);
        let recovered = conversation
            .engine::<TestAuth, _>(
                FinalTransport,
                Arc::new(JobScheduler::new(1).unwrap()),
                config,
                std::num::NonZeroU64::new(10000).unwrap(),
            )
            .unwrap();
        let (_cancel, cancel) = watch::channel(false);
        let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>();
        recovered
            .run_recovering_embedded(Some(head), vec![], cancel, mail)
            .await
            .unwrap();
        assert_eq!(
            dispatcher.calls.load(Ordering::SeqCst),
            expected,
            "recovery never reexecutes"
        );
        assert_eq!(host.acknowledgments.load(Ordering::SeqCst), expected * 2);
    }
}
