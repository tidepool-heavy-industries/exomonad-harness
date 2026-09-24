use super::{
    Auth, ResponsesRequest, ResponsesTurn, TransportError,
    sse::{ResponseAssembly, StreamEvent},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;

pub const ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
pub const CODEX_VERSION: &str = "0.155.1";

/// Only this transport builds the backend request; the stable session id is
/// shared with prompt_cache_key, while caller controls the complete stateless
/// item window. Never send previous_response_id or store:true.
pub fn request_body(request: &ResponsesRequest) -> Result<Value, TransportError> {
    if request.model.is_empty() || request.session_id.is_empty() {
        return Err(TransportError::Stream("empty model or session id".into()));
    }
    if request.tools.iter().any(|tool| {
        tool.get("type").and_then(Value::as_str) == Some("function")
            && tool.get("strict") != Some(&Value::Bool(true))
    }) {
        return Err(TransportError::Stream(
            "all function tools must be strict".into(),
        ));
    }
    Ok(json!({
        "model": request.model,
        "instructions": request.instructions,
        "input": request.input,
        "tools": request.tools,
        // TODO(correction-wave): `tool_choice` should come from the request as
        // `allowed_tools`/`none` for per-request availability, never by editing
        // `tools` (cache). Also missing: `prompt_cache_options: {ttl: "30m"}`
        // and explicit cache breakpoints at checkpoints. Cache counters read 0
        // on every live call so far; a findings-only probe (docs/tree.md) diffs
        // our body/headers against codex's before assuming the backend reports none.
        "tool_choice": "auto",
        "parallel_tool_calls": true,
        "reasoning": {"effort": request.pinned_effort},
        "stream": true,
        "store": false,
        "prompt_cache_key": request.session_id,
        "include": ["reasoning.encrypted_content"]
    }))
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
    let body = request_body(&request)?;
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
        .bearer_auth(token)
        .header("chatgpt-account-id", account)
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
        return Err(TransportError::Http(status));
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
            tools: vec![],
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "shared".into(),
        };
        let body = request_body(&request).expect("body");
        assert_eq!(body["stream"], true);
        assert_eq!(body["store"], false);
        assert_eq!(body["reasoning"]["effort"], "low");
        assert_eq!(body["prompt_cache_key"], "shared");
        assert_eq!(body["instructions"], "fixed");
        assert_eq!(body["input"][0]["content"], "hello");
        assert!(body.get("previous_response_id").is_none());
    }

    #[test]
    fn rejects_non_strict_tool() {
        let request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![json!({"type":"function","name":"bad"})],
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
            tools: vec![json!({"type":"custom","name":"run","description":"Freeform script"})],
            model: "gpt-6-sol".into(),
            pinned_effort: Effort::Low,
            session_id: "shared".into(),
        };
        assert!(request_body(&request).is_ok());
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
            tools: vec![],
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
