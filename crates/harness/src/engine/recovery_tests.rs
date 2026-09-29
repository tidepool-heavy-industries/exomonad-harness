//! Request-boundary recovery tests.
use super::*;

use crate::{
    provider::ProviderError,
    transport::{TransportError, Usage},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct TestAuth;
impl Auth for TestAuth {
    fn access(&self) -> Result<(String, String), TransportError> {
        Ok(("unused".into(), "unused".into()))
    }
}

struct Replay {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    turn: Mutex<Option<ResponsesTurn>>,
}

#[async_trait]
impl ResponsesTransport for Replay {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.requests.lock().unwrap().push(request);
        self.turn
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| TransportError::Stream("replay exhausted".into()))
    }
}

struct NoTools;
#[async_trait]
impl Provider for NoTools {
    async fn call(&self, _name: &str, _args: Value) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool("unexpected tool call".into()))
    }

    fn tools(&self) -> Vec<Value> {
        Vec::new()
    }
}

fn empty_mailbox() -> tokio::sync::mpsc::UnboundedReceiver<Envelope> {
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    receiver
}

fn final_turn() -> ResponsesTurn {
    ResponsesTurn {
        response_id: "recovered-final".into(),
        items: vec![Item(json!({
            "type":"function_call",
            "call_id":"typed-final",
            "name":"finalize",
            "arguments":{"result":{"answer":"recovered"}}
        }))],
        usage: Usage::default(),
    }
}

fn engine(
    store: Arc<Store>,
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
) -> Engine<TestAuth, NoTools, Replay> {
    Engine::with_transport(
        Replay {
            requests,
            turn: Mutex::new(Some(final_turn())),
        },
        store,
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(NoTools),
        EngineConfig {
            instructions: "Return the typed answer".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "recovery-boundary".into(),
            agent: AgentPath("/root".into()),
        },
    )
}

#[tokio::test]
async fn engine_recovery_pending_claim_becomes_interrupted_and_completes_typed_reply() {
    let store = Arc::new(Store::memory().unwrap());
    let head = RequestId("boundary-head".into());
    let agent = AgentPath("/root".into());
    let call = CallId("lost-in-flight".into());
    let prefix = Item(json!({
        "type":"function_call", "call_id":call.0, "name":"slow", "arguments":"{}"
    }));
    store
        .write_request(
            &head,
            None,
            &agent.0,
            &[prefix.clone()],
            StoredUsage::default(),
        )
        .unwrap();
    store.set_effort(&head, Effort::Low).unwrap();
    store.claim(&call, &head).unwrap();
    let resumed_prompt = Item(json!({"role":"user","content":"continue"}));
    let resumed_hash = store.put_item(&resumed_prompt).unwrap();

    let requests = Arc::new(Mutex::new(Vec::new()));
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let completion = engine(store.clone(), requests.clone())
        .run_with_reply_schema(
            Some(head.clone()),
            vec![resumed_prompt],
            cancel_rx,
            empty_mailbox(),
            json!({
                "type":"object",
                "properties":{"answer":{"type":"string"}},
                "required":["answer"],
                "additionalProperties":false
            }),
        )
        .await
        .unwrap();

    let reply = FinalizeParser::new()
        .parse_completed::<Value>(&completion.turn.items[0])
        .unwrap();
    assert_eq!(reply, json!({"answer":"recovered"}));
    let request_log = requests.lock().unwrap();
    assert_eq!(
        request_log.len(),
        1,
        "one boundary replay, no duplicate run"
    );
    let replay_input = &request_log[0].input;
    assert!(replay_input.contains(&prefix));
    assert!(replay_input.iter().any(|item| {
        item.0["type"] == "function_call_output"
            && item.0["call_id"] == call.0
            && item.0["output"]
                .as_str()
                .is_some_and(|output| output.contains("\"error\":\"job interrupted\""))
    }));
    let claims = store.claims(&call).unwrap();
    assert_eq!(claims.len(), 1, "recovery must not add a second claim");
    assert_eq!(claims[0].state, crate::store::ClaimState::Interrupted);
    assert!(
        store
            .claims_on(&completion.head_request)
            .unwrap()
            .is_empty(),
        "recovery must not attach a new claim to the descendant request"
    );

    let child_seen = store.seen_by(&completion.head_request).unwrap();
    let head_seen = store.seen_by(&head).unwrap();
    assert!(child_seen.len() > head_seen.len());
    assert!(head_seen.iter().all(|hash| child_seen.contains(hash)));
    assert!(
        !head_seen.contains(&resumed_hash),
        "new input was unseen at the source head"
    );
    assert!(
        child_seen.contains(&resumed_hash),
        "new input is visible in descendant ancestry"
    );
}

