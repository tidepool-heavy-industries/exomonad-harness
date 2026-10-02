use super::*;
use crate::provider::{CallContext, ProviderError};
use std::sync::Mutex;
use tokio::sync::{Notify, mpsc};

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("offline")
    }
}
fn call(id: &str, name: &str) -> Item {
    Item(json!({"type":"function_call","call_id":id,"name":name,"arguments":"{}"}))
}
fn turn(items: Vec<Item>) -> ResponsesTurn {
    ResponsesTurn {
        response_id: "test".into(),
        items,
        usage: Usage::default(),
    }
}
fn final_turn() -> ResponsesTurn {
    turn(vec![Item(
        json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}),
    )])
}
struct Mixed {
    store: Arc<Store>,
    started: mpsc::UnboundedSender<String>,
    release: Arc<Notify>,
}
#[async_trait::async_trait]
impl Provider for Mixed {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
    fn tool_scheduling(&self, name: &str) -> ToolScheduling {
        if name.starts_with("sync") {
            ToolScheduling::BeforeNextInference
        } else {
            ToolScheduling::Async
        }
    }
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        unreachable!()
    }
    async fn call_with_context(
        &self,
        name: &str,
        _: serde_json::Value,
        context: CallContext,
    ) -> Result<serde_json::Value, ProviderError> {
        if name.starts_with("sync") {
            let request = context.request.as_ref().unwrap();
            assert!(self.store.recorded_response(request).unwrap().is_some());
            let claims = self.store.claims_on(request).unwrap();
            assert_eq!(
                claims.len(),
                3,
                "all original identities admitted before sync starts"
            );
            if name == "sync_second" {
                assert!(
                    self.store
                        .items(request)
                        .unwrap()
                        .iter()
                        .any(|item| item.0["call_id"] == "first"
                            && item.0["type"] == "function_call_output")
                );
            }
        }
        self.started.send(name.to_owned()).unwrap();
        if name == "sync_first" {
            self.release.notified().await;
        }
        Ok(json!({"name":name}))
    }
    async fn call_custom_with_context(
        &self,
        name: &str,
        input: String,
        context: CallContext,
    ) -> Result<serde_json::Value, ProviderError> {
        assert_eq!(input, "exact raw cell");
        self.call_with_context(name, json!({}), context).await
    }
}
struct Streaming {
    requests: Mutex<Vec<ResponsesRequest>>,
    emitted: Arc<Notify>,
    finish: Arc<Notify>,
    fail: bool,
    omit_sync: bool,
}
#[async_trait::async_trait]
impl ResponsesTransport for Streaming {
    async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        unreachable!()
    }
    async fn create_streaming(
        &self,
        request: ResponsesRequest,
        sink: mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        if index > 1 {
            return Ok(final_turn());
        }
        let first = call("first", "sync_first");
        let asynchronous = call("async", "async_work");
        sink.send(StreamEvent::ItemDone(first.clone()))
            .await
            .unwrap();
        sink.send(StreamEvent::ItemDone(asynchronous.clone()))
            .await
            .unwrap();
        self.emitted.notify_one();
        self.finish.notified().await;
        if self.fail {
            return Err(TransportError::Stream("incomplete response".into()));
        }
        if self.omit_sync {
            return Ok(turn(vec![asynchronous]));
        }
        Ok(turn(vec![
            first,
            asynchronous,
            Item(
                json!({"type":"custom_tool_call","call_id":"second","name":"sync_second","input":"exact raw cell"}),
            ),
        ]))
    }
}
fn engine(
    fail: bool,
) -> (
    Arc<Engine<Offline, Mixed, Streaming>>,
    mpsc::UnboundedReceiver<String>,
    Arc<Notify>,
    Arc<Notify>,
    Arc<Notify>,
) {
    let store = Arc::new(Store::memory().unwrap());
    let (started, receiver) = mpsc::unbounded_channel();
    let release = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let emitted = Arc::new(Notify::new());
    let engine = Engine::with_transport(
        Streaming {
            requests: Mutex::new(vec![]),
            emitted: emitted.clone(),
            finish: finish.clone(),
            fail,
            omit_sync: false,
        },
        store.clone(),
        Arc::new(JobScheduler::new(3).unwrap()),
        Arc::new(Mixed {
            store,
            started,
            release: release.clone(),
        }),
        EngineConfig {
            instructions: "test".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "test".into(),
            agent: AgentPath("/root".into()),
        },
    );
    (Arc::new(engine), receiver, emitted, finish, release)
}
fn mailbox() -> mpsc::UnboundedReceiver<Envelope> {
    let (_, receiver) = mpsc::unbounded_channel();
    receiver
}

