use super::*;
use crate::{
    item::ToolKind,
    model::{CallId, Effort},
    store::{ClaimState, Usage},
    transport::{ResponsesRequest, ResponsesTurn},
};

const TRIMMED: &str = "[Trimmed: repeated diagnostics]\nretained result";

struct Fixture {
    store: Store,
    head: RequestId,
    edit: OperationId,
    completed: OperationId,
    pending: OperationId,
    snapshot: ContextSnapshot,
}

fn call(id: &str, asynchronous: bool) -> Item {
    Item(
        json!({"type":"custom_tool_call","call_id":id,"name":"cell","input":format!("original input {id}"),"async":asynchronous}),
    )
}

fn record(store: &Store, head: &RequestId, items: Vec<Item>) {
    store
        .record_replay_turn(
            head,
            &ResponsesRequest {
                input: vec![],
                instructions: "fixture".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "model-a".into(),
                pinned_effort: Effort::Low,
                session_id: "fixture".into(),
            },
            &ResponsesTurn {
                response_id: head.0.clone(),
                items,
                usage: Default::default(),
            },
        )
        .unwrap();
}

fn fixture(store: Store) -> Fixture {
    let previous = RequestId("previous".into());
    let response = vec![
        Item(json!({"type":"reasoning","encrypted_content":"original opaque","summary":[]})),
        Item(json!({"type":"message","role":"assistant","content":[
            {"type":"output_text","text":"verbose answer","annotations":[]},
            {"type":"output_text","text":"second part","annotations":[]}
        ],"future":{"preserve":true}})),
        call("completed", false),
        call("pending", true),
    ];
    store
        .write_request(&previous, None, "/root", &response, Usage::default())
        .unwrap();
    store.set_effort(&previous, Effort::Low).unwrap();
    record(&store, &previous, response);
    let completed = store.claim(&CallId("completed".into()), &previous).unwrap();
    let pending = store.claim(&CallId("pending".into()), &previous).unwrap();
    let terminal = JobOutput::Completed(Ok(json!("verbose result")));
    store
        .write_job_output(&completed, ToolKind::Custom, &terminal)
        .unwrap();
    store
        .append_items(
            &previous,
            &[Item::tool_output(
                &completed.call,
                ToolKind::Custom,
                &terminal,
            )],
        )
        .unwrap();
    let head = RequestId("editing".into());
    let response = vec![
        Item(json!({"type":"reasoning","encrypted_content":"editor opaque","summary":[]})),
        call("edit", false),
    ];
    store
        .write_request(&head, Some(&previous), "/root", &response, Usage::default())
        .unwrap();
    record(&store, &head, response);
    let edit = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_model(&edit.origin, "model-a")
        .unwrap();
    let snapshot = store.begin_context(&edit, &head).unwrap();
    Fixture {
        store,
        head,
        edit,
        completed,
        pending,
        snapshot,
    }
}

fn draft(snapshot: &ContextSnapshot, original: &str, replacement: &str) -> ContextDraft {
    let mut document = snapshot.document.clone();
    let mut count = 0;
    for block in &mut document.blocks {
        if let ContextBlock::Native { texts, .. } = block {
            for field in texts {
                if field.text == original {
                    assert!(field.editable);
                    field.text = replacement.into();
                    count += 1;
                }
            }
        }
    }
    assert_eq!(count, 1);
    ContextDraft {
        document,
        next_model: None,
        next_effort: None,
    }
}

fn output() -> JobOutput {
    JobOutput::Completed(Ok(json!("real editor completion")))
}

fn commit(fixture: &Fixture, draft: &ContextDraft) -> ContextCommitReceipt {
    fixture
        .store
        .commit_context(ContextCommit {
            snapshot: &fixture.snapshot,
            draft,
            output: &output(),
            pending: std::slice::from_ref(&fixture.pending),
        })
        .unwrap()
}

