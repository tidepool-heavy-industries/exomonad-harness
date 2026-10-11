use super::*;
use crate::{
    checkpoint::CheckpointChild,
    embedding::{HostIdentity, RecoveryMode},
    model::{CallId, Effort},
    store::provider_response::{IssuedProviderRequest, ProviderResponseState},
    transport::{ResponsesProtocol, ResponsesRequest, ResponsesRoute, sse::{ResponseAssembly, StreamEvent}},
};

fn target(model: &str) -> ContextProjectionTarget {
    ContextProjectionTarget { model: model.into(), route: ResponsesRoute::Codex, protocol: ResponsesProtocol::Standard }
}

fn request(model: &str) -> ResponsesRequest {
    ResponsesRequest { input: vec![], instructions: "projection test".into(), tools: vec![].into(), tools_allowed: None,
        model: model.into(), pinned_effort: Effort::Low, session_id: "projection".into() }
}

fn issue(store: &Store, head: &RequestId, model: &str) -> IssuedProviderRequest {
    store.issue_provider_request(head, store.seal_replay_request(&request(model)).unwrap(),
        (ResponsesRoute::Codex, ResponsesProtocol::Standard)).unwrap()
}

fn done(store: &Store, issued: &IssuedProviderRequest, index: u64, item: Item) {
    let raw = json!({"type":"response.output_item.done", "output_index":index, "item":item.0}).to_string();
    let Some(StreamEvent::CompletedItem(done)) = ResponseAssembly::default().accept(&raw).unwrap() else { panic!("completed item") };
    store.append_provider_done(issued, &done).unwrap();
}

fn reasoning(summary: &str) -> Item {
    Item(json!({"type":"reasoning", "encrypted_content":"retained secret", "summary":[{"type":"summary_text","text":summary}]}))
}

fn capture_call() -> Item {
    Item(json!({"type":"custom_tool_call","call_id":"capture","name":"haskell_sync","input":"checkpoint"}))
}

#[test]
fn interrupted_done_items_keep_same_model_replay_and_require_exact_transport() {
    let store = Store::memory().unwrap();
    let head = RequestId("interrupted-items".into());
    let actor = AgentPath("/root".into());
    store.create_request(&head, None, &actor.0).unwrap();
    let identity = store.standalone_identity(actor);
    store.initialize_context_target(&identity, &target("astra")).unwrap();
    let issued = issue(&store, &head, "astra");
    done(&store, &issued, 0, reasoning("visible reasoning"));
    store.stop_provider_response(&head, ProviderResponseState::Interrupted).unwrap();
    assert_eq!(store.context_request_state(&head, &identity).unwrap().history[0].2, reasoning("visible reasoning"));
    let incompatible = ContextProjectionTarget { route: ResponsesRoute::ChatGptPlan, ..target("astra") };
    assert!(matches!(store.preflight_context_target(&head, &identity, &incompatible),
        Err(StoreError::Context(ContextError::Replay(ContextReplayRejection::IncompatibleTransport)))));
    assert!(store.replay_turns(&head).unwrap().is_empty());
}

#[test]
fn equal_item_bytes_have_distinct_issued_positions_and_portable_sources() {
    let store = Store::memory().unwrap();
    let head = RequestId("equal-issued-items".into());
    store.create_request(&head, None, "/root").unwrap();
    let issued = issue(&store, &head, "astra");
    let equal = reasoning("equal visible reasoning");
    done(&store, &issued, 1, equal.clone());
    assert!(matches!(store.preflight_context_target(&head, &store.standalone_identity(AgentPath("/root".into())), &target("luna")),
        Err(StoreError::IncompleteProviderPrefix { index: 0, .. })));
    done(&store, &issued, 0, equal.clone());
    let retained = store.context_history(&head).unwrap();
    assert_eq!(retained.len(), 2);
    assert_eq!(retained[0].1, retained[1].1);
    let c = store.lock().unwrap();
    let canonical = history(&c, &head, true).unwrap();
    assert_ne!(canonical[0].origin, canonical[1].origin);
    let portable = checked_projection(&c, &head, &canonical, Some("luna"), None,
        Some((ResponsesRoute::Codex, ResponsesProtocol::Standard))).unwrap().into_items();
    assert_eq!(portable.len(), 1);
    let note = portable[0].2.0["content"].as_str().unwrap();
    assert!(!note.contains("retained secret"));
    assert_eq!(note.matches("equal visible reasoning").count(), 2);
    drop(c);
    assert_eq!(store.context_history(&head).unwrap(), retained);
}

