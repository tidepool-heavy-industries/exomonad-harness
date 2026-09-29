//! Demo-specific Engine construction over the shared tree lifecycle.
use async_trait::async_trait;
pub use harness::tree_driver::{
    Driver, EngineFactory, final_answer, initial_for_agent, published_final_answer, scan_inbox,
};
use harness::{
    engine::{Engine, EngineCompletion, EngineError, ResponsesTransport},
    item::Item,
    mailbox::Envelope,
    model::{AgentPath, Effort, RequestId},
    provider::Provider,
    store::Store,
    transport::Auth,
    turn::JobScheduler,
};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
#[cfg(test)]
#[path = "driver/recovery_tests.rs"]
mod recovery_tests;

/// Generic bridge for production and replay engine construction.
pub struct HarnessEngineFactory<A, P, C, F> {
    pub auth: Arc<A>,
    pub store: Arc<Store>,
    pub scheduler: Arc<JobScheduler>,
    pub provider: Arc<P>,
    pub compact_at_input_tokens: Option<u64>,
    pub config: F,
    pub transport: std::marker::PhantomData<C>,
}

#[async_trait]
impl<A, P, C, F> EngineFactory for HarnessEngineFactory<A, P, C, F>
where
    A: Auth + Clone + Send + Sync + 'static,
    P: Provider + 'static,
    C: ResponsesTransport + 'static,
    F: Fn(&AgentPath) -> Result<C, String> + Send + Sync + 'static,
{
    type Engine = (Engine<A, P, C>, Option<Value>);

    async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
        let transport = (self.config)(&agent)?;
        let engine = Engine::with_transport(
            transport,
            self.store.clone(),
            self.scheduler.clone(),
            self.provider.clone(),
            harness::engine::EngineConfig {
                instructions: "You are a helpful assistant. Complete the assigned task, and use wait_agent when coordinating with children.".into(),
                tools: Vec::new(),
                model: "gpt-6-sol".into(),
                effort: Effort::Low,
                session_id: format!("harness-tree-{}", agent.0),
                agent: agent.clone(),
            },
        );
        let reply_schema = self
            .store
            .agent(&agent)
            .map_err(|error| error.to_string())?
            .and_then(|record| record.contract.get("reply").cloned())
            .filter(|schema| !schema.is_null());
        let engine = match self.compact_at_input_tokens {
            Some(threshold) => engine.with_compaction_threshold(threshold),
            None => engine,
        };
        Ok((engine, reply_schema))
    }

    async fn run(
        &self,
        engine: &Self::Engine,
        head: Option<RequestId>,
        initial: Vec<Item>,
        cancel: watch::Receiver<bool>,
        inbox: mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<EngineCompletion, String> {
        let (engine, reply_schema) = engine;
        match reply_schema {
            Some(schema) => {
                engine
                    .run_with_reply_schema(head, initial, cancel, inbox, schema.clone())
                    .await
            }
            None => engine.run(head, initial, cancel, inbox).await,
        }
        .map_err(|e: EngineError| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness::{
        agent_runtime::StoreAgentToolService,
        agents::{AgentToolService, Contract},
        engine::ResponsesTransport,
        lifecycle::PublishedAnswer,
        mailbox::EnvelopeType,
        store::Usage as StoredUsage,
        transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, Usage as TurnUsage},
    };
    use serde_json::Value;
    use serde_json::json;
    use std::{
        collections::{HashMap, HashSet, VecDeque},
        sync::{
            Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    use tokio::sync::oneshot;

    #[derive(Clone)]
    struct ReplayAuth;
    impl Auth for ReplayAuth {
        fn access(&self) -> Result<(String, String), TransportError> {
            Err(TransportError::Authentication)
        }
    }

    #[derive(Clone)]
    struct ReplayTransport {
        agent: String,
        responses: Arc<StdMutex<HashMap<String, VecDeque<Vec<Item>>>>>,
        request_counts: Arc<StdMutex<HashMap<String, usize>>>,
        inputs: Arc<StdMutex<HashMap<String, Vec<Vec<Item>>>>>,
        compact_once: bool,
    }

    #[async_trait]
    impl ResponsesTransport for ReplayTransport {
        async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            let key = self.agent.clone();
            assert!(request.session_id.ends_with(&key));
            self.inputs
                .lock()
                .unwrap()
                .entry(key.clone())
                .or_default()
                .push(request.input.clone());
            let items = self
                .responses
                .lock()
                .unwrap()
                .get_mut(&key)
                .and_then(VecDeque::pop_front)
                .ok_or_else(|| TransportError::Stream(format!("no replay response for {key}")))?;
            let mut counts = self.request_counts.lock().unwrap();
            let count = counts.entry(key.clone()).or_default();
            *count += 1;
            // The compacting replay exercises one usage trigger. A pending
            // tool can settle before or after the first final response, so
            // later requests must not spuriously trigger another compaction.
            let input_tokens = if self.compact_once && *count > 1 {
                0
            } else {
                request.input.len() as u64
            };
            Ok(ResponsesTurn {
                response_id: format!("replay-{key}"),
                items,
                usage: TurnUsage {
                    input_tokens,
                    ..TurnUsage::default()
                },
            })
        }
    }

    fn function_call(id: &str, name: &str, args: Value) -> Item {
        Item(json!({
            "type":"function_call","call_id":id,"name":name,
            "arguments":serde_json::to_string(&args).unwrap()
        }))
    }

    fn final_answer(text: &str) -> Item {
        Item(json!({
            "type":"message","role":"assistant","phase":"final_answer",
            "content":[{"type":"output_text","text":text}]
        }))
    }

    fn reply_contract(marker: &str) -> Contract {
        Contract {
            clauses: vec![marker.into()],
            acceptance: vec!["structured answer".into()],
            owned: vec![],
            must_not: vec![],
            introduces: vec![],
            consumes: vec![],
            boundaries: vec![],
            reply: Some(json!({
                "type":"object",
                "properties":{"answer":{"type":"string"}},
                "required":["answer"],
                "additionalProperties":false
            })),
        }
    }

    fn wire_reply_contract(marker: &str) -> Value {
        let mut contract = serde_json::to_value(reply_contract(marker)).unwrap();
        contract["reply"] = Value::String(contract["reply"].to_string());
        contract
    }

    fn strict_answer(id: &str, answer: &str) -> Item {
        function_call(id, "finalize", json!({"result":{"answer":answer}}))
    }

    struct FirstRequestBarrier {
        arrival: mpsc::UnboundedSender<Vec<Item>>,
        release: StdMutex<Option<oneshot::Receiver<()>>>,
    }

    #[derive(Clone)]
    struct BarrierReplayTransport {
        replay: ReplayTransport,
        barrier: Option<Arc<FirstRequestBarrier>>,
    }

    #[async_trait]
    impl ResponsesTransport for BarrierReplayTransport {
        async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            if let Some(barrier) = &self.barrier {
                let release = barrier.release.lock().unwrap().take();
                if let Some(release) = release {
                    barrier.arrival.send(request.input.clone()).unwrap();
                    release
                        .await
                        .map_err(|_| TransportError::Stream("barrier dropped".into()))?;
                }
            }
            self.replay.create(request).await
        }
    }

    #[tokio::test]
    async fn followup_lifecycle_held_final_is_unseen_then_delivered_once() {
        let path = std::env::temp_dir().join(format!(
            "harness-followup-lifecycle-{}.sqlite",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Arc::new(Store::open(&path).unwrap());
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
        let responses = Arc::new(StdMutex::new(HashMap::from([
            (
                root.0.clone(),
                VecDeque::from([
                    vec![function_call(
                        "spawn-child",
                        "spawn_agent",
                        json!({
                            "task_name":"child",
                            "from":{"kind":"prompt","name":null},
                            "task":wire_reply_contract("initial")
                        }),
                    )],
                    vec![function_call("wait-1", "wait_agent", json!({}))],
                    vec![function_call("wait-2", "wait_agent", json!({}))],
                    vec![final_answer("root done")],
                ]),
            ),
            (
                child.0.clone(),
                VecDeque::from([
                    vec![strict_answer("final-1", "first")],
                    vec![strict_answer("final-2", "second")],
                ]),
            ),
        ])));
        let counts = Arc::new(StdMutex::new(HashMap::new()));
        let inputs = Arc::new(StdMutex::new(HashMap::new()));
        let (arrival_tx, mut arrival_rx) = mpsc::unbounded_channel();
        let (release_tx, release_rx) = oneshot::channel();
        let barrier = Arc::new(FirstRequestBarrier {
            arrival: arrival_tx,
            release: StdMutex::new(Some(release_rx)),
        });
        let factory = Arc::new(HarnessEngineFactory {
            auth: Arc::new(ReplayAuth),
            store: store.clone(),
            scheduler: Arc::new(JobScheduler::new(2).unwrap()),
            provider: Arc::new(crate::tree::TreeProvider::new(
                crate::CliProvider(crate::DemoProvider::development(".", false)),
                service.clone(),
            )),
            compact_at_input_tokens: None,
            config: {
                let responses = responses.clone();
                let counts = counts.clone();
                let inputs = inputs.clone();
                let barrier = barrier.clone();
                move |agent: &AgentPath| {
                    Ok(BarrierReplayTransport {
                        replay: ReplayTransport {
                            agent: agent.0.clone(),
                            responses: responses.clone(),
                            request_counts: counts.clone(),
                            inputs: inputs.clone(),
                            compact_once: false,
                        },
                        barrier: (agent.0 == "/root/child").then(|| barrier.clone()),
                    })
                }
            },
            transport: std::marker::PhantomData,
        });
        let driver = Driver::new(store.clone(), service.clone(), factory);
        driver
            .start(vec![Item(
                json!({"type":"message","role":"user","content":"start"}),
            )])
            .await
            .unwrap();
        let first_input =
            tokio::time::timeout(std::time::Duration::from_secs(2), arrival_rx.recv())
                .await
                .expect("first child request reaches explicit barrier")
                .expect("barrier source remains live");
        assert!(
            first_input
                .iter()
                .any(|item| item.0.to_string().contains("initial"))
        );
        let queued = service
            .followup_task(&root, child.clone(), reply_contract("followup-2"))
            .await
            .unwrap();
        assert_eq!(queued["status"], "queued");
        let queued_id = store.unread(&child.0).unwrap()[0].id;
        assert_eq!(queued["envelope_id"], queued_id);
        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if store.inbox(&root.0).unwrap().len() == 2
                    && store.agent(&child).unwrap().unwrap().head_request.is_some()
                    && driver.active_agent_count().await == 0
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("two child answers publish and driver reaps");
        let answers = store
            .inbox(&root.0)
            .unwrap()
            .iter()
            .map(|envelope| {
                let item = store.get_item(&envelope.item_hash).unwrap().unwrap();
                PublishedAnswer::from_message_item(&item).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0].result, json!({"answer":"first"}));
        assert_eq!(answers[1].result, json!({"answer":"second"}));
        assert_eq!(answers[0].provenance.unseen_envelopes, vec![queued_id]);
        assert!(!answers[0].provenance.seen_envelopes.contains(&queued_id));
        assert!(answers[1].provenance.seen_envelopes.contains(&queued_id));
        assert!(!answers[1].provenance.unseen_envelopes.contains(&queued_id));
        {
            let input_guard = inputs.lock().unwrap();
            let child_inputs = &input_guard[&child.0];
            assert_eq!(child_inputs.len(), 2, "no duplicate concurrent child run");
            assert!(
                !first_input
                    .iter()
                    .any(|item| item.0.to_string().contains("followup-2"))
            );
            assert_eq!(
                child_inputs[1]
                    .iter()
                    .filter(|item| item.0.to_string().contains("followup-2"))
                    .count(),
                1,
                "the next child request sees the follow-up exactly once"
            );
            assert_eq!(
                store
                    .inbox(&child.0)
                    .unwrap()
                    .iter()
                    .find(|envelope| envelope.id == queued_id)
                    .unwrap()
                    .delivered_request
                    .as_ref(),
                Some(&answers[1].provenance.final_request)
            );
            let child_head = store.agent(&child).unwrap().unwrap().head_request.unwrap();
            assert_eq!(child_head, answers[1].provenance.final_request);
            assert_eq!(
                store.request(&child_head).unwrap().unwrap().parent,
                Some(answers[0].provenance.final_request.clone())
            );
        }
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.agent(&child).unwrap().unwrap().head_request,
            Some(answers[1].provenance.final_request.clone())
        );
        let reopened_answers = reopened
            .inbox(&root.0)
            .unwrap()
            .iter()
            .map(|envelope| {
                let item = reopened.get_item(&envelope.item_hash).unwrap().unwrap();
                PublishedAnswer::from_message_item(&item).unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            reopened_answers, answers,
            "stored typed answers survive reopen"
        );
        let reopened_child_inbox = reopened.inbox(&child.0).unwrap();
        assert_eq!(reopened_child_inbox.len(), 2);
        assert_eq!(
            reopened_child_inbox
                .iter()
                .find(|envelope| envelope.id == queued_id)
                .unwrap()
                .delivered_request,
            Some(reopened_answers[1].provenance.final_request.clone())
        );
        assert_eq!(
            reopened
                .request(&reopened_answers[1].provenance.final_request)
                .unwrap()
                .unwrap()
                .parent,
            Some(reopened_answers[0].provenance.final_request.clone())
        );
        driver.shutdown().await.unwrap();
        drop(reopened);
        drop(store);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }

    #[tokio::test]
    async fn followup_lifecycle_before_boundary_is_seen_and_later_arrival_runs() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({"kind":"root"}))
            .unwrap();
        store
            .admit_agent(
                &child,
                Some(&root),
                None,
                &serde_json::to_value(reply_contract("original")).unwrap(),
                &json!({"kind":"prompt"}),
            )
            .unwrap();
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
        service
            .followup_task(&root, child.clone(), reply_contract("before-boundary"))
            .await
            .unwrap();
        let before_id = store.unread(&child.0).unwrap()[0].id;
        let responses = Arc::new(StdMutex::new(HashMap::from([
            (
                root.0.clone(),
                VecDeque::from([
                    vec![function_call("wait-before", "wait_agent", json!({}))],
                    vec![function_call("wait-late", "wait_agent", json!({}))],
                    vec![final_answer("root done")],
                ]),
            ),
            (
                child.0.clone(),
                VecDeque::from([
                    vec![strict_answer("seen-final", "saw before")],
                    vec![strict_answer("late-final", "saw late")],
                ]),
            ),
        ])));
        let counts = Arc::new(StdMutex::new(HashMap::new()));
        let inputs = Arc::new(StdMutex::new(HashMap::new()));
        let factory = Arc::new(HarnessEngineFactory {
            auth: Arc::new(ReplayAuth),
            store: store.clone(),
            scheduler: Arc::new(JobScheduler::new(2).unwrap()),
            provider: Arc::new(crate::tree::TreeProvider::new(
                crate::CliProvider(crate::DemoProvider::development(".", false)),
                service.clone(),
            )),
            compact_at_input_tokens: None,
            config: {
                let responses = responses.clone();
                let counts = counts.clone();
                let inputs = inputs.clone();
                move |agent: &AgentPath| {
                    Ok(ReplayTransport {
                        agent: agent.0.clone(),
                        responses: responses.clone(),
                        request_counts: counts.clone(),
                        inputs: inputs.clone(),
                        compact_once: false,
                    })
                }
            },
            transport: std::marker::PhantomData,
        });
        let driver = Driver::new(store.clone(), service.clone(), factory);
        driver
            .start(vec![Item(
                json!({"type":"message","role":"user","content":"start"}),
            )])
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while store.inbox(&root.0).unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first answer published before late arrival");
        let first_envelope = &store.inbox(&root.0).unwrap()[0];
        let first_item = store.get_item(&first_envelope.item_hash).unwrap().unwrap();
        let first = PublishedAnswer::from_message_item(&first_item).unwrap();
        assert_eq!(first.result, json!({"answer":"saw before"}));
        assert!(first.provenance.seen_envelopes.contains(&before_id));
        assert!(first.provenance.unseen_envelopes.is_empty());
        let queued = service
            .followup_task(&root, child.clone(), reply_contract("after-snapshot"))
            .await
            .unwrap();
        assert_eq!(queued["status"], "queued");
        let late_id = store.unread(&child.0).unwrap()[0].id;
        assert_eq!(queued["envelope_id"], late_id);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while store.inbox(&root.0).unwrap().len() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("arrival after snapshot triggers second child run");
        let second_envelope = &store.inbox(&root.0).unwrap()[1];
        let second_item = store.get_item(&second_envelope.item_hash).unwrap().unwrap();
        let second = PublishedAnswer::from_message_item(&second_item).unwrap();
        assert_eq!(second.result, json!({"answer":"saw late"}));
        assert!(!first.provenance.unseen_envelopes.contains(&late_id));
        assert!(second.provenance.seen_envelopes.contains(&late_id));
        assert_eq!(
            inputs.lock().unwrap()[&child.0]
                .iter()
                .filter(|request| {
                    request
                        .iter()
                        .any(|item| item.0.to_string().contains("after-snapshot"))
                })
                .count(),
            1
        );
        assert_eq!(*driver.failure_receiver().borrow(), None);
        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn production_factory_enables_configured_server_compaction() {
        let store = Arc::new(Store::memory().unwrap());
        let responses = Arc::new(StdMutex::new(HashMap::from([(
            "/root".to_owned(),
            VecDeque::from([
                vec![function_call(
                    "sleep-compact",
                    "sleep",
                    json!({"duration_ms":1}),
                )],
                vec![Item(
                    json!({"type":"compaction","encrypted_content":"opaque"}),
                )],
                vec![final_answer("after compaction")],
                vec![final_answer("after compaction")],
            ]),
        )])));
        let request_counts = Arc::new(StdMutex::new(HashMap::new()));
        let inputs = Arc::new(StdMutex::new(HashMap::new()));
        let factory = HarnessEngineFactory {
            auth: Arc::new(ReplayAuth),
            store: store.clone(),
            scheduler: Arc::new(JobScheduler::new(1).unwrap()),
            provider: Arc::new(crate::CliProvider(crate::DemoProvider::development(
                ".", false,
            ))),
            compact_at_input_tokens: Some(2),
            config: {
                let responses = responses.clone();
                let request_counts = request_counts.clone();
                let inputs = inputs.clone();
                move |agent: &AgentPath| {
                    Ok(ReplayTransport {
                        agent: agent.0.clone(),
                        responses: responses.clone(),
                        request_counts: request_counts.clone(),
                        inputs: inputs.clone(),
                        compact_once: true,
                    })
                }
            },
            transport: std::marker::PhantomData,
        };
        let engine = factory.create(AgentPath("/root".into())).await.unwrap();
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let completion = factory
            .run(
                &engine,
                None,
                vec![Item(
                    json!({"type":"message","role":"user","content":"start"}),
                )],
                cancel_rx,
                mpsc::unbounded_channel().1,
            )
            .await
            .unwrap();
        assert_eq!(
            super::final_answer(&completion.turn.items).as_deref(),
            Some("after compaction")
        );
        let sent = inputs.lock().unwrap();
        let sent = &sent["/root"];
        assert!(
            (3..=4).contains(&sent.len()),
            "a pending tool may require one more model turn: {sent:?}"
        );
        assert_eq!(sent[1].last().unwrap().0["type"], "compaction_trigger");
        assert_eq!(sent[2][0].0["role"], "developer");
        let mut cursor = Some(completion.head_request);
        let mut compaction_edges = 0;
        while let Some(id) = cursor {
            if store
                .events(Some(&id))
                .unwrap()
                .iter()
                .any(|event| event.kind == "compaction")
            {
                compaction_edges += 1;
            }
            cursor = store.request(&id).unwrap().unwrap().parent;
        }
        assert_eq!(compaction_edges, 1);
    }

    struct JoinFailureFactory {
        started: Arc<AtomicUsize>,
        exited: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EngineFactory for JoinFailureFactory {
        type Engine = AgentPath;

        async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
            Ok(agent)
        }

        async fn run(
            &self,
            engine: &Self::Engine,
            _head: Option<RequestId>,
            _initial: Vec<Item>,
            mut cancel: watch::Receiver<bool>,
            _inbox: mpsc::UnboundedReceiver<Envelope>,
        ) -> Result<EngineCompletion, String> {
            self.started.fetch_add(1, Ordering::SeqCst);
            while !*cancel.borrow() {
                cancel.changed().await.map_err(|error| error.to_string())?;
            }
            self.exited.fetch_add(1, Ordering::SeqCst);
            if engine.0 == "/root" {
                Err("intentional engine failure".into())
            } else {
                Err("cancelled".into())
            }
        }
    }

    struct NoAnswerFactory {
        store: Arc<Store>,
        calls: Arc<StdMutex<HashMap<String, usize>>>,
    }

    #[async_trait]
    impl EngineFactory for NoAnswerFactory {
        type Engine = AgentPath;

        async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
            Ok(agent)
        }

        async fn run(
            &self,
            engine: &Self::Engine,
            head: Option<RequestId>,
            _initial: Vec<Item>,
            _cancel: watch::Receiver<bool>,
            _inbox: mpsc::UnboundedReceiver<Envelope>,
        ) -> Result<EngineCompletion, String> {
            *self
                .calls
                .lock()
                .unwrap()
                .entry(engine.0.clone())
                .or_default() += 1;
            let id = RequestId(format!("no-answer-{}", engine.0.replace('/', "_")));
            self.store
                .write_request(&id, head.as_ref(), &engine.0, &[], StoredUsage::default())
                .map_err(|error| error.to_string())?;
            Ok(EngineCompletion {
                turn: ResponsesTurn {
                    response_id: id.0.clone(),
                    items: vec![],
                    usage: TurnUsage::default(),
                },
                transcript: vec![],
                head_request: id,
            })
        }
    }

    struct FastFailureFactory(Arc<AtomicUsize>);

    #[async_trait]
    impl EngineFactory for FastFailureFactory {
        type Engine = AgentPath;

        async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
            Ok(agent)
        }

        async fn run(
            &self,
            engine: &Self::Engine,
            _head: Option<RequestId>,
            _initial: Vec<Item>,
            _cancel: watch::Receiver<bool>,
            _inbox: mpsc::UnboundedReceiver<Envelope>,
        ) -> Result<EngineCompletion, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(format!("failure at {}", engine.0))
        }
    }

    struct MalformedAnswerFactory(Arc<Store>);

    #[async_trait]
    impl EngineFactory for MalformedAnswerFactory {
        type Engine = AgentPath;

        async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
            Ok(agent)
        }

        async fn run(
            &self,
            engine: &Self::Engine,
            head: Option<RequestId>,
            _initial: Vec<Item>,
            _cancel: watch::Receiver<bool>,
            _inbox: mpsc::UnboundedReceiver<Envelope>,
        ) -> Result<EngineCompletion, String> {
            let id = RequestId(format!("malformed-final-{}", engine.0.replace('/', "_")));
            self.0
                .write_request(&id, head.as_ref(), &engine.0, &[], StoredUsage::default())
                .map_err(|error| error.to_string())?;
            Ok(EngineCompletion {
                turn: ResponsesTurn {
                    response_id: id.0.clone(),
                    items: if engine.0 != "/root" {
                        vec![function_call("bad-final", "finalize", json!({}))]
                    } else {
                        Vec::new()
                    },
                    usage: TurnUsage::default(),
                },
                transcript: vec![],
                head_request: id,
            })
        }
    }

    #[tokio::test]
    async fn malformed_publication_does_not_advance_child_head_or_publish() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({}))
            .unwrap();
        store
            .admit_agent(&child, Some(&root), None, &json!({}), &json!({}))
            .unwrap();
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root));
        let driver = Driver::new(
            store.clone(),
            service,
            Arc::new(MalformedAnswerFactory(store.clone())),
        );
        let mut failure = driver.failure_receiver();
        driver.start(Vec::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), failure.changed())
            .await
            .expect("malformed publication must surface")
            .unwrap();
        assert!(
            failure.borrow().as_deref().unwrap().contains("finalize"),
            "typed publication failure is reported"
        );
        assert_eq!(store.agent(&child).unwrap().unwrap().head_request, None);
        assert!(store.inbox("/root").unwrap().is_empty());
        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn replay_root_child_final_answer_is_durable_before_root_wake() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
        let responses = Arc::new(StdMutex::new(HashMap::from([
            (
                "/root".to_owned(),
                VecDeque::from([
                    vec![function_call(
                        "spawn-1",
                        "spawn_agent",
                        json!({
                            "task_name":"child",
                            "from":{"kind":"prompt","name":null},
                            "task":{
                                "clauses":["answer child task"],"acceptance":["child answer"],
                                "owned":[],"must_not":[],"introduces":[],"consumes":[],"boundaries":[]
                            }
                        }),
                    )],
                    vec![function_call("wait-1", "wait_agent", json!({}))],
                    vec![final_answer("root-result")],
                ]),
            ),
            (
                "/root/child".to_owned(),
                VecDeque::from([vec![final_answer("child-result")]]),
            ),
        ])));
        let request_counts = Arc::new(StdMutex::new(HashMap::new()));
        let inputs = Arc::new(StdMutex::new(HashMap::new()));
        let provider = Arc::new(crate::tree::TreeProvider::new(
            crate::CliProvider(crate::DemoProvider::development(".", false)),
            service.clone(),
        ));
        let factory = Arc::new(HarnessEngineFactory {
            auth: Arc::new(ReplayAuth),
            store: store.clone(),
            scheduler: Arc::new(JobScheduler::new(2).unwrap()),
            provider,
            compact_at_input_tokens: None,
            config: {
                let responses = responses.clone();
                let request_counts = request_counts.clone();
                let inputs = inputs.clone();
                move |agent: &AgentPath| {
                    Ok(ReplayTransport {
                        agent: agent.0.clone(),
                        responses: responses.clone(),
                        request_counts: request_counts.clone(),
                        inputs: inputs.clone(),
                        compact_once: false,
                    })
                }
            },
            transport: std::marker::PhantomData,
        });
        let driver = Driver::new(store.clone(), service, factory);
        let root_prompt = Item(json!({
            "type":"message","role":"user","content":"initial root prompt"
        }));
        driver.start(vec![root_prompt.clone()]).await.unwrap();
        assert!(
            driver.start(vec![root_prompt.clone()]).await.is_err(),
            "a second start must not replace the live supervisor"
        );
        let settled = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let root_agent = store.agent(&root).unwrap().unwrap();
                let child = store.agent(&AgentPath("/root/child".into())).unwrap();
                let Some(child) = child else {
                    tokio::task::yield_now().await;
                    continue;
                };
                if root_agent.head_request.is_some() && child.head_request.is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        if settled.is_err() {
            eprintln!(
                "active={} root={:?} child={:?}",
                driver.active_agent_count().await,
                store.agent(&root).unwrap(),
                store.agent(&AgentPath("/root/child".into())).unwrap()
            );
        }
        settled.expect("root and child settle");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if driver.active_agent_count().await == 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("successful tasks are reaped from the running map");
        let completion = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            driver.wait_root_completion(),
        )
        .await
        .expect("root completion should be signaled")
        .expect("root Engine should complete successfully");
        assert_eq!(
            super::final_answer(&completion.turn.items).as_deref(),
            Some("root-result")
        );
        assert_eq!(
            driver.wait_root_completion().await.unwrap().head_request,
            completion.head_request,
            "completion remains available after task reaping"
        );
        let inbox = store.inbox("/root").unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].class, "AtBoundary");
        assert!(inbox[0].delivered_request.is_some());
        let root_head = store.agent(&root).unwrap().unwrap().head_request.unwrap();
        let child_head = store
            .agent(&AgentPath("/root/child".into()))
            .unwrap()
            .unwrap()
            .head_request
            .unwrap();
        let wait_request = inbox[0]
            .delivered_request
            .as_ref()
            .expect("final envelope was attached");
        assert_eq!(
            store.request(&root_head).unwrap().unwrap().parent.as_ref(),
            Some(wait_request),
            "final root response branches from the wait/inbox request"
        );
        assert_eq!(
            store.request(wait_request).unwrap().unwrap().branch,
            "/root",
            "wait/inbox request belongs to root"
        );
        assert_eq!(
            store.request(&child_head).unwrap().unwrap().branch,
            "/root/child",
            "child head belongs to child"
        );
        assert_eq!(
            store.inbox("/root/child").unwrap()[0]
                .delivered_request
                .as_ref(),
            Some(&child_head),
            "child task is attached to its durable head once"
        );
        let root_items = store.items(wait_request).unwrap();
        let wait_output = root_items
            .iter()
            .position(|item| {
                item.0["type"] == "function_call_output" && item.0["call_id"] == "wait-1"
            })
            .expect("wait call has a persisted output");
        let durable_final = root_items
            .iter()
            .position(|item| {
                item.0["role"] == "assistant"
                    && item.0["content"][0]["text"]
                        .as_str()
                        .is_some_and(|text| text.starts_with("Message Type: FINAL_ANSWER\n"))
            })
            .expect("child final envelope attached to root request");
        assert!(
            wait_output < durable_final,
            "wait status precedes inbox attachment"
        );
        let resumed: Value =
            serde_json::from_str(root_items[wait_output].0["output"].as_str().unwrap()).unwrap();
        assert_eq!(resumed["resumed_by"]["agent"], "/root/child");
        let item = store.get_item(&inbox[0].item_hash).unwrap().unwrap();
        let text = item.0["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Message Type: FINAL_ANSWER\n"));
        assert!(text.ends_with("child-result"));
        assert!(store.unread("/root").unwrap().is_empty());
        let child_inbox = store.inbox("/root/child").unwrap();
        assert_eq!(child_inbox.len(), 1, "one durable NEW_TASK");
        assert!(child_inbox[0].delivered_request.is_some());
        assert_eq!(
            request_counts.lock().unwrap()["/root"],
            3,
            "no spurious wait wake"
        );
        assert_eq!(request_counts.lock().unwrap()["/root/child"], 1);
        assert_eq!(*driver.failure_receiver().borrow(), None);
        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_cancels_a_root_wait_without_reporting_success_as_failure() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
        let responses = Arc::new(StdMutex::new(HashMap::from([(
            "/root".to_owned(),
            VecDeque::from([vec![function_call(
                "wait-shutdown",
                "wait_agent",
                json!({}),
            )]]),
        )])));
        let request_counts = Arc::new(StdMutex::new(HashMap::new()));
        let inputs = Arc::new(StdMutex::new(HashMap::new()));
        let provider = Arc::new(crate::tree::TreeProvider::new(
            crate::CliProvider(crate::DemoProvider::development(".", false)),
            service.clone(),
        ));
        let factory = Arc::new(HarnessEngineFactory {
            auth: Arc::new(ReplayAuth),
            store: store.clone(),
            scheduler: Arc::new(JobScheduler::new(1).unwrap()),
            provider,
            compact_at_input_tokens: None,
            config: {
                let responses = responses.clone();
                let request_counts = request_counts.clone();
                let inputs = inputs.clone();
                move |agent: &AgentPath| {
                    Ok(ReplayTransport {
                        agent: agent.0.clone(),
                        responses: responses.clone(),
                        request_counts: request_counts.clone(),
                        inputs: inputs.clone(),
                        compact_once: false,
                    })
                }
            },
            transport: std::marker::PhantomData,
        });
        let driver = Driver::new(store, service, factory);
        driver.start(Vec::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if request_counts.lock().unwrap().get("/root") == Some(&1) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("root reached wait_agent");
        driver
            .shutdown()
            .await
            .expect("expected Engine cancellation is a successful shutdown");
        assert!(driver.active_agent_count().await == 0);
        assert_eq!(*driver.failure_receiver().borrow(), None);
    }

    #[tokio::test]
    async fn shutdown_joins_remaining_tasks_after_first_task_failure() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({}))
            .unwrap();
        let child = AgentPath("/root/child".into());
        store
            .admit_agent(&child, Some(&root), None, &json!({}), &json!({}))
            .unwrap();
        let started = Arc::new(AtomicUsize::new(0));
        let exited = Arc::new(AtomicUsize::new(0));
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root));
        let driver = Driver::new(
            store.clone(),
            service,
            Arc::new(JoinFailureFactory {
                started: started.clone(),
                exited: exited.clone(),
            }),
        );
        driver.start(Vec::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while started.load(Ordering::SeqCst) != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both tasks started");
        let result = driver.shutdown().await;
        assert!(result.unwrap_err().contains("intentional engine failure"));
        assert_eq!(exited.load(Ordering::SeqCst), 2, "all engine tasks joined");
        assert!(driver.active_agent_count().await == 0);
    }

    #[tokio::test]
    async fn reaper_joins_all_finished_handles_after_first_failure() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({}))
            .unwrap();
        store
            .admit_agent(
                &AgentPath("/root/child".into()),
                Some(&root),
                None,
                &json!({}),
                &json!({}),
            )
            .unwrap();
        let failures = Arc::new(AtomicUsize::new(0));
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root));
        let driver = Driver::new(
            store,
            service,
            Arc::new(FastFailureFactory(failures.clone())),
        );
        let mut failure_rx = driver.failure_receiver();
        driver.start(Vec::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), failure_rx.changed())
            .await
            .expect("supervisor reports task failure")
            .expect("failure receiver remains open");
        assert!(
            failure_rx.borrow().as_deref().unwrap().contains("failed"),
            "error is surfaced, not swallowed"
        );
        assert_eq!(failures.load(Ordering::SeqCst), 2);
        assert!(driver.active_agent_count().await == 0);
        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn successful_no_answer_child_is_not_restarted_without_new_inbox() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({}))
            .unwrap();
        store
            .admit_agent(
                &AgentPath("/root/child".into()),
                Some(&root),
                None,
                &json!({}),
                &json!({}),
            )
            .unwrap();
        let calls = Arc::new(StdMutex::new(HashMap::new()));
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root));
        let driver = Driver::new(
            store.clone(),
            service,
            Arc::new(NoAnswerFactory {
                store: store.clone(),
                calls: calls.clone(),
            }),
        );
        driver.start(Vec::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if driver.active_agent_count().await == 0 && calls.lock().unwrap().len() == 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both no-answer runs complete and reap");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(calls.lock().unwrap()["/root/child"], 1);
        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn restart_with_existing_root_head_does_not_duplicate_prompt() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({"kind":"root"}))
            .unwrap();
        let original = Item(json!({
            "type":"message","role":"user","content":"persisted start prompt"
        }));
        let old_head = RequestId("prior-root-head".into());
        store
            .write_request(
                &old_head,
                None,
                &root.0,
                std::slice::from_ref(&original),
                StoredUsage::default(),
            )
            .unwrap();
        assert!(
            store
                .advance_agent_head(&root, None, Some(&old_head))
                .unwrap()
        );

        let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
        let calls = Arc::new(StdMutex::new(HashMap::new()));
        let driver = Driver::new(
            store.clone(),
            service,
            Arc::new(NoAnswerFactory {
                store: store.clone(),
                calls: calls.clone(),
            }),
        );
        driver.start(vec![original.clone()]).await.unwrap();
        assert!(driver.active_agent_count().await == 0);
        assert!(
            calls.lock().unwrap().is_empty(),
            "head/no-inbox agent not run"
        );
        assert_eq!(
            store.agent(&root).unwrap().unwrap().head_request,
            Some(old_head.clone()),
            "startup does not create a duplicate model turn"
        );
        let history = store.items(&old_head).unwrap();
        assert_eq!(
            history
                .iter()
                .filter(|item| {
                    item.0["role"] == "user" && item.0["content"] == "persisted start prompt"
                })
                .count(),
            1,
            "persisted original prompt remains exactly once"
        );
        assert!(
            driver.start(vec![original]).await.is_err(),
            "second start is rejected instead of replacing the supervisor"
        );
        driver.shutdown().await.unwrap();
    }

    #[test]
    fn unread_wake_hints_are_minimal_and_deduplicated() {
        let store = Store::memory().unwrap();
        store
            .admit_agent(
                &AgentPath("/root".into()),
                None,
                None,
                &json!({}),
                &json!({}),
            )
            .unwrap();
        store
            .admit_agent(
                &AgentPath("/root/child".into()),
                Some(&AgentPath("/root".into())),
                None,
                &json!({}),
                &json!({}),
            )
            .unwrap();
        store
            .add_envelope(
                "/root/child",
                "/root",
                "AtBoundary",
                &Item(json!({"type":"message","content":"payload is durable"})),
                None,
            )
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut seen = HashSet::new();
        let root = AgentPath("/root".into());
        scan_inbox(&store, &root, &tx, &mut seen, true).unwrap();
        scan_inbox(&store, &root, &tx, &mut seen, true).unwrap();
        let hint = rx.try_recv().unwrap();
        assert_eq!(hint.kind, EnvelopeType::Message);
        assert_eq!(hint.sender.0, "/root/child");
        assert_eq!(hint.recipient.0, "/root");
        assert!(hint.payload.is_empty());
        assert!(rx.try_recv().is_err());
        assert_eq!(
            store.unread("/root").unwrap().len(),
            1,
            "hint did not deliver row"
        );
    }

    #[test]
    fn existing_root_head_does_not_reappend_original_start_input() {
        let prompt = Item(json!({"type":"message","role":"user","content":"start"}));
        assert_eq!(
            initial_for_agent(
                &AgentPath("/root".into()),
                false,
                std::slice::from_ref(&prompt)
            ),
            vec![prompt.clone()]
        );
        assert!(
            initial_for_agent(
                &AgentPath("/root".into()),
                true,
                std::slice::from_ref(&prompt)
            )
            .is_empty()
        );
        assert!(
            initial_for_agent(
                &AgentPath("/root/child".into()),
                false,
                std::slice::from_ref(&prompt)
            )
            .is_empty()
        );
    }
}
