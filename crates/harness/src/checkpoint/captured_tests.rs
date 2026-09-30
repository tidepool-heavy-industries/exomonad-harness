use super::*;
use crate::{
    embedding::HostIdentity,
    engine::{Engine, EngineConfig, ResponsesTransport},
    model::Effort,
    provider::{CallContext, Provider, ProviderError},
    store::ClaimState,
    transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, Usage},
    turn::JobScheduler,
};
use async_trait::async_trait;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{mpsc, oneshot, watch};

fn item(value: Value) -> Item {
    Item(value)
}

fn config(actor: AgentPath) -> EngineConfig {
    EngineConfig {
        instructions: "captured child".into(),
        tools: vec![],
        model: "test".into(),
        effort: Effort::Medium,
        session_id: actor.0.clone(),
        agent: actor,
    }
}

#[derive(Clone)]
struct FakeAuth;
impl Auth for FakeAuth {
    fn access(&self) -> std::result::Result<(String, String), TransportError> {
        Ok(("unused".into(), "unused".into()))
    }
}

struct Replay {
    sent: Arc<Mutex<Vec<ResponsesRequest>>>,
    turns: Mutex<std::collections::VecDeque<ResponsesTurn>>,
}
impl Replay {
    fn new(items: Vec<Vec<Item>>) -> Self {
        Self {
            sent: Arc::new(Mutex::new(vec![])),
            turns: Mutex::new(
                items
                    .into_iter()
                    .enumerate()
                    .map(|(index, items)| ResponsesTurn {
                        response_id: format!("turn-{index}"),
                        items,
                        usage: Usage::default(),
                    })
                    .collect(),
            ),
        }
    }
}
#[async_trait]
impl ResponsesTransport for Replay {
    async fn create(
        &self,
        request: ResponsesRequest,
    ) -> std::result::Result<ResponsesTurn, TransportError> {
        self.sent.lock().unwrap().push(request);
        self.turns
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| TransportError::Stream("unexpected request".into()))
    }
}

