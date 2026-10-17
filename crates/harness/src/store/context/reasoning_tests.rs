use super::*;
use crate::{
    context::{ContextChange, ContextPatch, ContextTransactionDraft, ContextTransactionStatus},
    model::{CallId, Effort},
    store::Usage,
    transport::{ResponsesProtocol, ResponsesRequest, ResponsesRoute, ResponsesTurn},
};

fn call(id: &str) -> Item {
    Item(json!({"type":"function_call","call_id":id,"name":"edit","arguments":"{}"}))
}

fn record(store: &Store, head: &RequestId, items: Vec<Item>, route: ResponsesRoute) {
    store
        .record_replay_turn_for_target(
            head,
            &ResponsesRequest {
                input: vec![],
                instructions: "fixture".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "fixture-model".into(),
                pinned_effort: Effort::High,
                session_id: "fixture".into(),
                prompt_cache_comparison: None,
            },
            &ResponsesTurn {
                response_id: head.0.clone(),
                items,
                usage: Default::default(),
            },
            route,
            ResponsesProtocol::Standard,
        )
        .unwrap();
}

fn fixture(store: &Store, reasoning: Item, route: ResponsesRoute) -> (ContextSnapshot, Item) {
    let previous = RequestId("response".into());
    store
        .write_request(&previous, None, "/root", &[], Usage::default())
        .unwrap();
    record(
        store,
        &previous,
        vec![
            reasoning.clone(),
            Item(
                json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer"}]}),
            ),
        ],
        route,
    );
    let head = RequestId("editor".into());
    store
        .write_request(&head, Some(&previous), "/root", &[], Usage::default())
        .unwrap();
    record(store, &head, vec![call("edit")], route);
    let edit = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_target(
            &edit.origin,
            &ContextProjectionTarget {
                model: "fixture-model".into(),
                route,
                protocol: ResponsesProtocol::Standard,
            },
        )
        .unwrap();
    (store.begin_context(&edit, &head).unwrap(), reasoning)
}

fn fields(snapshot: &ContextSnapshot) -> Vec<ContextVisibleText> {
    snapshot
        .document
        .blocks
        .iter()
        .flat_map(|block| match block {
            ContextBlock::Native { texts, .. } => texts.clone(),
            _ => vec![],
        })
        .filter(|field| matches!(field.selector, ContextTextSelector::ReasoningText { .. }))
        .collect()
}

fn rewrite(
    store: &Store,
    snapshot: &ContextSnapshot,
    field: &ContextVisibleText,
    body: &str,
) -> ContextCommitReceipt {
    let patch = ContextPatch {
        changes: vec![ContextChange::ReplaceContextText {
            reference: field.reference.clone(),
            selector: field.selector.clone(),
            expected: crate::context::context_digest(field.text.as_bytes()),
            text: body.into(),
        }],
    };
    let draft = ContextTransactionDraft {
        patch,
        next_model: None,
        next_effort: None,
    };
    match store
        .commit_context_transaction_guarded(
            snapshot,
            &draft,
            &JobOutput::Completed(Ok(json!("editor done"))),
            &[],
            || false,
        )
        .unwrap()
    {
        ContextTransactionStatus::Committed(receipt) => ContextCommitReceipt {
            head: receipt.head,
            generation: receipt.generation,
            changed: receipt.changed,
            model: None,
        },
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn reasoning_rewrite_preserves_original_and_projects_only_selected_part() {
    let store = Store::memory().unwrap();
    let original = Item(
        json!({"type":"reasoning","id":"r","status":"completed","summary":[],"content":[{"type":"reasoning_text","text":"first λ"},{"type":"reasoning_text","text":"second"}]}),
    );
    let (snapshot, original) = fixture(&store, original, ResponsesRoute::DeepSeek);
    let fields = fields(&snapshot);
    assert_eq!(
        fields
            .iter()
            .map(|f| (&*f.text, f.editable))
            .collect::<Vec<_>>(),
        vec![("first λ", true), ("second", true)]
    );
    let receipt = rewrite(&store, &snapshot, &fields[0], "authored revision");
    assert!(receipt.changed);
    assert_eq!(
        store
            .original_context_text(&snapshot, &fields[0].reference, &fields[0].selector)
            .unwrap()
            .unwrap(),
        "first λ"
    );
    let projected = store
        .context_request_state(&receipt.head, &snapshot.operation.origin)
        .unwrap();
    let actual = projected
        .history
        .iter()
        .find(|(_, _, item)| item.0["type"] == "reasoning")
        .unwrap();
    let mut expected = original.clone();
    expected.0["content"][0]["text"] = json!("authored revision");
    assert_eq!(actual.2, expected);
    let canonical = store
        .context_history(&RequestId("response".into()))
        .unwrap();
    assert_eq!(
        canonical
            .iter()
            .find(|(_, _, item)| item.0["type"] == "reasoning")
            .unwrap()
            .2,
        original
    );
    let inspection = store.inspect_context(&snapshot).unwrap();
    assert!(
        inspection
            .authorship
            .iter()
            .any(|author| author.reference == fields[0].reference
                && author.author == crate::context::ContextTextAuthor::Assistant)
    );
}

#[test]
fn reasoning_is_readable_but_not_editable_for_other_routes_or_unknown_continuity() {
    for (route, extra) in [
        (ResponsesRoute::ChatGptPlan, None),
        (
            ResponsesRoute::DeepSeek,
            Some(("encrypted_content", json!("secret"))),
        ),
        (ResponsesRoute::DeepSeek, Some(("future", json!(true)))),
        (
            ResponsesRoute::DeepSeek,
            Some(("status", json!("incomplete"))),
        ),
        (
            ResponsesRoute::DeepSeek,
            Some(("status", json!("in_progress"))),
        ),
        (ResponsesRoute::DeepSeek, Some(("id", json!(42)))),
        (ResponsesRoute::DeepSeek, Some(("summary", json!(null)))),
        (
            ResponsesRoute::DeepSeek,
            Some((
                "summary",
                json!([{"type":"summary_text","text":"not empty"}]),
            )),
        ),
        (
            ResponsesRoute::DeepSeek,
            Some(("summary", json!("invalid"))),
        ),
    ] {
        let store = Store::memory().unwrap();
        let mut item = Item(
            json!({"type":"reasoning","content":[{"type":"reasoning_text","text":"readable"}]}),
        );
        if let Some((key, value)) = extra {
            item.0[key] = value;
        }
        let (snapshot, _) = fixture(&store, item, route);
        let fields = fields(&snapshot);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].text, "readable");
        assert!(!fields[0].editable);
        let mut document = snapshot.document.clone();
        for block in &mut document.blocks {
            if let ContextBlock::Native { texts, .. } = block {
                for field in texts {
                    if matches!(field.selector, ContextTextSelector::ReasoningText { .. }) {
                        field.text = "forged revision".into();
                        field.editable = true;
                    }
                }
            }
        }
        assert!(
            store
                .commit_context(ContextCommit {
                    snapshot: &snapshot,
                    draft: &ContextDraft {
                        document,
                        next_model: None,
                        next_effort: None
                    },
                    output: &JobOutput::Completed(Ok(json!("done"))),
                    pending: &[]
                })
                .is_err()
        );
    }
}

