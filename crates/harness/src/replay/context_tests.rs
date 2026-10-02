use super::{ReplayProvider, ReplayTransport};
use crate::{
    context::{ContextBlock, ContextCommit, ContextDraft, ContextRole},
    engine::{Engine, EngineConfig},
    item::{Item, ToolInput},
    mailbox::Envelope,
    model::{AgentPath, CallId, Effort, OperationId, RequestId},
    provider::{
        CallContext, ContextDisposition, Provider, ProviderCompletion, ProviderError,
        ToolScheduling,
    },
    store::Store,
    transport::{Auth, ResponsesTurn, TransportError, Usage, client::request_body},
    turn::{JobOutput, JobScheduler},
};
use serde_json::json;
use std::sync::{Arc, Mutex};

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("context replay must not access a provider")
    }
}

struct Editor {
    rewrite: bool,
    note: bool,
    operations: Arc<Mutex<Vec<OperationId>>>,
}

#[async_trait::async_trait]
impl Provider for Editor {
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        unreachable!("context edits require complete invocation evidence")
    }

    fn tools(&self) -> Vec<serde_json::Value> {
        vec![
            json!({"type":"function","name":"context_edit","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ]
    }

    fn tool_scheduling(&self, _: &str) -> ToolScheduling {
        ToolScheduling::BeforeNextInference
    }

    async fn complete_call(
        &self,
        _: &str,
        _: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        let snapshot = context.context.expect("engine supplies a sealed prefix");
        self.operations
            .lock()
            .unwrap()
            .push(snapshot.operation.clone());
        let mut document = snapshot.document;
        if self.rewrite {
            let Some(block) = document
                .blocks
                .iter_mut()
                .find(|block| matches!(block, ContextBlock::Text { .. }))
            else {
                panic!("source context has an editable message")
            };
            if self.note {
                let ContextBlock::Text {
                    reference: Some(reference),
                    ..
                } = block
                else {
                    panic!("source reference")
                };
                *block = ContextBlock::Text {
                    reference: None,
                    role: ContextRole::Assistant,
                    text: "retained edited context".into(),
                    sources: vec![reference.clone()],
                };
            } else if let ContextBlock::Text { text, .. } = block {
                *text = "retained edited context".into();
            }
        }
        ProviderCompletion {
            result: Ok(
                json!({"error":"a successful payload is still successful","changed":self.rewrite}),
            ),
            full_success: true,
            context: ContextDisposition::Draft(ContextDraft {
                document,
                next_model: Some("model-after-edit".into()),
            }),
        }
    }
}

fn config() -> EngineConfig {
    EngineConfig {
        instructions: "Edit context and then answer.".into(),
        tools: vec![],
        model: "model-before-edit".into(),
        effort: Effort::Low,
        session_id: "sealed-context-replay".into(),
        agent: AgentPath("/root".into()),
    }
}

fn incoming() -> tokio::sync::mpsc::UnboundedReceiver<Envelope> {
    let (_, receiver) = tokio::sync::mpsc::unbounded_channel();
    receiver
}

fn input() -> Vec<Item> {
    vec![Item(
        json!({"type":"message","role":"user","content":"original context"}),
    )]
}

struct Recording {
    path: std::path::PathBuf,
    root: RequestId,
    original: OperationId,
    rewrite: bool,
}

impl Drop for Recording {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(self.path.with_extension("sqlite-shm"));
    }
}

async fn record(rewrite: bool) -> Recording {
    record_with_note(rewrite, false).await
}

async fn record_with_note(rewrite: bool, note: bool) -> Recording {
    let path = std::env::temp_dir().join(format!(
        "harness-context-replay-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let store = Arc::new(Store::open(&path).unwrap());
    let operations = Arc::new(Mutex::new(vec![]));
    let transport = ReplayTransport::new([
        ResponsesTurn {
            response_id: "edit-call".into(),
            items: vec![Item(
                json!({"type":"function_call","call_id":"edit","name":"context_edit","arguments":"{}"}),
            )],
            usage: Usage::default(),
        },
        ResponsesTurn {
            response_id: "after-edit".into(),
            items: vec![Item(
                json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}),
            )],
            usage: Usage::default(),
        },
    ]);
    let engine = Engine::<Offline, Editor, _>::with_transport(
        transport,
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(Editor {
            rewrite,
            note,
            operations: operations.clone(),
        }),
        config(),
    );
    let (cancel_sender, cancelled) = tokio::sync::watch::channel(false);
    let completion = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        engine.run(None, input(), cancelled, incoming()),
    )
    .await
    .expect("recording completes")
    .expect("recording succeeds");
    drop(cancel_sender);
    let original = operations.lock().unwrap()[0].clone();
    let mut root = completion.head_request;
    while let Some(parent) = store.request(&root).unwrap().unwrap().parent {
        root = parent;
    }
    let turns = store.replay_turns(&root).unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].model_request.model, "model-after-edit");
    assert!(turns[1].model_request.input.iter().any(|item| {
        item.0["content"].as_str().is_some_and(|text| {
            text.ends_with(if rewrite {
                "retained edited context"
            } else {
                "original context"
            })
        })
    }));
    Recording {
        path,
        root,
        original,
        rewrite,
    }
}