#[tokio::test]
async fn mixed_sync_waits_for_complete_response_and_serial_terminal_persistence() {
    let (engine, mut starts, emitted, finish, release) = engine(false);
    let (_cancel, cancelled) = watch::channel(false);
    let running = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.run(None, vec![], cancelled, mailbox()).await })
    };
    emitted.notified().await;
    assert_eq!(starts.recv().await.unwrap(), "async_work");
    assert!(
        starts.try_recv().is_err(),
        "sync cannot start while issuing response is incomplete"
    );
    finish.notify_one();
    assert_eq!(starts.recv().await.unwrap(), "sync_first");
    assert!(
        starts.try_recv().is_err(),
        "second sync cannot overtake first full completion"
    );
    assert_eq!(engine.client.requests.lock().unwrap().len(), 1);
    release.notify_one();
    assert_eq!(starts.recv().await.unwrap(), "sync_second");
    let completion = running.await.unwrap().unwrap();
    let requests = engine.client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for id in ["first", "second", "async"] {
        assert_eq!(
            requests[1]
                .input
                .iter()
                .filter(|item| item.0["call_id"] == id
                    && matches!(
                        item.0["type"].as_str(),
                        Some("function_call_output" | "custom_tool_call_output")
                    ))
                .count(),
            1
        );
    }
    assert!(
        completion
            .transcript
            .iter()
            .any(|item| item.0["phase"] == "final_answer")
    );
}

#[tokio::test]
async fn incomplete_issuing_response_never_executes_queued_sync_calls() {
    let (engine, mut starts, emitted, finish, _) = engine(true);
    let (_cancel, cancelled) = watch::channel(false);
    let running = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.run(None, vec![], cancelled, mailbox()).await })
    };
    emitted.notified().await;
    assert_eq!(starts.recv().await.unwrap(), "async_work");
    finish.notify_one();
    assert!(running.await.unwrap().is_err());
    assert!(starts.try_recv().is_err());
    let claims = engine
        .store
        .claims_on_branch_lineage(
            &engine
                .store
                .events(None)
                .unwrap()
                .into_iter()
                .find(|event| event.kind == "tool_scheduling")
                .unwrap()
                .request
                .unwrap(),
            "/root",
        )
        .unwrap();
    assert!(
        claims
            .iter()
            .all(|claim| claim.state != crate::store::ClaimState::Pending)
    );
}

