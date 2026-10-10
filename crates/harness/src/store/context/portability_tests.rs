use super::*;
use crate::{
    model::{CallId, Effort},
    store::{ClaimState, Usage},
    transport::{ResponsesRequest, ResponsesTurn},
};

fn reasoning() -> Item {
    Item(
        json!({"type":"reasoning","encrypted_content":"model-a-secret", "summary":[
            {"type":"summary_text","text":"visible reasoning"}
        ]}),
    )
}

fn call(id: &str) -> Item {
    Item(
        json!({"type":"custom_tool_call","call_id":id,"name":"haskell_sync","input":"setNextModel modelB"}),
    )
}

fn record(store: &Store, head: &RequestId, items: Vec<Item>, model: &str) {
    store
        .record_replay_turn(
            head,
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
                response_id: "test".into(),
                items,
                usage: Default::default(),
            },
        )
        .unwrap();
}

fn setup(extra: Vec<Item>) -> (Store, RequestId, OperationId, ContextSnapshot) {
    let store = Store::memory().unwrap();
    let head = RequestId("response".into());
    let mut items = vec![reasoning(), call("edit")];
    items.extend(extra);
    store
        .write_request(&head, None, "/root", &[], Usage::default())
        .unwrap();
    store.set_effort(&head, Effort::Low).unwrap();
    store.append_items(&head, &items).unwrap();
    record(&store, &head, items, "model-a");
    let operation = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_model(&operation.origin, "model-a")
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    (store, head, operation, snapshot)
}

fn output() -> JobOutput {
    JobOutput::Completed(Ok(json!("real successful result")))
}

fn switch(store: &Store, snapshot: &ContextSnapshot) -> Result<ContextCommitReceipt> {
    store.commit_context(ContextCommit {
        snapshot,
        draft: &ContextDraft {
            document: snapshot.document.clone(),
            next_model: Some("model-b".into()),
            next_effort: None,
        },
        output: &output(),
        pending: &[],
    })
}

#[test]
fn successful_issuing_opaque_cell_switches_using_exact_real_output() {
    let (store, head, operation, snapshot) = setup(vec![Item(json!({
        "type":"message","role":"assistant","content":[
            {"type":"output_text","text":"first visible"},
            {"type":"output_text","text":"second visible"}
        ]
    }))]);
    let original = store.context_history(&head).unwrap();
    let receipt = switch(&store, &snapshot).unwrap();
    let projected = store
        .context_request_state(&receipt.head, &operation.origin)
        .unwrap();
    assert_eq!(projected.model.as_deref(), Some("model-b"));
    let text = projected
        .history
        .iter()
        .find_map(|(_, _, i)| {
            i.0["content"]
                .as_str()
                .filter(|t| t.starts_with("[Store-generated model portability note"))
        })
        .unwrap();
    for visible in [
        "visible reasoning",
        "first visible",
        "second visible",
        "setNextModel modelB",
        "real successful result",
        "response",
        &original[1].1.0,
    ] {
        assert!(text.contains(visible), "missing {visible}: {text}");
    }
    assert!(!text.contains("model-a-secret"));
    assert!(
        !projected
            .history
            .iter()
            .any(|(_, _, i)| opaque(i) || i.tool_call().unwrap().is_some())
    );
    let raw = store.context_history(&receipt.head).unwrap();
    assert_eq!(&raw[..original.len()], original.as_slice());
    assert_eq!(raw.last().unwrap().2.0["output"], "real successful result");
    assert!(
        store
            .claims_for_operation(&operation)
            .unwrap()
            .iter()
            .all(|c| c.state == ClaimState::Settled)
    );
    assert!(
        store
            .read_context(&receipt.head)
            .unwrap()
            .blocks
            .iter()
            .any(|b| matches!(
                b,
                ContextBlock::Native {
                    kind: ContextNativeKind::Opaque,
                    ..
                }
            ))
    );
    for (_, hash, item) in projected.history {
        assert_eq!(store.put_item(&item).unwrap(), hash);
    }
}

#[test]
fn same_model_request_preserves_exact_bytes_and_hashes() {
    let (store, head, operation, _) = setup(vec![]);
    let raw = store.context_history(&head).unwrap();
    let projected = store
        .context_request_state(&head, &operation.origin)
        .unwrap();
    assert_eq!(projected.history, raw);
}

