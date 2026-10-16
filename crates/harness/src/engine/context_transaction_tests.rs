use super::*;
use crate::turn::JobOutput;
use crate::{
    context::{
        ContextChange, ContextPatch, ContextProblemCode, ContextTransaction,
        ContextTransactionDraft, ContextTransactionStatus, inspect_context,
    },
    provider::{CallContext, ContextDisposition, ProviderCompletion, ProviderError},
    transport::{ResponsesProtocol, ResponsesRoute},
};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::mpsc;

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<String, TransportError> {
        panic!("offline context transaction test");
    }
}
#[derive(Clone, Copy)]
enum Mode {
    Committed,
    Rejected,
    Discarded,
}
struct Host {
    store: Arc<Store>,
    mode: Mode,
    handle: Mutex<Option<(OperationId, ContextTransaction)>>,
    committed: AtomicUsize,
    aborted: AtomicUsize,
}
#[async_trait::async_trait]
impl Provider for Host {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
    fn tool_scheduling(&self, _: &str) -> ToolScheduling {
        ToolScheduling::BeforeNextInference
    }
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        unreachable!()
    }
    async fn complete_call(
        &self,
        _: &str,
        _: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        let snapshot = context.context.unwrap();
        let target = inspect_context(&snapshot.document).text_versions[0].clone();
        let patch = ContextPatch {
            changes: vec![ContextChange::ReplaceContextText {
                reference: target.reference,
                selector: target.selector,
                expected: if matches!(self.mode, Mode::Rejected) {
                    crate::context::context_digest(b"stale text")
                } else {
                    target.digest
                },
                text: "replacement".into(),
            }],
        };
        *self.handle.lock().unwrap() = Some((
            snapshot.operation.clone(),
            self.store.context_transaction(&snapshot, &patch).unwrap(),
        ));
        let draft = ContextTransactionDraft {
            patch,
            next_model: None,
            next_effort: None,
        };
        ProviderCompletion {
            output: JobOutput::Completed(Ok(json!({"authored":"success"}))),
            full_success: !matches!(self.mode, Mode::Discarded),
            context: if matches!(self.mode, Mode::Discarded) {
                ContextDisposition::Discarded(draft)
            } else {
                ContextDisposition::Transaction(draft)
            },
            activation_terminal: None,
        }
    }
    async fn output_committed(&self, _: &OperationId) -> Result<(), ProviderError> {
        self.committed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn output_aborted(&self, _: &OperationId) -> Result<(), ProviderError> {
        self.aborted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
struct Script {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
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
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request.request().clone());
            requests.len()
        };
        Ok(ResponsesTurn {
            response_id: format!("response-{index}"),
            usage: Usage::default(),
            items: if index == 1 {
                vec![Item(
                    json!({"type":"function_call","call_id":"editor","name":"cell","arguments":"{}"}),
                )]
            } else {
                vec![Item(
                    json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}),
                )]
            },
        })
    }
}
async fn exercise(mode: Mode) {
    let store = Arc::new(Store::memory().unwrap());
    let host = Arc::new(Host {
        store: store.clone(),
        mode,
        handle: Mutex::new(None),
        committed: AtomicUsize::new(0),
        aborted: AtomicUsize::new(0),
    });
    let requests = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::<Offline, _, _>::with_transport(
        Script {
            requests: requests.clone(),
        },
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        host.clone(),
        EngineConfig {
            instructions: "context transaction".into(),
            tools: vec![],
            model: "model-a".into(),
            effort: Effort::Low,
            session_id: "context-transaction".into(),
            agent: AgentPath("/root".into()),
        },
    );
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = mpsc::unbounded_channel::<Envelope>();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        engine.run(None, vec![Item::user_text("original")], cancel, mail),
    )
    .await
    .unwrap()
    .unwrap();
    let (operation, handle) = host.handle.lock().unwrap().clone().unwrap();
    let status = store
        .context_transaction_status(&operation.origin, &handle)
        .unwrap();
    match mode {
        Mode::Committed => {
            assert!(matches!(status, ContextTransactionStatus::Committed(_)));
            assert_eq!(host.committed.load(Ordering::SeqCst), 1);
            assert_eq!(host.aborted.load(Ordering::SeqCst), 0);
        }
        Mode::Rejected | Mode::Discarded => {
            assert!(
                matches!(status, ContextTransactionStatus::Rejected(problems) if problems.iter().any(|problem| problem.code == if matches!(mode, Mode::Rejected) { ContextProblemCode::Stale } else { ContextProblemCode::Ineligible }))
            );
            assert_eq!(host.committed.load(Ordering::SeqCst), 0);
            assert_eq!(host.aborted.load(Ordering::SeqCst), 1);
        }
    }
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "normal rejection must continue inference"
    );
    assert!(requests[1].input.iter().any(|item| item
        == &Item::tool_output(
            &operation.call,
            crate::item::ToolKind::Function,
            &JobOutput::Completed(Ok(json!({"authored":"success"})))
        )));
    let expected = if matches!(mode, Mode::Committed) {
        "replacement"
    } else {
        "original"
    };
    assert!(requests[1].input.iter().any(|item| {
        item.0["content"].as_str() == Some(expected)
            || item.0["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["text"] == expected))
    }));
}
#[tokio::test]
async fn committed_transaction_acknowledges_and_continues_inference() {
    exercise(Mode::Committed).await;
}
#[tokio::test]
async fn rejected_transaction_aborts_unpublished_context_and_continues_inference() {
    exercise(Mode::Rejected).await;
}
#[tokio::test]
async fn discarded_transaction_retains_candidate_aborts_and_continues_inference() {
    exercise(Mode::Discarded).await;
}
