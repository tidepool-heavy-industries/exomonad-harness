use super::*;
use crate::provider::ProviderError;
use std::{collections::VecDeque, sync::Mutex, time::Duration};
use tokio::sync::Semaphore;

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("offline test")
    }
}
struct ControlledHost {
    started: tokio::sync::mpsc::UnboundedSender<String>,
    slow: Semaphore,
    fast: Semaphore,
}
#[async_trait::async_trait]
impl Provider for ControlledHost {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
    async fn call(
        &self,
        name: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.started.send(name.into()).unwrap();
        let gate = if name == "slow" {
            &self.slow
        } else {
            &self.fast
        };
        gate.acquire().await.unwrap().forget();
        Ok(json!({"actual_result":name}))
    }
    async fn call_custom_with_context(
        &self,
        name: &str,
        _: String,
        _: crate::provider::CallContext,
    ) -> Result<serde_json::Value, ProviderError> {
        self.call(name, json!({})).await
    }
}
struct Script {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    requested: tokio::sync::mpsc::UnboundedSender<()>,
    turns: Mutex<VecDeque<ResponsesTurn>>,
}
#[async_trait::async_trait]
impl ResponsesTransport for Script {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        self.requests.lock().unwrap().push(request);
        self.requested.send(()).unwrap();
        self.turns
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| TransportError::Stream("script exhausted".into()))
    }
}
fn turn(items: Vec<Item>) -> ResponsesTurn {
    ResponsesTurn {
        response_id: "test".into(),
        items,
        usage: Usage::default(),
    }
}
fn final_turn() -> ResponsesTurn {
    turn(vec![Item(
        json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}),
    )])
}
fn call(name: &str, custom: bool, mode: Option<bool>) -> Item {
    let mut item = json!({"type":if custom { "custom_tool_call" } else { "function_call" },"namespace":"functions","name":name,"call_id":format!("original-{name}")});
    if custom {
        item["input"] = json!("raw λ\ncell");
    } else {
        item["arguments"] = json!("{}");
    }
    if let Some(mode) = mode {
        item["async"] = json!(mode);
    }
    Item(item)
}
fn setup(
    turns: Vec<ResponsesTurn>,
) -> (
    Engine<Offline, ControlledHost, Script>,
    Arc<ControlledHost>,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    Arc<Mutex<Vec<ResponsesRequest>>>,
    tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    let (started, started_rx) = tokio::sync::mpsc::unbounded_channel();
    let host = Arc::new(ControlledHost {
        started,
        slow: Semaphore::new(0),
        fast: Semaphore::new(0),
    });
    let requests = Arc::new(Mutex::new(vec![]));
    let (requested, requested_rx) = tokio::sync::mpsc::unbounded_channel();
    let engine = Engine::with_transport(
        Script {
            requests: requests.clone(),
            requested: requested.clone(),
            turns: Mutex::new(turns.into()),
        },
        Arc::new(Store::memory().unwrap()),
        Arc::new(JobScheduler::new(2).unwrap()),
        host.clone(),
        EngineConfig {
            instructions: "test".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "test".into(),
            agent: AgentPath("/root".into()),
        },
    );
    (engine, host, started_rx, requests, requested_rx)
}
async fn next_request(requested: &mut tokio::sync::mpsc::UnboundedReceiver<()>) {
    tokio::time::timeout(Duration::from_secs(2), requested.recv())
        .await
        .unwrap();
}
fn outputs(request: &ResponsesRequest, id: &str) -> Vec<serde_json::Value> {
    request
        .input
        .iter()
        .filter(|item| {
            matches!(
                item.0["type"].as_str(),
                Some("function_call_output" | "custom_tool_call_output")
            ) && item.0["call_id"] == id
        })
        .map(|item| serde_json::from_str(item.0["output"].as_str().unwrap()).unwrap())
        .collect()
}

#[tokio::test]
async fn synchronous_and_omitted_modes_wait_for_actual_function_and_custom_results() {
    for custom in [false, true] {
        for mode in [None, Some(false)] {
            let original = call("slow", custom, mode);
            let (engine, host, mut started, requests, mut requested) =
                setup(vec![turn(vec![original.clone()]), final_turn()]);
            let (_cancel, cancel) = watch::channel(false);
            let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
            let running = tokio::spawn(async move { engine.run(None, vec![], cancel, mail).await });
            next_request(&mut requested).await;
            assert_eq!(started.recv().await.unwrap(), "slow");
            assert!(
                tokio::time::timeout(Duration::from_millis(40), requested.recv())
                    .await
                    .is_err()
            );
            assert_eq!(requests.lock().unwrap().len(), 1);
            host.slow.add_permits(1);
            let result = tokio::time::timeout(Duration::from_secs(2), running)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(result.transcript.contains(&original));
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(
                outputs(&requests[1], "original-slow"),
                vec![json!({"actual_result":"slow"})]
            );
        }
    }
}

