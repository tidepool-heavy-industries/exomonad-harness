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
    if let Some(ContextBlock::Text { text, .. }) = document
        .blocks
        .iter_mut()
        .find(|b| matches!(b, ContextBlock::Text { .. }))
    {
        *text = "edited context".into();
    } else {
        panic!("plain text");
    }
    ContextDraft {
        next_effort: None,
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
    assert_eq!(
        state.history[0].2.0["content"],
        "[Agent-authored context note; sources: root:0]\nedited context"
    );
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
        next_effort: None,
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
        next_effort: None,
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
        next_effort: None,
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
fn replay_restores_historical_native_exchange_with_local_original_claim() {
    fn exchange(store: &Store) -> (RequestId, OperationId, OperationId, OperationId) {
        let head = RequestId("exchange".into());
        let completed_output =
            Item(json!({"type":"function_call_output","call_id":"done","output":"evidence"}));
        store.write_request(&head, None, "/root", &[
            Item(json!({"type":"function_call","call_id":"done","name":"read","arguments":"{}"})),
            completed_output.clone(),
            Item(json!({"type":"custom_tool_call","call_id":"delete","name":"haskell_sync","input":"delete"})),
            Item(json!({"type":"custom_tool_call","call_id":"restore","name":"haskell_sync","input":"restore"})),
        ], Usage::default()).unwrap();
        let completed = store.claim(&CallId("done".into()), &head).unwrap();
        store
            .settle_claims(&completed, &completed_output, TerminalOutcome::Success)
            .unwrap();
        let first = store.claim(&CallId("delete".into()), &head).unwrap();
        let second = store.claim(&CallId("restore".into()), &head).unwrap();
        (head, completed, first, second)
    }
    let original = Store::memory().unwrap();
    let (head, _, first, second) = exchange(&original);
    let snapshot = original.begin_context(&first, &head).unwrap();
    let mut saved = snapshot.document.clone();
    let deleted = original
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                next_effort: None,
                document: ContextDocument::default(),
                next_model: None,
            },
            output: &output(),
            pending: std::slice::from_ref(&second),
        })
        .unwrap();
    let snapshot = original.begin_context(&second, &deleted.head).unwrap();
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
    original
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                next_effort: None,
                document: saved,
                next_model: None,
            },
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let deletion = original.context_commit_evidence(&first).unwrap().unwrap();
    let restoration = original.context_commit_evidence(&second).unwrap().unwrap();
    let local = Store::memory().unwrap();
    let (head, completed, first, second) = exchange(&local);
    let snapshot = local.begin_context(&first, &head).unwrap();
    let deleted = local
        .restore_context_commit(
            &snapshot,
            &deletion,
            &output(),
            std::slice::from_ref(&second),
        )
        .unwrap();
    let snapshot = local.begin_context(&second, &deleted.head).unwrap();
    let restored = local
        .restore_context_commit(&snapshot, &restoration, &output(), &[])
        .unwrap();
    let claims = local.claims_on(&restored.head).unwrap();
    let restored_claim = claims
        .iter()
        .filter(|claim| claim.operation == completed)
        .collect::<Vec<_>>();
    assert_eq!(restored_claim.len(), 1);
    assert_eq!(restored_claim[0].state, crate::store::ClaimState::Settled);
    assert_eq!(
        restored_claim[0].output,
        Some(local.put_item(&local.items(&head).unwrap()[1]).unwrap())
    );
    let retained = history(&local.lock(), &restored.head, true).unwrap();
    assert_eq!(retained[0].origin.request, completed.request);
    assert_eq!(retained[0].origin.position, 0);
    assert_eq!(
        local
            .context_commit_evidence(&second)
            .unwrap()
            .unwrap()
            .original_operation(),
        &restoration.original_operation
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

#[test]
fn deferred_child_inherits_committed_context_and_terminal_before_call_stays_frozen() {
    use crate::checkpoint::{CheckpointChild, CheckpointCut};
    use std::sync::Arc;
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    store.lock().execute("INSERT INTO agents(path,head_request,contract,fork_source,state,created_at) VALUES('/root',?1,'{}','{}','active',0)",[&head.0]).unwrap();
    store
        .lock()
        .execute(
            "UPDATE request_items SET position=-1 WHERE request_id=?1 AND position=2",
            [&head.0],
        )
        .unwrap();
    let cuts = store
        .capture_checkpoint_cuts(&operation, &json!({}), Arc::new(()))
        .unwrap();
    assert_eq!(cuts.before_call().cut(), CheckpointCut::BeforeCall);
    let frozen = store
        .context_history(cuts.before_call().snapshot_request())
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let continuing = store.context_receipt(&operation).unwrap().unwrap().head;
    store
        .append_items(
            &continuing,
            &[Item(
                json!({"type":"message","role":"user","content":"after commit"}),
            )],
        )
        .unwrap();
    let parent = AgentPath("/root".into());
    let deferred_path = AgentPath("/root/deferred".into());
    let (child, _) = store
        .attach_checkpoint_child(
            cuts.deferred(),
            CheckpointChild {
                path: &deferred_path,
                parent: &parent,
                contract: &json!({}),
                checkout: &json!({}),
                task: None,
            },
        )
        .unwrap();
    let history = store
        .context_history(child.head_request.as_ref().unwrap())
        .unwrap();
    assert!(
        history
            .iter()
            .any(|(_, _, i)| i.0["content"] == "edited context")
    );
    assert!(
        history
            .iter()
            .any(|(_, _, i)| i.0["type"] == "custom_tool_call_output")
    );
    assert!(
        !history
            .iter()
            .any(|(_, _, i)| i.0["content"] == "after commit")
    );
    let immediate_path = AgentPath("/root/immediate".into());
    let (immediate, _) = store
        .attach_checkpoint_child(
            cuts.before_call(),
            CheckpointChild {
                path: &immediate_path,
                parent: &parent,
                contract: &json!({}),
                checkout: &json!({}),
                task: None,
            },
        )
        .unwrap();
    assert_eq!(
        store
            .context_history(immediate.head_request.as_ref().unwrap())
            .unwrap()
            .iter()
            .map(|(_, _, i)| i)
            .collect::<Vec<_>>(),
        frozen.iter().map(|(_, _, i)| i).collect::<Vec<_>>()
    );
}

#[test]
fn saved_context_restores_owned_prior_text_and_native_group_preserving_suffix() {
    let store = Store::memory().unwrap();
    let head = RequestId("saved".into());
    let first = CallId("first".into());
    let second = CallId("second".into());
    store.write_request(&head,None,"/root",&[
        Item(json!({"type":"message","role":"user","content":"original"})),
        Item(json!({"type":"function_call","call_id":"done","name":"read","arguments":"{}"})),
        Item(json!({"type":"function_call_output","call_id":"done","output":"evidence"})),
        Item(json!({"type":"custom_tool_call","call_id":"first","name":"haskell_sync","input":"first"})),
        Item(json!({"type":"custom_tool_call","call_id":"second","name":"haskell_sync","input":"second"})),
    ],Usage::default()).unwrap();
    let completed = store.claim(&CallId("done".into()), &head).unwrap();
    store
        .settle_claims(
            &completed,
            &store.items(&head).unwrap()[2],
            TerminalOutcome::Success,
        )
        .unwrap();
    let op1 = store.claim(&first, &head).unwrap();
    let op2 = store.claim(&second, &head).unwrap();
    let snap1 = store.begin_context(&op1, &head).unwrap();
    let saved = snap1.document.clone();
    let first_receipt = store
        .commit_context(ContextCommit {
            snapshot: &snap1,
            draft: &ContextDraft {
                next_effort: None,
                document: ContextDocument { blocks: vec![] },
                next_model: None,
            },
            output: &output(),
            pending: std::slice::from_ref(&op2),
        })
        .unwrap();
    store
        .append_items(
            &first_receipt.head,
            &[Item(
                json!({"type":"message","role":"user","content":"late"}),
            )],
        )
        .unwrap();
    let snap2 = store.begin_context(&op2, &first_receipt.head).unwrap();
    let mut saved = saved;
    saved.blocks.extend(
        snap2
            .document
            .blocks
            .iter()
            .filter(|b| {
                matches!(
                    b,
                    ContextBlock::Native {
                        protected: true,
                        ..
                    }
                )
            })
            .cloned(),
    );
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &snap2,
            draft: &ContextDraft {
                next_effort: None,
                document: saved,
                next_model: None,
            },
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let claims = store.claims_on(&receipt.head).unwrap();
    let restored = claims
        .iter()
        .filter(|claim| claim.operation == completed)
        .collect::<Vec<_>>();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].state, crate::store::ClaimState::Settled);
    assert_eq!(
        restored[0].output,
        Some(store.put_item(&store.items(&head).unwrap()[2]).unwrap())
    );
    let items = store.context_history(&receipt.head).unwrap();
    assert_eq!(items[0].2.0["content"], "original");
    assert!(
        items
            .iter()
            .any(|(_, _, i)| i.0["call_id"] == "done" && i.0["type"] == "function_call_output")
    );
    assert!(items.iter().any(|(_, _, i)| i.0["content"] == "late"));
    assert!(items.iter().any(|(_,_,i)|i.0["call_id"]=="second" && i.0["type"]=="custom_tool_call_output"));
}

