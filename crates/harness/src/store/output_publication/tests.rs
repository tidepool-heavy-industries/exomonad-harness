use super::*;
use crate::{
    context::{ContextBlock, ContextCommit, ContextDraft, ContextRole},
    item::ToolKind,
    model::{AgentPath, CallId},
    store::Usage,
    turn::JobOutput,
};
use serde_json::json;

fn issue(store: &Store, request: &str, parent: Option<&RequestId>, name: &str) -> OperationId {
    let request = RequestId(request.into());
    let call = CallId("reused".into());
    store.write_request(&request, parent, "/root", &[Item(json!({
        "type":"function_call", "call_id":call.0, "name":name, "arguments":"{}", "async":true
    }))], Usage::default()).unwrap();
    store.claim(&call, &request).unwrap()
}

fn retain(store: &Store, operation: &OperationId, value: &str) {
    store
        .write_job_output(
            operation,
            ToolKind::Function,
            &JobOutput::Completed(Ok(json!(value))),
        )
        .unwrap();
}

fn assert_owners(store: &Store, head: &RequestId, expected: &[OperationId]) {
    let outputs = store
        .recovery_history(head)
        .unwrap()
        .into_iter()
        .filter_map(|record| record.output)
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), expected.len());
    for operation in expected {
        let owned = outputs
            .iter()
            .filter(|output| &output.operation == operation)
            .collect::<Vec<_>>();
        assert_eq!(owned.len(), 1);
        assert_eq!(
            owned[0].item,
            store
                .replay_tool_output_operation(operation)
                .unwrap()
                .unwrap()
                .item
        );
    }
}

#[test]
fn exact_output_histories_preserve_equal_hashes_order_and_here_copy() {
    let mut histories = 0;
    // Publication can precede reuse or follow it in either settlement order.
    for placement in 0..3 {
        for equal_payloads in [false, true] {
            for different_names in [false, true] {
                let store = Store::memory().unwrap();
                let first = issue(&store, "first", None, "one");
                retain(&store, &first, "first result");
                if placement == 0 {
                    assert!(
                        store
                            .append_operation_output(&first, &first.request, &first.request)
                            .unwrap()
                            .appended
                    );
                }
                let second = issue(
                    &store,
                    "second",
                    Some(&first.request),
                    if different_names { "two" } else { "one" },
                );
                retain(
                    &store,
                    &second,
                    if equal_payloads {
                        "first result"
                    } else {
                        "second result"
                    },
                );
                let order = if placement == 2 {
                    [&second, &first]
                } else {
                    [&first, &second]
                };
                for operation in order {
                    if operation == &first && placement == 0 {
                        continue;
                    }
                    assert!(
                        store
                            .append_operation_output(operation, &operation.request, &second.request)
                            .unwrap()
                            .appended
                    );
                }
                let expected = [first.clone(), second.clone()];
                assert_owners(&store, &second.request, &expected);
                let history = store.recovery_history(&second.request).unwrap();
                let publications = history
                    .iter()
                    .filter_map(|record| record.output.as_ref().map(|output| (record, output)))
                    .collect::<Vec<_>>();
                assert_eq!(
                    store.put_item(&publications[0].1.item).unwrap()
                        == store.put_item(&publications[1].1.item).unwrap(),
                    equal_payloads
                );
                for (record, output) in publications {
                    let retry = store
                        .append_operation_output(
                            &output.operation,
                            &output.operation.request,
                            &record.request,
                        )
                        .unwrap();
                    assert!(!retry.appended);
                }
                assert_owners(&store, &second.request, &expected);

                let parent = AgentPath("/root".into());
                let child = AgentPath("/root/child".into());
                store
                    .admit_agent(&parent, None, Some(&second.request), &json!({}), &json!({}))
                    .unwrap();
                let copy = RequestId("here".into());
                store
                    .admit_here_agent_with_snapshot(
                        &child,
                        &parent,
                        &copy,
                        &json!({}),
                        "/root",
                        "/root/child",
                        "AtBoundary",
                        &Item(json!({"type":"message","role":"user","content":"continue"})),
                    )
                    .unwrap();
                assert_owners(&store, &copy, &expected);
                let original =
                    super::super::context::history(&store.lock(), &second.request, true).unwrap();
                let copied =
                    super::super::context::request_occurrences(&store.lock(), &copy).unwrap();
                for source in original
                    .iter()
                    .filter(|source| source.output_operation.is_some())
                {
                    let retained = copied
                        .iter()
                        .filter(|copy| copy.origin == source.origin)
                        .collect::<Vec<_>>();
                    assert_eq!(retained.len(), 1);
                    assert_eq!(retained[0].output_operation, source.output_operation);
                    assert_eq!(retained[0].hash, source.hash);
                }
                let compacted = RequestId("compacted-here".into());
                let compact_items = copied
                    .iter()
                    .map(|occurrence| occurrence.item.clone())
                    .collect::<Vec<_>>();
                let selections = copied
                    .iter()
                    .enumerate()
                    .map(|(position, occurrence)| (position, occurrence.clone()))
                    .collect::<Vec<_>>();
                store
                    .write_compaction_request_with_evidence(
                        &compacted,
                        &copy,
                        &child.0,
                        &compact_items,
                        &selections,
                        &copied.iter().cloned().map(Some).collect::<Vec<_>>(),
                        &[],
                        None,
                        None,
                    )
                    .unwrap();
                assert_owners(&store, &compacted, &expected);
                let claims = store.claims_on(&compacted).unwrap();
                assert_eq!(claims.len(), 2);
                assert!(
                    claims
                        .iter()
                        .all(|claim| claim.state == super::super::ClaimState::Settled)
                );
                histories += 1;
            }
        }
    }
    assert_eq!(histories, 12);
    eprintln!(
        "exact_output_histories: histories=12 placements=3 payload_partitions=2 tool_name_partitions=2 publication_retries=24 Here_copies=12 terminal_compaction_copies=12"
    );
}

