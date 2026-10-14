use super::*;
use crate::transport::{ResponsesProtocol, ResponsesRoute};
use serde_json::Value;
use std::{collections::VecDeque, sync::Mutex};

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<String, TransportError> {
        panic!("scripted transport must not read credentials")
    }
}
struct NoTools;
#[async_trait::async_trait]
impl Provider for NoTools {
    async fn call(&self, _: &str, _: Value) -> Result<Value, crate::provider::ProviderError> {
        panic!("request rejection must not execute a tool")
    }
    fn tools(&self) -> Vec<Value> {
        Vec::new()
    }
}
struct Script {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    turns: Mutex<VecDeque<Result<ResponsesTurn, TransportError>>>,
}
#[async_trait::async_trait]
impl ResponsesTransport for Script {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard))
    }

    async fn create(
        &self,
        request: PreparedProviderRequest,
    ) -> Result<ResponsesTurn, TransportError> {
        let request = request.request().clone();
        self.requests.lock().unwrap().push(request);
        self.turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected automatic retry")
    }
}
fn success() -> ResponsesTurn {
    ResponsesTurn {
        response_id: "success".into(),
        items: vec![Item(
            json!({"type":"message", "role":"assistant", "phase":"final_answer",
            "content":[{"type":"output_text","text":"complete"}]}),
        )],
        usage: Usage::default(),
    }
}
fn engine(
    error: TransportError,
) -> (
    Engine<Offline, NoTools, Script>,
    Arc<Store>,
    Arc<Mutex<Vec<ResponsesRequest>>>,
) {
    let store = Arc::new(Store::memory().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::with_transport(
        Script {
            requests: requests.clone(),
            turns: Mutex::new(VecDeque::from([Err(error), Ok(success())])),
        },
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(NoTools),
        EngineConfig {
            instructions: "offline".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "rejected".into(),
            agent: AgentPath("/root".into()),
        },
    );
    (engine, store, requests)
}
fn mailbox() -> tokio::sync::mpsc::UnboundedReceiver<Envelope> {
    tokio::sync::mpsc::unbounded_channel().1
}

#[tokio::test]
async fn rejected_request_retains_exact_head_history_and_explicit_successor() {
    for error in [
        TransportError::Authentication,
        TransportError::Http {
            status: 400,
            diagnostic: None,
        },
    ] {
        let (engine, store, requests) = engine(error);
        let first = Item(json!({"type":"message","role":"user","content":"first input"}));
        let (_cancel, cancellation) = watch::channel(false);
        let error = engine
            .run(None, vec![first.clone()], cancellation.clone(), mailbox())
            .await
            .unwrap_err();
        let EngineError::RequestRejected { head_request, .. } = error else {
            panic!("unexpected failure: {error}")
        };
        assert_eq!(requests.lock().unwrap().len(), 1);
        let failures = store.events(Some(&head_request)).unwrap();
        assert_eq!(
            failures
                .iter()
                .filter(|event| event.kind == "request_failed")
                .count(),
            1
        );
        assert_eq!(
            store
                .items(&head_request)
                .unwrap()
                .iter()
                .filter(|item| *item == &first)
                .count(),
            1
        );
        let second = Item(json!({"type":"message","role":"user","content":"explicit input"}));
        let completion = engine
            .run(
                Some(head_request.clone()),
                vec![second.clone()],
                cancellation,
                mailbox(),
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .request(&completion.head_request)
                .unwrap()
                .unwrap()
                .parent,
            Some(head_request)
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for item in [first, second] {
            assert_eq!(
                requests[1]
                    .input
                    .iter()
                    .filter(|seen| **seen == item)
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn failure_persistence_error_is_fatal_and_stream_error_is_not_rejection() {
    let (engine, store, _) = engine(TransportError::Authentication);
    store.lock().unwrap().execute_batch("CREATE TRIGGER reject_failure BEFORE INSERT ON events WHEN NEW.kind='request_failed' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
    let (_cancel, cancellation) = watch::channel(false);
    let error = engine
        .run(None, Vec::new(), cancellation.clone(), mailbox())
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::Store(_)), "{error}");
    let (engine, store, _) = self::engine(TransportError::Stream("partial response".into()));
    assert!(matches!(
        engine.run(None, Vec::new(), cancellation, mailbox()).await,
        Err(EngineError::Transport(TransportError::Stream(_)))
    ));
    assert!(
        !store
            .events(None)
            .unwrap()
            .iter()
            .any(|event| event.kind == "request_failed")
    );
}

#[tokio::test]
async fn rejection_cleanup_error_remains_fatal_without_failed_event() {
    let (engine, store, _) = engine(TransportError::Authentication);
    let request = RequestId("cleanup-request".into());
    store.create_request(&request, None, "/root").unwrap();
    let pending = PendingCall {
        operation: OperationId {
            origin: ConversationIdentity::Standalone {
                store: "missing".into(),
                actor: AgentPath("/root".into()),
            },
            request: request.clone(),
            call: CallId("missing".into()),
        },
        call_id: CallId("missing".into()),
        claim_request: request.clone(),
        wait: None,
        tool_kind: ToolKind::Function,
        execution: ToolExecution::Asynchronous,
        persist_here_invocation_output: false,
        custody: PendingCustody::Observation,
        foreign: None,
        scheduling: ToolScheduling::Async,
        queued: false,
        completion: PendingCompletion::BlocksCompletion,
    };
    // A damaged claim owner cannot certify that cleanup settled.
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE claims")
        .unwrap();
    let error = engine
        .reject_or_cleanup(
            TransportError::Authentication,
            None,
            &request,
            vec![pending],
        )
        .await
        .cause;
    assert!(
        matches!(error, EngineError::Cleanup { primary, .. } if matches!(*primary, EngineError::RequestRejected { .. }))
    );
    assert!(
        !store
            .events(Some(&request))
            .unwrap()
            .iter()
            .any(|event| event.kind == "request_failed")
    );
}

struct ExternalWork(tokio::sync::Notify);
#[async_trait::async_trait]
impl Provider for ExternalWork {
    async fn call(&self, _: &str, _: Value) -> Result<Value, crate::provider::ProviderError> {
        self.0.notify_one();
        std::future::pending().await
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
    fn cancellation_owner(&self) -> Option<Arc<dyn crate::provider::CancellationOwner>> {
        Some(Arc::new(Unconfirmed))
    }
}
struct Unconfirmed;
#[async_trait::async_trait]
impl crate::provider::CancellationOwner for Unconfirmed {
    async fn cancel(
        &self,
        _: &OperationId,
        _: &crate::provider::JobHandle,
    ) -> crate::provider::CancellationAcknowledgment {
        crate::provider::CancellationAcknowledgment::Unconfirmed(
            "external work is still running".into(),
        )
    }
}
#[tokio::test]
async fn rejected_request_does_not_continue_after_unconfirmed_external_cancellation() {
    for transport_error in [
        TransportError::Authentication,
        TransportError::IncompleteResponse(crate::transport::StreamInterruption::MissingCompletion),
    ] {
        let (engine, store, _) = engine(TransportError::Authentication);
        let request = RequestId("external-cleanup".into());
        store.create_request(&request, None, "/root").unwrap();
        let operation = OperationId {
            origin: engine.origin.clone(),
            request: request.clone(),
            call: CallId("external".into()),
        };
        store.append_items(&request, &[Item(json!({"type":"function_call", "call_id":"external", "name":"external", "arguments":"{}"}))]).unwrap();
        store.claim_operation(&operation, &request).unwrap();
        let provider = Arc::new(ExternalWork(tokio::sync::Notify::new()));
        engine
            .scheduler
            .start_operation(
                provider.clone(),
                operation.clone(),
                AgentPath("/root".into()),
                Some(request.clone()),
                "external".into(),
                json!({}),
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), provider.0.notified())
            .await
            .unwrap();
        let pending = PendingCall {
            operation: operation.clone(),
            call_id: operation.call.clone(),
            claim_request: request.clone(),
            wait: None,
            tool_kind: ToolKind::Function,
            execution: ToolExecution::Asynchronous,
            persist_here_invocation_output: false,
            custody: PendingCustody::Producer,
            foreign: None,
            scheduling: ToolScheduling::Async,
            queued: false,
            completion: PendingCompletion::BlocksCompletion,
        };
        let error = engine
            .reject_or_cleanup(transport_error, None, &request, vec![pending])
            .await
            .cause;
        assert!(
            matches!(error, EngineError::Cleanup { primary, .. } if matches!(*primary, EngineError::RequestRejected { .. } | EngineError::Transport(TransportError::IncompleteResponse(_))))
        );
        assert!(
            !store
                .events(Some(&request))
                .unwrap()
                .iter()
                .any(|event| event.kind == "request_failed" || event.kind == "model_interrupted")
        );
        assert!(matches!(
            store
                .replay_tool_output_operation(&operation)
                .unwrap()
                .unwrap()
                .terminal,
            crate::store::TerminalOutcome::CancellationUnconfirmed(_)
        ));
    }
}

#[tokio::test]
async fn incomplete_response_retains_pending_frontier_and_requires_explicit_recovery() {
    use crate::transport::StreamInterruption;
    for cause in [
        StreamInterruption::MissingCompletion,
        StreamInterruption::ReadFailed,
    ] {
        let (engine, store, requests) = engine(TransportError::IncompleteResponse(cause));
        let identity = crate::embedding::HostIdentity {
            run: "interruption".into(),
            actor: AgentPath("/root".into()),
            incarnation: "1".into(),
        };
        store.bind_embedded_actor(&identity, None).unwrap();
        let engine = engine.with_origin(ConversationIdentity::Embedded {
            run: identity.run.clone(),
            actor: identity.actor.clone(),
            incarnation: identity.incarnation.clone(),
        });
        let (_stop, cancel) = watch::channel(false);
        let error = engine
            .run_embedded(
                None,
                vec![],
                cancel.clone(),
                tokio::sync::mpsc::unbounded_channel().1,
            )
            .await
            .unwrap_err();
        let EngineError::BlockedAction(recorded) = error else {
            panic!("interruption lost its Store-issued block: {error:?}");
        };
        let primary = *recorded.primary;
        let EngineError::InterruptedModelRound {
            head_request,
            cause: actual,
        } = primary
        else {
            panic!("unexpected primary: {primary}");
        };
        assert_eq!(recorded.block.request, head_request);
        assert_eq!(actual, cause);
        assert_eq!(requests.lock().unwrap().len(), 1);
        let frontier = store.embedded_round_frontier(&identity).unwrap();
        assert!(frontier.settled_head.is_none());
        assert_eq!(frontier.pending_head, Some(head_request.clone()));
        assert_eq!(
            store
                .events(Some(&head_request))
                .unwrap()
                .iter()
                .filter(|e| e.kind == "model_interrupted")
                .count(),
            1
        );
        assert!(
            !store
                .events(Some(&head_request))
                .unwrap()
                .iter()
                .any(|e| e.kind == "request_failed")
        );
        assert!(matches!(
            engine
                .run_recovering_embedded(
                    None,
                    vec![],
                    cancel.clone(),
                    tokio::sync::mpsc::unbounded_channel().1
                )
                .await,
            Err(EngineError::InferenceBlocked { .. })
        ));
        let queued = Item(json!({"type":"message","role":"user","content":"queued after EOF"}));
        store
            .admit_embedded_input(&identity, "after-eof", "human", &queued)
            .unwrap();
        assert!(matches!(
            engine
                .run_embedded(
                    None,
                    vec![],
                    cancel.clone(),
                    tokio::sync::mpsc::unbounded_channel().1
                )
                .await,
            Err(EngineError::InferenceBlocked { .. })
        ));
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert_eq!(store.unread(&identity.actor.0).unwrap().len(), 1);
        let crate::embedding::HostInferenceState::Blocked { block } =
            store.embedded_inference_state(&identity).unwrap()
        else {
            panic!("durable block");
        };
        let recovery = crate::embedding::ClientOperationId(uuid::Uuid::new_v4());
        store
            .enqueue_embedded_command(
                recovery,
                &crate::server::HostCommand::Recover {
                    target: identity.clone(),
                    expected_block: block,
                    mode: crate::embedding::RecoveryMode::Exact,
                },
            )
            .unwrap();
        store
            .claim_embedded_command(&identity.run, recovery)
            .unwrap()
            .unwrap();
        let receipt = store
            .authorize_embedded_recovery(&identity, recovery)
            .unwrap();
        assert!(matches!(
            receipt.outcome,
            crate::server::CommandReceiptOutcome::RecoveryAuthorized { .. }
        ));
        let completed = engine
            .run_recovering_embedded(
                None,
                vec![],
                cancel,
                tokio::sync::mpsc::unbounded_channel().1,
            )
            .await
            .unwrap();
        assert_eq!(requests.lock().unwrap().len(), 2);
        assert_eq!(
            store
                .authorize_embedded_recovery(&identity, recovery)
                .unwrap(),
            receipt
        );
        assert!(requests.lock().unwrap()[1].input.contains(&queued));
        assert_eq!(
            store
                .request(&completed.head_request)
                .unwrap()
                .unwrap()
                .parent,
            Some(head_request)
        );
    }
}

#[tokio::test]
async fn incomplete_response_cleanup_uncertainty_never_publishes_resumable_outcome() {
    let (engine, store, _) = engine(TransportError::IncompleteResponse(
        crate::transport::StreamInterruption::MissingCompletion,
    ));
    let request = RequestId("uncertain-stream".into());
    store.create_request(&request, None, "/root").unwrap();
    let call = PendingCall {
        operation: OperationId {
            origin: engine.origin.clone(),
            request: request.clone(),
            call: CallId("uncertain".into()),
        },
        call_id: CallId("uncertain".into()),
        claim_request: request.clone(),
        wait: None,
        tool_kind: ToolKind::Function,
        execution: ToolExecution::Asynchronous,
        persist_here_invocation_output: false,
        custody: PendingCustody::Observation,
        foreign: None,
        scheduling: ToolScheduling::Async,
        queued: false,
        completion: PendingCompletion::BlocksCompletion,
    };
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE claims")
        .unwrap();
    let error = engine
        .reject_or_cleanup(
            TransportError::IncompleteResponse(
                crate::transport::StreamInterruption::MissingCompletion,
            ),
            None,
            &request,
            vec![call],
        )
        .await
        .cause;
    assert!(matches!(error, EngineError::Cleanup { .. }));
    assert!(
        !store
            .events(Some(&request))
            .unwrap()
            .iter()
            .any(|e| e.kind == "model_interrupted")
    );
}

#[tokio::test]
async fn provider_failure_retains_primary_when_block_persistence_fails() {
    let diagnostic = crate::transport::HttpDiagnostic {
        code: Some("invalid_tools".into()),
        message: Some("unsupported tool schema".into()),
        ..Default::default()
    };
    let (engine, store, requests) = engine(TransportError::ProviderStreamFailure {
        event: crate::transport::ProviderStreamFailureEvent::Error,
        diagnostic: Some(diagnostic.clone()),
    });
    let identity = crate::embedding::HostIdentity {
        run: "provider-block-write-fault".into(),
        actor: AgentPath("/root".into()),
        incarnation: "one".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    store.lock().unwrap().execute_batch("CREATE TRIGGER reject_block BEFORE INSERT ON embedded_inference WHEN json_extract(NEW.state,'$.state')='blocked' BEGIN SELECT RAISE(ABORT,'block write refused'); END;").unwrap();
    let engine = engine.with_origin(ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    });
    let (_stop, cancel) = watch::channel(false);
    let error = engine
        .run_embedded(
            None,
            vec![],
            cancel,
            tokio::sync::mpsc::unbounded_channel().1,
        )
        .await
        .unwrap_err();
    let EngineError::InferenceBlockPersistence {
        primary,
        persistence,
    } = error
    else {
        panic!("provider primary was lost: {error}");
    };
    assert!(
        matches!(*primary, EngineError::Transport(TransportError::ProviderStreamFailure { diagnostic:Some(ref actual), .. }) if actual == &diagnostic)
    );
    assert!(persistence.to_string().contains("block write refused"));
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(
        store.embedded_inference_state(&identity).unwrap(),
        crate::embedding::HostInferenceState::Running
    );
}

#[tokio::test]
async fn provider_failure_cleanup_error_retains_sanitized_primary_without_certifying_block() {
    let diagnostic = crate::transport::HttpDiagnostic {
        code: Some("invalid_tools".into()),
        message: Some("unsupported tool schema".into()),
        ..Default::default()
    };
    let (engine, store, _) = engine(TransportError::Authentication);
    let request = RequestId("provider-cleanup-fault".into());
    store.create_request(&request, None, "/root").unwrap();
    let pending = PendingCall {
        operation: OperationId {
            origin: engine.origin.clone(),
            request: request.clone(),
            call: CallId("missing".into()),
        },
        call_id: CallId("missing".into()),
        claim_request: request.clone(),
        wait: None,
        tool_kind: ToolKind::Function,
        execution: ToolExecution::Asynchronous,
        persist_here_invocation_output: false,
        custody: PendingCustody::Observation,
        foreign: None,
        scheduling: ToolScheduling::Async,
        queued: false,
        completion: PendingCompletion::BlocksCompletion,
    };
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TABLE claims")
        .unwrap();
    let error = engine
        .reject_or_cleanup(
            TransportError::ProviderStreamFailure {
                event: crate::transport::ProviderStreamFailureEvent::Error,
                diagnostic: Some(diagnostic.clone()),
            },
            None,
            &request,
            vec![pending],
        )
        .await
        .cause;
    let EngineError::Cleanup { primary, cleanup } = error else {
        panic!("provider primary was lost: {error}");
    };
    assert!(
        matches!(*primary, EngineError::Transport(TransportError::ProviderStreamFailure { diagnostic:Some(ref actual), .. }) if actual == &diagnostic)
    );
    assert!(cleanup.to_string().contains("claims"));
    assert!(
        !store
            .events(Some(&request))
            .unwrap()
            .iter()
            .any(|event| event.kind == "request_failed")
    );
}

#[tokio::test]
async fn request_recording_refusal_preserves_action_stage_and_exact_head() {
    for interrupted in [false, true] {
        let diagnostic = crate::transport::HttpDiagnostic {
            code: Some("invalid_tools".into()),
            message: Some("unsupported tool schema".into()),
            ..Default::default()
        };
        let transport = if interrupted {
            TransportError::IncompleteResponse(
                crate::transport::StreamInterruption::MissingCompletion,
            )
        } else {
            TransportError::Http {
                status: 400,
                diagnostic: Some(diagnostic.clone()),
            }
        };
        let (engine, store, requests) = engine(transport);
        let identity = crate::embedding::HostIdentity {
            run: "recording-refusal".into(),
            actor: AgentPath("/root".into()),
            incarnation: "1".into(),
        };
        store.bind_embedded_actor(&identity, None).unwrap();
        let event = if interrupted {
            "model_interrupted"
        } else {
            "request_failed"
        };
        store.lock().unwrap().execute_batch(&format!(
            "CREATE TRIGGER refuse_recording BEFORE INSERT ON events WHEN NEW.kind='{event}' BEGIN SELECT RAISE(ABORT,'recording refused'); END;"
        )).unwrap();
        let engine = engine.with_origin(ConversationIdentity::Embedded {
            run: identity.run.clone(),
            actor: identity.actor.clone(),
            incarnation: identity.incarnation.clone(),
        });
        let (_stop, cancel) = watch::channel(false);
        let error = engine
            .run_embedded(
                None,
                Vec::new(),
                cancel,
                tokio::sync::mpsc::unbounded_channel().1,
            )
            .await
            .unwrap_err();
        let EngineError::Secondary(failure) = &error else {
            panic!("recording replaced the action cause: {error}");
        };
        assert_eq!(
            failure.stage(),
            if interrupted {
                RoundFailureStage::InterruptedRequestRecording
            } else {
                RoundFailureStage::RejectedRequestRecording
            }
        );
        assert!(matches!(
            failure.source_error(),
            EngineError::Store(StoreError::Sql(_))
        ));
        let frontier = store.embedded_round_frontier(&identity).unwrap();
        assert_eq!(failure.head_request(), frontier.pending_head.as_ref());
        assert!(frontier.pending_head.is_some());
        match failure.primary() {
            EngineError::Transport(TransportError::IncompleteResponse(cause)) if interrupted => {
                assert_eq!(
                    *cause,
                    crate::transport::StreamInterruption::MissingCompletion
                );
            }
            EngineError::RequestRejected {
                head_request,
                error:
                    TransportError::Http {
                        status,
                        diagnostic: Some(actual),
                    },
            } if !interrupted => {
                assert_eq!(*status, 400);
                assert_eq!(actual, &diagnostic);
                assert_eq!(Some(head_request), failure.head_request());
            }
            primary => panic!("unexpected primary: {primary:?}"),
        }
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert_eq!(
            store.embedded_inference_state(&identity).unwrap(),
            crate::embedding::HostInferenceState::Running
        );
    }
}

struct CompletedTools(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl Provider for CompletedTools {
    async fn call(&self, _: &str, _: Value) -> Result<Value, crate::provider::ProviderError> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(json!("finished"))
    }
    fn tools(&self) -> Vec<Value> {
        vec![
            json!({"type":"function","name":"work","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ]
    }
}

#[tokio::test]
async fn bounded_output_recording_failure_keeps_the_issued_operation_in_cleanup_owner() {
    let store = Arc::new(Store::memory().unwrap());
    store.lock().unwrap().execute_batch(
        "CREATE TRIGGER refuse_output BEFORE UPDATE OF state ON claims WHEN NEW.state='settled' BEGIN SELECT RAISE(ABORT,'output recording refused'); END;"
    ).unwrap();
    let provider = Arc::new(CompletedTools(std::sync::atomic::AtomicUsize::new(0)));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let engine: Engine<Offline, CompletedTools, Script> = Engine::with_transport(
        Script {
            requests: requests.clone(),
            turns: Mutex::new(VecDeque::from([Ok(ResponsesTurn {
                response_id: "bounded-call".into(),
                items: vec![Item(json!({"type":"function_call","call_id":"work-call","name":"work","arguments":"{}","async":true}))],
                usage: Usage::default(),
            }), Ok(success())])),
        }, store.clone(), Arc::new(JobScheduler::new(1).unwrap()), provider.clone(),
        EngineConfig {
            instructions: "test".into(), tools: provider.tools(), model: "test".into(),
            effort: Effort::Low, session_id: "bounded-output-fault".into(), agent: AgentPath("/root".into()),
        },
    ).bounded_invocation();
    let (_stop, cancel) = watch::channel(false);
    let receipt = engine
        .run_loop(
            None,
            Vec::new(),
            cancel,
            tokio::sync::mpsc::unbounded_channel().1,
            false,
            None,
            false,
        )
        .await;
    assert_eq!(
        provider.0.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "original outcome: {:?}; secondary failures: {:?}",
        receipt.result,
        receipt.secondary
    );
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert!(matches!(
        &receipt.result,
        Err(EngineError::Store(StoreError::Sql(_)))
    ));
    let assessment = receipt.assessment();
    assert_eq!(assessment.cleanup(), RoundCleanup::Unconfirmed);
    let issued = store.claims(&CallId("work-call".into())).unwrap();
    assert_eq!(issued.len(), 1);
    assert_eq!(
        assessment.outstanding_operations(),
        &[issued[0].operation.clone()]
    );
    assert_eq!(receipt.secondary.len(), 1);
    assert_eq!(
        receipt.secondary[0].stage(),
        RoundFailureStage::NativeCleanup
    );
}

#[tokio::test]
async fn policy_recording_failure_releases_unstarted_claim_without_dispatch() {
    let store = Arc::new(Store::memory().unwrap());
    store.lock().unwrap().execute_batch(
        "CREATE TRIGGER refuse_policy BEFORE INSERT ON events WHEN NEW.kind='tool_scheduling' BEGIN SELECT RAISE(ABORT,'policy recording refused'); END;"
    ).unwrap();
    let provider = Arc::new(CompletedTools(std::sync::atomic::AtomicUsize::new(0)));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let engine: Engine<Offline, CompletedTools, Script> = Engine::with_transport(
        Script {
            requests: requests.clone(),
            turns: Mutex::new(VecDeque::from([Ok(ResponsesTurn {
                response_id: "bounded-call".into(),
                items: vec![Item(json!({"type":"function_call","call_id":"work-call","name":"work","arguments":"{}","async":true}))],
                usage: Usage::default(),
            }), Ok(success())])),
        }, store.clone(), Arc::new(JobScheduler::new(1).unwrap()), provider.clone(),
        EngineConfig {
            instructions: "test".into(), tools: provider.tools(), model: "test".into(),
            effort: Effort::Low, session_id: "bounded-output-fault".into(), agent: AgentPath("/root".into()),
        },
    ).bounded_invocation();
    let (_stop, cancel) = watch::channel(false);
    let receipt = engine
        .run_loop(
            None,
            Vec::new(),
            cancel,
            tokio::sync::mpsc::unbounded_channel().1,
            false,
            None,
            false,
        )
        .await;
    assert_eq!(provider.0.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert!(matches!(
        &receipt.result,
        Err(EngineError::Store(StoreError::Sql(_)))
    ));
    let assessment = receipt.assessment();
    assert_eq!(assessment.cleanup(), RoundCleanup::Confirmed);
    let issued = store.claims(&CallId("work-call".into())).unwrap();
    assert_eq!(issued.len(), 1);
    assert!(assessment.outstanding_operations().is_empty());
    assert_eq!(issued[0].state, crate::store::ClaimState::Interrupted);
    assert!(receipt.secondary.is_empty());
}

#[tokio::test]
async fn response_settlement_failure_preserves_provider_action_and_stage() {
    let (engine, store, requests) = engine(TransportError::Authentication);
    store.lock().unwrap().execute_batch(
        "CREATE TRIGGER refuse_response_stop BEFORE UPDATE OF state ON provider_requests WHEN NEW.state='rejected' BEGIN SELECT RAISE(ABORT,'response settlement refused'); END;"
    ).unwrap();
    let (_stop, cancel) = watch::channel(false);
    let receipt = engine
        .run_loop(
            None,
            Vec::new(),
            cancel,
            tokio::sync::mpsc::unbounded_channel().1,
            false,
            None,
            false,
        )
        .await;
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert!(matches!(
        receipt.primary_error(),
        Some(EngineError::RequestRejected {
            error: TransportError::Authentication,
            ..
        })
    ));
    let flat = receipt.secondary_failures();
    let [secondary] = flat.as_slice() else {
        panic!("response settlement must have one flat stage");
    };
    assert_eq!(
        secondary.stage(),
        RoundFailureStage::ProviderResponseSettlement
    );
    assert!(matches!(
        secondary.source(),
        RoundFailureSourceRef::Engine(EngineError::Store(StoreError::Sql(_)))
    ));
    assert_eq!(
        receipt.assessment().secondary_failure_stages(),
        &[RoundFailureStage::ProviderResponseSettlement]
    );
    let error = receipt.result.as_ref().unwrap_err();
    let EngineError::Secondary(failure) = error else {
        panic!("unexpected outcome: {error:?}");
    };
    assert_eq!(
        failure.stage(),
        RoundFailureStage::ProviderResponseSettlement
    );
    assert!(matches!(
        failure.primary(),
        EngineError::RequestRejected {
            error: TransportError::Authentication,
            ..
        }
    ));
    assert!(matches!(
        failure.source_error(),
        EngineError::Store(StoreError::Sql(_))
    ));
    assert_eq!(receipt.assessment().execution(), RoundExecution::Rejected);
    assert_eq!(receipt.assessment().cleanup(), RoundCleanup::Confirmed);
}

#[tokio::test]
async fn unstarted_cleanup_requires_retained_not_started_evidence_even_when_update_succeeds() {
    for state in ["missing", "interrupted", "pending", "not_started"] {
        let (engine, store, _) = engine(TransportError::Authentication);
        let request = RequestId(format!("unstarted-{state}"));
        store.create_request(&request, None, "/root").unwrap();
        let operation = OperationId {
            origin: engine.origin.clone(),
            request: request.clone(),
            call: CallId("work".into()),
        };
        if state != "missing" {
            store.claim_operation(&operation, &request).unwrap();
        }
        if state == "interrupted" {
            store
                .interrupt_operation_claim(&operation, &request)
                .unwrap();
        }
        if state == "not_started" {
            store
                .interrupt_unstarted_operation_claim(&operation, &request)
                .unwrap();
        }
        let obligation = PendingCall {
            operation: operation.clone(),
            call_id: operation.call.clone(),
            claim_request: request,
            wait: None,
            tool_kind: ToolKind::Function,
            execution: ToolExecution::Asynchronous,
            persist_here_invocation_output: false,
            custody: PendingCustody::Unstarted,
            foreign: None,
            scheduling: ToolScheduling::Async,
            queued: false,
            completion: PendingCompletion::BlocksCompletion,
        };
        let outcome = engine.cancel_pending(&[obligation]).await;
        match state {
            "missing" | "interrupted" => assert!(matches!(outcome,
                Err(EngineError::UnconfirmedCancellation { operation: actual, .. }) if actual == operation)),
            "pending" | "not_started" => {
                outcome.unwrap();
                assert!(matches!(
                    store.operation_cleanup_evidence(&operation).unwrap(),
                    crate::store::OperationCleanupEvidence::NotStarted
                ));
            }
            _ => unreachable!(),
        }
    }
}

#[tokio::test]
async fn nested_stage_view_preserves_primary_and_independent_recovery_obligations() {
    let (engine, store, _) = engine(TransportError::Authentication);
    let identity = crate::embedding::HostIdentity {
        run: "flat-sources".into(),
        actor: AgentPath("/root".into()),
        incarnation: "original".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    let engine = engine.with_origin(ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    });
    let (_cancel, cancellation) = watch::channel(false);
    let mut receipt = engine
        .run_with_finalize_receipt(None, vec![], cancellation, mailbox(), None, false)
        .await
        .unwrap();
    let crate::embedding::HostInferenceState::Blocked { block } =
        store.embedded_inference_state(&identity).unwrap()
    else {
        panic!("real embedded rejection must issue its recovery block");
    };
    let original = receipt.primary_error().unwrap() as *const EngineError;
    let primary = Box::new(receipt.result.unwrap_err());
    let request = receipt.head.clone().expect("real rejected request");
    let tail = store
        .stop_provider_response(&request, crate::store::ProviderResponseState::Completed)
        .unwrap_err();
    let stop = store
        .stop_provider_response(&request, crate::store::ProviderResponseState::Streaming)
        .unwrap_err();
    store.lock().unwrap().execute_batch("CREATE TABLE flat_fence_parent(id INTEGER PRIMARY KEY); CREATE TABLE flat_fence_child(id INTEGER REFERENCES flat_fence_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER flat_fence_fault AFTER INSERT ON events WHEN NEW.kind='force_fence' BEGIN INSERT INTO flat_fence_child VALUES(1); END;").unwrap();
    let commit = store
        .record_event(Some(&request), "force_fence", &serde_json::Value::Null)
        .unwrap_err();
    assert!(matches!(&commit, StoreError::RecoveryRequired(_)));
    let fenced = store.ensure_ready().unwrap_err();
    assert!(matches!(&fenced, StoreError::AdmissionFenced));
    // Compose owner-issued failures to exercise the staged view. This is a
    // component composition control, not a second native cleanup execution.
    receipt.result = Err(EngineError::Cleanup {
        primary,
        cleanup: EngineCleanupError::Engine(Box::new(EngineError::CleanupFailures(vec![
            commit.into(),
            fenced.into(),
        ]))),
    }
    .with_secondary(
        RoundFailureStage::ProviderTailRecording,
        Some(request.clone()),
        tail.into(),
    )
    .with_secondary(
        RoundFailureStage::ProviderResponseSettlement,
        Some(request.clone()),
        stop.into(),
    ));
    assert!(std::ptr::eq(receipt.primary_error().unwrap(), original));
    assert!(
        matches!(receipt.primary_error(), Some(EngineError::RequestRejected { error: TransportError::Authentication, head_request }) if head_request == &request)
    );
    let flat = receipt.secondary_failures();
    assert_eq!(
        flat.iter()
            .map(|failure| failure.stage())
            .collect::<Vec<_>>(),
        vec![
            RoundFailureStage::NativeCleanup,
            RoundFailureStage::ProviderTailRecording,
            RoundFailureStage::ProviderResponseSettlement,
        ]
    );
    assert!(
        matches!(flat[0].source(), RoundFailureSourceRef::Cleanup(EngineCleanupError::Engine(error)) if matches!(error.as_ref(), EngineError::CleanupFailures(failures) if failures.len() == 2))
    );
    let assessment = receipt.assessment();
    assert_eq!(assessment.execution(), RoundExecution::Rejected);
    assert_eq!(
        assessment.store_continuation(),
        StoreContinuation::ReopenRequired
    );
    assert_eq!(assessment.cleanup(), RoundCleanup::Unconfirmed);
    assert_eq!(
        assessment.inference_obligations(),
        &[InferenceObligation::RecoverBlock { block }]
    );
    assert_eq!(
        assessment.secondary_failure_stages(),
        flat.iter()
            .map(|failure| failure.stage())
            .collect::<Vec<_>>()
    );
    let owned = receipt.into_failure();
    assert!(std::ptr::eq(owned.primary_error().unwrap(), original));
    assert_eq!(owned.secondary_failures().len(), 3);
    assert_eq!(
        owned.assessment().store_continuation(),
        StoreContinuation::ReopenRequired
    );
    assert_eq!(owned.assessment().cleanup(), RoundCleanup::Unconfirmed);
}

fn driver_fixture(
    turns: Vec<Result<ResponsesTurn, TransportError>>,
) -> (
    Engine<Offline, NoTools, Script>,
    Arc<Store>,
    crate::embedding::HostIdentity,
    Arc<Mutex<Vec<ResponsesRequest>>>,
) {
    let (mut engine, store, requests) = engine(TransportError::Authentication);
    engine.client.turns = Mutex::new(VecDeque::from(turns));
    let identity = crate::embedding::HostIdentity {
        run: "round-driver".into(),
        actor: AgentPath("/root".into()),
        incarnation: "original".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    let engine = engine.with_origin(ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    });
    store
        .admit_embedded_input(
            &identity,
            "first",
            "human",
            &Item(json!({"type":"message","role":"user","content":"first input"})),
        )
        .unwrap();
    (engine, store, identity, requests)
}
fn selected<A: Auth, P: Provider + 'static, C: ResponsesTransport>(
    driver: &EmbeddedRounds<A, P, C>,
) -> EmbeddedRoundSelection {
    let EmbeddedRoundReadiness::Selected(selection) = driver.next_round().unwrap() else {
        panic!("fixture must select actual input");
    };
    selection
}
fn driver_mailbox() -> tokio::sync::mpsc::UnboundedReceiver<DurableMailboxWake> {
    tokio::sync::mpsc::unbounded_channel().1
}

#[tokio::test]
async fn driver_foreign_refusal_consumes_epoch_before_any_provider_request() {
    let (engine, store, identity, requests) = driver_fixture(vec![Ok(success())]);
    let other = Engine::with_transport(
        Script {
            requests: requests.clone(),
            turns: Mutex::new(VecDeque::from([Ok(success())])),
        },
        store.clone(),
        engine.scheduler.clone(),
        engine.provider.clone(),
        engine.config.clone(),
    )
    .with_origin(engine.origin.clone());
    let first = engine.into_embedded_rounds().unwrap();
    let mut second = other.into_embedded_rounds().unwrap();
    let foreign = selected(&first);
    let stale = selected(&second);
    let (_cancel, cancellation) = watch::channel(false);
    for selection in [foreign, stale] {
        let error = second
            .run_round(selection, cancellation.clone(), driver_mailbox())
            .await
            .unwrap_err();
        assert_eq!(error.assessment().cleanup(), RoundCleanup::Unconfirmed);
        assert!(matches!(
            error.primary_error(),
            Some(EngineError::Store(StoreError::InvalidEmbeddedFrontier))
        ));
    }
    assert!(requests.lock().unwrap().is_empty());
    assert_eq!(
        store.embedded_inference_state(&identity).unwrap(),
        crate::embedding::HostInferenceState::Idle
    );
}

#[tokio::test]
async fn driver_revalidates_selected_frontier_in_existing_begin_transaction() {
    let (engine, store, identity, requests) = driver_fixture(vec![Ok(success())]);
    let mut driver = engine.into_embedded_rounds().unwrap();
    let old = selected(&driver);
    store
        .write_embedded_request(
            &identity,
            &RequestId("intervening".into()),
            None,
            &[],
            crate::store::Usage::default(),
        )
        .unwrap();
    let (_cancel, cancellation) = watch::channel(false);
    let error = driver
        .run_round(old, cancellation, driver_mailbox())
        .await
        .unwrap_err();
    assert!(matches!(
        error.primary_error(),
        Some(EngineError::Store(StoreError::StaleInferenceRecovery))
    ));
    assert!(requests.lock().unwrap().is_empty());
    assert_eq!(
        store.embedded_inference_state(&identity).unwrap(),
        crate::embedding::HostInferenceState::Idle
    );
}

#[tokio::test]
async fn driver_success_and_precancel_have_actual_head_commits() {
    for cancelled in [false, true] {
        let (engine, store, identity, requests) = driver_fixture(vec![Ok(success())]);
        let mut driver = engine.into_embedded_rounds().unwrap();
        let selection = selected(&driver);
        let (_cancel, cancellation) = watch::channel(cancelled);
        let report = driver
            .run_round(selection, cancellation, driver_mailbox())
            .await
            .unwrap();
        let assessment = report.assessment();
        let expected = if cancelled {
            RoundExecution::Cancelled
        } else {
            RoundExecution::Completed
        };
        assert_eq!(assessment.execution(), expected);
        let [commit] = assessment.head_settlements() else {
            panic!("round must return actual head commit");
        };
        assert_eq!(Some(commit.request()), assessment.head());
        assert!(store.matches_embedded_settlement_issuer(&identity, commit));
        let frontier = store.embedded_round_frontier(&identity).unwrap();
        assert_eq!(frontier.settled_head.as_ref(), assessment.head());
        assert!(frontier.pending_head.is_none());
        assert_eq!(requests.lock().unwrap().len(), usize::from(!cancelled));
        assert!(matches!(
            driver.next_round().unwrap(),
            EmbeddedRoundReadiness::Waiting(EmbeddedRoundWait::NoPendingWork)
        ));
    }
}

#[tokio::test]
async fn driver_rejection_keeps_issued_head_and_block_despite_queued_input() {
    let (engine, store, identity, requests) =
        driver_fixture(vec![Err(TransportError::Authentication)]);
    let mut driver = engine.into_embedded_rounds().unwrap();
    let selection = selected(&driver);
    let (_cancel, cancellation) = watch::channel(false);
    let report = driver
        .run_round(selection, cancellation, driver_mailbox())
        .await
        .unwrap();
    let assessment = report.assessment();
    assert_eq!(assessment.execution(), RoundExecution::Rejected);
    assert!(matches!(
        report.issue(),
        Some(EngineError::RequestRejected {
            error: TransportError::Authentication,
            ..
        })
    ));
    let [commit] = assessment.head_settlements() else {
        panic!("rejected transaction must carry its head receipt");
    };
    assert_eq!(
        commit.outcome(),
        crate::store::EmbeddedRoundOutcome::Rejected
    );
    store
        .admit_embedded_input(
            &identity,
            "later",
            "human",
            &Item(json!({"type":"message","role":"user","content":"later input"})),
        )
        .unwrap();
    let EmbeddedRoundReadiness::Waiting(EmbeddedRoundWait::RecoveryRequired { block }) =
        driver.next_round().unwrap()
    else {
        panic!("queued input cannot replace exact recovery grant");
    };
    assert_eq!(
        assessment.inference_obligations(),
        &[InferenceObligation::RecoverBlock { block }]
    );
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn driver_head_failure_retains_completed_action_and_stage() {
    let (engine, store, identity, _) = driver_fixture(vec![Ok(success())]);
    store.lock().unwrap().execute_batch("CREATE TRIGGER refuse_driver_head BEFORE UPDATE OF head_request ON agents BEGIN SELECT RAISE(ABORT,'head refused'); END;").unwrap();
    let mut driver = engine.into_embedded_rounds().unwrap();
    let selection = selected(&driver);
    let (_cancel, cancellation) = watch::channel(false);
    let failure = driver
        .run_round(selection, cancellation, driver_mailbox())
        .await
        .unwrap_err();
    assert!(failure.primary_error().is_none());
    let completion = failure
        .original_completion()
        .expect("head refusal retains full completion");
    assert_eq!(completion.turn.response_id, "success");
    assert_eq!(failure.assessment().execution(), RoundExecution::Completed);
    assert_eq!(
        failure.assessment().secondary_failure_stages(),
        &[RoundFailureStage::DurableHeadSettlement]
    );
    assert!(matches!(
        failure.secondary_failures()[0].source(),
        RoundFailureSourceRef::Engine(EngineError::Store(StoreError::Sql(_)))
    ));
    let frontier = store.embedded_round_frontier(&identity).unwrap();
    assert!(frontier.settled_head.is_none());
    assert_eq!(
        frontier.pending_head.as_ref(),
        Some(&completion.head_request)
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_driver_head")
        .unwrap();
    let retry = selected(&driver);
    let (_cancel, cancellation) = watch::channel(false);
    let settled = driver
        .run_round(retry, cancellation, driver_mailbox())
        .await
        .unwrap();
    assert_eq!(settled.assessment().execution(), RoundExecution::Completed);
    assert_eq!(settled.assessment().head(), Some(&completion.head_request));
    // The one scripted response was already consumed; any provider replay
    // would fail the transport's strict unexpected-retry guard.
    assert!(
        store
            .embedded_round_frontier(&identity)
            .unwrap()
            .pending_head
            .is_none()
    );
}

#[tokio::test]
async fn driver_completed_head_commit_preserves_independently_issued_block() {
    let (engine, store, identity, _) = driver_fixture(vec![Ok(success())]);
    let (_cancel, cancellation) = watch::channel(false);
    let mut receipt = engine
        .run_with_finalize_receipt(None, vec![], cancellation, mailbox(), None, false)
        .await
        .unwrap();
    let request = receipt.original_completion().unwrap().head_request.clone();
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
    let mut driver = engine.into_embedded_rounds().unwrap();
    let report = driver.finish(None, receipt).unwrap();
    let assessment = report.assessment();
    assert_eq!(assessment.execution(), RoundExecution::Completed);
    assert_eq!(
        assessment.inference_obligations(),
        &[InferenceObligation::RecoverBlock {
            block: block.clone()
        }]
    );
    assert_eq!(
        store.embedded_inference_state(&identity).unwrap(),
        crate::embedding::HostInferenceState::Blocked {
            block: block.clone()
        }
    );
    assert_eq!(
        store
            .embedded_round_frontier(&identity)
            .unwrap()
            .settled_head,
        Some(request)
    );
    let EmbeddedRoundReadiness::Waiting(EmbeddedRoundWait::RecoveryRequired { block: retained }) =
        driver.next_round().unwrap()
    else {
        panic!("head commit must not clear inference block");
    };
    assert_eq!(retained, block);
}
