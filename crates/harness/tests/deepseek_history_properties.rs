//! Context edits enter through an installed application tool and Engine commit.
//! Expected visible values come from the test script, not Store's projector.

#[path = "support/deepseek_http.rs"]
mod http_fixture;

use async_trait::async_trait;
use harness::{
    context::{
        ContextBlock, ContextChange, ContextNativeKind, ContextPatch, ContextProblemCode,
        ContextTextSelector, ContextTransaction, ContextTransactionDraft, ContextTransactionStatus,
    },
    embedding::{
        AdmissionGuard, Conversation, EmbeddedError, EmbeddedToolManifest, HostActor, HostControl,
        HostControlError, HostIdentity, ToolSurface,
    },
    engine::{EmbeddedRoundReadiness, EngineConfig, RoundCleanup, RoundExecution},
    item::{Item, ToolInput},
    model::{AgentPath, Effort, OperationId, RequestId},
    provider::{
        CallContext, ContextDisposition, Provider, ProviderCompletion, ProviderError,
        ToolScheduling,
    },
    store::Store,
    transport::{
        CredentialReference, InferenceLimits, NativeTransportAdmission, NativeTransportContract,
        ResponsesClient, ResponsesProtocol, ResponsesRoute, auth::DeepSeekAuth,
    },
    turn::JobScheduler,
};
use http_fixture::{HttpReply, LocalResponses, semantic_events};
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

async fn round(
    conversation: &Conversation,
    auth: DeepSeekAuth,
    scheduler: Arc<JobScheduler>,
    server: &mut LocalResponses,
    label: &str,
    first_output: Vec<Value>,
    expected_visible: Option<&str>,
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
        request = server.next() => request,
    };
    assert_eq!(first.captured.json()["model"], "deepseek-flash");
    first.respond(completed(&format!("{label}-first"), &first_output));
    if let Some(expected) = expected_visible {
        let next = tokio::select! {
            result = &mut running => panic!("round ended before its post-transaction HTTP request: {result:?}"),
            request = server.next() => request,
        };
        let body = next.captured.json();
        let actual = body["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .collect::<Vec<_>>();
        assert_eq!(actual.len(), 1);
        let mut projected = reasoning();
        projected["content"][0]["text"] = expected.into();
        assert_eq!(
            *actual[0], projected,
            "only the authored part changes in actual HTTP input"
        );
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
            Some(expected),
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