#[test]
fn output_publication_requires_exact_terminal_claim_and_is_atomic() {
    let store = Store::memory().unwrap();
    let operation = issue(&store, "issued", None, "work");
    let target = RequestId("target".into());
    store
        .create_request(&target, Some(&operation.request), "/root")
        .unwrap();
    let before = store.items(&target).unwrap();
    assert!(matches!(
        store.append_operation_output(&operation, &operation.request, &target),
        Err(StoreError::InvalidOutputPublication { .. })
    ));
    retain(&store, &operation, "terminal");
    let mut foreign = operation.clone();
    foreign.origin = store.standalone_identity(AgentPath("/foreign".into()));
    let mut missing = operation.clone();
    missing.call = CallId("missing".into());
    for (selected, claimant) in [
        (&foreign, &operation.request),
        (&missing, &operation.request),
        (&operation, &target),
    ] {
        assert!(
            store
                .append_operation_output(selected, claimant, &target)
                .is_err()
        );
        assert_eq!(store.items(&target).unwrap(), before);
    }
    store.lock().execute_batch("CREATE TRIGGER refuse_owned_output BEFORE INSERT ON request_items WHEN NEW.request_id='target' BEGIN SELECT RAISE(ABORT,'publication refused'); END;").unwrap();
    assert!(
        store
            .append_operation_output(&operation, &operation.request, &target)
            .is_err()
    );
    assert_eq!(store.items(&target).unwrap(), before);
    store
        .lock()
        .execute_batch("DROP TRIGGER refuse_owned_output;")
        .unwrap();
    assert!(
        store
            .append_operation_output(&operation, &operation.request, &target)
            .unwrap()
            .appended
    );
    assert!(
        !store
            .append_operation_output(&operation, &operation.request, &target)
            .unwrap()
            .appended
    );
    assert_owners(&store, &target, &[operation]);
}

