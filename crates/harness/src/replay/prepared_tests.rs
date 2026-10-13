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
        prompt_cache_comparison: None,
    }
}
#[tokio::test]
async fn replay_keeps_compaction_turns_but_only_inference_calls_require_claims() {
    let source = Arc::new(Store::memory().unwrap());
    let summary = RequestId("summary".into());
    let compact = RequestId("compact".into());
    let normal = RequestId("normal".into());
    let start = RequestId("start".into());
    source.create_request(&start, None, "/source").unwrap();
    source
        .create_request(&summary, Some(&start), "summary-storage")
        .unwrap();
    source
        .create_request(&compact, Some(&start), "compact-storage")
        .unwrap();
    source
        .create_request(&normal, Some(&start), "/source")
        .unwrap();
    let tools = vec![
        json!({"type":"function","name":"work","description":"work","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
    ];
    let records = [
        (
            &start,
            ProviderRequestPurpose::Inference,
            dto(tools.clone()),
            vec![],
        ),
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
    // Neither ordinary forks nor auxiliary descendants are actor-chain leaves.
    for (id, parent, branch, purpose) in [
        (
            "unrelated-inference",
            &start,
            "other-actor",
            ProviderRequestPurpose::Inference,
        ),
        (
            "auxiliary-descendant",
            &summary,
            "summary-storage",
            ProviderRequestPurpose::TextSummary,
        ),
    ] {
        let id = RequestId(id.into());
        source.create_request(&id, Some(parent), branch).unwrap();
        let prepared = source
            .prepare_provider_request(
                &id,
                &id,
                dto(vec![]),
                &[],
                None,
                Native.prepare_submission().unwrap(),
                purpose,
            )
            .unwrap();
        source.record_provider_turn(prepared.issuer(), &ResponsesTurn { response_id: id.0.clone(), items: vec![Item(json!({"type":"function_call","call_id":"unrelated-unclaimed","name":"ignored","arguments":"{}"}))], usage: Usage::default() }).unwrap();
    }
    assert!(
        matches!(ReplayProvider::new(source.clone(), &start), Err(ReplayError::MissingInvocation(ref id)) if id == "normal-call")
    );
    let operation = source
        .claim(&crate::model::CallId("normal-call".into()), &normal)
        .unwrap();
    let replay = ReplayProvider::new(source.clone(), &start).unwrap();
    assert_eq!(replay.turns_remaining(), 4);
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
            ProviderRequestPurpose::TextSummary,
        )
        .unwrap();
    assert!(matches!(
        replay.create(wrong).await,
        Err(TransportError::PreparedSelectionMismatch)
    ));
    assert_eq!(replay.turns_remaining(), 4);
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

#[test]
fn cache_comparison_baseline_is_part_of_exact_replay_request() {
    let expected = dto(vec![]);
    let mut changed = expected.clone();
    changed.prompt_cache_comparison = Some(crate::transport::PromptCacheComparison {
        comparison_response_id: "baseline".into(),
    });
    assert_eq!(
        super::request_mismatch(&expected, &changed),
        Some("prompt_cache_comparison")
    );
    let restored: ResponsesRequest =
        serde_json::from_value(serde_json::to_value(&changed).unwrap()).unwrap();
    assert!(super::request_mismatch(&changed, &restored).is_none());
}