#[tokio::test]
async fn asynchronous_call_allows_an_independent_model_request() {
    let (engine, host, mut started, requests, mut requested) = setup(vec![
        turn(vec![call("slow", false, Some(true))]),
        final_turn(),
        final_turn(),
    ]);
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
    let running = tokio::spawn(async move { engine.run(None, vec![], cancel, mail).await });
    next_request(&mut requested).await;
    assert_eq!(started.recv().await.unwrap(), "slow");
    next_request(&mut requested).await;
    assert!(outputs(&requests.lock().unwrap()[1], "original-slow").is_empty());
    host.slow.add_permits(1);
    let completion = tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        completion
            .transcript
            .iter()
            .any(|item| item.0["call_id"] == "original-slow"
                && item.0["type"] == "function_call_output"
                && serde_json::from_str::<serde_json::Value>(item.0["output"].as_str().unwrap())
                    .unwrap()
                    == json!({"actual_result":"slow"}))
    );
}

#[tokio::test]
async fn mixed_calls_start_concurrently_and_wait_only_for_synchronous_results() {
    let (engine, host, mut started, requests, mut requested) = setup(vec![
        turn(vec![
            call("slow", false, Some(true)),
            call("fast", false, Some(false)),
        ]),
        final_turn(),
        final_turn(),
    ]);
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
    let running = tokio::spawn(async move { engine.run(None, vec![], cancel, mail).await });
    next_request(&mut requested).await;
    let mut names = vec![started.recv().await.unwrap(), started.recv().await.unwrap()];
    names.sort();
    assert_eq!(names, ["fast", "slow"]);
    assert!(
        tokio::time::timeout(Duration::from_millis(40), requested.recv())
            .await
            .is_err()
    );
    host.fast.add_permits(1);
    next_request(&mut requested).await;
    let requests = requests.lock().unwrap();
    assert_eq!(
        outputs(&requests[1], "original-fast"),
        vec![json!({"actual_result":"fast"})]
    );
    assert!(outputs(&requests[1], "original-slow").is_empty());
    drop(requests);
    host.slow.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[test]
fn returned_mode_is_typed_and_invalid_modes_refuse_without_mutating_items() {
    for mode in [json!(null), json!(0), json!("true"), json!({})] {
        let mut item = call("slow", true, None);
        item.0["async"] = mode;
        let original = item.clone();
        assert_eq!(item.tool_call(), Err("invalid tool async mode"));
        assert_eq!(item, original);
    }
    assert_eq!(
        call("slow", true, None)
            .tool_call()
            .unwrap()
            .unwrap()
            .execution,
        ToolExecution::Synchronous
    );
    assert_eq!(
        call("slow", true, Some(true))
            .tool_call()
            .unwrap()
            .unwrap()
            .execution,
        ToolExecution::Asynchronous
    );
}

#[tokio::test]
async fn restored_and_inherited_calls_retain_original_mode_without_reexecution() {
    for inherited in [false, true] {
        for mode in [None, Some(false), Some(true)] {
            let (mut engine, host, mut started, requests, mut requested) =
                setup(vec![final_turn(), final_turn()]);
            let source = RequestId("original-request".into());
            let original = call("slow", true, mode);
            let decoded = original.tool_call().unwrap().unwrap();
            let parent = AgentPath("/root".into());
            engine
                .store
                .write_request(
                    &source,
                    None,
                    &parent.0,
                    &[original],
                    StoredUsage::default(),
                )
                .unwrap();
            engine.store.set_effort(&source, Effort::Low).unwrap();
            engine.store.claim(&decoded.call_id, &source).unwrap();
            let operation = engine
                .store
                .operation_for_request(&source, &decoded.call_id)
                .unwrap();
            engine
                .scheduler
                .start_operation(
                    host.clone(),
                    operation.clone(),
                    parent.clone(),
                    Some(source.clone()),
                    "slow".into(),
                    decoded.input,
                )
                .await
                .unwrap();
            engine
                .scheduler
                .claim_exact(&operation, engine.store.standalone_identity(parent.clone()))
                .await
                .unwrap();
            assert_eq!(started.recv().await.unwrap(), "slow");
            let head = if inherited {
                let child = AgentPath("/root/child".into());
                let snapshot = RequestId("child-snapshot".into());
                engine
                    .store
                    .admit_agent(
                        &parent,
                        None,
                        Some(&source),
                        &json!({}),
                        &json!({"kind":"root"}),
                    )
                    .unwrap();
                engine
                    .store
                    .admit_here_agent_with_snapshot(
                        &child,
                        &parent,
                        &snapshot,
                        &json!({}),
                        &parent.0,
                        &child.0,
                        "AtBoundary",
                        &Item(json!({"type":"message","role":"user","content":"child task"})),
                    )
                    .unwrap();
                engine.config.agent = child.clone();
                engine.origin = engine.store.standalone_identity(child);
                snapshot
            } else {
                source
            };
            let (_cancel, cancel) = watch::channel(false);
            let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
            let running = tokio::spawn(async move {
                if inherited {
                    engine.run(Some(head), vec![], cancel, mail).await
                } else {
                    engine
                        .run_recovering(Some(head), vec![], cancel, mail)
                        .await
                }
            });
            if mode == Some(true) {
                next_request(&mut requested).await;
                assert!(outputs(&requests.lock().unwrap()[0], "original-slow").is_empty());
            } else {
                assert!(
                    tokio::time::timeout(Duration::from_millis(40), requested.recv())
                        .await
                        .is_err()
                );
                assert!(requests.lock().unwrap().is_empty());
            }
            host.slow.add_permits(1);
            let completion = tokio::time::timeout(Duration::from_secs(2), running)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(completion.transcript.iter().any(|item| {
                item.0["call_id"] == "original-slow"
                    && item.0["type"] == "custom_tool_call_output"
                    && serde_json::from_str::<serde_json::Value>(item.0["output"].as_str().unwrap())
                        .unwrap()
                        == json!({"actual_result":"slow"})
            }));
            assert!(
                started.try_recv().is_err(),
                "original effect was not executed again"
            );
            if mode != Some(true) {
                assert_eq!(
                    outputs(&requests.lock().unwrap()[0], "original-slow"),
                    vec![json!({"actual_result":"slow"})]
                );
            }
        }
    }
}

#[tokio::test]
async fn cancellation_of_synchronous_barrier_uses_owned_job_cleanup() {
    let (engine, _host, mut started, requests, mut requested) = setup(vec![
        turn(vec![call("slow", true, Some(false))]),
        final_turn(),
    ]);
    let scheduler = engine.scheduler.clone();
    let store = engine.store.clone();
    let (cancel, cancellation) = watch::channel(false);
    let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
    let running = tokio::spawn(async move { engine.run(None, vec![], cancellation, mail).await });
    next_request(&mut requested).await;
    assert_eq!(started.recv().await.unwrap(), "slow");
    cancel.send(true).unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), running)
            .await
            .unwrap()
            .unwrap(),
        Err(EngineError::Cancelled { .. })
    ));
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(
        scheduler
            .output(&store.claims(&CallId("original-slow".into())).unwrap()[0].operation)
            .await
            .unwrap(),
        Some(crate::turn::JobOutput::Cancelled)
    );
}

