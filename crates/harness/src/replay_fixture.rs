//! Small wire fixtures for replay call identity and lineage boundaries.
//!
//! Invocation evidence comes from persisted response Items in the selected
//! request branch. A tool call carries its own identity and kind; neither an
//! agent envelope nor a `Here`-fork substitutes for that Item.

#[cfg(test)]
mod tests {
    use crate::{
        item::{Item, ToolInput, ToolKind},
        model::{CallId, Effort, RequestId},
        store::{Store, StoreError},
        transport::{ResponsesRequest, ResponsesTurn, Usage},
        turn::JobOutput,
    };
    use serde_json::json;
    use std::path::PathBuf;

    fn temporary_store_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "harness-replay-fixture-{}-{}.db",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    fn replay_request(input: Vec<Item>, session_id: &str) -> ResponsesRequest {
        ResponsesRequest {
            input,
            instructions: "fixture".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "fixture-model".into(),
            pinned_effort: Effort::Low,
            session_id: session_id.into(),
        }
    }

    fn custom_call(id: &str, name: &str, input: &str) -> Item {
        Item(json!({
            "type": "custom_tool_call",
            "call_id": id,
            "name": name,
            "input": input
        }))
    }

    fn function_call(id: &str, name: &str, args: serde_json::Value) -> Item {
        Item(json!({
            "type": "function_call",
            "call_id": id,
            "name": name,
            "arguments": args
        }))
    }

    #[test]
    fn root_call_fixture_preserves_kind_identity_and_raw_custom_input() {
        let custom_text = "line one\nquote: \" slash: \\\\ snowman: ☃";
        let custom = custom_call("root-custom-7", "shell", custom_text)
            .tool_call()
            .unwrap()
            .unwrap();
        assert_eq!(custom.call_id, CallId("root-custom-7".into()));
        assert_eq!(custom.name, "shell");
        assert_eq!(custom.input, ToolInput::Custom(custom_text.into()));

        let function = function_call("root-function-7", "lookup", json!({"query": "snowman ☃"}))
            .tool_call()
            .unwrap()
            .unwrap();
        assert_eq!(function.call_id, CallId("root-function-7".into()));
        assert_eq!(function.name, "lookup");
        assert_eq!(
            function.input,
            ToolInput::Function(json!({"query": "snowman ☃"}))
        );
    }

    #[test]
    fn non_root_lineage_items_are_not_replay_call_evidence() {
        // Envelope/fork metadata is intentionally not interpreted as an
        // invocation. Only a selected request Item's call shape is evidence.
        for item in [
            Item(json!({
                "type": "agent_message",
                "call_id": "root-custom-7",
                "name": "shell",
                "input": "must not be replayed"
            })),
            Item(json!({
                "type": "here_fork",
                "call_id": "root-custom-7",
                "name": "shell",
                "input": "must not be replayed"
            })),
        ] {
            assert_eq!(item.tool_call().unwrap(), None);
        }
    }

    #[test]
    fn recognized_call_malformed_or_wrong_kind_shapes_are_rejected() {
        let custom_without_raw_input = Item(json!({
            "type": "custom_tool_call",
            "call_id": "same-id",
            "name": "shell",
            "arguments": {"command": "wrong field"}
        }));
        assert_eq!(
            custom_without_raw_input.tool_call(),
            Err("invalid custom input")
        );

        let function_without_arguments = Item(json!({
            "type": "function_call",
            "call_id": "same-id",
            "name": "lookup",
            "input": "wrong field"
        }));
        assert_eq!(
            function_without_arguments.tool_call(),
            Err("missing function arguments")
        );
    }

