use super::*;
use crate::{
    context::{ContextDraft, ContextRole},
    model::{CallId, Effort},
    store::Usage,
};

fn setup(store: &Store) -> (RequestId, OperationId) {
    let head = RequestId("root".into());
    store.write_request(&head,None,"/root",&[
        Item(json!({"type":"message","role":"user","content":"old context","unknown":{"retain":true}})),
        Item(json!({"type":"custom_tool_call","call_id":"edit","name":"haskell_sync","input":"modifyContext"})),
    ],Usage::default()).unwrap();
    store.set_effort(&head, Effort::Low).unwrap();
    let operation = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_model(&operation.origin, "luna")
        .unwrap();
    (head, operation)
}

fn edited(snapshot: &ContextSnapshot) -> ContextDraft {
    let mut document = snapshot.document.clone();
    if let ContextBlock::Text { text, .. } = &mut document.blocks[0] {
        *text = "edited context".into();
    } else {
        panic!("plain text");
    }
    ContextDraft {
        document,
        next_model: Some("sol".into()),
    }
}

fn output() -> JobOutput {
    JobOutput::Completed(Ok(json!({"value":"done"})))
}

#[test]
fn commit_replaces_prefix_preserves_suffix_and_original_evidence() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let snapshot = store.begin_context(&operation, &head).unwrap();
    store
        .append_items(
            &head,
            &[Item(
                json!({"type":"message","role":"user","content":"late arrival"}),
            )],
        )
        .unwrap();
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[],
        })
        .unwrap();
    assert!(receipt.changed);
    assert_eq!(receipt.generation, 1);
    assert_ne!(receipt.head, head);
    let state = store
        .context_request_state(&receipt.head, &operation.origin)
        .unwrap();
    assert_eq!(state.model.as_deref(), Some("sol"));
    assert_eq!(state.history[0].2.0["content"], "edited context");
    assert_eq!(state.history[0].2.0["unknown"], json!({"retain":true}));
    assert_eq!(state.history[1].2.0["call_id"], "edit");
    assert!(
        state
            .history
            .iter()
            .any(|(_, _, i)| i.0["content"] == "late arrival")
    );
    assert_eq!(
        state.history.last().unwrap().2.0["type"],
        "custom_tool_call_output"
    );
    assert_eq!(store.items(&head).unwrap()[0].0["content"], "old context");
    assert_eq!(
        store
            .replay_tool_output_operation(&operation)
            .unwrap()
            .unwrap()
            .terminal,
        TerminalOutcome::Success
    );
}

#[test]
fn failed_publication_rolls_back_model_prefix_terminal_and_receipt() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let before = store.items(&head).unwrap();
    store.lock().execute_batch("CREATE TRIGGER reject_context BEFORE INSERT ON events WHEN NEW.kind='context_commit' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
    assert!(
        store
            .commit_context(ContextCommit {
                snapshot: &snapshot,
                draft: &edited(&snapshot),
                output: &output(),
                pending: &[]
            })
            .is_err()
    );
    assert_eq!(store.items(&head).unwrap(), before);
    assert_eq!(
        store.context_model(&operation.origin).unwrap().as_deref(),
        Some("luna")
    );
    assert!(store.context_receipt(&operation).unwrap().is_none());
    assert!(
        store
            .replay_tool_output_operation(&operation)
            .unwrap()
            .is_none()
    );
    assert_eq!(store.children_of(&head).unwrap().len(), 0);
}

#[test]
fn commit_and_lost_ack_survive_reopen_without_duplicate_output() {
    let path = std::env::temp_dir().join(format!("context-{}.sqlite", uuid::Uuid::new_v4()));
    let (snapshot, draft, receipt, operation) = {
        let store = Store::open(&path).unwrap();
        let (head, operation) = setup(&store);
        let snapshot = store.begin_context(&operation, &head).unwrap();
        let draft = edited(&snapshot);
        let receipt = store
            .commit_context(ContextCommit {
                snapshot: &snapshot,
                draft: &draft,
                output: &output(),
                pending: &[],
            })
            .unwrap();
        (snapshot, draft, receipt, operation)
    };
    let store = Store::open(&path).unwrap();
    let before = store.items(&receipt.head).unwrap();
    assert_eq!(
        store.context_receipt(&operation).unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(
        store
            .commit_context(ContextCommit {
                snapshot: &snapshot,
                draft: &draft,
                output: &output(),
                pending: &[]
            })
            .unwrap(),
        receipt
    );
    assert_eq!(store.items(&receipt.head).unwrap(), before);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn no_op_does_not_change_generation_or_native_item_bytes() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let before = store.items(&head).unwrap();
    let draft = ContextDraft {
        document: snapshot.document.clone(),
        next_model: None,
    };
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &draft,
            output: &output(),
            pending: &[],
        })
        .unwrap();
    assert!(!receipt.changed);
    assert_eq!(receipt.generation, 0);
    assert_eq!(receipt.head, head);
    assert_eq!(
        &store.items(&head).unwrap()[..before.len()],
        before.as_slice()
    );
}

#[test]
fn next_sync_execution_resolves_original_call_after_prior_rewrite() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    store.append_items(&head,&[Item(json!({"type":"custom_tool_call","call_id":"second","name":"haskell_sync","input":"next"}))]).unwrap();
    let second = store.claim(&CallId("second".into()), &head).unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let first = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[second.clone()],
        })
        .unwrap();
    let next = store.begin_context(&second, &first.head).unwrap();
    assert_eq!(next.generation, 1);
    assert!(
        next.document
            .blocks
            .iter()
            .any(|b| matches!(b,ContextBlock::Text{text,..}if text=="edited context"))
    );
    assert!(
        !next
            .document
            .blocks
            .iter()
            .any(|b| matches!(b,ContextBlock::Native{preview,..}if preview=="next"))
    );
}

