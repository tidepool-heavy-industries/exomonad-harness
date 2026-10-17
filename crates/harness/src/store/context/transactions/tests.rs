use super::*;
use crate::{context::ContextTransactionReceipt, item::ToolKind, model::CallId, store::Usage};

fn invocation(name: &str) -> Item {
    Item(json!({"type":"custom_tool_call","call_id":name,"name":"cell","input":"transaction"}))
}
fn output() -> JobOutput {
    JobOutput::Completed(Ok(json!({"authored":"original output"})))
}
fn fixture(store: &Store) -> (RequestId, ContextSnapshot, ContextSnapshot) {
    let head = RequestId("root".into());
    store
        .write_request(
            &head,
            None,
            "/root",
            &[
                Item::user_text("alpha"),
                Item::user_text("beta"),
                invocation("first"),
                invocation("second"),
            ],
            Usage::default(),
        )
        .unwrap();
    let a = store.claim(&CallId("first".into()), &head).unwrap();
    let b = store.claim(&CallId("second".into()), &head).unwrap();
    store
        .initialize_context_target(
            &a.origin,
            &ContextProjectionTarget {
                model: "model-a".into(),
                route: crate::transport::ResponsesRoute::ChatGptPlan,
                protocol: crate::transport::ResponsesProtocol::Standard,
                native_transport: None,
            },
        )
        .unwrap();
    (
        head.clone(),
        store.begin_context(&a, &head).unwrap(),
        store.begin_context(&b, &head).unwrap(),
    )
}
fn replacement(snapshot: &ContextSnapshot, old: &str, new: &str) -> ContextChange {
    let inspection = inspect_context(&snapshot.document);
    let reference = snapshot
        .document
        .blocks
        .iter()
        .find_map(|block| match block {
            ContextBlock::Text {
                reference: Some(reference),
                text,
                ..
            } if text == old => Some(reference),
            ContextBlock::Native { texts, .. } => texts
                .iter()
                .find(|field| field.text == old)
                .map(|field| &field.reference),
            _ => None,
        })
        .expect("target text");
    let version = inspection
        .text_versions
        .iter()
        .find(|version| &version.reference == reference)
        .unwrap();
    ContextChange::ReplaceContextText {
        reference: reference.clone(),
        selector: version.selector.clone(),
        expected: version.digest.clone(),
        text: new.into(),
    }
}
fn draft(changes: Vec<ContextChange>) -> ContextTransactionDraft {
    ContextTransactionDraft {
        patch: ContextPatch { changes },
        next_model: None,
        next_effort: None,
    }
}
fn committed(status: ContextTransactionStatus) -> ContextTransactionReceipt {
    match status {
        ContextTransactionStatus::Committed(receipt) => receipt,
        other => panic!("expected commit: {other:?}"),
    }
}
fn visible(store: &Store, head: &RequestId, identity: &ConversationIdentity) -> Vec<String> {
    store
        .context_request_state(head, identity)
        .unwrap()
        .history
        .iter()
        .filter_map(|(_, _, item)| message(item).map(|(_, text)| text))
        .collect()
}
fn next_editor(store: &Store, head: &RequestId, name: &str) -> ContextSnapshot {
    let next = RequestId(name.into());
    store
        .write_request(
            &next,
            Some(head),
            "/root",
            &[invocation(name)],
            Usage::default(),
        )
        .unwrap();
    let operation = store.claim(&CallId(name.into()), &next).unwrap();
    store.begin_context(&operation, &next).unwrap()
}

