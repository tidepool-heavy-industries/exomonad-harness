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
    // A host-declared synchronous tool remains synchronous even when the
    // provider returns async=true; ordinary tools retain their explicit mode.
    Item(json!({"type":"function_call","call_id":id,"name":name,"async":true,"arguments":"{}"}))
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
    cancelled_receipt: bool,
    refuse_draft: bool,
    summarize_completed: bool,
    effort_only: bool,
    committed: std::sync::atomic::AtomicUsize,
    aborted: std::sync::atomic::AtomicUsize,
    paused: Option<(Arc<Notify>, Arc<Notify>)>,
    completion_owner: Option<Arc<CompletedContextOwner>>,
}

struct CompletedContextOwner {
    ready: Notify,
    release_waiter: Notify,
    register_source: bool,
    unedited: bool,
}

#[async_trait::async_trait]
impl crate::provider::CancellationOwner for CompletedContextOwner {
    async fn cancel(
        &self,
        _: &OperationId,
        _: &crate::provider::JobHandle,
    ) -> crate::provider::CancellationAcknowledgment {
        crate::provider::CancellationAcknowledgment::Completed(Ok(json!({"done":"edit_first"})))
    }
}

struct RetainedContextCompletion(crate::provider::ProviderCompletion);
impl crate::provider::InvocationCompletionSource for RetainedContextCompletion {
    fn completion(
        &self,
        output: crate::turn::JobOutput,
    ) -> Option<crate::provider::ProviderCompletion> {
        let mut completion = self.0.clone();
        completion.output = output;
        Some(completion)
    }
}
#[async_trait::async_trait]
impl Provider for ContextEditor {
    fn cancellation_owner(&self) -> Option<Arc<dyn crate::provider::CancellationOwner>> {
        self.completion_owner
            .clone()
            .map(|owner| owner as Arc<dyn crate::provider::CancellationOwner>)
    }
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
        if self.summarize_completed && name == "edit_second" {
            let mut sources = Vec::new();
            document.blocks.retain(|block| {
                if let crate::context::ContextBlock::Native {
                    reference,
                    kind: crate::context::ContextNativeKind::CompletedExchange,
                    ..
                } = block
                {
                    sources.push(reference.clone());
                    false
                } else {
                    true
                }
            });
            assert!(
                !sources.is_empty(),
                "completed first exchange available to summarize"
            );
            document.blocks.push(crate::context::ContextBlock::Text {
                reference: None,
                role: crate::context::ContextRole::Assistant,
                text: "summarized completed exchange".into(),
                sources,
            });
        }
        if !self.effort_only {
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
        }
        self.operations
            .lock()
            .unwrap()
            .push(snapshot.operation.clone());
        if let Some((started, release)) = &self.paused {
            started.notify_one();
            release.notified().await;
        }
        let mut completion = crate::provider::ProviderCompletion {
            finalization: crate::provider::FinalizationResponsibility::provider(),
            output: if self.cancelled_receipt {
                crate::turn::JobOutput::CancelledWithReceipt(Ok(json!({"prefix":name})))
            } else {
                crate::turn::JobOutput::Completed(Ok(json!({"done":name})))
            },
            full_success: self.full_success,
            context: crate::provider::ContextDisposition::Draft(crate::context::ContextDraft {
                next_effort: self.effort_only.then_some(Effort::High),
                document,
                next_model: if self.effort_only {
                    None
                } else if self.refuse_draft {
                    Some(String::new())
                } else {
                    (name == "edit_first").then(|| "model-next".into())
                },
            }),
        };
        if let Some(owner) = &self.completion_owner {
            if owner.unedited {
                completion.context = crate::provider::ContextDisposition::Unedited;
            }
            if owner.register_source {
                context
                    .completion
                    .register(
                        &snapshot.operation,
                        Arc::new(RetainedContextCompletion(completion.clone())),
                    )
                    .unwrap();
            }
            owner.ready.notify_one();
            owner.release_waiter.notified().await;
            // The cancellation owner's immutable metadata is already retained.
            // A later ordinary waiter must not replace that winning carrier.
            return crate::provider::ProviderCompletion::unedited(Ok(json!({"late":"waiter"})));
        }
        completion
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
    opaque: bool,
    finalize_calls: Option<Vec<Item>>,
}