struct ContextEditor {
    store: Arc<Store>,
    operations: Arc<Mutex<Vec<OperationId>>>,
    lose_ack: std::sync::atomic::AtomicBool,
    full_success: bool,
    refuse_draft: bool,
    committed: std::sync::atomic::AtomicUsize,
    aborted: std::sync::atomic::AtomicUsize,
    paused: Option<(Arc<Notify>, Arc<Notify>)>,
}
#[async_trait::async_trait]
impl Provider for ContextEditor {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
    fn tool_scheduling(&self, _: &str) -> ToolScheduling {
        ToolScheduling::BeforeNextInference
    }
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        unreachable!()
    }
    async fn complete_call(
        &self,
        name: &str,
        _: ToolInput,
        context: CallContext,
    ) -> crate::provider::ProviderCompletion {
        let snapshot = context.context.expect("sync Store lease");
        assert_eq!(context.operation.as_ref(), Some(&snapshot.operation));
        let mut document = snapshot.document;
        if name == "edit_second" {
            assert!(document.blocks.iter().any(|block| matches!(block,crate::context::ContextBlock::Text {text,..} if text == "first commit")));
            assert_eq!(
                self.store
                    .context_model(&snapshot.operation.origin)
                    .unwrap()
                    .as_deref(),
                Some("model-next")
            );
        }
        document.blocks.push(crate::context::ContextBlock::Text {
            reference: None,
            role: crate::context::ContextRole::Assistant,
            text: if name == "edit_first" {
                "first commit"
            } else {
                "second commit"
            }
            .into(),
            sources: vec![],
        });
        self.operations.lock().unwrap().push(snapshot.operation);
        if let Some((started, release)) = &self.paused {
            started.notify_one();
            release.notified().await;
        }
        crate::provider::ProviderCompletion {
            result: Ok(json!({"done":name})),
            full_success: self.full_success,
            context: crate::provider::ContextDisposition::Draft(crate::context::ContextDraft {
                document,
                next_model: if self.refuse_draft {
                    Some(String::new())
                } else {
                    (name == "edit_first").then(|| "model-next".into())
                },
            }),
        }
    }
    async fn output_committed(&self, operation: &OperationId) -> Result<(), ProviderError> {
        self.committed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if !self.full_success {
            assert!(self.store.context_receipt(operation).unwrap().is_none());
            return Ok(());
        }
        let receipt = self
            .store
            .context_receipt(operation)
            .unwrap()
            .expect("commit precedes acknowledgment");
        assert_eq!(
            self.store
                .context_request_state(&receipt.head, &operation.origin)
                .unwrap()
                .generation,
            receipt.generation
        );
        if self
            .lose_ack
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            Err(ProviderError::Tool("lost acknowledgment".into()))
        } else {
            Ok(())
        }
    }
    async fn output_aborted(&self, operation: &OperationId) -> Result<(), ProviderError> {
        assert!(self.store.context_receipt(operation).unwrap().is_none());
        self.aborted
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}
struct ContextScript {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    two_calls: bool,
}
#[async_trait::async_trait]
impl ResponsesTransport for ContextScript {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        if index == 1 {
            let mut items = vec![call("edit-first", "edit_first")];
            if self.two_calls {
                items.push(call("edit-second", "edit_second"));
            }
            Ok(turn(items))
        } else {
            Ok(final_turn())
        }
    }
}
fn context_engine(
    two_calls: bool,
    lose_ack: bool,
) -> (
    Engine<Offline, ContextEditor, ContextScript>,
    Arc<Mutex<Vec<ResponsesRequest>>>,
    Arc<Mutex<Vec<OperationId>>>,
) {
    let store = Arc::new(Store::memory().unwrap());
    let requests = Arc::new(Mutex::new(vec![]));
    let operations = Arc::new(Mutex::new(vec![]));
    let engine = Engine::with_transport(
        ContextScript {
            requests: requests.clone(),
            two_calls,
        },
        store.clone(),
        Arc::new(JobScheduler::new(2).unwrap()),
        Arc::new(ContextEditor {
            store,
            operations: operations.clone(),
            lose_ack: std::sync::atomic::AtomicBool::new(lose_ack),
            full_success: true,
            refuse_draft: false,
            committed: std::sync::atomic::AtomicUsize::new(0),
            aborted: std::sync::atomic::AtomicUsize::new(0),
            paused: None,
        }),
        EngineConfig {
            instructions: "instructions".into(),
            tools: vec![],
            model: "model-old".into(),
            effort: Effort::Low,
            session_id: "context-tests".into(),
            agent: AgentPath("/root".into()),
        },
    );
    (engine, requests, operations)
}