#[test]
fn historical_unbound_outputs_refuse_without_rewriting_and_inconsistent_pairs_refuse() {
    let store = Store::memory().unwrap();
    let operation = issue(&store, "legacy", None, "work");
    retain(&store, &operation, "retained");
    store
        .append_operation_output(&operation, &operation.request, &operation.request)
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE request_items SET output_operation=NULL WHERE request_id=?1 AND position=1",
            [&operation.request.0],
        )
        .unwrap();
    let before = store.items(&operation.request).unwrap();
    assert!(
        matches!(store.recovery_history(&operation.request), Err(StoreError::UnboundOutputPublication { request, position: 1 }) if request == operation.request)
    );
    assert_eq!(store.items(&operation.request).unwrap(), before);
    // The row decoder must reject operation metadata attached to ordinary Items.
    store
        .lock()
        .execute(
            "UPDATE request_items SET output_operation=?2 WHERE request_id=?1 AND position=0",
            params![
                operation.request.0,
                serde_json::to_string(&operation).unwrap()
            ],
        )
        .unwrap();
    assert!(matches!(
        store.recovery_history(&operation.request),
        Err(StoreError::InconsistentOutputPublication { position: 0, .. })
    ));
    assert_eq!(store.items(&operation.request).unwrap(), before);
    // A structurally valid output cannot borrow an ordinary occurrence's source.
    store
        .lock()
        .execute(
            "UPDATE request_items SET output_operation=NULL WHERE request_id=?1 AND position=0",
            [&operation.request.0],
        )
        .unwrap();
    store.lock().execute("UPDATE request_items SET output_operation=?2,source_request=?1,source_position=0 WHERE request_id=?1 AND position=1", params![operation.request.0,serde_json::to_string(&operation).unwrap()]).unwrap();
    assert!(matches!(
        store.context_request_state(&operation.request, &operation.origin),
        Err(StoreError::InconsistentOutputPublication { position: 1, .. })
    ));
    assert_eq!(store.items(&operation.request).unwrap(), before);
}

#[test]
fn schema14_migration_preserves_unbound_output_and_refuses_ownership_guess() {
    let mut connection = Connection::open_in_memory().unwrap();
    super::super::schema::initialize(&mut connection).unwrap();
    connection.execute_batch("ALTER TABLE request_items DROP COLUMN output_operation; UPDATE schema_version SET version=14;").unwrap();
    let item = Item(json!({"type":"function_call_output","call_id":"reused","output":"old"}));
    let hash = ItemHash(
        blake3::hash(&serde_json::to_vec(&item).unwrap())
            .to_hex()
            .to_string(),
    );
    connection
        .execute(
            "INSERT INTO items(hash,json) VALUES (?1,?2)",
            params![hash.0, serde_json::to_string(&item).unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO requests(id,branch,created_at) VALUES('old','/root',0)",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO request_items(request_id,position,item_hash) VALUES('old',0,?1)",
            [&hash.0],
        )
        .unwrap();
    super::super::schema::initialize(&mut connection).unwrap();
    let (raw, owner): (String, Option<String>) = connection.query_row("SELECT i.json,ri.output_operation FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id='old'", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(serde_json::from_str::<Item>(&raw).unwrap(), item);
    assert!(owner.is_none());
    let occurrence =
        super::super::context::request_occurrences(&connection, &RequestId("old".into()))
            .unwrap()
            .pop()
            .unwrap();
    assert!(matches!(
        decode(
            occurrence,
            &RecoveryLedger {
                invocations: HashMap::new(),
                sources: HashMap::new()
            }
        ),
        Err(StoreError::UnboundOutputPublication { position: 0, .. })
    ));
    assert_eq!(
        connection
            .query_row("SELECT version FROM schema_version", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        super::super::schema::VERSION
    );
}

#[test]
fn context_rewrite_carries_distinct_operations_with_equal_output_hashes() {
    let store = Store::memory().unwrap();
    let first = issue(&store, "first", None, "one");
    let second = issue(&store, "second", Some(&first.request), "two");
    for operation in [&first, &second] {
        retain(&store, operation, "identical");
        store
            .append_operation_output(operation, &operation.request, &second.request)
            .unwrap();
    }
    let head = RequestId("edit".into());
    store.write_request(&head, Some(&second.request), "/root", &[Item(json!({
        "type":"custom_tool_call", "call_id":"edit", "name":"haskell_sync", "input":"edit context"
    }))], Usage::default()).unwrap();
    let edit = store.claim(&CallId("edit".into()), &head).unwrap();
    let snapshot = store.begin_context(&edit, &head).unwrap();
    let mut document = snapshot.document.clone();
    document.blocks.insert(
        0,
        ContextBlock::Text {
            reference: None,
            role: ContextRole::User,
            text: "retained note".into(),
            sources: vec![],
        },
    );
    let receipt = store
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                document,
                next_model: None,
                next_effort: None,
            },
            output: &JobOutput::Completed(Ok(json!("edited"))),
            pending: &[],
        })
        .unwrap();
    assert!(receipt.changed);
    assert_ne!(receipt.head, head);
    assert_owners(
        &store,
        &receipt.head,
        &[first.clone(), second.clone(), edit],
    );
    for operation in [first, second] {
        let claims = store.claims_for_operation(&operation).unwrap();
        assert!(claims.iter().any(|claim| claim.request == receipt.head
            && claim.state == super::super::ClaimState::Settled));
    }
}