#[test]
fn targeted_batch_rejects_stale_all_or_nothing_and_retains_authored_output() {
    let store = Store::memory().unwrap();
    let (_, first, second) = fixture(&store);
    let mut stale = draft(vec![
        replacement(&first, "beta", "must not appear"),
        replacement(&first, "alpha", "first edit"),
    ]);
    stale.next_model = Some("model-b".into());
    stale.next_effort = Some(crate::model::Effort::High);
    let intervening = draft(vec![replacement(&second, "alpha", "current alpha")]);
    let head = committed(
        store
            .commit_context_transaction_guarded(
                &second,
                &intervening,
                &output(),
                std::slice::from_ref(&first.operation),
                || false,
            )
            .unwrap(),
    )
    .head;
    store.set_effort(&head, crate::model::Effort::Low).unwrap();
    store
        .save_pending_effort(first.operation.origin.actor(), crate::model::Effort::Medium)
        .unwrap();
    let handle = store.context_transaction(&first, &stale.patch).unwrap();
    let status = store
        .commit_context_transaction_at_guarded(&first, &head, &stale, &output(), &[], || false)
        .unwrap();
    assert!(
        matches!(&status, ContextTransactionStatus::Rejected(problems) if problems.iter().any(|p| p.code == ContextProblemCode::Stale))
    );
    assert_eq!(
        store
            .context_transaction_status(&first.operation.origin, &handle)
            .unwrap(),
        status
    );
    let text = visible(&store, &head, &first.operation.origin);
    assert!(text.contains(&"current alpha".into()));
    assert!(text.contains(&"beta".into()));
    assert!(!text.contains(&"must not appear".into()));
    assert!(store.context_receipt(&first.operation).unwrap().is_none());
    assert_eq!(
        store
            .context_model(&first.operation.origin)
            .unwrap()
            .as_deref(),
        Some("model-a")
    );
    let efforts = store
        .context_history(&head)
        .unwrap()
        .iter()
        .filter_map(|(_, _, item)| item.configuration_effort())
        .collect::<Vec<_>>();
    assert_eq!(efforts, vec![crate::model::Effort::Low]);
    let next = RequestId("after-rejection".into());
    store
        .write_request(&next, Some(&head), "/root", &[], Usage::default())
        .unwrap();
    assert!(
        store
            .apply_pending_effort(first.operation.origin.actor(), &next)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store
            .items(&next)
            .unwrap()
            .last()
            .unwrap()
            .configuration_effort(),
        Some(crate::model::Effort::Medium)
    );
    let terminal = store
        .replay_tool_output_operation(&first.operation)
        .unwrap()
        .unwrap();
    assert_eq!(
        terminal.item,
        Item::tool_output(&first.operation.call, ToolKind::Custom, &output())
    );
    assert_eq!(terminal.terminal, TerminalOutcome::Success);
}

#[test]
fn targeted_commit_preserves_unrelated_edit_and_appended_suffix() {
    let store = Store::memory().unwrap();
    let (_, first, second) = fixture(&store);
    let original = first
        .prefix
        .iter()
        .map(|item| item.item.clone())
        .collect::<Vec<_>>();
    let intervening = draft(vec![replacement(&second, "beta", "current beta")]);
    let head = committed(
        store
            .commit_context_transaction_guarded(
                &second,
                &intervening,
                &output(),
                std::slice::from_ref(&first.operation),
                || false,
            )
            .unwrap(),
    )
    .head;
    store
        .append_items(&head, &[Item::user_text("late suffix")])
        .unwrap();
    let edit = draft(vec![replacement(&first, "alpha", "new alpha")]);
    let receipt = committed(
        store
            .commit_context_transaction_at_guarded(&first, &head, &edit, &output(), &[], || false)
            .unwrap(),
    );
    let text = visible(&store, &receipt.head, &first.operation.origin);
    for expected in ["new alpha", "current beta", "late suffix"] {
        assert!(
            text.iter().any(|text| text == expected),
            "{expected}: {text:?}"
        );
    }
    assert_eq!(
        first
            .prefix
            .iter()
            .map(|item| item.item.clone())
            .collect::<Vec<_>>(),
        original
    );
    assert_eq!(store.items(&first.head).unwrap()[0], original[0]);
}

#[test]
fn content_version_accepts_aba_across_generations() {
    let store = Store::memory().unwrap();
    let (_, first, second) = fixture(&store);
    let original = draft(vec![replacement(&first, "alpha", "accepted after ABA")]);
    let head = committed(
        store
            .commit_context_transaction_guarded(
                &second,
                &draft(vec![replacement(&second, "alpha", "temporary")]),
                &output(),
                std::slice::from_ref(&first.operation),
                || false,
            )
            .unwrap(),
    )
    .head;
    let restore = next_editor(&store, &head, "restore");
    let head = committed(
        store
            .commit_context_transaction_guarded(
                &restore,
                &draft(vec![replacement(&restore, "temporary", "alpha")]),
                &output(),
                std::slice::from_ref(&first.operation),
                || false,
            )
            .unwrap(),
    )
    .head;
    let receipt = committed(
        store
            .commit_context_transaction_at_guarded(&first, &head, &original, &output(), &[], || {
                false
            })
            .unwrap(),
    );
    assert!(receipt.generation > first.generation + 1);
    assert!(
        visible(&store, &receipt.head, &first.operation.origin)
            .contains(&"accepted after ABA".into())
    );
}

