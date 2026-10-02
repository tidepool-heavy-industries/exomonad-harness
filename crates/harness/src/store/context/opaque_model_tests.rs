use super::*;
use crate::{
    model::{CallId, Effort},
    store::{ClaimState, Usage},
    transport::{ResponsesRequest, ResponsesTurn},
};

enum Provenance<'a> {
    Issued(&'a str),
    CompactionIssued(&'a str),
    MissingModel,
    NotInResponse,
    UnversionedTurn,
}

struct Restoration {
    store: Store,
    snapshot: ContextSnapshot,
    saved: ContextDocument,
}

fn output() -> JobOutput {
    JobOutput::Completed(Ok(json!("edited")))
}

fn prepare(provenance: Provenance<'_>, selected: &str) -> Restoration {
    let store = Store::memory().unwrap();
    let parent = RequestId("opaque-response".into());
    let items = vec![
        Item(json!({"type":"reasoning","encrypted_content":"same-opaque-bytes"})),
        Item(json!({"type":"message","role":"assistant","content":"discovery"})),
    ];
    if matches!(provenance, Provenance::CompactionIssued(_)) {
        let source = RequestId("pre-compaction".into());
        store
            .write_request(&source, None, "/root", &[], Usage::default())
            .unwrap();
        store
            .write_compaction_request(&parent, &source, "/root", &items)
            .unwrap();
    } else {
        store
            .write_request(&parent, None, "/root", &items, Usage::default())
            .unwrap();
    }
    {
        let model = match provenance {
            Provenance::Issued(model) | Provenance::CompactionIssued(model) => model,
            _ => "model-a",
        };
        store
            .record_replay_turn(
                &parent,
                &ResponsesRequest {
                    input: vec![],
                    instructions: "test".into(),
                    tools: vec![].into(),
                    tools_allowed: None,
                    model: model.into(),
                    pinned_effort: Effort::Low,
                    session_id: "test".into(),
                },
                &ResponsesTurn {
                    response_id: "response".into(),
                    items: items.clone(),
                    usage: Default::default(),
                },
            )
            .unwrap();
        if matches!(provenance, Provenance::MissingModel) {
            store.lock().execute("UPDATE events SET payload=json_remove(payload,'$.issued.model') WHERE kind='model_turn'", []).unwrap();
        }
        if matches!(provenance, Provenance::NotInResponse) {
            store.lock().execute("UPDATE events SET payload=json_remove(payload,'$.response.items[0]') WHERE kind='model_turn'", []).unwrap();
        }
        if matches!(provenance, Provenance::UnversionedTurn) {
            store.lock().execute("UPDATE events SET payload=json_remove(payload,'$.format','$.issued') WHERE kind='model_turn'", []).unwrap();
        }
    }
    let head = RequestId("edit".into());
    store.write_request(&head, Some(&parent), "/root", &[
        Item(json!({"type":"custom_tool_call","call_id":"drop","name":"haskell_sync","input":"drop"})),
        Item(json!({"type":"custom_tool_call","call_id":"restore","name":"haskell_sync","input":"restore"})),
    ], Usage::default()).unwrap();
    store.set_effort(&head, Effort::Low).unwrap();
    let first = store.claim(&CallId("drop".into()), &head).unwrap();
    let second = store.claim(&CallId("restore".into()), &head).unwrap();
    let initial = match provenance {
        Provenance::Issued(model) | Provenance::CompactionIssued(model) => model,
        _ => "model-a",
    };
    store
        .initialize_context_model(&first.origin, initial)
        .unwrap();
    let snapshot = store.begin_context(&first, &head).unwrap();
    let mut saved = snapshot.document.clone();
    let sources = saved
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContextBlock::Native { reference, .. }
            | ContextBlock::Text {
                reference: Some(reference),
                ..
            } => Some(reference.clone()),
            _ => None,
        })
        .collect();
    let dropped = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                document: ContextDocument {
                    blocks: vec![ContextBlock::Text {
                        reference: None,
                        role: ContextRole::User,
                        text: "discovery note".into(),
                        sources,
                    }],
                },
                next_model: Some(selected.into()),
                next_effort: None,
            },
            output: &output(),
            pending: std::slice::from_ref(&second),
        })
        .unwrap();
    let snapshot = store.begin_context(&second, &dropped.head).unwrap();
    saved.blocks.extend(
        snapshot
            .document
            .blocks
            .iter()
            .filter(|block| {
                matches!(
                    block,
                    ContextBlock::Native {
                        protected: true,
                        ..
                    }
                )
            })
            .cloned(),
    );
    Restoration {
        store,
        snapshot,
        saved,
    }
}