#[test]
fn item_only_compaction_refuses_equal_hash_operation_selection_atomically() {
    let store = Store::memory().unwrap();
    let first = issue(&store, "first", None, "one");
    let second = issue(&store, "second", Some(&first.request), "two");
    for operation in [&first, &second] {
        retain(&store, operation, "identical");
        store
            .append_operation_output(operation, &operation.request, &second.request)
            .unwrap();
    }
    let before = store.context_history(&second.request).unwrap();
    let full = before
        .iter()
        .map(|(_, _, item)| item.clone())
        .collect::<Vec<_>>();
    for drop_one_output in [false, true] {
        let mut selected = full.clone();
        if drop_one_output {
            selected.pop();
        }
        let target = RequestId(if drop_one_output { "partial" } else { "full" }.into());
        assert!(matches!(
            store.write_compaction_request(&target, &second.request, "/root", &selected),
            Err(StoreError::CompactionOwnership {
                reason: super::super::CompactionOwnershipRefusal::AmbiguousSource(_),
                ..
            })
        ));
        assert!(store.request(&target).unwrap().is_none());
        assert_eq!(store.context_history(&second.request).unwrap(), before);
        assert_owners(&store, &second.request, &[first.clone(), second.clone()]);
    }
}

#[test]
fn generic_writers_refuse_unbound_outputs_without_partial_history() {
    for kind in ["function_call_output", "custom_tool_call_output"] {
        let store = Store::memory().unwrap();
        let head = RequestId("existing".into());
        store.create_request(&head, None, "/root").unwrap();
        let ordinary = Item(json!({"type":"message", "role":"user", "content":"before"}));
        let unbound = Item(json!({"type":kind, "call_id":"unissued", "output":"not authority"}));
        assert!(matches!(
            store.append_items(&head, &[ordinary.clone(), unbound.clone()]),
            Err(StoreError::UnboundOutputPublication { position: 1, .. })
        ));
        assert!(store.items(&head).unwrap().is_empty());
        let fresh = RequestId("fresh".into());
        assert!(matches!(
            store.write_request(
                &fresh,
                Some(&head),
                "/root",
                &[ordinary, unbound.clone()],
                Usage::default()
            ),
            Err(StoreError::UnboundOutputPublication { position: 1, .. })
        ));
        assert!(store.request(&fresh).unwrap().is_none());
        store
            .add_envelope("/outside", "/root", "AtBoundary", &unbound, None)
            .unwrap();
        assert!(matches!(
            store.append_unread_envelopes(&AgentPath("/root".into()), &head),
            Err(StoreError::UnboundOutputPublication { position: 0, .. })
        ));
        assert!(store.items(&head).unwrap().is_empty());
        assert_eq!(store.unread("/root").unwrap().len(), 1);
        let embedded = Store::memory().unwrap();
        let identity = crate::embedding::HostIdentity {
            run: "run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "one".into(),
        };
        embedded.bind_embedded_actor(&identity, None).unwrap();
        assert!(matches!(
            embedded.seed_embedded_context(&identity, "seed", &unbound),
            Err(crate::embedding::EmbeddedError::Store(
                StoreError::UnboundOutputPublication { .. }
            ))
        ));
        assert!(
            embedded
                .embedded_round_frontier(&identity)
                .unwrap()
                .settled_head
                .is_none()
        );
        let request = RequestId("embedded-round".into());
        assert!(matches!(
            embedded.write_embedded_request(
                &identity,
                &request,
                None,
                &[
                    Item(json!({"type":"message","role":"user","content":"new input"})),
                    unbound
                ],
                Usage::default()
            ),
            Err(StoreError::UnboundOutputPublication { position: 1, .. })
        ));
        assert!(embedded.request(&request).unwrap().is_none());
        assert!(
            embedded
                .embedded_round_frontier(&identity)
                .unwrap()
                .pending_head
                .is_none()
        );
    }
}

