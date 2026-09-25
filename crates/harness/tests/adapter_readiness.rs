//! Offline resident-adapter release gate.
//!
//! This exercises the production Engine/Store path with only deterministic
//! replay and a controllable resident cell; it never makes a live model call.

use async_trait::async_trait;
use harness::{
    cell_job::{CellInput, CellJob, CellJobProvider, CellOutput},
    engine::{Engine, EngineConfig, ResponsesTransport},
    item::Item,
    mailbox::{DeliveryClass, Envelope, EnvelopeType},
    model::{AgentPath, Effort},
    provider::{CallContext, ProviderError},
    replay::{FakeResidentCell, ReplayCellState, ReplaySessionKey, ReplayTransport},
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

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
struct Reply {
    answer: String,
}

/// Keep the control handle available to the test while Engine owns a transport.
struct SharedReplay(Arc<ReplayTransport>);

struct SharedCell(Arc<FakeResidentCell>);

#[async_trait]
impl CellJob for SharedCell {
    async fn run(
        &self,
        input: CellInput,
        context: CallContext,
    ) -> Result<CellOutput, ProviderError> {
        self.0.run(input, context).await
    }
}

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

fn turn(id: &str, items: Vec<Item>) -> ResponsesTurn {
    ResponsesTurn {
        response_id: id.into(),
        items,
        usage: Default::default(),
    }
}

fn wait_call(id: &str) -> Item {
    Item(json!({
        "type": "function_call",
        "call_id": id,
        "name": "wait_agent",
        "arguments": "{}"
    }))
}

fn envelope(sender: &str, payload: &str, timestamp_ms: i64) -> Envelope {
    Envelope {
        kind: EnvelopeType::Message,
        recipient: AgentPath("/root/adapter".into()),
        sender: AgentPath(sender.into()),
        payload: payload.into(),
        class: DeliveryClass::AtBoundary,
        timestamp_ms,
    }
}

fn temp_store_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "harness-adapter-readiness-{}-{}.sqlite",
        std::process::id(),
        uuid::Uuid::new_v4()
    ))
}

struct TempStore(PathBuf);

impl Drop for TempStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(format!("{}-wal", self.0.display()));
        let _ = std::fs::remove_file(format!("{}-shm", self.0.display()));
    }
}

