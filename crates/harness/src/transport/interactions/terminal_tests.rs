use super::*;
use crate::{
    embedding::{
        AdmissionGuard, BoundProvider, Conversation, EmbeddedError, EmbeddedToolManifest,
        HostActor, HostControl, HostControlError, HostIdentity, HostInferenceState,
        InferenceBlockReason, ToolSurface,
    },
    engine::{Engine, EngineConfig, EngineError},
    model::AgentPath,
    provider::{Provider, ProviderError},
    store::{ProviderResponseState, Store},
    transport::{
        CredentialReference, IncompleteReason, NativeTransportAdmission, NativeTransportContract,
        ProviderStreamFailureEvent, ResponseEnd, ResponseTerminal, ResponsesClient,
        StreamInterruption, TransportObservation, auth::GeminiAuth,
    },
    turn::JobScheduler,
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::State,
    http::HeaderMap,
    response::Response,
    routing::post,
};
use futures_util::StreamExt;
use proptest::prelude::*;
use std::{
    collections::VecDeque,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, watch};

const KEY: &str = "offline-gemini-terminal-key";

fn wire(events: Vec<Value>) -> Vec<u8> {
    events
        .into_iter()
        .flat_map(|event| format!("data: {event}\n\n").into_bytes())
        .collect()
}
fn created(id: &str) -> Value {
    json!({"event_type":"interaction.created","interaction":{"id":id,"status":"in_progress"}})
}
fn output(text: &str) -> Value {
    json!({"type":"model_output","content":[{"type":"text","text":text}]})
}
fn complete() -> Vec<u8> {
    wire(vec![
        created("success"),
        json!({"event_type":"step.start","index":0,"step":output("recovered λ")}),
        json!({"event_type":"step.stop","index":0}),
        json!({"event_type":"interaction.completed","interaction":{"id":"success","status":"completed"}}),
    ])
}

#[derive(Clone)]
struct HttpState {
    bodies: Arc<Mutex<VecDeque<Vec<u8>>>>,
    requests: Arc<AtomicUsize>,
    after_effect: Option<Arc<Notify>>,
}
async fn respond(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
    assert_eq!(headers["x-goog-api-key"], KEY);
    assert!(!headers.contains_key("authorization"));
    let request: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request["model"], "gemini-4-argon");
    assert_eq!(request["store"], false);
    state.requests.fetch_add(1, Ordering::SeqCst);
    let body = state
        .bodies
        .lock()
        .unwrap()
        .pop_front()
        .expect("unexpected provider retry");
    // Deliberately cross UTF-8/SSE boundaries; the optional tail keeps the
    // stream alive until a real Engine-dispatched tool has executed.
    let chunks = body
        .chunks(3)
        .map(|v| Ok::<_, std::io::Error>(Bytes::copy_from_slice(v)))
        .collect::<Vec<_>>();
    let tail = futures_util::stream::once(async move {
        if let Some(effect) = state.after_effect {
            effect.notified().await;
        }
        Ok::<_, std::io::Error>(Bytes::new())
    });
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(
            futures_util::stream::iter(chunks).chain(tail),
        ))
        .unwrap()
}

struct LocalHttp {
    directory: PathBuf,
    auth: GeminiAuth,
    requests: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}
