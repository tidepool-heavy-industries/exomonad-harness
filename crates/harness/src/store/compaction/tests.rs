use super::*;
use crate::{
    compaction::{
        CompactContext, Compactor, NewWindow, Server, ServerCompactFuture, TypedTurnFuture,
    },
    model::Effort,
    store::{ProviderRequestPurpose, Store, Usage as StoredUsage},
    transport::{
        PreparedTransportSelection, ResponsesProtocol, ResponsesRequest, ResponsesRoute,
        ResponsesTurn, Usage,
    },
};
use serde_json::json;

fn item(content: &str) -> Item {
    Item(json!({"type":"message","role":"assistant","content":content}))
}

fn window(items: Vec<Item>) -> Vec<Item> {
    [
        vec![Item(
            json!({"type":"message","role":"developer","content":"notice"}),
        )],
        items,
        vec![Item::configuration_update(Effort::Low)],
    ]
    .concat()
}

fn record_response(store: &Store, source: &RequestId, raw: &[Item]) -> ServerCompactionResponse {
    let provider = RequestId("server-response".into());
    store
        .create_request(&provider, Some(source), "harness:server-compaction-request")
        .unwrap();
    let occurrences = store.history_occurrences(source).unwrap();
    let request = ResponsesRequest {
        input: occurrences
            .iter()
            .map(|occurrence| occurrence.item.clone())
            .collect(),
        instructions: "compact the supplied history".into(),
        tools: vec![].into(),
        tools_allowed: None,
        model: "actual-model".into(),
        pinned_effort: Effort::Low,
        session_id: "compaction-test".into(),
    };
    let prepared = store
        .prepare_provider_request(
            source,
            &provider,
            request,
            &occurrences.into_iter().map(Some).collect::<Vec<_>>(),
            None,
            PreparedTransportSelection::native((
                ResponsesRoute::Codex,
                ResponsesProtocol::Standard,
            )),
            ProviderRequestPurpose::ServerCompaction,
        )
        .unwrap();
    store
        .record_provider_turn(
            prepared.issuer(),
            &ResponsesTurn {
                response_id: "completed-compaction".into(),
                items: raw.to_vec(),
                usage: Default::default(),
            },
        )
        .unwrap();
    ServerCompactionResponse {
        provider_request: provider,
    }
}

fn selected_window(store: &Store, source: &RequestId, raw: &[Item]) -> NewWindow {
    let facts = store.history_occurrences(source).unwrap();
    let items = facts
        .iter()
        .map(|fact| fact.item.clone())
        .collect::<Vec<_>>();
    let occurrences = facts.iter().cloned().map(Some).collect::<Vec<_>>();
    let pending_positions = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| item.tool_call().unwrap().map(|_| index))
        .collect::<Vec<_>>();
    let pending_occurrences = pending_positions
        .iter()
        .map(|index| facts[*index].clone())
        .collect::<Vec<_>>();
    let pending_calls = pending_positions
        .iter()
        .map(|index| items[*index].clone())
        .collect::<Vec<_>>();
    let compact = |_input: crate::compaction::CompactionInput| -> ServerCompactFuture<'_> {
        Box::pin(async { Ok(raw.to_vec()) })
    };
    let typed = |_: String,
                 _: crate::compaction::ToolName,
                 _: serde_json::Value|
     -> TypedTurnFuture<'_> { Box::pin(async { unreachable!() }) };
    let usage = Usage::default();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(Server.compact(CompactContext {
            items: &items,
            occurrences: &occurrences,
            pending_positions: &pending_positions,
            pending_occurrences: &pending_occurrences,
            usage: &usage,
            pending_calls: &pending_calls,
            effort: Effort::Low,
            server_compact: &compact,
            text_turn: &compact,
            typed_turn: &typed,
        }))
        .unwrap()
}

fn boundary(store: &Store, source_items: &[Item], raw: Vec<Item>) -> (RequestId, RequestId) {
    let source = RequestId("source".into());
    let target = RequestId("boundary".into());
    store
        .write_request(&source, None, "/root", source_items, StoredUsage::default())
        .unwrap();
    for item in source_items {
        if let Some(call) = item.tool_call().unwrap() {
            store.claim(&call.call_id, &source).unwrap();
        }
    }
    let response = record_response(store, &source, &raw);
    let selected = selected_window(store, &source, &raw);
    let pending = store
        .claims_on(&source)
        .unwrap()
        .into_iter()
        .map(|claim| claim.operation)
        .collect::<Vec<_>>();
    store
        .write_compaction_request_with_evidence(
            &target,
            &source,
            "/root",
            &selected.items,
            &selected.retained,
            &selected.generated,
            &store
                .history_occurrences(&source)
                .unwrap()
                .into_iter()
                .map(Some)
                .collect::<Vec<_>>(),
            &pending,
            None,
            Some(&response),
        )
        .unwrap();
    (target, response.provider_request)
}

