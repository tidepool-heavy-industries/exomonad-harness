//! A consumer outside the library implementing the host capability directly.
use async_trait::async_trait;
use harness::{
    embedding::{
        AdmissionGuard, Conversation, EmbeddedError, HostActor, HostControl, HostIdentity,
        InputObservation, ToolSurface,
    },
    engine::{EngineConfig, ResponsesTransport},
    item::Item,
    mailbox::DurableMailboxWake,
    model::{AgentPath, CallId, Effort, RequestId},
    provider::{CallContext, Provider, ProviderError},
    store::Store,
    transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError},
    turn::{JobOutput, JobScheduler},
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex, RwLock,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Permit;
impl AdmissionGuard for Permit {}
struct Host {
    identity: HostIdentity,
    surface: RwLock<Arc<ToolSurface>>,
    alive: AtomicBool,
    wakes: AtomicUsize,
    wake_tx: Mutex<Option<tokio::sync::mpsc::UnboundedSender<DurableMailboxWake>>>,
    store: Arc<Store>,
}
#[async_trait]
impl HostActor for Host {
    fn identity(&self) -> &HostIdentity {
        &self.identity
    }
    fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError> {
        if self.alive.load(Ordering::SeqCst) {
            Ok(Box::new(Permit))
        } else {
            Err(EmbeddedError::Host("retired".into()))
        }
    }
    fn tool_surface(&self) -> Result<Arc<ToolSurface>, EmbeddedError> {
        Ok(self.surface.read().unwrap().clone())
    }
    async fn wake(&self, envelope_id: i64) -> Result<(), String> {
        assert!(
            !self.store.inbox(&self.identity.actor.0).unwrap().is_empty(),
            "input committed before wake"
        );
        self.wakes.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = self.wake_tx.lock().unwrap().as_ref() {
            for _ in 0..2 {
                sender
                    .send(DurableMailboxWake { envelope_id })
                    .map_err(|_| "embedded Engine wake receiver closed".to_string())?;
            }
        }
        Ok(())
    }
    async fn control(&self, _: HostControl) -> Result<Value, String> {
        self.alive.store(false, Ordering::SeqCst);
        Ok(json!({"requested":true}))
    }
}
struct Dispatch {
    version: &'static str,
    seen: Arc<Mutex<Vec<Value>>>,
}
#[async_trait]
impl Provider for Dispatch {
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("identity required")
    }
    async fn call_custom_with_context(
        &self,
        _: &str,
        input: String,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        assert!(context.request.is_some());
        self.seen
            .lock()
            .unwrap()
            .push(json!({"version":self.version,"input":input,"call":context.call_id.0}));
        Ok(json!(self.version))
    }
}
fn surface(version: &'static str, seen: Arc<Mutex<Vec<Value>>>) -> Arc<ToolSurface> {
    Arc::new(
        ToolSurface::new(
            version.into(),
            vec![json!({"type":"custom","name":"haskell","format":{"type":"text"},"async":true})],
            Arc::new(Dispatch { version, seen }),
        )
        .unwrap(),
    )
}
fn host(store: Arc<Store>, seen: Arc<Mutex<Vec<Value>>>) -> Arc<Host> {
    Arc::new(Host {
        identity: HostIdentity {
            run: "run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "one".into(),
        },
        surface: RwLock::new(surface("old", seen)),
        alive: AtomicBool::new(true),
        wakes: AtomicUsize::new(0),
        wake_tx: Mutex::new(None),
        store,
    })
}
#[derive(Clone)]
struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("offline transport")
    }
}
struct ReloadDuringRequest {
    host: Arc<Host>,
    seen: Arc<Mutex<Vec<Value>>>,
    requests: AtomicUsize,
    jobs: Arc<JobScheduler>,
}
#[async_trait]
impl ResponsesTransport for ReloadDuringRequest {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        assert_eq!(
            request.tools.len(),
            1,
            "only the host tool surface is exposed"
        );
        assert_eq!(request.tools[0]["name"], "haskell");
        let round = self.requests.fetch_add(1, Ordering::SeqCst);
        let items = if round == 0 {
            *self.host.surface.write().unwrap() = surface("new", self.seen.clone());
            vec![Item(
                json!({"type":"custom_tool_call","name":"haskell","call_id":"old-call","input":"λ x → x\n\"raw\""}),
            )]
        } else {
            let old = self.host.store.claims(&CallId("old-call".into())).unwrap()[0]
                .operation
                .clone();
            self.jobs.wait(&old).await.unwrap();
            if round == 1 {
                vec![Item(
                    json!({"type":"custom_tool_call","name":"haskell","call_id":"new-call","input":"new"}),
                )]
            } else {
                let new = self.host.store.claims(&CallId("new-call".into())).unwrap()[0]
                    .operation
                    .clone();
                self.jobs.wait(&new).await.unwrap();
                vec![Item(
                    json!({"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"done"}]}),
                )]
            }
        };
        Ok(ResponsesTurn {
            response_id: format!("r{round}"),
            items,
            usage: Default::default(),
        })
    }
}
#[tokio::test]
async fn embedded_requests_pin_dispatch_and_inputs_record_actual_inclusion() {
    let store = Arc::new(Store::memory().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let host = host(store.clone(), seen.clone());
    let conversation = Conversation::attach(store.clone(), host.clone(), None).unwrap();
    let receipt = conversation
        .input("message-1", "operator", "do work")
        .await
        .unwrap();
    assert_eq!(
        conversation
            .input("message-1", "operator", "do work")
            .await
            .unwrap()
            .envelope_id,
        receipt.envelope_id
    );
    assert!(matches!(
        conversation
            .input("message-1", "operator", "different")
            .await,
        Err(EmbeddedError::ConflictingInput)
    ));
    assert_eq!(
        conversation.input_observation(receipt.envelope_id).unwrap(),
        InputObservation::Admitted
    );
    let jobs = Arc::new(JobScheduler::new(2).unwrap());
    let engine = conversation
        .engine::<Offline, _>(
            ReloadDuringRequest {
                host: host.clone(),
                seen: seen.clone(),
                requests: AtomicUsize::new(0),
                jobs: jobs.clone(),
            },
            jobs,
            EngineConfig {
                instructions: "instructions".into(),
                tools: vec![],
                model: "offline".into(),
                effort: Effort::Medium,
                session_id: "session".into(),
                agent: host.identity.actor.clone(),
            },
            std::num::NonZeroU64::new(200_000).unwrap(),
        )
        .unwrap();
    let (_cancel, rx) = tokio::sync::watch::channel(false);
    let (_send, incoming) = tokio::sync::mpsc::unbounded_channel();
    engine.run(None, vec![], rx, incoming).await.unwrap();
    assert!(matches!(
        conversation.input_observation(receipt.envelope_id).unwrap(),
        InputObservation::Included(_)
    ));
    let wakes = host.wakes.load(Ordering::SeqCst);
    assert_eq!(
        conversation
            .input("message-1", "operator", "do work")
            .await
            .unwrap()
            .envelope_id,
        receipt.envelope_id
    );
    assert_eq!(
        host.wakes.load(Ordering::SeqCst),
        wakes,
        "an included input retry must not wake another model turn"
    );
    let observed = seen.lock().unwrap();
    assert_eq!(observed.len(), 2);
    assert_eq!(observed[0]["version"], "old");
    assert_eq!(observed[0]["input"], "λ x → x\n\"raw\"");
    assert_eq!(observed[1]["version"], "new");
    assert_eq!(
        store
            .events(None)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "tool_surface")
            .count(),
        3
    );
}

#[derive(Clone)]
struct PendingDispatch {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl Provider for PendingDispatch {
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("identity required")
    }
    async fn call_custom_with_context(
        &self,
        name: &str,
        input: String,
        _: CallContext,
    ) -> Result<Value, ProviderError> {
        assert_eq!(name, "haskell");
        if input == "slow" {
            self.started.notify_one();
            self.release.notified().await;
        }
        Ok(json!({"result":input}))
    }
}

#[derive(Clone)]
struct ResumeAfterInput {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    first_final: Arc<tokio::sync::Notify>,
    second_request: Arc<tokio::sync::Notify>,
    pending_final: Arc<tokio::sync::Notify>,
    fourth_request: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl ResponsesTransport for ResumeAfterInput {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        assert_eq!(
            request.tools.len(),
            1,
            "only the host tool surface is exposed"
        );
        assert_eq!(request.tools[0]["name"], "haskell");
        let mut requests = self.requests.lock().unwrap();
        requests.push(request);
        let round = requests.len();
        let current = requests.last().unwrap();
        let items = match round {
            1 => {
                self.first_final.notify_one();
                vec![
                    Item(json!({
                        "type":"custom_tool_call", "call_id":"slow-cell",
                        "name":"haskell", "input":"slow"
                    })),
                    Item(json!({
                        "type":"message", "role":"assistant", "phase":"final_answer",
                        "content":[{"type":"output_text","text":"waiting for cell"}]
                    })),
                ]
            }
            2 => {
                self.second_request.notify_one();
                let input_count = current
                    .input
                    .iter()
                    .filter(|item| item.0["type"] == "message" && item.0["content"] == "wake me")
                    .count();
                assert_eq!(input_count, 1, "the durable wake attaches the input once");
                vec![Item(json!({
                    "type":"custom_tool_call", "call_id":"fast-cell",
                    "name":"haskell", "input":"fast"
                }))]
            }
            3 => {
                self.pending_final.notify_one();
                vec![Item(json!({
                    "type":"message", "role":"assistant", "phase":"final_answer",
                    "content":[{"type":"output_text","text":"waiting for remaining cell"}]
                }))]
            }
            4 => {
                self.fourth_request.notify_one();
                vec![Item(json!({
                    "type":"message", "role":"assistant", "phase":"final_answer",
                    "content":[{"type":"output_text","text":"done"}]
                }))]
            }
            _ => return Err(TransportError::Stream("unexpected extra request".into())),
        };
        Ok(ResponsesTurn {
            response_id: format!("resume-{round}"),
            items,
            usage: Default::default(),
        })
    }
}

#[tokio::test]
async fn embedded_final_waits_internally_and_durable_input_resumes_once() {
    let store = Arc::new(Store::memory().unwrap());
    let host = host(store.clone(), Arc::new(Mutex::new(vec![])));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *host.surface.write().unwrap() = Arc::new(
        ToolSurface::new(
            "delayed".into(),
            vec![json!({"type":"custom","name":"haskell","format":{"type":"text"}})],
            Arc::new(PendingDispatch {
                started: started.clone(),
                release: release.clone(),
            }),
        )
        .unwrap(),
    );
    let conversation = Conversation::attach(store.clone(), host.clone(), None).unwrap();
    let (wake_tx, wake_rx) = tokio::sync::mpsc::unbounded_channel();
    *host.wake_tx.lock().unwrap() = Some(wake_tx);
    let transport = ResumeAfterInput {
        requests: Arc::new(Mutex::new(vec![])),
        first_final: Arc::new(tokio::sync::Notify::new()),
        second_request: Arc::new(tokio::sync::Notify::new()),
        pending_final: Arc::new(tokio::sync::Notify::new()),
        fourth_request: Arc::new(tokio::sync::Notify::new()),
    };
    let engine = conversation
        .engine::<Offline, _>(
            transport.clone(),
            Arc::new(JobScheduler::new(2).unwrap()),
            EngineConfig {
                instructions: "instructions".into(),
                tools: vec![],
                model: "offline".into(),
                effort: Effort::Medium,
                session_id: "session".into(),
                agent: host.identity.actor.clone(),
            },
            std::num::NonZeroU64::new(200_000).unwrap(),
        )
        .unwrap();
    let (_cancel, cancellation) = tokio::sync::watch::channel(false);
    let running = tokio::spawn(async move {
        engine
            .run_embedded(
                None,
                vec![Item(json!({
                    "type":"message","role":"user","content":"start"
                }))],
                cancellation,
                wake_rx,
            )
            .await
    });
    started.notified().await;
    transport.first_final.notified().await;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(30),
            transport.second_request.notified()
        )
        .await
        .is_err(),
        "a final response with pending work parks instead of spinning"
    );

    let receipt = conversation
        .input("operator-1", "operator", "wake me")
        .await
        .unwrap();
    transport.second_request.notified().await;
    transport.pending_final.notified().await;
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(30),
            transport.fourth_request.notified()
        )
        .await
        .is_err(),
        "the next final also waits while its original operation is pending"
    );
    assert_eq!(transport.requests.lock().unwrap().len(), 3);
    release.notify_one();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        transport.fourth_request.notified(),
    )
    .await
    .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    let final_inputs = &requests[3].input;
    assert_eq!(
        final_inputs
            .iter()
            .filter(|item| {
                item.0["type"] == "custom_tool_call_output" && item.0["call_id"] == "slow-cell"
            })
            .count(),
        1,
        "the pending host operation settles on its original call identity"
    );
    let InputObservation::Included(included_request) =
        conversation.input_observation(receipt.envelope_id).unwrap()
    else {
        panic!("the durable input was not included");
    };
    assert_eq!(
        store
            .items(&included_request)
            .unwrap()
            .iter()
            .filter(|item| item.0["content"] == "wake me")
            .count(),
        1,
        "duplicate durable wake hints cannot duplicate Store content"
    );
    let wake_count = host.wakes.load(Ordering::SeqCst);
    conversation
        .input("operator-1", "operator", "wake me")
        .await
        .unwrap();
    assert_eq!(host.wakes.load(Ordering::SeqCst), wake_count);
}

