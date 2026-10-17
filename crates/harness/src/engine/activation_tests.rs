use super::*;
use crate::provider::{
    CallContext, ContextDisposition, InvocationRole, InvocationTerminal, ProviderCompletion,
    ProviderError, RequestActivation,
};
use crate::transport::{ResponsesProtocol, ResponsesRoute};
use serde_json::Value;
use std::sync::Mutex;

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
        panic!("offline")
    }
}
fn call(id: &str, name: &str) -> Item {
    Item(json!({"type":"function_call","call_id":id,"name":name,"async":true,"arguments":"{}"}))
}
struct Script {
    turns: Vec<Vec<Item>>,
    requests: Mutex<Vec<ResponsesRequest>>,
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
            requests.len() - 1
        };
        Ok(ResponsesTurn { response_id: format!("response-{index}"), usage: Usage::default(), items: self.turns.get(index).cloned().unwrap_or_else(|| vec![Item(json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}))]) })
    }
}
struct NativeFacts {
    current: RequestActivation,
    settled: Vec<RequestActivation>,
    terminals: Vec<InvocationTerminal>,
    calls: Vec<String>,
    refused: Vec<Vec<crate::provider::PendingInvocation>>,
    successor: Option<RequestActivation>,
}
struct Host {
    facts: Arc<Mutex<NativeFacts>>,
    store: Arc<Store>,
    edit: bool,
}
struct Snapshot {
    host: Arc<Host>,
    activation: RequestActivation,
}
#[async_trait::async_trait]
impl Provider for Host {
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        unreachable!()
    }
    async fn request_snapshot(&self) -> Result<Option<Arc<dyn Provider>>, ProviderError> {
        Ok(Some(Arc::new(Snapshot {
            host: Arc::new(Host {
                facts: self.facts.clone(),
                store: self.store.clone(),
                edit: self.edit,
            }),
            activation: self.facts.lock().unwrap().current.clone(),
        })))
    }
}
#[async_trait::async_trait]
impl Provider for Snapshot {
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
    fn request_activation(&self) -> Option<RequestActivation> {
        Some(self.activation.clone())
    }
    fn invocation_role(&self, name: &str) -> InvocationRole {
        match name {
            "reply" | "reply_no_context" | "reply_error" | "reply_uncertain" | "async_reply"
            | "foreign_terminal" => InvocationRole::Settlement,
            "status" => InvocationRole::Observation,
            _ => InvocationRole::Domain,
        }
    }
    fn tool_scheduling(&self, name: &str) -> ToolScheduling {
        if name == "async_reply" {
            ToolScheduling::Async
        } else {
            ToolScheduling::BeforeNextInference
        }
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        unreachable!()
    }
    async fn complete_call(
        &self,
        name: &str,
        _: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        let operation = context.operation.as_ref().unwrap();
        assert!(
            self.host
                .store
                .recorded_response(context.request.as_ref().unwrap())
                .unwrap()
                .is_some(),
            "settlement waits for the completed provider response"
        );
        self.host
            .facts
            .lock()
            .unwrap()
            .calls
            .push(operation.call.0.clone());
        if name == "foreign_terminal" {
            let mut completion = ProviderCompletion::unedited(Ok(json!({"status":"replied"})));
            completion.activation_terminal =
                self.host.facts.lock().unwrap().terminals.first().cloned();
            return completion;
        }
        if !matches!(
            name,
            "reply" | "reply_no_context" | "reply_error" | "reply_uncertain"
        ) {
            // The same JSON on an ordinary domain tool cannot stop inference.
            return ProviderCompletion::unedited(Ok(json!({"status":"replied"})));
        }
        let admitted = context
            .invocations
            .as_ref()
            .unwrap()
            .try_admit_terminal(|inventory| {
                let mut native = self.host.facts.lock().unwrap();
                if native.settled.contains(&self.activation) {
                    return Err("already_settled");
                }
                if !inventory.is_empty() {
                    native.refused.push(inventory.to_vec());
                    return Err("pending");
                }
                native.settled.push(self.activation.clone());
                Ok(())
            });
        let (_, terminal) = match admitted {
            Ok(accepted) => accepted,
            Err(crate::provider::InvocationAdmissionError::AlreadyTerminated) => {
                return ProviderCompletion::unedited(Err("already_settled".into()));
            }
            Err(error) => return ProviderCompletion::unedited(Err(format!("{error:?}").into())),
        };
        {
            let mut native = self.host.facts.lock().unwrap();
            native.terminals.push(terminal.clone());
            if let Some(next) = native.successor.take() {
                native.current = next;
                self.host
                    .store
                    .add_envelope(
                        "/operator",
                        "/root",
                        "at_boundary",
                        &Item::user_text("request B"),
                        None,
                    )
                    .unwrap();
            }
        }
        let mut completion =
            ProviderCompletion::unedited(Ok(json!({"opaque":"native value carries no control"})));
        completion.activation_terminal = Some(terminal);
        match name {
            "reply_no_context" => completion.full_success = false,
            "reply_error" => {
                completion.output =
                    crate::turn::JobOutput::Completed(Err("native settlement failure".into()));
                completion.full_success = false;
            }
            "reply_uncertain" => {
                completion.output = crate::turn::JobOutput::CancellationUnconfirmed(
                    "native cleanup unknown".into(),
                );
                completion.full_success = false;
            }
            _ => {}
        }
        if self.host.edit {
            let mut document = context.context.unwrap().document;
            document.blocks.push(crate::context::ContextBlock::Text {
                reference: None,
                role: crate::context::ContextRole::Assistant,
                text: "durable reply context".into(),
                sources: vec![],
            });
            completion.context = ContextDisposition::Draft(crate::context::ContextDraft {
                document,
                next_model: None,
                next_effort: None,
            });
        }
        completion
    }
}
fn engine(
    turns: Vec<Vec<Item>>,
    successor: bool,
    edit: bool,
) -> (
    Engine<Offline, Host, Script>,
    Arc<Mutex<NativeFacts>>,
    Arc<Store>,
) {
    let store = Arc::new(Store::memory().unwrap());
    let facts = Arc::new(Mutex::new(NativeFacts {
        current: RequestActivation::new(),
        settled: vec![],
        terminals: vec![],
        calls: vec![],
        refused: vec![],
        successor: successor.then(RequestActivation::new),
    }));
    let engine = Engine::with_transport(
        Script {
            turns,
            requests: Mutex::new(vec![]),
        },
        store.clone(),
        Arc::new(JobScheduler::new(8).unwrap()),
        Arc::new(Host {
            facts: facts.clone(),
            store: store.clone(),
            edit,
        }),
        EngineConfig {
            instructions: "test".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "test".into(),
            agent: AgentPath("/root".into()),
        },
    );
    (engine, facts, store)
}
async fn run(engine: &Engine<Offline, Host, Script>, head: Option<RequestId>) -> EngineCompletion {
    let (_cancel, cancelled) = watch::channel(false);
    let (_mail, inbox) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        engine.run(head, vec![], cancelled, inbox),
    )
    .await
    .unwrap()
    .unwrap()
}
fn output_count(completion: &EngineCompletion, id: &str) -> usize {
    completion
        .transcript
        .iter()
        .filter(|item| item.0["type"] == "function_call_output" && item.0["call_id"] == id)
        .count()
}