#[test]
fn fresh_server_membership_keeps_raw_occurrences_separate_from_retained_sources() {
    let store = Store::memory().unwrap();
    let user = Item(json!({"type":"message","role":"user","content":"original"}));
    let pending =
        Item(json!({"type":"function_call","call_id":"pending","name":"work","arguments":"{}"}));
    let opaque = Item(json!({"type":"compaction","encrypted_content":"opaque"}));
    let visible = item("summary");
    let filtered_user = Item(json!({"type":"message","role":"user","content":"server user"}));
    let (target, provider) = boundary(
        &store,
        &[user, pending.clone()],
        vec![opaque, visible, filtered_user.clone(), pending],
    );
    let c = store.lock().unwrap();
    let envelope = source_response_envelope(&c, &provider).unwrap().unwrap();
    assert_eq!(envelope.model, "actual-model");
    assert_eq!(envelope.kind, ResponseEnvelopeKind::ServerCompaction);
    let occurrences = context::request_occurrences(&c, &target).unwrap();
    assert_eq!(
        envelope.origins,
        vec![occurrences[1].origin.clone(), occurrences[2].origin.clone()]
    );
    assert!(
        envelope
            .origins
            .iter()
            .all(|origin| origin.request == provider)
    );
    assert_eq!(occurrences[3].origin.request.0, "source");
    assert_eq!(occurrences[4].origin.request.0, "source");
    let raw = context::request_occurrences(&c, &provider).unwrap();
    assert_eq!(raw.len(), 4);
    assert_eq!(raw[2].item, filtered_user);
    assert!(!envelope.origins.contains(&raw[2].origin));
    assert!(!envelope.origins.contains(&raw[3].origin));
    assert_ne!(raw[3].origin, occurrences[4].origin);
    assert_eq!(raw[3].item, occurrences[4].item);
    let record: Record = serde_json::from_str(
        &c.query_row(
            "SELECT payload FROM events WHERE kind='compaction'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(record.evidence.is_none());
    let exact = record.exact.unwrap();
    assert_eq!(exact.provider_request, provider);
    assert_eq!(exact.selected, vec![(1, 0), (2, 1)]);
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM claims WHERE request_id='source'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn equal_bytes_and_duplicate_raw_occurrences_keep_distinct_exact_provenance() {
    let store = Store::memory().unwrap();
    let reused = item("already existed");
    let duplicate = item("duplicate raw");
    let generated = Item(json!({"type":"compaction","encrypted_content":"fresh"}));
    let (target, provider) = boundary(
        &store,
        std::slice::from_ref(&reused),
        vec![
            reused.clone(),
            reused.clone(),
            duplicate.clone(),
            duplicate,
            generated,
        ],
    );
    let c = store.lock().unwrap();
    let envelope = source_response_envelope(&c, &provider).unwrap().unwrap();
    let occurrences = context::request_occurrences(&c, &target).unwrap();
    assert_eq!(
        envelope.origins,
        occurrences[1..6]
            .iter()
            .map(|occurrence| occurrence.origin.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(envelope.origins.iter().collect::<HashSet<_>>().len(), 5);
    assert_eq!(occurrences[1].hash, occurrences[2].hash);
    assert_ne!(occurrences[1].origin, occurrences[2].origin);
    assert!(
        envelope
            .origins
            .iter()
            .all(|origin| origin.request == provider)
    );
    assert!(
        !envelope
            .origins
            .iter()
            .any(|origin| origin.request.0 == "source")
    );
}

#[test]
fn legacy_boundary_does_not_infer_the_selected_model() {
    let store = Store::memory().unwrap();
    let source = RequestId("source".into());
    let target = RequestId("legacy".into());
    store
        .write_request(&source, None, "/root", &[], StoredUsage::default())
        .unwrap();
    assert!(matches!(
        store.write_compaction_request(
            &target,
            &source,
            "/root",
            &window(vec![Item(
                json!({"type":"compaction","encrypted_content":"legacy"})
            )]),
        ),
        Err(StoreError::Context(crate::context::ContextError::Replay(
            crate::context::ContextReplayRejection::MissingProvenance { .. }
        )))
    ));
    assert!(store.request(&target).unwrap().is_none());
    assert!(
        response_envelopes(&store.lock().unwrap(), &target)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn invalid_or_conflicting_exact_evidence_refuses() {
    for mutation in [
        "format",
        "request",
        "model",
        "origin",
        "raw",
        "source",
        "duplicate_event",
    ] {
        let store = Store::memory().unwrap();
        let opaque = Item(json!({"type":"compaction","encrypted_content":"opaque"}));
        let (_, provider) = boundary(&store, &[], vec![opaque]);
        let c = store.lock().unwrap();
        assert!(source_response_envelope(&c, &provider).unwrap().is_some());
        let sql = match mutation {
            "format" => {
                "UPDATE events SET payload=json_set(payload,'$.exact.format',999) WHERE kind='compaction'"
            }
            "request" => {
                "UPDATE events SET payload=json_set(payload,'$.exact.provider_request','other') WHERE kind='compaction'"
            }
            "model" => {
                "UPDATE provider_requests SET issued_json=json_set(issued_json,'$.model','')"
            }
            "origin" => {
                "UPDATE events SET payload=json_set(payload,'$.exact.selected[0][0]',0) WHERE kind='compaction'"
            }
            "raw" => {
                "UPDATE events SET payload=json_set(payload,'$.exact.selected[0][1]',99) WHERE kind='compaction'"
            }
            "source" => {
                "UPDATE events SET payload=json_set(payload,'$.source','other') WHERE kind='compaction'"
            }
            "duplicate_event" => {
                "INSERT INTO events(request_id,kind,payload,created_at) SELECT request_id,kind,payload,created_at FROM events WHERE kind='compaction_selection'"
            }
            _ => unreachable!(),
        };
        c.execute(sql, []).unwrap();
        assert!(
            !matches!(source_response_envelope(&c, &provider), Ok(Some(_))),
            "{mutation}"
        );
    }
}

#[test]
fn evidence_failure_rolls_back_boundary_selection_and_retains_completed_raw_facts() {
    let store = Store::memory().unwrap();
    let source = RequestId("source".into());
    let target = RequestId("boundary".into());
    store
        .write_request(&source, None, "/root", &[], StoredUsage::default())
        .unwrap();
    let raw_only = Item(json!({"type":"message","role":"user","content":"filtered raw"}));
    let opaque = Item(json!({"type":"compaction","encrypted_content":"fresh"}));
    let raw = vec![opaque, raw_only];
    let response = record_response(&store, &source, &raw);
    let before =
        context::request_occurrences(&store.lock().unwrap(), &response.provider_request).unwrap();
    let selected = selected_window(&store, &source, &raw);
    store.lock().unwrap().execute_batch("CREATE TRIGGER reject_compaction BEFORE INSERT ON events WHEN NEW.kind='compaction' BEGIN SELECT RAISE(ABORT,'reject'); END;").unwrap();
    assert!(
        store
            .write_compaction_request_with_evidence(
                &target,
                &source,
                "/root",
                &selected.items,
                &selected.retained,
                &selected.generated,
                &[],
                &[],
                None,
                Some(&response)
            )
            .is_err()
    );
    assert!(store.request(&target).unwrap().is_none());
    assert!(!store.is_compaction_boundary(&target).unwrap());
    let c = store.lock().unwrap();
    assert_eq!(
        context::request_occurrences(&c, &response.provider_request).unwrap(),
        before
    );
    assert!(
        source_response_envelope(&c, &response.provider_request)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='compaction_selection'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn later_duplicate_summary_bytes_do_not_change_sealed_membership() {
    let store = Store::memory().unwrap();
    let visible = item("summary");
    let (target, provider) = boundary(
        &store,
        &[],
        vec![
            Item(json!({"type":"compaction","encrypted_content":"opaque"})),
            visible.clone(),
        ],
    );
    let before = source_response_envelope(&store.lock().unwrap(), &provider)
        .unwrap()
        .unwrap();
    store.append_items(&target, &[visible]).unwrap();
    let c = store.lock().unwrap();
    assert_eq!(
        source_response_envelope(&c, &provider).unwrap().unwrap(),
        before
    );
    let appended = context::request_occurrences(&c, &target)
        .unwrap()
        .last()
        .unwrap()
        .origin
        .clone();
    assert!(!before.origins.contains(&appended));
}

#[test]
fn later_filtered_raw_bytes_do_not_enter_sealed_membership() {
    let store = Store::memory().unwrap();
    let pending = Item(
        json!({"type":"function_call","call_id":"pending","name":"original","arguments":"{}"}),
    );
    let filtered_user = Item(json!({"type":"message","role":"user","content":"filtered"}));
    let (target, provider) = boundary(
        &store,
        std::slice::from_ref(&pending),
        vec![
            Item(json!({"type":"compaction","encrypted_content":"opaque"})),
            filtered_user.clone(),
        ],
    );
    let before = source_response_envelope(&store.lock().unwrap(), &provider)
        .unwrap()
        .unwrap();
    store
        .append_items(&target, &[filtered_user, item("later assistant")])
        .unwrap();
    let c = store.lock().unwrap();
    assert_eq!(
        source_response_envelope(&c, &provider).unwrap().unwrap(),
        before
    );
    let occurrences = context::request_occurrences(&c, &target).unwrap();
    assert!(
        occurrences
            .iter()
            .rev()
            .take(2)
            .all(|occurrence| !before.origins.contains(&occurrence.origin))
    );
}