#[test]
fn exact_candidate_retry_survives_reopen_and_refuses_changed_output_or_patch() {
    let path = std::env::temp_dir().join(format!(
        "context-transactions-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let store = Store::open(&path).unwrap();
    let (_, first, _) = fixture(&store);
    let edit = draft(vec![replacement(&first, "alpha", "new alpha")]);
    let handle = store.context_transaction(&first, &edit.patch).unwrap();
    let status = store
        .commit_context_transaction_guarded(&first, &edit, &output(), &[], || false)
        .unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .context_transaction_status(&first.operation.origin, &handle)
            .unwrap(),
        status
    );
    assert_eq!(
        store
            .commit_context_transaction_guarded(&first, &edit, &output(), &[], || true)
            .unwrap(),
        status
    );
    assert!(matches!(
        store.commit_context_transaction_guarded(&first, &draft(vec![]), &output(), &[], || false),
        Err(StoreError::ConflictingReplayOutcome { .. })
    ));
    assert!(matches!(
        store.commit_context_transaction_guarded(
            &first,
            &edit,
            &JobOutput::Completed(Ok(json!("different"))),
            &[],
            || false
        ),
        Err(StoreError::ConflictingReplayOutcome { .. })
    ));
    assert_eq!(
        store
            .events(None)
            .unwrap()
            .iter()
            .filter(|event| event.kind == "context_commit")
            .count(),
        1
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn cancellation_guard_rolls_back_while_confirmed_cancel_is_terminal() {
    let store = Store::memory().unwrap();
    let (_, first, _) = fixture(&store);
    let edit = draft(vec![replacement(&first, "alpha", "must not appear")]);
    let handle = store.context_transaction(&first, &edit.patch).unwrap();
    assert!(matches!(
        store.commit_context_transaction_guarded(&first, &edit, &output(), &[], || true),
        Err(StoreError::Context(ContextError::Cancelled))
    ));
    assert!(matches!(
        store
            .context_transaction_status(&first.operation.origin, &handle)
            .unwrap(),
        ContextTransactionStatus::Unknown(_)
    ));
    let status = store
        .discard_context_transaction(&first, &edit, &JobOutput::Cancelled)
        .unwrap();
    assert_eq!(status, ContextTransactionStatus::Cancelled);
    assert_eq!(
        store
            .context_transaction_status(&first.operation.origin, &handle)
            .unwrap(),
        status
    );
    assert!(visible(&store, &first.head, &first.operation.origin).contains(&"alpha".into()));
}

#[test]
fn originals_remain_canonical_after_edit_removal_and_later_invocation() {
    let store = Store::memory().unwrap();
    let (_, first, _) = fixture(&store);
    let target = inspect_context(&first.document).text_versions[0].clone();
    let unit = inspect_context(&first.document)
        .unit_versions
        .iter()
        .find(|unit| unit.reference == target.reference)
        .unwrap()
        .clone();
    let original = store
        .original_context_text(&first, &target.reference, &target.selector)
        .unwrap()
        .unwrap();
    assert_eq!(original, "alpha");
    let head = committed(
        store
            .commit_context_transaction_guarded(
                &first,
                &draft(vec![ContextChange::RemoveContextUnit {
                    reference: unit.reference,
                    expected: unit.digest,
                }]),
                &output(),
                &[],
                || false,
            )
            .unwrap(),
    )
    .head;
    let later = next_editor(&store, &head, "later");
    assert!(!context_has_reference(&later.document, &target.reference));
    assert_eq!(
        store
            .original_context_text(&later, &target.reference, &target.selector)
            .unwrap()
            .unwrap(),
        original
    );
    assert_eq!(
        store
            .original_context_text(
                &later,
                &target.reference,
                &ContextTextSelector::ToolResultText
            )
            .unwrap()
            .unwrap_err()
            .code,
        ContextProblemCode::UnsupportedSelector
    );
    let restore = draft(vec![ContextChange::ReplaceContextText {
        reference: target.reference,
        selector: target.selector,
        expected: target.digest,
        text: "forbidden resurrection".into(),
    }]);
    assert!(
        matches!(store.commit_context_transaction_guarded(&later, &restore, &output(), &[], || false).unwrap(), ContextTransactionStatus::Rejected(problems) if problems.iter().any(|problem| problem.code == ContextProblemCode::Denied))
    );
}

#[test]
fn appended_notes_have_assistant_role_sources_and_exact_receipt_reference() {
    let store = Store::memory().unwrap();
    let (_, first, _) = fixture(&store);
    let source = inspect_context(&first.document).text_versions[0]
        .reference
        .clone();
    let receipt = committed(
        store
            .commit_context_transaction_guarded(
                &first,
                &draft(vec![ContextChange::AppendContextNote {
                    text: "authored note".into(),
                    sources: vec![source.clone()],
                }]),
                &output(),
                &[],
                || false,
            )
            .unwrap(),
    );
    assert_eq!(receipt.appended.len(), 1);
    let all = store.history_occurrences(&receipt.head).unwrap();
    let note = all
        .iter()
        .find(|item| super::super::reference(std::slice::from_ref(*item)) == receipt.appended[0])
        .unwrap();
    assert_eq!(note.item.0["role"], "assistant");
    assert_eq!(note.sources, vec![source]);
    assert!(note.note);
}

#[test]
fn discarded_candidate_uses_current_cursor_and_leaves_prior_rewrite_intact() {
    let store = Store::memory().unwrap();
    let (_, first, second) = fixture(&store);
    let head = committed(
        store
            .commit_context_transaction_guarded(
                &second,
                &draft(vec![replacement(&second, "beta", "retained rewrite")]),
                &output(),
                std::slice::from_ref(&first.operation),
                || false,
            )
            .unwrap(),
    )
    .head;
    let status = store
        .discard_context_transaction_at(&first, &head, &draft(vec![]), &output())
        .unwrap();
    assert!(matches!(status, ContextTransactionStatus::Rejected(_)));
    assert_eq!(
        store.context_transaction_head(&first.operation).unwrap(),
        Some(head.clone())
    );
    assert!(visible(&store, &head, &first.operation.origin).contains(&"retained rewrite".into()));
}

#[test]
fn invalid_digest_overlap_and_readonly_targets_are_typed_refusals() {
    let store = Store::memory().unwrap();
    let (_, first, _) = fixture(&store);
    let target = inspect_context(&first.document).text_versions[0].clone();
    let mut change = replacement(&first, "alpha", "edit");
    if let ContextChange::ReplaceContextText { expected, .. } = &mut change {
        *expected = "not a version".into();
    }
    assert_eq!(
        check_context_patch(
            &first.document,
            &ContextPatch {
                changes: vec![change]
            }
        )
        .unwrap_err()[0]
            .code,
        ContextProblemCode::InvalidPatch
    );
    let changes = vec![
        replacement(&first, "alpha", "edit"),
        ContextChange::RemoveContextUnit {
            reference: target.reference,
            expected: target.digest,
        },
    ];
    assert!(
        check_context_patch(&first.document, &ContextPatch { changes })
            .unwrap_err()
            .iter()
            .any(|problem| problem.code == ContextProblemCode::InvalidPatch)
    );
}

#[test]
fn targeted_note_histories_preserve_projection_originals_and_retry_outcomes() {
    use proptest::prelude::*;
    let strategy = proptest::collection::vec((0u8..7, 0usize..5, 0u8..4), 1..16);
    proptest::test_runner::TestRunner::new(proptest::test_runner::Config {
        cases: 48,
        ..Default::default()
    })
    .run(&strategy, |steps| {
        let path = std::env::temp_dir().join(format!(
            "transaction-history-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let mut store = Store::open(&path).unwrap();
        let (mut head, first, second) = fixture(&store);
        let pending = [first.operation, second.operation];
        let mut model = Vec::<(ContextReference, String)>::new();
        for (step, (kind, slot, body)) in steps.into_iter().enumerate() {
            if kind == 5 {
                drop(store);
                store = Store::open(&path).unwrap();
            }
            let snapshot = next_editor(&store, &head, &format!("step-{step}"));
            head = snapshot.head.clone();
            let note = (!model.is_empty()).then(|| model[slot % model.len()].clone());
            let text = format!("note-{body}");
            let mut changes = Vec::new();
            let mut next_model = model.clone();
            match (kind, note.as_ref()) {
                (0 | 5, _) => changes.push(ContextChange::AppendContextNote {
                    text: text.clone(),
                    sources: vec![],
                }),
                (1, Some((reference, old))) => {
                    changes.push(ContextChange::ReplaceContextText {
                        reference: reference.clone(),
                        selector: ContextTextSelector::MessageText { part: 0 },
                        expected: context_digest(old.as_bytes()),
                        text: text.clone(),
                    });
                    next_model
                        .iter_mut()
                        .find(|(r, _)| r == reference)
                        .unwrap()
                        .1 = text.clone();
                }
                (2, Some((reference, old))) => {
                    changes.push(ContextChange::RemoveContextUnit {
                        reference: reference.clone(),
                        expected: context_digest(old.as_bytes()),
                    });
                    next_model.retain(|(r, _)| r != reference);
                }
                (3, Some((reference, _))) => changes.push(ContextChange::ReplaceContextText {
                    reference: reference.clone(),
                    selector: ContextTextSelector::MessageText { part: 0 },
                    expected: context_digest(b"deliberately stale"),
                    text: "must not appear".into(),
                }),
                _ => {}
            }
            let draft = draft(changes);
            let handle = store.context_transaction(&snapshot, &draft.patch).unwrap();
            let status = if kind == 4 {
                prop_assert!(matches!(
                    store.commit_context_transaction_guarded(
                        &snapshot,
                        &draft,
                        &output(),
                        &pending,
                        || true
                    ),
                    Err(StoreError::Context(ContextError::Cancelled))
                ));
                store
                    .discard_context_transaction(&snapshot, &draft, &JobOutput::Cancelled)
                    .unwrap()
            } else {
                store
                    .commit_context_transaction_guarded(
                        &snapshot,
                        &draft,
                        &output(),
                        &pending,
                        || false,
                    )
                    .unwrap()
            };
            match &status {
                ContextTransactionStatus::Committed(receipt) => {
                    head = receipt.head.clone();
                    if matches!(kind, 0 | 5) {
                        prop_assert_eq!(receipt.appended.len(), 1);
                        next_model.push((receipt.appended[0].clone(), text));
                    }
                    model = next_model;
                    prop_assert_eq!(
                        store
                            .commit_context_transaction_guarded(
                                &snapshot,
                                &draft,
                                &output(),
                                &pending,
                                || true
                            )
                            .unwrap(),
                        status.clone()
                    );
                }
                ContextTransactionStatus::Rejected(problems) => {
                    prop_assert_eq!(kind, 3);
                    prop_assert!(
                        problems
                            .iter()
                            .any(|problem| problem.code == ContextProblemCode::Stale)
                    );
                }
                ContextTransactionStatus::Cancelled => prop_assert_eq!(kind, 4),
                other => {
                    return Err(proptest::test_runner::TestCaseError::fail(format!(
                        "unexpected terminal: {other:?}"
                    )));
                }
            }
            prop_assert_eq!(
                store
                    .context_transaction_status(&snapshot.operation.origin, &handle)
                    .unwrap(),
                status
            );
            let history = store.history_occurrences(&head).unwrap();
            let observed = history
                .iter()
                .filter(|item| item.note)
                .map(|item| {
                    (
                        super::super::reference(std::slice::from_ref(item)),
                        message(&project_bodies(item).unwrap()).unwrap().1,
                    )
                })
                .collect::<Vec<_>>();
            prop_assert_eq!(&observed, &model);
            let projected = store
                .context_request_state(&head, &snapshot.operation.origin)
                .unwrap();
            for (reference, expected) in &model {
                let canonical = history
                    .iter()
                    .find(|item| super::super::reference(std::slice::from_ref(*item)) == *reference)
                    .unwrap();
                prop_assert!(
                    projected
                        .history
                        .iter()
                        .any(|(_, _, item)| item.0["role"] == "assistant"
                            && message(item).is_some_and(|(_, text)| text.ends_with(expected)))
                );
                prop_assert_eq!(
                    store
                        .original_context_text(
                            &snapshot,
                            reference,
                            &ContextTextSelector::MessageText { part: 0 }
                        )
                        .unwrap()
                        .ok()
                        .or_else(|| {
                            // A note appended by this invocation was not yet in its cut.
                            canonical
                                .origin
                                .request
                                .eq(&head)
                                .then(|| message(&canonical.item).unwrap().1)
                        }),
                    Some(message(&canonical.item).unwrap().1)
                );
            }
        }
        drop(store);
        std::fs::remove_file(path).unwrap();
        Ok(())
    })
    .unwrap();
}

#[test]
fn shared_reference_multipart_edits_keep_selectors_and_canonical_native_bytes() {
    use crate::model::Effort;
    let store = Store::memory().unwrap();
    let previous = RequestId("multipart".into());
    store
        .write_request(&previous, None, "/root", &[], Usage::default())
        .unwrap();
    let native = vec![
        Item(json!({"type":"reasoning","encrypted_content":"retained opaque","summary":[]})),
        Item(
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"first part","annotations":[]},{"type":"output_text","text":"second part","annotations":[]}],"future":{"preserve":true}}),
        ),
    ];
    store
        .record_replay_turn_for_target(
            &previous,
            &crate::transport::ResponsesRequest {
                input: vec![],
                instructions: "multipart".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "model-a".into(),
                pinned_effort: Effort::Low,
                session_id: "multipart".into(),
                prompt_cache_comparison: None,
            },
            &crate::transport::ResponsesTurn {
                response_id: "multipart".into(),
                items: native.clone(),
                usage: Default::default(),
            },
            crate::transport::ResponsesRoute::ChatGptPlan,
            crate::transport::ResponsesProtocol::Standard,
        )
        .unwrap();
    let snapshot = next_editor(&store, &previous, "multipart-editor");
    store
        .initialize_context_target(
            &snapshot.operation.origin,
            &ContextProjectionTarget {
                model: "model-a".into(),
                route: crate::transport::ResponsesRoute::ChatGptPlan,
                protocol: crate::transport::ResponsesProtocol::Standard,
                native_transport: None,
            },
        )
        .unwrap();
    let inspection = inspect_context(&snapshot.document);
    let versions = inspection
        .text_versions
        .iter()
        .filter(|version| matches!(version.selector, ContextTextSelector::MessageText { .. }))
        .collect::<Vec<_>>();
    assert_eq!(versions.len(), 2);
    assert_eq!(versions[0].reference, versions[1].reference);
    assert_ne!(versions[0].selector, versions[1].selector);
    let changes = versions
        .iter()
        .enumerate()
        .map(|(index, version)| ContextChange::ReplaceContextText {
            reference: version.reference.clone(),
            selector: version.selector.clone(),
            expected: version.digest.clone(),
            text: format!("part-{index}"),
        })
        .collect();
    let receipt = committed(
        store
            .commit_context_transaction_guarded(&snapshot, &draft(changes), &output(), &[], || {
                false
            })
            .unwrap(),
    );
    let projection = store
        .context_request_state(&receipt.head, &snapshot.operation.origin)
        .unwrap();
    assert!(
        projection
            .history
            .iter()
            .any(|(_, _, item)| item == &native[0])
    );
    let message = projection
        .history
        .iter()
        .find(|(_, _, item)| item.0["future"]["preserve"] == true)
        .unwrap();
    assert_eq!(message.2.0["content"][0]["text"], "part-0");
    assert_eq!(message.2.0["content"][1]["text"], "part-1");
    assert_eq!(store.items(&previous).unwrap(), native);
    assert_eq!(
        store
            .original_context_text(&snapshot, &versions[0].reference, &versions[0].selector)
            .unwrap()
            .unwrap(),
        "first part"
    );
    let later = next_editor(&store, &receipt.head, "protected-editor");
    let protected = later
        .document
        .blocks
        .iter()
        .find_map(|block| match block {
            ContextBlock::Native {
                reference,
                kind: ContextNativeKind::Opaque,
                ..
            } => Some(reference.clone()),
            _ => None,
        })
        .unwrap();
    let status = store
        .commit_context_transaction_guarded(
            &later,
            &draft(vec![ContextChange::RemoveContextUnit {
                reference: protected,
                expected: context_digest(b"not removal authority"),
            }]),
            &output(),
            &[],
            || false,
        )
        .unwrap();
    assert!(
        matches!(status, ContextTransactionStatus::Rejected(problems) if problems.iter().any(|problem| problem.code == ContextProblemCode::Denied))
    );
}

#[test]
fn sqlite_publication_failure_rolls_back_context_terminal_and_outcome() {
    let store = Store::memory().unwrap();
    let (head, first, _) = fixture(&store);
    let before = store.items(&head).unwrap();
    let edit = draft(vec![replacement(&first, "alpha", "unpublished")]);
    let handle = store.context_transaction(&first, &edit.patch).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER refuse_transaction BEFORE INSERT ON events WHEN NEW.kind='context_commit' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
    let error = store
        .commit_context_transaction_guarded(&first, &edit, &output(), &[], || false)
        .unwrap_err();
    assert!(!matches!(error, StoreError::Context(_)));
    assert_eq!(store.items(&head).unwrap(), before);
    assert!(
        store
            .replay_tool_output_operation(&first.operation)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        store
            .context_transaction_status(&first.operation.origin, &handle)
            .unwrap(),
        ContextTransactionStatus::Unknown(_)
    ));
    assert!(store.context_receipt(&first.operation).unwrap().is_none());
}

