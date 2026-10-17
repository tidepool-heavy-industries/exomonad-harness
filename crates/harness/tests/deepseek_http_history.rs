//! Offline production HTTP submission, dispatch and durable history controls.

#[path = "support/deepseek_http.rs"]
mod http_fixture;

use harness::{
    engine::EngineConfig,
    invocation::{CellBudget, Invocation, InvocationOptions, Limits, Outcome, Receipt, Step},
    item::Item,
    model::{AgentPath, CallId, Effort},
    store::Store,
    transport::{
        Auth, CredentialReference, InferenceLimits, NativeTransportAdmission,
        NativeTransportContract, RequestFailure, ResponsesClient, ResponsesProtocol,
        ResponsesRoute, auth::DeepSeekAuth,
    },
    turn::JobScheduler,
};
use http_fixture::{HttpReply, LocalResponses, semantic_events};
use serde_json::{Value, json};
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf, sync::Arc, time::Duration};

const SYNTHETIC_KEY: &str = "offline-fixture-not-a-provider-key";
const UNICODE: &str = "λ → \"quoted\"\\path\n第二行 🪼";

struct Files(PathBuf);

impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("deepseek-http-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn auth(&self, endpoint: &str) -> DeepSeekAuth {
        let path = self.0.join("synthetic-key");
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        writeln!(file, "{SYNTHETIC_KEY}").unwrap();
        let admission = NativeTransportAdmission::rehydrate(NativeTransportContract {
            route: ResponsesRoute::DeepSeek,
            protocol: ResponsesProtocol::Standard,
            endpoint: endpoint.into(),
            credential: CredentialReference::File { path },
            limits: InferenceLimits::deepseek_default(),
        })
        .expect("ordinary admission accepts explicitly bound numeric loopback endpoint");
        DeepSeekAuth::from_admission(admission).unwrap()
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn config(tools: Vec<Value>) -> EngineConfig {
    EngineConfig {
        instructions: "offline integration standing instruction".into(),
        tools,
        model: "deepseek-flash".into(),
        effort: Effort::High,
        session_id: "deepseek-http-integration".into(),
        agent: AgentPath("/offline".into()),
    }
}

fn no_tools(auth: DeepSeekAuth, store: Arc<Store>, text: &str) -> Invocation {
    Invocation::start::<DeepSeekAuth, _>(
        ResponsesClient::new(auth),
        store,
        Arc::new(JobScheduler::new(1).unwrap()),
        config(vec![]),
        "http-owner".into(),
        CellBudget::new(Limits {
            requests: 1,
            tools: 0,
            reported_tokens: 1000,
            seconds: 15,
        }),
        vec![Item::user_text(text)],
        InvocationOptions::default(),
    )
    .unwrap()
}

async fn finished(invocation: &mut Invocation) -> Receipt {
    let Step::Finished(receipt) = tokio::time::timeout(Duration::from_secs(5), invocation.next())
        .await
        .expect("local HTTP invocation must finish without another callback")
    else {
        panic!("a no-tool invocation must not deliver a callback or hook");
    };
    assert_eq!(receipt.counts.requests, 1);
    assert_eq!(receipt.counts.tools, 0);
    assert_eq!(receipt.requests.len(), 1);
    receipt
}

fn reasoning() -> Value {
    // Includes the empty summary present in the official response example.
    json!({"type":"reasoning","id":"reasoning-1","status":"completed","summary":[],
        "content":[{"type":"reasoning_text","text":UNICODE}]})
}

fn assistant(text: &str) -> Value {
    // DeepSeek's documented completed messages have no `phase` field.
    json!({"type":"message","id":"message-1","status":"completed","role":"assistant",
        "content":[{"type":"output_text","text":text,"annotations":[]}]})
}

fn completed(id: &str, items: &[Value]) -> Vec<u8> {
    let mut events = vec![json!({"type":"response.created","sequence_number":0,
        "response":{"id":id,"object":"response","status":"in_progress"}})];
    for (index, item) in items.iter().enumerate() {
        events.push(
            json!({"type":"response.output_item.done","sequence_number":index+1,
            "output_index":index,"item":item}),
        );
    }
    events.push(
        json!({"type":"response.completed","sequence_number":items.len()+1,
        "response":{"id":id,"object":"response","status":"completed","model":"deepseek-flash",
            "output":items,"usage":{"input_tokens":22,"output_tokens":29,
                "input_tokens_details":{"cached_tokens":0},
                "output_tokens_details":{"reasoning_tokens":27},"total_tokens":51}}}),
    );
    semantic_events(&events)
}

fn assert_deepseek_request(request: &http_fixture::CapturedRequest) -> Value {
    assert_eq!(request.method, axum::http::Method::POST);
    assert_eq!(request.uri.path(), "/responses");
    assert_eq!(
        request.headers["authorization"].to_str().unwrap(),
        format!("Bearer {SYNTHETIC_KEY}")
    );
    for name in [
        "content-encoding",
        "chatgpt-account-id",
        "session-id",
        "thread-id",
        "x-client-request-id",
    ] {
        assert!(
            !request.headers.contains_key(name),
            "unexpected OpenAI header {name}"
        );
    }
    let body = request.json();
    assert_eq!(body["model"], "deepseek-flash");
    assert_eq!(body["reasoning"], json!({"effort":"high"}));
    assert_eq!(body["max_output_tokens"], 65_536);
    assert_eq!(body["stream"], true);
    for field in [
        "previous_response_id",
        "conversation",
        "include",
        "prompt_cache_key",
        "prompt_cache_options",
    ] {
        assert!(
            body.get(field).is_none(),
            "unexpected stateless/profile field {field}"
        );
    }
    if let Some(tools) = body.get("tools") {
        for tool in tools.as_array().unwrap() {
            assert_eq!(tool["type"], "function");
            assert!(tool.get("async").is_none());
        }
    }
    body
}

#[tokio::test]
async fn documented_plaintext_completion_survives_http_fragmentation_and_durable_history() {
    let mut server = LocalResponses::start().await;
    let files = Files::new();
    let auth = files.auth(&server.endpoint);
    let expected_contract = auth.admission().unwrap().contract().clone();
    let store = Arc::new(Store::open(files.0.join("history.sqlite")).unwrap());
    let mut invocation = no_tools(auth, store.clone(), "answer without tools");
    let request = server.next().await;
    let body = assert_deepseek_request(&request.captured);
    assert_eq!(
        body["input"].as_array().unwrap().last().unwrap()["role"],
        "user"
    );
    let items = vec![reasoning(), assistant(UNICODE)];
    request.respond(HttpReply::fragmented_sse(
        completed("documented", &items),
        &[1, 2, 7, 3],
    ));
    let receipt = finished(&mut invocation).await;
    assert!(
        matches!(&receipt.outcome, Outcome::Text(text) if text == UNICODE),
        "{:?}",
        receipt.outcome
    );
    assert_eq!(
        receipt
            .transcript
            .iter()
            .filter(|item| matches!(item.0["id"].as_str(), Some("reasoning-1" | "message-1")))
            .map(|item| item.0.clone())
            .collect::<Vec<_>>(),
        items
    );
    let head = receipt.requests[0].clone();
    drop(invocation);
    drop(store);

    let reopened = Store::open(files.0.join("history.sqlite")).unwrap();
    assert_eq!(
        reopened
            .items(&head)
            .unwrap()
            .iter()
            .filter(|item| matches!(item.0["id"].as_str(), Some("reasoning-1" | "message-1")))
            .map(|item| item.0.clone())
            .collect::<Vec<_>>(),
        items
    );
    assert_eq!(
        reopened.native_transport_contract(&head).unwrap(),
        Some(expected_contract)
    );
    server.finish().await;
}

#[tokio::test]
async fn bounded_invocation_dispatches_exact_unicode_then_replays_canonical_function_history() {
    let mut server = LocalResponses::start().await;
    let files = Files::new();
    let store = Arc::new(Store::memory().unwrap());
    let client = ResponsesClient::new(files.auth(&server.endpoint));
    let tool = json!({"type":"function","name":"echo","strict":true,
        "parameters":{"type":"object","properties":{"source":{"type":"string"}},
            "required":["source"],"additionalProperties":false}});
    let mut invocation = Invocation::start::<DeepSeekAuth, _>(
        client,
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        config(vec![tool]),
        "http-owner".into(),
        CellBudget::new(Limits {
            requests: 2,
            tools: 1,
            reported_tokens: 1000,
            seconds: 15,
        }),
        vec![Item::user_text("echo the exact source")],
        InvocationOptions::default(),
    )
    .unwrap();
    let first = server.next().await;
    let body = assert_deepseek_request(&first.captured);
    let echo = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "echo")
        .unwrap();
    assert!(
        echo["parameters"]["properties"].get("delivery").is_none(),
        "bounded synchronous tools retain ordinary arguments"
    );
    let call = json!({"type":"function_call","id":"call-item","status":"completed", "call_id":"echo-1", "name":"echo", "arguments":json!({"source":UNICODE}).to_string()});
    first.respond(HttpReply::fragmented_sse(
        completed("with-call", &[reasoning(), call.clone()]),
        &[1, 3, 2],
    ));
    let Step::Callback(callback) = tokio::time::timeout(Duration::from_secs(5), invocation.next())
        .await
        .unwrap()
    else {
        panic!("actual HTTP function must reach the bounded production callback");
    };
    assert_eq!(callback.name, "echo");
    assert_eq!(callback.arguments, json!({"source":UNICODE}));
    let operation = callback.operation.clone();
    callback.complete(Ok(json!({"echo":UNICODE}))).unwrap();
    let second = server.next().await;
    let body = assert_deepseek_request(&second.captured);
    let input = body["input"].as_array().unwrap();
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .collect::<Vec<_>>(),
        vec![&reasoning()]
    );
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "function_call")
            .collect::<Vec<_>>(),
        vec![&call]
    );
    let outputs = input
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), 1);
    assert_eq!(outputs[0]["call_id"], "echo-1");
    assert_eq!(
        serde_json::from_str::<Value>(outputs[0]["output"].as_str().unwrap()).unwrap(),
        json!({"echo":UNICODE})
    );
    second.respond(HttpReply::fragmented_sse(
        completed("after-call", &[assistant("done")]),
        &[2, 1, 5],
    ));
    let Step::Finished(receipt) = tokio::time::timeout(Duration::from_secs(5), invocation.next())
        .await
        .unwrap()
    else {
        panic!("one issued callback must finish without duplicate execution");
    };
    assert!(
        matches!(&receipt.outcome, Outcome::Text(text) if text == "done"),
        "{:?}",
        receipt.outcome
    );
    assert_eq!(receipt.counts.requests, 2);
    assert_eq!(receipt.counts.tools, 1);
    let claims = store.claims(&CallId("echo-1".into())).unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].operation, operation);
    assert!(receipt.transcript.iter().any(|item| item.0 == call));
    server.finish().await;
}