#[test]
fn captured_cut_preserves_multicall_order_and_exact_earlier_dependencies() {
    let store = Store::memory().unwrap();
    let root = AgentPath("/root".into());
    let identity = HostIdentity {
        run: "run".into(),
        actor: root.clone(),
        incarnation: "one".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    let source = RequestId("source".into());
    store.create_request(&source, None, &root.0).unwrap();
    store.set_effort(&source, Effort::Medium).unwrap();
    let earlier = CallId("earlier".into());
    let current = CallId("current".into());
    let later = CallId("later".into());
    let user = item(json!({"type":"message","role":"user","content":"immutable prefix"}));
    let reasoning = item(json!({"type":"reasoning","id":"reasoning-one","summary":[]}));
    let earlier_item = item(
        json!({"type":"function_call","id":"fc-one","call_id":earlier.0,"name":"earlier","arguments":"{}"}),
    );
    let current_item = item(
        json!({"type":"custom_tool_call","id":"ct-two","call_id":current.0,"name":"cell","input":"checkpoint then await"}),
    );
    let later_item = item(
        json!({"type":"function_call","id":"fc-three","call_id":later.0,"name":"later","arguments":"{}"}),
    );
    store
        .append_items(
            &source,
            &[
                user.clone(),
                reasoning.clone(),
                earlier_item.clone(),
                current_item.clone(),
                later_item,
            ],
        )
        .unwrap();
    let earlier_operation = store.claim(&earlier, &source).unwrap();
    let operation = store.claim(&current, &source).unwrap();
    store.claim(&later, &source).unwrap();
    let attachment = Arc::new("runtime scope");
    let cuts = store
        .capture_checkpoint_cuts(&operation, &json!({"name":"captured"}), attachment.clone())
        .unwrap();
    assert_eq!(cuts.before_call().cut(), CheckpointCut::BeforeCall);
    assert_eq!(cuts.deferred().cut(), CheckpointCut::Deferred);
    assert_eq!(cuts.before_call().operation(), Some(&operation));
    let before = store.items(cuts.before_call().snapshot_request()).unwrap();
    let transcript: Vec<_> = before
        .iter()
        .filter(|item| !item.is_configuration_update())
        .cloned()
        .collect();
    assert_eq!(transcript, [user, reasoning, earlier_item]);
    assert!(!before.contains(&current_item));
    assert_eq!(
        cuts.before_call().pending_claims(),
        &[CheckpointClaim {
            operation: earlier_operation.clone(),
            request: source.clone(),
            kind: ToolKind::Function,
        }]
    );
    assert_eq!(cuts.deferred().pending_claims().len(), 2);
    assert!(
        store
            .items(cuts.deferred().snapshot_request())
            .unwrap()
            .contains(&current_item)
    );
    assert_eq!(cuts.before_call().metadata(), &json!({"name":"captured"}));

    // Later history and cancellation of the current operation cannot alter
    // either frozen transcript or introduce a dependency on the current call.
    store
        .append_items(
            &source,
            &[item(
                json!({"type":"message","role":"user","content":"future"}),
            )],
        )
        .unwrap();
    store
        .interrupt_operation_claim(&operation, &source)
        .unwrap();
    let retained = cuts.before_call().clone();
    drop(cuts);
    for name in ["left", "right"] {
        let child = AgentPath(format!("/root/{name}"));
        let (agent, _) = store
            .attach_checkpoint_child(
                &retained,
                CheckpointChild {
                    path: &child,
                    parent: &root,
                    contract: &json!({}),
                    checkout: &json!({}),
                    task: None,
                },
            )
            .unwrap();
        assert_eq!(agent.fork_source["cut"], "before_call");
        assert_eq!(
            agent.fork_source["operation"],
            serde_json::to_value(&operation).unwrap()
        );
        let claims = store
            .claims_on(agent.head_request.as_ref().unwrap())
            .unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].operation, earlier_operation);
        assert_eq!(claims[0].state, ClaimState::Pending);
    }
    assert_eq!(store.items(retained.snapshot_request()).unwrap(), before);
    assert_eq!(Arc::strong_count(&attachment), 2);
}

