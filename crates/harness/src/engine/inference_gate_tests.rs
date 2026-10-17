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
use rusqlite::OptionalExtension;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone)]
struct Offline;
impl Auth for Offline {
    fn admission(
        &self,
    ) -> std::result::Result<crate::transport::NativeTransportAdmission, crate::transport::TransportError> {
        crate::transport::NativeTransportAdmission::chatgpt_plan(std::path::PathBuf::from(
            "/offline/synthetic-credential",
        ))
    }
    fn access(&self) -> Result<String, TransportError> {
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
            json!({"type":"function","name":"count_effect","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ]
    }
}

struct ObservedRequest {
    id: RequestId,
    body: serde_json::Value,
}

struct Script {
    effects: Arc<EffectCounter>,
    scheduler: Arc<JobScheduler>,
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
}

#[async_trait::async_trait]
impl ResponsesTransport for Script {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard))
    }
    async fn create(&self, _: PreparedProviderRequest) -> Result<ResponsesTurn, TransportError> {
        unreachable!()
    }
    async fn create_streaming(
        &self,
        request: PreparedProviderRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        let id = request.request_id().clone();
        assert_eq!(
            request.response_contract(),
            (ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard)
        );
        let body = serde_json::from_slice(request.wire_body()).unwrap();
        let ordinal = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(ObservedRequest {
                id: id.clone(),
                body,
            });
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
                    if let Some(claim) = self.effects.store.claims_on(&id).unwrap().first() {
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
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
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
        let receipt = runner
            .run_with_finalize_receipt(None, vec![], cancellation.clone(), mailbox(), None, false)
            .await
            .unwrap();
        let assessment = receipt.assessment();
        let error = receipt.into_result().unwrap_err();
        let EngineError::BlockedAction(recorded) = error else {
            return Err(TestCaseError::fail(format!(
                "EOF did not carry the Store-issued block: {error:?}"
            )));
        };
        prop_assert_eq!(
            assessment.inference_obligations(),
            &[InferenceObligation::RecoverBlock {
                block: recorded.block.clone()
            }]
        );
        let primary = *recorded.primary;
        let EngineError::InterruptedModelRound {
            head_request,
            cause,
        } = primary
        else {
            return Err(TestCaseError::fail(format!(
                "first real EOF lost its original action: {primary:?}"
            )));
        };
        prop_assert_eq!(cause, StreamInterruption::MissingCompletion);
        let HostInferenceState::Blocked { mut block } =
            store.embedded_inference_state(&identity).unwrap()
        else {
            return Err(TestCaseError::fail("EOF did not retain blocked state"));
        };
        prop_assert_eq!(&block, &recorded.block);
        prop_assert_eq!(&block.request, &head_request);
        prop_assert_eq!(block.generation, 1);
        let expected_effects = usize::from(!queued);
        prop_assert_eq!(effects.calls.load(Ordering::SeqCst), expected_effects);
        prop_assert_eq!(requests.lock().unwrap().len(), 1);
        let claims = store.claims_on(&head_request).unwrap();
        prop_assert_eq!(claims.len(), 1);
        let original = claims[0].operation.clone();
        let expected_operations = if queued {
            vec![]
        } else {
            vec![original.clone()]
        };
        prop_assert_eq!(
            effects.operations.lock().unwrap().clone(),
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
        let last_envelope = 0;
        for (index, action) in [0_u8, 1, 2, 3, 4, 5].iter().chain(actions).enumerate() {
            match action % 6 {
                0 => {
                    // Queuing a command is observation/work scheduling. Only
                    // the claimed Store admission below can grant continuation.
                    let operation = ClientOperationId(uuid::Uuid::new_v4());
                    store
                        .enqueue_embedded_command(
                            operation,
                            &HostCommand::Input {
                                target: identity.clone(),
                                text: format!("not-admitted-{index}"),
                            },
                        )
                        .unwrap();
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
                        matches!(
                            receipt.outcome,
                            CommandReceiptOutcome::ContinuationRefused { .. }
                        ),
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
        let input_operation = ClientOperationId(uuid::Uuid::new_v4());
        let text = "actually admitted Send";
        queued_text.push(text.to_owned());
        store
            .enqueue_embedded_command(
                input_operation,
                &HostCommand::Input {
                    target: identity.clone(),
                    text: text.into(),
                },
            )
            .unwrap();
        store
            .claim_embedded_command(&identity.run, input_operation)
            .unwrap()
            .unwrap();
        let admitted = match store
            .admit_embedded_command_input(&identity, input_operation, text)
            .unwrap()
        {
            crate::store::CommandInputAdmission::New(receipt) => receipt,
            _ => return Err(TestCaseError::fail("first Send was not new")),
        };
        let duplicate = match store
            .admit_embedded_command_input(&identity, input_operation, text)
            .unwrap()
        {
            crate::store::CommandInputAdmission::Retained(receipt) => receipt,
            _ => return Err(TestCaseError::fail("duplicate Send was not retained")),
        };
        prop_assert_eq!(admitted, duplicate);
        let HostInferenceState::Blocked { block: updated } =
            store.embedded_inference_state(&identity).unwrap()
        else {
            // Settled native operation can continue exactly without replay.
            let completed = runner
                .run_recovering_embedded(None, vec![], cancellation, mailbox())
                .await
                .map_err(|error| TestCaseError::fail(format!("Send continuation: {error:?}")))?;
            prop_assert_eq!(requests.lock().unwrap().len(), 2);
            prop_assert_eq!(effects.calls.load(Ordering::SeqCst), expected_effects);
            let observed = requests.lock().unwrap();
            prop_assert_eq!(observed[1].body.to_string().matches(text).count(), 1);
            prop_assert!(store.request(&completed.head_request).unwrap().is_some());
            return Ok(());
        };
        block = updated;
        prop_assert_eq!(
            block.continuation_refusal,
            Some(crate::embedding::ContinuationRefusal::ExactProjectionUnavailable)
        );
        let operation = command(&store, &identity, &block, RecoveryMode::VisibleHistory);
        let receipt = store
            .authorize_embedded_recovery(&identity, operation)
            .unwrap();
        prop_assert!(
            matches!(
                receipt.outcome,
                CommandReceiptOutcome::RecoveryAuthorized { .. }
            ),
            "explicit visible recovery refused: {:?}",
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
        prop_assert_eq!(
            effects.operations.lock().unwrap().clone(),
            expected_operations
        );
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
        prop_assert_eq!(&requests[0].id, &original.request);
        prop_assert_eq!(&requests[1].id, &completed.head_request);
        let input = requests[1].body["input"].as_array().unwrap();
        prop_assert_eq!(
            input
                .iter()
                .filter(|item| item["type"] == "function_call" && item["call_id"] == "effect")
                .count(),
            1
        );
        prop_assert_eq!(
            input
                .iter()
                .filter(|item| item["type"] == "function_call_output"
                    && item["call_id"] == "effect")
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

async fn continuity_refusal_preserves_action(
    cancelled: bool,
    direct_block: bool,
    owned_failure: bool,
) {
    let store = Arc::new(Store::memory().unwrap());
    let identity = HostIdentity {
        run: "continuity-composition".into(),
        actor: AgentPath("/root".into()),
        incarnation: "original".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    let effects = Arc::new(EffectCounter {
        store: store.clone(),
        queued: false,
        calls: AtomicUsize::new(0),
        operations: Mutex::new(vec![]),
        committed: Notify::new(),
    });
    let scheduler = Arc::new(JobScheduler::new(1).unwrap());
    // The script's second response completes without issuing any operation.
    let requests = Arc::new(Mutex::new(vec![ObservedRequest {
        id: RequestId("prior-observation".into()),
        body: serde_json::Value::Null,
    }]));
    let runner = engine(store.clone(), effects, scheduler, requests, &identity);
    let (route, protocol) = runner.client.response_contract().unwrap();
    store
        .initialize_context_target(
            &runner.origin,
            &crate::context::ContextProjectionTarget {
                model: runner.config.model.clone(),
                route,
                protocol,
            },
        )
        .unwrap();
    let attempt = store.begin_embedded_inference(&identity, None).unwrap();
    let (_cancel, cancellation) = watch::channel(cancelled);
    let (_incoming, mut envelopes) = tokio::sync::mpsc::unbounded_channel();
    let mut receipt = runner
        .run_loop(
            None,
            vec![],
            cancellation,
            &mut envelopes,
            true,
            None,
            false,
        )
        .await;
    let expected = if cancelled {
        RoundExecution::Cancelled
    } else {
        RoundExecution::Completed
    };
    assert_eq!(receipt.assessment().execution(), expected);
    assert_eq!(receipt.assessment().cleanup(), RoundCleanup::Confirmed);
    let request = receipt
        .assessment()
        .head()
        .cloned()
        .expect("actual round head");
    if owned_failure {
        let original = receipt
            .original_completion()
            .expect("actual completed action");
        let original_turn = original.turn.response_id.clone();
        let original_transcript = original.transcript.clone();
        let refusal = store
            .stop_provider_response(&request, crate::store::ProviderResponseState::Completed)
            .unwrap_err();
        receipt.add_failure(
            RoundFailureStage::ProviderResponseSettlement,
            refusal.into(),
        );
        let failure = receipt.into_failure();
        assert!(failure.primary_error().is_none());
        let retained = failure
            .original_completion()
            .expect("owned failure retains full completion");
        assert_eq!(retained.head_request, request);
        assert_eq!(retained.turn.response_id, original_turn);
        assert_eq!(retained.transcript, original_transcript);
        assert_eq!(failure.assessment().execution(), RoundExecution::Completed);
        let flat = failure.secondary_failures();
        let [secondary] = flat.as_slice() else {
            panic!("completed failure must preserve one secondary");
        };
        assert_eq!(
            secondary.stage(),
            RoundFailureStage::ProviderResponseSettlement
        );
        assert!(matches!(
            secondary.source(),
            RoundFailureSourceRef::Engine(EngineError::Store(StoreError::InvalidProviderEvidence(
                _
            )))
        ));
        return;
    }
    if direct_block {
        let block = store
            .block_embedded_inference(
                &identity,
                &request,
                crate::embedding::InferenceBlockReason::ContinuityUnproven,
            )
            .unwrap();
        receipt.inference.push(InferenceObligation::RecoverBlock {
            block: block.clone(),
        });
        let error = receipt.into_result().unwrap_err();
        let EngineError::CompletionSettlement(settlement) = error else {
            panic!("completed action with issued block lost its completion: {error:?}");
        };
        assert_eq!(settlement.completion.head_request, request);
        assert_eq!(
            settlement.stage,
            RoundFailureStage::InferenceAttemptSettlement
        );
        assert!(
            matches!(*settlement.source, EngineError::InferenceBlocked { block: retained } if retained == block)
        );
        return;
    }
    // Obtain the refusal through the real Store evidence boundary. This control
    // composes owner-issued round evidence with a secondary projection refusal.
    let refusal = store
        .stop_provider_response(&request, crate::store::ProviderResponseState::Completed)
        .unwrap_err();
    assert!(refusal.is_inference_projection_refusal());
    receipt.add_failure(
        RoundFailureStage::ProviderResponseSettlement,
        refusal.into(),
    );
    runner.settle_inference_attempt(&attempt, &mut receipt);
    let assessment = receipt.assessment();
    assert_eq!(assessment.execution(), expected);
    let HostInferenceState::Blocked { block } = store.embedded_inference_state(&identity).unwrap()
    else {
        panic!("continuity refusal did not block inference");
    };
    assert_eq!(block.request, request);
    let mut expected_obligations = Vec::new();
    if cancelled {
        expected_obligations.push(InferenceObligation::Reconcile {
            head: Some(request.clone()),
        });
    }
    expected_obligations.push(InferenceObligation::RecoverBlock {
        block: block.clone(),
    });
    assert_eq!(assessment.inference_obligations(), expected_obligations);
    let frontier = store.embedded_round_frontier(&identity).unwrap();
    assert!(matches!(
        store.begin_embedded_inference(&identity, frontier.settled_head.as_ref()),
        Err(StoreError::InferenceBlocked { .. })
    ));
    let error = receipt.into_result().unwrap_err();
    let EngineError::BlockedAction(recorded) = error else {
        panic!("legacy rendering lost issued block: {error:?}");
    };
    assert_eq!(recorded.block.request, request);
    if cancelled {
        let EngineError::Secondary(secondary) = *recorded.primary else {
            panic!("cancellation must retain its secondary stage");
        };
        assert!(matches!(*secondary.primary, EngineError::Cancelled { .. }));
    } else {
        assert!(matches!(
            *recorded.primary,
            EngineError::CompletionSettlement(_)
        ));
    }
}

#[tokio::test]
async fn cancelled_action_with_projection_refusal_blocks_before_finish() {
    continuity_refusal_preserves_action(true, false, false).await;
}

#[tokio::test]
async fn completed_action_with_projection_refusal_blocks_without_panicking() {
    continuity_refusal_preserves_action(false, false, false).await;
}

#[tokio::test]
async fn completed_receipt_with_issued_block_preserves_both_during_projection() {
    continuity_refusal_preserves_action(false, true, false).await;
}

#[tokio::test]
async fn post_completion_failure_retains_full_action_and_flat_secondary() {
    continuity_refusal_preserves_action(false, false, true).await;
}

#[derive(Clone)]
struct EmptyInterrupted(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl ResponsesTransport for EmptyInterrupted {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard))
    }
    async fn create(&self, _: PreparedProviderRequest) -> Result<ResponsesTurn, TransportError> {
        unreachable!()
    }
    async fn create_streaming(
        &self,
        _: PreparedProviderRequest,
        _: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(TransportError::IncompleteResponse(
            StreamInterruption::MissingCompletion,
        ))
    }
}
async fn one_empty_interruption(
    store: Arc<Store>,
    identity: &HostIdentity,
    requests: Arc<AtomicUsize>,
) {
    let effects = Arc::new(EffectCounter {
        store: store.clone(),
        queued: false,
        calls: AtomicUsize::new(0),
        operations: Mutex::new(vec![]),
        committed: Notify::new(),
    });
    let runner = Engine::<Offline, _, _>::with_transport(
        EmptyInterrupted(requests),
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        effects,
        EngineConfig {
            instructions: "model history".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "model-history".into(),
            agent: identity.actor.clone(),
        },
    )
    .with_origin(ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    });
    store
        .initialize_context_target(
            &runner.origin,
            &crate::context::ContextProjectionTarget {
                model: "test".into(),
                route: ResponsesRoute::ChatGptPlan,
                protocol: ResponsesProtocol::Standard,
            },
        )
        .unwrap();
    let frontier = store.embedded_round_frontier(identity).unwrap();
    let attempt = store
        .begin_embedded_inference(identity, frontier.settled_head.as_ref())
        .unwrap();
    let (_cancel, cancellation) = watch::channel(false);
    let mut inbox = tokio::sync::mpsc::unbounded_channel().1;
    let mut receipt = runner
        .run_loop(
            frontier.settled_head,
            vec![],
            cancellation,
            &mut inbox,
            true,
            None,
            true,
        )
        .await;
    runner.settle_inference_attempt(&attempt, &mut receipt);
    assert_eq!(
        receipt.assessment().execution(),
        RoundExecution::Interrupted
    );
    assert!(receipt.secondary_failures().is_empty());
}
fn admitted_send(
    store: &Store,
    identity: &HostIdentity,
    operation: ClientOperationId,
    text: &str,
) -> crate::server::CommandReceipt {
    store
        .enqueue_embedded_command(
            operation,
            &HostCommand::Input {
                target: identity.clone(),
                text: text.into(),
            },
        )
        .unwrap();
    if let Some(receipt) = store
        .embedded_command(&identity.run, operation)
        .unwrap()
        .and_then(|r| r.receipt)
    {
        return receipt;
    }
    store
        .claim_embedded_command(&identity.run, operation)
        .unwrap()
        .unwrap();
    match store
        .admit_embedded_command_input(identity, operation, text)
        .unwrap()
    {
        crate::store::CommandInputAdmission::New(receipt)
        | crate::store::CommandInputAdmission::Retained(receipt) => receipt,
    }
}

#[test]
fn generated_continuation_histories_preserve_budget_dedup_stop_and_restart() {
    use proptest::test_runner::{Config, TestRunner};
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let strategy = proptest::collection::vec(0u8..8, 8..40);
    let completed = std::cell::Cell::new(0usize);
    let config = Config {
        cases: 32,
        source_file: Some(file!()),
        ..Config::default()
    };
    let result = TestRunner::new(config).run(&strategy, |actions| {
        runtime.block_on(async {

        let path=std::env::temp_dir().join(format!("continuation-model-{}.sqlite",uuid::Uuid::new_v4()));
        let mut store=Arc::new(Store::open(&path).unwrap());
        let identity=HostIdentity{run:"history".into(),actor:AgentPath("/root".into()),incarnation:"one".into()};
        store.bind_embedded_actor(&identity,None).unwrap();
        let requests=Arc::new(AtomicUsize::new(0));
        one_empty_interruption(store.clone(),&identity,requests.clone()).await;
        let mut used=0u8; // Independent oracle: only reservation increments this budget.
        let mut reservation=None;
        let mut sends=vec![];
        for (index,action) in [0,2,5,3,6,4,2,6,0,1,7,4].into_iter().chain(actions).enumerate() {
            match action {
                0=>{
                    if let HostInferenceState::Blocked{block}=store.embedded_inference_state(&identity).unwrap() {
                        let eligible=matches!(block.reason,crate::embedding::InferenceBlockReason::StreamInterrupted{..})&&block.continuation_refusal.is_none()&&used<2;
                        let actual=store.reserve_automatic_retry(&identity,&block).unwrap();
                        prop_assert_eq!(actual.is_some(),eligible,"budget={}; step={}",used,index);
                        if let Some(actual)=actual {used+=1;prop_assert_eq!(actual.attempt,used);reservation=Some(actual);}
                    }
                }
                1=>{if let Some(retry)=reservation.as_ref(){let _=store.activate_automatic_retry(&identity,retry).unwrap();}}
                2=>{
                    let operation=ClientOperationId(uuid::Uuid::new_v4());let text=format!("send-{index}");
                    let receipt=admitted_send(&store,&identity,operation,&text);sends.push((operation,text,receipt));
                }
                3=>{for (operation,text,receipt) in &sends {prop_assert_eq!(admitted_send(&store,&identity,*operation,text),receipt.clone());}}
                4=>{
                    store=Arc::new(Store::open(&path).unwrap());
                    store.recover_embedded_inference_after_host_loss(&identity.run).unwrap();
                    if let Some(retry)=reservation.as_ref(){prop_assert!(!store.activate_automatic_retry(&identity,retry).unwrap(),"startup revived old timer");}
                }
                5=>{
                    if let Some(retry)=reservation.as_ref(){store.cancel_automatic_retry(&identity,retry).unwrap();
                        prop_assert!(!matches!(store.embedded_inference_state(&identity).unwrap(),HostInferenceState::Recovering{block,..} if block==retry.block),"Stop retained pre-Stop Send grant");}
                }
                6=>{if matches!(store.embedded_inference_state(&identity).unwrap(),HostInferenceState::Recovering{..}|HostInferenceState::Idle){one_empty_interruption(store.clone(),&identity,requests.clone()).await;}}
                _=>{
                    if matches!(store.embedded_inference_state(&identity).unwrap(),HostInferenceState::Recovering{..}) {
                        let frontier=store.embedded_round_frontier(&identity).unwrap();
                        let attempt=store.begin_embedded_inference(&identity,frontier.settled_head.as_ref()).unwrap();
                        store.finish_embedded_inference(&attempt,crate::store::InferenceAttemptDisposition::Cancelled).unwrap();
                    }
                }
            }
            let actual:u8=store.lock().unwrap().query_row("SELECT attempts FROM embedded_retry_lineage WHERE run_id=?1 AND agent_path=?2",rusqlite::params![identity.run,identity.actor.0],|r|r.get(0)).optional().unwrap().unwrap_or(0);
            prop_assert_eq!(actual,used,"budget reset without success at step {}",index);
            let unique:usize=store.lock().unwrap().query_row("SELECT COUNT(*) FROM embedded_inputs WHERE run_id=?1",[&identity.run],|r|r.get(0)).unwrap();
            prop_assert_eq!(unique,sends.len(),"duplicate command admitted input twice");
        }
        std::fs::remove_file(path).unwrap();
        completed.set(completed.get() + 1);
        Ok(())
        })
    });
    eprintln!(
        "generated continuation histories completed: {}",
        completed.get()
    );
    result.unwrap();
}
