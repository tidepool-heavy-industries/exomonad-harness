//! Actual issued purposes and legacy refusal through the Replay owner.
use super::*;
use crate::{
    item::Item,
    model::Effort,
    store::ProviderRequestPurpose,
    transport::{PreparedProviderRequest, ResponsesProtocol, ResponsesRoute, Usage},
};
use serde_json::json;
struct Native;
#[async_trait::async_trait]
impl ResponsesTransport for Native {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::Codex, ResponsesProtocol::Standard))
    }
    async fn create(&self, _: PreparedProviderRequest) -> Result<ResponsesTurn, TransportError> {
        unreachable!()
    }
}
fn dto(tools: Vec<serde_json::Value>) -> ResponsesRequest {
    ResponsesRequest {
        input: vec![],
        instructions: "fixed".into(),
        tools: tools.into(),
        tools_allowed: None,
        model: "model".into(),
        pinned_effort: Effort::Low,
        session_id: "session".into(),
    }
}
#[tokio::test]
async fn replay_keeps_compaction_turns_but_only_inference_calls_require_claims() {
    let source = Arc::new(Store::memory().unwrap());
    let summary = RequestId("summary".into());
    let compact = RequestId("compact".into());
    let normal = RequestId("normal".into());
    source.create_request(&summary, None, "/source").unwrap();
    source
        .create_request(&compact, Some(&summary), "/source")
        .unwrap();
    source
        .create_request(&normal, Some(&compact), "/source")
        .unwrap();
    let tools = vec![
        json!({"type":"function","name":"work","description":"work","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
    ];
    let records = [
        (
            &summary,
            ProviderRequestPurpose::TextSummary,
            dto(vec![]),
            vec![Item(
                json!({"type":"function_call","call_id":"summary-raw","name":"ignored","arguments":"{}"}),
            )],
        ),
        (
            &compact,
            ProviderRequestPurpose::ServerCompaction,
            dto(vec![]),
            vec![
                Item(
                    json!({"type":"custom_tool_call","call_id":"compact-raw","name":"ignored","input":"raw"}),
                ),
                Item(
                    json!({"type":"compaction","id":"compact","encrypted_content":"synthetic-test-content"}),
                ),
            ],
        ),
        (
            &normal,
            ProviderRequestPurpose::Inference,
            dto(tools.clone()),
            vec![Item(
                json!({"type":"function_call","call_id":"normal-call","name":"work","arguments":"{}"}),
            )],
        ),
    ];
    for (id, purpose, request, items) in &records {
        let prepared = source
            .prepare_provider_request(
                id,
                id,
                request.clone(),
                &[],
                None,
                Native.prepare_submission().unwrap(),
                *purpose,
            )
            .unwrap();
        source
            .record_provider_turn(
                prepared.issuer(),
                &ResponsesTurn {
                    response_id: id.0.clone(),
                    items: items.clone(),
                    usage: Usage::default(),
                },
            )
            .unwrap();
    }
    assert!(
        matches!(ReplayProvider::new(source.clone(), &summary), Err(ReplayError::MissingInvocation(ref id)) if id == "normal-call")
    );
    let operation = source
        .claim(&crate::model::CallId("normal-call".into()), &normal)
        .unwrap();
    let replay = ReplayProvider::new(source.clone(), &summary).unwrap();
    assert_eq!(replay.turns_remaining(), 3);
    assert_eq!(replay.calls.len(), 1);
    assert!(replay.calls.contains_key(&operation));
    assert_eq!(replay.tool_schemas, tools.into());
    let local = Store::memory().unwrap();
    let wrong = RequestId("wrong-purpose".into());
    local.create_request(&wrong, None, "/local").unwrap();
    let wrong = local
        .prepare_provider_request(
            &wrong,
            &wrong,
            records[0].2.clone(),
            &[],
            None,
            replay.prepare_submission().unwrap(),
            ProviderRequestPurpose::Inference,
        )
        .unwrap();
    assert!(matches!(
        replay.create(wrong).await,
        Err(TransportError::PreparedSelectionMismatch)
    ));
    assert_eq!(replay.turns_remaining(), 3);
    for (id, purpose, request, items) in records {
        let local_id = RequestId(format!("local-{}", id.0));
        local.create_request(&local_id, None, "/local").unwrap();
        let prepared = local
            .prepare_provider_request(
                &local_id,
                &local_id,
                request,
                &[],
                None,
                replay.prepare_submission().unwrap(),
                purpose,
            )
            .unwrap();
        let turn = replay.create(prepared).await.unwrap();
        assert_eq!(turn.items, items);
    }
    assert_eq!(replay.turns_remaining(), 0);
    assert!(source.claims_on(&summary).unwrap().is_empty());
    assert!(source.claims_on(&compact).unwrap().is_empty());
}
#[test]
fn legacy_serialized_contract_does_not_become_replay_authority() {
    let store = Arc::new(Store::memory().unwrap());
    let request = RequestId("legacy".into());
    store.create_request(&request, None, "/legacy").unwrap();
    let event = store
        .record_replay_turn(
            &request,
            &dto(vec![]),
            &ResponsesTurn {
                response_id: "legacy".into(),
                items: vec![],
                usage: Usage::default(),
            },
        )
        .unwrap();
    let c = store.lock().unwrap();
    let payload: String = c
        .query_row("SELECT payload FROM events WHERE id=?1", [event], |row| {
            row.get(0)
        })
        .unwrap();
    let mut record: serde_json::Value = serde_json::from_str(&payload).unwrap();
    record["provider_contract"] = json!([ResponsesRoute::Codex, ResponsesProtocol::Standard]);
    c.execute(
        "UPDATE events SET payload=?2 WHERE id=?1",
        rusqlite::params![event, serde_json::to_string(&record).unwrap()],
    )
    .unwrap();
    drop(c);
    assert!(
        store.replay_turns(&request).unwrap()[0]
            .provider_contract
            .is_none()
    );
    assert!(matches!(
        ReplayProvider::new(store, &request)
            .unwrap()
            .prepare_submission(),
        Err(TransportError::UnprovenResponseContract)
    ));
}
