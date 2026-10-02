use super::*;
use crate::{model::Effort, store::Usage};
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

fn boundary(
    store: &Store,
    source_items: &[Item],
    raw: Vec<Item>,
    installed: Vec<Item>,
) -> RequestId {
    let source = RequestId("source".into());
    let target = RequestId("boundary".into());
    store
        .write_request(&source, None, "/root", source_items, Usage::default())
        .unwrap();
    store
        .write_compaction_request_with_evidence(
            &target,
            &source,
            "/root",
            &window(installed),
            &[],
            None,
            Some(&ServerCompactionResponse {
                model: "actual-model".into(),
                items: raw,
            }),
        )
        .unwrap();
    target
}

#[test]
fn fresh_server_membership_excludes_reinserted_users_and_pending_calls() {
    let store = Store::memory().unwrap();
    let user = Item(json!({"type":"message","role":"user","content":"original"}));
    let pending =
        Item(json!({"type":"function_call","call_id":"pending","name":"work","arguments":"{}"}));
    let opaque = Item(json!({"type":"compaction","encrypted_content":"opaque"}));
    let visible = item("summary");
    let filtered_user = Item(json!({"type":"message","role":"user","content":"server user"}));
    let target = boundary(
        &store,
        &[user.clone(), pending.clone()],
        vec![
            opaque.clone(),
            visible.clone(),
            filtered_user.clone(),
            pending.clone(),
        ],
        vec![opaque.clone(), visible.clone(), user, pending],
    );
    let c = store.lock();
    let envelopes = response_envelopes(&c, &target).unwrap();
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].model, "actual-model");
    assert_eq!(envelopes[0].kind, ResponseEnvelopeKind::ServerCompaction);
    let occurrences = context::request_occurrences(&c, &target).unwrap();
    assert_eq!(
        envelopes[0].origins,
        vec![occurrences[1].origin.clone(), occurrences[2].origin.clone()]
    );
    assert_eq!(occurrences[3].origin.request.0, "source");
    assert_eq!(occurrences[4].origin.request.0, "source");
    let raw: String = c
        .query_row(
            "SELECT payload FROM events WHERE kind='compaction'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let record: Record = serde_json::from_str(&raw).unwrap();
    let evidence = record.evidence.unwrap();
    assert_eq!(evidence.raw_response.len(), 4);
    let filtered_hash = blake3::hash(&serde_json::to_vec(&filtered_user).unwrap())
        .to_hex()
        .to_string();
    assert!(
        evidence
            .raw_response
            .contains(&ItemHash(filtered_hash.clone()))
    );
    assert!(
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM items WHERE hash=?1)",
            [filtered_hash],
            |row| row.get::<_, bool>(0)
        )
        .unwrap()
    );
    assert_eq!(
        c.query_row(
            "SELECT COUNT(*) FROM events WHERE kind='model_turn'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn equal_bytes_and_duplicate_raw_occurrences_have_unknown_provenance() {
    let store = Store::memory().unwrap();
    let reused = item("already existed");
    let duplicate = item("duplicate raw");
    let generated = Item(json!({"type":"compaction","encrypted_content":"fresh"}));
    let target = boundary(
        &store,
        std::slice::from_ref(&reused),
        vec![
            reused.clone(),
            reused.clone(),
            duplicate.clone(),
            duplicate.clone(),
            generated.clone(),
        ],
        vec![
            reused.clone(),
            reused,
            duplicate.clone(),
            duplicate,
            generated,
        ],
    );
    let c = store.lock();
    let envelopes = response_envelopes(&c, &target).unwrap();
    let occurrences = context::request_occurrences(&c, &target).unwrap();
    assert_eq!(occurrences[1].origin.request.0, "source");
    assert_eq!(occurrences[2].origin.request, target);
    assert_eq!(envelopes[0].origins, vec![occurrences[5].origin.clone()]);
}

#[test]
fn legacy_boundary_does_not_infer_the_selected_model() {
    let store = Store::memory().unwrap();
    let source = RequestId("source".into());
    let target = RequestId("legacy".into());
    store
        .write_request(&source, None, "/root", &[], Usage::default())
        .unwrap();
    store
        .write_compaction_request(
            &target,
            &source,
            "/root",
            &window(vec![Item(
                json!({"type":"compaction","encrypted_content":"legacy"}),
            )]),
        )
        .unwrap();
    assert!(
        response_envelopes(&store.lock(), &target)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn invalid_or_conflicting_evidence_is_unknown() {
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
        let target = boundary(&store, &[], vec![opaque.clone()], vec![opaque]);
        assert_eq!(response_envelopes(&store.lock(), &target).unwrap().len(), 1);
        let c = store.lock();
        let sql = match mutation {
            "format" => {
                "UPDATE events SET payload=json_set(payload,'$.evidence.format',999) WHERE kind='compaction'"
            }
            "request" => {
                "UPDATE events SET payload=json_set(payload,'$.evidence.request','other') WHERE kind='compaction'"
            }
            "model" => {
                "UPDATE events SET payload=json_set(payload,'$.evidence.model','') WHERE kind='compaction'"
            }
            "origin" => {
                "UPDATE events SET payload=json_set(payload,'$.evidence.origins[0].position',0) WHERE kind='compaction'"
            }
            "raw" => {
                "UPDATE events SET payload=json_set(payload,'$.evidence.raw_response[0]','missing') WHERE kind='compaction'"
            }
            "source" => {
                "UPDATE events SET payload=json_set(payload,'$.source','other') WHERE kind='compaction'"
            }
            "duplicate_event" => {
                "INSERT INTO events(request_id,kind,payload,created_at) SELECT request_id,kind,payload,created_at FROM events WHERE kind='compaction'"
            }
            _ => unreachable!(),
        };
        c.execute(sql, []).unwrap();
        assert!(
            response_envelopes(&c, &target).unwrap().is_empty(),
            "{mutation}"
        );
    }
}

#[test]
fn evidence_failure_rolls_back_the_boundary_and_raw_items() {
    let store = Store::memory().unwrap();
    let source = RequestId("source".into());
    let target = RequestId("boundary".into());
    store
        .write_request(&source, None, "/root", &[], Usage::default())
        .unwrap();
    store.lock().execute_batch("CREATE TRIGGER reject_compaction BEFORE INSERT ON events WHEN NEW.kind='compaction' BEGIN SELECT RAISE(ABORT,'reject'); END;").unwrap();
    let raw_only = item("filtered raw");
    let opaque = Item(json!({"type":"compaction","encrypted_content":"fresh"}));
    assert!(
        store
            .write_compaction_request_with_evidence(
                &target,
                &source,
                "/root",
                &window(vec![opaque.clone()]),
                &[],
                None,
                Some(&ServerCompactionResponse {
                    model: "model".into(),
                    items: vec![opaque, raw_only.clone()]
                })
            )
            .is_err()
    );
    assert!(store.request(&target).unwrap().is_none());
    assert!(!store.is_compaction_boundary(&target).unwrap());
    let hash = blake3::hash(&serde_json::to_vec(&raw_only).unwrap())
        .to_hex()
        .to_string();
    assert!(
        !store
            .lock()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM items WHERE hash=?1)",
                [hash],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
}