#[test]
fn saved_native_restore_refuses_missing_or_forged_original_claim() {
    for forged in [false, true] {
        let store = Store::memory().unwrap();
        let (head, first) = setup(&store);
        let parent = RequestId("completed".into());
        let invocation =
            Item(json!({"type":"function_call","call_id":"done","name":"read","arguments":"{}"}));
        let completed_output =
            Item(json!({"type":"function_call_output","call_id":"done","output":"evidence"}));
        store
            .write_request(
                &parent,
                None,
                "/root",
                &[invocation, completed_output.clone()],
                Usage::default(),
            )
            .unwrap();
        let completed = store.claim(&CallId("done".into()), &parent).unwrap();
        store
            .settle_claims(&completed, &completed_output, TerminalOutcome::Success)
            .unwrap();
        store
            .lock()
            .execute(
                "UPDATE requests SET parent_id=?2 WHERE id=?1",
                params![head.0, parent.0],
            )
            .unwrap();
        store.append_items(&head, &[Item(json!({"type":"custom_tool_call","call_id":"restore","name":"haskell_sync","input":"restore"}))]).unwrap();
        let second = store.claim(&CallId("restore".into()), &head).unwrap();
        let snapshot = store.begin_context(&first, &head).unwrap();
        let mut saved = snapshot.document.clone();
        let deleted = store
            .commit_context(ContextCommit {
                snapshot: &snapshot,
                draft: &ContextDraft {
                    next_effort: None,
                    document: ContextDocument::default(),
                    next_model: None,
                },
                output: &output(),
                pending: std::slice::from_ref(&second),
            })
            .unwrap();
        if forged {
            let foreign = store.standalone_identity(AgentPath("/foreign".into()));
            store
                .lock()
                .execute(
                    "UPDATE claims SET origin=?2 WHERE origin_request_id=?1",
                    params![parent.0, serde_json::to_string(&foreign).unwrap()],
                )
                .unwrap();
        } else {
            store
                .lock()
                .execute("DELETE FROM claims WHERE origin_request_id=?1", [&parent.0])
                .unwrap();
        }
        let restore = store.begin_context(&second, &deleted.head).unwrap();
        saved.blocks.extend(
            restore
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
        let before = store.context_history(&deleted.head).unwrap();
        let result = store.commit_context(ContextCommit {
            snapshot: &restore,
            draft: &ContextDraft {
                next_effort: None,
                document: saved,
                next_model: None,
            },
            output: &output(),
            pending: &[],
        });
        if forged {
            assert!(matches!(result, Err(StoreError::OperationOriginMismatch)));
        } else {
            assert!(matches!(
                result,
                Err(StoreError::Context(ContextError::ProtectedGroup))
            ));
        }
        assert_eq!(store.context_history(&deleted.head).unwrap(), before);
        assert!(store.context_receipt(&second).unwrap().is_none());
        assert!(
            store
                .replay_tool_output_operation(&second)
                .unwrap()
                .is_none()
        );
        assert!(store.children_of(&deleted.head).unwrap().is_empty());
    }
}

#[test]
fn foreign_saved_context_reference_is_rejected() {
    let store = Store::memory().unwrap();
    let (head, op) = setup(&store);
    let snapshot = store.begin_context(&op, &head).unwrap();
    let foreign = RequestId("foreign".into());
    store
        .write_request(
            &foreign,
            None,
            "/other",
            &[Item(
                json!({"type":"message","role":"user","content":"secret"}),
            )],
            Usage::default(),
        )
        .unwrap();
    let draft = ContextDraft {
        next_effort: None,
        document: store.read_context(&foreign).unwrap(),
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
fn v9_migration_preserves_raw_items_and_adds_nullable_provenance() {
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    let old_schema = super::super::schema::SQL.replace(
        " source_request TEXT, source_position INTEGER, context_sources TEXT,\n context_note INTEGER NOT NULL DEFAULT 0,\n",
        "",
    );
    connection.execute_batch(&old_schema).unwrap();
    connection
        .execute("INSERT INTO schema_version VALUES(9)", [])
        .unwrap();
    connection
        .execute(
            "INSERT INTO requests(id,branch,created_at) VALUES('old','/root',0)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO items(hash,json) VALUES('oldhash','{\"future\":true}')",
            [],
        )
        .unwrap();
    connection
        .execute("INSERT INTO request_items VALUES('old',0,'oldhash')", [])
        .unwrap();
    super::super::schema::initialize(&mut connection).unwrap();
    let data: (String,Option<String>,Option<i64>,Option<String>) = connection.query_row("SELECT i.json,ri.source_request,ri.source_position,ri.context_sources FROM request_items ri JOIN items i ON i.hash=ri.item_hash",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(data, ("{\"future\":true}".into(), None, None, None));
    assert_eq!(
        connection
            .query_row("SELECT version FROM schema_version", [], |r| r
                .get::<_, u32>(0))
            .unwrap(),
        10
    );
}

#[test]
fn model_switch_refuses_opaque_protected_suffix_without_settling_output() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let snapshot = store.begin_context(&operation, &head).unwrap();
    store
        .append_items(
            &head,
            &[Item(
                json!({"type":"reasoning","encrypted_content":"opaque"}),
            )],
        )
        .unwrap();
    assert!(matches!(
        store.commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[]
        }),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
    assert_eq!(
        store.context_model(&operation.origin).unwrap().as_deref(),
        Some("luna")
    );
    assert!(
        terminal::exact_terminal(&store.lock(), &operation)
            .unwrap()
            .is_none()
    );
}

#[test]
fn replay_keeps_duplicate_item_occurrences_distinct_for_later_edit() {
    fn duplicate_setup(store: &Store) -> (RequestId, OperationId) {
        let (head, operation) = setup(store);
        store
            .lock()
            .execute(
                "UPDATE request_items SET position=-position-10 WHERE request_id=?1",
                [&head.0],
            )
            .unwrap();
        store
            .lock()
            .execute(
                "UPDATE request_items SET position=-position-8 WHERE request_id=?1",
                [&head.0],
            )
            .unwrap();
        let item = Item(json!({"type":"message","role":"user","content":"same"}));
        let hash = store.put_item(&item).unwrap();
        store.lock().execute("INSERT INTO request_items(request_id,position,item_hash) VALUES(?1,0,?2),(?1,1,?2)",params![head.0,hash.0]).unwrap();
        (head, operation)
    }
    let original = Store::memory().unwrap();
    let (head, operation) = duplicate_setup(&original);
    let snapshot = original.begin_context(&operation, &head).unwrap();
    let mut draft = ContextDraft {
        next_effort: None,
        document: snapshot.document.clone(),
        next_model: None,
    };
    if let ContextBlock::Text { text, .. } = &mut draft.document.blocks[2] {
        *text = "changed".into()
    };
    original
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &draft,
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let evidence = original
        .context_commit_evidence(&operation)
        .unwrap()
        .unwrap();
    let local = Store::memory().unwrap();
    let (localhead, localop) = duplicate_setup(&local);
    let localsnapshot = local.begin_context(&localop, &localhead).unwrap();
    let receipt = local
        .restore_context_commit(&localsnapshot, &evidence, &output(), &[])
        .unwrap();
    local.append_items(&receipt.head,&[Item(json!({"type":"custom_tool_call","call_id":"next","name":"haskell_sync","input":"noop"}))]).unwrap();
    let next = local.claim(&CallId("next".into()), &receipt.head).unwrap();
    let nextsnapshot = local.begin_context(&next, &receipt.head).unwrap();
    local
        .commit_context(ContextCommit {
            snapshot: &nextsnapshot,
            draft: &ContextDraft {
                next_effort: None,
                document: nextsnapshot.document.clone(),
                next_model: None,
            },
            output: &output(),
            pending: &[],
        })
        .unwrap();
}

#[test]
fn guarded_commit_cancellation_rolls_back_every_publication_write() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let checks = AtomicUsize::new(0);
    let error = store
        .commit_context_guarded(
            ContextCommit {
                snapshot: &snapshot,
                draft: &edited(&snapshot),
                output: &output(),
                pending: &[],
            },
            || checks.fetch_add(1, Ordering::SeqCst) >= 2,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        StoreError::Context(ContextError::Cancelled)
    ));
    assert_eq!(
        store.context_history(&head).unwrap()[0].2.0["content"],
        "old context"
    );
    assert_eq!(
        store.context_model(&operation.origin).unwrap().as_deref(),
        Some("luna")
    );
    assert!(store.context_receipt(&operation).unwrap().is_none());
    assert!(
        terminal::exact_terminal(&store.lock(), &operation)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM requests", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn second_sync_capture_uses_current_edited_context_and_original_call() {
    use std::sync::Arc;
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    store.lock().execute("INSERT INTO agents(path,head_request,contract,fork_source,state,created_at) VALUES('/root',?1,'{}','{}','active',0)",[&head.0]).unwrap();
    store
        .lock()
        .execute(
            "UPDATE request_items SET position=-1 WHERE request_id=?1 AND position=2",
            [&head.0],
        )
        .unwrap();
    store.append_items(&head,&[Item(json!({"type":"custom_tool_call","call_id":"second","name":"haskell_sync","input":"second"}))]).unwrap();
    let second = store.claim(&CallId("second".into()), &head).unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: std::slice::from_ref(&second),
        })
        .unwrap();
    let cuts = store
        .capture_checkpoint_cuts_at_head(&second, &receipt.head, &json!({}), Arc::new(()))
        .unwrap();
    assert!(
        store
            .context_history(cuts.before_call().snapshot_request())
            .unwrap()
            .iter()
            .any(|(_, _, i)| i.0["content"] == "edited context")
    );
    assert_eq!(cuts.before_call().operation(), Some(&second));
}

