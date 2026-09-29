//! Offline restart and atomic-publication tests.
use super::*;

fn temp_store_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "harness-{label}-{}-{}.db",
        std::process::id(),
        utc_millis()
    ))
}

fn request_id(s: &str) -> RequestId {
    RequestId(s.to_owned())
}

fn recovery_item(value: serde_json::Value) -> Item {
    Item(value)
}

fn admit_pair(store: &Store) -> (AgentPath, AgentPath) {
    let root = AgentPath("/root".into());
    let child = AgentPath("/root/child".into());
    store
        .admit_agent(
            &root,
            None,
            None,
            &serde_json::json!({}),
            &serde_json::json!({}),
        )
        .unwrap();
    store
        .admit_agent(
            &child,
            Some(&root),
            None,
            &serde_json::json!({}),
            &serde_json::json!({}),
        )
        .unwrap();
    (root, child)
}

#[test]
fn durable_completion_is_discovered_once_after_reopen() {
    let path = temp_store_path("completion-recovery");
    let root = AgentPath("/root".into());
    let child = AgentPath("/root/child".into());
    let current = request_id("durable-final");
    let answer = recovery_item(serde_json::json!({
        "type": "message",
        "role": "assistant",
        "content": [{"type":"output_text","text":"typed answer"}],
        "custom_typed_field": {"version": 1}
    }));

    {
        let store = Store::open(&path).unwrap();
        let (actual_root, actual_child) = admit_pair(&store);
        assert_eq!(actual_root, root);
        assert_eq!(actual_child, child);
        store.create_request(&current, None, &child.0).unwrap();
        assert!(matches!(
            store
                .complete_agent_with_publication(&child, None, &current, Some((&root, &answer)))
                .unwrap(),
            CompletionCommit::Committed {
                envelope_id: Some(_)
            }
        ));
    }

    {
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.agent(&child).unwrap().unwrap().head_request,
            Some(current.clone())
        );
        let recovered = store.unread(&root.0).unwrap();
        assert_eq!(
            recovered.len(),
            1,
            "the durable answer is discoverable after restart"
        );
        assert_eq!(recovered[0].sender, child.0);
        assert_eq!(
            store.get_item(&recovered[0].item_hash).unwrap(),
            Some(answer.clone()),
            "the wire-faithful typed answer survives persistence"
        );

        // Retrying completion cannot republish under the advanced head.
        assert_eq!(
            store
                .complete_agent_with_publication(&child, None, &current, Some((&root, &answer)))
                .unwrap(),
            CompletionCommit::HeadMismatch
        );
        assert_eq!(store.inbox(&root.0).unwrap().len(), 1);
        assert_eq!(store.unread(&root.0).unwrap().len(), 1);
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn restart_recovers_pending_claim_but_classifies_interrupted_claim_explicitly() {
    let path = temp_store_path("claim-recovery");
    let request = request_id("request");
    let pending = CallId("pending-call".into());
    let interrupted = CallId("interrupted-call".into());
    {
        let store = Store::open(&path).unwrap();
        store.create_request(&request, None, "/root").unwrap();
        store.claim(&pending, &request).unwrap();
        store.claim(&interrupted, &request).unwrap();
        assert_eq!(store.interrupt_claim(&interrupted, &request).unwrap(), 1);
    }
    {
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.recover_pending().unwrap(),
            vec![PendingCall {
                call_id: pending.clone(),
                request: request.clone()
            }]
        );
        let claims = store.claims_on(&request).unwrap();
        assert_eq!(
            claims
                .iter()
                .find(|claim| claim.call_id == interrupted)
                .unwrap()
                .state,
            ClaimState::Interrupted,
            "interrupted work must be distinguishable, not silently resumed as pending"
        );
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn custom_replay_output_and_claim_identity_survive_reopen() {
    let path = temp_store_path("custom-replay");
    let request = request_id("custom-replay-request");
    let custom_call = CallId("custom-replay-call".into());
    let mismatch_call = CallId("custom-mismatch-call".into());
    let custom_input = "line one\nquotes: \" \\\\ snowman: ☃";
    let custom_output = recovery_item(serde_json::json!({
        "type": "custom_tool_call_output",
        "call_id": custom_call.0,
        "output": "original raw result\nwith \"quotes\" and \\\\ and ☃"
    }));
    {
        let store = Store::open(&path).unwrap();
        store.create_request(&request, None, "/root").unwrap();
        store
            .append_items(
                &request,
                &[
                    recovery_item(serde_json::json!({
                        "type": "custom_tool_call",
                        "call_id": custom_call.0,
                        "name": "cell",
                        "input": custom_input
                    })),
                    recovery_item(serde_json::json!({
                        "type": "custom_tool_call",
                        "call_id": mismatch_call.0,
                        "name": "cell",
                        "input": "mismatched output case"
                    })),
                ],
            )
            .unwrap();
        store.claim(&custom_call, &request).unwrap();
        store.claim(&mismatch_call, &request).unwrap();
        store.write_output(&custom_call, &custom_output).unwrap();
        store
            .write_output(
                &mismatch_call,
                &recovery_item(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": mismatch_call.0,
                    "output": "{}"
                })),
            )
            .unwrap();
    }
    {
        let store = Store::open(&path).unwrap();
        let input = store
            .items(&request)
            .unwrap()
            .into_iter()
            .filter_map(|item| item.tool_call().ok().flatten())
            .find(|call| call.call_id == custom_call)
            .unwrap()
            .input;
        assert!(
            matches!(&input, crate::item::ToolInput::Custom(value) if value == custom_input),
            "raw custom input survives reopening without normalization"
        );
        let claims = store.claims(&custom_call).unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].request, request);
        assert_eq!(claims[0].state, ClaimState::Settled);
        assert_eq!(
            store.replay_output(&custom_call).unwrap(),
            Some(custom_output),
            "replay preserves the exact custom output Item after reopening"
        );
        assert!(matches!(
            store.replay_output(&mismatch_call),
            Err(StoreError::ReplayOutputKindMismatch {
                expected: ToolKind::Custom,
                ..
            })
        ));
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn replay_does_not_cross_from_child_claim_to_ancestor_tool_call() {
    let path = temp_store_path("replay-lineage");
    let root = request_id("lineage-root");
    let child = request_id("lineage-child");
    let call = CallId("lineage-call".into());
    let output = recovery_item(serde_json::json!({
        "type": "custom_tool_call_output",
        "call_id": call.0,
        "output": "settled on child claim"
    }));
    {
        let store = Store::open(&path).unwrap();
        store.create_request(&root, None, "/root").unwrap();
        store
            .create_request(&child, Some(&root), "/root/worker")
            .unwrap();
        store
            .append_items(
                &root,
                &[recovery_item(serde_json::json!({
                    "type": "custom_tool_call",
                    "call_id": call.0,
                    "name": "cell",
                    "input": "ancestor-only evidence"
                }))],
            )
            .unwrap();
        // The child has a claim but no call Item of its own. Its durable
        // output must not acquire the ancestor's call identity by lineage.
        store.claim(&call, &child).unwrap();
        store.write_output(&call, &output).unwrap();
    }
    {
        let store = Store::open(&path).unwrap();
        assert_eq!(store.replay_output(&call).unwrap(), None);
        assert_eq!(store.claims(&call).unwrap()[0].request, child);
    }
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}

