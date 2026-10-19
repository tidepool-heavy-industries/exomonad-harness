//! Context edits enter through an installed application tool and Engine commit.
//! Expected visible values come from the test script, not Store's projector.

#[path = "support/deepseek_http.rs"]
mod http_fixture;

use async_trait::async_trait;
use harness::{
    context::{
        ContextBlock, ContextChange, ContextNativeKind, ContextPatch, ContextProblemCode,
        ContextReference, ContextTextSelector, ContextTransaction, ContextTransactionDraft,
        ContextTransactionStatus, ContextUnitVersion, RetainedExchangeObservation,
        RetainedExchangeReference,
    },
    embedding::{
        AdmissionGuard, Conversation, EmbeddedError, EmbeddedToolManifest, HostActor, HostControl,
        HostControlError, HostIdentity, ToolSurface,
    },
    engine::{EmbeddedRoundReadiness, EngineConfig, RoundCleanup, RoundExecution},
    item::{Item, ToolInput},
    model::{AgentPath, CallId, Effort, OperationId, RequestId},
    provider::{
        CallContext, ContextDisposition, Provider, ProviderCompletion, ProviderError,
        ToolScheduling,
    },
    store::{Store, TerminalOutcome},
    transport::{
        CredentialReference, InferenceLimits, NativeTransportAdmission, NativeTransportContract,
        ResponsesClient, ResponsesProtocol, ResponsesRoute, auth::DeepSeekAuth,
    },
    turn::JobScheduler,
};
use http_fixture::{HttpReply, LocalResponses, semantic_events};
use proptest::prelude::*;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    io::Write,
    num::NonZeroU64,
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, watch};

const ORIGINAL: [&str; 2] = ["original thinking λ 🪼", "untouched second part 雪"];

enum Edit {
    Replace(String),
    RemoveOpaque,
}

struct Attempt {
    operation: OperationId,
    transaction: ContextTransaction,
    originals: Vec<String>,
}

struct Editor {
    store: Arc<Store>,
    edits: Mutex<VecDeque<Edit>>,
    attempts: Mutex<Vec<Attempt>>,
}