fn completed_context_owner(register_source: bool) -> Arc<CompletedContextOwner> {
    Arc::new(CompletedContextOwner {
        ready: Notify::new(),
        release_waiter: Notify::new(),
        register_source,
        unedited: false,
    })
}

#[tokio::test]
async fn completed_operation_cancellation_retains_context_before_delayed_provider_waiter() {
    let (mut engine, requests, operations) = context_engine(false, false);
    let owner = completed_context_owner(true);
    Arc::get_mut(&mut engine.provider).unwrap().completion_owner = Some(owner.clone());
    let engine = Arc::new(engine);
    let (_cancel, cancelled) = watch::channel(false);
    let running = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.run(None, vec![], cancelled, mailbox()).await })
    };
    owner.ready.notified().await;
    let operation = operations.lock().unwrap()[0].clone();
    engine.scheduler.cancel(&operation).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), running)
        .await
        .expect("owner completion must not wait for the provider waiter")
        .unwrap()
        .unwrap();
    assert!(engine.store.context_receipt(&operation).unwrap().is_some());
    assert_eq!(requests.lock().unwrap()[1].model, "model-next");
    assert!(requests.lock().unwrap()[1].input.iter().any(|item| {
        item.0["content"]
            .as_str()
            .is_some_and(|text| text.ends_with("first commit"))
    }));
    assert_eq!(
        engine
            .provider
            .committed
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let winning = engine
        .scheduler
        .invocation_completion(&operation)
        .await
        .unwrap()
        .unwrap();
    assert!(winning.full_success);
    assert!(matches!(
        winning.context,
        crate::provider::ContextDisposition::Draft(_)
    ));
    assert_eq!(
        winning.output,
        crate::turn::JobOutput::Completed(Ok(json!({"done":"edit_first"})))
    );
    owner.release_waiter.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while engine
            .scheduler
            .provider_completion(&operation)
            .await
            .unwrap()
            .is_none()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        engine
            .scheduler
            .invocation_completion(&operation)
            .await
            .unwrap(),
        Some(winning)
    );
}

#[tokio::test]
async fn round_cancellation_aborts_completed_owner_context_before_delayed_provider_waiter() {
    round_cancellation_aborts_completed_owner(false).await;
}

#[tokio::test]
async fn round_cancellation_aborts_unedited_sync_owner_before_store_publication() {
    round_cancellation_aborts_completed_owner(true).await;
}

async fn round_cancellation_aborts_completed_owner(unedited: bool) {
    let (mut engine, _, operations) = context_engine(false, false);
    let mut owner = completed_context_owner(true);
    Arc::get_mut(&mut owner).unwrap().unedited = unedited;
    Arc::get_mut(&mut engine.provider).unwrap().completion_owner = Some(owner.clone());
    let engine = Arc::new(engine);
    let (cancel, cancelled) = watch::channel(false);
    let running = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.run(None, vec![], cancelled, mailbox()).await })
    };
    owner.ready.notified().await;
    let operation = operations.lock().unwrap()[0].clone();
    cancel.send(true).unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), running)
        .await
        .expect("round cleanup must not wait for the ordinary provider waiter")
        .unwrap();
    assert!(matches!(result, Err(EngineError::Cancelled { .. })));
    assert!(engine.store.context_receipt(&operation).unwrap().is_none());
    assert_eq!(
        engine.store.context_model(&operation.origin).unwrap(),
        Some("model-old".into())
    );
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
    assert!(
        engine
            .store
            .events(Some(&operation.request))
            .unwrap()
            .iter()
            .any(|event| event.kind == "context_disposition")
    );
    owner.release_waiter.notify_one();
}

