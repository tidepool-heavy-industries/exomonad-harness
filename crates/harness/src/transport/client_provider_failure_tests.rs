use super::*;
use crate::{
    embedding::{
        ClientOperationId, HostIdentity, HostInferenceState, InferenceBlockReason, RecoveryMode,
    },
    engine::{Engine, EngineConfig, EngineError, ResponsesTransport},
    model::{AgentPath, ConversationIdentity},
    provider::{Provider, ProviderError},
    server::{CommandReceiptOutcome, HostCommand},
    store::{PreparedProviderRequest, Store},
    transport::{Effort, ProviderStreamFailureEvent},
    turn::JobScheduler,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TOKEN: &str = "mock-active-token";
const ACCOUNT: &str = "mock-active-account";
struct Offline;
impl Auth for Offline {
    fn admission(
        &self,
    ) -> std::result::Result<
        crate::transport::NativeTransportAdmission,
        crate::transport::TransportError,
    > {
        crate::transport::NativeTransportAdmission::chatgpt_plan(std::path::PathBuf::from(
            "/offline/synthetic-credential",
        ))
    }
    fn access(&self) -> Result<String, TransportError> {
        panic!("scripted transport must not read credentials")
    }
}
struct NoTools;
#[async_trait::async_trait]
impl Provider for NoTools {
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("provider failure must not execute a tool")
    }
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}
struct SseFailure {
    url: String,
    calls: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl ResponsesTransport for SseFailure {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard))
    }
    async fn create(&self, _: PreparedProviderRequest) -> Result<ResponsesTurn, TransportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let response = http_client().unwrap().get(&self.url).send().await.unwrap();
        read_response(response, None, TOKEN, ACCOUNT, None).await
    }
}

