use super::*;
use crate::{
    embedding::{HostIdentity, HostInferenceState, InferenceBlockReason},
    provider::{CancellationAcknowledgment, CancellationOwner, JobHandle, ProviderError},
    store::{EmbeddedRoundOutcome, OperationCleanupEvidence, TerminalOutcome},
    transport::{ResponsesProtocol, ResponsesRoute, sse::ResponseAssembly},
};
use std::{sync::Mutex, time::Duration};
use tokio::sync::Notify;

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("scripted cancellation must not authenticate")
    }
}

struct PendingWork {
    started: Notify,
    unconfirmed: bool,
}
#[async_trait::async_trait]
impl Provider for PendingWork {
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.started.notify_one();
        std::future::pending().await
    }
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![
            json!({"type":"function","name":"pending_work","strict":true,
            "parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ]
    }
    fn cancellation_owner(&self) -> Option<Arc<dyn CancellationOwner>> {
        self.unconfirmed
            .then(|| Arc::new(Unconfirmed) as Arc<dyn CancellationOwner>)
    }
}
struct Unconfirmed;
#[async_trait::async_trait]
impl CancellationOwner for Unconfirmed {
    async fn cancel(&self, _: &OperationId, _: &JobHandle) -> CancellationAcknowledgment {
        CancellationAcknowledgment::Unconfirmed("external work remains unresolved".into())
    }
}

struct Script {
    requests: Mutex<Vec<RequestId>>,
    rejection: bool,
}
#[async_trait::async_trait]
impl ResponsesTransport for Script {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::Codex, ResponsesProtocol::Standard))
    }
    async fn create(&self, _: PreparedProviderRequest) -> Result<ResponsesTurn, TransportError> {
        unreachable!("Engine uses streaming")
    }
    async fn create_streaming(
        &self,
        request: PreparedProviderRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        let ordinal = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request.request_id().clone());
            requests.len()
        };
        if ordinal == 1 {
            if self.rejection {
                return Err(TransportError::Authentication);
            }
            let call = json!({"type":"function_call","call_id":"pending","name":"pending_work","arguments":"{}","async":true});
            let event = ResponseAssembly::default()
                .accept(
                    &json!({"type":"response.output_item.done","output_index":0,"item":call})
                        .to_string(),
                )
                .unwrap()
                .unwrap();
            sink.send(event).await.unwrap();
            return std::future::pending().await;
        }
        Ok(ResponsesTurn {
            response_id: "after-cancellation".into(),
            items: vec![Item(
                json!({"type":"message","role":"assistant","phase":"final_answer",
                "content":[{"type":"output_text","text":"continued"}]}),
            )],
            usage: Default::default(),
        })
    }
}

type TestEngine = Engine<Offline, PendingWork, Script>;
fn engine(unconfirmed: bool, rejection: bool) -> (Arc<TestEngine>, HostIdentity) {
    let store = Arc::new(Store::memory().unwrap());
    let identity = HostIdentity {
        run: "cancel-gate".into(),
        actor: AgentPath("/root".into()),
        incarnation: "original".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    let engine = Engine::with_transport(
        Script {
            requests: Mutex::new(vec![]),
            rejection,
        },
        store,
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(PendingWork {
            started: Notify::new(),
            unconfirmed,
        }),
        EngineConfig {
            instructions: "embedded cancellation gate".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "cancel-gate".into(),
            agent: identity.actor.clone(),
        },
    )
    .with_origin(ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    });
    (Arc::new(engine), identity)
}
fn mailbox() -> tokio::sync::mpsc::UnboundedReceiver<DurableMailboxWake> {
    tokio::sync::mpsc::unbounded_channel().1
}
async fn cancel(engine: &Arc<TestEngine>) -> EngineError {
    let (cancel, cancellation) = watch::channel(false);
    let runner = engine.clone();
    let run = tokio::spawn(async move {
        runner
            .run_embedded(None, vec![], cancellation, mailbox())
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), engine.provider.started.notified())
        .await
        .expect("actual scheduler operation starts");
    cancel.send_replace(true);
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("Engine cancellation settles")
        .unwrap()
        .unwrap_err()
}