#[tokio::test]
async fn completed_owner_without_required_context_metadata_aborts_publication_promptly() {
    let (mut engine, requests, operations) = context_engine(false, false);
    let owner = completed_context_owner(false);
    Arc::get_mut(&mut engine.provider).unwrap().completion_owner = Some(owner.clone());
    let engine = Arc::new(engine);
    let (_cancel, cancelled) = watch::channel(false);
    let running = {
        let engine = engine.clone();
        tokio::spawn(async move { engine.run(None, vec![], cancelled, mailbox()).await })
    };
    owner.ready.notified().await;
    let operation = operations.lock().unwrap()[0].clone();
    engine.scheduler.cancel(&operation).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), running)
        .await
        .expect("missing metadata must not hang cancellation")
        .unwrap()
        .unwrap();
    assert!(engine.store.context_receipt(&operation).unwrap().is_none());
    assert_eq!(requests.lock().unwrap()[1].model, "model-old");
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
    let completion = engine
        .scheduler
        .invocation_completion(&operation)
        .await
        .unwrap()
        .unwrap();
    assert!(!completion.full_success);
    assert!(matches!(
        completion.context,
        crate::provider::ContextDisposition::Unavailable
    ));
    assert!(
        engine
            .store
            .events(Some(&operation.request))
            .unwrap()
            .iter()
            .any(|event| event.kind == "context_disposition")
    );
    owner.release_waiter.notify_one();
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
            let mut items = vec![];
            if self.opaque {
                items.push(Item(json!({"type":"reasoning","encrypted_content":"model-old-only","summary":[{"type":"summary_text","text":"visible plan"}]})));
            }
            items.push(call("edit-first", "edit_first"));
            if self.two_calls {
                items.push(call("edit-second", "edit_second"));
            }
            if let Some(completion) = &self.finalize_calls {
                items.extend(completion.clone());
            }
            Ok(turn(items))
        } else if self.finalize_calls.is_some() {
            Ok(turn(vec![context_finalize(
                "after-edit-final",
                json!({"answer":"ready"}),
            )]))
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
            opaque: false,
            finalize_calls: None,
        },
        store.clone(),
        Arc::new(JobScheduler::new(2).unwrap()),
        Arc::new(ContextEditor {
            store,
            operations: operations.clone(),
            lose_ack: std::sync::atomic::AtomicBool::new(lose_ack),
            full_success: true,
            cancelled_receipt: false,
            refuse_draft: false,
            summarize_completed: false,
            effort_only: false,
            committed: std::sync::atomic::AtomicUsize::new(0),
            aborted: std::sync::atomic::AtomicUsize::new(0),
            paused: None,
            completion_owner: None,
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
async fn opaque_issuing_sync_cell_switches_and_seals_only_portable_next_input() {
    let (mut engine, requests, operations) = context_engine(false, false);
    engine.client.opaque = true;
    let (_cancel, cancelled) = watch::channel(false);
    let completion = engine
        .run(None, vec![], cancelled, mailbox())
        .await
        .unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].model, "model-old");
    assert_eq!(requests[1].model, "model-next");
    assert!(
        !serde_json::to_string(&requests[1].input)
            .unwrap()
            .contains("model-old-only")
    );
    let note = requests[1]
        .input
        .iter()
        .find_map(|item| {
            item.0["content"]
                .as_str()
                .filter(|text| text.starts_with("[Store-generated model portability note"))
        })
        .unwrap();
    for visible in ["visible plan", "edit_first", "edit-first", "done"] {
        assert!(note.contains(visible), "missing {visible}: {note}");
    }
    assert!(requests[1].input.iter().any(|i| {
        i.0["content"]
            .as_str()
            .is_some_and(|t| t.ends_with("first commit"))
    }));
    let operation = operations.lock().unwrap()[0].clone();
    assert!(engine.store.context_receipt(&operation).unwrap().is_some());
    let raw = engine
        .store
        .context_history(&completion.head_request)
        .unwrap();
    assert!(
        raw.iter()
            .any(|(_, _, i)| i.0["encrypted_content"] == "model-old-only")
    );
    assert_eq!(
        raw.iter()
            .filter(
                |(_, _, i)| i.0["type"] == "function_call_output" && i.0["call_id"] == "edit-first"
            )
            .count(),
        1
    );
    let replay = engine.store.replay_turns(&operation.request).unwrap();
    assert_eq!(replay[1].model_request.input, requests[1].input);
    assert_eq!(engine.provider.operations.lock().unwrap().len(), 1);
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
async fn typed_cancelled_receipt_cannot_publish_staged_context_or_model() {
    let (mut engine, requests, operations) = context_engine(false, false);
    Arc::get_mut(&mut engine.provider)
        .unwrap()
        .cancelled_receipt = true;
    let (_cancel, cancelled) = watch::channel(false);
    engine
        .run(None, vec![], cancelled, mailbox())
        .await
        .unwrap();
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
        engine.scheduler.output(&operation).await.unwrap(),
        Some(crate::turn::JobOutput::CancelledWithReceipt(Ok(
            json!({"prefix":"edit_first"})
        ))),
    );
    assert!(
        !engine
            .scheduler
            .invocation_completion(&operation)
            .await
            .unwrap()
            .unwrap()
            .full_success
    );
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
            opaque: false,
            finalize_calls: None,
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
        next_effort: None,
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
async fn cross_model_child_first_request_projects_parent_evidence_without_reexecuting_calls() {
    let (mut parent, _, operations) = context_engine(false, false);
    parent.client.opaque = true;
    let (_cancel, cancelled) = watch::channel(false);
    let completed = parent
        .run(None, vec![], cancelled, mailbox())
        .await
        .unwrap();
    let child_path = AgentPath("/root/portable_child".into());
    let snapshot = RequestId("portable-child-snapshot".into());
    parent
        .store
        .admit_agent(
            &AgentPath("/root".into()),
            None,
            Some(&completed.head_request),
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
            &child_path.0,
            "AtBoundary",
            &Item(json!({"type":"message","role":"user","content":"child task"})),
        )
        .unwrap();
    let raw = parent.store.context_history(&snapshot).unwrap();
    let child_requests = Arc::new(Mutex::new(vec![]));
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
            model: "model-next".into(),
            effort: Effort::Low,
            session_id: "portable-child".into(),
            agent: child_path,
        },
    );
    let (_cancel, cancelled) = watch::channel(false);
    child
        .run(Some(snapshot.clone()), vec![], cancelled, mailbox())
        .await
        .unwrap();
    let sent = child_requests.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].model, "model-next");
    let note = sent[0]
        .input
        .iter()
        .find_map(|item| {
            item.0["content"]
                .as_str()
                .filter(|text| text.starts_with("[Store-generated model portability note"))
        })
        .unwrap();
    assert!(note.contains("visible plan") && note.contains("edit_first") && note.contains("done"));
    assert!(
        !serde_json::to_string(&sent[0].input)
            .unwrap()
            .contains("model-old-only")
    );
    assert_eq!(parent.store.context_history(&snapshot).unwrap(), raw);
    assert!(
        raw.iter()
            .any(|(_, _, i)| i.0["encrypted_content"] == "model-old-only")
    );
    assert_eq!(operations.lock().unwrap().len(), 1);
    let replay = parent.store.replay_turns(&snapshot).unwrap();
    assert_eq!(replay[0].model_request.input, sent[0].input);
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

