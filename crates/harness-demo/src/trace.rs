//! Opt-in, redacted evidence seam for manual item-2 runs.
//!
//! The request record is a projection of the production request builder's
//! output, not a serialized request. Only protocol structure and operational
//! correlation metadata are retained.

use async_trait::async_trait;
use harness::{
    engine::ResponsesTransport,
    transport::{ResponsesRequest, ResponsesTurn, TransportError, client::request_body},
};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc, oneshot};

const QUEUE_CAPACITY: usize = 32;

#[derive(Debug)]
pub enum TraceError {
    Io(io::Error),
    WriterStopped,
    WriteFailed,
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(_) => write!(f, "trace I/O failed"),
            Self::WriterStopped => write!(f, "trace writer stopped"),
            Self::WriteFailed => write!(f, "trace write failed"),
        }
    }
}

impl std::error::Error for TraceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

/// Timestamped correlation event emitted by the root's sleep lifecycle.
#[derive(Clone, Debug)]
pub enum JobEvent {
    SleepStarted {
        call_id: String,
        handle: String,
    },
    SleepSettled {
        call_id: String,
        handle: String,
        duration_ms: u64,
    },
}

enum Message {
    Event(Value, oneshot::Sender<Result<(), ()>>),
    Flush(oneshot::Sender<Result<(), ()>>),
}

/// Bounded asynchronous JSONL trace sink shared by transport and job observer.
#[derive(Clone)]
pub struct TraceSink {
    tx: mpsc::Sender<Message>,
}

impl TraceSink {
    /// Create a new, explicitly selected trace path off the async
    /// executor, then start the bounded background writer.
    pub async fn open(path: impl Into<PathBuf>) -> Result<Self, TraceError> {
        let path = path.into();
        let file = tokio::task::spawn_blocking(move || {
            OpenOptions::new().create_new(true).write(true).open(path)
        })
        .await
        .map_err(|_| TraceError::WriterStopped)?
        .map_err(TraceError::Io)?;
        Self::with_file(file)
    }

    fn with_file(file: File) -> Result<Self, TraceError> {
        let (tx, mut rx) = mpsc::channel::<Message>(QUEUE_CAPACITY);
        thread::Builder::new()
            .name("harness-trace-writer".into())
            .spawn(move || writer_loop(file, &mut rx))
            .map_err(TraceError::Io)?;
        Ok(Self { tx })
    }

    /// Record sleep start/settle metadata with a sink-generated timestamp.
    pub async fn record_job(&self, event: JobEvent) -> Result<(), TraceError> {
        self.send_event(job_projection(event)).await
    }

    /// Wait until queued events are durable and surface any prior writer error.
    pub async fn flush(&self) -> Result<(), TraceError> {
        let (ack, reply) = oneshot::channel();
        self.tx
            .send(Message::Flush(ack))
            .await
            .map_err(|_| TraceError::WriterStopped)?;
        reply
            .await
            .map_err(|_| TraceError::WriterStopped)?
            .map_err(|_| TraceError::WriteFailed)
    }

    async fn send_event(&self, value: Value) -> Result<(), TraceError> {
        let (ack, reply) = oneshot::channel();
        self.tx
            .send(Message::Event(value, ack))
            .await
            .map_err(|_| TraceError::WriterStopped)?;
        reply
            .await
            .map_err(|_| TraceError::WriterStopped)?
            .map_err(|_| TraceError::WriteFailed)
    }
}

fn writer_loop(mut file: File, rx: &mut mpsc::Receiver<Message>) {
    let mut failed = false;
    while let Some(message) = rx.blocking_recv() {
        match message {
            Message::Event(value, ack) => {
                let result = if failed {
                    Err(())
                } else {
                    serde_json::to_writer(&mut file, &value)
                        .map_err(|_| ())
                        .and_then(|_| file.write_all(b"\n").map_err(|_| ()))
                        .and_then(|_| file.flush().map_err(|_| ()))
                };
                failed |= result.is_err();
                let _ = ack.send(result);
            }
            Message::Flush(ack) => {
                let result = if failed {
                    Err(())
                } else {
                    file.flush().map_err(|_| ())
                };
                failed |= result.is_err();
                let _ = ack.send(result);
            }
        }
    }
}

