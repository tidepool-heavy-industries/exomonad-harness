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
    let op1 = store.claim(&first, &head).unwrap();
    let op2 = store.claim(&second, &head).unwrap();
    let snap1 = store.begin_context(&op1, &head).unwrap();
    let saved = snap1.document.clone();
    let first_receipt = store
        .commit_context(ContextCommit {
            snapshot: &snap1,
            draft: &ContextDraft {
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
                document: saved,
                next_model: None,
            },
            output: &output(),
            pending: &[],
        })
        .unwrap();
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