struct ContextNotesScript {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
}
#[async_trait::async_trait]
impl ResponsesTransport for ContextNotesScript {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        Ok(match index {
            1 => turn(vec![call("edit-first", "edit_first")]),
            2 => turn(vec![call("edit-second", "edit_second")]),
            _ => final_turn(),
        })
    }
}

#[tokio::test]
async fn completed_own_exchange_notes_do_not_reattach_on_ordinary_followup() {
    let (mut original, requests, _) = context_engine(false, false);
    Arc::get_mut(&mut original.provider)
        .unwrap()
        .summarize_completed = true;
    let engine = Engine::<Offline, ContextEditor, ContextNotesScript>::with_transport(
        ContextNotesScript {
            requests: requests.clone(),
        },
        original.store,
        original.scheduler,
        original.provider,
        original.config,
    );
    let (_cancel, cancelled) = watch::channel(false);
    let completed = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        engine.run(None, vec![], cancelled, mailbox()),
    )
    .await
    .expect("separate completed exchange can be summarized")
    .unwrap();
    assert_eq!(
        engine
            .provider
            .committed
            .load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    assert!(
        !completed
            .transcript
            .iter()
            .any(|item| item.0["type"] == "function_call" && item.0["call_id"] == "edit-first")
    );
    let (_cancel, cancelled) = watch::channel(false);
    engine
        .run(
            Some(completed.head_request),
            vec![Item(json!({
                "type":"message", "role":"user", "content":"continue"
            }))],
            cancelled,
            mailbox(),
        )
        .await
        .unwrap();
    assert_eq!(
        engine
            .provider
            .committed
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "ordinary followup must not acknowledge an old local boundary again"
    );
    assert!(
        requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .input
            .iter()
            .any(|item| item.0["content"]
                .as_str()
                .is_some_and(|text| text.ends_with("summarized completed exchange")))
    );
}

