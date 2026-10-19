fn preparation_reason() -> InferenceBlockReason {
    InferenceBlockReason::RequestPreparationRefused {
        route: crate::transport::ResponsesRoute::DeepSeek,
        semantics: crate::transport::UnsupportedSemantics::Message,
    }
}

#[test]
fn preparation_block_preserves_fifo_without_implicit_grants_and_requires_exact_recovery() {
    let store = Store::memory().unwrap();
    let identity = identity();
    ready(&store, &identity);
    let attempt = store.begin_embedded_inference(&identity, None).unwrap();
    store
        .admit_embedded_input(
            &identity,
            "before",
            "operator",
            &crate::item::Item::user_text("before"),
        )
        .unwrap();
    let request = RequestId("preparation-head".into());
    store
        .write_embedded_request(&identity, &request, None, &[], Usage::default())
        .unwrap();
    let block = store
        .block_embedded_inference(&identity, &request, preparation_reason())
        .unwrap();
    assert!(!store.continue_queued_input(&attempt).unwrap());
    store
        .admit_embedded_input(
            &identity,
            "after",
            "operator",
            &crate::item::Item::user_text("after"),
        )
        .unwrap();
    assert_eq!(
        store.embedded_inference_state(&identity).unwrap(),
        HostInferenceState::Blocked {
            block: block.clone()
        }
    );
    let queued = store.unread(&identity.actor.0).unwrap();
    assert_eq!(queued.len(), 2);
    assert!(queued[0].id < queued[1].id);
    for envelope in queued {
        assert_eq!(
            store.embedded_input_continuation(envelope.id).unwrap(),
            Some(crate::embedding::InputContinuation::Queued)
        );
    }
    assert!(
        store
            .reserve_automatic_retry(&identity, &block)
            .unwrap()
            .is_none()
    );
    let mut stale = block.clone();
    stale.generation += 1;
    let operation = recovery(&store, &identity, &stale);
    assert!(matches!(
        store
            .authorize_embedded_recovery(&identity, operation)
            .unwrap()
            .outcome,
        CommandReceiptOutcome::ContinuationRefused { .. }
    ));
    let operation = recovery(&store, &identity, &block);
    assert!(matches!(
        store
            .authorize_embedded_recovery(&identity, operation)
            .unwrap()
            .outcome,
        CommandReceiptOutcome::RecoveryAuthorized { .. }
    ));
    assert!(
        store
            .begin_embedded_inference(&identity, None)
            .unwrap()
            .recovering
    );
    assert_eq!(store.unread(&identity.actor.0).unwrap().len(), 2);
}

#[test]
fn preparation_block_commit_failure_fences_and_never_publishes_block() {
    let store = Store::memory().unwrap();
    let identity = identity();
    ready(&store, &identity);
    let _attempt = store.begin_embedded_inference(&identity, None).unwrap();
    let request = RequestId("commit-failure".into());
    store
        .write_embedded_request(&identity, &request, None, &[], Usage::default())
        .unwrap();
    store.lock().unwrap().execute_batch("CREATE TABLE fault_parent(id INTEGER PRIMARY KEY); CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER preparation_fault AFTER UPDATE ON embedded_inference BEGIN INSERT INTO fault_child VALUES(1); END;").unwrap();
    assert!(matches!(
        store.block_embedded_inference(&identity, &request, preparation_reason()),
        Err(StoreError::RecoveryRequired(_))
    ));
    assert!(matches!(
        store.embedded_inference_state(&identity),
        Err(StoreError::AdmissionFenced)
    ));
}

#[test]
fn preparation_block_refuses_uncertain_effect_cleanup() {
    let store = Store::memory().unwrap();
    let identity = identity();
    ready(&store, &identity);
    let _attempt = store.begin_embedded_inference(&identity, None).unwrap();
    let request = RequestId("uncertain-preparation".into());
    store.write_embedded_request(&identity, &request, None, &[crate::item::Item(serde_json::json!({"type":"function_call","call_id":"uncertain","name":"effect","arguments":"{}"}))], Usage::default()).unwrap();
    let operation = store.claim(&CallId("uncertain".into()), &request).unwrap();
    store
        .interrupt_operation_claim(&operation, &request)
        .unwrap();
    assert!(matches!(
        store.block_embedded_inference(&identity, &request, preparation_reason()),
        Err(StoreError::InferenceRecoveryUncertain { .. })
    ));
    assert_eq!(
        store.embedded_inference_state(&identity).unwrap(),
        HostInferenceState::Running
    );
}
