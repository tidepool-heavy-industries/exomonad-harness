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
    store.claim(&CallId("foreign".into()), &head).unwrap();
    store
        .append_items(
            &head,
            &[Item(
                json!({"type":"custom_tool_call_output","call_id":"foreign","output":"unowned"}),
            )],
        )
        .unwrap();
    assert!(matches!(
        switch(&store, &snapshot),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
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
                Item::configuration_update(Effort::Medium),
            ],
        )
        .unwrap();
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
        .append_items(&receipt.head, &[unrelated_output])
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
