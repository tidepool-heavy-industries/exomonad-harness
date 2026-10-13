use super::*;
use crate::{invocation::HookAnnotation, item::ToolKind, model::CallId, store::Usage};

#[test]
fn pruning_requires_the_exact_durable_operation_and_preserves_native_ownership() {
    let store = Store::memory().unwrap();
    let mut originals = Vec::new();
    for label in ["first", "second"] {
        let request = RequestId(label.into());
        let call = CallId("reused".into());
        store
            .write_request(
                &request,
                None,
                "/root",
                &[Item(json!({
                    "type":"function_call", "call_id":call.0, "name":"read", "arguments":"{}"
                }))],
                Usage::default(),
            )
            .unwrap();
        let operation = store.claim(&call, &request).unwrap();
        store
            .write_job_output(
                &operation,
                ToolKind::Function,
                &JobOutput::Completed(Ok(json!({"full":"same original"}))),
            )
            .unwrap();
        store
            .append_operation_output(&operation, &request, &request)
            .unwrap();
        let occurrence = history(&store.lock().unwrap(), &request, true)
            .unwrap()
            .into_iter()
            .find(|item| item.output_operation.as_ref() == Some(&operation))
            .unwrap();
        originals.push((operation, occurrence));
    }
    let (operation, occurrence) = &originals[0];
    let mut candidate = crate::provider::ModelVisibleItemCandidate {
        item: Item::tool_output(
            &operation.call,
            ToolKind::Function,
            &JobOutput::Completed(Ok(json!("short"))),
        ),
        result_event: -1,
        annotation_event: -1,
    };
    assert!(matches!(
        store.qualify_model_visible_item(occurrence, &candidate),
        Err(StoreError::Context(ContextError::Replay(
            ContextReplayRejection::InvalidMembership
        )))
    ));

    let foreign = store
        .record_event(
            Some(&originals[1].0.request),
            "model_tool_result",
            &json!({"operation":originals[1].0,"output":originals[1].1.item}),
        )
        .unwrap();
    candidate.result_event = foreign;
    candidate.annotation_event = store
        .record_event(
            Some(&operation.request),
            "model_tool_annotation",
            &json!({"ordinal":foreign,"annotation":HookAnnotation::Pruned {
                replacement:json!("short"), annotation:"retain full output".into()
            }}),
        )
        .unwrap();
    assert!(matches!(
        store.qualify_model_visible_item(occurrence, &candidate),
        Err(StoreError::Context(ContextError::Replay(
            ContextReplayRejection::InvalidMembership
        )))
    ));

    let result = store
        .record_event(
            Some(&operation.request),
            "model_tool_result",
            &json!({"operation":operation,"output":occurrence.item}),
        )
        .unwrap();
    candidate.result_event = result;
    candidate.annotation_event = store
        .record_event(
            Some(&operation.request),
            "model_tool_annotation",
            &json!({"ordinal":result,"annotation":HookAnnotation::Pruned {
                replacement:json!("short"), annotation:"retain full output".into()
            }}),
        )
        .unwrap();
    let (rendered, qualified) = store
        .qualify_model_visible_item(occurrence, &candidate)
        .unwrap()
        .into_parts();
    assert_eq!(rendered, candidate.item);
    assert_eq!(qualified.origin, occurrence.origin);
    assert_eq!(qualified.hash, occurrence.hash);
    assert_eq!(qualified.output_operation, occurrence.output_operation);
    assert_eq!(qualified.item, occurrence.item);
    assert_eq!(
        store.replay_output_operation(operation).unwrap(),
        Some(occurrence.item.clone())
    );

    let canonical = history(&store.lock().unwrap(), &operation.request, true).unwrap();
    let mut selected = canonical.clone();
    *selected
        .iter_mut()
        .find(|item| item.output_operation.as_ref() == Some(operation))
        .unwrap() = qualified.clone();
    let aligned = selected.iter().cloned().map(Some).collect::<Vec<_>>();
    let request = crate::transport::ResponsesRequest {
        input: selected
            .iter()
            .map(project_context_note)
            .collect::<Result<Vec<_>>>()
            .unwrap(),
        instructions: "fixed".into(),
        tools: vec![].into(),
        tools_allowed: None,
        model: "offline".into(),
        pinned_effort: crate::model::Effort::Low,
        session_id: "test".into(),
        prompt_cache_comparison: None,
    };
    let hashes = vec![None; request.input.len()];
    // The raw owner cannot admit an unpersisted overlay, even when its bytes are plausible.
    assert!(matches!(
        store.seal_replay_request_with_occurrences(
            &operation.request,
            &request,
            &hashes,
            None,
            &aligned
        ),
        Err(StoreError::OperationOriginMismatch)
    ));
    let make_projection = || {
        checked_projection(
            &store.lock().unwrap(),
            &operation.request,
            &selected,
            Some(&request.model),
            None,
            Some((
                crate::transport::ResponsesRoute::ChatGptPlan,
                crate::transport::ResponsesProtocol::Standard,
            )),
        )
        .unwrap()
    };
    store
        .seal_replay_request_with_checked_projection(
            &operation.request,
            &request,
            &hashes,
            None,
            &aligned,
            make_projection(),
        )
        .unwrap();
    let foreign_store = Store::memory().unwrap();
    assert!(matches!(
        foreign_store.seal_replay_request_with_checked_projection(
            &operation.request,
            &request,
            &hashes,
            None,
            &aligned,
            make_projection()
        ),
        Err(StoreError::OperationOriginMismatch)
    ));
    assert!(matches!(
        store.seal_replay_request_with_checked_projection(
            &originals[1].0.request,
            &request,
            &hashes,
            None,
            &aligned,
            make_projection()
        ),
        Err(StoreError::OperationOriginMismatch)
    ));
    let mut unrelated = aligned.clone();
    unrelated
        .iter_mut()
        .flatten()
        .find(|item| item.output_operation.as_ref() == Some(operation))
        .unwrap()
        .origin = originals[1].1.origin.clone();
    assert!(
        store
            .seal_replay_request_with_checked_projection(
                &operation.request,
                &request,
                &hashes,
                None,
                &unrelated,
                make_projection()
            )
            .is_err()
    );
    let unchanged = crate::transport::ResponsesRequest {
        input: canonical
            .iter()
            .map(project_context_note)
            .collect::<Result<Vec<_>>>()
            .unwrap(),
        ..request.clone()
    };
    store
        .seal_replay_request_with_occurrences(
            &operation.request,
            &unchanged,
            &hashes,
            None,
            &canonical.iter().cloned().map(Some).collect::<Vec<_>>(),
        )
        .unwrap();

    // An exact selected pair cannot borrow an older valid same-byte annotation.
    let wrong_result = store
        .record_event(
            Some(&operation.request),
            "model_tool_result",
            &json!({"operation":originals[1].0,"output":occurrence.item}),
        )
        .unwrap();
    let wrong_annotation = store
        .record_event(
            Some(&operation.request),
            "model_tool_annotation",
            &json!({"ordinal":wrong_result,"annotation":HookAnnotation::Pruned {
                replacement:json!("short"), annotation:"same visible bytes".into()
            }}),
        )
        .unwrap();
    let mut wrong_selection = candidate.clone();
    wrong_selection.result_event = wrong_result;
    wrong_selection.annotation_event = wrong_annotation;
    assert!(matches!(
        store.qualify_model_visible_item(occurrence, &wrong_selection),
        Err(StoreError::Context(ContextError::Replay(
            ContextReplayRejection::InvalidMembership
        )))
    ));
    assert_eq!(
        store
            .qualify_model_visible_item(occurrence, &candidate)
            .unwrap()
            .into_parts()
            .0,
        candidate.item
    );

    let mut forged = candidate.clone();
    forged.item.0["call_id"] = json!("other");
    assert!(matches!(
        store.qualify_model_visible_item(occurrence, &forged),
        Err(StoreError::Context(ContextError::Replay(
            ContextReplayRejection::InvalidMembership
        )))
    ));
}
