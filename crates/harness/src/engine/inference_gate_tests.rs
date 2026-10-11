//! Real parser, Store and scheduler histories; counters are external observations.
use super::*;
use crate::{
    embedding::{
        ClientOperationId, HostIdentity, HostInferenceState, InferenceBlock, RecoveryMode,
    },
    provider::{CallContext, ProviderError},
    server::{CommandReceiptOutcome, HostCommand},
    store::{OperationCleanupEvidence, TerminalOutcome},
    transport::{ResponsesProtocol, ResponsesRoute, StreamInterruption, sse::ResponseAssembly},
};
use proptest::prelude::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

#[derive(Clone)]
struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("scripted transport never authenticates")
    }
}

struct EffectCounter {
    store: Arc<Store>,
    queued: bool,
    calls: AtomicUsize,
    operations: Mutex<Vec<OperationId>>,
    committed: Notify,
}

#[async_trait::async_trait]
impl Provider for EffectCounter {
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        panic!("effects require the actual scheduler operation")
    }
    async fn call_with_context(
        &self,
        name: &str,
        _: serde_json::Value,
        context: CallContext,
    ) -> Result<serde_json::Value, ProviderError> {
        assert_eq!(name, "count_effect");
        self.operations
            .lock()
            .unwrap()
            .push(context.operation.expect("scheduler issued operation"));
        let occurrence = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(json!({"effect_occurrence":occurrence}))
    }
    async fn output_committed(&self, operation: &OperationId) -> Result<(), ProviderError> {
        let retained = self
            .store
            .replay_tool_output_operation(operation)
            .unwrap()
            .expect("commit notification follows stored output");
        assert_eq!(retained.terminal, TerminalOutcome::Success);
        self.committed.notify_one();
        Ok(())
    }
    fn tool_scheduling(&self, _: &str) -> ToolScheduling {
        if self.queued {
            ToolScheduling::BeforeNextInference
        } else {
            ToolScheduling::Async
        }
    }
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![
            json!({"type":"function","name":"count_effect","parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ]
    }
}

struct Script {
    effects: Arc<EffectCounter>,
    scheduler: Arc<JobScheduler>,
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
}

#[async_trait::async_trait]
impl ResponsesTransport for Script {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::Codex, ResponsesProtocol::Standard))
    }
    async fn create(&self, _: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        unreachable!()
    }
    async fn create_streaming_for_request(
        &self,
        id: &RequestId,
        request: ResponsesRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        let ordinal = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        if ordinal != 1 {
            return Ok(ResponsesTurn {
                response_id: "recovered".into(),
                items: vec![Item(
                    json!({"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"recovered"}]}),
                )],
                usage: Default::default(),
            });
        }
        let call = json!({"type":"function_call","call_id":"effect","name":"count_effect","arguments":"{}","async":!self.effects.queued});
        let event = ResponseAssembly::default()
            .accept(
                &json!({"type":"response.output_item.done","output_index":0,"item":call})
                    .to_string(),
            )
            .unwrap()
            .unwrap();
        sink.send(event).await.unwrap();
        if self.effects.queued {
            // Observe a real admitted scheduler job without claiming it or releasing it.
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Some(claim) = self.effects.store.claims(id).unwrap().first() {
                        if matches!(self.scheduler.output(&claim.operation).await, Ok(None)) {
                            break;
                        }
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("queued job must be admitted before EOF");
            assert_eq!(self.effects.calls.load(Ordering::SeqCst), 0);
        } else {
            tokio::time::timeout(Duration::from_secs(10), self.effects.committed.notified())
                .await
                .expect("real settled output precedes EOF");
            assert_eq!(self.effects.calls.load(Ordering::SeqCst), 1);
        }
        Err(TransportError::IncompleteResponse(
            StreamInterruption::MissingCompletion,
        ))
    }
}

type TestEngine = Engine<Offline, EffectCounter, Script>;
fn engine(
    store: Arc<Store>,
    effects: Arc<EffectCounter>,
    scheduler: Arc<JobScheduler>,
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    identity: &HostIdentity,
) -> TestEngine {
    Engine::with_transport(
        Script {
            effects: effects.clone(),
            scheduler: scheduler.clone(),
            requests,
        },
        store,
        scheduler,
        effects,
        EngineConfig {
            instructions: "scripted recovery history".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "gate-history".into(),
            agent: identity.actor.clone(),
        },
    )
    .with_origin(ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    })
}
fn mailbox() -> tokio::sync::mpsc::UnboundedReceiver<DurableMailboxWake> {
    tokio::sync::mpsc::unbounded_channel().1
}

fn command(
    store: &Store,
    identity: &HostIdentity,
    block: &InferenceBlock,
    mode: RecoveryMode,
) -> ClientOperationId {
    let operation = ClientOperationId(uuid::Uuid::new_v4());
    store
        .enqueue_embedded_command(
            operation,
            &HostCommand::Recover {
                target: identity.clone(),
                expected_block: block.clone(),
                mode,
            },
        )
        .unwrap();
    store
        .claim_embedded_command(&identity.run, operation)
        .unwrap()
        .unwrap();
    operation
}

async fn run_history(
    queued: bool,
    actions: &[u8],
    restart_grant: bool,
) -> std::result::Result<(), TestCaseError> {
    let path = std::env::temp_dir().join(format!(
        "harness-engine-gate-history-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let outcome = async {
        let mut store = Arc::new(Store::open(&path).unwrap());
        let identity = HostIdentity {
            run: "gate-history".into(),
            actor: AgentPath("/root".into()),
            incarnation: "original".into(),
        };
        store.bind_embedded_actor(&identity, None).unwrap();
        let effects = Arc::new(EffectCounter {
            store: store.clone(),
            queued,
            calls: AtomicUsize::new(0),
            operations: Mutex::new(vec![]),
            committed: Notify::new(),
        });
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        let requests = Arc::new(Mutex::new(vec![]));
        let mut runner = engine(
            store.clone(),
            effects.clone(),
            scheduler.clone(),
            requests.clone(),
            &identity,
        );
        let (_keep_cancel, cancellation) = watch::channel(false);
        let error = runner
            .run_embedded(None, vec![], cancellation.clone(), mailbox())
            .await
            .unwrap_err();
        let EngineError::InterruptedModelRound {
            head_request,
            cause,
        } = error
        else {
            return Err(TestCaseError::fail(format!(
                "first real EOF failed before recoverable block: {error:?}"
            )));
        };
        prop_assert_eq!(cause, StreamInterruption::MissingCompletion);
        let HostInferenceState::Blocked { mut block } =
            store.embedded_inference_state(&identity).unwrap()
        else {
            return Err(TestCaseError::fail("EOF did not retain blocked state"));
        };
        prop_assert_eq!(&block.request, &head_request);
        prop_assert_eq!(block.generation, 1);
        let expected_effects = usize::from(!queued);
        prop_assert_eq!(effects.calls.load(Ordering::SeqCst), expected_effects);
        prop_assert_eq!(requests.lock().unwrap().len(), 1);
        let claims = store.claims(&head_request).unwrap();
        prop_assert_eq!(claims.len(), 1);
        let original = claims[0].operation.clone();
        let expected_operations = if queued {
            vec![]
        } else {
            vec![original.clone()]
        };
        prop_assert_eq!(
            *effects.operations.lock().unwrap(),
            expected_operations.clone()
        );
        let cleanup = store.operation_cleanup_evidence(&original).unwrap();
        if queued {
            prop_assert!(
                matches!(
                    cleanup,
                    OperationCleanupEvidence::Terminal(TerminalOutcome::Cancelled)
                ),
                "admitted queued operation lacks actual scheduler cancellation"
            );
        } else {
            prop_assert!(
                matches!(
                    cleanup,
                    OperationCleanupEvidence::Terminal(TerminalOutcome::Success)
                ),
                "dispatched effect lacks actual success terminal"
            );
        }
        let retained = store
            .replay_tool_output_operation(&original)
            .unwrap()
            .expect("original terminal receipt retained");
        let mut queued_text = vec![];
        let mut last_envelope = 0;
        for (index, action) in [0_u8, 1, 2, 3, 4, 5].iter().chain(actions).enumerate() {
            match action % 6 {
                0 => {
                    let text = format!("queued-text-{index}");
                    last_envelope = store
                        .admit_embedded_input(
                            &identity,
                            &format!("input-{index}"),
                            "human",
                            &Item(json!({"type":"message","role":"user","content":text})),
                        )
                        .unwrap();
                    queued_text.push(text);
                }
                1 | 2 => {
                    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
                    sender
                        .send(DurableMailboxWake {
                            envelope_id: last_envelope,
                        })
                        .unwrap();
                    let error = if action % 6 == 1 {
                        runner
                            .run_embedded(None, vec![], cancellation.clone(), receiver)
                            .await
                    } else {
                        runner
                            .run_recovering_embedded(None, vec![], cancellation.clone(), receiver)
                            .await
                    }
                    .unwrap_err();
                    let EngineError::InferenceBlocked { block: actual } = error else {
                        return Err(TestCaseError::fail(format!(
                            "ungranted action {action} bypassed block: {error:?}"
                        )));
                    };
                    prop_assert_eq!(actual, block.clone());
                }
                3 | 4 => {
                    let mut target = identity.clone();
                    let mut stale = block.clone();
                    if action % 6 == 3 {
                        stale.generation += 1;
                    } else {
                        target.incarnation = "stale".into();
                    }
                    let operation = command(&store, &target, &stale, RecoveryMode::Exact);
                    let receipt = store
                        .authorize_embedded_recovery(&target, operation)
                        .unwrap();
                    prop_assert!(
                        matches!(receipt.outcome, CommandReceiptOutcome::Refused { .. }),
                        "stale recovery was accepted"
                    );
                    prop_assert_eq!(
                        store
                            .authorize_embedded_recovery(&target, operation)
                            .unwrap(),
                        receipt
                    );
                }
                _ => {
                    drop(runner);
                    store = Arc::new(Store::open(&path).unwrap());
                    runner = engine(
                        store.clone(),
                        effects.clone(),
                        scheduler.clone(),
                        requests.clone(),
                        &identity,
                    );
                }
            }
            prop_assert_eq!(
                requests.lock().unwrap().len(),
                1,
                "ungranted action {} made provider traffic",
                action
            );
            prop_assert_eq!(
                effects.calls.load(Ordering::SeqCst),
                expected_effects,
                "ungranted action {} replayed an effect",
                action
            );
            prop_assert_eq!(
                store.embedded_inference_state(&identity).unwrap(),
                HostInferenceState::Blocked {
                    block: block.clone()
                },
                "ungranted action changed the persistent block"
            );
            prop_assert_eq!(store.unread("/root").unwrap().len(), queued_text.len());
            prop_assert_eq!(
                store
                    .replay_tool_output_operation(&original)
                    .unwrap()
                    .unwrap(),
                retained.clone()
            );
        }
        if restart_grant {
            let old = command(&store, &identity, &block, RecoveryMode::Exact);
            let receipt = store.authorize_embedded_recovery(&identity, old).unwrap();
            prop_assert!(
                matches!(
                    receipt.outcome,
                    CommandReceiptOutcome::RecoveryAuthorized { .. }
                ),
                "valid original grant failed"
            );
            drop(runner);
            store = Arc::new(Store::open(&path).unwrap());
            prop_assert_eq!(
                store
                    .recover_embedded_inference_after_host_loss(&identity.run)
                    .unwrap(),
                1
            );
            let HostInferenceState::Blocked { block: new } =
                store.embedded_inference_state(&identity).unwrap()
            else {
                return Err(TestCaseError::fail("startup failed to revoke grant"));
            };
            prop_assert_eq!(new.generation, block.generation + 1);
            block = new;
            prop_assert_eq!(
                store.authorize_embedded_recovery(&identity, old).unwrap(),
                receipt
            );
            runner = engine(
                store.clone(),
                effects.clone(),
                scheduler.clone(),
                requests.clone(),
                &identity,
            );
            prop_assert!(
                matches!(
                    runner
                        .run_recovering_embedded(None, vec![], cancellation.clone(), mailbox())
                        .await,
                    Err(EngineError::InferenceBlocked { .. })
                ),
                "historical receipt revived a revoked grant"
            );
            prop_assert_eq!(requests.lock().unwrap().len(), 1);
        }
        let mode = if queued {
            RecoveryMode::VisibleHistory
        } else {
            RecoveryMode::Exact
        };
        let operation = command(&store, &identity, &block, mode);
        let receipt = store
            .authorize_embedded_recovery(&identity, operation)
            .unwrap();
        prop_assert!(
            matches!(
                receipt.outcome,
                CommandReceiptOutcome::RecoveryAuthorized { .. }
            ),
            "valid recovery refused: {:?}",
            receipt
        );
        let completed = runner
            .run_recovering_embedded(None, vec![], cancellation, mailbox())
            .await
            .map_err(|error| {
                TestCaseError::fail(format!("authorized continuation failed: {error:?}"))
            })?;
        prop_assert_eq!(requests.lock().unwrap().len(), 2);
        prop_assert_eq!(effects.calls.load(Ordering::SeqCst), expected_effects);
        prop_assert_eq!(
            store
                .authorize_embedded_recovery(&identity, operation)
                .unwrap(),
            receipt
        );
        prop_assert_eq!(
            store.embedded_inference_state(&identity).unwrap(),
            HostInferenceState::Idle
        );
        prop_assert_eq!(
            store
                .replay_tool_output_operation(&original)
                .unwrap()
                .unwrap(),
            retained
        );
        prop_assert_eq!(*effects.operations.lock().unwrap(), expected_operations);
        if queued {
            match scheduler.output(&original).await {
                Ok(Some(crate::turn::JobOutput::Cancelled)) => {
                    scheduler.release_operation(&original, None).await.unwrap();
                    prop_assert!(
                        matches!(
                            scheduler.wait(&original).await.unwrap(),
                            crate::turn::JobOutput::Cancelled
                        ),
                        "retired queue launched after recovery"
                    );
                }
                Err(JobError::UnknownCall) => {
                    prop_assert!(
                        matches!(
                            scheduler.release_operation(&original, None).await,
                            Err(JobError::UnknownCall)
                        ),
                        "retired queue was recreated"
                    );
                }
                _ => {
                    return Err(TestCaseError::fail(
                        "old admitted queue lacks a retained or released cancellation",
                    ));
                }
            }
            prop_assert_eq!(effects.calls.load(Ordering::SeqCst), 0);
        }
        let requests = requests.lock().unwrap();
        let input = &requests[1].input;
        prop_assert_eq!(
            input
                .iter()
                .filter(|item| item.0["type"] == "function_call" && item.0["call_id"] == "effect")
                .count(),
            1
        );
        prop_assert_eq!(
            input
                .iter()
                .filter(|item| item.0["type"] == "function_call_output"
                    && item.0["call_id"] == "effect")
                .count(),
            1
        );
        let encoded = serde_json::to_string(input).unwrap();
        for text in queued_text {
            prop_assert_eq!(
                encoded.matches(&text).count(),
                1,
                "queued input {} changed occurrence count",
                text
            );
        }
        prop_assert!(store.request(&completed.head_request).unwrap().is_some());
        Ok(())
    }
    .await;
    if outcome.is_ok() {
        std::fs::remove_file(&path).unwrap();
    } else {
        eprintln!(
            "retained engine history database {}; queued={queued}, restart={restart_grant}, actions={actions:?}",
            path.display()
        );
    }
    outcome
}

#[test]
fn settled_done_effect_eof_histories_require_explicit_recovery_without_replay() {
    use proptest::test_runner::{Config, TestRunner};
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let strategy = (any::<bool>(), proptest::collection::vec(0_u8..6, 0..24));
    let completed = std::cell::Cell::new([0_usize; 2]);
    let mut config = Config {
        cases: 32,
        source_file: Some(file!()),
        ..Config::default()
    };
    if let Ok(path) = std::env::var("HARNESS_PROPTEST_REGRESSIONS") {
        config.failure_persistence = Some(Box::new(
            proptest::test_runner::FileFailurePersistence::Direct(Box::leak(path.into_boxed_str())),
        ));
    }
    let result = TestRunner::new(config).run(&strategy, |(queued, actions)| {
        runtime.block_on(run_history(queued, &actions, false))?;
        let mut counts = completed.get();
        counts[usize::from(queued)] += 1;
        completed.set(counts);
        Ok(())
    });
    eprintln!(
        "engine recovery history completed async/queued={:?}",
        completed.get()
    );
    result.unwrap();
}

#[tokio::test]
async fn restart_invalidates_old_grant_without_effect_replay() {
    for queued in [false, true] {
        run_history(queued, &[], true).await.unwrap();
    }
}
