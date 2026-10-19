use super::*;
use crate::{
    engine::ResponsesTransport,
    item::Item,
    model::{Effort, RequestId},
    store::{ProviderRequestPurpose, Store},
    transport::{
        CredentialReference, InferenceLimits, NativeTransportAdmission, NativeTransportContract,
        ResponsesClient, UnsupportedSemantics,
    },
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn admission(source: &str) -> NativeTransportAdmission {
    NativeTransportAdmission::rehydrate(NativeTransportContract {
        route: ResponsesRoute::DeepSeek,
        protocol: ResponsesProtocol::Standard,
        endpoint: DEEPSEEK_ENDPOINT.into(),
        credential: CredentialReference::File {
            path: source.into(),
        },
        limits: InferenceLimits {
            max_inflight: 1,
            ..InferenceLimits::deepseek_default()
        },
    })
    .unwrap()
}
fn request() -> ResponsesRequest {
    ResponsesRequest {
        input: vec![Item::user_text("λ\nhello")], instructions: "fixed".into(),
        tools: vec![json!({"type":"function","name":"echo","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}})].into(),
        tools_allowed: None, model: "deepseek-flash".into(), pinned_effort: Effort::Low,
        session_id: "session".into(), prompt_cache_comparison: None,
    }
}
fn prepare(
    store: &Store,
    head: &RequestId,
    request: ResponsesRequest,
    admission: NativeTransportAdmission,
) -> crate::store::PreparedProviderRequest {
    let occurrences = vec![None; request.input.len()];
    store
        .prepare_provider_request(
            head,
            head,
            request,
            &occurrences,
            None,
            super::super::PreparedTransportSelection::native(admission),
            ProviderRequestPurpose::Inference,
        )
        .unwrap()
}
#[derive(Clone)]
struct CountingAuth {
    admission: NativeTransportAdmission,
    calls: Arc<AtomicUsize>,
}
impl Auth for CountingAuth {
    fn admission(&self) -> Result<NativeTransportAdmission, TransportError> {
        Ok(self.admission.clone())
    }
    fn access(&self) -> Result<String, TransportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(TransportError::Authentication)
    }
}

#[test]
fn deepseek_preparation_retains_contract_and_native_developer_role() {
    let store = Store::memory().unwrap();
    let head = RequestId("deepseek-prepared".into());
    store.create_request(&head, None, "/root").unwrap();
    let mut dto = request();
    dto.input.insert(
        0,
        Item(json!({"type":"message","role":"developer","content":"positional policy"})),
    );
    let original = serde_json::to_value(&dto).unwrap();
    let selected = admission("/offline/selected-key");
    let prepared = prepare(&store, &head, dto, selected.clone());
    let body: Value = serde_json::from_slice(prepared.wire_body()).unwrap();
    assert_eq!(body["input"][0], original["input"][0]);
    assert_eq!(body["input"][1]["content"], original["input"][1]["content"]);
    assert_eq!(serde_json::to_value(prepared.request()).unwrap(), original);
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["max_output_tokens"], 65_536);
    for absent in [
        "include",
        "store",
        "prompt_cache_key",
        "prompt_cache_options",
        "previous_response_id",
        "parallel_tool_calls",
    ] {
        assert!(body.get(absent).is_none(), "unexpected {absent}");
    }
    assert_eq!(
        store.native_transport_contract(&head).unwrap().as_ref(),
        Some(selected.contract())
    );
    let restored = NativeTransportAdmission::rehydrate(
        store.native_transport_contract(&head).unwrap().unwrap(),
    )
    .unwrap();
    assert_eq!(restored.contract(), selected.contract());
}

#[test]
fn deepseek_preserves_native_message_roles_and_developer_images() {
    let limits = InferenceLimits::deepseek_default();
    let messages = ["system", "developer", "user", "assistant"]
        .map(|role| json!({"type":"message", "role":role, "content":format!("{role} message")}));
    let dto = ResponsesRequest {
        input: messages.iter().cloned().map(Item).collect(),
        ..request()
    };
    assert_eq!(
        deepseek_request(&dto, limits).unwrap()["input"],
        json!(messages)
    );
    for role in ["user", "developer"] {
        let message = json!({"type":"message", "role":role, "content":[
            {"type":"input_text", "text":"image context"},
            {"type":"input_image", "image_url":"https://example.invalid/fixture.png"}
        ]});
        let dto = ResponsesRequest {
            input: vec![Item(message.clone())],
            ..request()
        };
        assert_eq!(
            deepseek_request(&dto, limits).unwrap()["input"],
            json!([message])
        );
    }
}

#[test]
fn deepseek_refuses_unsupported_semantics_and_invalid_endpoints() {
    let selected = admission("/offline/key");
    let limits = selected.contract().limits;
    for (item, expected) in [
        (
            json!({"type":"message","role":"alien","content":"x"}),
            UnsupportedSemantics::Message,
        ),
        (
            json!({"type":"message","role":"system","content":[{"type":"input_image","image_url":"unused"}]}),
            UnsupportedSemantics::Message,
        ),
        (
            json!({"type":"message","role":"assistant","content":[{"type":"input_image","image_url":"unused"}]}),
            UnsupportedSemantics::Message,
        ),
        (
            json!({"type":"reasoning","content":[],"encrypted_content":"opaque"}),
            UnsupportedSemantics::ReasoningContent,
        ),
        (
            json!({"type":"reasoning","content":[{"type":"reasoning_text","text":"plaintext"}],"encrypted_content":{}}),
            UnsupportedSemantics::ReasoningContent,
        ),
        (
            json!({"type":"reasoning","content":[{"type":"reasoning_text","text":"plaintext"}],"summary":[{"type":"summary_text","text":"unsupported summary"}]}),
            UnsupportedSemantics::ReasoningContent,
        ),
        (
            json!({"type":"reasoning","content":[],"summary":[{"text":"not plaintext reasoning"}]}),
            UnsupportedSemantics::ReasoningContent,
        ),
        (
            json!({"type":"function_call","namespace":"functions","name":"echo","arguments":"{}","call_id":"id"}),
            UnsupportedSemantics::InputItem,
        ),
        (
            json!({"type":"custom_tool_call","name":"haskell","input":"pure ()","call_id":"id"}),
            UnsupportedSemantics::InputItem,
        ),
    ] {
        let dto = ResponsesRequest {
            input: vec![Item(item)],
            ..request()
        };
        assert!(
            matches!(deepseek_request(&dto, limits), Err(TransportError::UnsupportedSemantics(actual)) if actual == expected)
        );
    }
    for endpoint in [
        "http://api.deepseek.com/responses",
        "https://key@api.deepseek.com/responses",
        "https://api.deepseek.com/responses?key=x",
        "https://api.deepseek.com/responses#fragment",
        CHATGPT_PLAN_ENDPOINT,
    ] {
        let mut contract = selected.contract().clone();
        contract.endpoint = endpoint.into();
        assert!(matches!(
            NativeTransportAdmission::rehydrate(contract),
            Err(TransportError::InvalidAdmission)
        ));
    }
    for effort in [Effort::None, Effort::Low, Effort::High, Effort::Max] {
        let dto = ResponsesRequest {
            pinned_effort: effort,
            ..request()
        };
        assert_eq!(
            deepseek_request(&dto, limits).unwrap()["reasoning"]["effort"],
            serde_json::to_value(effort).unwrap()
        );
    }
    assert!(matches!(
        deepseek_request(
            &ResponsesRequest {
                pinned_effort: Effort::Medium,
                ..request()
            },
            limits
        ),
        Err(TransportError::UnsupportedSemantics(
            UnsupportedSemantics::ReasoningEffort
        ))
    ));
}

#[test]
fn deepseek_preserves_plaintext_reasoning_optional_metadata() {
    let mut original = json!({"type":"reasoning","id":"reasoning-1","status":"completed",
        "content":[{"type":"reasoning_text","text":"provider plaintext"}],"summary":[]});
    for metadata in [
        None,
        Some(Value::Null),
        Some(json!("provider-owned-metadata")),
    ] {
        if let Some(metadata) = metadata {
            original["encrypted_content"] = metadata;
        }
        let dto = ResponsesRequest {
            input: vec![Item(original.clone())],
            ..request()
        };
        let body = deepseek_request(&dto, InferenceLimits::deepseek_default()).unwrap();
        assert_eq!(body["input"], json!([original]));
        assert_eq!(dto.input[0].0, original);
    }
    for original in [
        json!({"type":"reasoning","content":[]}),
        json!({"type":"reasoning","content":[],"encrypted_content":null}),
    ] {
        let dto = ResponsesRequest {
            input: vec![Item(original.clone())],
            ..request()
        };
        assert_eq!(
            deepseek_request(&dto, InferenceLimits::deepseek_default()).unwrap()["input"],
            json!([original])
        );
    }
}

#[test]
fn openai_default_wire_bytes_preserve_original_struct_serializer() {
    let dto = ResponsesRequest {
        input: vec![Item::user_text("hello")],
        tools: vec![].into(),
        model: "gpt-6.1-sol".into(),
        ..request()
    };
    let wire = normalize_for_dispatch(
        &dto,
        (ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard),
    )
    .unwrap();
    assert_eq!(
        std::str::from_utf8(wire.bytes()).unwrap(),
        r#"{"model":"gpt-6.1-sol","instructions":"fixed","input":[{"content":"hello","role":"user","type":"message"}],"reasoning":{"effort":"low","summary":"auto"},"stream":true,"store":false,"prompt_cache_key":"session","include":["reasoning.encrypted_content"]}"#
    );
}

#[tokio::test]
async fn deepseek_contract_mismatch_precedes_credentials_and_queue_cancellation_cannot_dispatch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let selected = admission("/offline/selected-key");
    let auth = CountingAuth {
        admission: selected.clone(),
        calls: calls.clone(),
    };
    let client = ResponsesClient::new(auth.clone());
    assert_eq!(
        client.response_contract().unwrap().0,
        ResponsesRoute::DeepSeek
    );
    let store = Store::memory().unwrap();
    let wrong = RequestId("wrong-credential-source".into());
    store.create_request(&wrong, None, "/root").unwrap();
    let prepared = prepare(&store, &wrong, request(), admission("/offline/other-key"));
    assert!(matches!(
        client.create(prepared).await,
        Err(TransportError::PreparedSelectionMismatch)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let queued = RequestId("queued".into());
    store.create_request(&queued, None, "/root").unwrap();
    let mut prepared = prepare(&store, &queued, request(), selected.clone());
    let observation = prepared.take_transport_observation().unwrap();
    assert!(prepared.take_transport_observation().is_none());
    let held = selected.acquire().await.unwrap();
    let second_client = ResponsesClient::new(auth);
    let mut pending = Box::pin(second_client.create(prepared));
    assert!(futures_util::poll!(&mut pending).is_pending());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(pending);
    let observed = observation.await.unwrap();
    assert_eq!(observed.end, super::super::ResponseEnd::Abandoned);
    assert_eq!(observed.backend, Some(ResponsesRoute::DeepSeek));
    assert_eq!(observed.dispatch_attempted, Some(false));
    assert_eq!(observed.terminal, None);
    assert!(observed.queue_ms.is_some());
    assert_eq!(observed.first_event_ms, None);
    assert_eq!(observed.usage, None);
    drop(held);
    let _released = selected.acquire().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[derive(Clone)]
struct PanickingAuth(NativeTransportAdmission);
impl Auth for PanickingAuth {
    fn admission(&self) -> Result<NativeTransportAdmission, TransportError> {
        Ok(self.0.clone())
    }
    fn access(&self) -> Result<String, TransportError> {
        panic!("synthetic credential worker failure")
    }
}

#[tokio::test]
async fn credential_worker_failure_preserves_route_error_and_observation_without_dispatch() {
    for route in [ResponsesRoute::ChatGptPlan, ResponsesRoute::DeepSeek] {
        let mut contract = admission("/offline/synthetic-key").contract().clone();
        contract.route = route;
        if route == ResponsesRoute::ChatGptPlan {
            contract.endpoint = CHATGPT_PLAN_ENDPOINT.into();
            contract.limits = InferenceLimits::chatgpt_default();
        }
        let selected = NativeTransportAdmission::rehydrate(contract).unwrap();
        let store = Store::memory().unwrap();
        let head = RequestId(format!("worker-failure-{route:?}"));
        store.create_request(&head, None, "/root").unwrap();
        let mut prepared = prepare(&store, &head, request(), selected.clone());
        let receipt = prepared.take_transport_observation().unwrap();
        let error = ResponsesClient::new(PanickingAuth(selected))
            .create(prepared)
            .await
            .unwrap_err();
        match route {
            ResponsesRoute::GeminiInteractions => {
                unreachable!("this credential fixture selects Responses providers")
            }
            ResponsesRoute::ChatGptPlan => assert!(matches!(error, TransportError::Authentication)),
            ResponsesRoute::DeepSeek => assert!(matches!(
                error,
                TransportError::CredentialUnavailable {
                    route: ResponsesRoute::DeepSeek,
                    reason: super::super::CredentialFailure::WorkerFailed
                }
            )),
        }
        let observed = receipt.await.unwrap();
        assert_eq!(
            observed.end,
            super::super::ResponseEnd::AuthenticationFailed
        );
        assert_eq!(observed.backend, Some(route));
        assert_eq!(observed.dispatch_attempted, Some(false));
        assert_eq!(observed.terminal, None);
        assert_eq!(observed.usage, None);
        assert!(observed.total_ms.is_some());
    }
}

#[test]
fn deepseek_http_builder_uses_plain_json_without_openai_headers() {
    let selected = admission("/offline/key");
    let dto = request();
    let bytes =
        serde_json::to_vec(&deepseek_request(&dto, selected.contract().limits).unwrap()).unwrap();
    let wire = native_http_request(
        &reqwest::Client::new(),
        &dto,
        bytes.clone(),
        "synthetic-token",
        selected.contract(),
    )
    .build()
    .unwrap();
    assert_eq!(wire.url().as_str(), "https://api.deepseek.com/responses");
    assert_eq!(
        wire.headers()[reqwest::header::AUTHORIZATION],
        "Bearer synthetic-token"
    );
    assert_eq!(wire.body().unwrap().as_bytes().unwrap(), bytes);
    for absent in [
        "session-id",
        "thread-id",
        "x-client-request-id",
        "chatgpt-account-id",
        "content-encoding",
    ] {
        assert!(!wire.headers().contains_key(absent), "unexpected {absent}");
    }
}

#[tokio::test]
async fn deepseek_loopback_is_canonical_bound_and_cannot_reinterpret_prepared_requests() {
    let selected = admission("/offline/selected-key");
    for endpoint in [
        "http://127.0.0.1:12345/responses",
        "http://[::1]:12345/responses",
    ] {
        let mut contract = selected.contract().clone();
        contract.endpoint = endpoint.into();
        let encoded = serde_json::to_value(&contract).unwrap();
        assert_eq!(encoded["endpoint"], endpoint);
        let restored =
            NativeTransportAdmission::rehydrate(serde_json::from_value(encoded).unwrap()).unwrap();
        assert_eq!(restored.contract(), &contract);
    }
    for endpoint in [
        "http://localhost:12345/responses",
        "http://127.1:12345/responses",
        "http://2130706433:12345/responses",
        "http://127.0.0.1:0/responses",
        "http://127.0.0.1/responses",
        "http://127.0.0.1:80/responses",
        "http://[::01]:12345/responses",
        "http://192.0.2.1:12345/responses",
        "http://127.0.0.1:12345/other",
        "http://127.0.0.1:12345/responses?x=1",
        "http://127.0.0.1:12345/responses#x",
        "http://token@127.0.0.1:12345/responses",
        "https://127.0.0.1:12345/responses",
    ] {
        let mut contract = selected.contract().clone();
        contract.endpoint = endpoint.into();
        assert!(
            matches!(
                NativeTransportAdmission::rehydrate(contract),
                Err(TransportError::InvalidAdmission)
            ),
            "accepted {endpoint}"
        );
    }
    let mut local = selected.contract().clone();
    local.endpoint = "http://127.0.0.1:12345/responses".into();
    let local = NativeTransportAdmission::rehydrate(local).unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let receiver = ResponsesClient::new(CountingAuth {
        admission: selected,
        calls: calls.clone(),
    });
    let store = Store::memory().unwrap();
    let head = RequestId("endpoint-swap".into());
    store.create_request(&head, None, "/root").unwrap();
    let prepared = prepare(&store, &head, request(), local.clone());
    assert_eq!(
        store.native_transport_contract(&head).unwrap().as_ref(),
        Some(local.contract())
    );
    assert!(matches!(
        receiver.create(prepared).await,
        Err(TransportError::PreparedSelectionMismatch)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut openai = NativeTransportAdmission::chatgpt_plan("/offline/openai".into())
        .unwrap()
        .contract()
        .clone();
    openai.endpoint = "http://127.0.0.1:12345/responses".into();
    assert!(matches!(
        NativeTransportAdmission::rehydrate(openai),
        Err(TransportError::InvalidAdmission)
    ));
}