#[tokio::test]
async fn http_rejections_retain_typed_causes_without_retry_or_effects() {
    for status in [400, 401, 403, 429] {
        let mut server = LocalResponses::start().await;
        let files = Files::new();
        let store = Arc::new(Store::memory().unwrap());
        let mut invocation = no_tools(
            files.auth(&server.endpoint),
            store.clone(),
            "do not retry a rejection",
        );
        let request = server.next().await;
        assert_deepseek_request(&request.captured);
        request.respond(HttpReply::json(axum::http::StatusCode::from_u16(status).unwrap(),
            json!({"error":{"code":"offline_rejected", "message":format!("refused {SYNTHETIC_KEY}")}})));
        let receipt = finished(&mut invocation).await;
        assert!(
            matches!(receipt.outcome, Outcome::Failed(_)),
            "{:?}",
            receipt.outcome
        );
        let head_request = &receipt.requests[0];
        let failures = store
            .events(Some(head_request))
            .unwrap()
            .into_iter()
            .filter(|event| event.kind == "request_failed")
            .collect::<Vec<_>>();
        assert_eq!(
            failures.len(),
            1,
            "one exact durable refusal, no ordinary completion/retry"
        );
        let error: RequestFailure = serde_json::from_str(&failures[0].payload).unwrap();
        match status {
            401 => assert!(matches!(
                &error,
                RequestFailure::ProviderAuthentication {
                    route: ResponsesRoute::DeepSeek
                }
            )),
            _ => assert!(
                matches!(&error, RequestFailure::Http { status: actual, .. } if *actual == status)
            ),
        }
        assert!(!format!("{error:?}").contains(SYNTHETIC_KEY));
        assert!(!format!("{:?}", receipt.outcome).contains(SYNTHETIC_KEY));
        assert!(
            store
                .items(head_request)
                .unwrap()
                .iter()
                .all(|item| !matches!(
                    item.0["type"].as_str(),
                    Some("function_call" | "function_call_output")
                ))
        );
        server.finish().await;
    }
}

#[tokio::test]
async fn completed_terminal_releases_client_before_http_eof() {
    let mut server = LocalResponses::start().await;
    let files = Files::new();
    let store = Arc::new(Store::memory().unwrap());
    let mut invocation = no_tools(files.auth(&server.endpoint), store, "complete before EOF");
    let request = server.next().await;
    assert_deepseek_request(&request.captured);
    let (response, chunks) = HttpReply::controlled_sse();
    request.respond(response);
    chunks
        .send(Ok(axum::body::Bytes::from(completed(
            "held-eof",
            &[assistant("done")],
        ))))
        .await
        .unwrap();
    let receipt = finished(&mut invocation).await;
    assert!(
        matches!(&receipt.outcome, Outcome::Text(text) if text == "done"),
        "{:?}",
        receipt.outcome
    );
    assert_eq!(
        receipt
            .transcript
            .iter()
            .filter(|item| item.0["id"] == "message-1")
            .map(|item| item.0.clone())
            .collect::<Vec<_>>(),
        vec![assistant("done")]
    );
    drop(chunks);
    server.finish().await;
}
