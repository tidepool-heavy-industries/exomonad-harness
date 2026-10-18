//! Offline production HTTP submission, dispatch and durable history controls.

#[path = "support/deepseek_http.rs"]
mod http_fixture;

use async_trait::async_trait;
use harness::{
    context::{ContextError, ContextProjectionTarget, ContextReplayRejection},
    engine::{Engine, EngineConfig},
    invocation::{CellBudget, Invocation, InvocationOptions, Limits, Outcome, Receipt, Step},
    item::Item,
    model::{AgentPath, CallId, Effort},
    provider::{Provider, ProviderError, ToolFailure},
    store::{Store, StoreError, TerminalOutcome},
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
const REASONING: &str = "offline reasoning marker — 独立 λ 🪼";

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

async fn next_http(
    server: &mut LocalResponses,
    invocation: &mut Invocation,
) -> http_fixture::PendingRequest {
    tokio::select! {
        step = invocation.next() => match step {
            Step::Finished(receipt) => panic!("invocation ended before its expected HTTP request: {:?}", receipt.outcome),
            Step::Callback(callback) => panic!("unexpected callback before the next HTTP response: {}", callback.name),
            Step::HookRequested(_) => panic!("unexpected hook before the next HTTP response"),
        },
        request = server.next() => request,
    }
}

fn reasoning() -> Value {
    // Includes the empty summary present in the official response example.
    json!({"type":"reasoning","id":"reasoning-1","status":"completed","summary":[],
        "content":[{"type":"reasoning_text","text":REASONING}]})
}

fn observed_reasoning() -> Value {
    let mut item = reasoning();
    // Sanitized shape observed from the official Responses route, including
    // its nonempty metadata field. The value does not establish provenance.
    item["encrypted_content"] = json!("4a66e889-79cb-419b-976a-572a9c63fbc8-0");
    item
}

struct NoTools;

#[async_trait]
impl Provider for NoTools {
    async fn request_snapshot_with_encoding(
        &self,
        profile: harness::transport::ToolEncodingProfile,
    ) -> Result<Option<Arc<dyn Provider>>, ProviderError> {
        assert_eq!(profile, ResponsesRoute::DeepSeek.tool_encoding_profile());
        assert!(self.all_tools().is_empty());
        Ok(None)
    }

    fn all_tools(&self) -> Vec<Value> {
        self.tools()
    }

    fn tools(&self) -> Vec<Value> {
        vec![]
    }

    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("reopened history must not execute the settled tool again")
    }
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
    let request = next_http(&mut server, &mut invocation).await;
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
    canonical_function_history(false).await;
}

#[tokio::test]
async fn observed_reasoning_survives_tool_refusal_next_dispatch_and_store_reopen() {
    canonical_function_history(true).await;
}