/// Opt-in wrapper around the actual production Responses transport.
pub struct TraceTransport<T> {
    inner: T,
    sink: Option<TraceSink>,
}

impl<T> TraceTransport<T> {
    pub fn new(inner: T, sink: TraceSink) -> Self {
        Self {
            inner,
            sink: Some(sink),
        }
    }

    /// Preserve the same concrete transport type when tracing is disabled.
    pub fn disabled(inner: T) -> Self {
        Self { inner, sink: None }
    }
}

#[async_trait]
impl<T: ResponsesTransport + 'static> ResponsesTransport for TraceTransport<T> {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.record_request(&request).await?;
        let turn = self.inner.create(request).await?;
        self.record_turn(&turn).await?;
        Ok(turn)
    }

    async fn create_streaming(
        &self,
        request: ResponsesRequest,
        sink: tokio::sync::mpsc::Sender<harness::transport::sse::StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        self.record_request(&request).await?;
        // Forward the caller's live event channel unchanged.
        let turn = self.inner.create_streaming(request, sink).await?;
        self.record_turn(&turn).await?;
        Ok(turn)
    }
}

impl<T> TraceTransport<T> {
    async fn record_request(&self, request: &ResponsesRequest) -> Result<(), TransportError> {
        let Some(sink) = &self.sink else {
            return Ok(());
        };
        let body = request_body(request)?;
        sink.send_event(request_projection(&body))
            .await
            .map_err(trace_transport_error)
    }

    async fn record_turn(&self, turn: &ResponsesTurn) -> Result<(), TransportError> {
        let Some(sink) = &self.sink else {
            return Ok(());
        };
        sink.send_event(json!({
            "event": "turn_complete",
            "timestamp_ms": timestamp_ms(),
            "item_count": turn.items.len(),
            "usage": {
                "input_tokens": turn.usage.input_tokens,
                "output_tokens": turn.usage.output_tokens,
                "cached_tokens": turn.usage.cached_tokens,
                "cache_write_tokens": turn.usage.cache_write_tokens,
            }
        }))
        .await
        .map_err(trace_transport_error)
    }
}

