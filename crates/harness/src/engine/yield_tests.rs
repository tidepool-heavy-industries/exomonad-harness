use super::*;
use crate::{embedding::HostIdentity, provider::ProviderError, turn::JobOutput};
use std::{collections::VecDeque, sync::Mutex, time::Duration};

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("offline fixture")
    }
}
struct StrictHost {
    collision: bool,
    release: Arc<tokio::sync::Notify>,
}
#[async_trait::async_trait]
impl Provider for StrictHost {
    fn validate_call(&self, name: &str, _: ToolKind) -> Result<(), ProviderError> {
        if name == "work" {
            Ok(())
        } else {
            Err(ProviderError::Tool("not in immutable host spec".into()))
        }
    }
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
    fn all_tools(&self) -> Vec<serde_json::Value> {
        if self.collision {
            vec![yield_tool_schema()]
        } else {
            vec![]
        }
    }
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.release.notified().await;
        Ok(json!({"done":true}))
    }
}
struct Script {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    turns: Mutex<VecDeque<ResponsesTurn>>,
}
#[async_trait::async_trait]
impl ResponsesTransport for Script {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.requests.lock().unwrap().push(request);
        Ok(self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected model request"))
    }
}
fn turn(items: Vec<Item>) -> ResponsesTurn {
    ResponsesTurn {
        response_id: "offline".into(),
        items,
        usage: Usage::default(),
    }
}
fn final_turn() -> ResponsesTurn {
    turn(vec![Item(
        json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}),
    )])
}
fn call(id: &str, name: &str, arguments: serde_json::Value) -> Item {
    Item(json!({"type":"function_call","call_id":id,"name":name,"arguments":arguments.to_string()}))
}
fn engine(
    turns: Vec<ResponsesTurn>,
    collision: bool,
) -> (
    Engine<Offline, StrictHost, Script>,
    Arc<Mutex<Vec<ResponsesRequest>>>,
    Arc<tokio::sync::Notify>,
) {
    let store = Arc::new(Store::memory().unwrap());
    let identity = HostIdentity {
        run: "yield-run".into(),
        actor: AgentPath("/root".into()),
        incarnation: "one".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    let requests = Arc::new(Mutex::new(vec![]));
    let release = Arc::new(tokio::sync::Notify::new());
    let engine = Engine::with_transport(
        Script {
            requests: requests.clone(),
            turns: Mutex::new(turns.into()),
        },
        store,
        Arc::new(JobScheduler::new(2).unwrap()),
        Arc::new(StrictHost {
            collision,
            release: release.clone(),
        }),
        EngineConfig {
            instructions: "yield fixture".into(),
            tools: vec![],
            model: "offline".into(),
            effort: Effort::Low,
            session_id: "yield".into(),
            agent: identity.actor.clone(),
        },
    )
    .with_origin(ConversationIdentity::Embedded {
        run: identity.run,
        actor: identity.actor,
        incarnation: identity.incarnation,
    });
    (engine, requests, release)
}
fn status(completion: &EngineCompletion) -> serde_json::Value {
    let output = completion
        .transcript
        .iter()
        .find(|item| item.0["type"] == "function_call_output" && item.0["call_id"] == "yield-call")
        .unwrap();
    serde_json::from_str(output.0["output"].as_str().unwrap()).unwrap()
}
#[tokio::test]
async fn yield_actual_embedded_manifest_accepts_call_and_timeout_zero() {
    let (engine, requests, _) = engine(
        vec![
            turn(vec![call("yield-call", "yield", json!({"until":0}))]),
            final_turn(),
        ],
        false,
    );
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>();
    let result = engine
        .run_embedded(None, vec![], cancel, mail)
        .await
        .unwrap();
    assert_eq!(
        status(&result),
        json!({"reason":"timeout","ready_results":[]})
    );
    let requests = requests.lock().unwrap();
    let tools = &requests[0].tools;
    assert_eq!(
        tools.iter().filter(|tool| tool["name"] == "yield").count(),
        1
    );
    tools.strict_tools().unwrap();
    assert_eq!(
        tools.iter().find(|tool| tool["name"] == "yield").unwrap()["parameters"]["required"],
        json!(["until"])
    );
}
#[tokio::test]
async fn yield_reserved_name_collision_refused_before_transport() {
    let (engine, requests, _) = engine(vec![], true);
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>();
    assert!(matches!(
        engine.run_embedded(None, vec![], cancel, mail).await,
        Err(EngineError::ReservedYieldTool)
    ));
    assert!(requests.lock().unwrap().is_empty());
}
#[test]
fn yield_until_normalization_and_invalid_values() {
    let before = tokio::time::Instant::now();
    let deadline = yield_deadline(&ToolInput::Function(json!({"until":0.25})))
        .unwrap()
        .unwrap();
    assert!(deadline.duration_since(before) >= Duration::from_millis(250));
    assert!(deadline.duration_since(before) < Duration::from_secs(1));
    for value in [json!({}), json!({"until":null})] {
        assert!(
            yield_deadline(&ToolInput::Function(value))
                .unwrap()
                .is_none()
        );
    }
    for value in [
        json!([]),
        json!({"until":-1}),
        json!({"until":"1"}),
        json!({"until":true}),
        json!({"until":1e100}),
        json!({"extra":1}),
    ] {
        assert!(matches!(
            yield_deadline(&ToolInput::Function(value)),
            Err(EngineError::InvalidYieldArguments)
        ));
    }
}
async fn start(
    engine: &Engine<Offline, StrictHost, Script>,
    origin: ConversationIdentity,
    request: &str,
) -> OperationId {
    let operation = OperationId {
        origin: origin.clone(),
        request: RequestId(request.into()),
        call: CallId("same-call".into()),
    };
    engine
        .scheduler
        .start_operation(
            engine.provider.clone(),
            operation.clone(),
            origin.actor().clone(),
            Some(operation.request.clone()),
            "work".into(),
            json!({}),
        )
        .await
        .unwrap();
    operation
}
#[tokio::test]
async fn yield_timeout_leaves_owned_job_alive_then_result_wakes_indefinite_wait() {
    let (engine, _, release) = engine(vec![], false);
    let operation = start(&engine, engine.origin.clone(), "pending").await;
    let (_cancel, mut cancel) = watch::channel(false);
    let (_mail, mut mail) = tokio::sync::mpsc::unbounded_channel::<MailboxSignal>();
    let result = wait_agent_and_drain_until_exact(
        &mut mail,
        &engine.scheduler,
        &mut cancel,
        std::slice::from_ref(&operation),
        Some(tokio::time::Instant::now() + Duration::from_millis(15)),
    )
    .await
    .unwrap();
    assert_eq!(result.resumed_by, WaitResumeExact::TimedOut);
    assert_eq!(engine.scheduler.output(&operation).await.unwrap(), None);
    release.notify_one();
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        wait_agent_and_drain_until_exact(
            &mut mail,
            &engine.scheduler,
            &mut cancel,
            std::slice::from_ref(&operation),
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(result.resumed_by, WaitResumeExact::Job(operation.clone()));
    assert_eq!(
        result.call_outputs,
        vec![(operation, JobOutput::Completed(Ok(json!({"done":true}))))]
    );
}
#[tokio::test]
async fn yield_exact_origin_ignores_same_call_in_other_conversation() {
    let (engine, _, release) = engine(vec![], false);
    let other = ConversationIdentity::Embedded {
        run: "other".into(),
        actor: AgentPath("/root".into()),
        incarnation: "two".into(),
    };
    let unrelated = start(&engine, other, "pending").await;
    release.notify_one();
    engine.scheduler.wait(&unrelated).await.unwrap();
    let owned = start(&engine, engine.origin.clone(), "pending").await;
    let (_cancel, mut cancel) = watch::channel(false);
    let (_mail, mut mail) = tokio::sync::mpsc::unbounded_channel::<MailboxSignal>();
    let result = wait_agent_and_drain_until_exact(
        &mut mail,
        &engine.scheduler,
        &mut cancel,
        &[owned.clone()],
        Some(tokio::time::Instant::now()),
    )
    .await
    .unwrap();
    assert_eq!(result.resumed_by, WaitResumeExact::TimedOut);
    assert!(result.call_outputs.is_empty());
    release.notify_one();
    engine.scheduler.wait(&owned).await.unwrap();
}
#[tokio::test]
async fn yield_ready_result_wins_zero_deadline() {
    let (engine, _, release) = engine(vec![], false);
    let operation = start(&engine, engine.origin.clone(), "pending").await;
    release.notify_one();
    engine.scheduler.wait(&operation).await.unwrap();
    let (_cancel, mut cancel) = watch::channel(false);
    let (_mail, mut mail) = tokio::sync::mpsc::unbounded_channel::<MailboxSignal>();
    let result = wait_agent_and_drain_until_exact(
        &mut mail,
        &engine.scheduler,
        &mut cancel,
        &[operation.clone()],
        Some(tokio::time::Instant::now()),
    )
    .await
    .unwrap();
    assert_eq!(result.resumed_by, WaitResumeExact::Job(operation));
    assert_eq!(result.call_outputs.len(), 1);
}
#[tokio::test]
async fn yield_indefinite_wakes_durable_user_and_worker_input() {
    for sender in ["operator", "worker"] {
        let (engine, _, _) = engine(
            vec![
                turn(vec![call("yield-call", "yield", json!({"until":null}))]),
                final_turn(),
            ],
            false,
        );
        let store = engine.store.clone();
        let (_cancel, cancel) = watch::channel(false);
        let (mail_tx, mail) = tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>();
        let run =
            tokio::spawn(async move { engine.run_embedded(None, vec![], cancel, mail).await });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !store
                    .claims(&CallId("yield-call".into()))
                    .unwrap()
                    .is_empty()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!run.is_finished());
        let item = Item(json!({"type":"message","role":"user","content":"arrived"}));
        let id = store
            .add_envelope(sender, "/root", "user", &item, None)
            .unwrap();
        mail_tx
            .send(DurableMailboxWake { envelope_id: id })
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            status(&result)["reason"],
            if sender == "operator" {
                "user_input"
            } else {
                "worker_input"
            }
        );
        assert_eq!(
            result
                .transcript
                .iter()
                .filter(|value| **value == item)
                .count(),
            1
        );
    }
}
#[tokio::test]
async fn yield_cancellation_wakes_without_timer() {
    let (engine, _, _) = engine(vec![], false);
    let (cancel_tx, mut cancel) = watch::channel(false);
    let (_mail, mut mail) = tokio::sync::mpsc::unbounded_channel::<MailboxSignal>();
    cancel_tx.send(true).unwrap();
    let result =
        wait_agent_and_drain_until_exact(&mut mail, &engine.scheduler, &mut cancel, &[], None)
            .await
            .unwrap();
    assert_eq!(result.resumed_by, WaitResumeExact::Cancelled);
}

