use async_trait::async_trait;
use harness::{
    engine::{Engine, EngineConfig, ResponsesTransport},
    item::Item,
    model::{AgentPath, Effort},
    provider::{Provider, ProviderError},
    store::Store,
    transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, Usage},
    turn::JobScheduler,
};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    num::NonZeroU64,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

#[derive(Clone)]
struct OfflineAuth;

impl Auth for OfflineAuth {
    fn access(&self) -> Result<(String, String), TransportError> {
        unreachable!("replay does not authenticate")
    }
}

struct Echo;

#[async_trait]
impl Provider for Echo {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        Ok(json!({"ok":true}))
    }

    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

struct Replay {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    turns: Mutex<VecDeque<Result<ResponsesTurn, TransportError>>>,
}

#[async_trait]
impl ResponsesTransport for Replay {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.requests.lock().unwrap().push(request);
        self.turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected model request")
    }
}

fn turn(id: &str, items: Vec<Item>) -> Result<ResponsesTurn, TransportError> {
    Ok(ResponsesTurn {
        response_id: id.into(),
        items,
        usage: Usage {
            input_tokens: 500,
            ..Default::default()
        },
    })
}

fn call() -> Item {
    Item(json!({"type":"function_call","call_id":"echo-1","name":"echo","arguments":"{}"}))
}

fn final_answer() -> Item {
    Item(json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}))
}

fn engine(
    turns: Vec<Result<ResponsesTurn, TransportError>>,
) -> (
    Engine<OfflineAuth, Echo, Replay>,
    Arc<Store>,
    Arc<Mutex<Vec<ResponsesRequest>>>,
) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let store = Arc::new(Store::memory().unwrap());
    let replay = Replay {
        requests: requests.clone(),
        turns: Mutex::new(turns.into()),
    };
    let engine = Engine::with_transport(
        replay,
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(Echo),
        EngineConfig {
            instructions: "standing config instruction".into(),
            tools: vec![],
            model: "offline".into(),
            effort: Effort::Medium,
            session_id: "text-compaction-test".into(),
            agent: AgentPath("/root".into()),
        },
    )
    .with_plain_text_compaction(NonZeroU64::new(800).unwrap());
    (engine, store, requests)
}

fn input() -> Vec<Item> {
    vec![
        Item(json!({"type":"message","role":"system","content":"standing system"})),
        Item(json!({"type":"message","role":"developer","content":"standing developer"})),
        Item(json!({"type":"message","role":"user","content":"old history ".repeat(2500)})),
        Item(json!({"type":"message","role":"user","content":"recent user"})),
    ]
}

fn compacted(store: &Store, head: &harness::model::RequestId) -> bool {
    store
        .session_state(&format!("harness:compaction:{}", head.0))
        .unwrap()
        .is_some()
}

async fn run(engine: &Engine<OfflineAuth, Echo, Replay>) -> harness::engine::EngineCompletion {
    let (_tx, rx) = watch::channel(false);
    let (_inbox_tx, inbox_rx) = tokio::sync::mpsc::unbounded_channel();
    engine.run(None, input(), rx, inbox_rx).await.unwrap()
}

#[tokio::test]
async fn plain_text_handoff_keeps_instructions_recent_user_and_effort() {
    let summary = Item(
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"The task is to finish Echo."}]}),
    );
    let (engine, store, requests) = engine(vec![
        turn("first", vec![call()]),
        turn("summary", vec![summary]),
        turn("final", vec![final_answer()]),
    ]);
    let completion = run(&engine).await;
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[1].tools_allowed, Some(vec![]));
    assert!(sent[1].tools.is_empty());
    assert_eq!(sent[1].instructions, "standing config instruction");
    assert_eq!(sent[1].pinned_effort, Effort::Medium);
    assert_eq!(sent[1].input.last().unwrap().0["role"], "user");
    let successor = &sent[2].input;
    assert!(
        successor
            .iter()
            .any(|item| item.0["content"] == "standing system")
    );
    assert!(
        successor
            .iter()
            .any(|item| item.0["content"] == "standing developer")
    );
    assert!(
        successor
            .iter()
            .any(|item| item.0["content"] == "recent user")
    );
    assert!(successor.iter().any(|item| {
        item.0["content"]
            .as_str()
            .is_some_and(|text| text.contains("The task is to finish Echo."))
    }));
    assert!(!successor.iter().any(|item| {
        item.0["content"]
            .as_str()
            .is_some_and(|text| text.starts_with("old history"))
    }));
    assert_eq!(
        successor.last().unwrap().configuration_effort(),
        Some(Effort::Medium)
    );
    assert!(compacted(&store, &completion.head_request));
}

