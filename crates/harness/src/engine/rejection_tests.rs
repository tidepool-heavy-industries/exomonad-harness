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
        let EngineError::InterruptedModelRound {
            head_request,
            cause: actual,
        } = error
        else {
            panic!("unexpected failure: {error}");
        };
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
            json!({"type":"function","name":"work","parameters":{"type":"object","properties":{}}}),
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
    let engine = Engine::with_transport(
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
    assert_eq!(provider.0.load(std::sync::atomic::Ordering::SeqCst), 1);
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
    let engine = Engine::with_transport(
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
