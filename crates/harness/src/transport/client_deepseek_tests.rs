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
fn deepseek_preparation_retains_contract_and_projects_only_authored_policy() {
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
    assert_eq!(body["input"][0]["role"], "system");
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
    assert!(matches!(
        deepseek_request(
            prepared.request(),
            selected.contract().limits,
            &[false, false]
        ),
        Err(TransportError::UnsupportedSemantics(
            UnsupportedSemantics::Message
        ))
    ));
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
            json!({"type":"message","role":"developer","content":[{"type":"input_image","image_url":"unused"}]}),
            UnsupportedSemantics::Message,
        ),
        (
            json!({"type":"reasoning","content":[],"encrypted_content":"opaque"}),
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
            matches!(deepseek_request(&dto, limits, &[true]), Err(TransportError::UnsupportedSemantics(actual)) if actual == expected)
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
            deepseek_request(&dto, limits, &[false]).unwrap()["reasoning"]["effort"],
            serde_json::to_value(effort).unwrap()
        );
    }
    assert!(matches!(
        deepseek_request(
            &ResponsesRequest {
                pinned_effort: Effort::Medium,
                ..request()
            },
            limits,
            &[false]
        ),
        Err(TransportError::UnsupportedSemantics(
            UnsupportedSemantics::ReasoningEffort
        ))
    ));
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
    let prepared = prepare(&store, &queued, request(), selected.clone());
    let held = selected.acquire().await.unwrap();
    let second_client = ResponsesClient::new(auth);
    let mut pending = Box::pin(second_client.create(prepared));
    assert!(futures_util::poll!(&mut pending).is_pending());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(pending);
    drop(held);
    let _released = selected.acquire().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn deepseek_http_builder_uses_plain_json_without_openai_headers() {
    let selected = admission("/offline/key");
    let dto = request();
    let bytes =
        serde_json::to_vec(&deepseek_request(&dto, selected.contract().limits, &[false]).unwrap())
            .unwrap();
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