#[tokio::test]
async fn yield_engine_timeout_then_result_preserves_output_order() {
    let (engine, requests, release) = engine(
        vec![
            turn(vec![
                call("work-call", "work", json!({})),
                call("yield-call", "yield", json!({"until":0})),
            ]),
            turn(vec![call("yield-result", "yield", json!({}))]),
            final_turn(),
        ],
        false,
    );
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>();
    let run = tokio::spawn(async move { engine.run_embedded(None, vec![], cancel, mail).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if requests.lock().unwrap().len() == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!run.is_finished());
    release.notify_one();
    let result = tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(status(&result)["reason"], "timeout");
    let outputs: Vec<_> = result
        .transcript
        .iter()
        .filter(|item| item.0["type"] == "function_call_output")
        .collect();
    assert_eq!(
        outputs
            .iter()
            .map(|item| item.0["call_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["yield-call", "work-call", "yield-result"]
    );
    let resumed: serde_json::Value =
        serde_json::from_str(outputs[2].0["output"].as_str().unwrap()).unwrap();
    assert_eq!(resumed["reason"], "tool_result");
    assert_eq!(resumed["ready_results"].as_array().unwrap().len(), 1);
    assert_eq!(resumed["ready_results"][0]["call"], "work-call");
    assert!(resumed["ready_results"][0].get("output").is_none());
}

#[tokio::test]
async fn yield_cancelled_claim_is_settled_before_engine_exit() {
    let (engine, _, _) = engine(
        vec![turn(vec![call("yield-call", "yield", json!({}))])],
        false,
    );
    let store = engine.store.clone();
    let (cancel_tx, cancel) = watch::channel(false);
    let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>();
    let run = tokio::spawn(async move { engine.run_embedded(None, vec![], cancel, mail).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if !store
                .claims(&CallId("yield-call".into()))
                .unwrap()
                .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // The claim is admitted before the model stream finishes. Let its wait
    // reach the event selector before requesting lifecycle cancellation.
    tokio::time::sleep(Duration::from_millis(10)).await;
    cancel_tx.send(true).unwrap();
    assert!(matches!(
        run.await.unwrap(),
        Err(EngineError::Cancelled { .. })
    ));
    let claims = store.claims(&CallId("yield-call".into())).unwrap();
    let output = store
        .get_item(claims[0].output.as_ref().unwrap())
        .unwrap()
        .unwrap();
    let status: serde_json::Value =
        serde_json::from_str(output.0["output"].as_str().unwrap()).unwrap();
    assert_eq!(status["reason"], "cancelled");
}

struct Host {
    identity: HostIdentity,
    surface: Arc<crate::embedding::ToolSurface>,
}
struct Guard;
impl crate::embedding::AdmissionGuard for Guard {}
#[async_trait::async_trait]
impl crate::embedding::HostActor for Host {
    fn identity(&self) -> &HostIdentity {
        &self.identity
    }
    fn admit(
        &self,
    ) -> Result<Box<dyn crate::embedding::AdmissionGuard>, crate::embedding::EmbeddedError> {
        Ok(Box::new(Guard))
    }
    fn tool_surface(
        &self,
    ) -> Result<Arc<crate::embedding::ToolSurface>, crate::embedding::EmbeddedError> {
        Ok(self.surface.clone())
    }
    async fn wake(&self, _: i64) -> Result<(), String> {
        Ok(())
    }
    async fn control(
        &self,
        _: crate::embedding::HostControl,
    ) -> Result<serde_json::Value, crate::embedding::HostControlError> {
        panic!("yield must not call host control")
    }
}
#[tokio::test]
async fn yield_pinned_embedded_surface_allows_intrinsic_without_host_authority() {
    let dispatcher = Arc::new(StrictHost {
        collision: false,
        release: Arc::new(tokio::sync::Notify::new()),
    });
    let surface = Arc::new(
        crate::embedding::ToolSurface::new("immutable-spec".into(), vec![], dispatcher).unwrap(),
    );
    let conversation = crate::embedding::Conversation::attach(
        Arc::new(Store::memory().unwrap()),
        Arc::new(Host {
            identity: HostIdentity {
                run: "pinned-yield".into(),
                actor: AgentPath("/root".into()),
                incarnation: "one".into(),
            },
            surface,
        }),
        None,
    )
    .unwrap();
    // PinnedProvider validates solely against the immutable host manifest.
    // The engine adds and interprets its intrinsic without granting host tools.
    let snapshot = conversation.provider().request_snapshot().unwrap().unwrap();
    assert!(snapshot.validate_call("yield", ToolKind::Function).is_err());
    let requests = Arc::new(Mutex::new(vec![]));
    let engine = conversation
        .engine::<Offline, _>(
            Script {
                requests: requests.clone(),
                turns: Mutex::new(
                    vec![
                        turn(vec![call("yield-call", "yield", json!({"until":0}))]),
                        final_turn(),
                    ]
                    .into(),
                ),
            },
            Arc::new(JobScheduler::new(1).unwrap()),
            EngineConfig {
                instructions: "native yield".into(),
                tools: vec![],
                model: "offline".into(),
                effort: Effort::Low,
                session_id: "pinned".into(),
                agent: AgentPath("/root".into()),
            },
            NonZeroU64::new(10000).unwrap(),
        )
        .unwrap();
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>();
    let result = engine
        .run_embedded(None, vec![], cancel, mail)
        .await
        .unwrap();
    assert_eq!(status(&result)["reason"], "timeout");
    assert_eq!(
        requests.lock().unwrap()[0]
            .tools
            .iter()
            .filter(|tool| tool["name"] == "yield")
            .count(),
        1
    );
}