#[test]
fn authored_notes_project_attribution_without_changing_retained_item_bytes() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let source = match &snapshot.document.blocks[0] {
        ContextBlock::Text {
            reference: Some(r), ..
        } => r.clone(),
        _ => panic!("text"),
    };
    let draft = ContextDraft {
        next_effort: None,
        document: ContextDocument {
            blocks: vec![ContextBlock::Text {
                reference: None,
                role: ContextRole::User,
                text: "summary".into(),
                sources: vec![source],
            }],
        },
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
    let raw = store.context_history(&receipt.head).unwrap();
    let projected = store
        .context_request_state(&receipt.head, &operation.origin)
        .unwrap();
    assert_eq!(raw[0].2.0["content"], "summary");
    assert_eq!(raw[0].0, projected.history[0].0);
    assert_ne!(raw[0].1, projected.history[0].1);
    let sealed_raw: String = store
        .lock()
        .query_row(
            "SELECT json FROM items WHERE hash=?1",
            [&projected.history[0].1.0],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Item>(&sealed_raw).unwrap(),
        projected.history[0].2
    );
    assert_eq!(
        projected.history[0].2.0["content"],
        "[Agent-authored context note; sources: root:0]\nsummary"
    );
    assert_eq!(raw[1..], projected.history[1..]);
    assert_eq!(
        store.context_history(&head).unwrap()[0].2.0["content"],
        "old context"
    );
    let document = store.read_context(&receipt.head).unwrap();
    assert!(matches!(&document.blocks[0],ContextBlock::Text {text,..} if text=="summary"));
}

