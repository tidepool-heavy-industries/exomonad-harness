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