fn counts(store: &Store) -> Vec<i64> {
    let c = store.lock();
    [
        "requests",
        "request_items",
        "items",
        "claims",
        "events",
        "session_state",
    ]
    .iter()
    .map(|table| {
        c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    })
    .collect()
}

fn refused(
    fixture: &Restoration,
    next_model: Option<&str>,
    replay: Option<&ContextCommitEvidence>,
) {
    let before = fixture
        .store
        .context_request_state(&fixture.snapshot.head, &fixture.snapshot.operation.origin)
        .unwrap();
    let before_counts = counts(&fixture.store);
    let result = if let Some(evidence) = replay {
        fixture
            .store
            .restore_context_commit(&fixture.snapshot, evidence, &output(), &[])
    } else {
        fixture.store.commit_context(ContextCommit {
            snapshot: &fixture.snapshot,
            draft: &ContextDraft {
                document: fixture.saved.clone(),
                next_model: next_model.map(str::to_owned),
                next_effort: Some(Effort::High),
            },
            output: &output(),
            pending: &[],
        })
    };
    assert!(
        matches!(
            result,
            Err(StoreError::Context(
                ContextError::OpaqueModel | ContextError::ProtectedGroup
            ))
        ),
        "{result:?}"
    );
    let after = fixture
        .store
        .context_request_state(&fixture.snapshot.head, &fixture.snapshot.operation.origin)
        .unwrap();
    assert_eq!(after.history, before.history);
    assert_eq!(after.model, before.model);
    assert_eq!(after.generation, before.generation);
    assert_eq!(counts(&fixture.store), before_counts);
    assert!(
        fixture
            .store
            .context_receipt(&fixture.snapshot.operation)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .store
            .claims_for_operation(&fixture.snapshot.operation)
            .unwrap()
            .iter()
            .all(|claim| claim.state == ClaimState::Pending && claim.output.is_none())
    );
}

fn restore(fixture: &Restoration, next_model: Option<&str>) -> ContextCommitReceipt {
    fixture
        .store
        .commit_context(ContextCommit {
            snapshot: &fixture.snapshot,
            draft: &ContextDraft {
                document: fixture.saved.clone(),
                next_model: next_model.map(str::to_owned),
                next_effort: None,
            },
            output: &output(),
            pending: &[],
        })
        .unwrap()
}

#[test]
fn historical_opaque_restore_refuses_mismatched_model_and_rolls_back() {
    for next in [None, Some("model-b")] {
        refused(
            &prepare(Provenance::Issued("model-a"), "model-b"),
            next,
            None,
        );
    }
}

#[test]
fn historical_opaque_restore_refuses_unknown_provenance() {
    for provenance in [
        Provenance::UnversionedTurn,
        Provenance::MissingModel,
        Provenance::NotInResponse,
    ] {
        refused(&prepare(provenance, "model-a"), None, None);
    }
}

#[test]
fn historical_opaque_restore_accepts_original_issuing_model() {
    for (provenance, selected, next) in [
        (Provenance::Issued("model-a"), "model-a", None),
        (Provenance::Issued("model-a"), "model-a", Some("model-a")),
        (Provenance::Issued("model-a"), "model-b", Some("model-a")),
        (Provenance::CompactionIssued("model-a"), "model-a", None),
    ] {
        let fixture = prepare(provenance, selected);
        let receipt = restore(&fixture, next);
        let state = fixture
            .store
            .context_request_state(&receipt.head, &fixture.snapshot.operation.origin)
            .unwrap();
        assert_eq!(state.model.as_deref(), Some("model-a"));
        assert!(
            state
                .history
                .iter()
                .any(|(_, _, item)| item.0["encrypted_content"] == "same-opaque-bytes")
        );
        let retained = history(&fixture.store.lock(), &receipt.head, true).unwrap();
        assert_eq!(retained[0].origin.request.0, "opaque-response");
    }
}

fn evidence() -> ContextCommitEvidence {
    let original = prepare(Provenance::Issued("model-a"), "model-a");
    restore(&original, None);
    original
        .store
        .context_commit_evidence(&original.snapshot.operation)
        .unwrap()
        .unwrap()
}

#[test]
fn historical_opaque_replay_uses_local_issuing_model() {
    refused(
        &prepare(Provenance::Issued("model-b"), "model-a"),
        None,
        Some(&evidence()),
    );
}

#[test]
fn historical_opaque_replay_refuses_unknown_provenance() {
    for provenance in [Provenance::UnversionedTurn, Provenance::MissingModel] {
        refused(&prepare(provenance, "model-a"), None, Some(&evidence()));
    }
}