#[tokio::test]
async fn serial_sync_context_commits_and_model_route_are_visible_before_next_call_and_request() {
    let (engine, requests, operations) = context_engine(true, false);
    let (_cancel, cancelled) = watch::channel(false);
    let complete = engine
        .run(
            None,
            vec![Item(
                json!({"type":"message","role":"user","content":"original"}),
            )],
            cancelled,
            mailbox(),
        )
        .await
        .unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].model, "model-old");
    assert_eq!(requests[1].model, "model-next");
    assert!(requests[1].input.iter().any(|item| {
        item.0["content"]
            .as_str()
            .is_some_and(|text| text.ends_with("first commit"))
    }));
    assert!(requests[1].input.iter().any(|item| {
        item.0["content"]
            .as_str()
            .is_some_and(|text| text.ends_with("second commit"))
    }));
    for operation in operations.lock().unwrap().iter() {
        assert!(engine.store.context_receipt(operation).unwrap().is_some());
        assert_eq!(
            complete
                .transcript
                .iter()
                .filter(|item| item.0["call_id"] == operation.call.0
                    && item.0["type"] == "function_call_output")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn lost_context_commit_acknowledgment_recovers_without_reexecuting_effects() {
    let (engine, requests, operations) = context_engine(false, true);
    let (_cancel, cancelled) = watch::channel(false);
    assert!(
        engine
            .run(None, vec![], cancelled, mailbox())
            .await
            .is_err()
    );
    let operation = operations.lock().unwrap()[0].clone();
    let receipt = engine
        .store
        .context_receipt(&operation)
        .unwrap()
        .expect("durable despite lost ack");
    let (_cancel, cancelled) = watch::channel(false);
    engine
        .run_recovering(Some(receipt.head), vec![], cancelled, mailbox())
        .await
        .unwrap();
    assert_eq!(operations.lock().unwrap().len(), 1);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].model, "model-next");
    assert!(requests[1].input.iter().any(|item| {
        item.0["content"]
            .as_str()
            .is_some_and(|text| text.ends_with("first commit"))
    }));
    assert_eq!(
        requests[1]
            .input
            .iter()
            .filter(|item| item.0["call_id"] == "edit-first"
                && item.0["type"] == "function_call_output")
            .count(),
        1
    );
}

#[tokio::test]
async fn partial_success_cannot_publish_staged_context_or_model() {
    let (mut engine, requests, operations) = context_engine(false, false);
    Arc::get_mut(&mut engine.provider).unwrap().full_success = false;
    let (_cancel, cancelled) = watch::channel(false);
    engine
        .run(None, vec![], cancelled, mailbox())
        .await
        .unwrap();
    let operation = operations.lock().unwrap()[0].clone();
    assert!(engine.store.context_receipt(&operation).unwrap().is_none());
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].model, "model-old");
    assert!(!requests[1].input.iter().any(|item| {
        item.0["content"]
            .as_str()
            .is_some_and(|text| text.ends_with("first commit"))
    }));
}

#[tokio::test]
async fn cancellation_of_sync_invocation_discards_draft_and_preserves_model() {
    let (mut engine, requests, operations) = context_engine(false, false);
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    Arc::get_mut(&mut engine.provider).unwrap().paused = Some((started.clone(), release));
    let engine = Arc::new(engine);
    let (cancel, cancelled) = watch::channel(false);
    let running = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.run(None, vec![], cancelled, mailbox()).await })
    };
    started.notified().await;
    cancel.send(true).unwrap();
    assert!(running.await.unwrap().is_err());
    let operation = operations.lock().unwrap()[0].clone();
    assert!(engine.store.context_receipt(&operation).unwrap().is_none());
    assert_eq!(
        engine
            .store
            .context_model(&operation.origin)
            .unwrap()
            .as_deref(),
        Some("model-old")
    );
    assert_eq!(requests.lock().unwrap().len(), 1);
}