proptest::proptest! {
    #[test]
    fn reasoning_overlay_parts_have_independent_canonical_oracle(parts in proptest::collection::vec("[a-zλ]{0,20}", 1..5), selected in 0usize..5, replacement in "[a-zΩ]{0,20}") {
        let store = Store::memory().unwrap();
        let selected = selected % parts.len();
        let original = Item(json!({"type":"reasoning","content":parts.iter().map(|text| json!({"type":"reasoning_text","text":text})).collect::<Vec<_>>()}));
        let (snapshot, original) = fixture(&store, original, ResponsesRoute::DeepSeek);
        let fields = fields(&snapshot);
        let receipt = rewrite(&store, &snapshot, &fields[selected], &replacement);
        let projected = store.context_request_state(&receipt.head, &snapshot.operation.origin).unwrap();
        let actual = &projected.history.iter().find(|(_, _, item)| item.0["type"] == "reasoning").unwrap().2;
        let mut expected = original.clone(); expected.0["content"][selected]["text"] = json!(replacement);
        proptest::prop_assert_eq!(actual, &expected);
        for (part, text) in parts.iter().enumerate() {
            proptest::prop_assert_eq!(store.original_context_text(&snapshot, &fields[part].reference, &fields[part].selector).unwrap().unwrap(), text.clone());
        }
    }
}