#[test]
fn pure_preview_refuses_nested_unit_removal_in_either_batch_order() {
    let group = ContextReference::from_raw("group".into());
    let child = ContextReference::from_raw("child".into());
    let document = ContextDocument {
        blocks: vec![ContextBlock::Native {
            reference: group.clone(),
            kind: ContextNativeKind::CompletedExchange,
            preview: "exchange".into(),
            protected: false,
            texts: vec![ContextVisibleText {
                reference: child.clone(),
                selector: ContextTextSelector::MessageText { part: 0 },
                text: "note".into(),
                editable: true,
                removable: true,
            }],
        }],
    };
    let versions = inspect_context(&document).unit_versions;
    for reverse in [false, true] {
        let mut changes = versions
            .iter()
            .map(|version| ContextChange::RemoveContextUnit {
                reference: version.reference.clone(),
                expected: version.digest.clone(),
            })
            .collect::<Vec<_>>();
        if reverse {
            changes.reverse();
        }
        assert!(
            check_context_patch(&document, &ContextPatch { changes })
                .unwrap_err()
                .iter()
                .any(|problem| problem.code == ContextProblemCode::InvalidPatch)
        );
    }
}

#[test]
fn patch_only_publication_preserves_selected_model_effort_and_pending_choice() {
    use crate::model::Effort;
    let store = Store::memory().unwrap();
    let (head, first, _) = fixture(&store);
    store.set_effort(&head, Effort::High).unwrap();
    store
        .save_pending_effort(first.operation.origin.actor(), Effort::Medium)
        .unwrap();
    let receipt = committed(
        store
            .commit_context_transaction_guarded(
                &first,
                &draft(vec![replacement(&first, "alpha", "edited")]),
                &output(),
                &[],
                || false,
            )
            .unwrap(),
    );
    assert_eq!(
        store
            .context_model(&first.operation.origin)
            .unwrap()
            .as_deref(),
        Some("model-a")
    );
    let history = store.context_history(&receipt.head).unwrap();
    assert_eq!(
        history
            .iter()
            .filter_map(|(_, _, item)| item.configuration_effort())
            .collect::<Vec<_>>(),
        vec![Effort::High]
    );
    let next = RequestId("after-patch-only".into());
    store
        .write_request(&next, Some(&receipt.head), "/root", &[], Usage::default())
        .unwrap();
    assert!(
        store
            .apply_pending_effort(first.operation.origin.actor(), &next)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store
            .items(&next)
            .unwrap()
            .last()
            .unwrap()
            .configuration_effort(),
        Some(Effort::Medium)
    );
}