#[test]
fn closed_opaque_response_converts_to_notes_preserving_raw_evidence() {
    let store = Store::memory().unwrap();
    let parent = RequestId("opaque-response".into());
    let reasoning = Item(json!({"type":"reasoning","encrypted_content":"sealed","summary":[]}));
    let answer = Item(json!({"type":"message","role":"assistant","content":"useful discovery"}));
    store
        .write_request(
            &parent,
            None,
            "/root",
            &[reasoning.clone(), answer.clone()],
            Usage::default(),
        )
        .unwrap();
    let hashes = store
        .items(&parent)
        .unwrap()
        .iter()
        .map(|item| store.put_item(item).unwrap().0)
        .collect::<Vec<_>>();
    store
        .lock()
        .execute(
            "INSERT INTO events(request_id,kind,payload,created_at) VALUES(?1,'model_turn',?2,0)",
            params![parent.0, json!({"response":{"items":hashes}}).to_string()],
        )
        .unwrap();
    let head = RequestId("opaque-edit".into());
    store.write_request(&head,Some(&parent),"/root",&[Item(json!({"type":"custom_tool_call","call_id":"edit","name":"haskell_sync","input":"edit"}))],Usage::default()).unwrap();
    let operation = store.claim(&CallId("edit".into()), &head).unwrap();
    store
        .initialize_context_model(&operation.origin, "luna")
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let reference = match &snapshot.document.blocks[0] {
        ContextBlock::Native {
            reference,
            kind: ContextNativeKind::Opaque,
            protected: false,
            ..
        } => reference.clone(),
        other => panic!("{other:?}"),
    };
    assert!(matches!(
        store.commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                next_effort: None,
                document: snapshot.document.clone(),
                next_model: Some("sol".into())
            },
            output: &output(),
            pending: &[]
        }),
        Err(StoreError::Context(ContextError::OpaqueModel))
    ));
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                next_effort: None,
                document: ContextDocument {
                    blocks: vec![ContextBlock::Text {
                        reference: None,
                        role: ContextRole::User,
                        text: "useful discovery".into(),
                        sources: vec![reference],
                    }],
                },
                next_model: Some("sol".into()),
            },
            output: &output(),
            pending: &[],
        })
        .unwrap();
    assert_eq!(
        store.context_model(&operation.origin).unwrap().as_deref(),
        Some("sol")
    );
    assert!(
        !store
            .context_history(&receipt.head)
            .unwrap()
            .iter()
            .any(|(_, _, i)| i.0["encrypted_content"] == "sealed")
    );
    assert_eq!(
        store.items(&parent).unwrap().iter().collect::<Vec<_>>(),
        vec![&reasoning, &answer]
    );
}