#[test]
fn replay_rejects_duplicate_call_items_in_claim_request() {
    let store = Store::memory().unwrap();
    let request = request_id("duplicate-call-request");
    let call = CallId("duplicate-call".into());
    store.create_request(&request, None, "/root").unwrap();
    store
        .append_items(
            &request,
            &[
                recovery_item(serde_json::json!({
                    "type":"custom_tool_call","call_id":call.0,"name":"cell","input":"first"
                })),
                recovery_item(serde_json::json!({
                    "type":"custom_tool_call","call_id":call.0,"name":"cell","input":"second"
                })),
            ],
        )
        .unwrap();
    store.claim(&call, &request).unwrap();
    store
        .write_output(
            &call,
            &recovery_item(serde_json::json!({
                "type":"custom_tool_call_output","call_id":call.0,"output":"ambiguous"
            })),
        )
        .unwrap();
    assert!(
        store.replay_output(&call).is_err(),
        "duplicate persisted invocation Items cannot be resolved by first-match order"
    );
}

#[test]
fn replay_rejects_ambiguous_claims_across_agent_branches() {
    let store = Store::memory().unwrap();
    let root = request_id("branch-root");
    let left = request_id("branch-left");
    let right = request_id("branch-right");
    let call = CallId("same-call-id".into());
    store.create_request(&root, None, "/root").unwrap();
    store
        .create_request(&left, Some(&root), "/root/left")
        .unwrap();
    store
        .create_request(&right, Some(&root), "/root/right")
        .unwrap();
    for (request, input) in [(&left, "left"), (&right, "right")] {
        store
            .append_items(
                request,
                &[recovery_item(serde_json::json!({
                    "type":"custom_tool_call","call_id":call.0,"name":"cell","input":input
                }))],
            )
            .unwrap();
        store.claim(&call, request).unwrap();
    }
    store
        .write_output(
            &call,
            &recovery_item(serde_json::json!({
                "type":"custom_tool_call_output","call_id":call.0,"output":"result"
            })),
        )
        .unwrap();
    assert!(
        store.replay_output(&call).is_err(),
        "unscoped replay must not silently select the first agent's invocation"
    );
    assert_eq!(
        store
            .replay_output_for_request(&left, &call)
            .unwrap()
            .unwrap()
            .0["output"],
        "result"
    );
    assert_eq!(
        store
            .replay_output_for_request(&right, &call)
            .unwrap()
            .unwrap()
            .0["output"],
        "result"
    );
    assert_eq!(store.replay_output_for_request(&root, &call).unwrap(), None);
}