#[test]
fn choice_only_legacy_commit_does_not_require_or_issue_a_patch_transaction() {
    use crate::model::Effort;
    let store = Store::memory().unwrap();
    let head = RequestId("choice-only".into());
    store
        .write_request(
            &head,
            None,
            "/root",
            &[Item::user_text("alpha"), invocation("choice-only")],
            Usage::default(),
        )
        .unwrap();
    let operation = store.claim(&CallId("choice-only".into()), &head).unwrap();
    store
        .initialize_context_target(
            &operation.origin,
            &ContextProjectionTarget {
                model: "model-a".into(),
                route: crate::transport::ResponsesRoute::ChatGptPlan,
                protocol: crate::transport::ResponsesProtocol::Standard,
                native_transport: None,
            },
        )
        .unwrap();
    let first = store.begin_context(&operation, &head).unwrap();
    let draft = ContextDraft {
        document: first.document.clone(),
        next_model: Some("selected-model".into()),
        next_effort: Some(Effort::High),
    };
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &first,
            draft: &draft,
            output: &output(),
            pending: &[],
        })
        .unwrap();
    assert_eq!(receipt.head, head);
    assert!(receipt.changed);
    assert_eq!(receipt.model.as_deref(), Some("selected-model"));
    assert!(
        store
            .context_transaction_head(&first.operation)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.context_receipt(&first.operation).unwrap(),
        Some(receipt)
    );
    let history = store.context_history(&head).unwrap();
    assert_eq!(
        history.last().unwrap().2.configuration_effort(),
        Some(Effort::High)
    );
    assert_eq!(
        history[history.len() - 2].2,
        Item::tool_output(&first.operation.call, ToolKind::Custom, &output())
    );
    assert!(visible(&store, &head, &first.operation.origin).contains(&"alpha".into()));
}

