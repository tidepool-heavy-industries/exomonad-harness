use super::*;
use crate::{
    checkpoint::CheckpointChild,
    item::Item,
    model::{AgentPath, CallId, Effort, RequestId},
    store::{Store, TerminalOutcome},
};
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The production serializer, HTTP request builder and SSE body reader run
/// unchanged; only the built request's URL points to the fixture listener.
async fn exchange(
    request: &ResponsesRequest,
    protocol: ResponsesProtocol,
    items: &[Item],
) -> (ResponsesTurn, Value) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut events = items
        .iter()
        .map(|item| {
            format!(
                "data: {}\n\n",
                json!({"type":"response.output_item.done","item":item.0})
            )
        })
        .collect::<String>();
    events.push_str(&format!(
        "data: {}\n\n",
        json!({"type":"response.completed","response":{"id":"response-fixture","output":[]}})
    ));
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut received = Vec::new();
        let (offset, length) = loop {
            let mut bytes = [0; 4096];
            let read = socket.read(&mut bytes).await.unwrap();
            assert_ne!(read, 0, "HTTP request ended before its body");
            received.extend_from_slice(&bytes[..read]);
            if let Some(offset) = received.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&received[..offset]).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                if received.len() >= offset + 4 + length {
                    break (offset + 4, length);
                }
            }
        };
        let body = serde_json::from_slice(&received[offset..offset + length]).unwrap();
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{events}", events.len()).as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
        body
    });
    let http = http_client().unwrap();
    let body = normalized_request(request, protocol, ResponsesRoute::Codex).unwrap();
    let mut built = http_request(
        &http,
        ResponsesRoute::Codex,
        protocol,
        request,
        &body,
        "fixture-token",
        Some("fixture-account"),
    )
    .build()
    .unwrap();
    *built.url_mut() = format!("http://{address}/responses").parse().unwrap();
    let response = http.execute(built).await.unwrap();
    let turn = read_response(response, None, "fixture-token", "fixture-account")
        .await
        .unwrap();
    (turn, server.await.unwrap())
}

#[tokio::test]
async fn captured_lite_history_seeds_standard_model_through_production_http_and_store() {
    captured_history(false, Descendant::None).await;
}

#[tokio::test]
async fn coissued_capture_preserves_only_its_authenticated_visible_response_prefix() {
    captured_history(true, Descendant::None).await;
}

#[derive(Clone, Copy, PartialEq)]
enum Descendant {
    None,
    Captured,
    Here,
}

#[tokio::test]
async fn recursive_captured_history_retains_original_model_cut_through_three_generations() {
    captured_history(true, Descendant::Captured).await;
}

#[tokio::test]
async fn recursive_here_history_retains_original_model_cut_through_three_generations() {
    captured_history(true, Descendant::Here).await;
}