#[test]
fn completed_result_trims_inside_protected_group_without_waiting_for_pending() {
    let fixture = fixture(Store::memory().unwrap());
    assert!(fixture.snapshot.document.blocks.iter().any(|block| matches!(block,
        ContextBlock::Native { protected: true, texts, .. } if texts.iter().any(|field| field.text == "verbose result" && field.editable))));
    let before = fixture.store.context_history(&fixture.head).unwrap();
    let terminal = fixture
        .store
        .replay_tool_output_operation(&fixture.completed)
        .unwrap()
        .unwrap();
    let receipt = commit(
        &fixture,
        &draft(&fixture.snapshot, "verbose result", TRIMMED),
    );
    let raw = fixture.store.context_history(&receipt.head).unwrap();
    assert_eq!(
        &raw[..before.len()]
            .iter()
            .map(|(_, _, item)| item)
            .collect::<Vec<_>>(),
        &before.iter().map(|(_, _, item)| item).collect::<Vec<_>>()
    );
    let projected = fixture
        .store
        .context_request_state(&receipt.head, &fixture.edit.origin)
        .unwrap();
    assert!(
        projected
            .history
            .iter()
            .any(|(_, _, item)| item.0["output"] == TRIMMED)
    );
    for opaque in ["original opaque", "editor opaque"] {
        assert!(
            projected
                .history
                .iter()
                .any(|(_, _, item)| item.0["encrypted_content"] == opaque)
        );
    }
    assert_eq!(
        fixture
            .store
            .replay_tool_output_operation(&fixture.completed)
            .unwrap()
            .unwrap(),
        terminal
    );
    assert_eq!(
        fixture
            .store
            .invocation_item(&fixture.completed.request, &fixture.completed.call)
            .unwrap()
            .unwrap()
            .input,
        crate::item::ToolInput::Custom("original input completed".into())
    );
    assert!(
        fixture
            .store
            .claims_for_operation(&fixture.pending)
            .unwrap()
            .iter()
            .all(|claim| claim.state == ClaimState::Pending)
    );
    let later = JobOutput::Completed(Ok(json!("late actual result")));
    fixture
        .store
        .write_job_output(&fixture.pending, ToolKind::Custom, &later)
        .unwrap();
    fixture
        .store
        .append_items(
            &receipt.head,
            &[Item::tool_output(
                &fixture.pending.call,
                ToolKind::Custom,
                &later,
            )],
        )
        .unwrap();
    let next = fixture
        .store
        .context_request_state(&receipt.head, &fixture.edit.origin)
        .unwrap();
    assert_eq!(
        next.history.last().unwrap().2.0["output"],
        "late actual result"
    );
    assert!(
        next.history
            .iter()
            .any(|(_, _, item)| item.0["output"] == TRIMMED)
    );
}

#[test]
fn native_message_part_edit_preserves_other_parts_unknown_fields_and_reasoning() {
    let fixture = fixture(Store::memory().unwrap());
    let receipt = commit(
        &fixture,
        &draft(&fixture.snapshot, "verbose answer", TRIMMED),
    );
    let projected = fixture
        .store
        .context_request_state(&receipt.head, &fixture.edit.origin)
        .unwrap();
    let answer = projected
        .history
        .iter()
        .find(|(_, _, item)| item.0["future"]["preserve"] == true)
        .unwrap();
    assert_eq!(answer.2.0["content"][0]["text"], TRIMMED);
    assert_eq!(answer.2.0["content"][1]["text"], "second part");
    assert!(
        fixture
            .store
            .context_history(&receipt.head)
            .unwrap()
            .iter()
            .any(|(_, _, item)| item.0["content"][0]["text"] == "verbose answer")
    );
}