impl LocalHttp {
    async fn new(bodies: Vec<Vec<u8>>, after_effect: Option<Arc<Notify>>) -> Self {
        let directory =
            std::env::temp_dir().join(format!("gemini-terminal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("key");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        writeln!(file, "{KEY}").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!(
            "http://{}/v1beta/interactions",
            listener.local_addr().unwrap()
        );
        let requests = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/v1beta/interactions", post(respond))
            .with_state(HttpState {
                bodies: Arc::new(Mutex::new(bodies.into())),
                requests: requests.clone(),
                after_effect,
            });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let admission = NativeTransportAdmission::rehydrate(NativeTransportContract {
            route: super::super::ResponsesRoute::GeminiInteractions,
            protocol: super::super::ResponsesProtocol::Interactions,
            endpoint,
            credential: CredentialReference::File { path },
            limits: InferenceLimits::gemini_default(),
        })
        .unwrap();
        Self {
            directory,
            auth: GeminiAuth::from_admission(admission).unwrap(),
            requests,
            server,
        }
    }
    fn store(&self) -> Arc<Store> {
        Arc::new(Store::open(self.directory.join("store.sqlite")).unwrap())
    }
    fn diagnose(&self, label: &str, store: &Store, calls: &AtomicUsize) {
        eprintln!(
            "{label}: HTTP={} effects={} Store={}",
            self.requests.load(Ordering::SeqCst),
            calls.load(Ordering::SeqCst),
            self.directory.join("store.sqlite").display()
        );
        let events = match store.events(None) {
            Ok(events) => events,
            Err(error) => {
                eprintln!("{label}: Store event observation failed: {error:?}");
                return;
            }
        };
        let mut requests = Vec::new();
        for event in events {
            eprintln!(
                "{label}: event={} kind={} request={:?}",
                event.id, event.kind, event.request
            );
            if let Some(request) = event.request {
                if !requests.contains(&request) {
                    requests.push(request);
                }
            }
        }
        for request in requests {
            let claims = store.claims_on(&request);
            let claim_states = claims.as_ref().map(|claims| {
                claims
                    .iter()
                    .map(|claim| (&claim.call_id, &claim.state))
                    .collect::<Vec<_>>()
            });
            eprintln!(
                "{label}: request={request:?} provider={:?} items={:?} claims={:?}",
                store.provider_response_state(&request),
                store.items(&request).map(|items| items.len()),
                claim_states
            );
        }
        eprintln!(
            "{label}: embedded={:?}",
            store.embedded_inference_state(&identity())
        );
    }
}
impl Drop for LocalHttp {
    fn drop(&mut self) {
        self.server.abort();
        if std::thread::panicking() {
            eprintln!(
                "failed Gemini terminal Store retained at {}",
                self.directory.display()
            );
        } else {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }
}

struct Effect {
    calls: Arc<AtomicUsize>,
    completed: Option<Arc<Notify>>,
}
#[async_trait::async_trait]
impl Provider for Effect {
    async fn call(&self, name: &str, args: Value) -> Result<Value, ProviderError> {
        assert_eq!(name, "effect");
        assert_eq!(args, json!({}));
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(done) = &self.completed {
            done.notify_one();
        }
        Ok(json!("effect completed once"))
    }
    fn tools(&self) -> Vec<Value> {
        vec![
            json!({"type":"function","name":"effect","description":"synthetic effect",
            "strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ]
    }
}
fn identity() -> HostIdentity {
    HostIdentity {
        run: "gemini-terminal".into(),
        actor: AgentPath("/root".into()),
        incarnation: "one".into(),
    }
}
struct LocalHost {
    identity: HostIdentity,
    surface: Arc<ToolSurface>,
}
struct LocalAdmission;
impl AdmissionGuard for LocalAdmission {}
#[async_trait::async_trait]
impl HostActor for LocalHost {
    fn identity(&self) -> &HostIdentity {
        &self.identity
    }
    async fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError> {
        Ok(Box::new(LocalAdmission))
    }
    async fn tool_surface(&self) -> Result<Arc<ToolSurface>, EmbeddedError> {
        Ok(self.surface.clone())
    }
    async fn wake(&self, _: i64) -> Result<(), String> {
        panic!("no input wake in HTTP fixture")
    }
    async fn control(&self, _: HostControl) -> Result<Value, HostControlError> {
        panic!("no host control in HTTP fixture")
    }
}
async fn engine(
    http: &LocalHttp,
    store: Arc<Store>,
    effect: Effect,
) -> Engine<GeminiAuth, BoundProvider> {
    // Install through the same host surface and request snapshot owner as an
    // embedding. A raw Provider does not own Gemini's wire encoding.
    let manifest = EmbeddedToolManifest::new(effect.tools()).unwrap();
    let surface = Arc::new(
        ToolSurface::from_manifest(
            "terminal-fixture".into(),
            Arc::new(manifest),
            Arc::new(effect),
        )
        .unwrap(),
    );
    let conversation = Conversation::attach(
        store,
        Arc::new(LocalHost {
            identity: identity(),
            surface,
        }),
        None,
    )
    .await
    .unwrap();
    conversation
        .engine(
            ResponsesClient::new(http.auth.clone()),
            Arc::new(JobScheduler::new(1).unwrap()),
            EngineConfig {
                instructions: "offline Gemini terminal control".into(),
                tools: vec![],
                model: "gemini-4-argon".into(),
                effort: Effort::Low,
                session_id: "gemini-terminal".into(),
                agent: identity().actor,
            },
            std::num::NonZeroU64::new(1_000_000).unwrap(),
        )
        .unwrap()
}

fn summary(store: &Store, request: &crate::model::RequestId) -> TransportObservation {
    let events = store.events(Some(request)).unwrap();
    let summaries = events
        .iter()
        .filter(|e| e.kind == "responses_transport")
        .collect::<Vec<_>>();
    assert_eq!(summaries.len(), 1);
    serde_json::from_str(&summaries[0].payload).unwrap()
}

#[tokio::test]
async fn empty_http_eof_uses_existing_effect_free_retry_and_retains_interruption_after_reopen() {
    let unfinished = wire(vec![
        created("empty"),
        json!({"event_type":"step.start","index":0,"step":{"type":"function_call","id":"never-executed","name":"effect"}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"arguments_delta","arguments":"{"}}),
    ]);
    let http = LocalHttp::new(vec![unfinished, complete()], None).await;
    let store = http.store();
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = engine(
        &http,
        store.clone(),
        Effect {
            calls: calls.clone(),
            completed: None,
        },
    )
    .await;
    let (_stop, cancel) = watch::channel(false);
    let completion = tokio::time::timeout(
        Duration::from_secs(10),
        engine.run_embedded(
            None,
            vec![Item::user_text("recover an effect-free interruption")],
            cancel,
            tokio::sync::mpsc::unbounded_channel().1,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let interrupted = store
        .request(&completion.head_request)
        .unwrap()
        .unwrap()
        .parent
        .unwrap();
    assert_eq!(http.requests.load(Ordering::SeqCst), 2);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        store.provider_response_state(&interrupted).unwrap(),
        Some(ProviderResponseState::Interrupted)
    );
    assert_eq!(summary(&store, &interrupted).end, ResponseEnd::CleanEof);
    assert_eq!(summary(&store, &interrupted).terminal, None);
    assert!(
        store
            .items(&interrupted)
            .unwrap()
            .iter()
            .all(|item| item.0.get("gemini_step").is_none())
    );
    assert_eq!(
        store
            .events(Some(&interrupted))
            .unwrap()
            .iter()
            .filter(|e| e.kind == "model_interrupted")
            .count(),
        1
    );
    drop(engine);
    drop(store);
    let reopened = http.store();
    assert_eq!(
        reopened.provider_response_state(&interrupted).unwrap(),
        Some(ProviderResponseState::Interrupted)
    );
    assert_eq!(
        reopened
            .provider_response_state(&completion.head_request)
            .unwrap(),
        Some(ProviderResponseState::Completed)
    );
    assert!(matches!(
        reopened.embedded_inference_state(&identity()).unwrap(),
        HostInferenceState::Idle
    ));
}

#[tokio::test]
async fn terminal_http_facts_and_escaped_prefix_survive_engine_block_and_store_reopen() {
    let native = output("complete prefix λ");
    for status in [
        None,
        Some("incomplete"),
        Some("budget_exceeded"),
        Some("failed"),
        Some("cancelled"),
        Some("error"),
    ] {
        let mut events = vec![
            created("failed-round"),
            json!({"event_type":"step.start","index":0,"step":native}),
            json!({"event_type":"step.stop","index":0}),
        ];
        if let Some(status) = status {
            events.push(if status == "error" { json!({"event_type":"error","error":{"message":KEY}}) }
                else { json!({"event_type":"interaction.status_update","interaction_id":"failed-round","status":status}) });
        }
        let http = LocalHttp::new(vec![wire(events)], None).await;
        let store = http.store();
        let calls = Arc::new(AtomicUsize::new(0));
        let engine = engine(
            &http,
            store.clone(),
            Effect {
                calls: calls.clone(),
                completed: None,
            },
        )
        .await;
        let (_stop, cancel) = watch::channel(false);
        let error = tokio::time::timeout(
            Duration::from_secs(10),
            engine.run_embedded(
                None,
                vec![Item::user_text("retain the prefix")],
                cancel.clone(),
                tokio::sync::mpsc::unbounded_channel().1,
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        let EngineError::BlockedAction(failure) = error else {
            panic!("lost typed block: {error:?}");
        };
        let block = failure.block().clone();
        let expected_terminal = match status {
            None => {
                assert!(matches!(
                    failure.primary(),
                    EngineError::InterruptedModelRound {
                        cause: StreamInterruption::MissingCompletion,
                        ..
                    }
                ));
                assert!(matches!(
                    block.reason,
                    InferenceBlockReason::StreamInterrupted {
                        cause: StreamInterruption::MissingCompletion
                    }
                ));
                None
            }
            Some("incomplete" | "budget_exceeded") => {
                let reason =
                    (status == Some("budget_exceeded")).then_some(IncompleteReason::BudgetExceeded);
                assert!(
                    matches!(failure.primary(), EngineError::Transport(TransportError::ResponseIncomplete { response_id, reason:actual }) if response_id == "failed-round" && *actual == reason)
                );
                assert_eq!(
                    block.reason,
                    InferenceBlockReason::ResponseIncomplete {
                        response_id: "failed-round".into(),
                        reason
                    }
                );
                Some(ResponseTerminal::Incomplete {
                    response_id: "failed-round".into(),
                    reason,
                })
            }
            Some(status) => {
                let (event, terminal) = match status {
                    "failed" => (
                        ProviderStreamFailureEvent::InteractionFailed,
                        ResponseTerminal::InteractionFailed,
                    ),
                    "cancelled" => (
                        ProviderStreamFailureEvent::InteractionCancelled,
                        ResponseTerminal::InteractionCancelled,
                    ),
                    "error" => (ProviderStreamFailureEvent::Error, ResponseTerminal::Error),
                    _ => unreachable!(),
                };
                assert!(
                    matches!(failure.primary(), EngineError::Transport(TransportError::ProviderStreamFailure { event:actual, diagnostic:None }) if *actual == event)
                );
                assert_eq!(
                    block.reason,
                    InferenceBlockReason::ProviderStreamFailure {
                        event,
                        diagnostic: None
                    }
                );
                Some(terminal)
            }
        };
        let expected_state = if status.is_none() {
            ProviderResponseState::Interrupted
        } else {
            ProviderResponseState::Failed
        };
        let observed = summary(&store, &block.request);
        assert_eq!(observed.terminal, expected_terminal);
        assert_eq!(
            observed.end,
            if status.is_none() {
                ResponseEnd::CleanEof
            } else {
                ResponseEnd::ProviderTerminal
            }
        );
        assert!(!serde_json::to_string(&observed).unwrap().contains(KEY));
        let items = store.items(&block.request).unwrap();
        assert_eq!(
            items
                .iter()
                .filter(|i| i.0.get("gemini_step").is_some())
                .cloned()
                .collect::<Vec<_>>(),
            vec![project(native.clone(), "failed-round:0").unwrap()]
        );
        assert!(
            matches!(engine.run_recovering_embedded(None, vec![], cancel,
            tokio::sync::mpsc::unbounded_channel().1).await, Err(EngineError::InferenceBlocked { block:actual }) if actual == block)
        );
        assert_eq!(
            http.requests.load(Ordering::SeqCst),
            1,
            "escaped prefix/provider terminal never grants retry"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        drop(engine);
        drop(store);
        let reopened = http.store();
        assert_eq!(reopened.items(&block.request).unwrap(), items);
        assert_eq!(
            reopened.provider_response_state(&block.request).unwrap(),
            Some(expected_state)
        );
        assert_eq!(
            reopened.embedded_inference_state(&identity()).unwrap(),
            HostInferenceState::Blocked { block }
        );
    }
}

#[tokio::test]
async fn escaped_tool_before_http_eof_executes_once_and_never_authorizes_retry() {
    let effect = Arc::new(Notify::new());
    let call = json!({"type":"function_call","id":"once","name":"effect",
        "arguments":{"arguments":{},"delivery":"wait"}});
    let http = LocalHttp::new(
        vec![wire(vec![
            created("escaped"),
            json!({"event_type":"step.start","index":0,"step":call}),
            json!({"event_type":"step.stop","index":0}),
        ])],
        Some(effect.clone()),
    )
    .await;
    let store = http.store();
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = engine(
        &http,
        store.clone(),
        Effect {
            calls: calls.clone(),
            completed: Some(effect),
        },
    )
    .await;
    let (_stop, cancel) = watch::channel(false);
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        engine.run_embedded(
            None,
            vec![Item::user_text("execute only the completed call")],
            cancel,
            tokio::sync::mpsc::unbounded_channel().1,
        ),
    )
    .await;
    match &outcome {
        Err(error) => eprintln!("escaped effect timeout: {error:?}"),
        Ok(Err(error)) => eprintln!("escaped effect typed error: {error:?}"),
        Ok(Ok(_)) => eprintln!("escaped effect unexpectedly completed"),
    }
    http.diagnose("escaped effect", &store, &calls);
    let error = outcome.unwrap().unwrap_err();
    let EngineError::BlockedAction(failure) = error else {
        panic!("lost interruption block: {error:?}");
    };
    let block = failure.block().clone();
    assert!(matches!(
        block.reason,
        InferenceBlockReason::StreamInterrupted {
            cause: StreamInterruption::MissingCompletion
        }
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(http.requests.load(Ordering::SeqCst), 1);
    let items = store.items(&block.request).unwrap();
    assert_eq!(
        items.iter().filter(|i| i.0["gemini_step"] == call).count(),
        1
    );
    let claims = store.claims_on(&block.request).unwrap();
    assert_eq!(claims.len(), 1);
    assert_ne!(claims[0].state, crate::store::ClaimState::Pending);
    assert!(
        store
            .reserve_automatic_retry(&identity(), &block)
            .unwrap()
            .is_none()
    );
    drop(engine);
    drop(store);
    let reopened = http.store();
    assert_eq!(reopened.items(&block.request).unwrap(), items);
    assert_eq!(
        reopened.provider_response_state(&block.request).unwrap(),
        Some(ProviderResponseState::Interrupted)
    );
    assert_eq!(
        reopened.embedded_inference_state(&identity()).unwrap(),
        HostInferenceState::Blocked { block }
    );
}

#[tokio::test]
async fn malformed_http_is_failed_protocol_not_interrupted_or_declared_terminal() {
    let http = LocalHttp::new(
        vec![wire(vec![
            created("malformed"),
            json!({"event_type":"step.stop","index":0}),
        ])],
        None,
    )
    .await;
    let store = http.store();
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = engine(
        &http,
        store.clone(),
        Effect {
            calls: calls.clone(),
            completed: None,
        },
    )
    .await
    .with_origin(store.standalone_identity(identity().actor));
    let (_stop, cancel) = watch::channel(false);
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        engine.run(
            None,
            vec![Item::user_text("refuse malformed stream")],
            cancel,
            tokio::sync::mpsc::unbounded_channel().1,
        ),
    )
    .await;
    match &outcome {
        Err(error) => eprintln!("malformed stream timeout: {error:?}"),
        Ok(Err(error)) => eprintln!("malformed stream typed error: {error:?}"),
        Ok(Ok(_)) => eprintln!("malformed stream unexpectedly completed"),
    }
    http.diagnose("malformed stream", &store, &calls);
    let error = outcome.unwrap().unwrap_err();
    assert!(matches!(
        error,
        EngineError::Transport(TransportError::Stream(_))
    ));
    let events = store.events(None).unwrap();
    let observed = events
        .iter()
        .find(|e| e.kind == "responses_transport")
        .unwrap();
    let request = observed.request.clone().unwrap();
    assert_eq!(summary(&store, &request).end, ResponseEnd::ProtocolError);
    assert_eq!(summary(&store, &request).terminal, None);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(http.requests.load(Ordering::SeqCst), 1);
    drop(engine);
    drop(store);
    assert_eq!(
        http.store().provider_response_state(&request).unwrap(),
        Some(ProviderResponseState::Failed)
    );
}

#[test]
fn malformed_events_remain_protocol_errors_and_incomplete_ids_are_bounded_and_redacted() {
    for event in [
        json!({"event_type":"step.stop","index":0}),
        json!({"event_type":"interaction.status_update","interaction_id":"bad","status":"unknown"}),
    ] {
        let mut assembly = Assembly::default();
        let mut observed = TransportObservation::new(ResponseEnd::ProtocolError);
        assert!(matches!(
            assembly
                .advance(&event.to_string(), KEY, "", &mut observed)
                .map_err(ProtocolFault::into_error),
            Err(TransportError::Stream(_))
        ));
        assert_eq!(observed.terminal, None);
    }
    let mut assembly = Assembly::default();
    let mut observed = TransportObservation::new(ResponseEnd::ProviderTerminal);
    let event = json!({"event_type":"interaction.status_update","interaction_id":format!("{KEY}{}", "λ".repeat(200)),"status":"incomplete"});
    let error = assembly
        .advance(&event.to_string(), KEY, "", &mut observed)
        .unwrap()
        .into_events(Some(&mut observed))
        .unwrap_err();
    let TransportError::ResponseIncomplete {
        response_id,
        reason: None,
    } = error
    else {
        panic!("wrong terminal");
    };
    assert!(response_id.len() <= 128);
    assert!(response_id.starts_with("[redacted]"));
    assert!(!response_id.contains(KEY));
    assert!(observed.metadata_truncated);
}

proptest! {
    #[test]
    fn segmented_unicode_text_preserves_native_bytes_and_visible_deltas(
        fragments in proptest::collection::vec(proptest::collection::vec(any::<char>(), 0..12), 0..40)
    ) {
        let fragments = fragments.into_iter().map(|chars| chars.into_iter().collect::<String>()).collect::<Vec<_>>();
        let mut expected = String::from("initial λ\r\n");
        expected.extend(fragments.iter().map(String::as_str));
        let mut assembly = Assembly::default();
        let mut observed = TransportObservation::new(ResponseEnd::ProviderTerminal);
        let mut send = |event: Value| assembly.advance(&event.to_string(), "", "", &mut observed)
            .map_err(ProtocolFault::into_error)?.into_events(Some(&mut observed));
        send(created("text")).unwrap();
        send(json!({"event_type":"step.start","index":0,"step":output("initial λ\r\n")})).unwrap();
        let mut visible = String::new();
        for fragment in &fragments {
            let events = send(json!({"event_type":"step.delta","index":0,"delta":{"type":"text","text":fragment}})).unwrap();
            prop_assert_eq!(events.len(), 1);
            let StreamEvent::Delta { text, channel:OutputChannel::Assistant, .. } = &events[0] else { panic!("wrong delta"); };
            visible.push_str(text);
        }
        prop_assert_eq!(visible, fragments.concat());
        let emitted = send(json!({"event_type":"step.stop","index":0})).unwrap();
        send(json!({"event_type":"interaction.completed","interaction":{"id":"text","status":"completed"}})).unwrap();
        let turn = assembly.finish_complete().unwrap();
        prop_assert_eq!(&turn.items[0].0["gemini_step"], &output(&expected));
        let StreamEvent::CompletedItem(done) = &emitted[0] else { panic!("missing complete item"); };
        prop_assert_eq!(done.item(), &turn.items[0]);
    }
}

#[tokio::test]
async fn native_completion_with_invalid_output_is_failed_and_blocked_after_reopen() {
    // A native completion fact is preserved, but a missing output position
    // cannot issue a successful turn, effect authority or interruption retry.
    let http = LocalHttp::new(vec![wire(vec![
        created("invalid-completion"),
        json!({"event_type":"step.start","index":1,"step":output("gapped λ")}),
        json!({"event_type":"step.stop","index":1}),
        json!({"event_type":"interaction.completed","interaction":{"id":"invalid-completion","status":"completed"}}),
    ])], None).await;
    let store = http.store();
    let calls = Arc::new(AtomicUsize::new(0));
    let engine = engine(
        &http,
        store.clone(),
        Effect {
            calls: calls.clone(),
            completed: None,
        },
    )
    .await;
    let (_stop, cancel) = watch::channel(false);
    let error = tokio::time::timeout(
        Duration::from_secs(10),
        engine.run_embedded(
            None,
            vec![Item::user_text("validate completed native output")],
            cancel,
            tokio::sync::mpsc::unbounded_channel().1,
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    let EngineError::BlockedAction(failure) = error else {
        panic!("lost block: {error:?}");
    };
    assert!(matches!(
        failure.primary(),
        EngineError::Transport(TransportError::IncompleteOutputIndices { index: 0 })
    ));
    let block = failure.block().clone();
    assert_eq!(block.reason, InferenceBlockReason::ContinuityUnproven);
    assert_eq!(
        summary(&store, &block.request).terminal,
        Some(ResponseTerminal::Completed)
    );
    assert_eq!(
        summary(&store, &block.request).end,
        ResponseEnd::ProviderTerminal
    );
    assert_eq!(
        store.provider_response_state(&block.request).unwrap(),
        Some(ProviderResponseState::Failed)
    );
    assert!(
        store
            .items(&block.request)
            .unwrap()
            .iter()
            .all(|item| item.0.get("gemini_step").is_none())
    );
    assert!(store.claims_on(&block.request).unwrap().is_empty());
    assert!(
        store
            .reserve_automatic_retry(&identity(), &block)
            .unwrap()
            .is_none()
    );
    assert_eq!(http.requests.load(Ordering::SeqCst), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    drop(engine);
    drop(store);
    let reopened = http.store();
    assert_eq!(
        reopened.provider_response_state(&block.request).unwrap(),
        Some(ProviderResponseState::Failed)
    );
    assert_eq!(
        reopened.embedded_inference_state(&identity()).unwrap(),
        HostInferenceState::Blocked { block }
    );
}