#[async_trait]
impl Provider for Editor {
    fn tools(&self) -> Vec<Value> {
        vec![]
    }

    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("installed context tool must receive its real invocation context");
    }

    async fn complete_call(
        &self,
        name: &str,
        input: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        assert_eq!(name, "edit");
        assert_eq!(input, ToolInput::Function(json!({})));
        let snapshot = context
            .context
            .expect("BeforeNextInference tool receives Store's cut");
        assert_eq!(context.operation.as_ref(), Some(&snapshot.operation));
        let inspection = self.store.inspect_context(&snapshot).unwrap();
        let fields = inspection
            .document
            .blocks
            .iter()
            .flat_map(|block| match block {
                ContextBlock::Native { texts, .. } => texts.as_slice(),
                _ => &[],
            })
            .filter(|field| matches!(field.selector, ContextTextSelector::ReasoningText { .. }))
            .collect::<Vec<_>>();
        assert_eq!(
            fields.len(),
            2,
            "fixture has exactly one two-part reasoning item"
        );
        assert!(fields.iter().all(|field| field.editable));
        let originals = fields
            .iter()
            .map(|field| {
                self.store
                    .original_context_text(&snapshot, &field.reference, &field.selector)
                    .unwrap()
                    .expect("authentic pre-cut reasoning field")
            })
            .collect::<Vec<_>>();
        let edit = self
            .edits
            .lock()
            .unwrap()
            .pop_front()
            .expect("one authored edit per invocation");
        let change = match edit {
            Edit::Replace(text) => {
                let target = fields
                    .iter()
                    .find(|field| field.selector == ContextTextSelector::ReasoningText { part: 0 })
                    .unwrap();
                let version = inspection
                    .text_versions
                    .iter()
                    .find(|version| {
                        version.reference == target.reference && version.selector == target.selector
                    })
                    .expect("Store inspection issues the actual visible version");
                ContextChange::ReplaceContextText {
                    reference: version.reference.clone(),
                    selector: version.selector.clone(),
                    expected: version.digest.clone(),
                    text,
                }
            }
            Edit::RemoveOpaque => {
                let groups = inspection
                    .document
                    .blocks
                    .iter()
                    .filter_map(|block| match block {
                        ContextBlock::Native {
                            reference,
                            kind: ContextNativeKind::Opaque,
                            texts,
                            ..
                        } if texts.iter().any(|field| {
                            matches!(field.selector, ContextTextSelector::ReasoningText { .. })
                        }) =>
                        {
                            Some(reference)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(groups.len(), 1);
                assert!(
                    inspection
                        .unit_versions
                        .iter()
                        .all(|version| &version.reference != groups[0]),
                    "editable thinking text does not issue whole-envelope removal authority"
                );
                ContextChange::RemoveContextUnit {
                    reference: groups[0].clone(),
                    expected: "no removal version was issued".into(),
                }
            }
        };
        let patch = ContextPatch {
            changes: vec![change],
        };
        let transaction = self.store.context_transaction(&snapshot, &patch).unwrap();
        self.attempts.lock().unwrap().push(Attempt {
            operation: snapshot.operation,
            transaction,
            originals,
        });
        let mut completion = ProviderCompletion::unedited(Ok(json!({"editor":"completed"})));
        completion.context = ContextDisposition::Transaction(ContextTransactionDraft {
            patch,
            next_model: None,
            next_effort: None,
        });
        completion
    }
}

struct Application {
    committed: Option<mpsc::UnboundedSender<OperationId>>,
    identity: HostIdentity,
    surface: Arc<ToolSurface>,
}

struct Admission;
impl AdmissionGuard for Admission {}

#[async_trait]
impl HostActor for Application {
    fn identity(&self) -> &HostIdentity {
        &self.identity
    }
    async fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError> {
        Ok(Box::new(Admission))
    }
    async fn tool_surface(&self) -> Result<Arc<ToolSurface>, EmbeddedError> {
        Ok(self.surface.clone())
    }
    async fn wake(&self, _: i64) -> Result<(), String> {
        Ok(())
    }
    async fn control(&self, _: HostControl) -> Result<Value, HostControlError> {
        panic!("application fixture does not exercise Exomonad actor lifecycle");
    }
    async fn output_committed(&self, operation: &OperationId) -> Result<(), String> {
        if let Some(committed) = &self.committed {
            committed
                .send(operation.clone())
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("deepseek-context-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn auth(&self, endpoint: &str) -> DeepSeekAuth {
        let path = self.0.join("synthetic-key");
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        writeln!(file, "offline-context-fixture-not-a-provider-key").unwrap();
        DeepSeekAuth::from_admission(
            NativeTransportAdmission::rehydrate(NativeTransportContract {
                route: ResponsesRoute::DeepSeek,
                protocol: ResponsesProtocol::Standard,
                endpoint: endpoint.into(),
                credential: CredentialReference::File { path },
                limits: InferenceLimits::deepseek_default(),
            })
            .unwrap(),
        )
        .unwrap()
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn reasoning() -> Value {
    json!({"type":"reasoning","id":"original-reasoning","status":"completed","summary":[],
        "content":ORIGINAL.map(|text| json!({"type":"reasoning_text","text":text}))})
}
fn assistant() -> Value {
    json!({"type":"message","id":"final-message","status":"completed","role":"assistant",
        "content":[{"type":"output_text","text":"round done","annotations":[]}]})
}
fn completed(id: &str, output: &[Value]) -> HttpReply {
    completed_with_usage(id, output, None)
}
fn completed_with_usage(id: &str, output: &[Value], input_tokens: Option<u64>) -> HttpReply {
    let mut events =
        vec![json!({"type":"response.created","response":{"id":id,"status":"in_progress"}})];
    for (index, item) in output.iter().enumerate() {
        events.push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
    }
    let mut response =
        json!({"id":id,"status":"completed", "model":"deepseek-flash","output":output});
    if let Some(input_tokens) = input_tokens {
        response["usage"] =
            json!({"input_tokens":input_tokens,"output_tokens":1,"total_tokens":input_tokens+1});
    }
    events.push(json!({"type":"response.completed","response":response}));
    HttpReply::fragmented_sse(semantic_events(&events), &[1, 3, 2, 7])
}

enum ExpectedProjection<'a> {
    Reasoning(&'a str),
    Exchange {
        present: bool,
        call: &'a Value,
        output: &'a Value,
    },
}

impl ExpectedProjection<'_> {
    fn assert_input(&self, body: &Value) {
        let input = body["input"].as_array().unwrap();
        match self {
            Self::Reasoning(expected) => {
                let actual = input
                    .iter()
                    .filter(|item| item["type"] == "reasoning")
                    .collect::<Vec<_>>();
                assert_eq!(actual.len(), 1);
                let mut projected = reasoning();
                projected["content"][0]["text"] = (*expected).into();
                assert_eq!(
                    *actual[0], projected,
                    "only the authored part changes in actual HTTP input"
                );
            }
            Self::Exchange {
                present,
                call,
                output,
            } => {
                assert!(
                    input.iter().all(|item| item["type"] != "reasoning"),
                    "this fixture earns a reasoning-free exchange"
                );
                let actual = input
                    .iter()
                    .filter(|item| item["call_id"] == call["call_id"])
                    .collect::<Vec<_>>();
                let expected = if *present {
                    vec![*call, *output]
                } else {
                    vec![]
                };
                assert_eq!(
                    actual, expected,
                    "actual HTTP must contain the exact source pair once or neither member"
                );
            }
        }
    }
}

async fn round(
    conversation: &Conversation,
    auth: DeepSeekAuth,
    scheduler: Arc<JobScheduler>,
    server: &mut LocalResponses,
    label: &str,
    first_output: Vec<Value>,
    expected_projection: Option<ExpectedProjection<'_>>,
) -> RequestId {
    conversation
        .input(label, "offline-human", label)
        .await
        .unwrap();
    let engine = conversation
        .engine::<DeepSeekAuth, _>(
            ResponsesClient::new(auth),
            scheduler,
            EngineConfig {
                instructions: "offline context transaction application".into(),
                tools: vec![],
                model: "deepseek-flash".into(),
                effort: Effort::High,
                session_id: "offline-context".into(),
                agent: conversation.identity().actor.clone(),
            },
            NonZeroU64::new(1_000_000).unwrap(),
        )
        .unwrap();
    let mut driver = engine.into_embedded_rounds().unwrap();
    let EmbeddedRoundReadiness::Selected(selection) = driver.next_round().unwrap() else {
        panic!("ordinary admitted input selects a round")
    };
    let (_cancel, cancel) = watch::channel(false);
    let (_wake, incoming) = mpsc::unbounded_channel();
    let running = driver.run_round(selection, cancel, incoming);
    tokio::pin!(running);
    let first = tokio::select! {
        result = &mut running => panic!("round ended before its first actual HTTP request: {result:?}"),
        request = tokio::time::timeout(Duration::from_secs(5), server.next()) => request.expect("actual first HTTP request must arrive within the round observation deadline"),
    };
    assert_eq!(first.captured.json()["model"], "deepseek-flash");
    assert_eq!(first.captured.json()["reasoning"]["effort"], "high");
    first.respond(completed(&format!("{label}-first"), &first_output));
    if let Some(expected) = expected_projection {
        let next = tokio::select! {
            result = &mut running => panic!("round ended before its post-transaction HTTP request: {result:?}"),
            request = tokio::time::timeout(Duration::from_secs(5), server.next()) => request.expect("actual post-transaction HTTP request must arrive within the round observation deadline"),
        };
        assert_eq!(next.captured.json()["reasoning"]["effort"], "high");
        expected.assert_input(&next.captured.json());
        next.respond(completed(&format!("{label}-final"), &[assistant()]));
    }
    let settled = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        settled.assessment().execution(),
        RoundExecution::Completed,
        "{settled:?}"
    );
    assert_eq!(
        settled.assessment().cleanup(),
        RoundCleanup::Confirmed,
        "{settled:?}"
    );
    assert!(settled.issue().is_none(), "{settled:?}");
    settled.assessment().head().unwrap().clone()
}

#[tokio::test]
async fn http_reasoning_transactions_preserve_originals_across_edit_refusal_and_reopen() {
    let mut server = LocalResponses::start().await;
    let files = Files::new();
    let auth = files.auth(&server.endpoint);
    let path = files.0.join("history.sqlite");
    let store = Arc::new(Store::open(&path).unwrap());
    let editor = Arc::new(Editor {
        store: store.clone(),
        edits: Mutex::new(VecDeque::new()),
        attempts: Mutex::new(vec![]),
    });
    let manifest = EmbeddedToolManifest::with_scheduling(vec![json!({"type":"function","name":"edit","strict":true,
        "parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}})],
        HashMap::from([("edit".into(), ToolScheduling::BeforeNextInference)])).unwrap();
    let application = Arc::new(Application {
        committed: None,
        identity: HostIdentity {
            run: "offline-context".into(),
            actor: AgentPath("/root".into()),
            incarnation: "live-application".into(),
        },
        surface: Arc::new(
            ToolSurface::from_manifest("context-v1".into(), Arc::new(manifest), editor.clone())
                .unwrap(),
        ),
    });
    let conversation = Conversation::attach(store.clone(), application.clone(), None)
        .await
        .unwrap();
    let scheduler = Arc::new(JobScheduler::new(1).unwrap());
    let original_head = round(
        &conversation,
        auth.clone(),
        scheduler.clone(),
        &mut server,
        "original",
        vec![reasoning(), assistant()],
        None,
    )
    .await;
    let canonical = store.items(&original_head).unwrap();
    let canonical_history = store.context_history(&original_head).unwrap();
    for (index, (edit, expected, rejected)) in [
        (
            Edit::Replace("authored first λ".into()),
            "authored first λ",
            false,
        ),
        (
            Edit::Replace("authored latest 雪".into()),
            "authored latest 雪",
            false,
        ),
        (Edit::RemoveOpaque, "authored latest 雪", true),
    ]
    .into_iter()
    .enumerate()
    {
        editor.edits.lock().unwrap().push_back(edit);
        let label = format!("editor-{index}");
        let call = json!({"type":"function_call","id":format!("item-{index}"),"call_id":label,"name":"edit","arguments":"{}"});
        round(
            &conversation,
            auth.clone(),
            scheduler.clone(),
            &mut server,
            &label,
            vec![call],
            Some(ExpectedProjection::Reasoning(expected)),
        )
        .await;
        let attempts = editor.attempts.lock().unwrap();
        assert_eq!(attempts.len(), index + 1);
        let attempt = attempts.last().unwrap();
        assert_eq!(attempt.originals, ORIGINAL.map(str::to_owned).to_vec());
        match store
            .context_transaction_status(&attempt.operation.origin, &attempt.transaction)
            .unwrap()
        {
            ContextTransactionStatus::Committed(receipt) if !rejected => assert!(receipt.changed),
            ContextTransactionStatus::Rejected(problems) if rejected => assert!(
                problems
                    .iter()
                    .any(|problem| problem.code == ContextProblemCode::Denied),
                "{problems:?}"
            ),
            other => panic!("unexpected actual transaction outcome: {other:?}"),
        }
        assert_eq!(store.items(&original_head).unwrap(), canonical);
        assert_eq!(
            store.context_history(&original_head).unwrap(),
            canonical_history
        );
    }
    assert!(editor.edits.lock().unwrap().is_empty());
    drop(conversation);
    drop(application);
    drop(editor);
    drop(scheduler);
    drop(store);
    let reopened = Store::open(path).unwrap();
    assert_eq!(reopened.items(&original_head).unwrap(), canonical);
    assert_eq!(
        reopened.context_history(&original_head).unwrap(),
        canonical_history
    );
    let originals = reopened
        .items(&original_head)
        .unwrap()
        .into_iter()
        .filter(|item| item.0["type"] == "reasoning")
        .map(|item| item.0)
        .collect::<Vec<_>>();
    assert_eq!(originals, vec![reasoning()]);
    server.finish().await;
}

const SOURCE_CALL: &str = "retained-source-call";
const SOURCE_VALUE: &str = "actual source effect λ 🪼";

#[derive(Clone, Copy, Debug)]
enum ExchangeEdit {
    Remove,
    AppendTwice,
    Append,
}

struct RetainedSource {
    group: ContextReference,
    source: RetainedExchangeReference,
    removal: ContextUnitVersion,
}

struct ExchangeEditor {
    store: Arc<Store>,
    actions: Mutex<VecDeque<ExchangeEdit>>,
    source_effects: Mutex<Vec<OperationId>>,
    retained: Mutex<Option<RetainedSource>>,
    attempts: Mutex<Vec<(OperationId, ContextTransaction, u64)>>,
}

#[async_trait]
impl Provider for ExchangeEditor {
    fn tools(&self) -> Vec<Value> {
        vec![]
    }

    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("installed tools must receive their actual operation context");
    }

    async fn complete_call(
        &self,
        name: &str,
        input: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        assert_eq!(input, ToolInput::Function(json!({})));
        if name == "source" {
            let operation = context
                .operation
                .expect("source effect has a scheduler-issued operation");
            assert_eq!(operation.call, CallId(SOURCE_CALL.into()));
            self.source_effects.lock().unwrap().push(operation);
            return ProviderCompletion::unedited(Ok(json!({"source_value":SOURCE_VALUE})));
        }
        assert_eq!(name, "edit");
        let snapshot = context.context.expect("actual synchronous editor cut");
        assert_eq!(context.operation.as_ref(), Some(&snapshot.operation));
        let inspection = self.store.inspect_context(&snapshot).unwrap();
        let mut authority = self.retained.lock().unwrap();
        if authority.is_none() {
            // At the first editor cut this fixture has issued exactly one source
            // call and published its output. Later edits retain its opaque facts.
            let groups = inspection
                .document
                .blocks
                .iter()
                .filter_map(|block| match block {
                    ContextBlock::Native {
                        reference,
                        kind: ContextNativeKind::CompletedExchange,
                        ..
                    } => Some(reference),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                groups.len(),
                1,
                "earn the source premise before context history grows"
            );
            let RetainedExchangeObservation::Available(source) = self
                .store
                .observe_retained_exchange(&snapshot, groups[0])
                .unwrap()
            else {
                panic!("actual successful source must already have an exact publication");
            };
            let removal = inspection
                .unit_versions
                .iter()
                .find(|version| &version.reference == groups[0])
                .expect("the genuine active source also issues its removal version")
                .clone();
            *authority = Some(RetainedSource {
                group: groups[0].clone(),
                source,
                removal,
            });
        }
        let authority = authority.as_ref().unwrap();
        assert_eq!(
            self.store
                .observe_retained_exchange(&snapshot, &authority.group)
                .unwrap(),
            RetainedExchangeObservation::Available(authority.source.clone()),
            "removed source stays authorized by exact pre-cut retained ancestry"
        );
        let action = self
            .actions
            .lock()
            .unwrap()
            .pop_front()
            .expect("one transaction action per editor");
        let changes = match action {
            ExchangeEdit::Remove => {
                let version = inspection
                    .unit_versions
                    .iter()
                    .find(|version| version.reference == authority.group)
                    // This real earlier version is outside the new private
                    // editable cut when inactive; submitting it tests scoped
                    // denial, not a stale-digest comparison or skipped action.
                    .unwrap_or(&authority.removal);
                vec![ContextChange::RemoveContextUnit {
                    reference: version.reference.clone(),
                    expected: version.digest.clone(),
                }]
            }
            ExchangeEdit::AppendTwice => vec![
                ContextChange::AppendRetainedExchange {
                    source: authority.source.clone()
                };
                2
            ],
            ExchangeEdit::Append => vec![ContextChange::AppendRetainedExchange {
                source: authority.source.clone(),
            }],
        };
        let patch = ContextPatch { changes };
        let transaction = self.store.context_transaction(&snapshot, &patch).unwrap();
        self.attempts
            .lock()
            .unwrap()
            .push((snapshot.operation, transaction, snapshot.generation));
        let mut completion = ProviderCompletion::unedited(Ok(json!({"editor":"completed"})));
        completion.context = ContextDisposition::Transaction(ContextTransactionDraft {
            patch,
            next_model: None,
            next_effort: None,
        });
        completion
    }
}

#[tokio::test]
async fn http_exchange_transactions_reinsert_exact_success_without_reexecution() {
    run_exchange_history(&[
        (ExchangeEdit::Remove, false),
        (ExchangeEdit::AppendTwice, false),
        (ExchangeEdit::Remove, false),
        (ExchangeEdit::Append, true),
    ])
    .await;
    // Deterministic support for both nonchanging partitions; the random cases
    // need not happen to generate either one in a small sixteen-case campaign.
    run_exchange_history(&[
        (ExchangeEdit::Remove, false),
        (ExchangeEdit::Remove, true),
        (ExchangeEdit::AppendTwice, true),
        (ExchangeEdit::Append, true),
        (ExchangeEdit::Remove, false),
        (ExchangeEdit::Append, true),
    ])
    .await;
}

async fn run_exchange_history(actions: &[(ExchangeEdit, bool)]) {
    eprintln!("retained HTTP history start: {actions:?}");
    let mut server = LocalResponses::start().await;
    let files = Files::new();
    let auth = files.auth(&server.endpoint);
    let path = files.0.join("exchange.sqlite");
    let store = Arc::new(Store::open(&path).unwrap());
    let editor = Arc::new(ExchangeEditor {
        store: store.clone(),
        actions: Mutex::new(VecDeque::new()),
        source_effects: Mutex::new(vec![]),
        retained: Mutex::new(None),
        attempts: Mutex::new(vec![]),
    });
    let tools = ["source", "edit"]
        .map(|name| {
            json!({"type":"function","name":name,"strict":true,
        "parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}})
        })
        .to_vec();
    let manifest = EmbeddedToolManifest::with_scheduling(
        tools,
        HashMap::from([
            ("source".into(), ToolScheduling::BeforeNextInference),
            ("edit".into(), ToolScheduling::BeforeNextInference),
        ]),
    )
    .unwrap();
    let application = Arc::new(Application {
        committed: None,
        identity: HostIdentity {
            run: "offline-exchange".into(),
            actor: AgentPath("/root".into()),
            incarnation: "live-application".into(),
        },
        surface: Arc::new(
            ToolSurface::from_manifest("exchange-v1".into(), Arc::new(manifest), editor.clone())
                .unwrap(),
        ),
    });
    let conversation = Conversation::attach(store.clone(), application, None)
        .await
        .unwrap();
    let scheduler = Arc::new(JobScheduler::new(1).unwrap());
    let source_call = json!({"type":"function_call","id":"retained-source-item","call_id":SOURCE_CALL,"name":"source","arguments":"{}"});
    let source_output = json!({"type":"function_call_output","call_id":SOURCE_CALL,"output":json!({"source_value":SOURCE_VALUE}).to_string()});
    let original_head = round(
        &conversation,
        auth.clone(),
        scheduler.clone(),
        &mut server,
        "source-round",
        vec![source_call.clone()],
        Some(ExpectedProjection::Exchange {
            present: true,
            call: &source_call,
            output: &source_output,
        }),
    )
    .await;
    let source_operation = {
        let effects = editor.source_effects.lock().unwrap();
        assert_eq!(effects.len(), 1);
        effects[0].clone()
    };
    let original_terminal = store
        .replay_tool_output_operation(&source_operation)
        .unwrap()
        .expect("source has a genuine retained terminal");
    assert_eq!(original_terminal.terminal, TerminalOutcome::Success);
    assert_eq!(original_terminal.item.0, source_output);
    let original_history = store.context_history(&original_head).unwrap();
    let original_pair = original_history
        .iter()
        .filter(|(_, _, item)| item.0["call_id"] == SOURCE_CALL)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(original_pair.len(), 2);
    assert_eq!(original_pair[0].0, source_operation.request);
    assert_eq!(original_pair[0].2.0, source_call);
    assert_eq!(original_pair[1].2.0, source_output);
    let mut previous_head = original_head.clone();
    // This model is independent of Store membership: only the authored action
    // changes this bit. Wire and canonical observations never set it.
    let mut present = true;
    let mut expected_generation = None;
    let mut transitions = [0usize; 5]; // remove, denied remove, restore, active append, reader
    for (index, &(action, second_view)) in actions.iter().enumerate() {
        if second_view {
            // The application/Conversation stays live. This second connection
            // only reads facts and supplies no snapshot, commit, or authority.
            let reader = Store::open(&path).unwrap();
            assert_eq!(
                reader.context_history(&original_head).unwrap(),
                original_history
            );
            assert_eq!(
                reader
                    .replay_tool_output_operation(&source_operation)
                    .unwrap(),
                Some(original_terminal.clone())
            );
            assert_active_pair(&reader, &previous_head, present, &original_pair, &editor);
            transitions[4] += 1;
        }
        let was_present = present;
        eprintln!(
            "retained HTTP step {index}: {action:?}, expected_present_before={was_present}, second_view={second_view}"
        );
        match action {
            ExchangeEdit::Remove => {
                transitions[usize::from(!present)] += 1;
                present = false;
            }
            ExchangeEdit::Append | ExchangeEdit::AppendTwice => {
                transitions[if present { 3 } else { 2 }] += 1;
                present = true;
            }
        }
        editor.actions.lock().unwrap().push_back(action);
        let label = format!("exchange-edit-{index}");
        let call = json!({"type":"function_call","id":format!("exchange-editor-item-{index}"),"call_id":label,"name":"edit","arguments":"{}"});
        previous_head = round(
            &conversation,
            auth.clone(),
            scheduler.clone(),
            &mut server,
            &label,
            vec![call],
            Some(ExpectedProjection::Exchange {
                present,
                call: &source_call,
                output: &source_output,
            }),
        )
        .await;
        let attempts = editor.attempts.lock().unwrap();
        assert_eq!(attempts.len(), index + 1);
        let (operation, transaction, observed_generation) = attempts.last().unwrap();
        let generation = *expected_generation.get_or_insert(*observed_generation);
        assert_eq!(
            *observed_generation, generation,
            "prior steps preserve the independently tracked version"
        );
        let status = store
            .context_transaction_status(&operation.origin, transaction)
            .unwrap();
        if matches!(action, ExchangeEdit::Remove) && !was_present {
            let ContextTransactionStatus::Rejected(problems) = status else {
                panic!("inactive removal must be refused, not silently skipped: {status:?}");
            };
            assert!(
                problems
                    .iter()
                    .any(|problem| problem.code == ContextProblemCode::Denied)
            );
        } else {
            let ContextTransactionStatus::Committed(receipt) = status else {
                panic!(
                    "valid generated action must commit separately from tool completion: {status:?}"
                );
            };
            assert_eq!(
                receipt.changed,
                was_present != present,
                "active append must be an actual no-op"
            );
            expected_generation = Some(generation + u64::from(was_present != present));
            assert_eq!(
                receipt.generation,
                expected_generation.unwrap(),
                "active duplicate append cannot advance the context version"
            );
        }
        assert_active_pair(&store, &previous_head, present, &original_pair, &editor);
        assert_eq!(
            *editor.source_effects.lock().unwrap(),
            vec![source_operation.clone()],
            "history changes cannot execute the source again"
        );
        assert_eq!(
            store.context_history(&original_head).unwrap(),
            original_history
        );
        assert_eq!(
            store
                .replay_tool_output_operation(&source_operation)
                .unwrap(),
            Some(original_terminal.clone())
        );
        let claims = store.claims(&CallId(SOURCE_CALL.into())).unwrap();
        assert!(!claims.is_empty());
        assert!(
            claims
                .iter()
                .all(|claim| claim.operation == source_operation)
        );
    }
    assert!(editor.actions.lock().unwrap().is_empty());
    let reader = Store::open(&path).unwrap();
    assert_active_pair(&reader, &previous_head, present, &original_pair, &editor);
    assert_eq!(
        reader.context_history(&original_head).unwrap(),
        original_history
    );
    assert_eq!(
        reader
            .replay_tool_output_operation(&source_operation)
            .unwrap(),
        Some(original_terminal)
    );
    transitions[4] += 1;
    assert_eq!(
        *editor.source_effects.lock().unwrap(),
        vec![source_operation],
        "second-reader observations cannot execute the source"
    );
    eprintln!(
        "retained HTTP history {actions:?}: remove={}, denied_remove={}, restore={}, active_append={}, second_reader={}",
        transitions[0], transitions[1], transitions[2], transitions[3], transitions[4]
    );
    server.finish().await;
}

fn assert_active_pair(
    reader: &Store,
    head: &RequestId,
    present: bool,
    original_pair: &[(RequestId, harness::item::ItemHash, Item)],
    editor: &ExchangeEditor,
) {
    let current = reader.context_history(head).unwrap();
    let actual = current
        .iter()
        .filter(|(_, _, item)| item.0["call_id"] == SOURCE_CALL)
        .map(|(_, hash, item)| (hash, item))
        .collect::<Vec<_>>();
    let expected = if present {
        original_pair
            .iter()
            .map(|(_, hash, item)| (hash, item))
            .collect()
    } else {
        vec![]
    };
    assert_eq!(
        actual, expected,
        "selected source hashes and literal pair remain exact"
    );
    if let Some(authority) = editor.retained.lock().unwrap().as_ref() {
        let document = reader.read_context(head).unwrap();
        let exact_groups = document
            .blocks
            .iter()
            .filter(|block| {
                matches!(block,
            ContextBlock::Native { reference, kind: ContextNativeKind::CompletedExchange, .. }
            if reference == &authority.group)
            })
            .count();
        assert_eq!(
            exact_groups,
            usize::from(present),
            "canonical source IDs remain the original owner-issued group"
        );
        if present {
            // This pure inspection only checks a view against previously issued
            // evidence; its result never authorizes an edit.
            let visible = harness::context::inspect_context(&document);
            let version = visible
                .unit_versions
                .iter()
                .find(|version| version.reference == authority.group)
                .unwrap();
            assert_eq!(
                *version, authority.removal,
                "the exact restored source version stays unchanged"
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 16, max_shrink_iters: 128, failure_persistence: Some(Box::new(proptest::test_runner::FileFailurePersistence::Direct("proptest-regressions/deepseek-history.txt"))), ..ProptestConfig::default() })]
    #[test]
    fn http_retained_exchange_generated_histories(
        actions in prop::collection::vec((prop_oneof![Just(ExchangeEdit::Remove), Just(ExchangeEdit::Append), Just(ExchangeEdit::AppendTwice)], any::<bool>()), 1..7)
    ) {
        // Fresh ordinary application/HTTP/Store issuers for every generated or
        // shrunk history. Output is scripted reasoning-free under EffortHigh;
        // this does not request thinking-off or claim process restart recovery.
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(run_exchange_history(&actions));
    }
}

/// The host scheduling axis is independent of the model's delivery envelope.
struct GroupTools {
    started: mpsc::UnboundedSender<String>,
    finished: mpsc::UnboundedSender<String>,
    status: tokio::sync::Notify,
    haskell: tokio::sync::Notify,
    calls: Mutex<Vec<OperationId>>,
}

#[async_trait]
impl Provider for GroupTools {
    fn tools(&self) -> Vec<Value> {
        vec![]
    }

    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("group tools require their actual invocation context")
    }

    async fn complete_call(
        &self,
        name: &str,
        input: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        match name {
            "status" => assert_eq!(input, ToolInput::Function(json!({}))),
            "haskell" => assert_eq!(input, ToolInput::Custom("pure 42".into())),
            _ => panic!("unexpected tool {name}"),
        }
        self.calls.lock().unwrap().push(context.operation.unwrap());
        self.started.send(name.into()).unwrap();
        match name {
            "status" => self.status.notified().await,
            "haskell" => self.haskell.notified().await,
            _ => unreachable!(),
        }
        self.finished.send(name.into()).unwrap();
        ProviderCompletion::unedited(Ok(json!({"completed":name})))
    }
}

#[tokio::test]
async fn http_sync_group_delivers_fifo_inputs_after_all_outputs_and_preserves_cancelled_inbox() {
    for (haskell_first, cancel) in [(true, false), (false, false), (true, true)] {
        let mut server = LocalResponses::start().await;
        let files = Files::new();
        let auth = files.auth(&server.endpoint);
        let path = files.0.join("sync-group.sqlite");
        let store = Arc::new(Store::open(&path).unwrap());
        let (started_tx, mut started) = mpsc::unbounded_channel();
        let (finished_tx, mut finished) = mpsc::unbounded_channel();
        let tools = Arc::new(GroupTools {
            started: started_tx,
            finished: finished_tx,
            status: tokio::sync::Notify::new(),
            haskell: tokio::sync::Notify::new(),
            calls: Mutex::new(vec![]),
        });
        let manifest = EmbeddedToolManifest::with_text_fields(
            vec![
                json!({"type":"function","name":"status","strict":true,
                    "parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
                json!({"type":"custom","name":"haskell","format":{"type":"text"}}),
            ],
            HashMap::from([("haskell".into(), ToolScheduling::BeforeNextInference)]),
            HashMap::from([("haskell".into(), "code".into())]),
        ).unwrap();
        let (committed_tx, mut committed) = mpsc::unbounded_channel();
        let application = Arc::new(Application {
            committed: Some(committed_tx),
            identity: HostIdentity {
                run: "sync-group".into(),
                actor: AgentPath("/root".into()),
                incarnation: "issued".into(),
            },
            surface: Arc::new(
                ToolSurface::from_manifest("group-v1".into(), Arc::new(manifest), tools.clone())
                    .unwrap(),
            ),
        });
        let conversation = Conversation::attach(store.clone(), application, None)
            .await
            .unwrap();
        conversation
            .input("initial", "operator", "initial group")
            .await
            .unwrap();
        let scheduler = Arc::new(JobScheduler::new(2).unwrap());
        let engine = conversation
            .engine::<DeepSeekAuth, _>(
                ResponsesClient::new(auth),
                scheduler,
                EngineConfig {
                    instructions: "offline response group".into(),
                    tools: vec![],
                    model: "deepseek-flash".into(),
                    effort: Effort::High,
                    session_id: "sync-group".into(),
                    agent: conversation.identity().actor.clone(),
                },
                NonZeroU64::new(1_000_000).unwrap(),
            )
            .unwrap();
        let mut driver = engine.into_embedded_rounds().unwrap();
        let EmbeddedRoundReadiness::Selected(selection) = driver.next_round().unwrap() else {
            panic!("initial input selects round")
        };
        let (cancel_tx, cancellation) = watch::channel(false);
        let (_wake, incoming) = mpsc::unbounded_channel();
        let running = driver.run_round(selection, cancellation, incoming);
        tokio::pin!(running);
        let first = tokio::select! {
            result = &mut running => panic!("missing first request: {result:?}"),
            request = server.next() => request,
        };
        let wire = first.captured.json();
        let declared = wire["tools"].as_array().unwrap();
        assert!(declared.iter().any(|tool| tool["name"] == "status"
            && tool["parameters"]["properties"]["delivery"]["enum"]
                == json!(["wait", "background"])));
        first.respond(completed("sync-group-first", &[
            reasoning(),
            json!({"type":"function_call","id":"status-item","call_id":"call00","name":"status",
                "arguments":json!({"arguments":{},"delivery":"wait"}).to_string()}),
            json!({"type":"function_call","id":"haskell-item","call_id":"call01","name":"haskell",
                "arguments":json!({"code":"pure 42"}).to_string()}),
        ]));
        let mut actual_starts = Vec::new();
        for _ in 0..2 {
            actual_starts.push(tokio::select! {
                result = &mut running => panic!("round ended before tools started: {result:?}"),
                value = tokio::time::timeout(Duration::from_secs(5), started.recv()) => value.unwrap().unwrap(),
            });
        }
        actual_starts.sort();
        assert_eq!(actual_starts, ["haskell", "status"]);
        let one = tokio::time::timeout(
            Duration::from_secs(5),
            conversation.input("queued-one", "operator", "queued first"),
        )
        .await
        .unwrap()
        .unwrap();
        let two = tokio::time::timeout(
            Duration::from_secs(5),
            conversation.input("queued-two", "operator", "queued second"),
        )
        .await
        .unwrap()
        .unwrap();
        let replay = conversation
            .input("queued-one", "operator", "queued first")
            .await
            .unwrap();
        assert_eq!(one.envelope_id, replay.envelope_id);
        assert!(
            store
                .envelope(one.envelope_id)
                .unwrap()
                .unwrap()
                .delivered_request
                .is_none()
        );
        assert!(
            store
                .envelope(two.envelope_id)
                .unwrap()
                .unwrap()
                .delivered_request
                .is_none()
        );
        if haskell_first {
            tools.haskell.notify_one();
        } else {
            tools.status.notify_one();
        }
        let first_finished = tokio::select! {
            result = &mut running => panic!("round ended with another tool held: {result:?}"),
            value = tokio::time::timeout(Duration::from_secs(5), finished.recv()) => value.unwrap().unwrap(),
        };
        assert_eq!(
            first_finished,
            if haskell_first { "haskell" } else { "status" }
        );
        if haskell_first {
            let operation = tokio::select! {
                result = &mut running => panic!("round ended before actual output publication: {result:?}"),
                value = tokio::time::timeout(Duration::from_secs(5), committed.recv()) => value.unwrap().unwrap(),
            };
            let invocation = store
                .invocation_item(&operation.request, &operation.call)
                .unwrap()
                .expect("actual issued invocation");
            assert_eq!(invocation.call_id, operation.call);
            assert_eq!(invocation.name, "haskell");
            // The installed custom source is decoded for the provider, but the
            // DeepSeek invocation and its canonical reply remain a function pair.
            assert!(matches!(invocation.input, ToolInput::Function(_)));
            let retained = store
                .replay_tool_output_operation(&operation)
                .unwrap()
                .expect("commit callback requires durable terminal");
            assert_eq!(retained.terminal, TerminalOutcome::Success);
            assert_eq!(retained.item.0["type"], "function_call_output");
            assert_eq!(retained.item.0["call_id"], invocation.call_id.0);
            assert!(
                store
                    .items(&operation.request)
                    .unwrap()
                    .contains(&retained.item)
            );
            // The other synchronous operation is still held; admission has not
            // needed to wait for its provider future or this durable terminal.
            for id in [one.envelope_id, two.envelope_id] {
                assert!(
                    store
                        .envelope(id)
                        .unwrap()
                        .unwrap()
                        .delivered_request
                        .is_none()
                );
            }
        }
        if cancel {
            cancel_tx.send(true).unwrap();
            let settled = tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                settled.assessment().execution(),
                RoundExecution::Cancelled,
                "{settled:?}"
            );
            let reopened = Store::open(&path).unwrap();
            for id in [one.envelope_id, two.envelope_id] {
                assert!(
                    reopened
                        .envelope(id)
                        .unwrap()
                        .unwrap()
                        .delivered_request
                        .is_none(),
                    "cancellation must preserve queued durable input"
                );
            }
        } else {
            if haskell_first {
                tools.status.notify_one();
            } else {
                tools.haskell.notify_one();
            }
            let next = tokio::select! {
                result = &mut running => panic!("missing next request: {result:?}"),
                request = server.next() => request,
            };
            let body = next.captured.json();
            let input = body["input"].as_array().unwrap();
            let output_position = |call: &str| {
                let matches = input
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| {
                        item["type"] == "function_call_output" && item["call_id"] == call
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                assert_eq!(matches.len(), 1, "one durable output for {call}: {input:?}");
                matches[0]
            };
            let user_position = |text: &str| {
                let matches = input
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| {
                        item["type"] == "message"
                            && item["role"] == "user"
                            && item.get("content").and_then(Value::as_str) == Some(text)
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                assert_eq!(
                    matches.len(),
                    1,
                    "each admitted user appears once: {input:?}"
                );
                matches[0]
            };
            let first_user = user_position("queued first");
            assert!(
                output_position("call00") < first_user,
                "status output must precede new user input"
            );
            assert!(
                output_position("call01") < first_user,
                "haskell output must precede new user input"
            );
            assert!(
                first_user < user_position("queued second"),
                "durable admission FIFO"
            );
            for id in [one.envelope_id, two.envelope_id] {
                assert!(
                    store
                        .envelope(id)
                        .unwrap()
                        .unwrap()
                        .delivered_request
                        .is_some()
                );
            }
            next.respond(completed("sync-group-final", &[assistant()]));
            let settled = tokio::time::timeout(Duration::from_secs(5), running)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                settled.assessment().execution(),
                RoundExecution::Completed,
                "{settled:?}"
            );
        }
        let calls = tools.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0], calls[1]);
        let ids = calls
            .iter()
            .map(|operation| operation.call.0.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids, std::collections::BTreeSet::from(["call00", "call01"]));
        drop(calls);
        server.finish().await;
    }
}

struct CompactionSteps {
    store: Arc<Store>,
    calls: Mutex<Vec<OperationId>>,
}

#[async_trait]
impl Provider for CompactionSteps {
    fn tools(&self) -> Vec<Value> {
        vec![json!({"type":"function", "name":"step", "strict":true,
            "parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}})]
    }
    fn tool_scheduling(&self, _: &str) -> ToolScheduling {
        ToolScheduling::BeforeNextInference
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("step requires its actual invocation and context cut")
    }
    async fn complete_call(
        &self,
        name: &str,
        input: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        assert_eq!(name, "step");
        assert_eq!(input, ToolInput::Function(json!({})));
        let snapshot = context
            .context
            .expect("serial tool receives real Store context");
        assert_eq!(context.operation.as_ref(), Some(&snapshot.operation));
        let items = self.store.items(&snapshot.head).unwrap();
        let expected_summaries = usize::from(self.calls.lock().unwrap().len() == 1);
        let expected_summary = json!({"type":"message", "role":"developer", "content":
            "[plain-text-compaction-v1] Earlier conversation summary:\nFirst model-generated handoff"});
        assert_eq!(
            items
                .iter()
                .filter(|item| item.0 == expected_summary)
                .count(),
            expected_summaries,
            "the second actual step must retain the first generated handoff"
        );
        let mut calls = self.calls.lock().unwrap();
        calls.push(snapshot.operation);
        // The second source really grows enough to cross the compactor's
        // repeated-attempt growth gate; token counts alone cannot force it.
        let repeat = if calls.len() == 1 { 1_000 } else { 12_000 };
        ProviderCompletion::unedited(Ok(Value::String("step evidence ".repeat(repeat))))
    }
}

#[tokio::test]
async fn http_repeated_plain_text_compaction_preserves_native_developer_and_system_roles() {
    let mut server = LocalResponses::start().await;
    let files = Files::new();
    let auth = files.auth(&server.endpoint);
    let store = Arc::new(Store::open(files.0.join("compaction.sqlite")).unwrap());
    let provider = Arc::new(CompactionSteps {
        store: store.clone(),
        calls: Mutex::new(vec![]),
    });
    let manifest = EmbeddedToolManifest::with_scheduling(
        provider.tools(),
        HashMap::from([("step".into(), ToolScheduling::BeforeNextInference)]),
    )
    .unwrap();
    let application = Arc::new(Application {
        committed: None,
        identity: HostIdentity {
            run: "compaction-roles".into(),
            actor: AgentPath("/root".into()),
            incarnation: "compaction-application".into(),
        },
        surface: Arc::new(
            ToolSurface::from_manifest(
                "compaction-v1".into(),
                Arc::new(manifest),
                provider.clone(),
            )
            .unwrap(),
        ),
    });
    let conversation = Conversation::attach(store.clone(), application, None)
        .await
        .unwrap();
    let system = json!({"type":"message", "role":"system", "content":"Explicit system policy"});
    let developer =
        json!({"type":"message", "role":"developer", "content":"Editable developer context"});
    let human_marker = "[plain-text-compaction-v1] genuine user message";
    let recent_user = "Continue the two-step task";
    let engine = conversation
        .engine::<DeepSeekAuth, _>(
            ResponsesClient::new(auth),
            Arc::new(JobScheduler::new(1).unwrap()),
            EngineConfig {
                instructions: "Explicit configuration instructions".into(),
                tools: vec![],
                model: "deepseek-flash".into(),
                effort: Effort::High,
                session_id: "compaction-roles".into(),
                agent: AgentPath("/root".into()),
            },
            NonZeroU64::new(500_000).unwrap(),
        )
        .unwrap();
    let initial = vec![
        Item(system.clone()),
        Item(developer.clone()),
        Item::user_text("Historical work and evidence. ".repeat(2_000)),
        Item::user_text(human_marker),
        Item::user_text(recent_user),
    ];
    let (_cancel, cancellation) = watch::channel(false);
    let (_mail, incoming) = mpsc::unbounded_channel();
    let running = engine.run(None, initial, cancellation, incoming);
    tokio::pin!(running);
    let first = tokio::select! {
        result = &mut running => panic!("initial DeepSeek request missing: {result:?}"),
        request = server.next() => request,
    };
    assert_eq!(first.captured.json()["input"][0], system);
    assert_eq!(first.captured.json()["input"][1], developer);
    let step = |id: &str| {
        json!({"type":"function_call", "id":format!("item-{id}"),
        "call_id":id,"name":"step","arguments":"{}"})
    };
    first.respond(completed_with_usage(
        "first-step",
        &[step("step01")],
        Some(600_000),
    ));
    let summaries = [
        "First model-generated handoff",
        "Second model-generated handoff",
    ];
    for (index, summary_text) in summaries.into_iter().enumerate() {
        let summary_request = tokio::select! {
            result = &mut running => panic!("summary request {index} missing: {result:?}"),
            request = server.next() => request,
        };
        let summary_body = summary_request.captured.json();
        assert!(
            summary_body
                .get("tool_choice")
                .is_none_or(|choice| choice == "none")
        );
        assert!(
            summary_body
                .get("tools")
                .is_none_or(|tools| tools.as_array().unwrap().is_empty())
        );
        assert_eq!(
            summary_body["instructions"],
            "Explicit configuration instructions"
        );
        summary_request.respond(completed(&format!("summary-{index}"), &[
            json!({"type":"message", "id":format!("summary-item-{index}"), "role":"assistant", "status":"completed",
                "content":[{"type":"output_text", "text":summary_text,"annotations":[]}]}),
        ]));
        let continued = tokio::select! {
            result = &mut running => panic!("post-compaction HTTP request {index} missing: {result:?}"),
            request = server.next() => request,
        };
        let body = continued.captured.json();
        let input = body["input"].as_array().unwrap();
        assert_eq!(body["instructions"], "Explicit configuration instructions");
        assert_eq!(input.iter().filter(|item| **item == system).count(), 1);
        assert_eq!(input.iter().filter(|item| **item == developer).count(), 1);
        let expected = json!({"type":"message", "role":"developer", "content":
            format!("[plain-text-compaction-v1] Earlier conversation summary:\n{summary_text}")});
        assert_eq!(
            input.iter().filter(|item| **item == expected).count(),
            1,
            "generated developer context must reach actual HTTP without rejection or system promotion"
        );
        let summary_messages = input
            .iter()
            .filter(|item| {
                item["role"] == "developer"
                    && item["content"]
                        .as_str()
                        .is_some_and(|text| text.starts_with("[plain-text-compaction-v1]"))
            })
            .count();
        assert_eq!(
            summary_messages, 1,
            "recompaction replaces the prior summary"
        );
        for text in [human_marker, recent_user] {
            assert_eq!(
                input
                    .iter()
                    .filter(|item| item["role"] == "user" && item["content"].as_str() == Some(text))
                    .count(),
                1,
                "real user context survives even when its text resembles the summary marker"
            );
        }
        if index == 0 {
            continued.respond(completed_with_usage(
                "second-step",
                &[step("step02")],
                Some(600_000),
            ));
        } else {
            continued.respond(completed("compaction-final", &[assistant()]));
        }
    }
    let completion = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap();
    let attempts = store
        .events(None)
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "compaction_attempt")
        .map(|event| serde_json::from_str::<Value>(&event.payload).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(attempts.len(), 2);
    assert!(
        attempts
            .iter()
            .all(|attempt| attempt["outcome"] == "applied")
    );
    let calls = provider.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls
            .iter()
            .map(|operation| operation.call.0.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
        std::collections::BTreeSet::from(["step01", "step02"])
    );
    assert!(
        store
            .items(&completion.head_request)
            .unwrap()
            .iter()
            .any(|item| item.0 == assistant())
    );
    drop(calls);
    server.finish().await;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReceiptProjection {
    Valid,
    Missing,
    Foreign,
}

struct ReceiptTools {
    store: Arc<Store>,
    mode: ReceiptProjection,
    calls: Mutex<Vec<OperationId>>,
    projection_issuers: Mutex<Vec<RequestId>>,
}

#[async_trait]
impl Provider for ReceiptTools {
    fn tools(&self) -> Vec<Value> {
        ["receipt", "trigger"].into_iter().map(|name| json!({
            "type":"function", "name":name, "strict":true,
            "parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}
        })).collect()
    }
    fn tool_scheduling(&self, _: &str) -> ToolScheduling {
        ToolScheduling::BeforeNextInference
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("receipt fixture requires its actual operation")
    }
    async fn complete_call(
        &self,
        name: &str,
        input: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        assert_eq!(input, ToolInput::Function(json!({})));
        self.calls.lock().unwrap().push(context.operation.unwrap());
        let mut completion = ProviderCompletion::unedited(Ok(if name == "receipt" {
            json!({"raw_receipt":"canonical receipt body ".repeat(8_192)})
        } else {
            assert_eq!(name, "trigger");
            json!("trigger complete")
        }));
        if name == "receipt" {
            completion.output_format = harness::provider::OutputFormat::NativeReceiptV1;
        }
        completion
    }
    async fn output_retained(&self, operation: &OperationId) -> Result<(), ProviderError> {
        let retained = self
            .store
            .replay_tool_output_operation(operation)
            .unwrap()
            .unwrap();
        if retained.output_format == harness::provider::OutputFormat::NativeReceiptV1
            && self.mode != ReceiptProjection::Missing
        {
            self.store
                .publish_model_output_projection(
                    operation,
                    json!({"approved":"compact annotation"}),
                )
                .unwrap();
        }
        Ok(())
    }
    fn model_visible_item(
        &self,
        request: &RequestId,
        item: &Item,
    ) -> Result<Option<harness::provider::ModelVisibleItemCandidate>, ProviderError> {
        if item.output_format().unwrap() != harness::provider::OutputFormat::NativeReceiptV1 {
            return Ok(None);
        }
        let operation = self
            .calls
            .lock()
            .unwrap()
            .iter()
            .find(|operation| operation.call.0 == item.0["call_id"].as_str().unwrap())
            .unwrap()
            .clone();
        assert_eq!(
            request, &operation.request,
            "projection must use the original output issuer across successor occurrences"
        );
        self.projection_issuers
            .lock()
            .unwrap()
            .push(request.clone());
        let source_operation =
            if self.mode == ReceiptProjection::Foreign && operation.call.0 == "native02" {
                self.calls.lock().unwrap()[0].clone()
            } else {
                operation
            };
        let candidate = self
            .store
            .model_output_projection(&source_operation)
            .map_err(|error| ProviderError::Tool(error.to_string().into()))?;
        Ok(candidate)
    }
}

struct ReceiptApplication {
    application: Application,
    provider: Arc<ReceiptTools>,
}

#[async_trait]
impl HostActor for ReceiptApplication {
    fn identity(&self) -> &HostIdentity {
        self.application.identity()
    }
    async fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError> {
        self.application.admit().await
    }
    async fn tool_surface(&self) -> Result<Arc<ToolSurface>, EmbeddedError> {
        self.application.tool_surface().await
    }
    async fn wake(&self, id: i64) -> Result<(), String> {
        self.application.wake(id).await
    }
    async fn control(&self, control: HostControl) -> Result<Value, HostControlError> {
        self.application.control(control).await
    }
    async fn output_retained(&self, operation: &OperationId) -> Result<(), String> {
        self.provider
            .output_retained(operation)
            .await
            .map_err(|error| error.to_string())
    }
}

#[tokio::test]
async fn http_text_compaction_uses_qualified_receipt_annotations_and_refuses_invalid_evidence() {
    for mode in [
        ReceiptProjection::Valid,
        ReceiptProjection::Missing,
        ReceiptProjection::Foreign,
    ] {
        let mut server = LocalResponses::start().await;
        let files = Files::new();
        let store = Arc::new(Store::open(files.0.join("receipt.sqlite")).unwrap());
        let provider = Arc::new(ReceiptTools {
            store: store.clone(),
            mode,
            calls: Mutex::new(vec![]),
            projection_issuers: Mutex::new(vec![]),
        });
        let manifest = EmbeddedToolManifest::with_scheduling(
            provider.tools(),
            HashMap::from([
                ("receipt".into(), ToolScheduling::BeforeNextInference),
                ("trigger".into(), ToolScheduling::BeforeNextInference),
            ]),
        )
        .unwrap();
        let application = Application {
            committed: None,
            identity: HostIdentity {
                run: "receipt-compaction".into(),
                actor: AgentPath("/root".into()),
                incarnation: "first".into(),
            },
            surface: Arc::new(
                ToolSurface::from_manifest(
                    "receipt-v1".into(),
                    Arc::new(manifest),
                    provider.clone(),
                )
                .unwrap(),
            ),
        };
        let application = Arc::new(ReceiptApplication {
            application,
            provider: provider.clone(),
        });
        let conversation = Conversation::attach(store.clone(), application, None)
            .await
            .unwrap();
        let engine = conversation
            .engine::<DeepSeekAuth, _>(
                ResponsesClient::new(files.auth(&server.endpoint)),
                Arc::new(JobScheduler::new(1).unwrap()),
                EngineConfig {
                    instructions: "Preserve the result evidence".into(),
                    tools: vec![],
                    model: "deepseek-flash".into(),
                    effort: Effort::High,
                    session_id: "receipt-compaction".into(),
                    agent: AgentPath("/root".into()),
                },
                NonZeroU64::new(100).unwrap(),
            )
            .unwrap();
        let (_cancel, cancellation) = watch::channel(false);
        let (_mail, incoming) = mpsc::unbounded_channel();
        let running = engine.run(
            None,
            vec![Item::user_text("Inspect the receipt and continue")],
            cancellation,
            incoming,
        );
        tokio::pin!(running);
        let first = tokio::select! {
            result = &mut running => panic!("initial request missing: {result:?}"),
            request = server.next() => request,
        };
        let call = |id: &str, name: &str| json!({"type":"function_call", "id":format!("item-{id}"), "call_id":id, "name":name, "arguments":"{}"});
        first.respond(completed_with_usage(
            "receipt-response",
            &[call("native01", "receipt")],
            Some(if mode == ReceiptProjection::Missing {
                100
            } else {
                1
            }),
        ));
        let expected = json!({"type":"function_call_output", "call_id":"native01", "output":"{\"approved\":\"compact annotation\"}"});
        let output = |body: &Value| {
            body["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| {
                    item["type"] == "function_call_output" && item["call_id"] == "native01"
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        if mode != ReceiptProjection::Missing {
            let normal = tokio::select! {
                result = &mut running => panic!("ordinary projected request missing: {result:?}"),
                request = server.next() => request,
            };
            assert_eq!(output(&normal.captured.json()), vec![expected.clone()]);
            normal.respond(completed_with_usage(
                "trigger-response",
                &[if mode == ReceiptProjection::Valid {
                    call("trigger02", "trigger")
                } else {
                    call("native02", "receipt")
                }],
                Some(100),
            ));
        }
        if mode == ReceiptProjection::Valid {
            let summary = tokio::select! {
                result = &mut running => panic!("qualified summary request missing: {result:?}"),
                request = server.next() => request,
            };
            let body = summary.captured.json();
            assert_eq!(output(&body), vec![expected]);
            assert!(
                body.get("tools")
                    .is_none_or(|tools| tools.as_array().unwrap().is_empty())
            );
            let receipt_output = output(&body).pop().unwrap();
            assert!(receipt_output.get("output_format").is_none());
            assert!(receipt_output["output"].as_str().unwrap().len() < 100);
            summary.respond(completed("receipt-summary", &[json!({"type":"message", "id":"handoff", "role":"assistant", "status":"completed", "content":[{"type":"output_text", "text":"Receipt inspected. Continue the task.", "annotations":[]} ]})]));
            let continued = tokio::select! {
                result = &mut running => panic!("post-summary request missing: {result:?}"),
                request = server.next() => request,
            };
            continued.respond(completed("receipt-final", &[assistant()]));
            tokio::time::timeout(Duration::from_secs(5), &mut running)
                .await
                .unwrap()
                .unwrap();
            let attempts = store
                .events(None)
                .unwrap()
                .into_iter()
                .filter(|event| event.kind == "compaction_attempt")
                .collect::<Vec<_>>();
            assert_eq!(attempts.len(), 1);
            assert_eq!(
                serde_json::from_str::<Value>(&attempts[0].payload).unwrap()["outcome"],
                "applied"
            );
            assert_eq!(
                provider
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|operation| operation.call.0.clone())
                    .collect::<Vec<_>>(),
                vec!["native01", "trigger02"]
            );
            assert_eq!(
                provider.projection_issuers.lock().unwrap().len(),
                2,
                "ordinary and summary requests both consult the exact issuer"
            );
        } else {
            let refused = tokio::select! {
                result = &mut running => result.expect_err("missing or foreign projection must refuse"),
                request = server.next() => panic!("unqualified receipt reached HTTP: {:?}", request.captured.json()),
            };
            let operation = provider.calls.lock().unwrap()[0].clone();
            match mode {
                ReceiptProjection::Missing => {
                    assert!(
                        matches!(store.model_output_projection(&operation), Err(harness::store::StoreError::MissingModelOutputProjection { operation: denied }) if denied == operation)
                    );
                    assert!(matches!(
                        refused,
                        harness::engine::EngineError::ProviderCall(_)
                    ));
                }
                ReceiptProjection::Foreign => assert!(
                    matches!(
                        &refused,
                        harness::engine::EngineError::Store(harness::store::StoreError::Context(
                            harness::context::ContextError::Replay(
                                harness::context::ContextReplayRejection::InvalidMembership
                            )
                        ))
                    ),
                    "{refused:?}"
                ),
                ReceiptProjection::Valid => unreachable!(),
            }
            let attempts = store
                .events(None)
                .unwrap()
                .into_iter()
                .filter(|event| event.kind == "compaction_attempt")
                .collect::<Vec<_>>();
            assert_eq!(attempts.len(), 1);
            assert_eq!(
                serde_json::from_str::<Value>(&attempts[0].payload).unwrap()["outcome"],
                "failed"
            );
            assert_eq!(
                provider.calls.lock().unwrap().len(),
                if mode == ReceiptProjection::Missing {
                    1
                } else {
                    2
                }
            );
        }
        if mode == ReceiptProjection::Foreign {
            let calls = provider.calls.lock().unwrap();
            assert_ne!(calls[0], calls[1]);
            assert!(
                store.model_output_projection(&calls[1]).unwrap().is_some(),
                "the second operation has its own genuinely issued valid annotation"
            );
            let first = store
                .replay_tool_output_operation(&calls[0])
                .unwrap()
                .unwrap();
            let second = store
                .replay_tool_output_operation(&calls[1])
                .unwrap()
                .unwrap();
            assert_eq!(
                first.item.0["output"], second.item.0["output"],
                "equal payloads do not authorize another operation's annotation"
            );
        }
        let operation = provider.calls.lock().unwrap()[0].clone();
        let canonical = store.replay_output_operation(&operation).unwrap().unwrap();
        assert_eq!(
            canonical.output_format().unwrap(),
            harness::provider::OutputFormat::NativeReceiptV1
        );
        let raw: Value = serde_json::from_str(canonical.0["output"].as_str().unwrap()).unwrap();
        assert_eq!(
            raw,
            json!({"raw_receipt":"canonical receipt body ".repeat(8_192)})
        );
        assert!(canonical.0["output"].as_str().unwrap().len() > 128 * 1024);
        server.finish().await;
    }
}