#[test]
fn later_gap_completion_cannot_repair_an_old_captured_cut() {
    let store = Store::memory().unwrap();
    let head = RequestId("capture-gap".into());
    let root = AgentPath("/root".into());
    store.create_request(&head, None, &root.0).unwrap();
    store.set_effort(&head, Effort::Low).unwrap();
    store.admit_agent(&root, None, Some(&head), &json!({}), &json!({})).unwrap();
    let issued = issue(&store, &head, "astra");
    done(&store, &issued, 1, capture_call());
    let operation = store.claim(&CallId("capture".into()), &head).unwrap();
    let old = store.capture_checkpoint_cuts(&operation, &json!({}), std::sync::Arc::new(())).unwrap();
    let frozen = store.context_history(old.before_call().snapshot_request()).unwrap();
    done(&store, &issued, 0, reasoning("completed later"));
    let path = AgentPath("/root/old".into());
    assert!(matches!(store.attach_checkpoint_child(old.before_call(), CheckpointChild {
        path:&path, parent:&root, contract:&json!({}), checkout:&json!({}), task:None, target:&target("luna"),
    }), Err(StoreError::Context(ContextError::Replay(ContextReplayRejection::InvalidPrefix)))));
    assert!(store.agent(&path).unwrap().is_none());
    assert_eq!(store.context_history(old.before_call().snapshot_request()).unwrap(), frozen);
    let fresh = store.capture_checkpoint_cuts(&operation, &json!({}), std::sync::Arc::new(())).unwrap();
    let path = AgentPath("/root/fresh".into());
    let (child, _) = store.attach_checkpoint_child(fresh.before_call(), CheckpointChild {
        path:&path, parent:&root, contract:&json!({}), checkout:&json!({}), task:None, target:&target("luna"),
    }).unwrap();
    let identity = store.standalone_identity(path);
    let rendered = serde_json::to_string(&store.context_request_state(child.head_request.as_ref().unwrap(), &identity).unwrap().history).unwrap();
    assert!(rendered.contains("completed later"));
    assert!(!rendered.contains("retained secret"));
    assert!(!rendered.contains("haskell_sync"));
}

#[test]
fn retained_reflection_survives_unproven_opaque_history() {
    let store = Store::memory().unwrap();
    let host = HostIdentity { run:"reflection".into(), actor:AgentPath("/root".into()), incarnation:"first".into() };
    store.bind_embedded_actor(&host, None).unwrap();
    let identity = ConversationIdentity::Embedded { run:host.run.clone(), actor:host.actor.clone(), incarnation:host.incarnation.clone() };
    store.initialize_context_target(&identity, &target("astra")).unwrap();
    let head = RequestId("legacy-opaque".into());
    store.write_embedded_request(&host, &head, None, &[reasoning("retained visible summary")], crate::store::Usage::default()).unwrap();
    assert!(matches!(store.context_request_state(&head, &identity), Err(StoreError::Context(ContextError::Replay(ContextReplayRejection::MissingProvenance { .. })))));
    let observed = store.embedded_conversation_history(&host).unwrap();
    assert_eq!(observed.head, Some(head));
    assert_eq!(observed.history[0].2, reasoning("retained visible summary"));
}

#[test]
fn visible_recovery_preserves_pending_operation_and_original_opaque_bytes() {
    let store = Store::memory().unwrap();
    let host = HostIdentity { run:"visible-recovery".into(), actor:AgentPath("/root".into()), incarnation:"first".into() };
    store.bind_embedded_actor(&host, None).unwrap();
    let identity = ConversationIdentity::Embedded { run:host.run.clone(), actor:host.actor.clone(), incarnation:host.incarnation.clone() };
    store.initialize_context_target(&identity, &target("astra")).unwrap();
    let head = RequestId("recovery-source".into());
    store.write_embedded_request(&host, &head, None, &[], crate::store::Usage::default()).unwrap();
    let issued = issue(&store, &head, "astra");
    done(&store, &issued, 0, reasoning("portable summary"));
    done(&store, &issued, 1, capture_call());
    let operation = store.claim(&CallId("capture".into()), &head).unwrap();
    let originals = store.context_history(&head).unwrap();
    let mut connection = store.lock().unwrap();
    let (replacement, receipt) = connection.write_transaction(|tx| publish_recovery_projection(tx, &host, &head, RecoveryMode::VisibleHistory, &store.store_id)).unwrap();
    drop(connection);
    assert_eq!(receipt.omitted.len(), 1);
    assert_eq!(receipt.unknown_response_completeness, vec![head.clone()]);
    assert_eq!(store.context_history(&head).unwrap(), originals);
    let projected = store.context_request_state(&replacement, &identity).unwrap();
    let serialized = serde_json::to_string(&projected.history).unwrap();
    assert!(serialized.contains("portable summary"));
    assert!(!serialized.contains("retained secret"));
    assert_eq!(projected.history.iter().filter(|(_, _, item)| item.tool_call().unwrap().is_some()).count(), 1);
    assert!(store.claims_for_operation(&operation).unwrap().iter().any(|claim| claim.request == replacement && claim.state == crate::store::ClaimState::Pending));
}