#[test]
fn intervening_compaction_conflicts_and_preserves_call_occurrence() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let compact = RequestId("compact".into());
    let mut items = store.items(&head).unwrap();
    items[0] = Item(json!({"type":"message","role":"user","content":"compact"}));
    store
        .write_compaction_request_with_claims(
            &compact,
            &head,
            "/root",
            &items,
            &[operation.clone()],
            None,
        )
        .unwrap();
    assert!(matches!(
        store.commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[]
        }),
        Err(StoreError::Context(ContextError::Conflict))
    ));
    assert_eq!(
        store
            .begin_context(&operation, &compact)
            .unwrap()
            .generation,
        1
    );
}

#[test]
fn pending_and_standing_groups_are_protected() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let prefix = RequestId("prefix".into());
    store.write_request(&prefix,None,"/root",&[
        Item(json!({"type":"message","role":"developer","content":"standing"})),
        Item(json!({"type":"function_call","call_id":"pending","name":"work","arguments":"{}"})),
    ],Usage::default()).unwrap();
    store
        .lock()
        .execute(
            "UPDATE requests SET parent_id=?2 WHERE id=?1",
            params![head.0, prefix.0],
        )
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let draft = ContextDraft {
        document: ContextDocument::default(),
        next_model: None,
    };
    assert!(matches!(
        store.commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &draft,
            output: &output(),
            pending: &[]
        }),
        Err(StoreError::Context(ContextError::ProtectedGroup))
    ));
}

#[test]
fn failure_output_cannot_commit_staged_context() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let failed = JobOutput::Completed(Err("failed".into()));
    assert!(matches!(
        store.commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &failed,
            pending: &[]
        }),
        Err(StoreError::Context(ContextError::Ineligible))
    ));
}

#[test]
fn model_state_is_exact_incarnation_owned() {
    let store = Store::memory().unwrap();
    let identity = crate::embedding::HostIdentity {
        run: "run".into(),
        actor: AgentPath("/root".into()),
        incarnation: "1".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    let origin = ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    };
    store.initialize_context_model(&origin, "luna").unwrap();
    store
        .lock()
        .execute(
            "UPDATE embedded_bindings SET incarnation='2' WHERE agent_path='/root'",
            [],
        )
        .unwrap();
    let successor = ConversationIdentity::Embedded {
        run: identity.run,
        actor: identity.actor,
        incarnation: "2".into(),
    };
    assert_eq!(store.context_model(&successor).unwrap(), None);
    assert!(store.context_model(&origin).is_err());
}

#[test]
fn invalid_reference_and_duplicate_native_retention_are_refused() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let draft = ContextDraft {
        document: ContextDocument {
            blocks: vec![ContextBlock::Text {
                reference: Some(ContextReference("forged".into())),
                role: ContextRole::User,
                text: "new".into(),
                sources: vec![],
            }],
        },
        next_model: None,
    };
    assert!(matches!(
        store.commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &draft,
            output: &output(),
            pending: &[]
        }),
        Err(StoreError::Context(ContextError::InvalidReference))
    ));
}

#[test]
fn replay_restores_sealed_prefix_model_and_original_operation() {
    let original = Store::memory().unwrap();
    let (head, operation) = setup(&original);
    let snapshot = original.begin_context(&operation, &head).unwrap();
    original
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let evidence = original
        .context_commit_evidence(&operation)
        .unwrap()
        .unwrap();
    let local = Store::memory().unwrap();
    let (local_head, local_operation) = setup(&local);
    let local_snapshot = local.begin_context(&local_operation, &local_head).unwrap();
    let receipt = local
        .restore_context_commit(&local_snapshot, &evidence, &output(), &[])
        .unwrap();
    assert_eq!(receipt.model.as_deref(), Some("sol"));
    assert_eq!(
        local.context_history(&receipt.head).unwrap()[0].2.0["content"],
        "edited context"
    );
    let restored = local
        .context_commit_evidence(&local_operation)
        .unwrap()
        .unwrap();
    assert_eq!(restored.original_operation(), &operation);
    assert_eq!(
        local
            .restore_context_commit(&local_snapshot, &evidence, &output(), &[])
            .unwrap(),
        receipt
    );
}

#[test]
fn replay_refuses_malformed_commit_evidence_and_wrong_output() {
    let original = Store::memory().unwrap();
    let (head, operation) = setup(&original);
    let snapshot = original.begin_context(&operation, &head).unwrap();
    original
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let evidence = original
        .context_commit_evidence(&operation)
        .unwrap()
        .unwrap();
    let local = Store::memory().unwrap();
    let (local_head, local_operation) = setup(&local);
    let local_snapshot = local.begin_context(&local_operation, &local_head).unwrap();
    assert!(
        local
            .restore_context_commit(
                &local_snapshot,
                &evidence,
                &JobOutput::Completed(Ok(json!({"different":true}))),
                &[]
            )
            .is_err()
    );
    original.lock().execute("UPDATE events SET payload=json_remove(payload,'$.version') WHERE kind='context_commit'",[]).unwrap();
    assert!(original.context_commit_evidence(&operation).is_err());
    assert!(local.context_receipt(&local_operation).unwrap().is_none());
}