#[derive(Clone)]
struct FinalOnly(Arc<Mutex<Vec<ResponsesRequest>>>);

#[async_trait]
impl ResponsesTransport for FinalOnly {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.0.lock().unwrap().push(request);
        Ok(ResponsesTurn {
            response_id: "final".into(),
            items: vec![Item(json!({
                "type":"message", "role":"assistant", "phase":"final_answer",
                "content":[{"type":"output_text","text":"done"}]
            }))],
            usage: Default::default(),
        })
    }
}

#[tokio::test]
async fn committed_input_survives_reconnect_without_a_wake_hint() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("embedded.sqlite");
    let store = Arc::new(Store::open(&path).unwrap());
    let initial_host = host(store.clone(), Arc::new(Mutex::new(vec![])));
    let conversation = Conversation::attach(store.clone(), initial_host, None).unwrap();
    let receipt = conversation
        .input("before-reconnect", "operator", "retained")
        .await
        .unwrap();
    drop(conversation);
    drop(store);

    let store = Arc::new(Store::open(&path).unwrap());
    let host = host(store.clone(), Arc::new(Mutex::new(vec![])));
    let conversation = Conversation::attach(store.clone(), host.clone(), None).unwrap();
    let transport = FinalOnly(Arc::new(Mutex::new(vec![])));
    let engine = conversation
        .engine::<Offline, _>(
            transport.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            EngineConfig {
                instructions: "instructions".into(),
                tools: vec![],
                model: "offline".into(),
                effort: Effort::Medium,
                session_id: "session".into(),
                agent: host.identity.actor.clone(),
            },
            std::num::NonZeroU64::new(200_000).unwrap(),
        )
        .unwrap();
    let (_cancel, cancellation) = tokio::sync::watch::channel(false);
    let (_unused, incoming) = tokio::sync::mpsc::unbounded_channel();
    engine
        .run_recovering_embedded(None, vec![], cancellation, incoming)
        .await
        .unwrap();
    let seen = transport.0.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0]
            .input
            .iter()
            .filter(|item| item.0["content"] == "retained")
            .count(),
        1
    );
    assert!(matches!(
        conversation.input_observation(receipt.envelope_id).unwrap(),
        InputObservation::Included(_)
    ));
}

