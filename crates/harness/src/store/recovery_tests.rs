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