#[test]
fn reasoning_overlay_reopens_resets_and_refuses_a_different_model_contract() {
    let path =
        std::env::temp_dir().join(format!("reasoning-overlay-{}.sqlite", uuid::Uuid::new_v4()));
    let original = Item(
        json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"canonical"}]}),
    );
    let (head, identity) = {
        let store = Store::open(&path).unwrap();
        let (snapshot, _) = fixture(&store, original.clone(), ResponsesRoute::DeepSeek);
        let observed = fields(&snapshot);
        let receipt = rewrite(&store, &snapshot, &observed[0], "authored");
        assert!(
            store
                .preflight_context_target(
                    &receipt.head,
                    &snapshot.operation.origin,
                    &ContextProjectionTarget {
                        model: "different-model".into(),
                        route: ResponsesRoute::DeepSeek,
                        protocol: ResponsesProtocol::Standard,
                    }
                )
                .is_err()
        );
        (receipt.head, snapshot.operation.origin)
    };
    let store = Store::open(&path).unwrap();
    let projected = store.context_request_state(&head, &identity).unwrap();
    assert_eq!(
        projected
            .history
            .iter()
            .find(|(_, _, item)| item.0["type"] == "reasoning")
            .unwrap()
            .2
            .0["content"][0]["text"],
        "authored"
    );
    let next = RequestId("reset".into());
    store
        .write_request(&next, Some(&head), "/root", &[], Usage::default())
        .unwrap();
    record(&store, &next, vec![call("reset")], ResponsesRoute::DeepSeek);
    let operation = store.claim(&CallId("reset".into()), &next).unwrap();
    let snapshot = store.begin_context(&operation, &next).unwrap();
    let observed = fields(&snapshot);
    assert_eq!(observed[0].text, "authored");
    assert_eq!(
        store
            .original_context_text(&snapshot, &observed[0].reference, &observed[0].selector)
            .unwrap()
            .unwrap(),
        "canonical"
    );
    let receipt = rewrite(&store, &snapshot, &observed[0], "canonical");
    let projected = store
        .context_request_state(&receipt.head, &identity)
        .unwrap();
    assert_eq!(
        projected
            .history
            .iter()
            .find(|(_, _, item)| item.0["type"] == "reasoning")
            .unwrap()
            .2,
        original
    );
    let occurrences = store.history_occurrences(&receipt.head).unwrap();
    assert!(
        occurrences
            .iter()
            .filter(|item| item.item.0["type"] == "reasoning")
            .all(|item| item.overlays.is_empty())
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn interrupted_plaintext_reasoning_is_readable_without_edit_authority() {
    use crate::store::provider_response::ProviderResponseState;
    use crate::transport::sse::{ResponseAssembly, StreamEvent};
    let store = Store::memory().unwrap();
    let previous = RequestId("interrupted".into());
    store.create_request(&previous, None, "/root").unwrap();
    let request = ResponsesRequest {
        input: vec![],
        instructions: "fixture".into(),
        tools: vec![].into(),
        tools_allowed: None,
        model: "fixture-model".into(),
        pinned_effort: Effort::High,
        session_id: "fixture".into(),
        prompt_cache_comparison: None,
    };
    let issued = store
        .issue_provider_request(
            &previous,
            store.seal_replay_request(&request).unwrap(),
            (ResponsesRoute::DeepSeek, ResponsesProtocol::Standard),
        )
        .unwrap();
    let Some(StreamEvent::CompletedItem(done)) = ResponseAssembly::default().accept(&json!({"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"partial response"}]}}).to_string()).unwrap() else { panic!("completed item") };
    store.append_provider_done(&issued, &done).unwrap();
    store
        .stop_provider_response(&previous, ProviderResponseState::Interrupted)
        .unwrap();
    let head = RequestId("editor".into());
    store
        .write_request(&head, Some(&previous), "/root", &[], Usage::default())
        .unwrap();
    record(&store, &head, vec![call("edit")], ResponsesRoute::DeepSeek);
    let operation = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_target(
            &operation.origin,
            &ContextProjectionTarget {
                model: "fixture-model".into(),
                route: ResponsesRoute::DeepSeek,
                protocol: ResponsesProtocol::Standard,
            },
        )
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let observed = fields(&snapshot);
    assert_eq!(observed[0].text, "partial response");
    assert!(!observed[0].editable);
}

#[test]
fn effective_selection_reads_current_owner_head_and_final_positional_pin() {
    let store = Store::memory().unwrap();
    let actor = AgentPath("/root".into());
    let identity = store.standalone_identity(actor.clone());
    assert!(
        store
            .effective_inference_selection(&identity, "default", Effort::High)
            .is_err()
    );
    store
        .admit_agent(&actor, None, None, &json!({}), &json!({}))
        .unwrap();
    let empty = store
        .effective_inference_selection(&identity, "default", Effort::High)
        .unwrap();
    assert_eq!(
        (empty.head, empty.model, empty.effort),
        (None, "default".into(), Effort::High)
    );
    let old = RequestId("old-selection".into());
    store
        .write_request(
            &old,
            None,
            "/root",
            &[Item::configuration_update(Effort::Low)],
            Usage::default(),
        )
        .unwrap();
    store.advance_agent_head(&actor, None, Some(&old)).unwrap();
    store
        .initialize_context_target(
            &identity,
            &ContextProjectionTarget {
                model: "selected".into(),
                route: ResponsesRoute::DeepSeek,
                protocol: ResponsesProtocol::Standard,
            },
        )
        .unwrap();
    let current = RequestId("current-selection".into());
    store
        .write_request(
            &current,
            Some(&old),
            "/root",
            &[
                Item::user_text("unrelated"),
                Item::configuration_update(Effort::High),
                Item::configuration_update(Effort::Max),
            ],
            Usage::default(),
        )
        .unwrap();
    store
        .advance_agent_head(&actor, Some(&old), Some(&current))
        .unwrap();
    let observed = store
        .effective_inference_selection(&identity, "stale-default", Effort::None)
        .unwrap();
    assert_eq!(
        (observed.head, observed.model, observed.effort),
        (Some(current), "selected".into(), Effort::Max)
    );
}
