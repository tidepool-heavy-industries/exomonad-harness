use super::{
    Auth, ResponsesRequest, ResponsesTurn, TransportError,
    sse::{ResponseAssembly, StreamEvent},
};
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use std::time::Duration;

pub const ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const CODEX_VERSION: &str = "0.155.1";

/// Only this transport builds the backend request; the stable session id is
/// shared with prompt_cache_key, while caller controls the complete stateless
/// item window. Never send previous_response_id or store:true.
pub fn request_body(request: &ResponsesRequest) -> Result<Value, TransportError> {
    serde_json::to_value(normalized_request(request)?)
        .map_err(|_| TransportError::Stream("request serialization failed".into()))
}

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    instructions: &'a str,
    input: &'a [crate::item::Item],
    tools: super::StrictToolManifest<'a>,
    tool_choice: ToolChoice<'a>,
    parallel_tool_calls: bool,
    reasoning: Reasoning,
    stream: bool,
    store: bool,
    prompt_cache_key: &'a str,
    include: [&'static str; 1],
}

#[derive(Serialize)]
struct Reasoning {
    effort: crate::model::Effort,
    summary: &'static str,
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

fn normalized_request(request: &ResponsesRequest) -> Result<RequestBody<'_>, TransportError> {
    if request.model.is_empty() || request.session_id.is_empty() {
        return Err(TransportError::Stream("empty model or session id".into()));
    }
    Ok(RequestBody {
        model: &request.model,
        instructions: &request.instructions,
        input: &request.input,
        tools: request.tools.strict_tools()?,
        tool_choice: match request.tools_allowed.as_deref() {
            None => ToolChoice::Automatic("auto"),
            Some([]) => ToolChoice::Automatic("none"),
            Some(names) => ToolChoice::Allowed {
                kind: "allowed_tools",
                mode: "auto",
                tools: AllowedTools(names),
            },
        },
        parallel_tool_calls: true,
        reasoning: Reasoning {
            effort: request.pinned_effort,
            summary: "auto",
        },
        stream: true,
        store: false,
        prompt_cache_key: &request.session_id,
        include: ["reasoning.encrypted_content"],
    })
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

/// Runs the stateless streaming request. Auth disk access happens in a
/// blocking-pool task, never on an async executor worker. No credential value
/// is included in errors, traces, stored items, or returned data.
pub(super) async fn execute<A: Auth + Clone + 'static>(
    auth: A,
    request: ResponsesRequest,
    sink: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
) -> Result<ResponsesTurn, TransportError> {
    let body = normalized_request(&request)?;
    let (token, account) = tokio::task::spawn_blocking(move || auth.access())
        .await
        .map_err(|_| TransportError::Authentication)??;
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|_| TransportError::Stream("HTTP client initialization failed".into()))?;
    let response = http
        .post(ENDPOINT)
        .bearer_auth(&token)
        .header("chatgpt-account-id", &account)
        .header("version", CODEX_VERSION)
        .header("originator", "codex_cli_rs")
        .header("session-id", &request.session_id)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|_| TransportError::Stream("HTTP request failed".into()))?;
    let status = response.status().as_u16();
    if status == 401 {
        return Err(TransportError::Authentication);
    }
    if status != 200 {
        let diagnostic = super::http_error::read(response, &token, &account).await;
        return Err(TransportError::Http { status, diagnostic });
    }
    let mut stream = response.bytes_stream();
    let mut framer = SseFramer::new();
    let mut assembly = ResponseAssembly::default();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| TransportError::Stream("SSE read failed".into()))?;
        for data in framer.push(&chunk)? {
            if let Some(event) = assembly.accept(&data)? {
                if let Some(sender) = &sink {
                    match event {
                        // State-bearing items are lossless. A closed sink
                        // means its consumer has stopped; final turn remains
                        // available from the returned ResponsesTurn.
                        StreamEvent::ItemDone(_) => {
                            let _ = sender.send(event).await;
                        }
                        // Text deltas are recoverable from the final item, so
                        // a slow subscriber cannot stall the model stream.
                        StreamEvent::Delta(_) => {
                            let _ = sender.try_send(event);
                        }
                    }
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
