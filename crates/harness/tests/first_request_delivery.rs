//! Offline first-request follow-up delivery integration gate.

use async_trait::async_trait;
use harness::{
    agent_runtime::StoreAgentToolService,
    agents::{AgentToolService, Contract, SpawnSource, dispatch_agent_verb},
    engine::{Engine, EngineConfig, ResponsesTransport},
    item::Item,
    model::{AgentPath, Effort},
    provider::{Provider, ProviderError},
    replay::ReplayTransport,
    store::Store,
    transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, sse::StreamEvent},
    turn::JobScheduler,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::watch;

#[derive(Clone)]
struct OfflineAuth;

impl Auth for OfflineAuth {
    fn access(&self) -> Result<(String, String), TransportError> {
        Ok(("offline-only".into(), "https://offline.invalid".into()))
    }
}

struct NoTools;

#[async_trait]
impl Provider for NoTools {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("the only model call must be strict finalize")
    }

    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

struct SharedReplay(Arc<ReplayTransport>);

#[async_trait]
impl ResponsesTransport for SharedReplay {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.0.create(request).await
    }

    async fn create_streaming(
        &self,
        request: ResponsesRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        self.0.create_streaming(request, sink).await
    }
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
struct Reply {
    answer: String,
}

fn contract(marker: &str) -> Contract {
    Contract {
        clauses: vec![marker.into()],
        acceptance: vec!["typed final answer".into()],
        owned: vec![],
        must_not: vec![],
        introduces: vec![],
        consumes: vec![],
        boundaries: vec![],
        reply: None,
    }
}

struct TempStore(PathBuf);

impl TempStore {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "harness-first-request-{}-{}.sqlite",
            std::process::id(),
            uuid::Uuid::new_v4()
        )))
    }
}

impl Drop for TempStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(format!("{}-wal", self.0.display()));
        let _ = std::fs::remove_file(format!("{}-shm", self.0.display()));
    }
}

#[tokio::test]
async fn followup_queued_before_first_request_is_delivered_once_and_durable() {
    let db = TempStore::new();
    let store = Arc::new(Store::open(&db.0).expect("open file Store"));
    let root = AgentPath("/root".into());
    let child = AgentPath("/root/worker".into());
    store
        .admit_agent(&root, None, None, &json!({}), &json!({"kind":"root"}))
        .expect("register root");
    let service = StoreAgentToolService::new(store.clone(), root.clone());
    assert_eq!(
        service
            .spawn_agent(
                &root,
                "worker",
                SpawnSource::Prompt,
                contract("initial-task-identity"),
            )
            .await
            .expect("register child")["task_name"],
        child.0
    );
    assert_eq!(
        store.agent(&child).unwrap().unwrap().head_request,
        None,
        "the child has no model head before the follow-up"
    );
    let initial = store.inbox(&child.0).unwrap();
    assert_eq!(initial.len(), 1);
    let initial_id = initial[0].id;
    let initial_hash = initial[0].item_hash.clone();

    let final_call = Item(json!({
        "type": "function_call",
        "call_id": "strict-final-call",
        "name": "finalize",
        "arguments": "{\"result\":{\"answer\":\"follow-up observed\"}}"
    }));
    let replay = Arc::new(ReplayTransport::gated([ResponsesTurn {
        response_id: "first-and-final".into(),
        items: vec![final_call.clone()],
        usage: Default::default(),
    }]));
    assert!(
        replay.recorded_requests().is_empty(),
        "no model request existed before follow-up queueing"
    );
    let queued = dispatch_agent_verb(
        &service,
        &root,
        None,
        "followup_task",
        json!({"target":child.0,"task":contract("followup-identity-unique")}),
    )
    .await
    .expect("production followup_task service");
    assert_eq!(queued["status"], "queued");
    assert!(replay.recorded_requests().is_empty());
    assert_eq!(store.agent(&child).unwrap().unwrap().head_request, None);
    let before = store.inbox(&child.0).expect("read durable queue");
    assert_eq!(before.len(), 2);
    let followup = before[1].clone();
    assert_ne!(followup.id, initial_id);
    assert_ne!(followup.item_hash, initial_hash);
    assert_eq!(followup.delivered_request, None);
    let followup_item = store
        .get_item(&followup.item_hash)
        .unwrap()
        .expect("follow-up item exists before Engine starts");
    assert!(
        followup_item
            .0
            .to_string()
            .contains("followup-identity-unique")
    );
    assert!(
        !store
            .get_item(&initial_hash)
            .unwrap()
            .unwrap()
            .0
            .to_string()
            .contains("followup-identity-unique")
    );

    let engine = Engine::<OfflineAuth, NoTools, _>::with_transport(
        SharedReplay(replay.clone()),
        store.clone(),
        Arc::new(JobScheduler::new(1).expect("scheduler")),
        Arc::new(NoTools),
        EngineConfig {
            instructions: "offline first-request gate".into(),
            tools: vec![],
            model: "offline-replay".into(),
            effort: Effort::Low,
            session_id: "first-request-delivery".into(),
            agent: child.clone(),
        },
    );
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let (_inbox_tx, inbox_rx) = tokio::sync::mpsc::unbounded_channel();
    let running = tokio::spawn(async move {
        engine
            .run_finalized::<Reply>(None, vec![], cancel_rx, inbox_rx)
            .await
    });
    replay.wait_requested(1).await;
    let requests = replay.recorded_requests();
    assert_eq!(requests.len(), 1);
    let text = requests[0]
        .input
        .iter()
        .map(|item| item.0.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(text.matches("followup-identity-unique").count(), 1);
    assert_eq!(text.matches("initial-task-identity").count(), 1);
    assert!(
        !running.is_finished(),
        "first request is held at replay barrier"
    );
    replay.release_next();
    let (completion, reply) = running.await.unwrap().expect("strict typed finalize");
    assert_eq!(
        reply,
        Reply {
            answer: "follow-up observed".into()
        }
    );
    assert_eq!(completion.turn.items, vec![final_call]);
    assert_eq!(replay.recorded_requests().len(), 1);
    assert!(
        store
            .advance_agent_head(&child, None, Some(&completion.head_request))
            .expect("persist completed head")
    );
    let final_head = completion.head_request;
    drop(service);
    drop(store);

    let reopened = Store::open(&db.0).expect("reopen file Store");
    assert_eq!(
        reopened.agent(&child).unwrap().unwrap().head_request,
        Some(final_head.clone())
    );
    let persisted = reopened.inbox(&child.0).expect("reopen mailbox");
    assert_eq!(persisted.len(), 2);
    assert_eq!(persisted[0].id, initial_id);
    assert_eq!(persisted[0].item_hash, initial_hash);
    assert_eq!(persisted[1].id, followup.id);
    assert_eq!(persisted[1].item_hash, followup.item_hash);
    assert_eq!(persisted[1].delivered_request, Some(final_head));
    assert!(reopened.unread(&child.0).unwrap().is_empty());
    assert_eq!(
        reopened.get_item(&persisted[1].item_hash).unwrap().unwrap(),
        followup_item,
        "the delivered envelope must be the exact queued item"
    );
}