#[tokio::test]
async fn unavailable_host_snapshot_fails_before_model_request() {
    struct Unavailable(Arc<Host>);

    #[async_trait]
    impl HostActor for Unavailable {
        fn identity(&self) -> &HostIdentity {
            self.0.identity()
        }
        fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError> {
            self.0.admit()
        }
        fn tool_surface(&self) -> Result<Arc<ToolSurface>, EmbeddedError> {
            Err(EmbeddedError::Surface("source lease retired".into()))
        }
        async fn wake(&self, envelope_id: i64) -> Result<(), String> {
            self.0.wake(envelope_id).await
        }
        async fn control(&self, control: HostControl) -> Result<Value, String> {
            self.0.control(control).await
        }
    }

    let store = Arc::new(Store::memory().unwrap());
    let host = Arc::new(Unavailable(host(
        store.clone(),
        Arc::new(Mutex::new(vec![])),
    )));
    let conversation = Conversation::attach(store.clone(), host.clone(), None).unwrap();
    let transport = FinalOnly(Arc::new(Mutex::new(vec![])));
    let engine = conversation
        .engine::<Offline, _>(
            transport.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            EngineConfig {
                instructions: "instructions".into(),
                tools: vec![],
                model: "offline".into(),
                effort: Effort::Medium,
                session_id: "session".into(),
                agent: host.identity().actor.clone(),
            },
            std::num::NonZeroU64::new(200_000).unwrap(),
        )
        .unwrap();
    let (_cancel, cancellation) = tokio::sync::watch::channel(false);
    let (_wake_tx, incoming) = tokio::sync::mpsc::unbounded_channel();
    let result = engine
        .run_embedded(
            None,
            vec![Item(
                json!({"type":"message", "role":"user", "content":"hello"}),
            )],
            cancellation,
            incoming,
        )
        .await;
    assert!(
        result.is_err(),
        "unavailable snapshot must fail the request"
    );
    assert!(transport.0.lock().unwrap().is_empty());
}