#[test]
fn compaction_preserves_exact_pending_occurrence_and_other_equal_wire_id() {
    let store = Store::memory().unwrap();
    let first = issue(&store, "first", None, "first_tool");
    let second = issue(&store, "second", Some(&first.request), "second_tool");
    retain(&store, &first, "first result");
    store
        .append_operation_output(&first, &first.request, &second.request)
        .unwrap();
    let issued = store.history_occurrences(&second.request).unwrap();
    let pending = issued
        .iter()
        .find(|occurrence| {
            occurrence.origin.request == second.request
                && occurrence.item.tool_call().ok().flatten().is_some()
        })
        .unwrap()
        .clone();
    let first_call = issued
        .iter()
        .find(|occurrence| occurrence.origin.request == first.request)
        .unwrap()
        .item
        .clone();
    let output = issued
        .iter()
        .find(|occurrence| occurrence.output_operation.as_ref() == Some(&first))
        .unwrap()
        .item
        .clone();
    let target = RequestId("compacted".into());
    // The raw echo is omitted only because this exact source is explicitly
    // retained. The earlier invocation with the same wire ID remains visible.
    store
        .write_compaction_request_with_evidence(
            &target,
            &second.request,
            "/root",
            &[
                first_call.clone(),
                output.clone(),
                pending.item.clone(),
                pending.item.clone(),
            ],
            &[(3, pending.clone())],
            &issued.iter().cloned().map(Some).collect::<Vec<_>>(),
            std::slice::from_ref(&second),
            None,
            None,
        )
        .unwrap();
    let copied = store.history_occurrences(&target).unwrap();
    assert_eq!(copied.len(), 3);
    assert_eq!(copied[0].item, first_call);
    assert_eq!(copied[1].output_operation.as_ref(), Some(&first));
    assert_eq!(copied[2].origin, pending.origin);
    assert_owners(&store, &target, std::slice::from_ref(&first));
    let claims = store.claims_on(&target).unwrap();
    assert_eq!(claims.len(), 2);
    assert_eq!(
        claims
            .iter()
            .find(|claim| claim.operation == first)
            .unwrap()
            .state,
        super::super::ClaimState::Settled
    );
    assert_eq!(
        claims
            .iter()
            .find(|claim| claim.operation == second)
            .unwrap()
            .state,
        super::super::ClaimState::Pending
    );

    // Settlement can race with the external request. The transaction copies
    // the actual terminal state, rather than reissuing a pending claim.
    let race_issued = store.history_occurrences(&target).unwrap();
    let race_pending = race_issued
        .iter()
        .find(|occurrence| occurrence.origin == pending.origin)
        .unwrap()
        .clone();
    retain(&store, &second, "late second result");
    let race = RequestId("settled-before-copy".into());
    store
        .write_compaction_request_with_evidence(
            &race,
            &target,
            "/root",
            std::slice::from_ref(&race_pending.item),
            &[(0, race_pending.clone())],
            &race_issued.iter().cloned().map(Some).collect::<Vec<_>>(),
            std::slice::from_ref(&second),
            None,
            None,
        )
        .unwrap();
    let claims = store.claims_on(&race).unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].operation, second);
    assert_eq!(claims[0].state, super::super::ClaimState::Settled);
    store
        .append_operation_output(&second, &race, &race)
        .unwrap();
    assert_owners(&store, &race, &[second]);
}

#[test]
fn raw_compaction_refuses_unowned_tool_items_without_partial_boundary() {
    for item in [
        Item(
            json!({"type":"function_call","call_id":"reused","name":"rewritten","arguments":"{}"}),
        ),
        Item(json!({"type":"function_call_output","call_id":"reused","output":"foreign"})),
    ] {
        let store = Store::memory().unwrap();
        let operation = issue(&store, "source", None, "original");
        let issued = store.history_occurrences(&operation.request).unwrap();
        let target = RequestId("refused".into());
        assert!(matches!(
            store.write_compaction_request_with_evidence(
                &target,
                &operation.request,
                "/root",
                &[
                    Item(json!({"type":"message","role":"assistant","content":"new summary"})),
                    item
                ],
                &[],
                &issued.iter().cloned().map(Some).collect::<Vec<_>>(),
                &[],
                None,
                None
            ),
            Err(StoreError::CompactionOwnership {
                reason: super::super::CompactionOwnershipRefusal::UnownedToolItem,
                ..
            })
        ));
        assert!(store.request(&target).unwrap().is_none());
        assert_eq!(
            store.history_occurrences(&operation.request).unwrap(),
            issued
        );
        assert_eq!(
            store.claims_for_operation(&operation).unwrap()[0].state,
            super::super::ClaimState::Pending
        );
    }
}