#[test]
fn historical_opaque_replay_restores_compatible_original_model() {
    let local = prepare(Provenance::Issued("model-a"), "model-a");
    let receipt = local
        .store
        .restore_context_commit(&local.snapshot, &evidence(), &output(), &[])
        .unwrap();
    assert_eq!(receipt.model.as_deref(), Some("model-a"));
    assert!(
        local
            .store
            .context_history(&receipt.head)
            .unwrap()
            .iter()
            .any(|(_, _, item)| item.0["encrypted_content"] == "same-opaque-bytes")
    );
}

#[test]
fn historical_compaction_without_issuing_evidence_stays_protected() {
    let store = Store::memory().unwrap();
    let source = RequestId("source".into());
    let compact = RequestId("compact-opaque".into());
    let note = RequestId("compact-note".into());
    let head = RequestId("restore-compaction".into());
    store
        .write_request(&source, None, "/root", &[], Usage::default())
        .unwrap();
    store
        .write_compaction_request(
            &compact,
            &source,
            "/root",
            &[Item(
                json!({"type":"compaction","encrypted_content":"opaque-compaction"}),
            )],
        )
        .unwrap();
    let mut saved = store.read_context(&compact).unwrap();
    store
        .write_compaction_request(
            &note,
            &compact,
            "/root",
            &[Item(
                json!({"type":"message","role":"user","content":"plain summary"}),
            )],
        )
        .unwrap();
    store.write_request(&head, Some(&note), "/root", &[
        Item(json!({"type":"custom_tool_call","call_id":"restore","name":"haskell_sync","input":"restore"})),
    ], Usage::default()).unwrap();
    let operation = store.claim(&CallId("restore".into()), &head).unwrap();
    store
        .initialize_context_model(&operation.origin, "model-a")
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    saved.blocks.extend(snapshot.document.blocks.clone());
    refused(
        &Restoration {
            store,
            snapshot,
            saved,
        },
        None,
        None,
    );
}

#[test]
fn identical_current_opaque_bytes_do_not_authorize_another_origin() {
    let mut fixture = prepare(Provenance::Issued("model-a"), "model-b");
    let head = RequestId("model-b-equal-bytes".into());
    let opaque = Item(json!({"type":"reasoning","encrypted_content":"same-opaque-bytes"}));
    fixture.store.write_request(&head, Some(&fixture.snapshot.head), "/root", &[
        opaque.clone(),
        Item(json!({"type":"custom_tool_call","call_id":"restore-again","name":"haskell_sync","input":"restore"})),
    ], Usage::default()).unwrap();
    fixture
        .store
        .record_replay_turn(
            &head,
            &ResponsesRequest {
                input: vec![],
                instructions: "test".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "model-b".into(),
                pinned_effort: Effort::Low,
                session_id: "test".into(),
            },
            &ResponsesTurn {
                response_id: "equal-bytes".into(),
                items: vec![opaque],
                usage: Default::default(),
            },
        )
        .unwrap();
    let operation = fixture
        .store
        .claim(&CallId("restore-again".into()), &head)
        .unwrap();
    fixture.snapshot = fixture.store.begin_context(&operation, &head).unwrap();
    fixture.saved.blocks.retain(|block| {
        matches!(
            block,
            ContextBlock::Native {
                protected: false,
                ..
            }
        )
    });
    fixture
        .saved
        .blocks
        .extend(fixture.snapshot.document.blocks.clone());
    refused(&fixture, None, None);
}

#[test]
fn current_opaque_retention_needs_no_historical_provenance() {
    let store = Store::memory().unwrap();
    let head = RequestId("current-opaque".into());
    store.write_request(&head, None, "/root", &[
        Item(json!({"type":"reasoning","encrypted_content":"current-opaque"})),
        Item(json!({"type":"custom_tool_call","call_id":"retain","name":"haskell_sync","input":"retain"})),
    ], Usage::default()).unwrap();
    let operation = store.claim(&CallId("retain".into()), &head).unwrap();
    store
        .initialize_context_model(&operation.origin, "model-a")
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                document: snapshot.document.clone(),
                next_model: None,
                next_effort: None,
            },
            output: &output(),
            pending: &[],
        })
        .unwrap();
    assert_eq!(receipt.model.as_deref(), Some("model-a"));
    assert!(
        store
            .context_history(&receipt.head)
            .unwrap()
            .iter()
            .any(|(_, _, item)| item.0["encrypted_content"] == "current-opaque")
    );
}