#[tokio::test]
async fn synchronous_results_settle_before_compaction_requests() {
    let mut first = turn(vec![call("slow", false, Some(false))]);
    first.usage.input_tokens = 3;
    let (engine, host, mut started, requests, mut requested) = setup(vec![
        first,
        turn(vec![Item(
            json!({"type":"compaction","encrypted_content":"opaque"}),
        )]),
        final_turn(),
    ]);
    let engine = engine.with_compaction_threshold(3);
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
    let running = tokio::spawn(async move { engine.run(None, vec![], cancel, mail).await });
    next_request(&mut requested).await;
    assert_eq!(started.recv().await.unwrap(), "slow");
    assert!(
        tokio::time::timeout(Duration::from_millis(40), requested.recv())
            .await
            .is_err()
    );
    host.slow.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[1].input.last().unwrap().0["type"],
        "compaction_trigger"
    );
    assert_eq!(
        outputs(&requests[1], "original-slow"),
        vec![json!({"actual_result":"slow"})]
    );
}

#[tokio::test]
async fn yield_timeout_and_durable_input_do_not_bypass_synchronous_result() {
    let yield_call = Item(
        json!({"type":"function_call","name":"yield","call_id":"native-yield","arguments":"{\"until\":0}"}),
    );
    let (engine, host, mut started, requests, mut requested) = setup(vec![
        turn(vec![call("slow", true, Some(false)), yield_call]),
        final_turn(),
    ]);
    let identity = crate::embedding::HostIdentity {
        run: "tool-mode".into(),
        actor: AgentPath("/root".into()),
        incarnation: "one".into(),
    };
    engine.store.bind_embedded_actor(&identity, None).unwrap();
    let store = engine.store.clone();
    let engine = engine.with_origin(ConversationIdentity::Embedded {
        run: identity.run,
        actor: identity.actor,
        incarnation: identity.incarnation,
    });
    let (_cancel, cancel) = watch::channel(false);
    let (mail_tx, mail) = tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>();
    let running =
        tokio::spawn(async move { engine.run_embedded(None, vec![], cancel, mail).await });
    next_request(&mut requested).await;
    assert_eq!(started.recv().await.unwrap(), "slow");
    let input = Item(
        json!({"type":"message","role":"user","content":"input while synchronous result is held"}),
    );
    let envelope_id = store
        .add_envelope("operator", "/root", "user", &input, None)
        .unwrap();
    mail_tx.send(DurableMailboxWake { envelope_id }).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(40), requested.recv())
            .await
            .is_err()
    );
    assert_eq!(store.unread("/root").unwrap().len(), 1);
    host.slow.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        outputs(&requests[1], "original-slow"),
        vec![json!({"actual_result":"slow"})]
    );
    assert_eq!(
        outputs(&requests[1], "native-yield"),
        vec![json!({"reason":"timeout","ready_results":[]})]
    );
    assert_eq!(
        requests[1]
            .input
            .iter()
            .filter(|item| **item == input)
            .count(),
        1
    );
}