async fn replay(recording: &Recording) -> Result<(Arc<Store>, OperationId), String> {
    let source = Arc::new(Store::open(&recording.path).unwrap());
    let expected = source.replay_turns(&recording.root).unwrap();
    let provider = Arc::new(
        ReplayProvider::new(source.clone(), &recording.root).map_err(|error| error.to_string())?,
    );
    let destination = Arc::new(Store::memory().unwrap());
    let engine = Engine::<Offline, ReplayProvider, _>::with_transport(
        provider.clone(),
        destination.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        provider.clone(),
        config(),
    );
    let (cancel_sender, cancelled) = tokio::sync::watch::channel(false);
    let completion = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        engine.run(None, input(), cancelled, incoming()),
    )
    .await
    .map_err(|_| "replay timed out".to_string())?
    .map_err(|error| error.to_string())?;
    drop(cancel_sender);
    assert_eq!(provider.turns_remaining(), 0);
    assert_eq!(completion.turn.response_id, "after-edit");
    let mut root = completion.head_request;
    while let Some(parent) = destination.request(&root).unwrap().unwrap().parent {
        root = parent;
    }
    let actual = destination.replay_turns(&root).unwrap();
    assert_eq!(actual.len(), expected.len());
    for (expected, actual) in expected.iter().zip(&actual) {
        assert_eq!(
            request_body(&actual.model_request).unwrap(),
            request_body(&expected.model_request).unwrap()
        );
    }
    let local = destination
        .recorded_operation_for_request(&actual[0].request, &CallId("edit".into()))
        .unwrap()
        .unwrap();
    assert_ne!(local, recording.original);
    let receipt = destination.context_receipt(&local).unwrap().unwrap();
    assert!(receipt.changed);
    assert_eq!(receipt.model.as_deref(), Some("model-after-edit"));
    let projected = destination
        .context_request_state(&receipt.head, &local.origin)
        .unwrap();
    assert_eq!(projected.generation, receipt.generation);
    assert!(
        projected
            .history
            .iter()
            .any(|(_, _, item)| item.0["content"]
                .as_str()
                .is_some_and(|text| text.ends_with(if recording.rewrite {
                    "retained edited context"
                } else {
                    "original context"
                })))
    );
    // Replay preserves raw source history and immutable sealed requests.
    assert!(
        source
            .items(&recording.original.request)
            .unwrap()
            .iter()
            .any(|item| item.0["content"] == "original context")
    );
    assert_eq!(
        source.replay_turns(&recording.root).unwrap().len(),
        expected.len()
    );
    Ok((destination, local))
}