#[test]
fn wave18_reopen_interrupts_pending_call_once() {
    let path = temp_store_path("orphaned-claim");
    let request = request_id("orphaned-request");
    let call = CallId("orphaned-call".into());
    let output = recovery_item(serde_json::json!({"result":"must not be successful"}));
    {
        let store = Store::open(&path).unwrap();
        store.create_request(&request, None, "/root").unwrap();
        store.claim(&call, &request).unwrap();
    }
    {
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.recover_pending().unwrap(),
            vec![PendingCall {
                call_id: call.clone(),
                request: request.clone(),
            }],
            "read-only recovery discovers the orphaned call"
        );
        // The runtime has classified the orphan as UnknownCall; persist its
        // terminal interruption rather than treating the claim as a success.
        assert_eq!(store.interrupt_claim(&call, &request).unwrap(), 1);
        let claims = store.claims_on(&request).unwrap();
        assert_eq!(claims.len(), 1, "the orphaned claim remains recorded");
        assert_eq!(claims[0].call_id, call);
        assert_eq!(claims[0].state, ClaimState::Interrupted);
        assert_eq!(claims[0].output, None, "interruption is not success");
    }
    {
        let store = Store::open(&path).unwrap();
        // Once persisted, the interrupted claim is neither rediscovered nor
        // transitioned a second time.
        assert!(store.recover_pending().unwrap().is_empty());
        assert_eq!(store.interrupt_claim(&call, &request).unwrap(), 0);
        assert_eq!(store.settle_claims(&call, &output).unwrap(), 0);
        let claims = store.claims_on(&request).unwrap();
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].state, ClaimState::Interrupted);
        assert_eq!(claims[0].output, None);
    }
    let _ = std::fs::remove_file(&path);
}
