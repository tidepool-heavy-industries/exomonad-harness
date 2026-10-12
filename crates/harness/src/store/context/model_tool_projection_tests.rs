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
    let candidate = Item::tool_output(
        &operation.call,
        ToolKind::Function,
        &JobOutput::Completed(Ok(json!("short"))),
    );
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
    store
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
    store
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
    assert_eq!(rendered, candidate);
    assert_eq!(qualified.origin, occurrence.origin);
    assert_eq!(qualified.hash, occurrence.hash);
    assert_eq!(qualified.output_operation, occurrence.output_operation);
    assert_eq!(qualified.item, occurrence.item);
    assert_eq!(
        store.replay_output_operation(operation).unwrap(),
        Some(occurrence.item.clone())
    );

    let mut forged = candidate;
    forged.0["call_id"] = json!("other");
    assert!(matches!(
        store.qualify_model_visible_item(occurrence, &forged),
        Err(StoreError::Context(ContextError::Replay(
            ContextReplayRejection::InvalidMembership
        )))
    ));
}