#[tokio::test]
async fn staged_effort_preserves_the_issued_request_prefix_and_changes_the_next_inference() {
    use crate::transport::{ResponsesProtocol, client::request_body_for_protocol};
    let (mut engine, requests, operations) = context_engine(false, false);
    Arc::get_mut(&mut engine.provider).unwrap().effort_only = true;
    let (_cancel, cancelled) = watch::channel(false);
    engine
        .run(
            None,
            vec![Item(
                json!({"type":"message","role":"user","content":"keep this context"}),
            )],
            cancelled,
            mailbox(),
        )
        .await
        .unwrap();
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].model, sent[1].model);
    assert_eq!(sent[0].pinned_effort, Effort::Low);
    assert_eq!(sent[1].pinned_effort, Effort::Low);
    assert_eq!(&sent[1].input[..sent[0].input.len()], &sent[0].input);
    assert_eq!(
        sent[1].input.last(),
        Some(&Item::configuration_update(Effort::High))
    );
    assert_eq!(
        sent[1].input[sent[1].input.len() - 2].0["type"],
        "function_call_output"
    );
    let first = request_body_for_protocol(&sent[0], ResponsesProtocol::Standard).unwrap();
    let second = request_body_for_protocol(&sent[1], ResponsesProtocol::Standard).unwrap();
    let first_prefix = serde_json::to_vec(&first["input"]).unwrap();
    let second_prefix = serde_json::to_vec(
        &second["input"].as_array().unwrap()[..first["input"].as_array().unwrap().len()],
    )
    .unwrap();
    assert_eq!(first_prefix, second_prefix);
    assert_eq!(first["reasoning"], second["reasoning"]);
    assert_eq!(first["instructions"], second["instructions"]);
    assert_eq!(first["tools"], second["tools"]);
    assert_eq!(first["prompt_cache_key"], second["prompt_cache_key"]);
    assert_eq!(
        second["input"].as_array().unwrap().last().unwrap()["reasoning"]["effort"],
        "high"
    );
    let lite = request_body_for_protocol(&sent[1], ResponsesProtocol::Lite).unwrap();
    assert_eq!(lite["reasoning"]["effort"], "high");
    let operation = operations.lock().unwrap()[0].clone();
    let receipt = engine.store.context_receipt(&operation).unwrap().unwrap();
    assert_eq!(receipt.head, operation.request);
    assert_eq!(receipt.model.as_deref(), Some("model-old"));
    assert!(receipt.changed);
    assert_eq!(receipt.generation, 1);
}