#[test]
fn committed_native_prefix_retains_exact_origins_and_rejects_corrupted_restore_provenance() {
    let store = Store::memory().unwrap();
    let root = AgentPath("/root".into());
    let identity = store.standalone_identity(root);
    store.initialize_context_target(&identity, &target("astra")).unwrap();
    let first = RequestId("native-first".into());
    store.create_request(&first, None, "/root").unwrap();
    store.set_effort(&first, Effort::Low).unwrap();
    let turn = crate::transport::ResponsesTurn { response_id:"native-response".into(), items:vec![reasoning("native visible finding")], usage:Default::default() };
    store.record_replay_turn_for_target(&first, &request("astra"), &turn, ResponsesRoute::Codex, ResponsesProtocol::Standard).unwrap();
    let edit = RequestId("native-edit".into());
    store.create_request(&edit, Some(&first), "/root").unwrap();
    let turn = crate::transport::ResponsesTurn { response_id:"edit-response".into(), items:vec![capture_call()], usage:Default::default() };
    store.record_replay_turn_for_target(&edit, &request("astra"), &turn, ResponsesRoute::Codex, ResponsesProtocol::Standard).unwrap();
    let operation = store.claim(&CallId("capture".into()), &edit).unwrap();
    let snapshot = store.begin_context(&operation, &edit).unwrap();
    store.commit_context(ContextCommit { snapshot:&snapshot, draft:&ContextDraft {
        document:snapshot.document.clone(), next_model:None, next_effort:Some(Effort::High),
    }, output:&JobOutput::Completed(Ok(json!("completed"))), pending:&[] }).unwrap();
    let evidence = store.context_commit_evidence(&operation).unwrap().unwrap();
    assert_eq!(evidence.version, 4);
    let origins = evidence.prefix_origins.unwrap();
    assert!(origins.iter().any(|origin| origin.request == first));
    let c = store.lock().unwrap();
    let (event, raw): (i64,String) = c.query_row("SELECT id,payload FROM events WHERE kind='context_commit' ORDER BY id DESC LIMIT 1", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    let mut record: serde_json::Value = serde_json::from_str(&raw).unwrap();
    record["prefix"][0]["origin"]["request"] = json!("unrelated-equal-bytes");
    c.execute("UPDATE events SET payload=?2 WHERE id=?1", params![event,record.to_string()]).unwrap();
    drop(c);
    assert!(matches!(store.context_commit_evidence(&operation), Err(StoreError::Context(ContextError::UnsupportedState))));
}

#[test]
fn exact_recovery_refuses_unowned_invocations_and_visible_mode_cites_the_loss() {
    let store = Store::memory().unwrap();
    let host = HostIdentity {
        run: "unowned-invocation".into(),
        actor: AgentPath("/root".into()),
        incarnation: "first".into(),
    };
    store.bind_embedded_actor(&host, None).unwrap();
    let identity = ConversationIdentity::Embedded {
        run: host.run.clone(),
        actor: host.actor.clone(),
        incarnation: host.incarnation.clone(),
    };
    store
        .initialize_context_target(&identity, &target("astra"))
        .unwrap();
    let head = RequestId("unclaimed-source".into());
    store
        .write_embedded_request(
            &host,
            &head,
            None,
            &[capture_call()],
            crate::store::Usage::default(),
        )
        .unwrap();
    let originals = store.context_history(&head).unwrap();
    let mut c = store.lock().unwrap();
    assert!(matches!(
        c.write_transaction(|tx| publish_recovery_projection(
            tx,
            &host,
            &head,
            RecoveryMode::Exact,
            &store.store_id
        )),
        Err(StoreError::Context(ContextError::Replay(
            ContextReplayRejection::UnownedInvocation { .. }
        )))
    ));
    let (replacement, receipt) = c
        .write_transaction(|tx| {
            publish_recovery_projection(
                tx,
                &host,
                &head,
                RecoveryMode::VisibleHistory,
                &store.store_id,
            )
        })
        .unwrap();
    drop(c);
    assert_eq!(receipt.omitted.len(), 1);
    assert_eq!(store.context_history(&head).unwrap(), originals);
    let rendered = store
        .context_request_state(&replacement, &identity)
        .unwrap();
    assert!(
        rendered
            .history
            .iter()
            .all(|(_, _, item)| item.tool_call().unwrap().is_none())
    );
    assert!(
        rendered.history[0].2.0["content"]
            .as_str()
            .unwrap()
            .contains("checkpoint")
    );
    assert!(store.claims(&CallId("capture".into())).unwrap().is_empty());
}