#[test]
fn foreign_pending_call_in_opaque_envelope_refuses_and_rolls_back() {
    let (store, head, operation, snapshot) = setup(vec![call("foreign")]);
    let foreign = store.claim(&CallId("foreign".into()), &head).unwrap();
    let before = store.context_history(&head).unwrap();
    assert!(matches!(
        switch(&store, &snapshot),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
    assert_eq!(store.context_history(&head).unwrap(), before);
    assert_eq!(
        store.context_model(&operation.origin).unwrap().as_deref(),
        Some("model-a")
    );
    assert!(store.context_receipt(&operation).unwrap().is_none());
    for op in [operation, foreign] {
        assert!(
            store
                .claims_for_operation(&op)
                .unwrap()
                .iter()
                .all(|c| c.state == ClaimState::Pending && c.output.is_none())
        );
    }
}

#[test]
fn actual_output_without_settled_claim_does_not_close_foreign_call() {
    let (store, head, _, snapshot) = setup(vec![call("foreign")]);
    let foreign = store.claim(&CallId("foreign".into()), &head).unwrap();
    let before = store.context_history(&head).unwrap();
    assert!(matches!(
        store.append_items(
            &head,
            &[Item(
                json!({"type":"custom_tool_call_output","call_id":"foreign","output":"unowned"})
            )]
        ),
        Err(StoreError::UnboundOutputPublication { .. })
    ));
    assert_eq!(store.context_history(&head).unwrap(), before);
    assert!(matches!(
        switch(&store, &snapshot),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
    assert_eq!(
        store.claims_for_operation(&foreign).unwrap()[0].state,
        ClaimState::Pending
    );
}

#[test]
fn arrivals_keep_order_outside_projected_envelope() {
    let (store, head, operation, snapshot) = setup(vec![]);
    for text in ["arrival one", "arrival two"] {
        store
            .append_items(
                &head,
                &[Item(json!({"type":"message","role":"user","content":text}))],
            )
            .unwrap();
    }
    let receipt = switch(&store, &snapshot).unwrap();
    let projected = store
        .context_request_state(&receipt.head, &operation.origin)
        .unwrap();
    // Arrivals retain their role, exact bytes, provenance and ordered positions
    // outside the projected provider response family.
    let text = projected
        .history
        .iter()
        .find_map(|(_, _, i)| {
            i.0["content"]
                .as_str()
                .filter(|t| t.starts_with("[Store-generated model portability note"))
        })
        .unwrap();
    assert!(!text.contains("arrival one") && !text.contains("arrival two"));
    assert!(text.contains("real successful result"));
    assert_eq!(projected.history[2].2.0["content"], "arrival one");
    assert_eq!(projected.history[3].2.0["content"], "arrival two");
    let raw = store.context_history(&receipt.head).unwrap();
    assert_eq!(raw[3].2.0["content"], "arrival one");
    assert_eq!(raw[4].2.0["content"], "arrival two");
}

#[test]
fn interleaved_unrelated_calls_arrivals_and_settings_remain_exact() {
    let (store, head, operation, snapshot) = setup(vec![]);
    let foreign_call = call("unrelated");
    store
        .append_items(
            &head,
            &[
                Item(json!({"type":"message","role":"user","content":"outside user"})),
                foreign_call,
                Item(json!({"type":"message","role":"developer","content":"standing instruction"})),
            ],
        )
        .unwrap();
    store.set_effort(&head, Effort::Medium).unwrap();
    let unrelated = store.claim(&CallId("unrelated".into()), &head).unwrap();
    let unrelated_output = Item(
        json!({"type":"custom_tool_call_output","call_id":"unrelated","output":"outside result"}),
    );
    store
        .write_output(&unrelated, &unrelated_output, TerminalOutcome::Success)
        .unwrap();
    let receipt = switch(&store, &snapshot).unwrap();
    // The unrelated invocation's result can arrive after the switching result.
    store
        .append_operation_output(&unrelated, &head, &receipt.head)
        .unwrap();
    let raw = store.context_history(&receipt.head).unwrap();
    let projected = store
        .context_request_state(&receipt.head, &operation.origin)
        .unwrap();
    let expected = raw
        .iter()
        .filter(|(_, _, item)| {
            item.0["type"] != "reasoning" && item.0["call_id"].as_str() != Some("edit")
        })
        .cloned()
        .collect::<Vec<_>>();
    let preserved = projected
        .history
        .iter()
        .filter(|(_, _, item)| {
            !item.0["content"]
                .as_str()
                .is_some_and(|text| text.starts_with("[Store-generated model portability note"))
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(preserved, expected);
    assert_eq!(
        preserved
            .iter()
            .filter(|(_, _, i)| i.is_configuration_update())
            .count(),
        2
    );
    let note = projected
        .history
        .iter()
        .find_map(|(_, _, item)| {
            item.0["content"]
                .as_str()
                .filter(|text| text.starts_with("[Store-generated model portability note"))
        })
        .unwrap();
    for excluded in [
        "outside user",
        "unrelated",
        "standing instruction",
        "outside result",
    ] {
        assert!(!note.contains(excluded), "swallowed {excluded}: {note}");
    }
}

#[test]
fn empty_here_child_without_source_remains_a_valid_frozen_context() {
    let store = Store::memory().unwrap();
    let parent = AgentPath("/root".into());
    store
        .admit_agent(&parent, None, None, &json!({}), &json!({}))
        .unwrap();
    let child = AgentPath("/root/child".into());
    let head = RequestId("empty-here-snapshot".into());
    let task = Item(json!({"type":"message","role":"user","content":"fresh child task"}));
    store
        .admit_here_agent_with_snapshot(
            &child,
            &parent,
            &head,
            &json!({}),
            &parent.0,
            &child.0,
            "new_task",
            &task,
        )
        .unwrap();
    assert_eq!(store.request(&head).unwrap().unwrap().parent, None);
    let identity = store.standalone_identity(child.clone());
    store
        .initialize_context_model(&identity, "gpt-6-luna")
        .unwrap();
    let context = store.context_request_state(&head, &identity).unwrap();
    assert!(
        context
            .history
            .iter()
            .all(|(_, _, item)| item.is_configuration_update())
    );
    assert!(store.pending_at(&head).unwrap().is_empty());
    let inbox = store.unread(&child.0).unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(store.get_item(&inbox[0].item_hash).unwrap(), Some(task));
}

#[test]
fn cross_model_frozen_child_projects_without_losing_source_occurrences_or_claims() {
    let (store, _, operation, snapshot) = setup(vec![]);
    let receipt = switch(&store, &snapshot).unwrap();
    store
        .admit_agent(
            &AgentPath("/root".into()),
            None,
            Some(&receipt.head),
            &json!({}),
            &json!({}),
        )
        .unwrap();
    let child = AgentPath("/root/child".into());
    let child_head = RequestId("child-snapshot".into());
    store
        .admit_here_agent_with_snapshot(
            &child,
            &AgentPath("/root".into()),
            &child_head,
            &json!({}),
            "/root",
            &child.0,
            "new_task",
            &Item(json!({"type":"message","role":"user","content":"child task"})),
        )
        .unwrap();
    let identity = ConversationIdentity::Standalone {
        store: store.store_id.clone(),
        actor: child,
    };
    store
        .initialize_context_model(&identity, "model-b")
        .unwrap();
    let raw_before = store.context_history(&child_head).unwrap();
    let projected = store.context_request_state(&child_head, &identity).unwrap();
    assert!(!projected.history.iter().any(|(_, _, i)| opaque(i)));
    assert!(projected.history.iter().any(|(_, _, i)| {
        i.0["content"]
            .as_str()
            .is_some_and(|t| t.contains("real successful result"))
    }));
    assert_eq!(store.context_history(&child_head).unwrap(), raw_before);
    assert!(raw_before.iter().any(|(_, _, i)| i == &reasoning()));
    assert!(
        store
            .claims_for_operation(&operation)
            .unwrap()
            .iter()
            .all(|c| c.state == ClaimState::Settled)
    );
}

#[test]
fn missing_and_conflicting_issuing_models_fail_closed() {
    for conflicting in [false, true] {
        let (store, head, operation, snapshot) = setup(vec![]);
        if conflicting {
            record(
                &store,
                &head,
                vec![reasoning(), call("edit")],
                "model-other",
            );
        } else {
            store.lock().execute("UPDATE events SET payload=json_remove(payload,'$.issued.model') WHERE kind='model_turn'", []).unwrap();
        }
        assert!(matches!(
            switch(&store, &snapshot),
            Err(StoreError::Context(ContextError::OpaqueModel))
        ));
        assert!(matches!(
            store.context_request_state(&head, &operation.origin),
            Err(StoreError::Context(ContextError::OpaqueModel))
        ));
    }
}

#[test]
fn identical_bytes_do_not_establish_ambiguous_response_origin_membership() {
    let (store, head, operation, _) = setup(vec![reasoning()]);
    // Two equal hashes in the source request cannot be assigned to a response
    // that claims only one of those occurrences.
    store.lock().execute("UPDATE events SET payload=json_remove(payload,'$.response.items[2]') WHERE kind='model_turn'", []).unwrap();
    assert!(matches!(
        store.context_request_state(&head, &operation.origin),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
}

fn replay_collision_fixture(
    identical_group: bool,
    duplicate_spine: bool,
) -> (Store, ContextSnapshot, ContextDocument) {
    let store = Store::memory().unwrap();
    let a = RequestId("original-a".into());
    store
        .write_request(
            &a,
            None,
            "/root",
            &[Item::configuration_update(Effort::Low)],
            Usage::default(),
        )
        .unwrap();
    let a_call = Item(
        json!({"type":"custom_tool_call","call_id":"shared","name":"haskell_sync","input":"original A"}),
    );
    let a_items = vec![reasoning(), a_call];
    store.append_items(&a, &a_items).unwrap();
    record(&store, &a, a_items, "model-a");
    let a_operation = store.claim(&CallId("shared".into()), &a).unwrap();
    store
        .initialize_context_model(&a_operation.origin, "model-a")
        .unwrap();
    let same_output = Item(
        json!({"type":"custom_tool_call_output","call_id":"shared","output":"identical result"}),
    );
    store
        .write_output(&a_operation, &same_output, TerminalOutcome::Success)
        .unwrap();
    let a_output_request = RequestId("original-a-output".into());
    store
        .write_request(&a_output_request, Some(&a), "/root", &[], Usage::default())
        .unwrap();
    store
        .append_operation_output(&a_operation, &a, &a_output_request)
        .unwrap();
    let saved = store.read_context(&a_output_request).unwrap();
    let drop_head = RequestId("drop-original-a".into());
    store.write_request(&drop_head, Some(&a_output_request), "/root", &[Item(json!({"type":"custom_tool_call","call_id":"drop","name":"haskell_sync","input":"drop"}))], Usage::default()).unwrap();
    let drop_operation = store.claim(&CallId("drop".into()), &drop_head).unwrap();
    let snapshot = store.begin_context(&drop_operation, &drop_head).unwrap();
    let mut document = snapshot.document.clone();
    document.blocks.retain(|block| {
        matches!(
            block,
            ContextBlock::Native {
                protected: true,
                ..
            }
        )
    });
    // Retain a historical v2 deletion fixture without reopening opaque-group
    // deletion to new drafts.
    let legacy = ContextCommitEvidence {
        version: 2,
        original_operation: drop_operation.clone(),
        prefix: snapshot
            .blocks
            .iter()
            .filter(|stored| document.blocks.contains(&stored.block))
            .flat_map(|stored| {
                stored.items.iter().map(|item| {
                    (
                        item.item.clone(),
                        item.sources.clone(),
                        item.note,
                        Vec::new(),
                    )
                })
            })
            .collect(),
        output: Item::tool_output(
            &drop_operation.call,
            crate::item::ToolKind::Custom,
            &output(),
        ),
        invocation: store
            .items(&drop_head)
            .unwrap()
            .into_iter()
            .find(|item| item.0["call_id"] == drop_operation.call.0)
            .unwrap(),
        model: Some("model-b".into()),
        next_effort: None,
    };
    let dropped = store
        .restore_context_commit(&snapshot, &legacy, &output(), &[])
        .unwrap();
    let b = RequestId("current-b".into());
    let b_call = Item(
        json!({"type":"custom_tool_call","call_id":"shared","name":"haskell_sync","input":if identical_group {"original A"} else {"distinct B"}}),
    );
    let mut b_items = vec![];
    if identical_group {
        b_items.push(reasoning());
    }
    b_items.push(b_call);
    store
        .write_request(&b, Some(&dropped.head), "/root", &b_items, Usage::default())
        .unwrap();
    record(&store, &b, b_items, "model-b");
    let b_operation = store.claim(&CallId("shared".into()), &b).unwrap();
    store
        .write_output(&b_operation, &same_output, TerminalOutcome::Success)
        .unwrap();
    store.append_operation_output(&b_operation, &b, &b).unwrap();
    if duplicate_spine {
        store.set_effort(&b, Effort::Low).unwrap();
    }
    let restore = RequestId("restore-collision".into());
    store.write_request(&restore, Some(&b), "/root", &[Item(json!({"type":"custom_tool_call","call_id":"restore","name":"haskell_sync","input":"restore"}))], Usage::default()).unwrap();
    let operation = store.claim(&CallId("restore".into()), &restore).unwrap();
    let snapshot = store.begin_context(&operation, &restore).unwrap();
    (store, snapshot, saved)
}

fn collision_evidence() -> ContextCommitEvidence {
    let (store, snapshot, saved) = replay_collision_fixture(false, false);
    store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                document: saved,
                next_model: None,
                next_effort: None,
            },
            output: &output(),
            pending: &[],
        })
        .unwrap();
    store
        .context_commit_evidence(&snapshot.operation)
        .unwrap()
        .unwrap()
}

#[test]
fn replay_restores_a_whole_native_family_with_its_original_equal_byte_output() {
    let evidence = collision_evidence();
    let (store, snapshot, _) = replay_collision_fixture(false, false);
    let receipt = store
        .restore_context_commit(&snapshot, &evidence, &output(), &[])
        .unwrap();
    let raw = history(&store.lock(), &receipt.head, true).unwrap();
    let shared = raw
        .iter()
        .filter(|i| i.item.0["call_id"] == "shared")
        .collect::<Vec<_>>();
    assert_eq!(shared.len(), 2);
    assert_eq!(
        shared
            .iter()
            .map(|i| i.origin.request.0.as_str())
            .collect::<Vec<_>>(),
        vec!["original-a", "original-a-output"]
    );
    assert_eq!(shared[0].item.0["input"], "original A");
    let projected = store
        .context_request_state(&receipt.head, &snapshot.operation.origin)
        .unwrap();
    let note = projected
        .history
        .iter()
        .find_map(|(_, _, i)| {
            i.0["content"]
                .as_str()
                .filter(|text| text.starts_with("[Store-generated model portability note"))
        })
        .unwrap();
    assert!(note.contains("original-a") && note.contains("identical result"));
    assert!(!note.contains("current-b"));
}

#[test]
fn replay_refuses_identical_native_groups_with_distinct_origins_atomically() {
    let evidence = collision_evidence();
    let (store, snapshot, _) = replay_collision_fixture(true, false);
    let before = store.context_history(&snapshot.head).unwrap();
    let result = store.restore_context_commit(&snapshot, &evidence, &output(), &[]);
    assert!(
        matches!(
            result,
            Err(StoreError::Context(ContextError::ProtectedGroup))
        ),
        "{result:?}"
    );
    assert_eq!(store.context_history(&snapshot.head).unwrap(), before);
    assert!(
        store
            .context_receipt(&snapshot.operation)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .claims_for_operation(&snapshot.operation)
            .unwrap()
            .iter()
            .all(|claim| claim.state == ClaimState::Pending && claim.output.is_none())
    );
}

#[test]
fn replay_cannot_substitute_equal_spine_bytes_for_another_protected_origin() {
    let evidence = collision_evidence();
    let (store, snapshot, _) = replay_collision_fixture(false, true);
    let before = store.context_history(&snapshot.head).unwrap();
    let result = store.restore_context_commit(&snapshot, &evidence, &output(), &[]);
    assert!(
        matches!(
            result,
            Err(StoreError::Context(ContextError::ProtectedGroup))
        ),
        "{result:?}"
    );
    assert_eq!(store.context_history(&snapshot.head).unwrap(), before);
    assert!(
        store
            .context_receipt(&snapshot.operation)
            .unwrap()
            .is_none()
    );
}

#[test]
fn whole_native_replay_preserves_later_async_output_with_reused_historical_call_id() {
    let evidence = collision_evidence();
    let (local, snapshot, _) = replay_collision_fixture(false, false);
    let receipt = local
        .restore_context_commit(&snapshot, &evidence, &output(), &[])
        .unwrap();
    let raw = history(&local.lock(), &receipt.head, true).unwrap();
    let result = raw
        .iter()
        .find(|i| i.item.0["call_id"] == "shared" && i.item.0["type"] == "custom_tool_call_output")
        .unwrap();
    assert_eq!(result.origin.request.0, "original-a-output");
    assert!(
        local
            .claims_on(&RequestId("original-a-output".into()))
            .unwrap()
            .is_empty()
    );
    assert!(
        local
            .claims_on(&RequestId("original-a".into()))
            .unwrap()
            .iter()
            .any(|claim| claim.call_id.0 == "shared" && claim.state == ClaimState::Settled)
    );
    assert!(
        local
            .context_request_state(&receipt.head, &snapshot.operation.origin)
            .unwrap()
            .history
            .iter()
            .any(|(_, _, i)| i.0["content"]
                .as_str()
                .is_some_and(|text| text.contains("identical result")))
    );
}

struct CutFixture {
    store: Store,
    source: RequestId,
    cuts: crate::checkpoint::CheckpointCuts<()>,
    child: crate::store::Agent,
    identity: ConversationIdentity,
}

fn cut_fixture(earlier_pending: bool) -> CutFixture {
    cut_fixture_with_reasoning(earlier_pending, true, reasoning())
}

fn cut_fixture_with_reasoning(earlier_pending: bool, visible: bool, reasoning: Item) -> CutFixture {
    use crate::checkpoint::CheckpointChild;
    let store = Store::memory().unwrap();
    let initial = RequestId("cut-input".into());
    let source = RequestId("cut-response".into());
    store
        .write_request(
            &initial,
            None,
            "/root",
            &[Item(
                json!({"type":"message","role":"user","content":"earlier useful conversation"}),
            )],
            Usage::default(),
        )
        .unwrap();
    store.set_effort(&initial, Effort::Low).unwrap();
    let mut items = vec![reasoning];
    if visible {
        items.push(Item(
            json!({"type":"message","role":"assistant","content":"visible prefix"}),
        ));
    }
    if earlier_pending {
        items.push(call("earlier"));
    }
    items.extend([
        call("capture"),
        Item(json!({"type":"message","role":"assistant","content":"excluded tail"})),
    ]);
    store
        .write_request(&source, Some(&initial), "/root", &items, Usage::default())
        .unwrap();
    record(&store, &source, items, "model-a");
    store.lock().execute("INSERT INTO agents(path,head_request,contract,fork_source,state,created_at) VALUES('/root',?1,'{}','{}','active',0)", [&source.0]).unwrap();
    if earlier_pending {
        store.claim(&CallId("earlier".into()), &source).unwrap();
    }
    let operation = store.claim(&CallId("capture".into()), &source).unwrap();
    let cuts = store
        .capture_checkpoint_cuts(&operation, &json!({}), std::sync::Arc::new(()))
        .unwrap();
    let parent = AgentPath("/root".into());
    let path = AgentPath("/root/child".into());
    let (child, _) = store
        .attach_checkpoint_child(
            cuts.before_call(),
            CheckpointChild {
                path: &path,
                parent: &parent,
                contract: &json!({}),
                checkout: &json!({}),
                task: None,
            },
        )
        .unwrap();
    let identity = store.standalone_identity(path);
    store
        .initialize_context_model(&identity, "model-b")
        .unwrap();
    CutFixture {
        store,
        source,
        cuts,
        child,
        identity,
    }
}

#[test]
fn authenticated_cut_survives_commit_freeze_and_deferred_child_without_later_arrivals() {
    use crate::checkpoint::CheckpointChild;
    let fixture = cut_fixture(false);
    let CutFixture {
        store,
        source,
        child,
        identity,
        ..
    } = fixture;
    let head = RequestId("child-turn".into());
    let items = vec![call("child-capture")];
    store
        .write_request(
            &head,
            child.head_request.as_ref(),
            &child.path.0,
            &items,
            Usage::default(),
        )
        .unwrap();
    record(&store, &head, items, "model-b");
    let operation = store.claim(&CallId("child-capture".into()), &head).unwrap();
    let cuts = store
        .capture_checkpoint_cuts(&operation, &json!({}), std::sync::Arc::new(()))
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
    let committed = store
        .context_request_state(&receipt.head, &identity)
        .unwrap();
    assert!(
        serde_json::to_string(&committed.history)
            .unwrap()
            .contains("visible prefix")
    );
    store
        .append_items(
            &head,
            &[
                Item(json!({"type":"message","role":"user","content":"later parent arrival"})),
                call("later-parent-call"),
            ],
        )
        .unwrap();
    let later = store
        .claim(&CallId("later-parent-call".into()), &head)
        .unwrap();
    store
        .append_items(
            &source,
            &[Item(
                json!({"type":"message","role":"assistant","content":"later issuer arrival"}),
            )],
        )
        .unwrap();
    let path = AgentPath("/root/child/grandchild".into());
    let (grandchild, _) = store
        .attach_checkpoint_child(
            cuts.deferred(),
            CheckpointChild {
                path: &path,
                parent: &child.path,
                contract: &json!({}),
                checkout: &json!({}),
                task: None,
            },
        )
        .unwrap();
    let grandchild_identity = store.standalone_identity(path);
    store
        .initialize_context_model(&grandchild_identity, "model-b")
        .unwrap();
    let projected = store
        .context_request_state(
            grandchild.head_request.as_ref().unwrap(),
            &grandchild_identity,
        )
        .unwrap();
    let rendered = serde_json::to_string(&projected.history).unwrap();
    assert!(rendered.contains("visible prefix"));
    assert!(rendered.contains("earlier useful conversation"));
    assert!(rendered.contains("real successful result"));
    for excluded in [
        "excluded tail",
        "later parent arrival",
        "later issuer arrival",
        "later-parent-call",
        "model-a-secret",
    ] {
        assert!(!rendered.contains(excluded), "leaked {excluded}");
    }
    assert!(
        !store
            .pending_at(grandchild.head_request.as_ref().unwrap())
            .unwrap()
            .iter()
            .any(|pending| pending.operation == later)
    );
    let claims = store.claims_for_operation(&later).unwrap();
    assert!(claims.iter().all(|claim| claim.request == head));
}

#[test]
fn partial_envelope_needs_selected_checkpoint_ancestry_and_exact_prefix_origins() {
    let fixture = cut_fixture(false);
    let CutFixture {
        store,
        source,
        cuts,
        child,
        identity,
    } = fixture;
    let good = store
        .context_request_state(child.head_request.as_ref().unwrap(), &identity)
        .unwrap();
    assert!(
        serde_json::to_string(&good.history)
            .unwrap()
            .contains("visible prefix")
    );
    let copied = RequestId("detached-prefix".into());
    store
        .write_request(&copied, None, &child.path.0, &[], Usage::default())
        .unwrap();
    let originals =
        request_occurrences(&store.lock(), cuts.before_call().snapshot_request()).unwrap();
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        for (position, occurrence) in originals.iter().enumerate() {
            insert_occurrence(&tx, &copied, position as i64, occurrence).unwrap();
        }
        tx.commit().unwrap();
    }
    assert!(matches!(
        store.context_request_state(&copied, &identity),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
    // The real cut is present elsewhere in Store, but not on this lineage.
    let equal = RequestId("equal-byte-prefix".into());
    store
        .write_request(
            &equal,
            None,
            &child.path.0,
            &originals.iter().map(|i| i.item.clone()).collect::<Vec<_>>(),
            Usage::default(),
        )
        .unwrap();
    assert!(matches!(
        store.context_request_state(&equal, &identity),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
    // A checkpoint on the lineage cannot excuse a hole or reintroduced suffix.
    let hole = RequestId("cut-prefix-hole".into());
    store
        .write_request(
            &hole,
            child.head_request.as_ref(),
            &child.path.0,
            &[],
            Usage::default(),
        )
        .unwrap();
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        tx.execute(
            "INSERT INTO session_state(session_id,state,updated_at) VALUES(?1,'true',0)",
            [format!("harness:compaction:{}", hole.0)],
        )
        .unwrap();
        let selected = originals
            .iter()
            .filter(|i| i.item.0["type"] != "message" || i.item.0["role"] == "user");
        for (position, occurrence) in selected.enumerate() {
            insert_occurrence(&tx, &hole, position as i64, occurrence).unwrap();
        }
        tx.commit().unwrap();
    }
    assert!(matches!(
        store.context_request_state(&hole, &identity),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
    let suffix = request_occurrences(&store.lock(), &source)
        .unwrap()
        .into_iter()
        .find(|i| i.item.0["content"] == "excluded tail")
        .unwrap();
    {
        let mut connection = store.lock();
        let tx = connection.transaction().unwrap();
        insert_occurrence(&tx, child.head_request.as_ref().unwrap(), 99, &suffix).unwrap();
        tx.commit().unwrap();
    }
    assert!(matches!(
        store.context_request_state(child.head_request.as_ref().unwrap(), &identity),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
}

#[test]
fn captured_prefix_keeps_prior_pending_call_and_deferred_boundary_refusals() {
    use crate::checkpoint::CheckpointChild;
    let fixture = cut_fixture(true);
    assert!(matches!(
        fixture.store.context_request_state(
            fixture.child.head_request.as_ref().unwrap(),
            &fixture.identity
        ),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
    let fixture = cut_fixture(false);
    let parent = AgentPath("/root".into());
    let path = AgentPath("/root/deferred".into());
    let (child, _) = fixture
        .store
        .attach_checkpoint_child(
            fixture.cuts.deferred(),
            CheckpointChild {
                path: &path,
                parent: &parent,
                contract: &json!({}),
                checkout: &json!({}),
                task: None,
            },
        )
        .unwrap();
    let identity = fixture.store.standalone_identity(path);
    fixture
        .store
        .initialize_context_model(&identity, "model-b")
        .unwrap();
    assert!(matches!(
        fixture
            .store
            .context_request_state(child.head_request.as_ref().unwrap(), &identity),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
}

#[test]
fn authenticated_metadata_only_cut_is_omitted_but_full_or_malformed_groups_refuse() {
    for content in [
        json!([]),
        json!(null),
        json!([{"type":"reasoning_text","text":"auxiliary hidden content"}]),
    ] {
        let item = Item(
            json!({"type":"reasoning","summary":[],"content":content,"encrypted_content":"synthetic continuity"}),
        );
        let fixture = cut_fixture_with_reasoning(false, false, item);
        let state = fixture
            .store
            .context_request_state(
                fixture.child.head_request.as_ref().unwrap(),
                &fixture.identity,
            )
            .unwrap();
        let visible = serde_json::to_string(&state.history).unwrap();
        assert!(visible.contains("earlier useful conversation"));
        assert!(!visible.contains("synthetic continuity"));
        assert!(!visible.contains("auxiliary hidden content"));
        assert!(
            !state
                .history
                .iter()
                .any(|(_, _, item)| item.0["type"] == "reasoning")
        );
        assert!(
            fixture
                .store
                .context_history(fixture.child.head_request.as_ref().unwrap())
                .unwrap()
                .iter()
                .any(|(_, _, item)| item.0["type"] == "reasoning")
        );
    }
    for item in [
        Item(
            json!({"type":"reasoning","summary":[],"unknown_provider_field":"must remain refused"}),
        ),
        Item(
            json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":7}]}),
        ),
        Item(json!({"type":"reasoning","summary":[],"encrypted_content":7})),
    ] {
        let fixture = cut_fixture_with_reasoning(false, false, item);
        assert!(matches!(
            fixture.store.context_request_state(
                fixture.child.head_request.as_ref().unwrap(),
                &fixture.identity
            ),
            Err(StoreError::Context(ContextError::Portability(_)))
        ));
    }
    let store = Store::memory().unwrap();
    let head = RequestId("full-empty-response".into());
    let items = vec![Item(
        json!({"type":"reasoning","summary":[],"content":[],"encrypted_content":"continuity"}),
    )];
    store
        .write_request(&head, None, "/root", &items, Usage::default())
        .unwrap();
    record(&store, &head, items, "model-a");
    let identity = store.standalone_identity(AgentPath("/root".into()));
    store
        .initialize_context_model(&identity, "model-b")
        .unwrap();
    assert!(matches!(
        store.context_request_state(&head, &identity),
        Err(StoreError::Context(ContextError::Portability(
            crate::context::ContextPortabilityRejection::NoVisibleContent
        )))
    ));
}

#[test]
fn genuine_saved_reference_from_unrelated_checkpoint_cannot_authorize_model_switch() {
    let fixture = cut_fixture(false);
    let store = &fixture.store;
    let foreign = store
        .read_context(fixture.child.head_request.as_ref().unwrap())
        .unwrap();
    let foreign_native = foreign
        .blocks
        .into_iter()
        .filter(|block| matches!(block, ContextBlock::Native { texts, .. } if texts.iter().any(|text| text.text == "visible prefix")))
        .collect::<Vec<_>>();
    assert!(!foreign_native.is_empty());
    let head = RequestId("independent-consumer".into());
    store
        .write_request(&head, None, "/other", &[call("edit")], Usage::default())
        .unwrap();
    let retained = store
        .history_occurrences(fixture.child.head_request.as_ref().unwrap())
        .unwrap();
    {
        let connection = store.lock();
        assert!(
            portable_request(
                &connection,
                fixture.child.head_request.as_ref().unwrap(),
                &retained,
                Some("model-b"),
                None
            )
            .is_ok()
        );
        // Genuine retained occurrences carry the checkpoint's physical rows,
        // but only the actual consuming head may select its issued authority.
        assert!(matches!(
            portable_request(&connection, &head, &retained, Some("model-b"), None),
            Err(StoreError::Context(ContextError::OpaqueModel))
        ));
    }
    let operation = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_model(&operation.origin, "model-a")
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let mut document = snapshot.document.clone();
    document.blocks.extend(foreign_native);
    let refusal = store.commit_context(ContextCommit {
        snapshot: &snapshot,
        draft: &ContextDraft {
            document,
            next_model: Some("model-b".into()),
            next_effort: None,
        },
        output: &output(),
        pending: &[],
    });
    // A genuine reference is still unavailable outside its owning lineage;
    // refusal precedes projection, terminal publication and model selection.
    assert!(matches!(
        refusal,
        Err(StoreError::Context(ContextError::InvalidReference))
    ));
    assert!(store.context_receipt(&operation).unwrap().is_none());
    assert!(
        store
            .claims_for_operation(&operation)
            .unwrap()
            .iter()
            .all(|claim| claim.state == ClaimState::Pending)
    );
    assert_eq!(
        state(&store.lock(), &operation.origin)
            .unwrap()
            .model
            .as_deref(),
        Some("model-a")
    );
}