#[test]
fn checkpoint_retains_equal_hash_output_owners_and_only_unpublished_claim() {
    let store = Store::memory().unwrap();
    let first = issue(&store, "first", None, "one");
    let second = issue(&store, "second", Some(&first.request), "two");
    for operation in [&first, &second] {
        retain(&store, operation, "identical");
        store
            .append_operation_output(operation, &operation.request, &second.request)
            .unwrap();
    }
    store
        .set_effort(&second.request, crate::model::Effort::Low)
        .unwrap();
    let boundary = RequestId("boundary".into());
    let call = CallId("checkpoint".into());
    store
        .write_request(
            &boundary,
            Some(&second.request),
            "/root",
            &[Item(
                json!({"type":"function_call","call_id":call.0,"name":"capture","arguments":"{}"}),
            )],
            Usage::default(),
        )
        .unwrap();
    let operation = store.claim(&call, &boundary).unwrap();
    let agent = AgentPath("/root".into());
    store
        .admit_agent(&agent, None, Some(&boundary), &json!({}), &json!({}))
        .unwrap();
    let checkpoint = store
        .capture_checkpoint(
            &agent,
            &boundary,
            &call,
            &json!({}),
            std::sync::Arc::new(()),
        )
        .unwrap();
    assert_owners(
        &store,
        checkpoint.snapshot_request(),
        &[first.clone(), second.clone()],
    );
    let copied = store
        .history_occurrences(checkpoint.snapshot_request())
        .unwrap();
    let outputs = copied
        .iter()
        .filter(|occurrence| occurrence.output_operation.is_some())
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), 2);
    assert_eq!(outputs[0].hash, outputs[1].hash);
    assert_ne!(outputs[0].output_operation, outputs[1].output_operation);
    let claims = store.claims_on(checkpoint.snapshot_request()).unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].operation, operation);
}

#[test]
fn here_copy_carries_nearest_exact_terminal_claim_without_reviving_it() {
    for interrupted in [false, true] {
        let store = Store::memory().unwrap();
        let operation = issue(&store, "original", None, "work");
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        let grandchild = AgentPath("/root/child/grandchild".into());
        store
            .admit_agent(
                &root,
                None,
                Some(&operation.request),
                &json!({}),
                &json!({}),
            )
            .unwrap();
        let child_head = RequestId("child-copy".into());
        let grandchild_head = RequestId("grandchild-copy".into());
        let task = Item(json!({"type":"message","role":"user","content":"continue"}));
        store
            .admit_here_agent_with_snapshot(
                &child,
                &root,
                &child_head,
                &json!({}),
                &root.0,
                &child.0,
                "AtBoundary",
                &task,
            )
            .unwrap();
        if interrupted {
            store
                .interrupt_operation_claim(&operation, &child_head)
                .unwrap();
        } else {
            retain(&store, &operation, "settled before second copy");
        }
        store
            .admit_here_agent_with_snapshot(
                &grandchild,
                &child,
                &grandchild_head,
                &json!({}),
                &child.0,
                &grandchild.0,
                "AtBoundary",
                &task,
            )
            .unwrap();
        let expected = if interrupted {
            super::super::ClaimState::Interrupted
        } else {
            super::super::ClaimState::Settled
        };
        let claims = store.claims_on(&grandchild_head).unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].operation, operation);
        assert_eq!(claims[0].state, expected);
        let canonical = store
            .replay_tool_output_claim(&operation, &grandchild_head)
            .unwrap()
            .unwrap();
        store
            .append_operation_output(&operation, &grandchild_head, &grandchild_head)
            .unwrap();
        let publications = store
            .recovery_history(&grandchild_head)
            .unwrap()
            .into_iter()
            .filter_map(|record| record.output)
            .collect::<Vec<_>>();
        assert_eq!(publications.len(), 1);
        assert_eq!(publications[0].operation, operation);
        assert_eq!(publications[0].item, canonical.item);
        if interrupted {
            assert_eq!(
                store.claims_on(&operation.request).unwrap()[0].state,
                super::super::ClaimState::Pending
            );
        }
    }
}