#[tokio::test]
async fn failed_or_cancelled_cell_keeps_the_next_inference_effort_unchanged() {
    for cancelled_receipt in [false, true] {
        let (mut engine, requests, operations) = context_engine(false, false);
        let provider = Arc::get_mut(&mut engine.provider).unwrap();
        provider.effort_only = true;
        provider.full_success = false;
        provider.cancelled_receipt = cancelled_receipt;
        let (_cancel, cancelled) = watch::channel(false);
        engine
            .run(None, vec![], cancelled, mailbox())
            .await
            .unwrap();
        let sent = requests.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[1]
                .input
                .iter()
                .rev()
                .find_map(Item::configuration_effort),
            Some(Effort::Low)
        );
        assert_eq!(sent[1].pinned_effort, Effort::Low);
        let operation = operations.lock().unwrap()[0].clone();
        assert!(engine.store.context_receipt(&operation).unwrap().is_none());
    }
}

fn context_finalize(id: &str, result: serde_json::Value) -> Item {
    Item(
        json!({"type":"function_call","call_id":id,"name":"finalize","arguments":{"result":result}}),
    )
}

#[derive(Debug, PartialEq, serde::Deserialize, schemars::JsonSchema)]
struct ContextFinalReply {
    answer: String,
}

#[tokio::test]
async fn validated_finalize_in_a_sync_edit_response_preserves_context_publication() {
    let (mut engine, requests, operations) = context_engine(false, false);
    engine.client.opaque = true;
    engine.client.finalize_calls = Some(vec![context_finalize(
        "first-final",
        json!({"answer":"provisional"}),
    )]);
    let (_cancel, cancelled) = watch::channel(false);
    let (completion, reply) = engine
        .run_finalized::<ContextFinalReply>(None, vec![], cancelled, mailbox())
        .await
        .unwrap();
    assert_eq!(reply.answer, "ready");
    let operation = operations.lock().unwrap()[0].clone();
    let receipt = engine
        .store
        .context_receipt(&operation)
        .unwrap()
        .expect("mixed response must publish the valid edit");
    assert!(receipt.changed);
    assert_eq!(receipt.model.as_deref(), Some("model-next"));
    assert!(completion.transcript.iter().any(|item| {
        item.0["content"]
            .as_str()
            .is_some_and(|text| text.ends_with("first commit"))
    }));
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].model, "model-next");
    assert!(
        sent[1]
            .input
            .iter()
            .any(|item| item.0["content"].as_str().is_some_and(|text| text
                .starts_with("[Store-generated model portability note")
                && text.contains("first-final")))
    );
    assert!(
        !serde_json::to_string(&sent[1].input)
            .unwrap()
            .contains("model-old-only")
    );
    assert!(
        engine
            .store
            .claims(&CallId("first-final".into()))
            .unwrap()
            .is_empty()
    );
    assert!(
        engine
            .store
            .claims(&CallId("after-edit-final".into()))
            .unwrap()
            .is_empty()
    );
    let events = engine.store.events(Some(&operation.request)).unwrap();
    let issued = events
        .iter()
        .find(|event| event.kind == "model_turn")
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&issued.payload).unwrap()["completion"]["kind"],
        "finalize"
    );
}

#[tokio::test]
async fn invalid_finalize_in_a_sync_edit_response_cannot_publish_or_mint_completion_evidence() {
    for calls in [
        vec![context_finalize("malformed-final", json!({"answer":7}))],
        vec![
            context_finalize("first-final", json!({"answer":"a"})),
            context_finalize("second-final", json!({"answer":"b"})),
        ],
    ] {
        let (mut engine, _, operations) = context_engine(false, false);
        engine.client.finalize_calls = Some(calls);
        let (_cancel, cancelled) = watch::channel(false);
        assert!(
            engine
                .run_finalized::<ContextFinalReply>(None, vec![], cancelled, mailbox())
                .await
                .is_err()
        );
        assert!(operations.lock().unwrap().is_empty());
        assert!(
            engine
                .store
                .events(None)
                .unwrap()
                .iter()
                .all(|event| event.kind != "context_commit")
        );
        for event in engine
            .store
            .events(None)
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "model_turn")
        {
            assert!(
                serde_json::from_str::<serde_json::Value>(&event.payload)
                    .unwrap()
                    .get("completion")
                    .is_none()
            );
        }
    }
}