#[derive(Clone)]
struct NativeWaitAttempt(Arc<Mutex<Vec<ResponsesRequest>>>);

#[async_trait]
impl ResponsesTransport for NativeWaitAttempt {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0]["name"], "haskell");
        self.0.lock().unwrap().push(request);
        Ok(ResponsesTurn {
            response_id: "native-wait-attempt".into(),
            items: vec![Item(json!({
                "type":"function_call", "call_id":"native-wait-call",
                "name":"wait_agent", "arguments":"{}"
            }))],
            usage: Default::default(),
        })
    }
}

#[tokio::test]
async fn embedded_surface_does_not_advertise_or_accept_native_wait_agent() {
    let store = Arc::new(Store::memory().unwrap());
    let host = host(store.clone(), Arc::new(Mutex::new(vec![])));
    let conversation = Conversation::attach(store.clone(), host.clone(), None).unwrap();
    let transport = NativeWaitAttempt(Arc::new(Mutex::new(vec![])));
    let engine = conversation
        .engine::<Offline, _>(
            transport.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            EngineConfig {
                instructions: "instructions".into(),
                tools: vec![],
                model: "offline".into(),
                effort: Effort::Medium,
                session_id: "session".into(),
                agent: host.identity.actor.clone(),
            },
            std::num::NonZeroU64::new(200_000).unwrap(),
        )
        .unwrap();
    let (_cancel, cancellation) = tokio::sync::watch::channel(false);
    let (_unused, incoming) = tokio::sync::mpsc::unbounded_channel();
    let result = engine
        .run_embedded(
            None,
            vec![Item(
                json!({"type":"message","role":"user","content":"start"}),
            )],
            cancellation,
            incoming,
        )
        .await;
    assert!(matches!(
        result,
        Err(harness::engine::EngineError::ProviderCall(_))
    ));
    assert!(
        store
            .claims(&CallId("native-wait-call".into()))
            .unwrap()
            .is_empty(),
        "undeclared native wait call fails before admission"
    );
}

