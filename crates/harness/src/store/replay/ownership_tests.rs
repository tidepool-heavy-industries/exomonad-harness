use super::*;
use crate::{item::ToolKind, model::CallId, turn::JobOutput};
use serde_json::json;

fn request(input: Vec<Item>) -> ResponsesRequest {
    ResponsesRequest {
        input,
        instructions: "issued ownership".into(),
        tools: vec![].into(),
        tools_allowed: None,
        model: "test".into(),
        pinned_effort: Effort::Low,
        session_id: "ownership".into(),
    }
}

fn issue(store: &Store, id: &str, parent: Option<&RequestId>, kind: ToolKind) -> OperationId {
    let request = RequestId(id.into());
    store.create_request(&request, parent, "/root").unwrap();
    let call = CallId("reused".into());
    let invocation = Item(match kind {
        ToolKind::Function => {
            json!({"type":"function_call","call_id":call.0,"name":"echo","arguments":"{}"})
        }
        ToolKind::Custom => {
            json!({"type":"custom_tool_call","call_id":call.0,"name":"echo","input":"raw"})
        }
    });
    store.append_items(&request, &[invocation]).unwrap();
    let operation = store.claim(&call, &request).unwrap();
    store
        .write_job_output(
            &operation,
            kind,
            &JobOutput::Completed(Ok(json!("equal result"))),
        )
        .unwrap();
    store
        .append_operation_output(&operation, &request, &request)
        .unwrap();
    operation
}

fn response() -> ResponsesTurn {
    ResponsesTurn {
        response_id: "done".into(),
        items: vec![],
        usage: Usage::default(),
    }
}

// The finite history model retains operation identities assigned by the test.
// It never infers the expected owner from bytes or Store's replay reader.
#[test]
fn issued_output_ownership_survives_projection_injection_and_history_growth() {
    let mut covered = 0;
    for kind in [ToolKind::Function, ToolKind::Custom] {
        for reverse in [false, true] {
            for project in [false, true] {
                for inject_before in [false, true] {
                    for late_count in 0..3 {
                        let store = Store::memory().unwrap();
                        let a = issue(&store, "a", None, kind);
                        let b = issue(&store, "b", Some(&a.request), kind);
                        let mut selected = store
                            .history_occurrences(&b.request)
                            .unwrap()
                            .into_iter()
                            .filter(|occurrence| occurrence.output_operation.is_some())
                            .collect::<Vec<_>>();
                        let mut expected = vec![a.clone(), b.clone()];
                        if reverse {
                            selected.reverse();
                            expected.reverse();
                        }
                        let mut input = selected.iter().map(|o| o.item.clone()).collect::<Vec<_>>();
                        assert_eq!(input[0], input[1]);
                        if project {
                            for item in &mut input {
                                item.0["output"] = json!("projected result");
                            }
                        }
                        let injected = input[0].clone();
                        let mut occurrences =
                            selected.iter().cloned().map(Some).collect::<Vec<_>>();
                        let position = if inject_before { 0 } else { input.len() };
                        input.insert(position, injected);
                        occurrences.insert(position, None);
                        let model_request = request(input);
                        let issued = store
                            .seal_replay_request_with_occurrences(
                                &b.request,
                                &model_request,
                                &vec![None; model_request.input.len()],
                                None,
                                &occurrences,
                            )
                            .unwrap();
                        let mut parent = b.request.clone();
                        for index in 0..late_count {
                            let late = issue(&store, &format!("late-{index}"), Some(&parent), kind);
                            parent = late.request;
                        }
                        // Complete only after equal outputs have enlarged the live lineage.
                        store
                            .record_issued_replay_turn(&b.request, issued, &response())
                            .unwrap();
                        let turn = store.replay_turns(&b.request).unwrap().remove(0);
                        assert_eq!(turn.model_request.input, model_request.input);
                        let owners = turn.issued_outputs.unwrap();
                        assert!(
                            owners[position].is_none(),
                            "injected bytes cannot inherit an owner"
                        );
                        assert_eq!(
                            owners
                                .into_iter()
                                .flatten()
                                .map(|owner| owner.operation)
                                .collect::<Vec<_>>(),
                            expected
                        );
                        covered += 1;
                    }
                }
            }
        }
    }
    assert_eq!(covered, 48);
    eprintln!(
        "issued_output_history_model: {covered} histories; both kinds, both orders, projection, injection, 0..2 late outputs"
    );
}