#[test]
fn blocks_preserve_duplicate_call_order_orphans_and_cut_opaque_groups() {
    let store = Store::memory().unwrap();
    let parent = RequestId("grouping-history".into());
    let items = [
        Item(json!({"type":"message","role":"user","content":"before"})),
        Item(json!({"type":"custom_tool_call","call_id":"duplicate","name":"one","input":"1"})),
        Item(json!({"type":"custom_tool_call_output","call_id":"duplicate","output":"first"})),
        Item(json!({"type":"custom_tool_call","call_id":"duplicate","name":"two","input":"2"})),
        Item(json!({"type":"custom_tool_call_output","call_id":"duplicate","output":"second"})),
        Item(json!({"type":"function_call_output","call_id":"orphan","output":"unpaired"})),
        Item(json!({"type":"reasoning","encrypted_content":"sealed-1"})),
        Item(json!({"type":"message","role":"assistant","content":"between"})),
        Item(json!({"type":"reasoning","encrypted_content":"sealed-2"})),
        Item(json!({"type":"message","role":"assistant","content":"after"})),
        Item(json!({"type":"reasoning","encrypted_content":"sealed-3"})),
    ];
    store
        .write_request(&parent, None, "/root", &items, Usage::default())
        .unwrap();
    let hashes = [items[6].clone(), items[8].clone(), items[10].clone()]
        .iter()
        .map(|item| store.put_item(item).unwrap().0)
        .collect::<Vec<_>>();
    store
        .lock()
        .execute(
            "INSERT INTO events(request_id,kind,payload,created_at) VALUES(?1,'model_turn',?2,0)",
            params![parent.0, json!({"response":{"items":hashes}}).to_string()],
        )
        .unwrap();

    let c = store.lock();
    let all = history(&c, &parent, true).unwrap();
    let grouped = blocks(&c, &all, 10).unwrap();
    assert_eq!(grouped.len(), 5);
    assert!(matches!(grouped[0].block, ContextBlock::Text { .. }));
    assert_eq!(grouped[1].items.len(), 2);
    assert_eq!(grouped[1].items[0].item.0["name"], "one");
    assert_eq!(grouped[1].items[1].item.0["output"], "first");
    assert_eq!(
        grouped[1].block,
        ContextBlock::Native {
            reference: reference(&grouped[1].items),
            kind: ContextNativeKind::CompletedExchange,
            preview: preview(&grouped[1].items),
            protected: false,
        }
    );
    assert_eq!(grouped[2].items.len(), 2);
    assert_eq!(grouped[2].items[0].item.0["name"], "two");
    assert_eq!(grouped[2].items[1].item.0["output"], "second");
    assert!(
        grouped[3].mandatory,
        "an output without an earlier call is pending"
    );
    assert_eq!(grouped[3].items[0].item.0["call_id"], "orphan");
    assert_eq!(grouped[4].items.len(), 4);
    assert!(grouped[4].opaque);
    assert!(
        grouped[4].mandatory,
        "the opaque membership crosses the cut"
    );
    assert_eq!(grouped[4].items[0].item.0["encrypted_content"], "sealed-1");
    assert_eq!(grouped[4].items[1].item.0["content"], "between");
    assert_eq!(grouped[4].items[2].item.0["encrypted_content"], "sealed-2");
    assert_eq!(grouped[4].items[3].item.0["content"], "after");
}

#[test]
fn claim_lineage_selects_nearest_exact_operation_for_continuation() {
    use crate::store::ClaimState;
    let store = Store::memory().unwrap();
    let (root, operation) = setup(&store);
    let initial = RequestId("child-initial".into());
    let final_head = RequestId("child-final".into());
    let next = RequestId("child-next".into());
    for (request, parent) in [
        (&initial, &root),
        (&final_head, &initial),
        (&next, &final_head),
    ] {
        store
            .write_request(request, Some(parent), "/root/child", &[], Usage::default())
            .unwrap();
    }
    let origin = serde_json::to_string(&operation.origin).unwrap();
    let hash = store.put_item(&Item(json!({"result":"retained"}))).unwrap();
    store.lock().execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state,output_hash) VALUES(?1,?2,?3,?4,'pending',NULL),(?1,?2,?3,?5,'settled',?6)",params![origin,operation.request.0,operation.call.0,initial.0,final_head.0,hash.0]).unwrap();
    let claims = store
        .claims_on_branch_lineage(&next, "/root/child")
        .unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].operation, operation);
    assert_eq!(claims[0].request, final_head);
    assert_eq!(claims[0].state, ClaimState::Settled);
    assert_eq!(claims[0].output, Some(hash));
    assert!(
        store
            .claims_on_branch_lineage(&next, "/root/other")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn claim_lineage_stops_at_fork_and_compaction_boundaries() {
    let store = Store::memory().unwrap();
    let (root, operation) = setup(&store);
    let initial = RequestId("isolated-child".into());
    let compacted = RequestId("child-compacted".into());
    store
        .write_request(&initial, Some(&root), "/root/child", &[], Usage::default())
        .unwrap();
    assert!(
        store
            .claims_on_branch_lineage(&initial, "/root/child")
            .unwrap()
            .is_empty()
    );
    let origin = serde_json::to_string(&operation.origin).unwrap();
    store.lock().execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state) VALUES(?1,?2,?3,?4,'pending')",params![origin,operation.request.0,operation.call.0,initial.0]).unwrap();
    store
        .write_request(
            &compacted,
            Some(&initial),
            "/root/child",
            &[],
            Usage::default(),
        )
        .unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO session_state(session_id,state,updated_at) VALUES(?1,'true',0)",
            [format!("harness:compaction:{}", compacted.0)],
        )
        .unwrap();
    assert!(
        store
            .claims_on_branch_lineage(&compacted, "/root/child")
            .unwrap()
            .is_empty()
    );
    store.lock().execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state) VALUES(?1,?2,?3,?4,'pending')",params![origin,operation.request.0,operation.call.0,compacted.0]).unwrap();
    let claims = store
        .claims_on_branch_lineage(&compacted, "/root/child")
        .unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].request, compacted);
    assert_eq!(claims[0].operation, operation);
}

