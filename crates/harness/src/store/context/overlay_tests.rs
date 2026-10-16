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
        .record_replay_turn_for_target(
            head,
            &ResponsesRequest {
                input: vec![],
                instructions: "fixture".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "model-a".into(),
                pinned_effort: Effort::Low,
                session_id: "fixture".into(),
                prompt_cache_comparison: None,
            },
            &ResponsesTurn {
                response_id: head.0.clone(),
                items,
                usage: Default::default(),
            },
            crate::transport::ResponsesRoute::ChatGptPlan,
            crate::transport::ResponsesProtocol::Standard,
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
        .write_request(&previous, None, "/root", &[], Usage::default())
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
        .append_operation_output(&completed, &previous, &previous)
        .unwrap();
    let head = RequestId("editing".into());
    let response = vec![
        Item(json!({"type":"reasoning","encrypted_content":"editor opaque","summary":[]})),
        call("edit", false),
    ];
    store
        .write_request(&head, Some(&previous), "/root", &[], Usage::default())
        .unwrap();
    record(&store, &head, response);
    let edit = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_target(
            &edit.origin,
            &crate::context::ContextProjectionTarget {
                model: "model-a".into(),
                route: crate::transport::ResponsesRoute::ChatGptPlan,
                protocol: crate::transport::ResponsesProtocol::Standard,
            },
        )
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
        .append_operation_output(&fixture.pending, &fixture.pending.request, &receipt.head)
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
        .write_compaction_request_with_evidence(
            &target,
            &receipt.head,
            "/root",
            &projected
                .history
                .iter()
                .map(|(_, _, item)| item.clone())
                .collect::<Vec<_>>(),
            &[],
            &[],
            &projected.occurrences,
            std::slice::from_ref(&fixture.pending),
            None,
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
    let c = store.lock().unwrap();
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
        candidate.as_array_mut().unwrap().pop();
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
        .unwrap()
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
            .unwrap()
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
        .initialize_context_target(
            &operation.origin,
            &crate::context::ContextProjectionTarget {
                model: "model-a".into(),
                route: crate::transport::ResponsesRoute::ChatGptPlan,
                protocol: crate::transport::ResponsesProtocol::Standard,
            },
        )
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
        .write_compaction_request_with_evidence(
            &target,
            &receipt.head,
            "/root",
            &projected
                .history
                .iter()
                .map(|(_, _, item)| item.clone())
                .collect::<Vec<_>>(),
            &projected
                .occurrences
                .iter()
                .enumerate()
                .filter_map(|(position, source)| source.clone().map(|source| (position, source)))
                .collect::<Vec<_>>(),
            &[],
            &projected.occurrences,
            &[],
            None,
            None,
        )
        .unwrap();
    let canonical = request_occurrences(&store.lock().unwrap(), &target).unwrap();
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
    store
        .admit_agent(
            &AgentPath("/root".into()),
            None,
            Some(&next),
            &json!({}),
            &json!({}),
        )
        .unwrap();
    let cuts = store
        .capture_checkpoint_cuts_at_head(&operation, &next, &json!({}), std::sync::Arc::new(()))
        .unwrap();
    let canonical = request_occurrences(
        &store.lock().unwrap(),
        cuts.before_call().snapshot_request(),
    )
    .unwrap();
    assert_eq!(canonical[0].origin, snapshot.prefix[0].origin);
    assert_eq!(canonical[1].origin, snapshot.prefix[1].origin);
    assert_eq!(canonical[0].overlays[0].text, "trimmed");
    assert!(canonical[1].overlays.is_empty());
}

#[test]
fn ambiguous_ordinary_compaction_authors_bytes_without_inventing_provenance() {
    for reversed in [false, true] {
        let (store, snapshot) = message_fixture("a", "b");
        let receipt = commit_messages(&store, &snapshot, &["b", "c"]);
        let projected = store
            .context_request_state(&receipt.head, &snapshot.operation.origin)
            .unwrap();
        let mut items = projected
            .history
            .iter()
            .map(|(_, _, item)| item.clone())
            .collect::<Vec<_>>();
        if reversed {
            items.swap(0, 1);
        }
        let target = RequestId("ambiguous-compaction".into());
        store
            .write_compaction_request_with_evidence(
                &target,
                &receipt.head,
                "/root",
                &items,
                &[],
                &[],
                &projected.occurrences,
                &[],
                None,
                None,
            )
            .unwrap();
        let canonical = request_occurrences(&store.lock().unwrap(), &target).unwrap();
        let ambiguous = &canonical[usize::from(reversed)];
        assert_eq!(ambiguous.item.0["content"], "b");
        assert_eq!(ambiguous.origin.request, target);
        assert_eq!(ambiguous.origin.position, ambiguous.position);
        assert!(ambiguous.overlays.is_empty());
        assert!(ambiguous.sources.is_empty());
        assert!(ambiguous.output_operation.is_none());
        let unambiguous = &canonical[usize::from(!reversed)];
        assert_eq!(unambiguous.origin, snapshot.prefix[1].origin);
        assert_eq!(unambiguous.item.0["content"], "b");
        assert_eq!(unambiguous.overlays[0].text, "c");
        let after = store
            .context_request_state(&target, &snapshot.operation.origin)
            .unwrap();
        assert_eq!(
            after
                .history
                .iter()
                .map(|(_, _, item)| item.clone())
                .collect::<Vec<_>>(),
            items
        );
        assert_eq!(
            store
                .context_request_state(&receipt.head, &snapshot.operation.origin)
                .unwrap()
                .history,
            projected.history
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
        crate::store::schema::VERSION
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
    let before = fixture.store.history_occurrences(&fixture.head).unwrap();
    assert_eq!(
        before
            .iter()
            .filter(|item| item.output_operation.as_ref() == Some(&fixture.completed))
            .count(),
        1
    );
    assert!(
        fixture
            .store
            .replay_tool_output_operation(&fixture.completed)
            .unwrap()
            .is_some()
    );
    fixture
        .store
        .lock().unwrap()
        .execute(
            // Remove terminal authority while retaining immutable issuing evidence.
            "UPDATE claims SET state='pending',output_hash=NULL,terminal_json=NULL WHERE origin_request_id=?1 AND call_id=?2",
            params![fixture.completed.request.0, fixture.completed.call.0],
        )
        .unwrap();
    assert!(
        fixture
            .store
            .replay_tool_output_operation(&fixture.completed)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture.store.history_occurrences(&fixture.head).unwrap(),
        before
    );
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
    assert_eq!(
        fixture.store.history_occurrences(&fixture.head).unwrap(),
        before
    );
    assert!(
        fixture
            .store
            .context_receipt(&fixture.edit)
            .unwrap()
            .is_none()
    );
}

fn next_note_editor(
    store: &Store,
    parent: &RequestId,
    ordinal: usize,
) -> (RequestId, OperationId, ContextSnapshot) {
    let head = RequestId(format!("note-edit-{ordinal}"));
    store
        .write_request(&head, Some(parent), "/root", &[], Usage::default())
        .unwrap();
    let call_id = format!("note-edit-{ordinal}");
    record(
        store,
        &head,
        vec![
            Item(
                json!({"type":"reasoning","encrypted_content":format!("opaque-editor-{ordinal}"),"summary":[]}),
            ),
            Item(
                json!({"type":"custom_tool_call","call_id":call_id,"name":"cell","input":"edit context"}),
            ),
        ],
    );
    let operation = store.claim(&CallId(call_id), &head).unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    (head, operation, snapshot)
}

fn note_draft(
    snapshot: &ContextSnapshot,
    append: Option<&str>,
    remove: Option<&ContextReference>,
) -> ContextDraft {
    let mut document = snapshot.document.clone();
    if let Some(selected) = remove {
        document.blocks.retain(|block| !matches!(block, ContextBlock::Text {reference: Some(reference),..} if reference == selected));
        for block in &mut document.blocks {
            if let ContextBlock::Native { texts, .. } = block {
                texts.retain(|field| &field.reference != selected);
            }
        }
    }
    if let Some(body) = append {
        document.blocks.push(ContextBlock::Text {
            reference: None,
            role: crate::context::ContextRole::Assistant,
            text: body.into(),
            sources: vec![],
        });
    }
    ContextDraft {
        document,
        next_model: None,
        next_effort: None,
    }
}

fn projected_notes(
    store: &Store,
    head: &RequestId,
    identity: &ConversationIdentity,
) -> Vec<String> {
    store
        .context_request_state(head, identity)
        .unwrap()
        .history
        .into_iter()
        .filter_map(|(_, _, item)| {
            item.0["content"]
                .as_str()
                .filter(|body| body.starts_with("[Agent-authored context note]\n"))
                .map(str::to_owned)
        })
        .collect()
}

#[test]
fn authored_note_removal_preserves_opaque_members_and_replays_after_reopen() {
    let original = fixture(Store::memory().unwrap());
    let first = commit(
        &original,
        &note_draft(&original.snapshot, Some("same note"), None),
    );
    let (_, _, second_snapshot) = next_note_editor(&original.store, &first.head, 1);
    let second = original
        .store
        .commit_context(ContextCommit {
            snapshot: &second_snapshot,
            draft: &note_draft(&second_snapshot, Some("same note"), None),
            output: &output(),
            pending: std::slice::from_ref(&original.pending),
        })
        .unwrap();
    let (head, edit, snapshot) = next_note_editor(&original.store, &second.head, 2);
    let notes = snapshot
        .document
        .blocks
        .iter()
        .flat_map(|block| match block {
            ContextBlock::Native { texts, .. } => texts
                .iter()
                .filter(|field| field.removable)
                .cloned()
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect::<Vec<_>>();
    assert_eq!(
        notes.len(),
        2,
        "both authored occurrences remain individually addressable within opaque groups"
    );
    assert_ne!(notes[0].reference, notes[1].reference);
    let before = original.store.history_occurrences(&head).unwrap();
    let native = before
        .iter()
        .filter(|occurrence| !occurrence.note)
        .map(|occurrence| (occurrence.origin.clone(), occurrence.item.clone()))
        .collect::<Vec<_>>();
    let path = std::env::temp_dir().join(format!("note-replay-{}.sqlite", uuid::Uuid::new_v4()));
    original
        .store
        .lock()
        .unwrap()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let terminal = original
        .store
        .replay_tool_output_operation(&original.completed)
        .unwrap();
    let receipt = original
        .store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &note_draft(&snapshot, None, Some(&notes[0].reference)),
            output: &output(),
            pending: std::slice::from_ref(&original.pending),
        })
        .unwrap();
    assert_eq!(
        projected_notes(&original.store, &receipt.head, &edit.origin),
        vec!["[Agent-authored context note]\nsame note"]
    );
    let after = original.store.history_occurrences(&receipt.head).unwrap();
    for (origin, item) in &native {
        assert!(
            after
                .iter()
                .any(|actual| &actual.origin == origin && &actual.item == item),
            "native member/order remains owned"
        );
    }
    let retained_native = after
        .iter()
        .filter(|occurrence| {
            native
                .iter()
                .any(|(origin, _)| origin == &occurrence.origin)
        })
        .map(|occurrence| (occurrence.origin.clone(), occurrence.item.clone()))
        .collect::<Vec<_>>();
    assert_eq!(retained_native, native);
    let evidence = original
        .store
        .context_commit_evidence(&edit)
        .unwrap()
        .unwrap();
    let local = Store::open(&path).unwrap();
    let local_snapshot = local.begin_context(&edit, &head).unwrap();
    let restored = local
        .restore_context_commit(
            &local_snapshot,
            &evidence,
            &output(),
            std::slice::from_ref(&original.pending),
        )
        .unwrap();
    assert_eq!(
        projected_notes(&local, &restored.head, &edit.origin),
        vec!["[Agent-authored context note]\nsame note"]
    );
    assert_eq!(
        local
            .replay_tool_output_operation(&original.completed)
            .unwrap(),
        terminal
    );
    assert_eq!(
        local
            .restore_context_commit(&local_snapshot, &evidence, &output(), &[])
            .unwrap(),
        restored
    );
    drop(local);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn completed_native_history_replays_repeated_note_removal() {
    let fixture = fixture(Store::memory().unwrap());
    let first = commit(&fixture, &note_draft(&fixture.snapshot, Some("same"), None));
    let (_, _, snapshot) = next_note_editor(&fixture.store, &first.head, 1);
    let second = fixture
        .store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &note_draft(&snapshot, Some("same"), None),
            output: &output(),
            pending: std::slice::from_ref(&fixture.pending),
        })
        .unwrap();
    let (_, _, snapshot) = next_note_editor(&fixture.store, &second.head, 2);
    let references = snapshot
        .document
        .blocks
        .iter()
        .flat_map(|block| match block {
            ContextBlock::Native { texts, .. } => texts
                .iter()
                .filter(|field| field.removable)
                .map(|field| field.reference.clone())
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect::<Vec<_>>();
    assert_eq!(references.len(), 2);
    let removed = fixture
        .store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &note_draft(&snapshot, None, Some(&references[0])),
            output: &output(),
            pending: std::slice::from_ref(&fixture.pending),
        })
        .unwrap();
    fixture
        .store
        .write_job_output(
            &fixture.pending,
            ToolKind::Custom,
            &JobOutput::Completed(Ok(json!("pending finished"))),
        )
        .unwrap();
    fixture
        .store
        .append_operation_output(&fixture.pending, &fixture.pending.request, &removed.head)
        .unwrap();
    let (_, _, snapshot) = next_note_editor(&fixture.store, &removed.head, 3);
    let edited = fixture
        .store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &draft(&snapshot, "same", "edited"),
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let (head, operation, snapshot) = next_note_editor(&fixture.store, &edited.head, 4);
    let path = std::env::temp_dir().join(format!(
        "completed-note-replay-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    fixture
        .store
        .lock()
        .unwrap()
        .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let removed = fixture
        .store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &note_draft(&snapshot, None, Some(&references[1])),
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let evidence = fixture
        .store
        .context_commit_evidence(&operation)
        .unwrap()
        .unwrap();
    let restored_store = Store::open(&path).unwrap();
    let snapshot = restored_store.begin_context(&operation, &head).unwrap();
    let restored = restored_store
        .restore_context_commit(&snapshot, &evidence, &output(), &[])
        .unwrap();
    assert_eq!(
        projected_notes(&fixture.store, &removed.head, &operation.origin),
        Vec::<String>::new()
    );
    assert_eq!(
        projected_notes(&restored_store, &restored.head, &operation.origin),
        Vec::<String>::new()
    );
    assert_eq!(
        restored_store
            .restore_context_commit(&snapshot, &evidence, &output(), &[])
            .unwrap(),
        restored
    );
    assert_eq!(
        restored_store
            .replay_tool_output_operation(&fixture.pending)
            .unwrap(),
        fixture
            .store
            .replay_tool_output_operation(&fixture.pending)
            .unwrap()
    );
    drop(restored_store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn note_removal_cannot_drop_provider_text_or_forge_removal_metadata() {
    let fixture = fixture(Store::memory().unwrap());
    let before = fixture
        .store
        .context_request_state(&fixture.head, &fixture.edit.origin)
        .unwrap()
        .history;
    for forge in [false, true] {
        let mut draft = note_draft(&fixture.snapshot, None, None);
        let texts = draft
            .document
            .blocks
            .iter_mut()
            .find_map(|block| match block {
                ContextBlock::Native { texts, .. }
                    if texts.iter().any(|field| field.text == "verbose result") =>
                {
                    Some(texts)
                }
                _ => None,
            })
            .unwrap();
        let index = texts
            .iter()
            .position(|field| field.text == "verbose result")
            .unwrap();
        if forge {
            texts[index].removable = true;
        } else {
            texts.remove(index);
        }
        assert!(matches!(
            fixture.store.commit_context(ContextCommit {
                snapshot: &fixture.snapshot,
                draft: &draft,
                output: &output(),
                pending: std::slice::from_ref(&fixture.pending)
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
        assert_eq!(
            fixture
                .store
                .context_request_state(&fixture.head, &fixture.edit.origin)
                .unwrap()
                .history,
            before
        );
    }
}

#[test]
fn authored_note_histories_match_provider_projection() {
    use proptest::prelude::*;
    let strategy = proptest::collection::vec((0u8..6, 0usize..5, 0u8..3), 1..18);
    proptest::test_runner::TestRunner::new(proptest::test_runner::Config {
        cases: 48,
        ..Default::default()
    })
    .run(&strategy, |steps| {
        let path =
            std::env::temp_dir().join(format!("note-history-{}.sqlite", uuid::Uuid::new_v4()));
        let fixture = fixture(Store::open(&path).unwrap());
        let mut store = fixture.store;
        let pending = fixture.pending;
        let mut model = Vec::<(ContextReference, String)>::new();
        let initial = store
            .commit_context(ContextCommit {
                snapshot: &fixture.snapshot,
                draft: &note_draft(&fixture.snapshot, Some("note-0"), None),
                output: &output(),
                pending: std::slice::from_ref(&pending),
            })
            .unwrap();
        let mut head = initial.head;
        for (step, (kind, slot, body)) in std::iter::once((0, 0, 0)).chain(steps).enumerate() {
            if kind == 5 {
                drop(store);
                store = Store::open(&path).unwrap();
            }
            let (_, operation, snapshot) = next_note_editor(&store, &head, step + 1);
            let observed = snapshot
                .document
                .blocks
                .iter()
                .flat_map(|block| match block {
                    ContextBlock::Native { texts, .. } => texts
                        .iter()
                        .filter(|field| field.removable)
                        .map(|field| (field.reference.clone(), field.text.clone()))
                        .collect::<Vec<_>>(),
                    ContextBlock::Text {
                        reference: Some(reference),
                        text,
                        ..
                    } => vec![(reference.clone(), text.clone())],
                    _ => vec![],
                })
                .collect::<Vec<_>>();
            if model.is_empty() && step == 0 {
                model = observed.clone();
            }
            prop_assert_eq!(&observed, &model);
            let selected = (!model.is_empty()).then(|| model[slot % model.len()].0.clone());
            let append = (kind <= 1).then(|| format!("note-{body}"));
            let remove = (kind == 2).then_some(selected.as_ref()).flatten();
            let mut draft = note_draft(&snapshot, append.as_deref(), remove);
            if kind == 3 {
                if let Some(selected) = &selected {
                    for block in &mut draft.document.blocks {
                        if let ContextBlock::Native { texts, .. } = block {
                            for field in texts {
                                if &field.reference == selected {
                                    field.text = format!("note-{body}");
                                }
                            }
                        }
                    }
                }
            }
            if kind == 4 {
                prop_assert!(
                    matches!(
                        store.commit_context_guarded(
                            ContextCommit {
                                snapshot: &snapshot,
                                draft: &draft,
                                output: &output(),
                                pending: std::slice::from_ref(&pending)
                            },
                            || true
                        ),
                        Err(StoreError::Context(ContextError::Cancelled))
                    ),
                    "cancelled context must refuse publication"
                );
            }
            let receipt = store
                .commit_context(ContextCommit {
                    snapshot: &snapshot,
                    draft: &draft,
                    output: &output(),
                    pending: std::slice::from_ref(&pending),
                })
                .unwrap();
            head = receipt.head;
            if let Some(selected) = remove {
                model.retain(|(reference, _)| reference != selected);
            }
            if kind == 3 {
                if let Some(selected) = selected {
                    if let Some((_, text)) = model
                        .iter_mut()
                        .find(|(reference, _)| reference == &selected)
                    {
                        *text = format!("note-{body}");
                    }
                }
            }
            if let Some(body) = append {
                let occurrences = store.history_occurrences(&head).unwrap();
                let new = occurrences
                    .iter()
                    .filter(|occurrence| occurrence.note)
                    .map(|occurrence| reference(std::slice::from_ref(occurrence)))
                    .filter(|reference| !model.iter().any(|(known, _)| known == reference))
                    .collect::<Vec<_>>();
                prop_assert_eq!(new.len(), 1);
                model.push((new[0].clone(), body));
            }
            prop_assert_eq!(
                projected_notes(&store, &head, &operation.origin),
                model
                    .iter()
                    .map(|(_, body)| format!("[Agent-authored context note]\n{body}"))
                    .collect::<Vec<_>>()
            );
        }
        drop(store);
        std::fs::remove_file(path).unwrap();
        Ok(())
    })
    .unwrap();
}

#[test]
fn inspection_authorship_follows_canonical_native_fields_and_legacy_user_notes() {
    use crate::context::{ContextTextAuthor, inspect_context};
    let fixture = fixture(Store::memory().unwrap());
    let native = fixture.store.inspect_context(&fixture.snapshot).unwrap();
    assert!(
        inspect_context(&fixture.snapshot.document)
            .authorship
            .is_empty()
    );
    assert_eq!(
        native.text_versions,
        inspect_context(&fixture.snapshot.document).text_versions
    );
    assert_eq!(
        native.unit_versions,
        inspect_context(&fixture.snapshot.document).unit_versions
    );
    let mut expected = Vec::new();
    for block in &fixture.snapshot.document.blocks {
        if let ContextBlock::Native { texts, .. } = block {
            for field in texts {
                let occurrence = fixture
                    .snapshot
                    .prefix
                    .iter()
                    .find(|item| reference(std::slice::from_ref(*item)) == field.reference)
                    .unwrap();
                let author = if occurrence.item.0["type"] == "message" {
                    ContextTextAuthor::Assistant
                } else {
                    ContextTextAuthor::Tool
                };
                expected.push((
                    field.reference.clone(),
                    field.selector.clone(),
                    author,
                    false,
                ));
            }
        }
    }
    assert!(!expected.is_empty());
    assert_eq!(
        native
            .authorship
            .iter()
            .map(|field| (
                field.reference.clone(),
                field.selector.clone(),
                field.author,
                field.authored_note
            ))
            .collect::<Vec<_>>(),
        expected
    );
    let assistant = native
        .authorship
        .iter()
        .filter(|field| field.author == ContextTextAuthor::Assistant)
        .collect::<Vec<_>>();
    assert_eq!(assistant.len(), 2);
    assert_eq!(assistant[0].reference, assistant[1].reference);
    assert_ne!(assistant[0].selector, assistant[1].selector);
    assert!(
        native
            .authorship
            .iter()
            .any(|field| field.author == ContextTextAuthor::Tool
                && field.selector == ContextTextSelector::ToolResultText)
    );
    assert!(fixture.snapshot.document.blocks.iter().any(|block| matches!(block, ContextBlock::Native { protected:true, texts, .. } if texts.iter().any(|text| native.authorship.iter().any(|field| field.reference == text.reference && field.selector == text.selector)))));
    let mut legacy = note_draft(&fixture.snapshot, None, None);
    legacy.document.blocks.push(ContextBlock::Text {
        reference: None,
        role: ContextRole::User,
        text: "legacy authored user note".into(),
        sources: vec![],
    });
    let receipt = fixture
        .store
        .commit_context(ContextCommit {
            snapshot: &fixture.snapshot,
            draft: &legacy,
            output: &output(),
            pending: std::slice::from_ref(&fixture.pending),
        })
        .unwrap();
    let (_, _, snapshot) = next_note_editor(&fixture.store, &receipt.head, 9001);
    let reread = fixture.store.inspect_context(&snapshot).unwrap();
    let note = snapshot
        .prefix
        .iter()
        .find(|item| item.note && item.item.0["role"] == "user")
        .unwrap();
    let note_reference = reference(std::slice::from_ref(note));
    assert!(
        reread
            .authorship
            .iter()
            .any(|field| field.reference == note_reference
                && field.author == ContextTextAuthor::User
                && field.authored_note)
    );
    for original in &native.authorship {
        assert!(reread.authorship.contains(original));
    }
    let draft = draft(
        &snapshot,
        "verbose answer",
        "changed visible assistant field",
    );
    let receipt = fixture
        .store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &draft,
            output: &output(),
            pending: std::slice::from_ref(&fixture.pending),
        })
        .unwrap();
    let (_, _, after) = next_note_editor(&fixture.store, &receipt.head, 9002);
    let after = fixture.store.inspect_context(&after).unwrap();
    for original in &reread.authorship {
        assert!(after.authorship.contains(original));
    }
    assert!(after.document.blocks.iter().any(|block| matches!(block, ContextBlock::Native { texts, .. } if texts.iter().any(|field| field.text == "changed visible assistant field"))));
}