#[test]
fn sealing_refuses_misalignment_forgery_foreign_lineage_and_rewritten_identity() {
    let store = Store::memory().unwrap();
    let operation = issue(&store, "a", None, ToolKind::Function);
    let output = store
        .history_occurrences(&operation.request)
        .unwrap()
        .pop()
        .unwrap();
    let exact = request(vec![output.item.clone()]);
    let foreign = RequestId("foreign".into());
    store.create_request(&foreign, None, "/foreign").unwrap();
    let events_before = store.events(Some(&operation.request)).unwrap();
    for variant in 0..7 {
        let mut req = exact.clone();
        let mut occurrences = vec![Some(output.clone())];
        let mut head = &operation.request;
        let mut hashes = vec![None];
        match variant {
            0 => occurrences.clear(),
            1 => {
                occurrences[0]
                    .as_mut()
                    .unwrap()
                    .output_operation
                    .as_mut()
                    .unwrap()
                    .request = foreign.clone()
            }
            2 => occurrences[0].as_mut().unwrap().origin.position += 1,
            3 => head = &foreign,
            4 => req.input[0].0["call_id"] = json!("other"),
            5 => {
                req.input.push(output.item.clone());
                occurrences.push(Some(output.clone()));
                hashes.push(None);
            }
            6 => hashes[0] = Some(store.put_item(&Item(json!("wrong bytes"))).unwrap()),
            _ => unreachable!(),
        }
        assert!(
            store
                .seal_replay_request_with_occurrences(head, &req, &hashes, None, &occurrences)
                .is_err(),
            "variant {variant}"
        );
        assert_eq!(
            store.events(Some(&operation.request)).unwrap(),
            events_before,
            "variant {variant} changed replay events"
        );
    }
}

#[test]
fn issued_ownership_reopens_and_atomic_failure_leaves_no_completion() {
    let path = std::env::temp_dir().join(format!(
        "harness-issued-owners-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let (operation, input, replay_event) = {
        let store = Store::open(&path).unwrap();
        let operation = issue(&store, "a", None, ToolKind::Function);
        let occurrences = store
            .history_occurrences(&operation.request)
            .unwrap()
            .into_iter()
            .map(Some)
            .collect::<Vec<_>>();
        let input = request(store.items(&operation.request).unwrap());
        let issued = store
            .seal_replay_request_with_occurrences(
                &operation.request,
                &input,
                &vec![None; input.input.len()],
                None,
                &occurrences,
            )
            .unwrap();
        store.lock().execute_batch("CREATE TRIGGER reject_owned_replay BEFORE INSERT ON events WHEN NEW.kind='model_turn' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
        let failed_response = ResponsesTurn {
            items: vec![Item(json!({"new":"response"}))],
            ..response()
        };
        let events_before = store.events(Some(&operation.request)).unwrap();
        assert!(
            store
                .record_issued_replay_turn(&operation.request, issued.clone(), &failed_response)
                .is_err()
        );
        assert_eq!(
            store.events(Some(&operation.request)).unwrap(),
            events_before
        );
        let hash = Store::put_item_tx_hash(&failed_response.items[0]).unwrap();
        assert!(store.get_item(&hash).unwrap().is_none());
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_owned_replay")
            .unwrap();
        let replay_event = store
            .record_issued_replay_turn(&operation.request, issued, &response())
            .unwrap();
        let events_after = store.events(Some(&operation.request)).unwrap();
        assert_eq!(
            &events_after[..events_before.len()],
            events_before.as_slice()
        );
        assert_eq!(events_after.len(), events_before.len() + 1);
        assert_eq!(events_after.last().unwrap().id, replay_event);
        assert_eq!(events_after.last().unwrap().kind, "model_turn");
        (operation, input, replay_event)
    };
    let store = Store::open(&path).unwrap();
    let turn = store
        .replay_turns(&operation.request)
        .unwrap()
        .into_iter()
        .find(|turn| turn.replay_event == Some(replay_event))
        .unwrap();
    assert_eq!(turn.model_request.input, input.input);
    assert_eq!(
        turn.issued_outputs.unwrap()[1].as_ref().unwrap().operation,
        operation
    );
    drop(store);
    std::fs::remove_file(path).unwrap();
}