#[test]
fn staged_effort_is_atomic_preserves_native_prefix_and_consumes_older_pending() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    store
        .append_items(
            &head,
            &[Item(
                json!({"type":"reasoning","encrypted_content":"keep opaque continuity"}),
            )],
        )
        .unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let before = store.context_history(&head).unwrap();
    store
        .save_pending_effort(operation.origin.actor(), Effort::Medium)
        .unwrap();
    let draft = ContextDraft {
        document: snapshot.document.clone(),
        next_model: None,
        next_effort: Some(Effort::High),
    };
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &draft,
            output: &output(),
            pending: &[],
        })
        .unwrap();
    assert_eq!(receipt.head, head);
    assert_eq!(receipt.model.as_deref(), Some("luna"));
    assert!(receipt.changed);
    let after = store.context_history(&head).unwrap();
    assert_eq!(&after[..before.len()], &before);
    assert_eq!(
        after[after.len() - 2].2.0["type"],
        "custom_tool_call_output"
    );
    assert_eq!(
        after.last().unwrap().2,
        Item::configuration_update(Effort::High)
    );
    assert!(
        store
            .apply_pending_effort(operation.origin.actor(), &head)
            .unwrap()
            .is_none()
    );
    // A lost acknowledgment cannot append the setting or output twice.
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
    assert_eq!(store.context_history(&head).unwrap(), after);
    // A later ordinary effort choice still uses the existing pending boundary.
    store
        .save_pending_effort(operation.origin.actor(), Effort::Low)
        .unwrap();
    let next = RequestId("next-effort-request".into());
    store
        .write_request(&next, Some(&head), "/root", &[], Usage::default())
        .unwrap();
    assert!(
        store
            .apply_pending_effort(operation.origin.actor(), &next)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        store
            .context_history(&next)
            .unwrap()
            .last()
            .unwrap()
            .2
            .configuration_effort(),
        Some(Effort::Low)
    );
    assert_eq!(store.context_history(&head).unwrap(), after);
}

#[test]
fn staged_effort_publication_failure_and_cancellation_restore_pending_and_terminal() {
    for cancel in [false, true] {
        let store = Store::memory().unwrap();
        let (head, operation) = setup(&store);
        let snapshot = store.begin_context(&operation, &head).unwrap();
        let before = store.context_history(&head).unwrap();
        store
            .save_pending_effort(operation.origin.actor(), Effort::Medium)
            .unwrap();
        if !cancel {
            store.lock().execute_batch("CREATE TRIGGER reject_effort_commit BEFORE INSERT ON events WHEN NEW.kind='context_commit' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
        }
        let draft = edited(&snapshot);
        let draft = ContextDraft {
            next_effort: Some(Effort::High),
            ..draft
        };
        let cancellation_checks = std::cell::Cell::new(0);
        let result = store.commit_context_guarded(
            ContextCommit {
                snapshot: &snapshot,
                draft: &draft,
                output: &output(),
                pending: &[],
            },
            || {
                cancellation_checks.set(cancellation_checks.get() + 1);
                cancel && cancellation_checks.get() == 3
            },
        );
        assert!(result.is_err());
        assert_eq!(store.context_history(&head).unwrap(), before);
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
        assert!(store.children_of(&head).unwrap().is_empty());
        assert!(
            store
                .apply_pending_effort(operation.origin.actor(), &head)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            store
                .context_history(&head)
                .unwrap()
                .last()
                .unwrap()
                .2
                .configuration_effort(),
            Some(Effort::Medium)
        );
    }
}

#[test]
fn sealed_replay_restores_staged_effort_after_local_output_without_rewriting_prefix() {
    let source = Store::memory().unwrap();
    let (head, operation) = setup(&source);
    let snapshot = source.begin_context(&operation, &head).unwrap();
    source
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                document: snapshot.document.clone(),
                next_model: None,
                next_effort: Some(Effort::High),
            },
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let evidence = source.context_commit_evidence(&operation).unwrap().unwrap();
    let destination = Store::memory().unwrap();
    let (local_head, local_operation) = setup(&destination);
    let local_snapshot = destination
        .begin_context(&local_operation, &local_head)
        .unwrap();
    let before = destination.context_history(&local_head).unwrap();
    let receipt = destination
        .restore_context_commit(&local_snapshot, &evidence, &output(), &[])
        .unwrap();
    let after = destination.context_history(&receipt.head).unwrap();
    assert_eq!(&after[..before.len()], &before);
    assert_eq!(
        after.last().unwrap().2.configuration_effort(),
        Some(Effort::High)
    );
    assert_eq!(
        after[after.len() - 2].2.0["call_id"],
        local_operation.call.0
    );
    assert_eq!(
        destination
            .context_commit_evidence(&local_operation)
            .unwrap()
            .unwrap()
            .next_effort,
        Some(Effort::High)
    );
}

#[test]
fn staged_effort_receipt_requires_the_current_internal_version() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let snapshot = store.begin_context(&operation, &head).unwrap();
    store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                document: snapshot.document.clone(),
                next_model: None,
                next_effort: Some(Effort::High),
            },
            output: &output(),
            pending: &[],
        })
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE events SET payload=json_set(payload,'$.version',1) WHERE kind='context_commit'",
            [],
        )
        .unwrap();
    assert!(matches!(
        store.context_commit_evidence(&operation),
        Err(StoreError::Context(ContextError::UnsupportedState))
    ));
}