#[test]
fn captured_cut_refuses_stale_incarnation_and_nonpending_original_transactionally() {
    let store = Store::memory().unwrap();
    let root = AgentPath("/root".into());
    let identity = HostIdentity {
        run: "run".into(),
        actor: root.clone(),
        incarnation: "one".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    let source = RequestId("source".into());
    let call = CallId("call".into());
    store.create_request(&source, None, &root.0).unwrap();
    store.set_effort(&source, Effort::Low).unwrap();
    store
        .append_items(
            &source,
            &[item(
                json!({"type":"function_call","call_id":call.0,"name":"capture","arguments":"{}"}),
            )],
        )
        .unwrap();
    let operation = store.claim(&call, &source).unwrap();
    let attachment = Arc::new(());
    let mut stale = operation.clone();
    if let ConversationIdentity::Embedded { incarnation, .. } = &mut stale.origin {
        *incarnation = "stale".into();
    }
    assert!(matches!(
        store.capture_checkpoint_cuts(&stale, &json!({}), attachment.clone()),
        Err(StoreError::OperationOriginMismatch)
    ));
    let unknown = OperationId {
        call: CallId("missing".into()),
        ..operation.clone()
    };
    assert!(matches!(
        store.capture_checkpoint_cuts(&unknown, &json!({}), attachment.clone()),
        Err(StoreError::MissingCheckpointClaim { .. })
    ));
    store
        .interrupt_operation_claim(&operation, &source)
        .unwrap();
    assert!(matches!(
        store.capture_checkpoint_cuts(&operation, &json!({}), attachment.clone()),
        Err(StoreError::CheckpointBoundaryNotPending(_))
    ));
    let count: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM checkpoints", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(Arc::strong_count(&attachment), 1);
}

#[test]
fn captured_metadata_migrates_schema_five_as_deferred_and_refuses_unknown_cuts() {
    let store = Store::memory().unwrap();
    let root = AgentPath("/root".into());
    let source = RequestId("source".into());
    let call = CallId("call".into());
    store.create_request(&source, None, &root.0).unwrap();
    store.set_effort(&source, Effort::Low).unwrap();
    store
        .admit_agent(&root, None, Some(&source), &json!({}), &json!({}))
        .unwrap();
    store
        .append_items(
            &source,
            &[item(
                json!({"type":"function_call","call_id":call.0,"name":"capture","arguments":"{}"}),
            )],
        )
        .unwrap();
    let operation = store.claim(&call, &source).unwrap();
    let checkpoint = store
        .capture_checkpoint(
            &root,
            &source,
            &call,
            &json!({"legacy":"host"}),
            Arc::new(()),
        )
        .unwrap();
    let mut conn = store.lock();
    conn.execute("UPDATE schema_version SET version=5", [])
        .unwrap();
    conn.execute(
        "UPDATE checkpoints SET metadata=?1",
        [r#"{"legacy":"host"}"#],
    )
    .unwrap();
    crate::store::schema::initialize(&mut conn).unwrap();
    let raw: String = conn
        .query_row(
            "SELECT metadata FROM checkpoints WHERE id=?1",
            [&checkpoint.id],
            |row| row.get(0),
        )
        .unwrap();
    let migrated = CheckpointMetadata::decode(&raw).unwrap();
    assert_eq!(migrated.cut, CheckpointCut::Deferred);
    assert_eq!(migrated.operation, None);
    assert_eq!(migrated.host, json!({"legacy":"host"}));
    drop(conn);
    assert!(
        store
            .claims_for_operation(&operation)
            .unwrap()
            .iter()
            .all(|claim| claim.operation == operation && claim.state == ClaimState::Pending)
    );
    let mut conn = store.lock();
    for invalid in [
        json!({"version":1,"cut":"unknown","operation":null,"host":{}}),
        json!({"version":2,"cut":"deferred","operation":null,"host":{}}),
        json!({"version":1,"cut":"before_call","operation":null,"host":{}}),
    ] {
        conn.execute("UPDATE checkpoints SET metadata=?1", [invalid.to_string()])
            .unwrap();
        assert!(crate::store::schema::initialize(&mut conn).is_err());
    }
}

struct StalledTransport {
    started: Mutex<Option<oneshot::Sender<()>>>,
}
#[async_trait]
impl ResponsesTransport for StalledTransport {
    async fn create(
        &self,
        _request: ResponsesRequest,
    ) -> std::result::Result<ResponsesTurn, TransportError> {
        self.started
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(())
            .unwrap();
        std::future::pending().await
    }
}

struct ParentTransport {
    sent: Arc<Mutex<Vec<ResponsesRequest>>>,
    committed: watch::Receiver<bool>,
    waiting: Mutex<Option<oneshot::Sender<()>>>,
}
#[async_trait]
impl ResponsesTransport for ParentTransport {
    async fn create(
        &self,
        request: ResponsesRequest,
    ) -> std::result::Result<ResponsesTurn, TransportError> {
        let first = {
            let mut sent = self.sent.lock().unwrap();
            sent.push(request);
            sent.len() == 1
        };
        let items = if first {
            vec![item(
                json!({"type":"function_call","call_id":"parent-call","name":"capture","arguments":"{}"}),
            )]
        } else {
            self.waiting
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            let mut committed = self.committed.clone();
            committed
                .wait_for(|committed| *committed)
                .await
                .map_err(|error| TransportError::Stream(error.to_string()))?;
            vec![final_answer()]
        };
        Ok(ResponsesTurn {
            response_id: if first { "call" } else { "final" }.into(),
            items,
            usage: Usage::default(),
        })
    }
}

struct CaptureProvider {
    store: Arc<Store>,
    captured: Mutex<Option<oneshot::Sender<CheckpointCuts<()>>>>,
    release: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
    calls: AtomicUsize,
    committed: watch::Sender<bool>,
    acknowledgments: AtomicUsize,
    original: Mutex<Option<OperationId>>,
}
#[async_trait]
impl Provider for CaptureProvider {
    async fn output_committed(
        &self,
        operation: &OperationId,
    ) -> std::result::Result<(), ProviderError> {
        assert_eq!(self.original.lock().unwrap().as_ref(), Some(operation));
        let claims = self.store.claims_for_operation(operation).unwrap();
        assert!(
            claims
                .iter()
                .any(|claim| claim.request == operation.request
                    && claim.state == ClaimState::Settled)
        );
        if !self.committed.send_replace(true) {
            self.acknowledgments.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn call(&self, _name: &str, _args: Value) -> std::result::Result<Value, ProviderError> {
        unreachable!("Engine supplies exact call context")
    }
    async fn call_with_context(
        &self,
        _name: &str,
        _args: Value,
        context: CallContext,
    ) -> std::result::Result<Value, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.original.lock().unwrap() = context.operation.clone();
        let cuts = self
            .store
            .capture_checkpoint_cuts(
                context.operation.as_ref().unwrap(),
                &json!({"name":"live"}),
                Arc::new(()),
            )
            .map_err(|e| ProviderError::Tool(e.to_string()))?;
        self.captured
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(cuts)
            .unwrap();
        self.release.lock().await.take().unwrap().await.unwrap();
        Err(ProviderError::Tool("later parent failure".into()))
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

fn final_answer() -> Item {
    item(
        json!({"type":"message","role":"assistant","phase":"final_answer","content":"child settled"}),
    )
}

#[tokio::test]
async fn captured_cut_two_child_engines_finish_while_original_provider_call_is_pending() {
    let store = Arc::new(Store::memory().unwrap());
    let root = AgentPath("/root".into());
    store
        .admit_agent(&root, None, None, &json!({}), &json!({}))
        .unwrap();
    let (captured_tx, captured_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let (committed_tx, committed_rx) = watch::channel(false);
    let provider = Arc::new(CaptureProvider {
        store: store.clone(),
        captured: Mutex::new(Some(captured_tx)),
        release: tokio::sync::Mutex::new(Some(release_rx)),
        calls: AtomicUsize::new(0),
        committed: committed_tx,
        acknowledgments: AtomicUsize::new(0),
        original: Mutex::new(None),
    });
    let scheduler = Arc::new(JobScheduler::new(3).unwrap());
    let root_sent = Arc::new(Mutex::new(vec![]));
    let (waiting_tx, waiting_rx) = oneshot::channel();
    let root_replay = ParentTransport {
        sent: root_sent.clone(),
        committed: committed_rx,
        waiting: Mutex::new(Some(waiting_tx)),
    };
    let root_engine = Engine::<FakeAuth, _, _>::with_transport(
        root_replay,
        store.clone(),
        scheduler.clone(),
        provider.clone(),
        config(root.clone()),
    );
    let (_root_cancel, root_cancel_rx) = watch::channel(false);
    let (_root_mail, root_mail_rx) = mpsc::unbounded_channel();
    let root_run = tokio::spawn(async move {
        root_engine
            .run(
                None,
                vec![item(
                    json!({"type":"message","role":"user","content":"retained task"}),
                )],
                root_cancel_rx,
                root_mail_rx,
            )
            .await
    });
    let cuts = tokio::time::timeout(std::time::Duration::from_secs(2), captured_rx)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), waiting_rx)
        .await
        .unwrap()
        .unwrap();
    let operation = cuts.before_call().operation().unwrap().clone();
    let retained = cuts.before_call().clone();
    let deferred = cuts.deferred().clone();
    drop(cuts);
    for name in ["left", "right"] {
        let child = AgentPath(format!("/root/{name}"));
        let (attached, _) = store
            .attach_checkpoint_child(
                &retained,
                CheckpointChild {
                    path: &child,
                    parent: &root,
                    contract: &json!({}),
                    checkout: &json!({}),
                    task: None,
                },
            )
            .unwrap();
        let replay = Replay::new(vec![vec![final_answer()]]);
        let sent = replay.sent.clone();
        let child_engine = Engine::<FakeAuth, _, _>::with_transport(
            replay,
            store.clone(),
            scheduler.clone(),
            provider.clone(),
            config(child),
        );
        let (_cancel, cancel_rx) = watch::channel(false);
        let (_mail, mail_rx) = mpsc::unbounded_channel();
        let complete = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            child_engine.run(attached.head_request, vec![], cancel_rx, mail_rx),
        )
        .await
        .unwrap()
        .unwrap();
        let requests = sent.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .input
                .iter()
                .any(|item| item.0["content"] == "retained task")
        );
        assert!(
            !requests[0]
                .input
                .iter()
                .any(|item| item.0["call_id"] == operation.call.0)
        );
        assert!(
            !complete
                .transcript
                .iter()
                .any(|item| item.0["call_id"] == operation.call.0)
        );
        assert_eq!(
            store
                .claims_for_operation(&operation)
                .unwrap()
                .iter()
                .find(|claim| claim.request == operation.request)
                .unwrap()
                .state,
            ClaimState::Pending
        );
        assert!(!root_run.is_finished());
    }
    // Cancelling a child drops only its request. It cannot acknowledge or
    // cancel the original parent's excluded operation.
    let child = AgentPath("/root/cancelled".into());
    let (attached, _) = store
        .attach_checkpoint_child(
            &retained,
            CheckpointChild {
                path: &child,
                parent: &root,
                contract: &json!({}),
                checkout: &json!({}),
                task: None,
            },
        )
        .unwrap();
    let (started_tx, started_rx) = oneshot::channel();
    let child_engine = Engine::<FakeAuth, _, _>::with_transport(
        StalledTransport {
            started: Mutex::new(Some(started_tx)),
        },
        store.clone(),
        scheduler.clone(),
        provider.clone(),
        config(child),
    );
    let (cancel, cancel_rx) = watch::channel(false);
    let (_mail, mail_rx) = mpsc::unbounded_channel();
    let child_run = tokio::spawn(async move {
        child_engine
            .run(attached.head_request, vec![], cancel_rx, mail_rx)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), started_rx)
        .await
        .unwrap()
        .unwrap();
    cancel.send(true).unwrap();
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), child_run)
            .await
            .unwrap()
            .unwrap(),
        Err(crate::engine::EngineError::Cancelled { .. })
    ));
    assert!(!root_run.is_finished());
    assert_eq!(
        store
            .claims_for_operation(&operation)
            .unwrap()
            .iter()
            .find(|claim| claim.request == operation.request)
            .unwrap()
            .state,
        ClaimState::Pending
    );
    assert_eq!(provider.acknowledgments.load(Ordering::SeqCst), 0);
    assert!(!provider.committed.borrow().to_owned());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(deferred.pending_claims()[0].operation, operation);
    release_tx.send(()).unwrap();
    let completion = tokio::time::timeout(std::time::Duration::from_secs(2), root_run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        completion
            .transcript
            .iter()
            .any(|item| item.0["call_id"] == operation.call.0
                && item.0["type"] == "function_call_output")
    );
    assert!(
        store
            .claims_for_operation(&operation)
            .unwrap()
            .iter()
            .all(|claim| claim.state == ClaimState::Settled)
    );
    assert!(
        store
            .claims_on(retained.snapshot_request())
            .unwrap()
            .is_empty()
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.acknowledgments.load(Ordering::SeqCst), 1);
    let sent = root_sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(
        !sent[1]
            .input
            .iter()
            .any(|item| item.0["call_id"] == operation.call.0
                && item.0["type"] == "function_call_output"),
        "issued request stays immutable while real output is settled durably"
    );
}