#[tokio::test]
async fn embedded_binding_rejects_foreign_context_kind_and_retired_input() {
    let store = Arc::new(Store::memory().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let host = host(store.clone(), seen.clone());
    let conversation = Conversation::attach(store.clone(), host.clone(), None).unwrap();
    let request = RequestId("request".into());
    store
        .write_request(&request, None, "/root", &[], Default::default())
        .unwrap();
    let provider = conversation.provider().request_snapshot().unwrap().unwrap();
    let jobs = JobScheduler::new(1).unwrap();
    jobs.start_for_agent(
        provider.clone(),
        host.identity.actor.clone(),
        Some(request.clone()),
        CallId("wrong-kind".into()),
        "haskell".into(),
        json!({}),
    )
    .await
    .unwrap();
    assert!(matches!(
        jobs.wait(&CallId("wrong-kind".into())).await.unwrap(),
        JobOutput::Completed(Err(_))
    ));
    jobs.start_for_agent(
        provider,
        AgentPath("/root/foreign".into()),
        Some(request),
        CallId("foreign".into()),
        "haskell".into(),
        harness::item::ToolInput::Custom("no".into()),
    )
    .await
    .unwrap();
    assert!(matches!(
        jobs.wait(&CallId("foreign".into())).await.unwrap(),
        JobOutput::Completed(Err(_))
    ));
    assert!(seen.lock().unwrap().is_empty());
    conversation.control(HostControl::Retire).await.unwrap();
    assert!(
        conversation
            .input("after-retirement", "operator", "no")
            .await
            .is_err()
    );
    assert!(store.inbox("/root").unwrap().is_empty());
}

#[tokio::test]
async fn embedded_browser_login_input_history_and_reconnect_use_external_owner() {
    use futures_util::StreamExt;
    use harness::server::{
        self, ClientCommand, HostActorIdentity, HostActorKind, HostActorLifecycle,
        HostActorProjection, ServerConfig, SessionSecret, Snapshot,
    };
    use tokio_tungstenite::{connect_async, tungstenite::client::IntoClientRequest};
    let store = Arc::new(Store::memory().unwrap());
    let seen = Arc::new(Mutex::new(vec![]));
    let host = host(store.clone(), seen.clone());
    let conversation = Conversation::attach(store.clone(), host.clone(), None).unwrap();
    let secret = "embedded-browser-offline-test-secret-32-bytes";
    let assets = std::env::var_os("HARNESS_WEB_DIST")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(
                option_env!("CARGO_MANIFEST_DIR").expect("manifest path is required"),
            )
            .join("../../web/dist")
        });
    let (router, control, mut commands) = server::server_with_config(
        ServerConfig::new(assets)
            .with_history_store(store.clone())
            .with_browser_session(
                SessionSecret::new(secret).unwrap(),
                std::time::Duration::from_secs(60),
            )
            .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, stopped) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    let origin = format!("http://{address}");
    let client = reqwest::Client::new();
    assert_eq!(
        client
            .get(format!("{origin}/"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        client
            .get(format!("{origin}/api/history/absent"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let login = client
        .post(format!("{origin}/api/session"))
        .header("Origin", &origin)
        .json(&json!({"secret":secret}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let response = client
        .post(format!("{origin}/api/commands"))
        .header("Origin", &origin)
        .header("Cookie", &cookie)
        .json(&ClientCommand::Submit {
            command: "run deterministic cells".into(),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    let command = commands.recv().await.unwrap();
    let ClientCommand::Submit { command: text } = command.command else {
        panic!("expected standalone input command");
    };
    let receipt = conversation
        .input(&command.command_id, "operator", &text)
        .await
        .unwrap();
    assert_eq!(
        conversation.input_observation(receipt.envelope_id).unwrap(),
        InputObservation::Admitted
    );
    let jobs = Arc::new(JobScheduler::new(2).unwrap());
    let engine = conversation
        .engine::<Offline, _>(
            ReloadDuringRequest {
                host: host.clone(),
                seen: seen.clone(),
                requests: AtomicUsize::new(0),
                jobs: jobs.clone(),
            },
            jobs,
            EngineConfig {
                instructions: "use the actual host tools".into(),
                tools: vec![],
                model: "offline".into(),
                effort: Effort::Medium,
                session_id: "browser".into(),
                agent: host.identity.actor.clone(),
            },
            std::num::NonZeroU64::new(200_000).unwrap(),
        )
        .unwrap();
    let (_cancel, cancelled) = tokio::sync::watch::channel(false);
    let (_wake, incoming) = tokio::sync::mpsc::unbounded_channel();
    let completion = engine.run(None, vec![], cancelled, incoming).await.unwrap();
    let identity = HostActorIdentity {
        run: "run".into(),
        actor: AgentPath("/root".into()),
        incarnation: "one".into(),
    };
    control.set_snapshot(Snapshot {
        actors: vec![
            HostActorProjection {
                identity: identity.clone(),
                parent: None,
                kind: HostActorKind::Model,
                lifecycle: HostActorLifecycle::Waiting,
                model_conversation: Some("/root".into()),
            },
            HostActorProjection {
                identity: HostActorIdentity {
                    run: "run".into(),
                    actor: AgentPath("/root/workflow".into()),
                    incarnation: "workflow-one".into(),
                },
                parent: Some(identity),
                kind: HostActorKind::Workflow,
                lifecycle: HostActorLifecycle::Running,
                model_conversation: None,
            },
        ],
        conversations: vec![json!({"id":"/root","path":"/root","state":"idle"})],
        requests: vec![
            json!({"id":completion.head_request.0,"conversationId":"/root","state":"completed"}),
        ],
        ..Snapshot::default()
    });
    for _ in 0..2 {
        let mut request = format!("ws://{address}/api/ws")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("Origin", origin.parse().unwrap());
        request
            .headers_mut()
            .insert("Cookie", cookie.parse().unwrap());
        let (mut socket, _) = connect_async(request).await.unwrap();
        let snapshot: Value =
            serde_json::from_str(socket.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
        assert_eq!(snapshot["snapshot"]["actors"].as_array().unwrap().len(), 2);
        assert_eq!(snapshot["snapshot"]["actors"][1]["kind"], "workflow");
        socket.close(None).await.unwrap();
    }
    assert_eq!(
        seen.lock().unwrap().len(),
        2,
        "reconnect never repeats tools"
    );
    let history = client
        .get(format!(
            "{origin}/api/history/{}",
            completion.head_request.0
        ))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(history.status(), 200);
    let history: Value = history.json().await.unwrap();
    assert!(history.to_string().contains("done"));
    assert!(matches!(
        conversation.input_observation(receipt.envelope_id).unwrap(),
        InputObservation::Included(_)
    ));
    conversation.control(HostControl::Retire).await.unwrap();
    assert!(conversation.input("after", "operator", "no").await.is_err());
    shutdown.send(()).unwrap();
    serving.await.unwrap();
}

#[tokio::test]
async fn structured_host_calls_retain_progress_and_admitted_input_survives_failed_wake() {
    struct Structured;
    #[async_trait]
    impl Provider for Structured {
        fn tools(&self) -> Vec<Value> {
            vec![]
        }
        async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
            panic!("context required")
        }
        async fn call_with_context(
            &self,
            _: &str,
            args: Value,
            context: CallContext,
        ) -> Result<Value, ProviderError> {
            context
                .progress
                .try_send(json!({"step":"admitted"}))
                .unwrap();
            Ok(args)
        }
    }
    struct WakeFailure(Arc<Host>);
    #[async_trait]
    impl HostActor for WakeFailure {
        fn identity(&self) -> &HostIdentity {
            &self.0.identity
        }
        fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError> {
            self.0.admit()
        }
        fn tool_surface(&self) -> Result<Arc<ToolSurface>, EmbeddedError> {
            self.0.tool_surface()
        }
        async fn wake(&self, _: i64) -> Result<(), String> {
            Err("host wake queue temporarily closed".into())
        }
        async fn control(&self, c: HostControl) -> Result<Value, String> {
            self.0.control(c).await
        }
    }
    let store = Arc::new(Store::memory().unwrap());
    let host = host(store.clone(), Arc::new(Mutex::new(vec![])));
    *host.surface.write().unwrap()=Arc::new(ToolSurface::new("structured".into(),vec![json!({"type":"function","name":"echo","strict":true,"parameters":{"type":"object","properties":{"answer":{"type":"integer"}},"required":["answer"],"additionalProperties":false}})],Arc::new(Structured)).unwrap());
    let conversation =
        Conversation::attach(store.clone(), Arc::new(WakeFailure(host)), None).unwrap();
    let first = conversation
        .input("once", "operator", "hello")
        .await
        .unwrap();
    assert!(first.wake_error.is_some());
    assert_eq!(
        conversation
            .input("once", "operator", "hello")
            .await
            .unwrap()
            .envelope_id,
        first.envelope_id
    );
    assert_eq!(store.inbox("/root").unwrap().len(), 1);
    assert_eq!(
        conversation.input_observation(first.envelope_id).unwrap(),
        InputObservation::Admitted
    );
    let request = RequestId("structured-request".into());
    let item = Item(
        json!({"type":"function_call","name":"echo","call_id":"structured-call","arguments":"{\"answer\":42}"}),
    );
    store
        .write_request(&request, None, "/root", &[item], Default::default())
        .unwrap();
    store
        .record_event(
            Some(&request),
            "tool_surface",
            &json!({"version":"structured"}),
        )
        .unwrap();
    let call = CallId("structured-call".into());
    let operation = store.claim(&call, &request).unwrap();
    let jobs = JobScheduler::new(1).unwrap();
    jobs.start_operation(
        conversation.provider().request_snapshot().unwrap().unwrap(),
        operation.clone(),
        AgentPath("/root".into()),
        Some(request),
        "echo".into(),
        json!({"answer":42}),
    )
    .await
    .unwrap();
    assert_eq!(
        jobs.wait(&operation).await.unwrap(),
        JobOutput::Completed(Ok(json!({"answer":42})))
    );
    assert_eq!(
        jobs.progress(&operation).await.unwrap(),
        vec![json!({"step":"admitted"})]
    );
}

#[tokio::test]
async fn checkpoint_attachment_uses_host_admission_and_commits_binding_atomically() {
    let store = Arc::new(Store::memory().unwrap());
    let root = host(store.clone(), Arc::new(Mutex::new(vec![])));
    let root_conversation = Conversation::attach(store.clone(), root.clone(), None).unwrap();
    struct Finished;
    #[async_trait]
    impl ResponsesTransport for Finished {
        async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            Ok(ResponsesTurn {
                response_id: "source".into(),
                items: vec![Item(
                    json!({"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"scaffold ready"}]}),
                )],
                usage: Default::default(),
            })
        }
    }
    let engine = root_conversation
        .engine::<Offline, _>(
            Finished,
            Arc::new(JobScheduler::new(1).unwrap()),
            EngineConfig {
                instructions: "test".into(),
                tools: vec![],
                model: "offline".into(),
                effort: Effort::Medium,
                session_id: "source".into(),
                agent: root.identity.actor.clone(),
            },
            std::num::NonZeroU64::new(200_000).unwrap(),
        )
        .unwrap();
    let (_cancel, rx) = tokio::sync::watch::channel(false);
    let (_wake, incoming) = tokio::sync::mpsc::unbounded_channel();
    let request = engine
        .run(None, vec![], rx, incoming)
        .await
        .unwrap()
        .head_request;
    let call = CallId("source-call".into());
    store.append_items(&request,&[Item(json!({"type":"custom_tool_call","name":"haskell","call_id":call.0,"input":"checkpoint"}))]).unwrap();
    store.claim(&call, &request).unwrap();
    let checkpoint = store
        .capture_checkpoint(
            &root.identity.actor,
            &request,
            &call,
            &json!({"source":"immutable"}),
            Arc::new("private scaffold"),
        )
        .unwrap();
    let make_child = |alive| {
        Arc::new(Host {
            identity: HostIdentity {
                run: "run".into(),
                actor: AgentPath("/root/child".into()),
                incarnation: "child-one".into(),
            },
            surface: RwLock::new(root.tool_surface().unwrap()),
            alive: AtomicBool::new(alive),
            wakes: AtomicUsize::new(0),
            wake_tx: Mutex::new(None),
            store: store.clone(),
        })
    };
    assert!(
        Conversation::from_checkpoint(
            store.clone(),
            make_child(false),
            &root.identity.actor,
            &checkpoint,
            &json!({}),
            &json!({"revision":"abc"})
        )
        .is_err()
    );
    assert!(
        store
            .agent(&AgentPath("/root/child".into()))
            .unwrap()
            .is_none()
    );
    let child = Conversation::from_checkpoint(
        store.clone(),
        make_child(true),
        &root.identity.actor,
        &checkpoint,
        &json!({}),
        &json!({"revision":"abc"}),
    )
    .unwrap();
    assert_eq!(child.identity().incarnation, "child-one");
    assert_eq!(root_conversation.identity(), &root.identity);
    assert_eq!(
        store
            .agent(&child.identity().actor)
            .unwrap()
            .unwrap()
            .fork_source["checkout"]["revision"],
        "abc"
    );
}