fn here_context_snapshot() -> (Store, RequestId, RequestId, OperationId) {
    let store = Store::memory().unwrap();
    let (head, first) = setup(&store);
    let snapshot = store.begin_context(&first, &head).unwrap();
    let edited = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let spawn_call = CallId("spawn".into());
    let spawn_output =
        Item(json!({"type":"function_call_output","call_id":"spawn","output":"child admitted"}));
    store.append_items(&edited.head, &[
        Item(json!({"type":"function_call","call_id":"spawn","name":"spawn_agent","arguments":"{}"})),
        spawn_output.clone(),
    ]).unwrap();
    let spawn = store.claim(&spawn_call, &edited.head).unwrap();
    let source = RequestId("rewritten-parent".into());
    let retained = store
        .context_history(&edited.head)
        .unwrap()
        .into_iter()
        .map(|(_, _, item)| item)
        .collect::<Vec<_>>();
    store
        .write_compaction_request_with_claims(
            &source,
            &edited.head,
            "/root",
            &retained,
            std::slice::from_ref(&spawn),
            None,
        )
        .unwrap();
    let parent = AgentPath("/root".into());
    let child = AgentPath("/root/child".into());
    store
        .admit_agent(
            &parent,
            None,
            Some(&source),
            &json!({}),
            &json!({"kind":"root"}),
        )
        .unwrap();
    let target = RequestId("here-snapshot".into());
    store
        .admit_here_agent_from_invocation(
            &child,
            &parent,
            &target,
            &source,
            &spawn_call,
            &json!({}),
            "/root",
            "/root/child",
            "AtBoundary",
            &Item(json!({"type":"message","role":"user","content":"edit inherited context"})),
        )
        .unwrap();
    assert!(
        store
            .claims_on(&target)
            .unwrap()
            .iter()
            .any(|claim| claim.operation == spawn)
    );
    store
        .settle_claims(&spawn, &spawn_output, TerminalOutcome::Success)
        .unwrap();
    (store, source, target, spawn)
}

#[test]
fn here_output_copy_preserves_occurrences_and_child_context_freeze() {
    let (store, source, target, spawn) = here_context_snapshot();
    let before = request_occurrences(&store.lock(), &target).unwrap();
    let source_output = request_occurrences(&store.lock(), &source)
        .unwrap()
        .into_iter()
        .find(|occurrence| {
            occurrence.item.0["type"] == "function_call_output"
                && occurrence.item.0["call_id"] == spawn.call.0
        })
        .unwrap();
    assert_ne!(source_output.origin.request, source);
    assert!(
        before
            .iter()
            .any(|occurrence| occurrence.note && !occurrence.sources.is_empty())
    );
    store.lock().execute_batch("CREATE TRIGGER refuse_here_copy BEFORE INSERT ON request_items WHEN NEW.request_id='here-snapshot' AND (SELECT json_extract(json,'$.type') FROM items WHERE hash=NEW.item_hash)='function_call_output' AND (SELECT json_extract(json,'$.call_id') FROM items WHERE hash=NEW.item_hash)='spawn' BEGIN SELECT RAISE(ABORT,'refuse output'); END;").unwrap();
    assert!(
        store
            .copy_call_output_if_persisted(&source, &target, &spawn.call)
            .is_err()
    );
    assert_eq!(request_occurrences(&store.lock(), &target).unwrap(), before);
    store
        .lock()
        .execute_batch("DROP TRIGGER refuse_here_copy;")
        .unwrap();
    assert!(
        store
            .copy_call_output_if_persisted(&source, &target, &spawn.call)
            .unwrap()
    );
    let copied = request_occurrences(&store.lock(), &target).unwrap();
    let spawn_position = copied
        .iter()
        .position(|occurrence| {
            occurrence.item.0["type"] == "function_call"
                && occurrence.item.0["call_id"] == spawn.call.0
        })
        .unwrap();
    assert_eq!(copied[spawn_position + 1].origin, source_output.origin);
    assert_eq!(copied[spawn_position + 1].item, source_output.item);
    for previous in &before {
        let retained = copied
            .iter()
            .find(|occurrence| occurrence.origin == previous.origin)
            .unwrap();
        assert_eq!(retained.item, previous.item);
        assert_eq!(retained.sources, previous.sources);
        assert_eq!(retained.note, previous.note);
    }
    assert!(
        store
            .copy_call_output_if_persisted(&source, &target, &spawn.call)
            .unwrap()
    );
    assert_eq!(request_occurrences(&store.lock(), &target).unwrap(), copied);
    store.append_items(&target, &[Item(json!({"type":"custom_tool_call","call_id":"child-edit","name":"haskell_sync","input":"keep context"}))]).unwrap();
    let operation = store.claim(&CallId("child-edit".into()), &target).unwrap();
    let snapshot = store.begin_context(&operation, &target).unwrap();
    let committed = store
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
    assert!(!committed.changed);
    let deferred = receipt(&store.lock(), &operation)
        .unwrap()
        .unwrap()
        .deferred_head;
    let frozen = request_occurrences(&store.lock(), &deferred).unwrap();
    for occurrence in copied {
        let retained = frozen
            .iter()
            .find(|retained| retained.origin == occurrence.origin)
            .unwrap();
        assert_eq!(retained.item, occurrence.item);
        assert_eq!(retained.sources, occurrence.sources);
        assert_eq!(retained.note, occurrence.note);
    }
    let claims = store.claims_on(&deferred).unwrap();
    let inherited = claims
        .iter()
        .filter(|claim| claim.operation == spawn)
        .collect::<Vec<_>>();
    assert_eq!(inherited.len(), 1);
    assert_eq!(inherited[0].state, crate::store::ClaimState::Settled);
    assert_eq!(inherited[0].output, Some(source_output.hash));
    assert!(
        store
            .context_request_state(&target, &operation.origin)
            .unwrap()
            .history
            .iter()
            .any(|(_, _, item)| item.0["content"]
                .as_str()
                .is_some_and(|text| text.contains("sources: root:0")))
    );
}

