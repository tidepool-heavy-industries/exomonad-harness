use super::{
    Auth, AuthCredentials, ResponsesProtocol, ResponsesRequest, ResponsesRoute, ResponsesTurn,
    TransportError,
    sse::{ResponseAssembly, StreamEvent},
};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::{borrow::Cow, time::Duration};

pub const ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const CHATGPT_PLAN_ENDPOINT: &str = "https://api.openai.com/v1/responses";
pub const CODEX_VERSION: &str = "0.160.0";
const RESPONSES_LITE_HEADER: &str = "x-openai-internal-codex-responses-lite";

#[cfg(test)]
#[path = "client_portability_tests.rs"]
mod portability_tests;

/// Only this transport builds the backend request; the stable session id is
/// shared with prompt_cache_key, while caller controls the complete stateless
/// item window. Never send previous_response_id or store:true.
pub fn request_body(request: &ResponsesRequest) -> Result<Value, TransportError> {
    request_body_for_protocol(request, ResponsesProtocol::Standard)
}

/// Serialize the selected client wire contract without credentials or traffic.
pub fn request_body_for_protocol(
    request: &ResponsesRequest,
    protocol: ResponsesProtocol,
) -> Result<Value, TransportError> {
    serde_json::to_value(normalized_request(
        request,
        protocol,
        ResponsesRoute::Codex,
    )?)
    .map_err(|_| TransportError::Stream("request serialization failed".into()))
}

/// The native plan route uses the public Standard contract and retains host
/// async tool declarations. This is also the serializer used for real traffic.
pub fn request_body_for_route(
    request: &ResponsesRequest,
    route: ResponsesRoute,
) -> Result<Value, TransportError> {
    serde_json::to_value(normalized_request(
        request,
        ResponsesProtocol::Standard,
        route,
    )?)
    .map_err(|_| TransportError::Stream("request serialization failed".into()))
}

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<&'a str>,
    input: Cow<'a, [crate::item::Item]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<RequestTools<'a>>,
    tool_choice: ToolChoice<'a>,
    parallel_tool_calls: bool,
    reasoning: Reasoning,
    stream: bool,
    store: bool,
    prompt_cache_key: &'a str,
    include: [&'static str; 1],
}

#[derive(Serialize)]
#[serde(untagged)]
enum RequestTools<'a> {
    Flat(super::StrictToolManifest<'a>),
    Namespaced(Vec<Value>),
}

#[derive(Serialize)]
struct Reasoning {
    effort: crate::model::Effort,
    summary: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ToolChoice<'a> {
    Automatic(&'static str),
    Allowed {
        #[serde(rename = "type")]
        kind: &'static str,
        mode: &'static str,
        tools: AllowedTools<'a>,
    },
}

struct AllowedTools<'a>(&'a [String]);
impl Serialize for AllowedTools<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        #[derive(Serialize)]
        struct AllowedTool<'a> {
            #[serde(rename = "type")]
            kind: &'static str,
            name: &'a str,
        }
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for name in self.0 {
            sequence.serialize_element(&AllowedTool {
                kind: "function",
                name,
            })?;
        }
        sequence.end()
    }
}

