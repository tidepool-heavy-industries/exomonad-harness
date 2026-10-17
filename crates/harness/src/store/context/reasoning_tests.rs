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
        json!({"type":"reasoning","id":"r","status":"completed","content":[{"type":"reasoning_text","text":"first λ"},{"type":"reasoning_text","text":"second"}]}),
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