async fn captured_history(coissued: bool, descendant: Descendant) {
    let initial = Item(json!({"type":"message","role":"user","content":"original user input"}));
    let request = ResponsesRequest {
        model: "gpt-6.1-sol".into(),
        instructions: "source instructions".into(),
        input: vec![initial.clone()],
        tools: vec![].into(),
        tools_allowed: None,
        pinned_effort: Effort::High,
        session_id: "source-session".into(),
    };
    let source_items = vec![
        Item(
            json!({"type":"reasoning","id":"reasoning-item","content":[],"summary":[],"encrypted_content":"synthetic-ciphertext"}),
        ),
        Item(
            json!({"type":"message","id":"message-item","status":"completed","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"original visible answer","annotations":[],"logprobs":[]}]}),
        ),
        Item(
            json!({"type":"custom_tool_call","id":"tool-item","status":"completed","call_id":"work-call","name":"cell","input":"original Haskell input"}),
        ),
    ];
    let (turn, sent) = exchange(&request, ResponsesProtocol::Lite, &source_items).await;
    assert_eq!(
        turn.items, source_items,
        "SSE ingest must retain original provider bytes"
    );
    assert!(sent.get("instructions").is_none());
    assert_eq!(sent["model"], "gpt-6.1-sol");
    let store = Store::memory().unwrap();
    let initial_head = RequestId("input".into());
    let head = RequestId("response".into());
    store
        .write_request(&initial_head, None, "/root", &[initial], Default::default())
        .unwrap();
    store.set_effort(&initial_head, Effort::High).unwrap();
    store
        .write_request(
            &head,
            Some(&initial_head),
            "/root",
            &turn.items,
            Default::default(),
        )
        .unwrap();
    store.record_replay_turn(&head, &request, &turn).unwrap();
    let work = store.claim(&CallId("work-call".into()), &head).unwrap();
    store.settle_claims(&work, &Item(json!({"type":"custom_tool_call_output","call_id":"work-call","output":"original completed output"})), TerminalOutcome::Success).unwrap();
    store.append_operation_output(&work, &head, &head).unwrap();
    let spawn_head = RequestId("spawn".into());
    let mut spawn_items = Vec::new();
    if coissued {
        spawn_items.extend([
            Item(json!({"type":"reasoning","content":[],"summary":[],"encrypted_content":"capture-ciphertext"})),
            Item(json!({"type":"message","role":"assistant","phase":null,"content":[{"type":"output_text","text":"visible capture companion","annotations":[],"logprobs":[]}]})),
        ]);
    }
    spawn_items.push(Item(json!({"type":"custom_tool_call","call_id":"spawn-call","name":"cell","input":"spawn child"})));
    let (spawn_turn, _) = exchange(&request, ResponsesProtocol::Lite, &spawn_items).await;
    store
        .write_request(
            &spawn_head,
            Some(&head),
            "/root",
            &spawn_turn.items,
            Default::default(),
        )
        .unwrap();
    store
        .record_replay_turn(&spawn_head, &request, &spawn_turn)
        .unwrap();
    store.lock().execute("INSERT INTO agents(path,head_request,contract,fork_source,state,created_at) VALUES('/root',?1,'{}','{}','active',0)", [&spawn_head.0]).unwrap();
    let spawn = store
        .claim(&CallId("spawn-call".into()), &spawn_head)
        .unwrap();
    let cuts = store
        .capture_checkpoint_cuts(&spawn, &json!({}), Arc::new(()))
        .unwrap();
    let parent = AgentPath("/root".into());
    let child_path = AgentPath("/root/child".into());
    let (child, _) = store
        .attach_checkpoint_child(
            cuts.before_call(),
            CheckpointChild {
                path: &child_path,
                parent: &parent,
                contract: &json!({}),
                checkout: &json!({}),
                task: None,
            },
        )
        .unwrap();
    let identity = store.standalone_identity(child_path);
    store
        .initialize_context_model(&identity, "gpt-6-luna")
        .unwrap();
    let state = store
        .context_request_state(child.head_request.as_ref().unwrap(), &identity)
        .unwrap();
    let input = state
        .history
        .into_iter()
        .map(|(_, _, item)| item)
        .collect::<Vec<_>>();
    let portable = serde_json::to_string(&input).unwrap();
    for text in [
        "original user input",
        "original visible answer",
        "original Haskell input",
        "original completed output",
        "work-call",
    ] {
        assert!(portable.contains(text), "lost {text}");
    }
    for metadata in [
        "synthetic-ciphertext",
        "reasoning-item",
        "message-item",
        "tool-item",
        "logprobs",
        "capture-ciphertext",
        "spawn-call",
        "spawn child",
    ] {
        assert!(
            !portable.contains(metadata),
            "portable history retained {metadata}"
        );
    }
    assert_eq!(portable.contains("visible capture companion"), coissued);
    assert_eq!(store.items(&spawn_head).unwrap(), spawn_items);
    assert!(
        input
            .iter()
            .filter(|item| !item.is_configuration_update())
            .all(|item| item.0["type"] == "message")
    );
    assert_eq!(
        store.items(&head).unwrap()[..source_items.len()],
        source_items
    );
    let destination = ResponsesRequest {
        model: "gpt-6-luna".into(),
        input,
        session_id: "child-session".into(),
        ..request
    };
    let mut answer = vec![Item(
        json!({"type":"message","role":"assistant","content":"child ran"}),
    )];
    if descendant != Descendant::None {
        answer.insert(0, Item(json!({"type":"reasoning","summary":[],"content":[],"encrypted_content":"luna-native-continuity"})));
    }
    if descendant == Descendant::Captured {
        answer.push(Item(json!({"type":"custom_tool_call","call_id":"recursive-call","name":"cell","input":"capture recursive helper"})));
    }
    let (turn, sent) = exchange(&destination, ResponsesProtocol::Standard, &answer).await;
    assert_eq!(turn.items, answer);
    assert_eq!(sent["model"], "gpt-6-luna");
    assert_eq!(
        sent["input"],
        serde_json::to_value(&destination.input).unwrap()
    );
    assert_eq!(sent["instructions"], "source instructions");
    if descendant == Descendant::None {
        return;
    }

    let child_turn = RequestId("child-response".into());
    store
        .write_request(
            &child_turn,
            child.head_request.as_ref(),
            &child.path.0,
            &turn.items,
            Default::default(),
        )
        .unwrap();
    store
        .record_replay_turn(&child_turn, &destination, &turn)
        .unwrap();
    assert!(
        store
            .advance_agent_head(&child.path, child.head_request.as_ref(), Some(&child_turn))
            .unwrap()
    );
    let grandchild_path = AgentPath("/root/child/grandchild".into());
    let grandchild = match descendant {
        Descendant::Captured => {
            let operation = store
                .claim(&CallId("recursive-call".into()), &child_turn)
                .unwrap();
            let cuts = store
                .capture_checkpoint_cuts(&operation, &json!({}), Arc::new(()))
                .unwrap();
            store
                .attach_checkpoint_child(
                    cuts.before_call(),
                    CheckpointChild {
                        path: &grandchild_path,
                        parent: &child.path,
                        contract: &json!({}),
                        checkout: &json!({}),
                        task: None,
                    },
                )
                .unwrap()
                .0
        }
        Descendant::Here => {
            store
                .admit_here_agent_with_snapshot(
                    &grandchild_path,
                    &child.path,
                    &RequestId("here-grandchild".into()),
                    &json!({}),
                    &child.path.0,
                    &grandchild_path.0,
                    "new_task",
                    &Item(json!({"type":"message","role":"user","content":"grandchild task"})),
                )
                .unwrap()
                .0
        }
        Descendant::None => unreachable!(),
    };
    let grandchild_head = grandchild.head_request.as_ref().unwrap();
    let grandchild_identity = store.standalone_identity(grandchild_path);
    store
        .initialize_context_model(&grandchild_identity, "gpt-6-luna")
        .unwrap();
    let canonical = store.context_history(grandchild_head).unwrap();
    let state = store
        .context_request_state(grandchild_head, &grandchild_identity)
        .unwrap();
    let visible = serde_json::to_string(&state.history).unwrap();
    for text in [
        "original user input",
        "original visible answer",
        "original Haskell input",
        "original completed output",
        "visible capture companion",
        "child ran",
    ] {
        assert!(visible.contains(text), "recursive history lost {text}");
    }
    for excluded in [
        "synthetic-ciphertext",
        "capture-ciphertext",
        "spawn-call",
        "recursive-call",
    ] {
        assert!(
            !visible.contains(excluded),
            "recursive history leaked {excluded}"
        );
    }
    assert!(
        visible.contains("luna-native-continuity"),
        "same-model native continuity must stay intact"
    );
    let retained = store.history_occurrences(grandchild_head).unwrap();
    let wrong_head = RequestId("detached-recursive-prefix".into());
    store
        .write_request(
            &wrong_head,
            None,
            &grandchild.path.0,
            &[],
            Default::default(),
        )
        .unwrap();
    {
        let mut connection = store.lock();
        let transaction = connection.transaction().unwrap();
        for (position, occurrence) in retained
            .iter()
            .filter(|occurrence| occurrence.origin.request == spawn_head)
            .enumerate()
        {
            crate::store::context::insert_occurrence(
                &transaction,
                &wrong_head,
                position as i64,
                occurrence,
            )
            .unwrap();
        }
        transaction.commit().unwrap();
    }
    assert!(matches!(
        store.context_request_state(&wrong_head, &grandchild_identity),
        Err(crate::store::StoreError::Context(
            crate::context::ContextError::OpaqueModel
        ))
    ));
    store
        .append_items(
            &spawn_head,
            &[Item(
                json!({"type":"message","role":"assistant","content":"later root arrival"}),
            )],
        )
        .unwrap();
    store.append_items(&child_turn, &[Item(json!({"type":"message","role":"assistant","content":"later child arrival"})), Item(json!({"type":"custom_tool_call","call_id":"late-child-call","name":"cell","input":"later pending child call"}))]).unwrap();
    store
        .claim(&CallId("late-child-call".into()), &child_turn)
        .unwrap();
    assert_eq!(store.context_history(grandchild_head).unwrap(), canonical);
    assert!(store.pending_at(grandchild_head).unwrap().is_empty());
    assert!(
        store
            .claims_on_branch_lineage(grandchild_head, &grandchild.path.0)
            .unwrap()
            .iter()
            .all(|claim| claim.operation.call.0 != "late-child-call")
    );
    let repeated = store
        .context_request_state(grandchild_head, &grandchild_identity)
        .unwrap();
    assert_eq!(state.history, repeated.history);
    let request = ResponsesRequest {
        model: "gpt-6-luna".into(),
        input: repeated
            .history
            .into_iter()
            .map(|(_, _, item)| item)
            .collect(),
        session_id: "grandchild-session".into(),
        ..destination
    };
    let grandchild_answer = vec![Item(
        json!({"type":"message","role":"assistant","content":"grandchild ran"}),
    )];
    let (completed, sent) =
        exchange(&request, ResponsesProtocol::Standard, &grandchild_answer).await;
    assert_eq!(completed.items, grandchild_answer);
    assert_eq!(sent["input"], serde_json::to_value(&request.input).unwrap());
    assert_eq!(
        store.items(&head).unwrap()[..source_items.len()],
        source_items
    );
}