fn normalized_request(
    request: &ResponsesRequest,
    protocol: ResponsesProtocol,
    route: ResponsesRoute,
) -> Result<RequestBody<'_>, TransportError> {
    if request.model.is_empty() || request.session_id.is_empty() {
        return Err(TransportError::Stream("empty model or session id".into()));
    }
    let admitted_tools = request.tools.strict_tools()?;
    let lite = protocol == ResponsesProtocol::Lite;
    let public = route == ResponsesRoute::ChatGptPlan;
    if public && lite {
        return Err(TransportError::Stream(
            "ChatGPT plan usage requires the public Standard contract".into(),
        ));
    }
    let input = if lite {
        Cow::Owned(lite_input(request)?)
    } else if public {
        if request.input.iter().any(|item| item.0["role"] == "system") {
            return Err(TransportError::Stream(
                "ChatGPT plan input requires instructions or developer messages".into(),
            ));
        }
        Cow::Owned(
            request
                .input
                .iter()
                .filter(|item| !item.is_configuration_update())
                .cloned()
                .collect(),
        )
    } else {
        Cow::Borrowed(request.input.as_slice())
    };
    Ok(RequestBody {
        model: &request.model,
        instructions: (!lite).then_some(request.instructions.as_str()),
        input,
        tools: if lite {
            None
        } else if public {
            let mut tools = Vec::new();
            for tool in request.tools.iter() {
                if !matches!(tool["type"].as_str(), Some("function" | "custom")) {
                    return Err(TransportError::Stream(
                        "ChatGPT plan route requires function or custom host tools".into(),
                    ));
                }
                if request
                    .tools_allowed
                    .as_ref()
                    .is_none_or(|allowed| allowed.iter().any(|name| tool["name"] == name.as_str()))
                {
                    tools.push(tool.clone());
                }
            }
            Some(RequestTools::Namespaced(if tools.is_empty() {
                vec![]
            } else {
                vec![
                    serde_json::json!({"type":"namespace","name":"functions","description":"Exomonad host tools","tools":tools}),
                ]
            }))
        } else {
            Some(RequestTools::Flat(admitted_tools))
        },
        tool_choice: match request.tools_allowed.as_deref() {
            None => ToolChoice::Automatic("auto"),
            Some([]) => ToolChoice::Automatic("none"),
            Some(_) if lite || public => ToolChoice::Automatic("auto"),
            Some(names) => ToolChoice::Allowed {
                kind: "allowed_tools",
                mode: "auto",
                tools: AllowedTools(names),
            },
        },
        parallel_tool_calls: !lite,
        reasoning: Reasoning {
            // Project local effort controls into the selected request field;
            // the original durable history remains immutable.
            effort: if lite || public {
                request
                    .input
                    .iter()
                    .rev()
                    .find_map(crate::item::Item::configuration_effort)
                    .unwrap_or(request.pinned_effort)
            } else {
                request.pinned_effort
            },
            summary: "auto",
            context: lite.then_some("all_turns"),
        },
        stream: true,
        store: false,
        prompt_cache_key: &request.session_id,
        include: ["reasoning.encrypted_content"],
    })
}

// The backend caps item IDs at 64 characters. A UUID-size content digest
// preserves stable identities without adding hashing dependencies or a registry.
fn content_id(prefix: &str, payload: &[u8]) -> String {
    let digest = blake3::hash(payload);
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    format!("{prefix}_{}", uuid::Uuid::from_bytes(bytes))
}

fn lite_input(request: &ResponsesRequest) -> Result<Vec<crate::item::Item>, TransportError> {
    use crate::item::Item;
    let mut tools = Vec::new();
    for tool in request.tools.iter() {
        if request
            .tools_allowed
            .as_ref()
            .is_some_and(|allowed| !allowed.iter().any(|name| tool["name"] == name.as_str()))
        {
            continue;
        }
        if !matches!(tool["type"].as_str(), Some("function" | "custom")) {
            return Err(TransportError::Stream(
                "Lite requires function or custom host tools".into(),
            ));
        }
        let mut tool = tool.clone();
        // Async ownership belongs to Engine, independently of wire scheduling.
        tool.as_object_mut()
            .expect("admitted tool declaration")
            .remove("async");
        tools.push(tool);
    }
    let tools = if tools.is_empty() {
        serde_json::json!([])
    } else {
        serde_json::json!([{"type":"namespace","name":"functions","description":"","tools":tools}])
    };
    let bytes = serde_json::to_vec(&tools)
        .map_err(|_| TransportError::Stream("tool serialization failed".into()))?;
    let mut input = vec![Item(
        serde_json::json!({"type":"additional_tools","role":"developer","id":content_id("at", &bytes),"tools":tools}),
    )];
    if !request.instructions.is_empty() {
        input.push(Item(serde_json::json!({"type":"message","role":"developer","id":content_id("msg", request.instructions.as_bytes()),"content":[{"type":"input_text","text":request.instructions}],"internal_chat_message_metadata_passthrough":{"content_item_kinds":["model.base_instructions"]}})));
    }
    input.extend(
        request
            .input
            .iter()
            .filter(|item| !item.is_configuration_update())
            .cloned(),
    );
    Ok(input)
}

/// A small SSE framer for the Codex endpoint. We keep it here rather than
/// splitting by network chunk or by single line: one event may contain
/// multiple `data:` lines and span arbitrary chunks. The caller feeds events
/// to `ResponseAssembly` without waiting for the stream to end.
struct SseFramer {
    line: Vec<u8>,
    data: Vec<String>,
}

