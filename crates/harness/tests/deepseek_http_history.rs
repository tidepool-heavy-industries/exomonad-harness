//! Offline production HTTP submission, dispatch and durable history controls.

#[path = "support/deepseek_http.rs"]
mod http_fixture;

use async_trait::async_trait;
use harness::{
    engine::{Engine, EngineConfig, EngineError},
    invocation::{CellBudget, Invocation, InvocationOptions, Limits, Outcome, Step},
    item::Item,
    model::{AgentPath, CallId, Effort},
    provider::{Provider, ProviderError},
    store::Store,
    transport::{
        Auth, CredentialReference, InferenceLimits, NativeTransportAdmission,
        NativeTransportContract, ResponsesClient, ResponsesProtocol, ResponsesRoute,
        TransportError, auth::DeepSeekAuth,
    },
    turn::JobScheduler,
};
use http_fixture::{HttpReply, LocalResponses, semantic_events};
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

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

struct NoEffects(AtomicUsize);

#[async_trait]
impl Provider for NoEffects {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("assistant-only completion must not execute a tool")
    }

    fn tools(&self) -> Vec<Value> {
        vec![]
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
    let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
    let jobs = Arc::new(JobScheduler::new(1).unwrap());
    let engine = Engine::with_transport(
        ResponsesClient::new(auth.clone()),
        store.clone(),
        jobs,
        effects.clone(),
        config(vec![]),
    );
    let (_cancel, cancelled) = tokio::sync::watch::channel(false);
    let (_inbox, incoming) = tokio::sync::mpsc::unbounded_channel();
    let running = tokio::spawn(async move {
        engine
            .run(
                None,
                vec![Item::user_text("answer without tools")],
                cancelled,
                incoming,
            )
            .await
    });
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
    let completion = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap()
        .expect("documented no-phase terminal message completes the production Engine");
    assert_eq!(
        completion
            .turn
            .items
            .iter()
            .map(|item| item.0.clone())
            .collect::<Vec<_>>(),
        items
    );
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
    let head = completion.head_request.clone();
    drop(completion);
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
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
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
        let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
        let engine = Engine::with_transport(
            ResponsesClient::new(files.auth(&server.endpoint)),
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            effects.clone(),
            config(vec![]),
        );
        let (_cancel, cancelled) = tokio::sync::watch::channel(false);
        let (_inbox, incoming) = tokio::sync::mpsc::unbounded_channel();
        let running = tokio::spawn(async move {
            engine
                .run(
                    None,
                    vec![Item::user_text("do not retry a rejection")],
                    cancelled,
                    incoming,
                )
                .await
        });
        let request = server.next().await;
        assert_deepseek_request(&request.captured);
        request.respond(HttpReply::json(axum::http::StatusCode::from_u16(status).unwrap(),
            json!({"error":{"code":"offline_rejected", "message":format!("refused {SYNTHETIC_KEY}")}})));
        let error = tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        let EngineError::RequestRejected {
            head_request,
            error,
        } = error
        else {
            panic!("HTTP {status} must retain exact rejection classification: {error:?}");
        };
        match status {
            401 => assert!(matches!(
                &error,
                TransportError::ProviderAuthentication {
                    route: ResponsesRoute::DeepSeek
                }
            )),
            _ => assert!(
                matches!(&error, TransportError::Http { status: actual, .. } if *actual == status)
            ),
        }
        assert!(!format!("{error:?}").contains(SYNTHETIC_KEY));
        assert_eq!(effects.0.load(Ordering::SeqCst), 0);
        assert!(
            store
                .items(&head_request)
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
    let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
    let engine = Engine::with_transport(
        ResponsesClient::new(files.auth(&server.endpoint)),
        store,
        Arc::new(JobScheduler::new(1).unwrap()),
        effects.clone(),
        config(vec![]),
    );
    let (_cancel, cancelled) = tokio::sync::watch::channel(false);
    let (_inbox, incoming) = tokio::sync::mpsc::unbounded_channel();
    let running = tokio::spawn(async move {
        engine
            .run(
                None,
                vec![Item::user_text("complete before EOF")],
                cancelled,
                incoming,
            )
            .await
    });
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
    let completion = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("terminal response must complete while the HTTP stream owner still retains EOF")
        .unwrap()
        .unwrap();
    assert_eq!(completion.turn.items, vec![Item(assistant("done"))]);
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
    drop(chunks);
    server.finish().await;
}