#[tokio::test]
async fn active_cell_survives_three_boundary_envelopes_and_finalizes_durably() {
    let db = TempStore(temp_store_path());
    let store = Arc::new(Store::open(&db.0).expect("open file Store"));
    let agent = AgentPath("/root/adapter".into());
    let cell_key = ReplaySessionKey::new("adapter-readiness:long-cell");
    let cell = Arc::new(FakeResidentCell::new(store.clone(), cell_key.clone()));
    let cell_output = CellOutput {
        value: json!({"answer": 42, "complete": true}),
        stdout: "complete stdout from the resident cell\n".into(),
        stderr: "complete stderr from the resident cell\n".into(),
    };

    let cell_call = Item(json!({
        "type": "function_call",
        "call_id": "long-cell-call",
        "name": "cell",
        "arguments": "{\"source\":\"long-running source\"}"
    }));
    let final_call = Item(json!({
        "type": "function_call",
        "call_id": "strict-final-call",
        "name": "finalize",
        "arguments": "{\"result\":{\"answer\":\"three boundaries observed\"}}"
    }));
    // Each wait turn lets an arriving envelope resume the wait job and forces
    // a fresh model request, where that envelope must be visible. The fifth
    // response also waits so the resident cell can be released only after its
    // third envelope-bearing request was captured.
    let replay = Arc::new(ReplayTransport::gated([
        turn("cell-admitted", vec![cell_call]),
        turn("wait-for-first", vec![wait_call("wait-one")]),
        turn("wait-for-second", vec![wait_call("wait-two")]),
        turn("wait-for-third", vec![wait_call("wait-three")]),
        turn("wait-for-cell", vec![wait_call("wait-cell")]),
        turn("finalized", vec![final_call.clone()]),
    ]));
    let engine = Engine::<OfflineAuth, CellJobProvider<SharedCell>, _>::with_transport(
        SharedReplay(replay.clone()),
        store.clone(),
        Arc::new(JobScheduler::new(2).expect("job scheduler")),
        Arc::new(CellJobProvider::new(SharedCell(cell.clone()))),
        EngineConfig {
            instructions: "offline adapter readiness test".into(),
            tools: Vec::new(),
            model: "offline-replay".into(),
            effort: Effort::Low,
            session_id: "adapter-readiness-session".into(),
            agent: agent.clone(),
        },
    );
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let (inbox_tx, inbox_rx) = tokio::sync::mpsc::unbounded_channel();
    let running = tokio::spawn(async move {
        engine
            .run_finalized::<Reply>(None, vec![], cancel_rx, inbox_rx)
            .await
    });

    replay.wait_requested(1).await;
    assert_eq!(replay.recorded_requests().len(), 1);
    replay.release_next();

    cell.wait_started(1).await;
    replay.wait_requested(2).await;
    assert_request_lacks(&replay.recorded_requests()[1], "boundary-one");
    assert_eq!(cell.start_count(), 1);
    assert_eq!(cell.cancel_count(), 0);
    assert!(!running.is_finished());
    let first = envelope("/root/sender-one", "boundary-one", 1);
    store
        .add_envelope(
            &first.sender.0,
            &first.recipient.0,
            "AtBoundary",
            &Item(json!({"type":"message","role":"assistant","content":[{
                "type":"output_text","text":"boundary-one"
            }]})),
            None,
        )
        .expect("persist first envelope");
    inbox_tx.send(first).expect("signal first envelope");
    replay.release_next();

    replay.wait_requested(3).await;
    assert_request_contains(&replay.recorded_requests()[2], "boundary-one");
    assert_request_lacks(&replay.recorded_requests()[2], "boundary-two");
    assert_eq!(cell.start_count(), 1);
    assert_eq!(cell.cancel_count(), 0);
    let second = envelope("/root/sender-two", "boundary-two", 2);
    store
        .add_envelope(
            &second.sender.0,
            &second.recipient.0,
            "AtBoundary",
            &Item(json!({"type":"message","role":"assistant","content":[{
                "type":"output_text","text":"boundary-two"
            }]})),
            None,
        )
        .expect("persist second envelope");
    inbox_tx.send(second).expect("signal second envelope");
    replay.release_next();

    replay.wait_requested(4).await;
    assert_request_contains(&replay.recorded_requests()[3], "boundary-two");
    assert_request_lacks(&replay.recorded_requests()[3], "boundary-three");
    assert_eq!(cell.start_count(), 1);
    assert_eq!(cell.cancel_count(), 0);
    let third = envelope("/root/sender-three", "boundary-three", 3);
    store
        .add_envelope(
            &third.sender.0,
            &third.recipient.0,
            "AtBoundary",
            &Item(json!({"type":"message","role":"assistant","content":[{
                "type":"output_text","text":"boundary-three"
            }]})),
            None,
        )
        .expect("persist third envelope");
    inbox_tx.send(third).expect("signal third envelope");
    replay.release_next();

    replay.wait_requested(5).await;
    let requests = replay.recorded_requests();
    assert_request_contains(&requests[4], "boundary-three");
    assert_eq!(cell.start_count(), 1);
    assert_eq!(cell.cancel_count(), 0);
    assert!(
        !running.is_finished(),
        "cell must remain pending through the third-envelope request"
    );
    // Request 5 is already captured with the third envelope but its replay
    // response is still gated. Completing the cell now must not finish the
    // run until response 5 is released and request 6 strict-finalizes.
    cell.release(cell_output.clone());
    replay.release_next();

    replay.wait_requested(6).await;
    let requests = replay.recorded_requests();
    assert_request_contains(&requests[5], "boundary-one");
    assert_request_contains(&requests[5], "boundary-two");
    assert_request_contains(&requests[5], "boundary-three");
    assert_tool_flags(&requests[5], "cell", true, true);
    assert_tool_strict(&requests[5], "finalize");
    assert!(
        requests[5]
            .input
            .iter()
            .any(|item| item.0["call_id"] == "long-cell-call"
                && stored_cell_output_matches(item, &cell_output)),
        "finalize request must include the complete cell output"
    );
    assert_eq!(cell.start_count(), 1);
    assert_eq!(cell.cancel_count(), 0);
    replay.release_next();

    let (completion, reply) = running
        .await
        .expect("engine task joins")
        .expect("run_finalized succeeds");
    assert_eq!(
        reply,
        Reply {
            answer: "three boundaries observed".into()
        }
    );
    assert_eq!(completion.turn.items, vec![final_call.clone()]);
    assert_eq!(cell.start_count(), 1);
    assert_eq!(cell.cancel_count(), 0);
    assert!(completion.transcript.iter().any(|item| {
        item.0["type"] == "function_call_output"
            && item.0["call_id"] == "long-cell-call"
            && stored_cell_output_matches(item, &cell_output)
    }));
    drop(store);

    let reopened = Store::open(&db.0).expect("reopen file Store");
    assert_eq!(
        ReplayCellState::load(&reopened, &cell_key).expect("query durable cell state"),
        ReplayCellState {
            evaluations: 1,
            last_source: Some("long-running source".into()),
        }
    );
    let mut request_chain = vec![completion.head_request.clone()];
    loop {
        let request = reopened
            .request(request_chain.last().expect("request chain not empty"))
            .expect("load request")
            .expect("request exists");
        let Some(parent) = request.parent else {
            break;
        };
        request_chain.push(parent);
    }
    let root = request_chain.last().expect("root request exists");
    let stored_turns = reopened
        .replay_turns(root)
        .expect("load durable turn history");
    assert_eq!(stored_turns.len(), 6);
    assert!(stored_turns[5].model_response.items.contains(&final_call));
    let durable_items: Vec<_> = request_chain
        .iter()
        .flat_map(|request| reopened.items(request).expect("load request items"))
        .collect();
    assert!(durable_items.iter().any(|item| {
        item.0["type"] == "function_call_output"
            && item.0["call_id"] == "long-cell-call"
            && stored_cell_output_matches(item, &cell_output)
    }));
    assert!(durable_items.iter().any(|item| item == &final_call));

    drop(reopened);
    drop(cell);
}

