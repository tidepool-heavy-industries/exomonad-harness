use super::*;
use crate::transport::ResponsesClient;
use crate::{item::Item, model::Effort};
use serde_json::json;

fn plan_request() -> ResponsesRequest {
    ResponsesRequest {
        input: vec![
            Item::configuration_update(Effort::Low),
            Item(json!({"type":"message","role":"user","content":"Start work"})),
            Item(json!({"type":"custom_tool_call","call_id":"pending","name":"cell","namespace":"functions","async":true,"input":"work"})),
            Item::configuration_update(Effort::High),
            Item(json!({"type":"function_call","call_id":"settled","name":"yield","arguments":"{}"})),
            Item(json!({"type":"function_call_output","call_id":"settled","output":"ready"})),
        ],
        instructions: "Persistent shared instructions".into(),
        tools: vec![
            json!({"type":"custom","name":"cell","description":"Execute a Haskell cell","async":true}),
            json!({"type":"function","name":"yield","description":"Wait for work","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ].into(),
        tools_allowed: None,
        model: "gpt-6.1-sol".into(),
        pinned_effort: Effort::Low,
        session_id: "plan-production-request".into(),
        prompt_cache_comparison: None,
    }
}

#[test]
fn public_production_request_preserves_async_and_full_history() {
    let request = plan_request();
    let original = serde_json::to_value(&request).unwrap();
    let normalized = normalized_request(&request).unwrap();
    let wire = http_request(
        &reqwest::Client::new(),
        &request,
        serde_json::to_vec(&normalized).unwrap(),
        "test-bearer",
    )
    .build()
    .unwrap();
    assert_eq!(wire.url().as_str(), CHATGPT_PLAN_ENDPOINT);
    assert_eq!(
        wire.headers()[reqwest::header::AUTHORIZATION],
        "Bearer test-bearer"
    );
    for header in [
        "chatgpt-account-id",
        "version",
        "originator",
        "x-openai-internal-codex-responses-lite",
    ] {
        assert!(!wire.headers().contains_key(header));
    }
    for header in ["session-id", "thread-id", "x-client-request-id"] {
        assert_eq!(wire.headers()[header], request.session_id.thread());
    }
    assert_eq!(
        wire.headers()[reqwest::header::USER_AGENT],
        "exomonad-harness/0.1.0"
    );
    let body: Value = serde_json::from_slice(wire.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["reasoning"]["effort"], "high");
    assert!(body["reasoning"].get("context").is_none());
    assert_eq!(body["instructions"], request.instructions);
    assert_eq!(body["tools"][0]["type"], "namespace");
    assert_eq!(body["tools"][0]["name"], "functions");
    assert_eq!(body["tools"][0]["tools"][0]["async"], true);
    assert!(body["tools"][0]["tools"][1].get("async").is_none());
    let expected: Vec<_> = request
        .input
        .iter()
        .filter(|item| !item.is_configuration_update())
        .cloned()
        .collect();
    assert_eq!(body["input"], serde_json::to_value(expected).unwrap());
    assert_eq!(body["input"][1]["call_id"], "pending");
    assert_eq!(body["input"][1]["async"], true);
    for field in [
        "previous_response_id",
        "background",
        "max_output_tokens",
        "conversation",
        "metadata",
    ] {
        assert!(body.get(field).is_none());
    }
    assert_eq!(serde_json::to_value(&request).unwrap(), original);
}

#[test]
fn public_route_enforces_its_wire_contract_and_allowed_host_tools() {
    let mut request = plan_request();
    request.tools_allowed = Some(vec!["yield".into()]);
    let body = request_body(&request).unwrap();
    assert_eq!(body["tools"][0]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["tools"][0]["name"], "yield");
    request.tools_allowed = Some(vec![]);
    let body = request_body(&request).unwrap();
    assert!(body.get("tool_choice").is_none());
    assert!(body.get("tools").is_none());
    request.input.push(Item(
        json!({"type":"message","role":"system","content":"bad"}),
    ));
    assert!(request_body(&request).is_err());
    request.input.pop();
    request.tools = vec![json!({"type":"tool_search"})].into();
    assert!(request_body(&request).is_err());
}

#[tokio::test]
async fn missing_plan_credentials_are_refused_before_traffic() {
    #[derive(Clone)]
    struct WrongAuthority;
    impl Auth for WrongAuthority {
        fn access(&self) -> Result<String, TransportError> {
            Err(TransportError::Authentication)
        }
    }
    let client = ResponsesClient::new(WrongAuthority);
    let selection = crate::engine::ResponsesTransport::prepare_submission(&client).unwrap();
    let store = crate::store::Store::memory().unwrap();
    let head = crate::model::RequestId("wrong-authority".into());
    store.create_request(&head, None, "/root").unwrap();
    let request = ResponsesRequest {
        input: vec![],
        ..plan_request()
    };
    let prepared = store
        .prepare_provider_request(
            &head,
            &head,
            request,
            &[],
            None,
            selection,
            crate::store::ProviderRequestPurpose::Inference,
        )
        .unwrap();
    assert_eq!(prepared.request_id(), &head);
    assert_eq!(
        prepared.response_contract(),
        (ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard)
    );
    assert!(matches!(
        client.create(prepared).await,
        Err(TransportError::Authentication)
    ));
    assert_eq!(
        store.provider_response_state(&head).unwrap(),
        Some(crate::store::ProviderResponseState::Streaming)
    );
}

#[test]
fn streamed_async_calls_retain_the_original_item_and_call_id() {
    let original = json!({"type":"custom_tool_call","id":"item-1","namespace":"functions","name":"cell","call_id":"original-call","input":"work","async":true,"provider_extension":"kept"});
    let mut assembly = ResponseAssembly::default();
    let event = assembly
        .accept(
            &json!({"type":"response.output_item.done","output_index":0,"item":original})
                .to_string(),
        )
        .unwrap();
    assert!(
        matches!(event, Some(StreamEvent::CompletedItem(ref item)) if item.item().0 == original)
    );
    assembly
        .accept(
            &json!({"type":"response.completed","response":{"id":"response-1","output":[]}})
                .to_string(),
        )
        .unwrap();
    let turn = assembly.finish().unwrap();
    assert_eq!(turn.items[0].0, original);
    let call = turn.items[0].tool_call().unwrap().unwrap();
    assert_eq!(call.call_id.0, "original-call");
}

#[tokio::test]
async fn production_http_client_never_forwards_history_on_redirect() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = first.read(&mut request).await.unwrap();
        first.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{address}/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        drop(first);
        match tokio::time::timeout(Duration::from_millis(100), listener.accept()).await {
            Err(_) => false,
            Ok(Ok((mut redirected, _))) => {
                let _ = redirected.read(&mut request).await;
                let _ = redirected
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .await;
                true
            }
            Ok(Err(error)) => panic!("redirect fixture failed: {error}"),
        }
    });
    let response = http_client()
        .unwrap()
        .post(format!("http://{address}/original"))
        .body("private request history")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
    assert!(
        !server.await.unwrap(),
        "history was sent to the redirect target"
    );
}

#[test]
fn cache_comparison_defaults_omit_and_invalid_selections_refuse_before_dispatch() {
    let mut request = plan_request();
    let body = request_body(&request).unwrap();
    assert!(body.get("prompt_cache_options").is_none());
    request.prompt_cache_comparison = Some(crate::transport::PromptCacheComparison {
        comparison_response_id: "baseline".into(),
    });
    let body = request_body(&request).unwrap();
    assert_eq!(
        body["prompt_cache_options"],
        json!({"comparison_response_id":"baseline"})
    );
    request
        .prompt_cache_comparison
        .as_mut()
        .unwrap()
        .comparison_response_id
        .clear();
    assert!(request_body(&request).is_err());
}