#[tokio::test]
async fn engine_recovery_zero_row_interrupt_uses_durable_settlement_output() {
    let store = Arc::new(Store::memory().unwrap());
    let head = RequestId("missing-job-head".into());
    let agent = AgentPath("/root".into());
    let call = CallId("durable-pending-call".into());
    let call_item = Item(json!({
        "type":"function_call", "call_id":call.0, "name":"slow", "arguments":"{}"
    }));
    store
        .write_request(&head, None, &agent.0, &[call_item], StoredUsage::default())
        .unwrap();
    store.set_effort(&head, Effort::Low).unwrap();
    store.claim(&call, &head).unwrap();
    let stale_pending_claim = store.claims(&call).unwrap().remove(0);
    let actual_output = Item(json!({
        "type":"function_call_output",
        "call_id":call.0,
        "output":"{\"result\":\"already settled\"}"
    }));
    assert_eq!(store.settle_claims(&call, &actual_output).unwrap(), 1);

    let engine = engine(store.clone(), Arc::new(Mutex::new(Vec::new())));
    let recovered = engine
        .recover_missing_job(&stale_pending_claim)
        .await
        .unwrap();
    assert_eq!(recovered, actual_output);
    let claims = store.claims(&call).unwrap();
    assert_eq!(
        claims.len(),
        1,
        "reconciliation must not create another claim"
    );
    assert_eq!(claims[0].state, crate::store::ClaimState::Settled);
    assert_ne!(
        recovered.0["output"].as_str(),
        Some("{\"error\":\"job interrupted\"}"),
        "durable settlement output must not be replaced with Interrupted"
    );
}

#[tokio::test]
async fn engine_recovering_reconciles_same_branch_ancestor_after_descendant_barrier() {
    let store = Arc::new(Store::memory().unwrap());
    let branch = AgentPath("/root".into());
    let origin = RequestId("recovery-origin".into());
    let barrier = RequestId("recovery-descendant-barrier".into());
    let fork = RequestId("recovery-child-fork".into());
    let call = CallId("ancestor-lost-call".into());
    let call_item = Item(json!({
        "type":"function_call",
        "call_id":call.0,
        "name":"must_not_replay",
        "arguments":"{}"
    }));
    store
        .write_request(
            &origin,
            None,
            &branch.0,
            &[call_item.clone()],
            StoredUsage::default(),
        )
        .unwrap();
    store.set_effort(&origin, Effort::Low).unwrap();
    store.claim(&call, &origin).unwrap();
    store
        .write_request(
            &barrier,
            Some(&origin),
            &branch.0,
            &[],
            StoredUsage::default(),
        )
        .unwrap();
    store.set_effort(&barrier, Effort::Low).unwrap();
    store
        .write_request(
            &fork,
            Some(&origin),
            "/root/child",
            &[],
            StoredUsage::default(),
        )
        .unwrap();

    let requests = Arc::new(Mutex::new(Vec::new()));
    let provider_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    struct CountCalls(Arc<std::sync::atomic::AtomicUsize>);
    #[async_trait]
    impl Provider for CountCalls {
        async fn call(&self, _name: &str, _args: Value) -> Result<Value, ProviderError> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(ProviderError::Tool(
                "provider invocation was forbidden".into(),
            ))
        }

        fn tools(&self) -> Vec<Value> {
            vec![]
        }
    }
    let engine = Engine::<TestAuth, CountCalls, Replay>::with_transport(
        Replay {
            requests: requests.clone(),
            turn: Mutex::new(Some(ResponsesTurn {
                response_id: "recovered-final".into(),
                items: vec![Item(json!({
                    "type":"message",
                    "role":"assistant",
                    "phase":"final_answer",
                    "content":"done"
                }))],
                usage: Usage::default(),
            })),
        },
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(CountCalls(provider_calls.clone())),
        EngineConfig {
            instructions: "Return the typed answer".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "recovery-descendant".into(),
            agent: branch.clone(),
        },
    );
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let completion = engine
        .run_recovering(Some(barrier.clone()), vec![], cancel_rx, empty_mailbox())
        .await
        .unwrap();

    let claims = store.claims(&call).unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].request, origin);
    assert_eq!(claims[0].state, crate::store::ClaimState::Interrupted);
    let request_log = requests.lock().unwrap();
    assert_eq!(request_log.len(), 1, "exactly one model request");
    let outputs: Vec<_> = request_log[0]
        .input
        .iter()
        .filter(|item| item.0["type"] == "function_call_output" && item.0["call_id"] == call.0)
        .collect();
    assert_eq!(outputs.len(), 1, "interrupted output is sent exactly once");
    assert!(request_log[0].input.contains(&call_item));
    assert_eq!(
        provider_calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "lost external calls are never replayed"
    );
    assert_eq!(
        store
            .claims_on_branch_lineage(&fork, &branch.0)
            .unwrap()
            .len(),
        0,
        "recovery lineage stops at a different-agent fork"
    );
    assert!(
        store
            .claims_on(&completion.head_request)
            .unwrap()
            .is_empty(),
        "the recovery request does not claim the ancestor call again"
    );
}