struct ReloadingPolicy {
    asynchronous: Arc<std::sync::atomic::AtomicBool>,
    started: Arc<Notify>,
}
#[async_trait::async_trait]
impl Provider for ReloadingPolicy {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![
            json!({"type":"function","name":"pinned","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ]
    }
    fn tool_scheduling(&self, name: &str) -> ToolScheduling {
        if name == "pinned" && !self.asynchronous.load(std::sync::atomic::Ordering::SeqCst) {
            ToolScheduling::BeforeNextInference
        } else {
            ToolScheduling::Async
        }
    }
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.started.notify_one();
        Ok(json!({"done":true}))
    }
}
struct PolicyStream {
    asynchronous: Arc<std::sync::atomic::AtomicBool>,
    emitted: Arc<Notify>,
    finish: Arc<Notify>,
    count: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl ResponsesTransport for PolicyStream {
    async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        unreachable!()
    }
    async fn create_streaming(
        &self,
        request: ResponsesRequest,
        sink: mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        if self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
            return Ok(final_turn());
        }
        assert_eq!(
            request
                .tools
                .iter()
                .find(|tool| tool["name"] == "pinned")
                .unwrap()["async"],
            false
        );
        self.asynchronous
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let item = call("pinned-call", "pinned");
        sink.send(StreamEvent::ItemDone(item.clone()))
            .await
            .unwrap();
        self.emitted.notify_one();
        self.finish.notified().await;
        Ok(turn(vec![item]))
    }
}
#[tokio::test]
async fn scheduling_reload_during_stream_keeps_issuing_typed_policy() {
    let asynchronous = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let emitted = Arc::new(Notify::new());
    let finish = Arc::new(Notify::new());
    let started = Arc::new(Notify::new());
    let engine = Engine::<Offline, _, _>::with_transport(
        PolicyStream {
            asynchronous: asynchronous.clone(),
            emitted: emitted.clone(),
            finish: finish.clone(),
            count: std::sync::atomic::AtomicUsize::new(0),
        },
        Arc::new(Store::memory().unwrap()),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(ReloadingPolicy {
            asynchronous,
            started: started.clone(),
        }),
        EngineConfig {
            instructions: "test".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "reload".into(),
            agent: AgentPath("/root".into()),
        },
    );
    let (_cancel, cancelled) = watch::channel(false);
    let running = tokio::spawn(async move { engine.run(None, vec![], cancelled, mailbox()).await });
    emitted.notified().await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), started.notified())
            .await
            .is_err()
    );
    finish.notify_one();
    started.notified().await;
    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn refused_context_commit_aborts_publication_and_recovers_without_releasing_children() {
    let (mut engine, requests, operations) = context_engine(false, false);
    Arc::get_mut(&mut engine.provider).unwrap().refuse_draft = true;
    let (_cancel, cancelled) = watch::channel(false);
    assert!(
        engine
            .run(None, vec![], cancelled, mailbox())
            .await
            .is_err()
    );
    let operation = operations.lock().unwrap()[0].clone();
    assert!(engine.store.context_receipt(&operation).unwrap().is_none());
    assert_eq!(
        engine
            .provider
            .committed
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        engine
            .provider
            .aborted
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    // Replace the scheduler to exercise durable gating after process loss.
    let recovered = Engine::<Offline, _, _>::with_transport(
        ContextScript {
            requests: requests.clone(),
            two_calls: false,
        },
        engine.store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        engine.provider.clone(),
        engine.config.clone(),
    );
    let (_cancel, cancelled) = watch::channel(false);
    recovered
        .run_recovering(Some(operation.request), vec![], cancelled, mailbox())
        .await
        .unwrap();
    assert_eq!(operations.lock().unwrap().len(), 1);
    assert_eq!(
        engine
            .provider
            .committed
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        engine
            .provider
            .aborted
            .load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    assert_eq!(requests.lock().unwrap()[1].model, "model-old");
}

#[tokio::test]
async fn sync_call_missing_from_final_response_is_refused_without_execution() {
    let (mut engine, mut starts, emitted, finish, _) = engine(false);
    Arc::get_mut(&mut engine).unwrap().client.omit_sync = true;
    let (_cancel, cancelled) = watch::channel(false);
    let running = tokio::spawn(async move { engine.run(None, vec![], cancelled, mailbox()).await });
    emitted.notified().await;
    assert_eq!(starts.recv().await.unwrap(), "async_work");
    finish.notify_one();
    assert!(matches!(
        running.await.unwrap(),
        Err(EngineError::InvalidFunctionCall)
    ));
    assert!(starts.try_recv().is_err());
}

#[tokio::test]
async fn round_cancellation_while_store_is_locked_refuses_context_publication() {
    let (engine, _, _) = context_engine(false, false);
    let request = RequestId("blocked-context-publication".into());
    let call = call("blocked", "edit_first");
    engine
        .store
        .write_request(
            &request,
            None,
            "/root",
            &[Item::configuration_update(Effort::Low), call],
            StoredUsage::default(),
        )
        .unwrap();
    engine
        .store
        .initialize_context_model(&engine.origin, "model-old")
        .unwrap();
    let operation = OperationId {
        origin: engine.origin.clone(),
        request: request.clone(),
        call: CallId("blocked".into()),
    };
    engine.store.claim_operation(&operation, &request).unwrap();
    let snapshot = engine.store.begin_context(&operation, &request).unwrap();
    let mut document = snapshot.document.clone();
    document.blocks.push(crate::context::ContextBlock::Text {
        reference: None,
        role: crate::context::ContextRole::Assistant,
        text: "must roll back".into(),
        sources: vec![],
    });
    let draft = crate::context::ContextDraft {
        document,
        next_model: Some("model-next".into()),
    };
    let (cancel, cancelled) = watch::channel(false);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let lock = engine.store.lock();
    let store = engine.store.clone();
    let publication = tokio::task::spawn_blocking(move || {
        entered.send(()).unwrap();
        store.commit_context_guarded(
            crate::context::ContextCommit {
                snapshot: &snapshot,
                draft: &draft,
                output: &crate::turn::JobOutput::Completed(Ok(json!({"done":true}))),
                pending: &[],
            },
            || *cancelled.borrow() || cancelled.has_changed().is_err(),
        )
    });
    ready.await.unwrap();
    cancel.send(true).unwrap();
    drop(lock);
    assert!(matches!(
        publication.await.unwrap(),
        Err(StoreError::Context(crate::context::ContextError::Cancelled))
    ));
    assert!(engine.store.context_receipt(&operation).unwrap().is_none());
    let state = engine
        .store
        .context_request_state(&request, &engine.origin)
        .unwrap();
    assert_eq!(state.model.as_deref(), Some("model-old"));
    assert_eq!(state.generation, 0);
    assert!(
        !state
            .history
            .iter()
            .any(|(_, _, item)| item.0["content"] == "must roll back")
    );
}

#[tokio::test]
async fn queued_sync_identity_is_claimable_but_foreign_output_waits_for_store_publication() {
    let (engine, mut starts, _, _, _) = engine(false);
    let request = RequestId("queued-publication".into());
    let item = call("queued", "async_work");
    engine
        .store
        .write_request(&request, None, "/root", &[item], StoredUsage::default())
        .unwrap();
    let operation = OperationId {
        origin: engine.origin.clone(),
        request: request.clone(),
        call: CallId("queued".into()),
    };
    engine.store.claim_operation(&operation, &request).unwrap();
    engine
        .scheduler
        .queue_operation(
            engine.provider.clone(),
            operation.clone(),
            engine.config.agent.clone(),
            Some(request.clone()),
            "async_work".into(),
            json!({}),
        )
        .await
        .unwrap();
    let foreign = engine
        .store
        .standalone_identity(AgentPath("/root/child".into()));
    assert_eq!(
        engine
            .scheduler
            .fork_claim_exact(&operation, foreign.clone(), true)
            .await
            .unwrap(),
        None
    );
    assert!(starts.try_recv().is_err());
    let (_mailbox, mut inbox) = mpsc::unbounded_channel::<crate::mailbox::Envelope>();
    let (_cancel, mut cancelled) = watch::channel(false);
    let outstanding = [operation.clone()];
    let waiting = crate::turn::wait_agent_and_drain_exact(
        &mut inbox,
        &engine.scheduler,
        &mut cancelled,
        &outstanding,
    );
    tokio::pin!(waiting);
    assert!(futures_util::poll!(&mut waiting).is_pending());
    let mut raw_settlements = engine.scheduler.operation_settlements();
    engine
        .scheduler
        .release_operation(&operation, None)
        .await
        .unwrap();
    assert_eq!(starts.recv().await.unwrap(), "async_work");
    let terminal = engine.scheduler.wait(&operation).await.unwrap();
    assert_eq!(raw_settlements.recv().await.unwrap(), operation);
    assert!(
        futures_util::poll!(&mut waiting).is_pending(),
        "raw completion cannot wake a foreign wait before Store publication"
    );
    assert_eq!(engine.scheduler.output(&operation).await.unwrap(), None);
    assert_eq!(
        engine
            .scheduler
            .fork_claim_exact(&operation, foreign.clone(), true)
            .await
            .unwrap(),
        None
    );
    engine
        .store
        .write_job_output(&operation, ToolKind::Function, &terminal)
        .unwrap();
    engine
        .scheduler
        .mark_output_committed(&operation)
        .await
        .unwrap();
    let drained = waiting.await.unwrap();
    assert_eq!(
        drained.resumed_by,
        crate::turn::WaitResumeExact::Job(operation.clone())
    );
    assert_eq!(
        drained.call_outputs,
        vec![(operation.clone(), terminal.clone())]
    );
    assert_eq!(
        engine.scheduler.output(&operation).await.unwrap(),
        Some(terminal.clone())
    );
    assert_eq!(
        engine
            .scheduler
            .fork_claim_exact(&operation, foreign, true)
            .await
            .unwrap(),
        Some(terminal)
    );
}

struct FinalOnly {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
}
#[async_trait::async_trait]
impl ResponsesTransport for FinalOnly {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.requests.lock().unwrap().push(request);
        Ok(final_turn())
    }
}

#[tokio::test]
async fn child_final_completes_while_parent_sync_waits_for_child() {
    let (mut parent, _, operations) = context_engine(false, false);
    let started = Arc::new(Notify::new());
    let child_final = Arc::new(Notify::new());
    Arc::get_mut(&mut parent.provider).unwrap().paused =
        Some((started.clone(), child_final.clone()));
    let parent = Arc::new(parent);
    let (_parent_cancel, cancelled) = watch::channel(false);
    let running_parent = {
        let parent = parent.clone();
        tokio::spawn(async move { parent.run(None, vec![], cancelled, mailbox()).await })
    };
    started.notified().await;
    let operation = operations.lock().unwrap()[0].clone();
    let child_path = AgentPath("/root/child".into());
    let snapshot = RequestId("child-pending-sync-snapshot".into());
    parent
        .store
        .admit_agent(
            &AgentPath("/root".into()),
            None,
            Some(&operation.request),
            &json!({}),
            &json!({"kind":"root"}),
        )
        .unwrap();
    parent
        .store
        .admit_here_agent_with_snapshot(
            &child_path,
            &AgentPath("/root".into()),
            &snapshot,
            &json!({}),
            "/root",
            "/root/child",
            "AtBoundary",
            &Item(json!({"type":"message","role":"assistant","content":"NEW_TASK"})),
        )
        .unwrap();
    let child_requests = Arc::new(Mutex::new(Vec::new()));
    let child = Engine::<Offline, ContextEditor, FinalOnly>::with_transport(
        FinalOnly {
            requests: child_requests.clone(),
        },
        parent.store.clone(),
        parent.scheduler.clone(),
        parent.provider.clone(),
        EngineConfig {
            instructions: "child".into(),
            tools: vec![],
            model: "model-old".into(),
            effort: Effort::Low,
            session_id: "child-sync-tests".into(),
            agent: child_path,
        },
    );
    let (_child_cancel, cancelled) = watch::channel(false);
    let mut completed = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        child.run(Some(snapshot.clone()), vec![], cancelled, mailbox()),
    )
    .await
    .expect("child final must not wait for parent's synchronous invocation")
    .unwrap();
    assert!(is_final(&completed.turn, false));
    assert_eq!(parent.scheduler.output(&operation).await.unwrap(), None);
    let claims = parent.store.claims_for_operation(&operation).unwrap();
    assert_eq!(claims.len(), 2);
    assert!(
        claims
            .iter()
            .all(|claim| claim.state == crate::store::ClaimState::Pending)
    );
    assert!(claims.iter().any(|claim| claim.request == snapshot));
    child_final.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), running_parent)
        .await
        .expect("parent resumes when child finishes")
        .unwrap()
        .unwrap();
    assert!(parent.store.context_receipt(&operation).unwrap().is_some());
    assert!(
        parent
            .store
            .claims_for_operation(&operation)
            .unwrap()
            .iter()
            .all(|claim| claim.state == crate::store::ClaimState::Settled)
    );
    // Ordinary follow-up uses the returned head, whose claim attachment remains
    // on the earlier snapshot. The same-branch lineage owns that continuation.
    for _ in 0..2 {
        let (_cancel, cancelled) = watch::channel(false);
        let followed = child
            .run(
                Some(completed.head_request.clone()),
                vec![Item(
                    json!({"type":"message","role":"user","content":"continue"}),
                )],
                cancelled,
                mailbox(),
            )
            .await
            .unwrap();
        assert_eq!(
            followed
                .transcript
                .iter()
                .filter(|item| item.0["type"] == "function_call_output"
                    && item.0["call_id"] == operation.call.0)
                .count(),
            1
        );
        completed = followed;
        let requests = child_requests.lock().unwrap();
        assert_eq!(
            requests
                .last()
                .unwrap()
                .input
                .iter()
                .filter(|item| item.0["type"] == "function_call_output"
                    && item.0["call_id"] == operation.call.0)
                .count(),
            1
        );
    }
}
