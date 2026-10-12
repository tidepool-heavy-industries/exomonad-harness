//! Production submission boundaries, including cancellation and raw compaction facts.
use super::*;
use crate::store::{ProviderRequestPurpose, ProviderResponseState};
use crate::transport::{ResponsesProtocol, ResponsesRoute, sse::ResponseAssembly};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        unreachable!()
    }
}
struct NoEffects(AtomicUsize);
#[async_trait::async_trait]
impl Provider for NoEffects {
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, crate::provider::ProviderError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("compaction must not dispatch raw provider calls")
    }
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
}
struct Script {
    items: Vec<Item>,
    streaming: bool,
    cancel: Option<watch::Sender<bool>>,
    request: Arc<Mutex<Option<RequestId>>>,
}
#[async_trait::async_trait]
impl ResponsesTransport for Script {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::Codex, ResponsesProtocol::Standard))
    }
    async fn create(
        &self,
        request: PreparedProviderRequest,
    ) -> Result<ResponsesTurn, TransportError> {
        *self.request.lock().unwrap() = Some(request.request_id().clone());
        Ok(ResponsesTurn {
            response_id: "complete".into(),
            items: self.items.clone(),
            usage: Usage::default(),
        })
    }
    async fn create_streaming(
        &self,
        request: PreparedProviderRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        if !self.streaming {
            return self.create(request).await;
        }
        *self.request.lock().unwrap() = Some(request.request_id().clone());
        let mut assembly = ResponseAssembly::default();
        for (index, item) in self.items.iter().enumerate() {
            let event = assembly.accept(&json!({"type":"response.output_item.done", "output_index":index, "item":item.0}).to_string())?.unwrap();
            sink.send(event).await.unwrap();
        }
        if let Some(cancel) = &self.cancel {
            cancel.send(true).unwrap();
            std::future::pending::<()>().await;
        }
        assembly.accept(
            &json!({"type":"response.completed", "response":{"id":"streamed","output":self.items}})
                .to_string(),
        )?;
        assembly.finish()
    }
}
fn engine(
    store: Arc<Store>,
    script: Script,
    effects: Arc<NoEffects>,
) -> Engine<Offline, NoEffects, Script> {
    Engine::with_transport(
        script,
        store,
        Arc::new(JobScheduler::new(1).unwrap()),
        effects,
        EngineConfig {
            instructions: "private submission controls".into(),
            tools: vec![],
            model: "model".into(),
            effort: Effort::Low,
            session_id: "session".into(),
            agent: AgentPath("/test".into()),
        },
    )
}
fn raw_items() -> Vec<Item> {
    vec![
        Item(json!({"type":"configuration_update","reasoning_effort":"high"})),
        Item(json!({"type":"function_call","call_id":"raw-call","name":"never","arguments":"{}"})),
        Item(json!({"type":"function_call_output","call_id":"raw-call","output":"raw"})),
        Item(json!({"type":"additional_tools","tools":[]})),
        Item(json!({"type":"compaction_trigger"})),
        Item(
            json!({"type":"compaction","id":"compact","encrypted_content":"synthetic-test-content"}),
        ),
    ]
}
async fn completed_compaction(streaming: bool) {
    let store = Arc::new(Store::memory().unwrap());
    let source = RequestId("source".into());
    let child = RequestId("compactor".into());
    store.create_request(&source, None, "/test").unwrap();
    store
        .create_request(&child, Some(&source), "/test")
        .unwrap();
    let items = raw_items();
    let request = Arc::new(Mutex::new(None));
    let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
    let script = Script {
        items: items.clone(),
        streaming,
        cancel: None,
        request,
    };
    let selected = script.prepare_submission().unwrap();
    let prepared = store
        .prepare_provider_request(
            &source,
            &child,
            ResponsesRequest {
                input: vec![],
                instructions: "fixed".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "model".into(),
                pinned_effort: Effort::Low,
                session_id: "session".into(),
            },
            &[],
            None,
            selected,
            ProviderRequestPurpose::ServerCompaction,
        )
        .unwrap();
    let engine = engine(store.clone(), script, effects.clone());
    let (_cancel, receiver) = watch::channel(false);
    let turn = engine
        .execute_compaction_submission(prepared, receiver, &source)
        .await
        .unwrap();
    assert_eq!(turn.items, items);
    assert_eq!(store.items(&child).unwrap(), items);
    assert_eq!(
        store.provider_response_state(&child).unwrap(),
        Some(ProviderResponseState::Completed)
    );
    assert!(store.claims_on(&child).unwrap().is_empty());
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
    let c = store.lock().unwrap();
    let indexes = c
        .prepare(
            "SELECT output_index FROM provider_items WHERE request_id=?1 ORDER BY output_index",
        )
        .unwrap()
        .query_map([&child.0], |row| row.get::<_, u64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(indexes, (0..items.len() as u64).collect::<Vec<_>>());
    assert_eq!(
        items
            .iter()
            .filter(|item| crate::compaction::server_keeps(item))
            .count(),
        1
    );
}
#[tokio::test]
async fn complete_compaction_retains_raw_facts_without_effect_authority() {
    completed_compaction(false).await;
}
#[tokio::test]
async fn streamed_compaction_retains_raw_facts_without_effect_authority() {
    completed_compaction(true).await;
}

#[tokio::test]
async fn cancelled_compaction_prefix_is_interrupted_after_reopen() {
    let path = std::env::temp_dir().join(format!(
        "prepared-compactor-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let store = Arc::new(Store::open(&path).unwrap());
    let source = RequestId("source".into());
    let child = RequestId("compactor".into());
    store.create_request(&source, None, "/test").unwrap();
    store
        .create_request(&child, Some(&source), "/test")
        .unwrap();
    let items = raw_items();
    let (cancel, receiver) = watch::channel(false);
    let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
    let script = Script {
        items: items.clone(),
        streaming: true,
        cancel: Some(cancel),
        request: Arc::new(Mutex::new(None)),
    };
    let selected = script.prepare_submission().unwrap();
    let prepared = store
        .prepare_provider_request(
            &source,
            &child,
            ResponsesRequest {
                input: vec![],
                instructions: "fixed".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "model".into(),
                pinned_effort: Effort::Low,
                session_id: "session".into(),
            },
            &[],
            None,
            selected,
            ProviderRequestPurpose::ServerCompaction,
        )
        .unwrap();
    let engine = engine(store.clone(), script, effects.clone());
    let error = engine
        .execute_compaction_submission(prepared, receiver, &source)
        .await
        .unwrap_err();
    assert!(
        matches!(error, EngineError::Cancelled { head_request: Some(ref head) } if head == &source)
    );
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
    drop(engine);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.items(&child).unwrap(), items);
    assert_eq!(
        reopened.provider_response_state(&child).unwrap(),
        Some(ProviderResponseState::Interrupted)
    );
    assert!(reopened.replay_turns(&source).unwrap().is_empty());
    assert!(reopened.claims_on(&child).unwrap().is_empty());
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn cancelled_inference_retains_buffered_prefix_after_reopen() {
    let path = std::env::temp_dir().join(format!(
        "prepared-inference-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let store = Arc::new(Store::open(&path).unwrap());
    let items = vec![Item(
        json!({"type":"reasoning","id":"prefix","summary":[],"encrypted_content":"synthetic-test-content"}),
    )];
    let (cancel, receiver) = watch::channel(false);
    let request = Arc::new(Mutex::new(None));
    let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
    let engine = engine(
        store.clone(),
        Script {
            items: items.clone(),
            streaming: true,
            cancel: Some(cancel),
            request: request.clone(),
        },
        effects,
    );
    let (_incoming, incoming) = tokio::sync::mpsc::unbounded_channel();
    let error = engine
        .run(
            None,
            vec![Item(
                json!({"type":"message","role":"user","content":"start"}),
            )],
            receiver,
            incoming,
        )
        .await
        .unwrap_err();
    let head = request.lock().unwrap().clone().unwrap();
    assert!(
        matches!(error, EngineError::Cancelled { head_request: Some(ref actual) } if actual == &head)
    );
    drop(engine);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert!(reopened.items(&head).unwrap().ends_with(&items));
    assert_eq!(
        reopened.provider_response_state(&head).unwrap(),
        Some(ProviderResponseState::Interrupted)
    );
    assert!(reopened.replay_turns(&head).unwrap().is_empty());
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}