#[test]
fn inspection_authorship_deduplicates_identical_origins_and_refuses_conflicting_copies() {
    let store = Store::memory().unwrap();
    let (_, snapshot, _) = fixture(&store);
    let original = store.inspect_context(&snapshot).unwrap();
    let mut repeated = snapshot.clone();
    repeated.prefix.push(snapshot.prefix[0].clone());
    repeated.blocks.push(snapshot.blocks[0].clone());
    assert_eq!(
        store.inspect_context(&repeated).unwrap().authorship,
        original.authorship
    );
    let mut readonly = snapshot.clone();
    let reference = super::super::reference(&snapshot.prefix[..1]);
    let text = message(&snapshot.prefix[0].item).unwrap().1;
    readonly.blocks[0].block = ContextBlock::Native {
        reference: reference.clone(),
        kind: ContextNativeKind::Pending,
        preview: "protected visible text".into(),
        protected: true,
        texts: vec![ContextVisibleText {
            reference,
            selector: ContextTextSelector::MessageText { part: 0 },
            text,
            editable: false,
            removable: false,
        }],
    };
    let inspected = store.inspect_context(&readonly).unwrap();
    assert_eq!(inspected.authorship, original.authorship);
    assert_eq!(
        inspected.text_versions.len() + 1,
        original.text_versions.len()
    );
    for conflict in [false, true] {
        let mut conflicting = repeated.clone();
        let duplicate = conflicting.prefix.last_mut().unwrap();
        if conflict {
            duplicate.item.0["role"] = json!("assistant");
        } else {
            duplicate.note = !duplicate.note;
        }
        assert!(matches!(
            store.inspect_context(&conflicting),
            Err(StoreError::Context(ContextError::UnsupportedState))
        ));
    }
}

#[test]
fn canonical_authorship_reads_raw_owner_role_and_type_without_selector_inference() {
    for (role, expected) in [
        ("user", ContextTextAuthor::User),
        ("assistant", ContextTextAuthor::Assistant),
        ("system", ContextTextAuthor::System),
        ("developer", ContextTextAuthor::Developer),
        ("unknown", ContextTextAuthor::Unknown),
    ] {
        assert_eq!(
            canonical_author(&Item(
                json!({"type":"message","role":role,"content":"visible"})
            )),
            expected
        );
    }
    for kind in ["function_call_output", "custom_tool_call_output"] {
        assert_eq!(
            canonical_author(&Item(
                json!({"type":kind,"role":"assistant","output":"visible"})
            )),
            ContextTextAuthor::Tool
        );
    }
    assert_eq!(
        canonical_author(&Item(json!({"type":"reasoning","role":"assistant"}))),
        ContextTextAuthor::Unknown
    );
}