fn request_projection(body: &Value) -> Value {
    let tools = body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|tool| {
            json!({
                "name": safe_tool_name(tool.get("name").and_then(Value::as_str)),
                "async": tool.get("async").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect::<Vec<_>>();
    let input = body
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| {
            let mut structural = serde_json::Map::new();
            if let Some(value) = item.get("type").and_then(Value::as_str) {
                structural.insert("type".into(), Value::String(safe_item_type(value).into()));
            }
            if let Some(value) = item.get("name").and_then(Value::as_str) {
                structural.insert(
                    "name".into(),
                    Value::String(safe_tool_name(Some(value)).into()),
                );
            }
            if let Some(value) = item.get("call_id").and_then(Value::as_str) {
                structural.insert("call_id_hash".into(), Value::String(hash_id(value)));
            }
            if let Some(value) = item.get("status").and_then(Value::as_str) {
                if let Some(status) = safe_item_status(value) {
                    structural.insert("status".into(), Value::String(status.into()));
                }
            }
            if safe_item_type(item.get("type").and_then(Value::as_str).unwrap_or(""))
                == "function_call_output"
            {
                structural.insert("resumed_by".into(), resume_projection(item.get("output")));
            }
            Value::Object(structural)
        })
        .collect::<Vec<_>>();
    json!({
        "event": "request",
        "timestamp_ms": timestamp_ms(),
        "model": body.get("model").and_then(Value::as_str).unwrap_or("<unknown>"),
        "effort": body.get("reasoning").and_then(|v| v.get("effort"))
            .and_then(Value::as_str).unwrap_or("<unknown>"),
        "tools": tools,
        "input": input,
    })
}

fn resume_projection(output: Option<&Value>) -> Value {
    let Some(output) = output else {
        return json!({"kind":"unknown"});
    };
    let parsed;
    let payload = match output {
        Value::String(text) => {
            parsed = match serde_json::from_str::<Value>(text) {
                Ok(value) => value,
                Err(_) => return json!({"kind":"unknown"}),
            };
            &parsed
        }
        Value::Object(_) => output,
        _ => return json!({"kind":"unknown"}),
    };
    match payload.get("resumed_by") {
        Some(Value::String(value)) if value == "user_input" => json!({"kind":"user_input"}),
        Some(Value::String(value)) if value == "cancelled" => json!({"kind":"cancelled"}),
        Some(Value::Object(resumer)) if resumer.len() == 1 => {
            if let Some(Value::String(handle)) = resumer.get("job") {
                json!({"kind":"job","handle_hash":hash_id(handle)})
            } else if resumer.get("agent").is_some() {
                json!({"kind":"agent"})
            } else {
                json!({"kind":"unknown"})
            }
        }
        _ => json!({"kind":"unknown"}),
    }
}

fn job_projection(event: JobEvent) -> Value {
    match event {
        JobEvent::SleepStarted { call_id, handle } => json!({
            "event": "sleep_started",
            "timestamp_ms": timestamp_ms(),
            "call_id_hash": hash_id(&call_id),
            "handle_hash": hash_id(&handle),
        }),
        JobEvent::SleepSettled {
            call_id,
            handle,
            duration_ms,
        } => json!({
            "event": "sleep_settled",
            "timestamp_ms": timestamp_ms(),
            "call_id_hash": hash_id(&call_id),
            "handle_hash": hash_id(&handle),
            "duration_ms": duration_ms,
        }),
    }
}

fn safe_tool_name(name: Option<&str>) -> &'static str {
    match name {
        Some("sleep") => "sleep",
        Some("wait_agent") => "wait_agent",
        _ => "<other>",
    }
}

fn safe_item_type(kind: &str) -> &'static str {
    match kind {
        "function_call" => "function_call",
        "function_call_output" => "function_call_output",
        "message" => "message",
        _ => "<other>",
    }
}

fn safe_item_status(status: &str) -> Option<&'static str> {
    match status {
        "in_progress" => Some("in_progress"),
        "completed" => Some("completed"),
        _ => None,
    }
}

