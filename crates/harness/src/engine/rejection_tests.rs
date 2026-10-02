use super::*;
use serde_json::Value;
use std::{collections::VecDeque, sync::Mutex};

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
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
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
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
    store.lock().execute_batch("CREATE TRIGGER reject_failure BEFORE INSERT ON events WHEN NEW.kind='request_failed' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
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
        cancel_job_on_cleanup: false,
        scheduling: ToolScheduling::Async,
        queued: false,
        completion: PendingCompletion::BlocksCompletion,
    };
    // A damaged claim owner cannot certify that cleanup settled.
    store.lock().execute_batch("DROP TABLE claims").unwrap();
    let error = engine
        .reject_or_cleanup(TransportError::Authentication, None, &request, &[pending])
        .await;
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
            cancel_job_on_cleanup: true,
        };
        let error = engine
            .reject_or_cleanup(transport_error, None, &request, &[pending])
            .await;
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
        cancel_job_on_cleanup: false,
        scheduling: ToolScheduling::Async,
        queued: false,
        completion: PendingCompletion::BlocksCompletion,
    };
    store.lock().execute_batch("DROP TABLE claims").unwrap();
    let error = engine
        .reject_or_cleanup(
            TransportError::IncompleteResponse(
                crate::transport::StreamInterruption::MissingCompletion,
            ),
            None,
            &request,
            &[call],
        )
        .await;
    assert!(matches!(error, EngineError::Cleanup { .. }));
    assert!(
        !store
            .events(Some(&request))
            .unwrap()
            .iter()
            .any(|e| e.kind == "model_interrupted")
    );
}