#[test]
fn edited_opaque_model_switch_rolls_back_instead_of_dropping_reasoning() {
    let fixture = fixture(Store::memory().unwrap());
    let mut candidate = draft(&fixture.snapshot, "verbose result", TRIMMED);
    candidate.next_model = Some("model-b".into());
    let before = fixture.store.context_history(&fixture.head).unwrap();
    assert!(matches!(
        fixture.store.commit_context(ContextCommit {
            snapshot: &fixture.snapshot,
            draft: &candidate,
            output: &output(),
            pending: std::slice::from_ref(&fixture.pending),
        }),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
    assert_eq!(
        fixture.store.context_history(&fixture.head).unwrap(),
        before
    );
    assert!(
        fixture
            .store
            .context_receipt(&fixture.edit)
            .unwrap()
            .is_none()
    );
    assert!(
        fixture
            .store
            .replay_tool_output_operation(&fixture.edit)
            .unwrap()
            .is_none()
    );
}

#[test]
fn native_visible_field_authority_cannot_change() {
    for mutation in 0..4 {
        let fixture = fixture(Store::memory().unwrap());
        let mut candidate = draft(&fixture.snapshot, "verbose result", TRIMMED);
        let ContextBlock::Native { texts, .. } = candidate.document.blocks.iter_mut().find(|block|
            matches!(block, ContextBlock::Native {texts, ..} if texts.iter().any(|field| field.text == TRIMMED))).unwrap() else { unreachable!() };
        let field = texts
            .iter_mut()
            .find(|field| field.text == TRIMMED)
            .unwrap();
        match mutation {
            0 => field.reference = ContextReference::from_raw("forged".into()),
            1 => field.selector = ContextTextSelector::MessageText { part: 0 },
            2 => field.editable = false,
            _ => {
                texts.pop();
            }
        }
        assert!(matches!(
            fixture.store.commit_context(ContextCommit {
                snapshot: &fixture.snapshot,
                draft: &candidate,
                output: &output(),
                pending: &[],
            }),
            Err(StoreError::Context(ContextError::NativeEdit))
        ));
        assert!(
            fixture
                .store
                .context_receipt(&fixture.edit)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn overlay_compaction_retains_canonical_outputs_and_projection() {
    let fixture = fixture(Store::memory().unwrap());
    let receipt = commit(
        &fixture,
        &draft(&fixture.snapshot, "verbose result", TRIMMED),
    );
    let projected = fixture
        .store
        .context_request_state(&receipt.head, &fixture.edit.origin)
        .unwrap();
    let target = RequestId("compacted".into());
    fixture
        .store
        .write_compaction_request_with_claims(
            &target,
            &receipt.head,
            "/root",
            &projected
                .history
                .into_iter()
                .map(|(_, _, item)| item)
                .collect::<Vec<_>>(),
            std::slice::from_ref(&fixture.pending),
            None,
        )
        .unwrap();
    assert!(
        fixture
            .store
            .context_history(&target)
            .unwrap()
            .iter()
            .any(|(_, _, item)| item.0["output"] == "verbose result")
    );
    assert!(
        fixture
            .store
            .context_request_state(&target, &fixture.edit.origin)
            .unwrap()
            .history
            .iter()
            .any(|(_, _, item)| item.0["output"] == TRIMMED)
    );
}

#[test]
fn overlay_survives_reopen_and_saved_context_restores_original_body() {
    let path =
        std::env::temp_dir().join(format!("harness-overlay-{}.sqlite", uuid::Uuid::new_v4()));
    let (head, identity, saved) = {
        let fixture = fixture(Store::open(&path).unwrap());
        let receipt = commit(
            &fixture,
            &draft(&fixture.snapshot, "verbose result", TRIMMED),
        );
        (
            receipt.head,
            fixture.edit.origin.clone(),
            fixture.snapshot.document.clone(),
        )
    };
    let store = Store::open(&path).unwrap();
    assert!(
        store
            .context_request_state(&head, &identity)
            .unwrap()
            .history
            .iter()
            .any(|(_, _, item)| item.0["output"] == TRIMMED)
    );
    let next = RequestId("restoring".into());
    store
        .write_request(
            &next,
            Some(&head),
            "/root",
            &[call("restore", false)],
            Usage::default(),
        )
        .unwrap();
    let operation = store.claim(&CallId("restore".into()), &next).unwrap();
    let snapshot = store.begin_context(&operation, &next).unwrap();
    let mut document = snapshot.document.clone();
    for block in &mut document.blocks {
        if let ContextBlock::Native { reference, .. } = block {
            if let Some(previous) = saved.blocks.iter().find(|previous| {
                matches!(previous,
                ContextBlock::Native {reference: old, ..} if old == reference)
            }) {
                *block = previous.clone();
            }
        }
    }
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                document,
                next_model: None,
                next_effort: None,
            },
            output: &output(),
            pending: &[],
        })
        .unwrap();
    assert!(
        store
            .context_request_state(&receipt.head, &identity)
            .unwrap()
            .history
            .iter()
            .any(|(_, _, item)| item.0["output"] == "verbose result")
    );
    std::fs::remove_file(path).unwrap();
}

fn downgrade_receipt(store: &Store, operation: &OperationId, replay: bool) {
    let c = store.lock();
    let (id, raw): (i64, String) = c.query_row("SELECT id,payload FROM events WHERE kind='context_commit' AND json_extract(payload,'$.operation')=?1",
        [serde_json::to_string(operation).unwrap()], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    let mut record: serde_json::Value = serde_json::from_str(&raw).unwrap();
    record["version"] = json!(2);
    for entry in record["prefix"].as_array_mut().unwrap() {
        entry.as_object_mut().unwrap().remove("overlays");
    }
    let mut candidate: serde_json::Value =
        serde_json::from_str(record["candidate"].as_str().unwrap()).unwrap();
    if replay {
        for tuple in candidate[0].as_array_mut().unwrap() {
            assert_eq!(tuple.as_array_mut().unwrap().pop(), Some(json!([])));
        }
    } else {
        for block in candidate["document"]["blocks"].as_array_mut().unwrap() {
            block.as_object_mut().unwrap().remove("texts");
        }
    }
    c.execute(
        "UPDATE events SET payload=json_set(payload,'$.version',2,'$.prefix',json(?2),'$.candidate',?3) WHERE id=?1",
        params![id, serde_json::to_string(&record["prefix"]).unwrap(), serde_json::to_string(&candidate).unwrap()],
    )
    .unwrap();
}

#[test]
fn v2_draft_retry_accepts_unchanged_fields_but_rejects_forged_public_snapshot() {
    let fixture = fixture(Store::memory().unwrap());
    let unchanged = ContextDraft {
        document: fixture.snapshot.document.clone(),
        next_model: None,
        next_effort: None,
    };
    let receipt = commit(&fixture, &unchanged);
    downgrade_receipt(&fixture.store, &fixture.edit, false);
    assert_eq!(commit(&fixture, &unchanged), receipt);
    let candidate = draft(&fixture.snapshot, "verbose result", TRIMMED);
    let mut forged = fixture.snapshot.clone();
    forged.document = candidate.document.clone();
    assert!(matches!(
        fixture.store.commit_context(ContextCommit {
            snapshot: &forged,
            draft: &candidate,
            output: &output(),
            pending: &[],
        }),
        Err(StoreError::ConflictingReplayOutcome { .. })
    ));
    assert_eq!(
        fixture.store.context_receipt(&fixture.edit).unwrap(),
        Some(receipt)
    );
}

#[test]
fn v2_replay_retry_after_reopen_preserves_exact_receipt_and_terminal() {
    let original = fixture(Store::memory().unwrap());
    commit(
        &original,
        &ContextDraft {
            document: original.snapshot.document.clone(),
            next_model: None,
            next_effort: None,
        },
    );
    let evidence = original
        .store
        .context_commit_evidence(&original.edit)
        .unwrap()
        .unwrap();
    let path = std::env::temp_dir().join(format!(
        "harness-overlay-v2-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let local = fixture(Store::open(&path).unwrap());
    let receipt = local
        .store
        .restore_context_commit(
            &local.snapshot,
            &evidence,
            &output(),
            std::slice::from_ref(&local.pending),
        )
        .unwrap();
    downgrade_receipt(&local.store, &local.edit, true);
    let snapshot = local.snapshot.clone();
    drop(local);
    let store = Store::open(&path).unwrap();
    let before = store.context_history(&receipt.head).unwrap();
    let count = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM events WHERE kind='context_commit'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(
        store
            .restore_context_commit(&snapshot, &evidence, &output(), &[])
            .unwrap(),
        receipt
    );
    assert_eq!(store.context_history(&receipt.head).unwrap(), before);
    assert_eq!(
        store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind='context_commit'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        count
    );
    std::fs::remove_file(path).unwrap();
}

fn message_fixture(first: &str, second: &str) -> (Store, ContextSnapshot) {
    let store = Store::memory().unwrap();
    let head = RequestId("messages".into());
    store
        .write_request(
            &head,
            None,
            "/root",
            &[
                Item(json!({"type":"message","role":"user","content":first})),
                Item(json!({"type":"message","role":"user","content":second})),
                call("edit", false),
            ],
            Usage::default(),
        )
        .unwrap();
    store.set_effort(&head, Effort::Low).unwrap();
    let operation = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_model(&operation.origin, "model-a")
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    (store, snapshot)
}

fn commit_messages(
    store: &Store,
    snapshot: &ContextSnapshot,
    replacements: &[&str],
) -> ContextCommitReceipt {
    let mut document = snapshot.document.clone();
    for (block, text) in document.blocks.iter_mut().zip(replacements) {
        let ContextBlock::Text { text: body, .. } = block else {
            unreachable!()
        };
        *body = (*text).into();
    }
    store
        .commit_context(ContextCommit {
            snapshot,
            draft: &ContextDraft {
                document,
                next_model: None,
                next_effort: None,
            },
            output: &output(),
            pending: &[],
        })
        .unwrap()
}

#[test]
fn projected_compaction_preserves_distinct_equal_original_occurrences() {
    let (store, snapshot) = message_fixture("continue", "continue");
    let receipt = commit_messages(&store, &snapshot, &[TRIMMED]);
    let projected = store
        .context_request_state(&receipt.head, &snapshot.operation.origin)
        .unwrap();
    let target = RequestId("messages-compacted".into());
    store
        .write_compaction_request(
            &target,
            &receipt.head,
            "/root",
            &projected
                .history
                .into_iter()
                .map(|(_, _, item)| item)
                .collect::<Vec<_>>(),
        )
        .unwrap();
    let canonical = request_occurrences(&store.lock(), &target).unwrap();
    assert_eq!(canonical[0].origin, snapshot.prefix[0].origin);
    assert_eq!(canonical[1].origin, snapshot.prefix[1].origin);
    assert_eq!(canonical[0].item, canonical[1].item);
    assert_eq!(canonical[0].overlays[0].text, TRIMMED);
    assert!(canonical[1].overlays.is_empty());
}

#[test]
fn canonical_checkpoint_copy_preserves_projection_collision_without_ambiguity() {
    let (store, snapshot) = message_fixture("original", "trimmed");
    let receipt = commit_messages(&store, &snapshot, &["trimmed"]);
    let next = RequestId("next-message".into());
    store
        .write_request(
            &next,
            Some(&receipt.head),
            "/root",
            &[call("checkpoint", false)],
            Usage::default(),
        )
        .unwrap();
    let operation = store.claim(&CallId("checkpoint".into()), &next).unwrap();
    store.lock().execute("INSERT INTO agents(path,head_request,contract,fork_source,state,created_at) VALUES('/root',?1,'{}','{}','active',0)",[&next.0]).unwrap();
    let cuts = store
        .capture_checkpoint_cuts_at_head(&operation, &next, &json!({}), std::sync::Arc::new(()))
        .unwrap();
    let canonical =
        request_occurrences(&store.lock(), cuts.before_call().snapshot_request()).unwrap();
    assert_eq!(canonical[0].origin, snapshot.prefix[0].origin);
    assert_eq!(canonical[1].origin, snapshot.prefix[1].origin);
    assert_eq!(canonical[0].overlays[0].text, "trimmed");
    assert!(canonical[1].overlays.is_empty());
}

#[test]
fn ambiguous_projected_compaction_refuses_both_item_orders_atomically() {
    for reversed in [false, true] {
        let (store, snapshot) = message_fixture("a", "b");
        let receipt = commit_messages(&store, &snapshot, &["b", "c"]);
        let mut items = store
            .context_request_state(&receipt.head, &snapshot.operation.origin)
            .unwrap()
            .history
            .into_iter()
            .map(|(_, _, item)| item)
            .collect::<Vec<_>>();
        if reversed {
            items.swap(0, 1);
        }
        let target = RequestId("ambiguous-compaction".into());
        assert!(matches!(
            store.write_compaction_request(&target, &receipt.head, "/root", &items),
            Err(StoreError::Context(ContextError::InvalidReference))
        ));
        assert!(store.request(&target).unwrap().is_none());
        assert_eq!(
            store
                .context_request_state(&receipt.head, &snapshot.operation.origin)
                .unwrap()
                .history[0]
                .2
                .0["content"],
            "b"
        );
    }
}

#[test]
fn v10_database_migration_preserves_original_bytes_and_defaults_to_identity() {
    let mut c = Connection::open_in_memory().unwrap();
    c.execute_batch(&super::super::schema::SQL.replace(" context_overlays TEXT,\n", ""))
        .unwrap();
    c.execute("INSERT INTO schema_version VALUES(10)", [])
        .unwrap();
    c.execute("INSERT INTO requests(id,branch) VALUES('old','/root')", [])
        .unwrap();
    c.execute("INSERT INTO items(hash,json) VALUES('opaque','{\"type\":\"reasoning\",\"encrypted_content\":\"same bytes\"}')", []).unwrap();
    c.execute(
        "INSERT INTO request_items(request_id,position,item_hash) VALUES('old',0,'opaque')",
        [],
    )
    .unwrap();
    super::super::schema::initialize(&mut c).unwrap();
    let original = request_occurrences(&c, &RequestId("old".into())).unwrap();
    assert_eq!(original[0].item.0["encrypted_content"], "same bytes");
    assert!(original[0].overlays.is_empty());
    assert_eq!(
        c.query_row("SELECT version FROM schema_version", [], |row| row
            .get::<_, u32>(0))
            .unwrap(),
        11
    );
}

#[test]
fn sealed_native_overlay_replay_resolves_local_owner_without_changing_terminal() {
    let original = fixture(Store::memory().unwrap());
    commit(
        &original,
        &draft(&original.snapshot, "verbose result", TRIMMED),
    );
    let evidence = original
        .store
        .context_commit_evidence(&original.edit)
        .unwrap()
        .unwrap();
    let local = fixture(Store::memory().unwrap());
    let terminal = local
        .store
        .replay_tool_output_operation(&local.completed)
        .unwrap();
    let receipt = local
        .store
        .restore_context_commit(
            &local.snapshot,
            &evidence,
            &output(),
            std::slice::from_ref(&local.pending),
        )
        .unwrap();
    assert!(
        local
            .store
            .context_request_state(&receipt.head, &local.edit.origin)
            .unwrap()
            .history
            .iter()
            .any(|(_, _, item)| item.0["output"] == TRIMMED)
    );
    assert_eq!(
        local
            .store
            .replay_tool_output_operation(&local.completed)
            .unwrap(),
        terminal
    );
    assert_eq!(
        local
            .store
            .restore_context_commit(&local.snapshot, &evidence, &output(), &[])
            .unwrap(),
        receipt
    );
}

#[test]
fn visible_result_without_exact_terminal_has_no_edit_authority() {
    let fixture = fixture(Store::memory().unwrap());
    fixture
        .store
        .lock()
        .execute(
            "DELETE FROM claims WHERE origin_request_id=?1 AND call_id=?2",
            params![fixture.completed.request.0, fixture.completed.call.0],
        )
        .unwrap();
    let snapshot = fixture
        .store
        .begin_context(&fixture.edit, &fixture.head)
        .unwrap();
    let mut document = snapshot.document.clone();
    let field = document
        .blocks
        .iter_mut()
        .find_map(|block| match block {
            ContextBlock::Native { texts, .. } => texts
                .iter_mut()
                .find(|field| field.text == "verbose result"),
            _ => None,
        })
        .unwrap();
    assert!(!field.editable);
    field.text = TRIMMED.into();
    assert!(matches!(
        fixture.store.commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                document,
                next_model: None,
                next_effort: None
            },
            output: &output(),
            pending: &[]
        }),
        Err(StoreError::Context(ContextError::NativeEdit))
    ));
}