fn hash_id(value: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn trace_transport_error(_: TraceError) -> TransportError {
    TransportError::Stream("trace recording failed".into())
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness::{
        item::Item,
        transport::{ResponsesRequest, ResponsesTurn, Usage, sse::StreamEvent},
    };
    use std::sync::atomic::{AtomicBool, Ordering};

    struct MustNotDispatch(AtomicBool);

    #[async_trait]
    impl ResponsesTransport for MustNotDispatch {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            self.0.store(true, Ordering::SeqCst);
            Err(TransportError::Stream("unexpected dispatch".into()))
        }

        async fn create_streaming(
            &self,
            _: ResponsesRequest,
            _: tokio::sync::mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            self.0.store(true, Ordering::SeqCst);
            Err(TransportError::Stream("unexpected dispatch".into()))
        }
    }

    struct StreamingProbe;

    #[async_trait]
    impl ResponsesTransport for StreamingProbe {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            Err(TransportError::Stream("create should not be called".into()))
        }

        async fn create_streaming(
            &self,
            _: ResponsesRequest,
            sink: mpsc::Sender<StreamEvent>,
        ) -> Result<ResponsesTurn, TransportError> {
            sink.send(StreamEvent::Delta("sentinel-event".into()))
                .await
                .map_err(|_| TransportError::Stream("receiver closed".into()))?;
            Ok(ResponsesTurn {
                response_id: "not traced".into(),
                items: vec![],
                usage: Usage::default(),
            })
        }
    }

    struct AcceptAny;

    #[async_trait]
    impl ResponsesTransport for AcceptAny {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            Ok(ResponsesTurn {
                response_id: "response".into(),
                items: vec![],
                usage: Usage::default(),
            })
        }
    }

    #[test]
    fn projection_keeps_request_structure_and_excludes_sensitive_fields() {
        let body = json!({
            "model": "gpt-6-sol",
            "instructions": "SENTINEL_INSTRUCTIONS",
            "reasoning": {"effort": "low"},
            "prompt_cache_key": "SENTINEL_SESSION",
            "token": "SENTINEL_TOKEN",
            "account": "SENTINEL_ACCOUNT",
            "tools": [{
                "type": "function", "name": "sleep", "async": true,
                "description": "SENTINEL_DESCRIPTION",
                "parameters": {"secret": "SENTINEL_SCHEMA"}
            }],
            "input": [
                {"type": "function_call", "name": "sleep", "call_id": "call-1",
                 "arguments": "SENTINEL_ARGUMENTS"},
                {"type": "function_call_output", "call_id": "call-1",
                 "output": "SENTINEL_OUTPUT"},
                {"type": "message", "role": "user", "content": "SENTINEL_TEXT"}
            ]
        });
        let projected = request_projection(&body);
        assert_eq!(projected["model"], "gpt-6-sol");
        assert_eq!(projected["effort"], "low");
        assert_eq!(projected["tools"][0]["name"], "sleep");
        assert_eq!(projected["tools"][0]["async"], true);
        assert_eq!(projected["input"][0]["call_id_hash"], hash_id("call-1"));
        assert_eq!(
            projected["input"][0]["call_id_hash"],
            projected["input"][1]["call_id_hash"]
        );
        assert_eq!(projected["input"][1]["type"], "function_call_output");
        assert_eq!(projected["input"][2]["type"], "message");
        let trace = projected.to_string();
        assert!(!trace.contains("call-1"));
        for excluded in [
            "SENTINEL_INSTRUCTIONS",
            "SENTINEL_SESSION",
            "SENTINEL_TOKEN",
            "SENTINEL_ACCOUNT",
            "SENTINEL_DESCRIPTION",
            "SENTINEL_SCHEMA",
            "SENTINEL_ARGUMENTS",
            "SENTINEL_OUTPUT",
            "SENTINEL_TEXT",
        ] {
            assert!(!trace.contains(excluded), "trace leaked {excluded}");
        }
    }

    #[test]
    fn completion_event_has_no_response_identity_or_content() {
        let turn = ResponsesTurn {
            response_id: "SENTINEL_RESPONSE_ID".into(),
            items: vec![Item(json!({"text":"SENTINEL_RESPONSE_TEXT"}))],
            usage: Usage {
                input_tokens: 2,
                output_tokens: 3,
                cached_tokens: 1,
                cache_write_tokens: 0,
            },
        };
        let event = json!({
            "event":"turn_complete",
            "timestamp_ms":timestamp_ms(),
            "item_count":turn.items.len(),
            "usage":{"input_tokens":turn.usage.input_tokens}
        });
        assert!(!event.to_string().contains("SENTINEL_RESPONSE_ID"));
        assert!(!event.to_string().contains("SENTINEL_RESPONSE_TEXT"));
    }

    #[tokio::test]
    async fn trace_write_failure_prevents_transport_dispatch() {
        let Ok(file) = OpenOptions::new().write(true).open("/dev/full") else {
            // Some non-Unix test hosts do not provide /dev/full.
            return;
        };
        let sink = TraceSink::with_file(file).unwrap();
        let transport = TraceTransport::new(MustNotDispatch(AtomicBool::new(false)), sink);
        let request = ResponsesRequest {
            input: vec![],
            instructions: "not traced".into(),
            tools: vec![],
            model: "safe-model".into(),
            pinned_effort: harness::model::Effort::Low,
            session_id: "not-traced".into(),
        };
        assert!(transport.create(request).await.is_err());
        assert!(!transport.inner.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn opening_existing_path_fails_without_overwriting_contents() {
        let path = std::env::temp_dir().join(format!(
            "harness-trace-existing-{}-{}.jsonl",
            std::process::id(),
            timestamp_ms()
        ));
        let sentinel = b"existing user evidence";
        std::fs::write(&path, sentinel).unwrap();
        assert!(TraceSink::open(path.clone()).await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), sentinel);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn create_streaming_forwards_the_supplied_event_channel_unchanged() {
        let path = std::env::temp_dir().join(format!(
            "harness-trace-stream-{}-{}.jsonl",
            std::process::id(),
            timestamp_ms()
        ));
        let sink = TraceSink::open(path.clone()).await.unwrap();
        let transport = TraceTransport::new(StreamingProbe, sink.clone());
        let (events_tx, mut events_rx) = mpsc::channel(2);
        let request = ResponsesRequest {
            input: vec![],
            instructions: "not traced".into(),
            tools: vec![],
            model: "safe-model".into(),
            pinned_effort: harness::model::Effort::Low,
            session_id: "not-traced".into(),
        };
        transport
            .create_streaming(request, events_tx)
            .await
            .unwrap();
        assert!(matches!(
            events_rx.recv().await,
            Some(StreamEvent::Delta(text)) if text == "sentinel-event"
        ));
        sink.flush().await.unwrap();
        drop(sink);
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn disabled_transport_has_same_concrete_type_and_bypasses_trace_builder() {
        let transport = TraceTransport::disabled(AcceptAny);
        let request = ResponsesRequest {
            input: vec![],
            instructions: String::new(),
            tools: vec![],
            model: String::new(),
            pinned_effort: harness::model::Effort::Low,
            session_id: String::new(),
        };
        assert!(transport.create(request).await.is_ok());
    }

    #[tokio::test]
    async fn job_events_include_hashed_correlations_and_sink_timestamps() {
        let path = std::env::temp_dir().join(format!(
            "harness-trace-jobs-{}-{}.jsonl",
            std::process::id(),
            timestamp_ms()
        ));
        let sink = TraceSink::open(path.clone()).await.unwrap();
        sink.record_job(JobEvent::SleepStarted {
            call_id: "call-7".into(),
            handle: "job-9".into(),
        })
        .await
        .unwrap();
        sink.record_job(JobEvent::SleepSettled {
            call_id: "call-7".into(),
            handle: "job-9".into(),
            duration_ms: 42,
        })
        .await
        .unwrap();
        sink.flush().await.unwrap();
        drop(sink);
        let events = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["event"], "sleep_started");
        assert_eq!(events[1]["event"], "sleep_settled");
        for event in &events {
            assert!(event["timestamp_ms"].as_u64().is_some());
            assert_eq!(event["call_id_hash"], hash_id("call-7"));
            assert_eq!(event["handle_hash"], hash_id("job-9"));
        }
        assert_eq!(events[1]["duration_ms"], 42);
        let serialized = events.iter().map(Value::to_string).collect::<String>();
        assert!(!serialized.contains("call-7"));
        assert!(!serialized.contains("job-9"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn calls_and_later_outputs_correlate_by_hash_without_premature_output() {
        let call_id = "SENTINEL_RAW_CALL_ID";
        let request_n = request_projection(&json!({
            "model":"gpt-6-sol",
            "reasoning":{"effort":"low"},
            "tools":[{"name":"wait_agent","async":true},{"name":"private_tool","async":true}],
            "input":[
                {"type":"function_call","name":"wait_agent","call_id":call_id,
                 "status":"in_progress","arguments":"SENTINEL_ARGS"}
            ]
        }));
        let n = request_n["input"].as_array().unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0]["type"], "function_call");
        assert_eq!(n[0]["name"], "wait_agent");
        assert_eq!(n[0]["status"], "in_progress");
        assert_eq!(n[0]["call_id_hash"], hash_id(call_id));
        assert_eq!(request_n["tools"][0]["name"], "wait_agent");
        assert_eq!(request_n["tools"][0]["async"], true);
        assert_eq!(request_n["tools"][1]["name"], "<other>");
        assert_eq!(request_n["tools"][1]["async"], true);
        assert!(!request_n.to_string().contains(call_id));
        assert!(!request_n.to_string().contains("SENTINEL_ARGS"));

        let request_n_plus_1 = request_projection(&json!({
            "model":"gpt-6-sol",
            "reasoning":{"effort":"low"},
            "tools":[],
            "input":[
                {"type":"function_call","name":"wait_agent","call_id":call_id,
                 "status":"in_progress","arguments":"SENTINEL_ARGS"},
                {"type":"function_call_output","call_id":call_id,
                 "output":"SENTINEL_PRIVATE_OUTPUT"}
            ]
        }));
        let later = request_n_plus_1["input"].as_array().unwrap();
        assert_eq!(later.len(), 2);
        assert_eq!(later[0]["call_id_hash"], later[1]["call_id_hash"]);
        assert_eq!(later[1]["type"], "function_call_output");
        assert_eq!(later[1]["call_id_hash"], hash_id(call_id));
        assert!(!request_n_plus_1.to_string().contains(call_id));
        assert!(
            !request_n_plus_1
                .to_string()
                .contains("SENTINEL_PRIVATE_OUTPUT")
        );
    }

    #[test]
    fn wait_agent_resume_kinds_are_redacted_and_job_matches_sleep_settlement() {
        let raw_handle = "SENTINEL_SLEEP_HANDLE";
        let sleep_settled = job_projection(JobEvent::SleepSettled {
            call_id: "SENTINEL_CALL_ID".into(),
            handle: raw_handle.into(),
            duration_ms: 37,
        });
        let wait_output = json!({
            "type":"function_call_output",
            "call_id":"SENTINEL_WAIT_CALL_ID",
            "output":json!({"resumed_by":{"job":raw_handle}}).to_string()
        });
        let projected = request_projection(&json!({
            "model":"gpt-6-sol",
            "reasoning":{"effort":"low"},
            "tools":[{"name":"wait_agent","async":true}],
            "input":[wait_output]
        }));
        let resume = &projected["input"][0]["resumed_by"];
        assert_eq!(resume["kind"], "job");
        assert_eq!(resume["handle_hash"], sleep_settled["handle_hash"]);
        assert_eq!(resume["handle_hash"], hash_id(raw_handle));

        let cases = [
            (json!({"resumed_by":{"agent":"/secret/agent"}}), "agent"),
            (json!({"resumed_by":"user_input"}), "user_input"),
            (json!({"resumed_by":"cancelled"}), "cancelled"),
            (
                json!({"resumed_by":{"mystery":"SENTINEL_UNKNOWN"}}),
                "unknown",
            ),
        ];
        for (value, expected) in cases {
            let projection = resume_projection(Some(&value));
            assert_eq!(projection["kind"], expected);
            assert!(!projection.to_string().contains("/secret/agent"));
            assert!(!projection.to_string().contains("SENTINEL_UNKNOWN"));
        }
        assert_eq!(
            resume_projection(Some(&json!("malformed")))["kind"],
            "unknown"
        );
        assert!(!projected.to_string().contains(raw_handle));
        assert!(!projected.to_string().contains("SENTINEL_WAIT_CALL_ID"));
    }
}