#[tokio::test]
async fn confirmed_cancellation_releases_gate_and_later_input_continues() {
    let (engine, identity) = engine(false, false);
    let EngineError::Cancelled {
        head_request: Some(head),
    } = cancel(&engine).await
    else {
        panic!("confirmed cancellation must retain its exact request")
    };
    assert_eq!(
        engine.store.embedded_inference_state(&identity).unwrap(),
        HostInferenceState::Idle
    );
    let claim = engine.store.claims_on(&head).unwrap().remove(0);
    assert!(matches!(
        engine
            .store
            .operation_cleanup_evidence(&claim.operation)
            .unwrap(),
        OperationCleanupEvidence::Terminal(TerminalOutcome::Cancelled)
    ));
    assert!(
        engine
            .store
            .settle_embedded_round(&identity, None, &head, EmbeddedRoundOutcome::Cancelled)
            .unwrap()
    );
    let envelope = engine
        .store
        .admit_embedded_input(
            &identity,
            "later-input",
            "human",
            &Item(json!({"type":"message","role":"user","content":"continue"})),
        )
        .unwrap();
    let (sender, incoming) = tokio::sync::mpsc::unbounded_channel();
    sender
        .send(DurableMailboxWake {
            envelope_id: envelope,
        })
        .unwrap();
    let (_keep_cancel, cancellation) = watch::channel(false);
    let completion = engine
        .run_recovering_embedded(Some(head.clone()), vec![], cancellation, incoming)
        .await
        .unwrap();
    assert_eq!(engine.client.requests.lock().unwrap().len(), 2);
    assert_ne!(completion.head_request, head);
    assert!(engine.store.unread("/root").unwrap().is_empty());
    assert_eq!(
        engine.store.embedded_inference_state(&identity).unwrap(),
        HostInferenceState::Idle
    );
}

#[tokio::test]
async fn unconfirmed_cancellation_keeps_gate_closed() {
    let (engine, identity) = engine(true, false);
    let error = cancel(&engine).await;
    assert!(matches!(error, EngineError::Cleanup { primary, .. }
        if matches!(*primary, EngineError::Cancelled { .. })));
    assert_eq!(
        engine.store.embedded_inference_state(&identity).unwrap(),
        HostInferenceState::Running
    );
    let frontier = engine.store.embedded_round_frontier(&identity).unwrap();
    let claim = engine
        .store
        .claims_on(&frontier.pending_head.unwrap())
        .unwrap()
        .remove(0);
    assert!(matches!(
        engine
            .store
            .operation_cleanup_evidence(&claim.operation)
            .unwrap(),
        OperationCleanupEvidence::Uncertain
    ));
    let (_keep_cancel, cancellation) = watch::channel(false);
    assert!(matches!(
        engine
            .run_recovering_embedded(None, vec![], cancellation, mailbox())
            .await,
        Err(EngineError::Store(StoreError::InferenceRunning))
    ));
    assert_eq!(engine.client.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn rejection_keeps_its_explicit_recovery_block() {
    let (engine, identity) = engine(false, true);
    let (_keep_cancel, cancellation) = watch::channel(false);
    let EngineError::RequestRejected { head_request, .. } = engine
        .run_embedded(None, vec![], cancellation.clone(), mailbox())
        .await
        .unwrap_err()
    else {
        panic!("scripted rejection must retain its request")
    };
    let HostInferenceState::Blocked { block } =
        engine.store.embedded_inference_state(&identity).unwrap()
    else {
        panic!("provider rejection requires explicit recovery")
    };
    assert_eq!(block.request, head_request);
    assert!(matches!(
        block.reason,
        InferenceBlockReason::RequestRejected { .. }
    ));
    assert!(
        matches!(engine.run_recovering_embedded(Some(head_request), vec![], cancellation, mailbox()).await,
        Err(EngineError::InferenceBlocked { block: retained }) if retained == block)
    );
    assert_eq!(engine.client.requests.lock().unwrap().len(), 1);
}