async fn canonical_function_history(refused: bool) {
    let mut server = LocalResponses::start().await;
    let files = Files::new();
    let database = files.0.join("callback-history.sqlite");
    let store = Arc::new(Store::open(&database).unwrap());
    let auth = files.auth(&server.endpoint);
    let client = ResponsesClient::new(auth.clone());
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
    let first = next_http(&mut server, &mut invocation).await;
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
    let reasoning = if refused {
        observed_reasoning()
    } else {
        reasoning()
    };
    first.respond(HttpReply::fragmented_sse(
        completed("with-call", &[reasoning.clone(), call.clone()]),
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
    let failure = json!({"kind":"compile_error","detail":"Not in scope: missingValue"});
    callback
        .complete(if refused {
            Err(ToolFailure::with_metadata(
                "Haskell compilation refused",
                failure.clone(),
            ))
        } else {
            Ok(json!({"echo":UNICODE}))
        })
        .unwrap();
    let second = next_http(&mut server, &mut invocation).await;
    let body = assert_deepseek_request(&second.captured);
    let input = body["input"].as_array().unwrap();
    assert_eq!(
        input
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .collect::<Vec<_>>(),
        vec![&reasoning]
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
    let output = serde_json::from_str::<Value>(outputs[0]["output"].as_str().unwrap()).unwrap();
    if refused {
        assert_eq!(output["failure"], failure);
        assert_eq!(output["error"], "tool failed: Haskell compilation refused");
        assert!(matches!(
            store
                .replay_tool_output_operation(&operation)
                .unwrap()
                .unwrap()
                .terminal,
            TerminalOutcome::Failure(_)
        ));
    } else {
        assert_eq!(output, json!({"echo":UNICODE}));
    }
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
    assert!(receipt.transcript.iter().any(|item| item.0 == reasoning));
    let head = receipt.requests.last().unwrap().clone();
    drop(invocation);
    drop(store);

    let reopened = Arc::new(Store::open(&database).unwrap());
    let identity = reopened.standalone_identity(operation.origin.actor().clone());
    let foreign = ContextProjectionTarget::component(
        "deepseek-flash",
        ResponsesRoute::ChatGptPlan,
        ResponsesProtocol::Standard,
    );
    assert!(matches!(
        reopened.preflight_context_target(&head, &identity, &foreign),
        Err(StoreError::Context(ContextError::Replay(
            ContextReplayRejection::IncompatibleTransport
        )))
    ));
    let mut resumed_config = config(vec![]);
    resumed_config.agent = operation.origin.actor().clone();
    let engine = Engine::<DeepSeekAuth, _, _>::with_transport(
        ResponsesClient::new(auth),
        reopened.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(NoTools),
        resumed_config,
    );
    let (_cancel, cancellation) = tokio::sync::watch::channel(false);
    let (_incoming, incoming) = tokio::sync::mpsc::unbounded_channel();
    let mut resumed = Box::pin(engine.run(
        Some(head),
        vec![Item::user_text("continue")],
        cancellation,
        incoming,
    ));
    let third = tokio::select! {
        result = &mut resumed => panic!("reopened history ended before dispatch: {result:?}"),
        request = server.next() => request,
    };
    let body = assert_deepseek_request(&third.captured);
    let replayed = body["input"].as_array().unwrap();
    assert_eq!(
        replayed
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .collect::<Vec<_>>(),
        vec![&reasoning]
    );
    assert_eq!(
        replayed
            .iter()
            .filter(|item| item["type"] == "function_call")
            .collect::<Vec<_>>(),
        vec![&call]
    );
    assert_eq!(
        replayed
            .iter()
            .filter(|item| item["type"] == "function_call_output")
            .map(|item| serde_json::from_str::<Value>(item["output"].as_str().unwrap()).unwrap())
            .collect::<Vec<_>>(),
        vec![output]
    );
    third.respond(HttpReply::fragmented_sse(
        completed("reopened", &[assistant("continued")]),
        &[3, 1, 2],
    ));
    let completion = tokio::time::timeout(Duration::from_secs(5), resumed)
        .await
        .unwrap()
        .unwrap();
    assert!(completion.transcript.iter().any(|item| item.0 == reasoning));
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
        let request = next_http(&mut server, &mut invocation).await;
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
    let request = next_http(&mut server, &mut invocation).await;
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

fn retained_observation(
    store: &Store,
    request: &harness::model::RequestId,
) -> harness::transport::TransportObservation {
    let events = store
        .events(Some(request))
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "responses_transport")
        .collect::<Vec<_>>();
    assert_eq!(
        events.len(),
        1,
        "one transport owner observation for the actual issued request"
    );
    let payload: Value = serde_json::from_str(&events[0].payload).unwrap();
    let retained = json!({"request_id":request.0,"observation":payload});
    let serialized = serde_json::to_string(&retained).unwrap();
    assert!(
        serialized.len() <= 16 * 1024,
        "synthetic metadata artifact must remain bounded"
    );
    assert!(!serialized.contains(SYNTHETIC_KEY));
    // Actual Store metadata is an artifact for the offline report consumer.
    // No request body, message content or key file is included.
    println!("DEEPSEEK_OFFLINE_TRANSPORT_OBSERVATION {serialized}");
    serde_json::from_value(payload).unwrap()
}

#[tokio::test]
async fn native_http_observation_distinguishes_requested_returned_and_raw_usage() {
    use harness::transport::{RawUsage, ResponseEnd, ResponseTerminal};
    let mut server = LocalResponses::start().await;
    let files = Files::new();
    let store = Arc::new(Store::memory().unwrap());
    let mut invocation = no_tools(
        files.auth(&server.endpoint),
        store.clone(),
        "retain metadata separately",
    );
    let request = next_http(&mut server, &mut invocation).await;
    assert_deepseek_request(&request.captured);
    let raw_usage = json!({"input_tokens":13,"output_tokens":7,"total_tokens":20,
        "input_tokens_details":{"cached_tokens":0},
        "output_tokens_details":{"reasoning_tokens":null},
        "offline_unknown_counter":{"preserved":77}});
    let item = assistant("observed");
    let events = vec![
        json!({"type":"response.created","sequence_number":0,"response":{"id":"observation-success","status":"in_progress"}}),
        json!({"type":"response.output_text.delta","sequence_number":1,"item_id":"message-1","output_index":0,"content_index":0,"delta":"observed"}),
        json!({"type":"response.output_item.done","sequence_number":2,"output_index":0,"item":item}),
        json!({"type":"response.completed","sequence_number":3,"response":{
            "id":"observation-success","status":"completed","output":[item],
            "model":"deepseek-flash-offline-reported","reasoning":{"effort":"low"},
            "max_output_tokens":17,"usage":raw_usage}}),
    ];
    request.respond(HttpReply::fragmented_sse(
        semantic_events(&events),
        &[1, 4, 2],
    ));
    let receipt = finished(&mut invocation).await;
    assert!(
        matches!(&receipt.outcome, Outcome::Text(text) if text == "observed"),
        "{:?}",
        receipt.outcome
    );
    let issued = &receipt.requests[0];
    assert_eq!(
        store.provider_response_state(issued).unwrap(),
        Some(harness::store::ProviderResponseState::Completed)
    );
    let observation = retained_observation(&store, issued);
    assert_eq!(observation.version, 3);
    assert_eq!(
        observation.requested_model.as_deref(),
        Some("deepseek-flash")
    );
    assert_eq!(observation.backend, Some(ResponsesRoute::DeepSeek));
    assert_eq!(observation.effective_reasoning, Some(Effort::High));
    assert_eq!(observation.max_output_tokens, Some(65_536));
    assert_eq!(
        observation.returned_model.as_deref(),
        Some("deepseek-flash-offline-reported")
    );
    assert_eq!(
        observation.returned_reasoning_effort.as_deref(),
        Some("low")
    );
    assert_eq!(observation.reported_max_output_tokens, Some(17));
    assert_eq!(observation.http_status, Some(200));
    assert_eq!(observation.dispatch_attempted, Some(true));
    assert_eq!(observation.terminal, Some(ResponseTerminal::Completed));
    assert_eq!(observation.end, ResponseEnd::ProviderTerminal);
    assert!(observation.started_at_unix_ms.is_some());
    assert!(observation.completed_at_unix_ms.is_some());
    assert!(observation.first_event_ms.is_some());
    assert!(observation.first_text_ms.is_some());
    assert!(observation.first_reasoning_ms.is_none());
    let usage = observation.usage.expect("provider returned usage metadata");
    assert_eq!(usage.input_tokens, Some(13));
    assert_eq!(usage.output_tokens, Some(7));
    assert_eq!(usage.cached_tokens, Some(0));
    assert_eq!(usage.cache_miss_tokens, None);
    assert_eq!(usage.cache_write_tokens, None);
    assert_eq!(usage.reasoning_tokens, None);
    assert_eq!(usage.raw, RawUsage::Observed(raw_usage));
    server.finish().await;
}

#[tokio::test]
async fn failed_http_observation_keeps_null_and_missing_usage_distinct_from_success() {
    use harness::transport::{RawUsage, ResponseEnd, ResponseTerminal};
    for explicit_null in [false, true] {
        let mut server = LocalResponses::start().await;
        let files = Files::new();
        let store = Arc::new(Store::memory().unwrap());
        let mut invocation = no_tools(
            files.auth(&server.endpoint),
            store.clone(),
            "failed metadata is not completion",
        );
        let request = next_http(&mut server, &mut invocation).await;
        assert_deepseek_request(&request.captured);
        let mut response = json!({"id":"observation-failed","status":"failed",
            "error":{"code":"offline_failed","message":"synthetic terminal failure"}});
        if explicit_null {
            response["usage"] = Value::Null;
        }
        request.respond(HttpReply::fragmented_sse(semantic_events(&[
            json!({"type":"response.created","sequence_number":0,"response":{"id":"observation-failed","status":"in_progress"}}),
            json!({"type":"response.failed","sequence_number":1,"response":response}),
        ]), &[3, 1, 4]));
        let receipt = finished(&mut invocation).await;
        assert!(
            matches!(receipt.outcome, Outcome::Failed(_)),
            "{:?}",
            receipt.outcome
        );
        let issued = &receipt.requests[0];
        assert_eq!(
            store.provider_response_state(issued).unwrap(),
            Some(harness::store::ProviderResponseState::Failed)
        );
        assert!(
            store
                .items(issued)
                .unwrap()
                .iter()
                .all(|item| item.0["role"] != "assistant")
        );
        let observation = retained_observation(&store, issued);
        assert_eq!(
            observation.requested_model.as_deref(),
            Some("deepseek-flash")
        );
        assert_eq!(observation.returned_model, None);
        assert_eq!(observation.terminal, Some(ResponseTerminal::ResponseFailed));
        assert_eq!(observation.end, ResponseEnd::ProviderTerminal);
        let usage = observation
            .usage
            .expect("terminal observation records whether usage was supplied");
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.output_tokens, None);
        assert_eq!(usage.cached_tokens, None);
        assert_eq!(usage.reasoning_tokens, None);
        assert_eq!(
            usage.raw,
            if explicit_null {
                RawUsage::Observed(Value::Null)
            } else {
                RawUsage::Unavailable
            }
        );
        server.finish().await;
    }
}
