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
    let mut events =
        vec![json!({"type":"response.created","response":{"id":id,"status":"in_progress"}})];
    for (index, item) in output.iter().enumerate() {
        events.push(json!({"type":"response.output_item.done","output_index":index,"item":item}));
    }
    events.push(
        json!({"type":"response.completed","response":{"id":id,"status":"completed",
        "model":"deepseek-flash","output":output}}),
    );
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
    #![proptest_config(ProptestConfig { cases: 16, ..ProptestConfig::default() })]
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
