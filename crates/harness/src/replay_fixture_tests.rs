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
        store::Store,
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
            prompt_cache_comparison: None,
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
                .write_output(
                    &store.claims(&call_id).unwrap()[0].operation,
                    &output,
                    crate::store::TerminalOutcome::Success,
                )
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
        store
            .write_output(
                &root_operation,
                &output,
                crate::store::TerminalOutcome::Success,
            )
            .unwrap();
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
}