impl SseFramer {
    fn new() -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, TransportError> {
        let mut events = Vec::new();
        for byte in chunk {
            if *byte != b'\n' {
                self.line.push(*byte);
                if self.line.len() > 1024 * 1024 {
                    return Err(TransportError::Stream("SSE line too large".into()));
                }
                continue;
            }
            if self.line.last() == Some(&b'\r') {
                self.line.pop();
            }
            let line = std::str::from_utf8(&self.line)
                .map_err(|_| TransportError::Stream("invalid SSE UTF-8".into()))?;
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
            }
            self.line.clear();
        }
        Ok(events)
    }
}

fn protocol_headers(protocol: ResponsesProtocol) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    if protocol == ResponsesProtocol::Lite {
        headers.insert(
            RESPONSES_LITE_HEADER,
            reqwest::header::HeaderValue::from_static("true"),
        );
    }
    headers
}

fn http_request(
    http: &reqwest::Client,
    route: ResponsesRoute,
    protocol: ResponsesProtocol,
    request: &ResponsesRequest,
    body: &RequestBody<'_>,
    token: &str,
    account: Option<&str>,
) -> reqwest::RequestBuilder {
    let call = http
        .post(match route {
            ResponsesRoute::Codex => ENDPOINT,
            ResponsesRoute::ChatGptPlan => CHATGPT_PLAN_ENDPOINT,
        })
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .headers(protocol_headers(protocol))
        .json(body);
    if let Some(account) = account {
        call.header("chatgpt-account-id", account)
            .header("version", CODEX_VERSION)
            .header("originator", "codex_cli_rs")
            .header("session-id", &request.session_id)
    } else {
        call
    }
}

fn http_client() -> Result<reqwest::Client, TransportError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| TransportError::Stream("HTTP client initialization failed".into()))
}

/// Runs the stateless streaming request. Auth disk access happens in a
/// blocking-pool task, never on an async executor worker. No credential value
/// is included in errors, traces, stored items, or returned data.
pub(super) async fn execute<A: Auth + Clone + 'static>(
    auth: A,
    protocol: ResponsesProtocol,
    request: ResponsesRequest,
    sink: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
) -> Result<ResponsesTurn, TransportError> {
    let route = auth.route();
    let body = normalized_request(&request, protocol, route)?;
    let credentials = tokio::task::spawn_blocking(move || auth.credentials())
        .await
        .map_err(|_| TransportError::Authentication)??;
    let http = http_client()?;
    let (token, account) = match (route, credentials) {
        (
            ResponsesRoute::Codex,
            AuthCredentials::Codex {
                access_token,
                account_id,
            },
        ) => (access_token, Some(account_id)),
        (ResponsesRoute::ChatGptPlan, AuthCredentials::ChatGptPlan { access_token }) => {
            (access_token, None)
        }
        _ => return Err(TransportError::Authentication),
    };
    let call = http_request(
        &http,
        route,
        protocol,
        &request,
        &body,
        &token,
        account.as_deref(),
    );
    let response = call
        .send()
        .await
        .map_err(|_| TransportError::Stream("HTTP request failed".into()))?;
    let status = response.status().as_u16();
    if status == 401 {
        return Err(TransportError::Authentication);
    }
    if status != 200 {
        let diagnostic =
            super::http_error::read(response, &token, account.as_deref().unwrap_or("")).await;
        return Err(TransportError::Http { status, diagnostic });
    }
    read_response(response, sink, &token, account.as_deref().unwrap_or("")).await
}