    #[test]
    fn encoded_outputs_keep_the_recorded_kind_and_call_identity() {
        let id = CallId("root-call-identity".into());
        let text = "raw output\nwith \"quotes\" and ☃";
        let custom_output = Item::tool_output(
            &id,
            ToolKind::Custom,
            &JobOutput::Completed(Ok(json!(text))),
        );
        assert_eq!(custom_output.0["type"], "custom_tool_call_output");
        assert_eq!(custom_output.0["call_id"], id.0);
        assert_eq!(custom_output.0["output"], text);

        let function_output = Item::tool_output(
            &id,
            ToolKind::Function,
            &JobOutput::Completed(Ok(json!({"text": text}))),
        );
        assert_eq!(function_output.0["type"], "function_call_output");
        assert_eq!(function_output.0["call_id"], id.0);

        // Durable call kind comes from the call itself. A differently encoded
        // output for that same identity is a mismatch, not a substitute.
        let call = custom_call(&id.0, "shell", "raw input")
            .tool_call()
            .unwrap()
            .unwrap();
        assert_eq!(call.input.kind(), ToolKind::Custom);
        assert_ne!(
            function_output.0["type"],
            serde_json::Value::String("custom_tool_call_output".into())
        );
    }

    #[test]
    fn durable_custom_call_and_output_reopen_without_agent_or_here_lineage() {
        let path = temporary_store_path();
        let root = RequestId("fixture-root".into());
        let agent = RequestId("fixture-agent".into());
        let here = RequestId("fixture-here".into());
        let call_id = CallId("fixture-custom-id".into());
        let invocation = custom_call(&call_id.0, "shell", "printf 'raw\\ncustom ☃'");
        let output = Item::tool_output(
            &call_id,
            ToolKind::Custom,
            &JobOutput::Completed(Ok(json!("raw\ncustom ☃"))),
        );
        let model_turn = |request_id: &str, item: Item| {
            (
                replay_request(vec![item.clone()], request_id),
                ResponsesTurn {
                    response_id: format!("response-{request_id}"),
                    items: vec![item],
                    usage: Usage::default(),
                },
            )
        };

        {
            let store = Store::open(&path).unwrap();
            store.create_request(&root, None, "/root").unwrap();
            store
                .create_request(&agent, Some(&root), "/root/agent")
                .unwrap();
            store
                .create_request(&here, Some(&root), "/root/here")
                .unwrap();
            store
                .append_items(&root, std::slice::from_ref(&invocation))
                .unwrap();
            let (request, response) = model_turn("fixture-root", invocation.clone());
            store
                .record_replay_turn(&root, &request, &response)
                .unwrap();
            store.claim(&call_id, &root).unwrap();
            store
                .write_output(&store.claims(&call_id).unwrap()[0].operation, &output)
                .unwrap();

            // Descendant-agent and Here-fork histories carry same-shaped
            // decoys, but neither is the root's durable invocation evidence.
            let agent_decoy = custom_call("agent-decoy", "shell", "agent only");
            store
                .append_items(&agent, std::slice::from_ref(&agent_decoy))
                .unwrap();
            let (request, response) = model_turn("fixture-agent", agent_decoy);
            store
                .record_replay_turn(&agent, &request, &response)
                .unwrap();
            let here_decoy = custom_call("here-decoy", "shell", "fork only");
            store
                .append_items(&here, std::slice::from_ref(&here_decoy))
                .unwrap();
            let (request, response) = model_turn("fixture-here", here_decoy);
            store
                .record_replay_turn(&here, &request, &response)
                .unwrap();
        }

        {
            let store = Store::open(&path).unwrap();
            let turns = store.replay_turns(&root).unwrap();
            assert_eq!(turns.len(), 1);
            let recovered_call = turns[0].model_response.items[0]
                .tool_call()
                .unwrap()
                .unwrap();
            assert_eq!(recovered_call.call_id, call_id);
            assert_eq!(
                recovered_call.input,
                ToolInput::Custom("printf 'raw\\ncustom ☃'".into())
            );
            let recovered_output = store.replay_output(&call_id).unwrap().unwrap();
            assert_eq!(recovered_output, output);
            assert_eq!(recovered_output.0["type"], "custom_tool_call_output");
            assert_eq!(recovered_output.0["call_id"], call_id.0);
            assert_eq!(recovered_output.0["output"], "raw\ncustom ☃");
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn replay_turns_include_same_branch_descendants_but_not_forked_descendants() {
        let store = Store::memory().unwrap();
        let root = RequestId("closure-root".into());
        let child = RequestId("closure-child".into());
        let grandchild = RequestId("closure-grandchild".into());
        let fork = RequestId("closure-fork".into());
        let sibling = RequestId("closure-sibling".into());
        store.create_request(&root, None, "/root").unwrap();
        store.create_request(&child, Some(&root), "/root").unwrap();
        store
            .create_request(&grandchild, Some(&child), "/root")
            .unwrap();
        store
            .create_request(&fork, Some(&child), "/root/here")
            .unwrap();
        store
            .create_request(&sibling, Some(&root), "/other-branch")
            .unwrap();
        for (id, label) in [
            (&root, "root"),
            (&child, "child"),
            (&grandchild, "grandchild"),
            (&fork, "fork"),
            (&sibling, "sibling"),
        ] {
            let call = custom_call(label, "shell", label);
            let request = replay_request(vec![call.clone()], label);
            let response = ResponsesTurn {
                response_id: label.into(),
                items: vec![call],
                usage: Usage::default(),
            };
            store.record_replay_turn(id, &request, &response).unwrap();
        }
        let selected = store.replay_turns(&root).unwrap();
        assert_eq!(
            selected
                .iter()
                .map(|turn| turn.request.0.as_str())
                .collect::<Vec<_>>(),
            ["closure-root", "closure-child", "closure-grandchild"]
        );
    }

    #[test]
    fn durable_function_invocation_and_output_survive_reopen() {
        let path = temporary_store_path();
        let request = RequestId("function-reopen-request".into());
        let call = CallId("function-reopen-call".into());
        let invocation = function_call(&call.0, "lookup", json!({"query":"☃"}));
        let output = Item::tool_output(
            &call,
            ToolKind::Function,
            &JobOutput::Completed(Ok(json!({"result":"snowman ☃"}))),
        );
        {
            let store = Store::open(&path).unwrap();
            store
                .write_request(
                    &request,
                    None,
                    "/root",
                    std::slice::from_ref(&invocation),
                    crate::store::Usage::default(),
                )
                .unwrap();
            store.claim(&call, &request).unwrap();
            store
                .write_output(&store.claims(&call).unwrap()[0].operation, &output)
                .unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            let retained = store.items(&request).unwrap();
            assert_eq!(retained, vec![invocation]);
            assert_eq!(
                retained[0].tool_call().unwrap().unwrap().input,
                ToolInput::Function(json!({"query":"☃"}))
            );
            assert_eq!(store.replay_output(&call).unwrap(), Some(output));
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn pending_invocation_has_no_replay_output() {
        let store = Store::memory().unwrap();
        let request = RequestId("missing-output-request".into());
        let call = CallId("missing-output-call".into());
        store
            .write_request(
                &request,
                None,
                "/root",
                &[custom_call(&call.0, "shell", "echo pending")],
                crate::store::Usage::default(),
            )
            .unwrap();
        store.claim(&call, &request).unwrap();
        assert_eq!(store.replay_output(&call).unwrap(), None);
    }

    #[test]
    fn same_call_id_agent_and_here_claims_do_not_extend_root_replay_turns() {
        let store = Store::memory().unwrap();
        let root = RequestId("shared-id-root".into());
        let agent = RequestId("shared-id-agent".into());
        let here = RequestId("shared-id-here".into());
        let call = CallId("shared-branch-call".into());
        let output = Item::tool_output(
            &call,
            ToolKind::Custom,
            &JobOutput::Completed(Ok(json!("root output"))),
        );
        for (request, parent, branch, input) in [
            (&root, None, "/root", "root invocation"),
            (&agent, Some(&root), "/root/agent", "agent decoy"),
            (&here, Some(&root), "/root/here", "Here decoy"),
        ] {
            let item = custom_call(&call.0, "shell", input);
            store
                .write_request(
                    request,
                    parent,
                    branch,
                    std::slice::from_ref(&item),
                    crate::store::Usage::default(),
                )
                .unwrap();
            let model_request = replay_request(vec![item.clone()], &request.0);
            store
                .record_replay_turn(
                    request,
                    &model_request,
                    &ResponsesTurn {
                        response_id: request.0.clone(),
                        items: vec![item],
                        usage: Usage::default(),
                    },
                )
                .unwrap();
            store.claim(&call, request).unwrap();
        }
        let claims = store.claims(&call).unwrap();
        assert_eq!(claims.len(), 3);
        assert_ne!(claims[0].operation, claims[1].operation);
        assert_ne!(claims[1].operation, claims[2].operation);
        let root_operation = claims
            .iter()
            .find(|claim| claim.request == root)
            .unwrap()
            .operation
            .clone();
        store.write_output(&root_operation, &output).unwrap();
        assert_eq!(store.claims(&call).unwrap().len(), 3);
        let selected = store.replay_turns(&root).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].request, root);
        assert_eq!(
            selected[0].model_response.items[0]
                .tool_call()
                .unwrap()
                .unwrap()
                .input,
            ToolInput::Custom("root invocation".into())
        );
        assert_eq!(
            store.replay_output_for_request(&root, &call).unwrap(),
            Some(output)
        );
        assert_eq!(
            store.replay_output_for_request(&agent, &call).unwrap(),
            None
        );
        assert_eq!(store.replay_output_for_request(&here, &call).unwrap(), None);
    }

    #[test]
    fn duplicate_invocations_are_not_replay_evidence() {
        let store = Store::memory().unwrap();
        let request = RequestId("ambiguous-request".into());
        let call = CallId("ambiguous-call".into());
        store.create_request(&request, None, "/root").unwrap();
        store
            .append_items(
                &request,
                &[
                    custom_call(&call.0, "shell", "first"),
                    custom_call(&call.0, "shell", "second"),
                ],
            )
            .unwrap();
        store.claim(&call, &request).unwrap();
        assert!(matches!(
            store.write_output(
                &store.claims(&call).unwrap()[0].operation,
                &Item(json!({
                    "type": "custom_tool_call_output",
                    "call_id": call.0,
                    "output": "ambiguous invocation"
                })),
            ),
            Err(StoreError::AmbiguousReplayCall { .. })
        ));
    }

    #[test]
    fn mismatched_output_call_id_is_not_replay_evidence() {
        let store = Store::memory().unwrap();
        let request = RequestId("mismatched-request".into());
        store.create_request(&request, None, "/root").unwrap();
        let distinct = CallId("wrong-output-id".into());
        store
            .append_items(&request, &[custom_call(&distinct.0, "shell", "unique")])
            .unwrap();
        store.claim(&distinct, &request).unwrap();
        assert!(
            store
                .write_output(
                    &store.claims(&distinct).unwrap()[0].operation,
                    &Item(json!({
                        "type": "custom_tool_call_output",
                        "call_id": "not-wrong-output-id",
                        "output": "mismatched output"
                    })),
                )
                .is_err()
        );
        assert_eq!(store.replay_output(&distinct).unwrap(), None);
    }

    #[test]
    fn mismatched_output_kind_is_not_replay_evidence() {
        let store = Store::memory().unwrap();
        let request = RequestId("wrong-kind-request".into());
        store.create_request(&request, None, "/root").unwrap();
        let wrong_kind = CallId("wrong-kind-id".into());
        store
            .append_items(&request, &[custom_call(&wrong_kind.0, "shell", "unique")])
            .unwrap();
        store.claim(&wrong_kind, &request).unwrap();
        assert!(
            store
                .write_output(
                    &store.claims(&wrong_kind).unwrap()[0].operation,
                    &Item(json!({
                        "type": "function_call_output",
                        "call_id": wrong_kind.0,
                        "output": "{}"
                    })),
                )
                .is_err()
        );
        assert_eq!(store.replay_output(&wrong_kind).unwrap(), None);
    }
}