fn assert_request_contains(request: &ResponsesRequest, expected: &str) {
    assert!(
        request.input.iter().any(|item| {
            item.0["type"] == "message" && item.0["content"].to_string().contains(expected)
        }),
        "request did not include envelope {expected:?}"
    );
}

fn assert_request_lacks(request: &ResponsesRequest, unexpected: &str) {
    assert!(
        !request.input.iter().any(|item| {
            item.0["type"] == "message" && item.0["content"].to_string().contains(unexpected)
        }),
        "request unexpectedly contained envelope {unexpected:?}"
    );
}

fn assert_tool_flags(request: &ResponsesRequest, name: &str, asynchronous: bool, strict: bool) {
    let tool = request
        .tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("request has no {name:?} tool schema"));
    assert_eq!(tool["async"], json!(asynchronous), "{name} async flag");
    assert_eq!(tool["strict"], json!(strict), "{name} strict flag");
}

fn assert_tool_strict(request: &ResponsesRequest, name: &str) {
    let tool = request
        .tools
        .iter()
        .find(|tool| tool["name"] == name)
        .unwrap_or_else(|| panic!("request has no {name:?} tool schema"));
    assert_eq!(tool["strict"], json!(true), "{name} strict flag");
}

fn stored_cell_output_matches(item: &Item, expected: &CellOutput) -> bool {
    let Some(encoded) = item.0["output"].as_str() else {
        return false;
    };
    serde_json::from_str::<Value>(encoded)
        .is_ok_and(|output| output == serde_json::to_value(expected).unwrap())
}
