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
}
struct Streaming {
    requests: Mutex<Vec<ResponsesRequest>>,
    emitted: Arc<Notify>,
    finish: Arc<Notify>,
    fail: bool,
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
        Ok(turn(vec![
            first,
            asynchronous,
            call("second", "sync_second"),
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
                .filter(|item| item.0["call_id"] == id && item.0["type"] == "function_call_output")
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