#[tokio::test]
async fn reply_then_later_sync_refuses_exact_queued_id_and_executes_domain_once() {
    let (engine, facts, _) = engine(
        vec![
            vec![call("reply-first", "reply"), call("later", "domain")],
            vec![call("reply-second", "reply")],
        ],
        false,
        false,
    );
    let completion = run(&engine, None).await;
    let facts = facts.lock().unwrap();
    assert_eq!(facts.calls, ["reply-first", "later", "reply-second"]);
    assert_eq!(facts.refused.len(), 1);
    assert_eq!(facts.refused[0].len(), 1);
    assert_eq!(facts.refused[0][0].operation.call.0, "later");
    assert_eq!(
        facts.refused[0][0].state,
        crate::provider::InvocationState::Queued
    );
    assert_eq!(
        engine.client.requests.lock().unwrap().len(),
        2,
        "no successor after the accepted reply"
    );
    for id in ["reply-first", "later", "reply-second"] {
        assert_eq!(output_count(&completion, id), 1);
    }
}

#[tokio::test]
async fn sync_then_reply_accepts_completed_unread_and_commits_context_before_stop() {
    let (engine, facts, store) = engine(
        vec![vec![call("first", "domain"), call("reply", "reply")]],
        false,
        true,
    );
    let completion = run(&engine, None).await;
    assert_eq!(facts.lock().unwrap().calls, ["first", "reply"]);
    assert!(facts.lock().unwrap().refused.is_empty());
    assert_eq!(engine.client.requests.lock().unwrap().len(), 1);
    assert!(
        completion
            .transcript
            .iter()
            .any(|item| item.0.to_string().contains("durable reply context"))
    );
    let terminal = facts.lock().unwrap().terminals[0].clone();
    assert!(
        store
            .context_receipt(&terminal.operation)
            .unwrap()
            .is_some()
    );
    assert_eq!(output_count(&completion, "reply"), 1);
}

#[tokio::test]
async fn first_reply_wins_second_settlement_and_observation_still_execute() {
    let (engine, facts, _) = engine(
        vec![vec![
            call("one", "reply"),
            call("two", "reply"),
            call("observe", "status"),
        ]],
        false,
        false,
    );
    let completion = run(&engine, None).await;
    assert_eq!(facts.lock().unwrap().calls, ["one", "two", "observe"]);
    assert_eq!(facts.lock().unwrap().settled.len(), 1);
    assert_eq!(engine.client.requests.lock().unwrap().len(), 1);
    for id in ["one", "two", "observe"] {
        assert_eq!(output_count(&completion, id), 1);
    }
    assert!(
        completion
            .transcript
            .iter()
            .any(|item| item.0.to_string().contains("already_settled"))
    );
}