#[test]
fn here_child_context_freeze_refuses_forged_parent_authority() {
    let (store, source, target, spawn) = here_context_snapshot();
    assert!(
        store
            .copy_call_output_if_persisted(&source, &target, &spawn.call)
            .unwrap()
    );
    store.append_items(&target, &[Item(json!({"type":"custom_tool_call","call_id":"child-edit","name":"haskell_sync","input":"keep context"}))]).unwrap();
    let operation = store.claim(&CallId("child-edit".into()), &target).unwrap();
    let foreign = store.standalone_identity(AgentPath("/foreign".into()));
    store
        .lock()
        .execute(
            "UPDATE claims SET origin=?3 WHERE origin_request_id=?1 AND call_id=?2",
            params![
                spawn.request.0,
                spawn.call.0,
                serde_json::to_string(&foreign).unwrap()
            ],
        )
        .unwrap();
    let snapshot = store.begin_context(&operation, &target).unwrap();
    let before = request_occurrences(&store.lock(), &target).unwrap();
    let requests: i64 = store
        .lock()
        .query_row("SELECT COUNT(*) FROM requests", [], |row| row.get(0))
        .unwrap();
    assert!(matches!(
        store.commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                document: snapshot.document.clone(),
                next_model: None,
                next_effort: None
            },
            output: &output(),
            pending: &[],
        }),
        Err(StoreError::OperationOriginMismatch)
    ));
    assert_eq!(request_occurrences(&store.lock(), &target).unwrap(), before);
    assert_eq!(
        store
            .lock()
            .query_row("SELECT COUNT(*) FROM requests", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        requests
    );
    assert!(store.context_receipt(&operation).unwrap().is_none());
    assert!(
        store
            .replay_tool_output_operation(&operation)
            .unwrap()
            .is_none()
    );
}

fn record_finalize_marker(store: &Store, head: &RequestId, item: &Item) -> i64 {
    let schema = crate::finalize::tool_schema::<String>().unwrap();
    let issued = crate::transport::ResponsesRequest {
        input: vec![],
        instructions: "typed reply".into(),
        tools: vec![schema.clone()].into(),
        tools_allowed: None,
        model: "sol".into(),
        pinned_effort: Effort::Low,
        session_id: "marker".into(),
    };
    let event = store
        .record_replay_turn(
            head,
            &issued,
            &crate::transport::ResponsesTurn {
                response_id: "finalize-response".into(),
                items: vec![item.clone()],
                usage: crate::transport::Usage::default(),
            },
        )
        .unwrap();
    let validated = crate::finalize::ValidatedFinalize::parse(item, &schema).unwrap();
    store
        .record_validated_finalize(event, head, &validated)
        .unwrap();
    event
}

#[test]
fn validated_finalize_marker_survives_owned_context_rewrites_and_frozen_copies() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let item = Item(
        json!({"type":"function_call","call_id":"final","name":"finalize","arguments":{"result":"ready"}}),
    );
    store.append_items(&head, &[item]).unwrap();
    record_finalize_marker(
        &store,
        &head,
        &store.items(&head).unwrap().last().unwrap().clone(),
    );
    let snapshot = store.begin_context(&operation, &head).unwrap();
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[],
        })
        .unwrap();
    let mut connection = store.lock();
    let tx = connection.transaction().unwrap();
    let frozen = freeze_committed_context(&tx, &receipt.head, &store.store_id).unwrap();
    let all = history(&tx, &frozen, true).unwrap();
    let final_item = all
        .iter()
        .find(|item| item.item.0["call_id"] == "final")
        .unwrap();
    assert_eq!(final_item.origin.request, head);
    assert!(super::super::replay::is_validated_completion(&tx, &final_item.origin).unwrap());
    tx.commit().unwrap();
}

#[test]
fn unclaimed_provider_calls_cannot_borrow_finalize_name_or_another_occurrences_marker() {
    for name in ["ordinary", "finalize"] {
        let store = Store::memory().unwrap();
        let (head, operation) = setup(&store);
        let item = Item(
            json!({"type":"function_call","call_id":"unclaimed","name":name,"arguments":{"result":"ready"}}),
        );
        store.append_items(&head, &[item]).unwrap();
        let snapshot = store.begin_context(&operation, &head).unwrap();
        assert!(matches!(
            store.commit_context(ContextCommit {
                snapshot: &snapshot,
                draft: &edited(&snapshot),
                output: &output(),
                pending: &[]
            }),
            Err(StoreError::Context(ContextError::ProtectedGroup))
        ));
        assert!(store.context_receipt(&operation).unwrap().is_none());
    }
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let item = Item(
        json!({"type":"function_call","call_id":"final","name":"finalize","arguments":{"result":"ready"}}),
    );
    store.append_items(&head, &[item.clone()]).unwrap();
    record_finalize_marker(&store, &head, &item);
    // Identical newly appended bytes have a different immutable occurrence.
    store.append_items(&head, &[item]).unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    assert!(matches!(
        store.commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[]
        }),
        Err(StoreError::Context(ContextError::ProtectedGroup))
    ));
}

#[test]
fn forged_finalize_marker_cannot_authenticate_a_schema_invalid_response() {
    let store = Store::memory().unwrap();
    let (head, operation) = setup(&store);
    let item = Item(
        json!({"type":"function_call","call_id":"final","name":"finalize","arguments":{"result":"ready"}}),
    );
    store.append_items(&head, &[item.clone()]).unwrap();
    let event = record_finalize_marker(&store, &head, &item);
    let invalid_schema = crate::finalize::tool_schema::<u32>().unwrap();
    let schema_hash = store.put_item(&Item(invalid_schema.clone())).unwrap();
    let tools_hash = store.put_item(&Item(json!([invalid_schema]))).unwrap();
    store.lock().execute("UPDATE events SET payload=json_set(payload,'$.completion.schema',?2,'$.issued.tools',?3) WHERE id=?1", params![event, schema_hash.0, tools_hash.0]).unwrap();
    let snapshot = store.begin_context(&operation, &head).unwrap();
    assert!(matches!(
        store.commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &edited(&snapshot),
            output: &output(),
            pending: &[]
        }),
        Err(StoreError::InvalidCompletionMarker { .. })
    ));
    assert!(store.context_receipt(&operation).unwrap().is_none());
}