#[tokio::test]
async fn sealed_context_replay_restores_edited_prefix_model_and_original_operation() {
    let recording = record(true).await;
    let (destination, local) = replay(&recording).await.unwrap();
    let evidence = destination
        .context_commit_evidence(&local)
        .unwrap()
        .unwrap();
    assert_eq!(evidence.original_operation(), &recording.original);
}

#[tokio::test]
async fn sealed_context_replay_restores_model_only_edit() {
    let recording = record(false).await;
    replay(&recording).await.unwrap();
}

#[tokio::test]
async fn sealed_context_replay_restores_note_attribution_and_reuses_its_sources() {
    let recording = record_with_note(true, true).await;
    let (destination, local) = replay(&recording).await.unwrap();
    assert_ne!(local.request, recording.original.request);
    let receipt = destination.context_receipt(&local).unwrap().unwrap();
    let projection = destination
        .context_request_state(&receipt.head, &local.origin)
        .unwrap();
    let note = projection
        .history
        .iter()
        .find(|(_, _, item)| {
            item.0["content"]
                .as_str()
                .is_some_and(|text| text.ends_with("retained edited context"))
        })
        .unwrap();
    assert!(
        note.2.0["content"]
            .as_str()
            .unwrap()
            .contains(&format!("sources: {}:0", recording.original.request.0))
    );
    let raw = destination.context_history(&receipt.head).unwrap();
    assert!(
        raw.iter()
            .any(|(_, _, item)| item.0["content"] == "retained edited context")
    );

    destination.append_items(&receipt.head, &[Item(json!({"type":"function_call","call_id":"second-edit","name":"context_edit","arguments":"{}"}))]).unwrap();
    let next = destination
        .claim(&CallId("second-edit".into()), &receipt.head)
        .unwrap();
    let snapshot = destination.begin_context(&next, &receipt.head).unwrap();
    let mut document = snapshot.document.clone();
    let block = document.blocks.iter_mut().find(|block| matches!(block, ContextBlock::Text { text, .. } if text == "retained edited context")).unwrap();
    let ContextBlock::Text {
        reference: Some(reference),
        sources,
        ..
    } = block
    else {
        panic!("restored note has a local reference")
    };
    assert!(!sources.is_empty());
    assert!(reference.as_str().contains(&receipt.head.0));
    *block = ContextBlock::Text {
        reference: None,
        role: ContextRole::Assistant,
        text: "notes in the next transaction".into(),
        sources: sources.clone(),
    };
    let next_receipt = destination
        .commit_context(ContextCommit {
            snapshot: &snapshot,
            draft: &ContextDraft {
                document,
                next_model: None,
            },
            output: &JobOutput::Completed(Ok(json!("second edit done"))),
            pending: &[],
        })
        .expect("recorded note citations remain valid in a subsequent transaction");
    let next_projection = destination
        .context_request_state(&next_receipt.head, &next.origin)
        .unwrap();
    assert!(next_projection.history.iter().any(|(_, _, item)| {
        item.0["content"].as_str().is_some_and(|text| {
            text.ends_with("notes in the next transaction")
                && text.contains(&format!("sources: {}:0", recording.original.request.0))
        })
    }));
}

#[tokio::test]
async fn sealed_context_replay_refuses_legacy_edit_receipt() {
    let recording = record(true).await;
    let source = Store::open(&recording.path).unwrap();
    source.lock().execute("UPDATE events SET payload=json_remove(payload,'$.version') WHERE kind='context_commit'", []).unwrap();
    assert!(replay(&recording).await.is_err());
}

#[tokio::test]
async fn sealed_context_replay_cannot_silently_skip_missing_edit_evidence() {
    let recording = record(true).await;
    let source = Store::open(&recording.path).unwrap();
    source
        .lock()
        .execute("DELETE FROM events WHERE kind='context_commit'", [])
        .unwrap();
    let error = replay(&recording)
        .await
        .err()
        .expect("missing edit evidence must fail");
    assert!(error.contains("replay request does not match"), "{error}");
}