#[tokio::test]
async fn accepting_a_preserves_b_envelope_and_distinct_snapshot_then_b_runs() {
    let (engine, facts, store) = engine(
        vec![
            vec![call("reply-a", "reply")],
            vec![call("reply-b", "reply")],
        ],
        true,
        false,
    );
    let a = run(&engine, None).await;
    assert_eq!(engine.client.requests.lock().unwrap().len(), 1);
    assert_eq!(store.unread("/root").unwrap().len(), 1);
    assert!(
        !a.transcript
            .iter()
            .any(|item| item.0.to_string().contains("request B"))
    );
    let b = run(&engine, Some(a.head_request)).await;
    assert_eq!(engine.client.requests.lock().unwrap().len(), 2);
    assert!(store.unread("/root").unwrap().is_empty());
    assert!(
        engine.client.requests.lock().unwrap()[1]
            .input
            .iter()
            .any(|item| item.0.to_string().contains("request B"))
    );
    assert_eq!(facts.lock().unwrap().settled.len(), 2);
    assert_eq!(output_count(&b, "reply-b"), 1);
}

#[tokio::test]
async fn json_replied_and_foreign_terminal_cannot_stop_successor_activation() {
    let (engine, facts, _) = engine(
        vec![
            vec![call("reply-a", "reply")],
            vec![
                call("foreign", "foreign_terminal"),
                call("json-only", "domain"),
            ],
        ],
        true,
        false,
    );
    let a = run(&engine, None).await;
    let b = run(&engine, Some(a.head_request)).await;
    assert_eq!(
        engine.client.requests.lock().unwrap().len(),
        3,
        "B must infer after foreign token and JSON-only values"
    );
    assert_eq!(facts.lock().unwrap().settled.len(), 1);
    assert_eq!(output_count(&b, "foreign"), 1);
    assert_eq!(output_count(&b, "json-only"), 1);
}

#[tokio::test]
async fn accepted_transfer_stops_without_context_commit_eligibility() {
    let (engine, facts, store) = engine(
        vec![vec![call("transfer", "reply_no_context")]],
        false,
        false,
    );
    let completion = run(&engine, None).await;
    assert_eq!(engine.client.requests.lock().unwrap().len(), 1);
    assert_eq!(facts.lock().unwrap().settled.len(), 1);
    assert!(
        store
            .context_receipt(&facts.lock().unwrap().terminals[0].operation)
            .unwrap()
            .is_none()
    );
    assert_eq!(output_count(&completion, "transfer"), 1);
}

#[tokio::test]
async fn accepted_transfer_with_native_error_retains_error_without_another_inference() {
    let (engine, facts, _) = engine(vec![vec![call("transfer", "reply_error")]], false, false);
    let completion = run(&engine, None).await;
    assert_eq!(engine.client.requests.lock().unwrap().len(), 1);
    assert_eq!(facts.lock().unwrap().settled.len(), 1);
    assert_eq!(output_count(&completion, "transfer"), 1);
    assert!(
        completion
            .transcript
            .iter()
            .any(|item| item.0.to_string().contains("native settlement failure"))
    );
}

#[tokio::test]
async fn uncertain_transfer_preserves_cleanup_obligations_and_never_infers_again() {
    let (engine, facts, _) = engine(
        vec![vec![call("transfer", "reply_uncertain")]],
        false,
        false,
    );
    let (_cancel, cancelled) = watch::channel(false);
    let (_mail, inbox) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
    let receipt = engine
        .run_with_finalize_receipt(None, vec![], cancelled, inbox, None, false)
        .await
        .unwrap();
    assert!(receipt.result.is_err());
    assert_eq!(receipt.assessment().cleanup(), RoundCleanup::Unconfirmed);
    assert_eq!(engine.client.requests.lock().unwrap().len(), 1);
    assert_eq!(facts.lock().unwrap().settled.len(), 1);
}

#[tokio::test]
async fn asynchronous_settlement_is_refused_before_claim_or_dispatch() {
    let (engine, facts, store) = engine(vec![vec![call("bad", "async_reply")]], false, false);
    let (_cancel, cancelled) = watch::channel(false);
    let (_mail, inbox) = tokio::sync::mpsc::unbounded_channel::<Envelope>();
    let receipt = engine
        .run_with_finalize_receipt(None, vec![], cancelled, inbox, None, false)
        .await
        .unwrap();
    assert!(matches!(
        receipt.result,
        Err(EngineError::InvalidFunctionCall)
    ));
    assert!(facts.lock().unwrap().calls.is_empty());
    assert!(
        store
            .claims_on(receipt.head.as_ref().unwrap())
            .unwrap()
            .is_empty()
    );
}