async fn read_response(
    response: reqwest::Response,
    sink: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
    token: &str,
    account: &str,
) -> Result<ResponsesTurn, TransportError> {
    let mut stream = response.bytes_stream();
    let mut framer = SseFramer::new();
    let mut assembly = ResponseAssembly::default();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                // Body-read errors contain transport causes, never provider body data.
                // Remove the request URL before retaining a bounded diagnostic.
                let diagnostic: String = format!("{:?}", error.without_url())
                    .chars()
                    .take(512)
                    .map(|c| if c.is_control() { ' ' } else { c })
                    .collect();
                tracing::warn!(error = %diagnostic, "provider response body read failed");
                // A valid completion remains authoritative even if the HTTP body
                // fails afterwards. Otherwise retain the transport interruption cause.
                return assembly.finish().map_err(|_| {
                    TransportError::IncompleteResponse(super::StreamInterruption::ReadFailed)
                });
            }
        };
        for data in framer.push(&chunk)? {
            if let Some(event) = assembly.accept_with_credentials(&data, token, account)? {
                if let Some(sender) = &sink {
                    // Bounded backpressure preserves every provider delta and completed item.
                    // A closed consumer has stopped; the final turn remains authoritative.
                    let _ = sender.send(event).await;
                }
            }
        }
    }
    assembly.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        item::Item,
        model::Effort,
        transport::{ResponsesClient, auth::CodexFileAuth},
    };

    #[test]
    fn request_is_stateless_and_pins_effort() {
        let request = ResponsesRequest {
            input: vec![Item(json!({"role":"user","content":"hello"}))],
            instructions: "fixed".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "shared".into(),
        };
        let body = request_body(&request).expect("body");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["reasoning"]["effort"], "low");
        assert_eq!(body["reasoning"]["summary"], "auto");
        assert_eq!(body["prompt_cache_key"], "shared");
        assert_eq!(body["instructions"], "fixed");
        assert_eq!(body["input"][0]["content"], "hello");
        assert_eq!(body["tool_choice"], "auto");
        assert!(body.get("previous_response_id").is_none());
    }

    #[test]
    fn restricted_tools_serialize_exact_order_and_empty_as_none() {
        let mut request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![
                json!({"type":"function","name":"a","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
                json!({"type":"function","name":"b","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
            ]
            .into(),
            tools_allowed: Some(vec!["b".into(), "a".into()]),
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "shared".into(),
        };
        let body = request_body(&request).unwrap();
        assert_eq!(body["tools"].as_array().unwrap().len(), 2);
        assert_eq!(body["tool_choice"]["tools"][0]["name"], "b");
        assert_eq!(body["tool_choice"]["tools"][1]["name"], "a");
        request.tools_allowed = Some(vec![]);
        assert_eq!(request_body(&request).unwrap()["tool_choice"], "none");
    }

    #[test]
    fn rejects_non_strict_tool() {
        let request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![json!({"type":"function","name":"bad"})].into(),
            tools_allowed: None,
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "shared".into(),
        };
        assert!(request_body(&request).is_err());
    }

    #[test]
    fn accepts_custom_tool_without_function_strict_field() {
        let request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![json!({"type":"custom","name":"run","description":"Freeform script"})]
                .into(),
            tools_allowed: None,
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "shared".into(),
        };
        assert!(request_body(&request).is_ok());
    }

    #[test]
    fn request_gate_checks_nested_strict_contracts_and_preserves_wire_metadata() {
        let valid = json!({"type":"function","name":"lookup","strict":true,"parameters":{
            "type":"object","properties":{"nested":{"type":"object","properties":{
                "value":{"type":["string","null"],"description":"An optional value"}
            },"required":["value"],"additionalProperties":false}},
            "required":["nested"],"additionalProperties":false,"description":"Host contract"
        }});
        let mut request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![valid.clone()].into(),
            tools_allowed: None,
            model: "test-model".into(),
            pinned_effort: Effort::Low,
            session_id: "gate".into(),
        };
        assert_eq!(request_body(&request).unwrap()["tools"][0], valid);
        // Deserialization retains the same immutable admission result.
        let restored: ResponsesRequest =
            serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
        assert_eq!(request_body(&restored).unwrap()["tools"][0], valid);
        for path in [
            "root-required",
            "nested-required",
            "nested-properties",
            "parameters",
        ] {
            let mut invalid = valid.clone();
            match path {
                "root-required" => {
                    invalid["parameters"]
                        .as_object_mut()
                        .unwrap()
                        .remove("required");
                }
                "nested-required" => {
                    invalid["parameters"]["properties"]["nested"]["required"] = json!([])
                }
                "nested-properties" => {
                    invalid["parameters"]["properties"]["nested"]
                        .as_object_mut()
                        .unwrap()
                        .remove("additionalProperties");
                }
                "parameters" => invalid["parameters"] = json!({"type":"string"}),
                _ => unreachable!(),
            }
            request.tools = vec![invalid].into();
            assert!(
                request_body(&request).is_err(),
                "invalid {path} was serialized"
            );
        }
    }

    #[tokio::test]
    async fn request_gate_refuses_invalid_schema_before_authentication_access() {
        #[derive(Clone)]
        struct NoAuth;
        impl Auth for NoAuth {
            fn access(&self) -> Result<(String, String), TransportError> {
                panic!("invalid schema must be refused before authentication");
            }
        }
        let request = ResponsesRequest {
            input: vec![], instructions:String::new(), tools:vec![json!({
                "type":"function","name":"status","strict":true,
                "parameters":{"type":"object","properties":{"view":{"type":"string"}},"additionalProperties":false}
            })].into(),tools_allowed:None,model:"test-model".into(),pinned_effort:Effort::Low,session_id:"gate-before-auth".into()
        };
        assert!(matches!(
            ResponsesClient::new(NoAuth).create(request).await,
            Err(TransportError::Stream(_))
        ));
    }

    #[test]
    fn sse_frame_accepts_split_multiline_event() {
        let mut f = SseFramer::new();
        assert!(f.push(b"data: {\"type\":\r\n").expect("first").is_empty());
        assert!(f.push(b"data: \"x\"}\n").expect("second").is_empty());
        assert_eq!(
            f.push(b"\n").expect("end"),
            vec!["{\"type\":\n\"x\"}".to_owned()]
        );
    }

    #[tokio::test]
    async fn provider_stream_error_retains_redacted_fields_through_http_body_reader() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let token = "mock-active-token";
        let account = "mock-active-account";
        let packet = json!({"type":"error", "code":"invalid_tools", "param":"tools[0].parameters",
            "message":format!("schema refused {token} {account}"), "echoed_request":{"token":token}});
        let body = format!("data: {packet}\n\n");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            socket.read(&mut request).await.unwrap();
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len(),
            ).as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let response = http_client()
            .unwrap()
            .get(format!("http://{address}/responses"))
            .send()
            .await
            .unwrap();
        let failure = read_response(response, None, token, account)
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(matches!(&failure, TransportError::ProviderStreamFailure {
            event: super::super::ProviderStreamFailureEvent::Error,
            diagnostic: Some(diagnostic),
        } if diagnostic.code.as_deref() == Some("invalid_tools")
            && diagnostic.param.as_deref() == Some("tools[0].parameters")
            && diagnostic.message.as_deref() == Some("schema refused [redacted] [redacted]")));
        assert!(failure.request_failure().is_none());
        let rendered = failure.to_string();
        for private in [token, account, "echoed_request"] {
            assert!(!rendered.contains(private));
        }
    }

    #[tokio::test]
    async fn body_truncation_preserves_terminal_proof_and_distinguishes_clean_eof() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (completed, truncated) in [(false, false), (false, true), (true, true)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let body = if completed {
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"terminal-proof\",\"output\":[]}}\n\n"
            } else {
                "data: {\"type\":\"response.created\"}\n\n"
            };
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0u8; 4096];
                socket.read(&mut request).await.unwrap();
                let length = body.len() + usize::from(truncated) * 100;
                socket.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n{body}"
                ).as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            });
            let response = http_client()
                .unwrap()
                .get(format!("http://{address}/responses"))
                .send()
                .await
                .unwrap();
            let result = read_response(response, None, "", "").await;
            server.await.unwrap();
            match (completed, truncated, result) {
                (true, _, Ok(turn)) => assert_eq!(turn.response_id, "terminal-proof"),
                (
                    false,
                    false,
                    Err(TransportError::IncompleteResponse(
                        super::super::StreamInterruption::MissingCompletion,
                    )),
                ) => {}
                (
                    false,
                    true,
                    Err(TransportError::IncompleteResponse(
                        super::super::StreamInterruption::ReadFailed,
                    )),
                ) => {}
                (_, _, result) => panic!("unexpected response disposition: {result:?}"),
            }
        }
    }

    /// Explicit live smoke test; never run in ordinary CI. Only the public
    /// item/usage shape is asserted, never the credential or request headers.
    #[tokio::test]
    #[ignore]
    async fn live_subscription_response() {
        let auth = CodexFileAuth::new(CodexFileAuth::default_path().expect("auth path"));
        let client = ResponsesClient::new(auth);
        let request = ResponsesRequest {
            input: vec![Item(
                json!({"role":"user","content":"Reply with exactly: TRANSPORT_OK"}),
            )],
            instructions: "This is a transport smoke test. Reply briefly.".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "harness-wave0-transport-smoke".into(),
        };
        let turn = client.create(request).await.expect("live response");
        assert!(
            turn.items
                .iter()
                .any(|item| item.0["phase"] == "final_answer")
        );
        assert!(!turn.response_id.is_empty());
    }
}

#[cfg(test)]
#[path = "client_lite_tests.rs"]
mod lite_tests;

#[cfg(test)]
#[path = "client_plan_tests.rs"]
mod plan_tests;