#[tokio::test]
async fn provider_failure_diagnostic_survives_engine_block_reopen_and_recovery_receipt() {
    for (event, with_diagnostic, code) in [
        (
            ProviderStreamFailureEvent::ResponseFailed,
            true,
            "invalid_tools",
        ),
        (ProviderStreamFailureEvent::Error, true, "invalid_tools"),
        (
            ProviderStreamFailureEvent::ResponseFailed,
            false,
            "invalid_tools",
        ),
        (ProviderStreamFailureEvent::Error, false, "invalid_tools"),
        (
            ProviderStreamFailureEvent::Error,
            true,
            "subscription_sharing_usage_limit_exceeded",
        ),
    ] {
        let diagnostic = json!({
            "code":code, "type":"invalid_request_error", "param":"tools[0].parameters",
            "message":format!("schema refused {TOKEN} {ACCOUNT} Bearer other-secret sk-other acct_other {}", "λ".repeat(2048)),
        });
        let payload = if with_diagnostic {
            diagnostic
        } else {
            Value::Null
        };
        let packet = match event {
            ProviderStreamFailureEvent::ResponseFailed => {
                json!({"type":"response.failed", "response":{
                "error":payload, "input":"private prompt", "output":"private response"}})
            }
            ProviderStreamFailureEvent::Error => json!({"type":"error", "error":payload,
                "request":{"input":"private prompt"}, "authorization":TOKEN}),
            _ => unreachable!("this Responses fixture enumerates only Responses failure events"),
        };
        let body = format!("data: {packet}\n\n");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            socket.read(&mut request).await.unwrap();
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
        });
        let path = std::env::temp_dir().join(format!(
            "harness-provider-failure-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let identity = HostIdentity {
            run: "provider-failure".into(),
            actor: AgentPath("/root".into()),
            incarnation: "one".into(),
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let store = Arc::new(Store::open(&path).unwrap());
        store.bind_embedded_actor(&identity, None).unwrap();
        let engine = Engine::<Offline, _, _>::with_transport(
            SseFailure {
                url: format!("http://{address}/responses"),
                calls: calls.clone(),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(NoTools),
            EngineConfig {
                instructions: "offline".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "provider-failure".into(),
                agent: identity.actor.clone(),
            },
        )
        .with_origin(ConversationIdentity::Embedded {
            run: identity.run.clone(),
            actor: identity.actor.clone(),
            incarnation: identity.incarnation.clone(),
        });
        let (_stop, cancel) = tokio::sync::watch::channel(false);
        let error = engine
            .run_embedded(
                None,
                vec![],
                cancel.clone(),
                tokio::sync::mpsc::unbounded_channel().1,
            )
            .await
            .unwrap_err();
        server.await.unwrap();
        let EngineError::BlockedAction(failure) = error else {
            panic!("unexpected error: {error}");
        };
        let block = failure.block().clone();
        assert!(matches!(
            failure.primary(),
            EngineError::Transport(TransportError::ProviderStreamFailure { .. })
        ));
        let InferenceBlockReason::ProviderStreamFailure {
            event: actual,
            diagnostic,
        } = &block.reason
        else {
            panic!("wrong block reason");
        };
        assert_eq!(*actual, event);
        if with_diagnostic {
            let diagnostic = diagnostic.as_ref().expect("sanitized provider fields");
            assert_eq!(diagnostic.code.as_deref(), Some(code));
            assert_eq!(
                diagnostic.error_type.as_deref(),
                Some("invalid_request_error")
            );
            assert_eq!(diagnostic.param.as_deref(), Some("tools[0].parameters"));
            let message = diagnostic.message.as_ref().unwrap();
            assert!(message.starts_with(
                "schema refused [redacted] [redacted] Bearer [redacted] [redacted] [redacted]"
            ));
            assert_eq!(message.chars().count(), 2048);
        } else {
            assert_eq!(*diagnostic, None);
            assert!(
                serde_json::to_value(&block.reason)
                    .unwrap()
                    .get("diagnostic")
                    .is_none()
            );
        }
        let encoded = serde_json::to_string(&block).unwrap();
        for private in [
            TOKEN,
            ACCOUNT,
            "other-secret",
            "sk-other",
            "acct_other",
            "private prompt",
            "private response",
            "authorization",
        ] {
            assert!(
                !encoded.contains(private),
                "retained private value: {private}"
            );
        }
        assert_eq!(
            store.embedded_inference_state(&identity).unwrap(),
            HostInferenceState::Blocked {
                block: block.clone()
            }
        );
        assert!(
            matches!(engine.run_recovering_embedded(None, vec![], cancel, tokio::sync::mpsc::unbounded_channel().1).await,
            Err(EngineError::InferenceBlocked { block:retained }) if retained == block)
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "block must prevent automatic retry"
        );
        assert!(
            !store
                .events(Some(&block.request))
                .unwrap()
                .iter()
                .any(|event| event.kind == "request_failed"),
            "provider failure does not prove pre-response rejection"
        );
        drop(engine);
        drop(store);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.embedded_inference_state(&identity).unwrap(),
            HostInferenceState::Blocked {
                block: block.clone()
            }
        );
        let mut changed = block.clone();
        let InferenceBlockReason::ProviderStreamFailure { diagnostic, .. } = &mut changed.reason
        else {
            unreachable!()
        };
        *diagnostic = Some(super::super::HttpDiagnostic {
            message: Some("different diagnostic".into()),
            ..Default::default()
        });
        let mut missing = block.clone();
        let InferenceBlockReason::ProviderStreamFailure { diagnostic, .. } = &mut missing.reason
        else {
            unreachable!()
        };
        *diagnostic = None;
        for stale in [changed, missing]
            .into_iter()
            .filter(|stale| stale != &block)
        {
            let operation = ClientOperationId(uuid::Uuid::new_v4());
            reopened
                .enqueue_embedded_command(
                    operation,
                    &HostCommand::Recover {
                        target: identity.clone(),
                        expected_block: stale,
                        mode: RecoveryMode::Exact,
                    },
                )
                .unwrap();
            reopened
                .claim_embedded_command(&identity.run, operation)
                .unwrap()
                .unwrap();
            let receipt = reopened
                .authorize_embedded_recovery(&identity, operation)
                .unwrap();
            assert!(matches!(
                receipt.outcome,
                CommandReceiptOutcome::ContinuationRefused {
                    reason: crate::embedding::ContinuationRefusal::Stale,
                    ..
                }
            ));
            assert_eq!(
                reopened.embedded_inference_state(&identity).unwrap(),
                HostInferenceState::Blocked {
                    block: block.clone()
                }
            );
        }
        let operation = ClientOperationId(uuid::Uuid::new_v4());
        reopened
            .enqueue_embedded_command(
                operation,
                &HostCommand::Recover {
                    target: identity.clone(),
                    expected_block: block.clone(),
                    mode: RecoveryMode::Exact,
                },
            )
            .unwrap();
        reopened
            .claim_embedded_command(&identity.run, operation)
            .unwrap()
            .unwrap();
        let receipt = reopened
            .authorize_embedded_recovery(&identity, operation)
            .unwrap();
        assert!(
            matches!(&receipt.outcome, CommandReceiptOutcome::RecoveryAuthorized { block:retained, .. } if retained == &block)
        );
        let retained = reopened
            .embedded_command(&identity.run, operation)
            .unwrap()
            .unwrap();
        assert_eq!(retained.receipt, Some(receipt));
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn old_provider_failure_block_without_diagnostic_remains_readable() {
    let reason: InferenceBlockReason =
        serde_json::from_value(json!({"kind":"provider_stream_failure", "event":"error"})).unwrap();
    assert_eq!(
        reason,
        InferenceBlockReason::ProviderStreamFailure {
            event: ProviderStreamFailureEvent::Error,
            diagnostic: None
        }
    );
    assert_eq!(
        serde_json::to_value(reason).unwrap(),
        json!({"kind":"provider_stream_failure", "event":"error"})
    );
}