#[tokio::test]
async fn failed_summary_keeps_history_and_does_not_retry_without_growth() {
    let (engine, store, requests) = engine(vec![
        turn("first", vec![call()]),
        Err(TransportError::Authentication),
        turn("final", vec![final_answer()]),
    ]);
    let completion = run(&engine).await;
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[1].tools_allowed, Some(vec![]));
    assert_eq!(sent[2].tools_allowed, None);
    assert!(sent[2].input.iter().any(|item| {
        item.0["content"]
            .as_str()
            .is_some_and(|text| text.starts_with("old history"))
    }));
    assert!(!compacted(&store, &completion.head_request));
}

#[tokio::test]
async fn tool_producing_summary_is_rejected_without_dispatch() {
    let injected =
        Item(json!({"type":"function_call","call_id":"forbidden","name":"echo","arguments":"{}"}));
    let (engine, store, requests) = engine(vec![
        turn("first", vec![call()]),
        turn("invalid-summary", vec![injected]),
        turn("final", vec![final_answer()]),
    ]);
    let completion = run(&engine).await;
    assert_eq!(requests.lock().unwrap().len(), 3);
    assert!(!compacted(&store, &completion.head_request));
    assert!(
        store
            .claims(&harness::model::CallId("forbidden".into()))
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn ineffective_summary_keeps_history_and_suppresses_immediate_retry() {
    let summary = Item(json!({"type":"message","role":"assistant","content":"x".repeat(35000)}));
    let (engine, store, requests) = engine(vec![
        turn("first", vec![call()]),
        turn("oversize-summary", vec![summary]),
        turn("final", vec![final_answer()]),
    ]);
    let completion = run(&engine).await;
    assert_eq!(requests.lock().unwrap().len(), 3);
    assert!(!compacted(&store, &completion.head_request));
}

struct SlowCustom {
    release: tokio::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

#[async_trait]
impl Provider for SlowCustom {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        unreachable!("only a custom call is emitted")
    }

    async fn call_custom_with_context(
        &self,
        _: &str,
        _: String,
        _: harness::provider::CallContext,
    ) -> Result<Value, ProviderError> {
        self.release.lock().await.take().unwrap().await.unwrap();
        Ok(json!("custom result"))
    }

    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

#[tokio::test]
async fn pending_custom_call_survives_handoff_and_receives_late_output() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let call = Item(
        json!({"type":"custom_tool_call","call_id":"custom-1","name":"custom","input":"echo raw","opaque":"exact"}),
    );
    let mut provisional = turn("provisional", vec![final_answer()]).unwrap();
    provisional.usage.input_tokens = 0;
    let replay = Replay {
        requests: requests.clone(),
        turns: Mutex::new(vec![
            turn("first", vec![call.clone()]),
            turn("summary", vec![Item(json!({"type":"message","role":"assistant","content":"Continue the custom work."}))]),
            Ok(provisional),
            turn("final", vec![final_answer()]),
        ].into()),
    };
    let store = Arc::new(Store::memory().unwrap());
    let engine = Engine::<OfflineAuth, SlowCustom, _>::with_transport(
        replay,
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(SlowCustom {
            release: tokio::sync::Mutex::new(Some(release_rx)),
        }),
        EngineConfig {
            instructions: "standing config instruction".into(),
            tools: vec![],
            model: "offline".into(),
            effort: Effort::High,
            session_id: "custom-pending".into(),
            agent: AgentPath("/root".into()),
        },
    )
    .with_plain_text_compaction(NonZeroU64::new(800).unwrap());
    let release_requests = requests.clone();
    tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if release_requests.lock().unwrap().len() >= 3 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("successor request with pending custom call");
        release_tx.send(()).unwrap();
    });
    let (_tx, rx) = watch::channel(false);
    let (_inbox_tx, inbox_rx) = tokio::sync::mpsc::unbounded_channel();
    let completion = engine.run(None, input(), rx, inbox_rx).await.unwrap();
    let sent = requests.lock().unwrap();
    assert!(sent[2].input.contains(&call));
    assert!(completion.transcript.contains(&call));
    assert!(completion.transcript.iter().any(|item| {
        item.0["type"] == "custom_tool_call_output" && item.0["call_id"] == "custom-1"
    }));
    assert!(
        store
            .pending_at(&completion.head_request)
            .unwrap()
            .is_empty()
    );
}
