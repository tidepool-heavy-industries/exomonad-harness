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
    captured_history(false).await;
}

#[tokio::test]
async fn coissued_capture_preserves_only_its_authenticated_visible_response_prefix() {
    captured_history(true).await;
}

async fn captured_history(coissued: bool) {
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
    let answer = vec![Item(
        json!({"type":"message","role":"assistant","content":"child ran"}),
    )];
    let (turn, sent) = exchange(&destination, ResponsesProtocol::Standard, &answer).await;
    assert_eq!(turn.items, answer);
    assert_eq!(sent["model"], "gpt-6-luna");
    assert_eq!(
        sent["input"],
        serde_json::to_value(&destination.input).unwrap()
    );
    assert_eq!(sent["instructions"], "source instructions");
}
