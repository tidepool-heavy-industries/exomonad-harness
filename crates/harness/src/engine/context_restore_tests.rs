//! Historical native context retains exact claim authority across restart.
use super::*;
use crate::{
    context::{ContextBlock, ContextCommit, ContextDocument, ContextDraft},
    embedding::HostIdentity,
    provider::ProviderError,
    store::{ClaimState, TerminalOutcome},
    turn::JobOutput,
};
use std::sync::Mutex;

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("offline fixture")
    }
}
struct NoTools;
#[async_trait::async_trait]
impl Provider for NoTools {
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        panic!("completed native exchanges must never execute again")
    }
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
}
struct FinalResponse(Arc<Mutex<Vec<ResponsesRequest>>>);
#[async_trait::async_trait]
impl ResponsesTransport for FinalResponse {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        let mut requests = self.0.lock().unwrap();
        assert!(requests.is_empty(), "one recovery continuation");
        requests.push(request);
        Ok(ResponsesTurn {
            response_id: "restored-final".into(),
            items: vec![Item(
                json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}),
            )],
            usage: crate::transport::Usage::default(),
        })
    }
}

#[tokio::test]
async fn saved_native_exchange_restores_exact_claim_and_recovers_after_reopen() {
    let path =
        std::env::temp_dir().join(format!("context-restore-{}.sqlite", uuid::Uuid::new_v4()));
    let identity = HostIdentity {
        run: "context-restore".into(),
        actor: AgentPath("/root".into()),
        incarnation: "original".into(),
    };
    let original = RequestId("original".into());
    let invocation =
        Item(json!({"type":"function_call","call_id":"done","name":"read","arguments":"{}"}));
    let completed_output =
        Item(json!({"type":"function_call_output","call_id":"done","output":"retained evidence"}));
    let (restored_head, completed, first, second) = {
        let store = Store::open(&path).unwrap();
        store.bind_embedded_actor(&identity, None).unwrap();
        store
            .write_embedded_request(
                &identity,
                &original,
                None,
                std::slice::from_ref(&invocation),
                StoredUsage::default(),
            )
            .unwrap();
        let completed = store.claim(&CallId("done".into()), &original).unwrap();
        store
            .settle_claims(&completed, &completed_output, TerminalOutcome::Success)
            .unwrap();
        store
            .append_operation_output(&completed, &original, &original)
            .unwrap();
        store.append_items(&original, &[
            Item(json!({"type":"custom_tool_call","call_id":"delete","name":"haskell_sync","input":"delete saved native exchange"})),
            Item(json!({"type":"custom_tool_call","call_id":"restore","name":"haskell_sync","input":"restore saved context"})),
        ]).unwrap();
        let first = store.claim(&CallId("delete".into()), &original).unwrap();
        let second = store.claim(&CallId("restore".into()), &original).unwrap();
        let snapshot = store.begin_context(&first, &original).unwrap();
        let mut saved = snapshot.document.clone();
        let outcome = JobOutput::Completed(Ok(json!({"value":"done"})));
        let deleted = store
            .commit_context(ContextCommit {
                snapshot: &snapshot,
                draft: &ContextDraft {
                    document: ContextDocument::default(),
                    next_model: None,
                    next_effort: None,
                },
                output: &outcome,
                pending: std::slice::from_ref(&second),
            })
            .unwrap();
        assert!(
            !store
                .context_history(&deleted.head)
                .unwrap()
                .iter()
                .any(|(_, _, item)| item == &invocation)
        );
        assert!(
            !store
                .claims_on(&deleted.head)
                .unwrap()
                .iter()
                .any(|claim| claim.operation == completed)
        );
        let restore = store.begin_context(&second, &deleted.head).unwrap();
        saved.blocks.extend(
            restore
                .document
                .blocks
                .iter()
                .filter(|block| {
                    matches!(
                        block,
                        ContextBlock::Native {
                            protected: true,
                            ..
                        }
                    )
                })
                .cloned(),
        );
        for block in &mut saved.blocks {
            if let ContextBlock::Native { texts, .. } = block {
                for field in texts {
                    if field.text == "retained evidence" {
                        field.text = "[Trimmed: lengthy evidence]\nretained observation".into();
                    }
                }
            }
        }
        let restored = store
            .commit_context(ContextCommit {
                snapshot: &restore,
                draft: &ContextDraft {
                    document: saved,
                    next_model: None,
                    next_effort: None,
                },
                output: &outcome,
                pending: &[],
            })
            .unwrap();
        (restored.head, completed, first, second)
    };
    let store = Arc::new(Store::open(&path).unwrap());
    let claims = store
        .claims_on_branch_lineage(&restored_head, &identity.actor.0)
        .unwrap();
    let restored = claims
        .iter()
        .filter(|claim| claim.operation == completed)
        .collect::<Vec<_>>();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].request, restored_head);
    assert_eq!(restored[0].state, ClaimState::Settled);
    assert_eq!(
        restored[0].output,
        Some(store.put_item(&completed_output).unwrap())
    );
    for operation in [&first, &second] {
        assert_eq!(
            claims
                .iter()
                .filter(|claim| &claim.operation == operation)
                .count(),
            1
        );
    }
    let requests = Arc::new(Mutex::new(vec![]));
    let engine: Engine<Offline, NoTools, FinalResponse> = Engine::with_transport(
        FinalResponse(requests.clone()),
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(NoTools),
        EngineConfig {
            instructions: "recover".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "restore".into(),
            agent: identity.actor.clone(),
        },
    )
    .with_origin(completed.origin.clone());
    let (_cancel, cancel) = watch::channel(false);
    engine
        .run_recovering_embedded(
            None,
            vec![],
            cancel,
            tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>().1,
        )
        .await
        .unwrap();
    let input = &requests.lock().unwrap()[0].input;
    assert_eq!(input.iter().filter(|item| *item == &invocation).count(), 1);
    assert_eq!(
        input
            .iter()
            .filter(|item| item.0["call_id"] == "done"
                && item.0["output"] == "[Trimmed: lengthy evidence]\nretained observation")
            .count(),
        1
    );
    drop(engine);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn checkpoint_child_context_rewrite_preserves_consuming_claim_through_recovery() {
    use crate::{checkpoint::CheckpointChild, context::ContextRole, item::ToolKind};
    let mut histories = 0;
    for descendant in [false, true] {
        for reused_wire_id in [false, true] {
            let store = Arc::new(Store::memory().unwrap());
            let root = AgentPath("/root".into());
            let child = AgentPath("/root/child".into());
            let source = RequestId("checkpoint-source".into());
            let call = CallId("shared-wire".into());
            store.create_request(&source, None, &root.0).unwrap();
            store.set_effort(&source, Effort::Low).unwrap();
            store.append_items(&source, &[Item(json!({"type":"function_call","call_id":call.0,"name":"work","arguments":"{}","async":true}))]).unwrap();
            let original = store.claim(&call, &source).unwrap();
            store
                .admit_agent(&root, None, Some(&source), &json!({}), &json!({}))
                .unwrap();
            let checkpoint = store
                .capture_checkpoint(&root, &source, &call, &json!({}), Arc::new(()))
                .unwrap();
            let (attached, _) = store
                .attach_checkpoint_child(
                    &checkpoint,
                    CheckpointChild {
                        path: &child,
                        parent: &root,
                        contract: &json!({}),
                        checkout: &json!({}),
                        task: None,
                    },
                )
                .unwrap();
            let child_head = attached.head_request.unwrap();
            assert_eq!(
                store
                    .interrupt_operation_claim(&original, &child_head)
                    .unwrap(),
                1
            );
            let consumer = if descendant {
                let next = RequestId("child-later-request".into());
                store
                    .create_request(&next, Some(&child_head), &child.0)
                    .unwrap();
                next
            } else {
                child_head.clone()
            };
            let interrupted = Item::tool_output(&call, ToolKind::Function, &JobOutput::Interrupted);
            store
                .append_operation_output(&original, &child_head, &consumer)
                .unwrap();
            let edit_call = if reused_wire_id {
                call.clone()
            } else {
                CallId("edit".into())
            };
            store.append_items(&consumer,&[Item(json!({"type":"custom_tool_call","call_id":edit_call.0,"name":"haskell_sync","input":"edit context"}))]).unwrap();
            let edit = store.claim(&edit_call, &consumer).unwrap();
            assert_ne!(edit, original);
            let snapshot = store.begin_context(&edit, &consumer).unwrap();
            let mut document = snapshot.document.clone();
            document.blocks.insert(
                0,
                ContextBlock::Text {
                    reference: None,
                    role: ContextRole::Assistant,
                    text: "rewritten child context".into(),
                    sources: vec![],
                },
            );
            let receipt = store
                .commit_context(ContextCommit {
                    snapshot: &snapshot,
                    draft: &ContextDraft {
                        document,
                        next_model: None,
                        next_effort: None,
                    },
                    output: &JobOutput::Completed(Ok(json!("edited"))),
                    pending: &[],
                })
                .unwrap();
            assert_ne!(receipt.head, consumer);
            let selected = store
                .claims_on_branch_lineage(&receipt.head, &child.0)
                .unwrap();
            let inherited = selected
                .iter()
                .filter(|claim| claim.operation == original)
                .collect::<Vec<_>>();
            assert_eq!(inherited.len(), 1);
            assert_eq!(inherited[0].state, ClaimState::Interrupted);
            assert_eq!(
                store.claims_on(checkpoint.snapshot_request()).unwrap()[0].state,
                ClaimState::Pending
            );
            assert_eq!(
                store.claims_on(&source).unwrap()[0].state,
                ClaimState::Pending
            );
            let mut head = receipt.head;
            for _ in 0..2 {
                let requests = Arc::new(Mutex::new(vec![]));
                let engine: Engine<Offline, NoTools, FinalResponse> = Engine::with_transport(
                    FinalResponse(requests.clone()),
                    store.clone(),
                    Arc::new(JobScheduler::new(1).unwrap()),
                    Arc::new(NoTools),
                    EngineConfig {
                        instructions: "recover".into(),
                        tools: vec![],
                        model: "test".into(),
                        effort: Effort::Low,
                        session_id: "child-rewrite".into(),
                        agent: child.clone(),
                    },
                );
                let (_cancel, cancel) = watch::channel(false);
                let completion = engine
                    .run_recovering(
                        Some(head),
                        vec![],
                        cancel,
                        tokio::sync::mpsc::unbounded_channel::<Envelope>().1,
                    )
                    .await
                    .unwrap();
                assert_eq!(requests.lock().unwrap().len(), 1);
                let owned = store
                    .recovery_history(&completion.head_request)
                    .unwrap()
                    .into_iter()
                    .filter_map(|record| record.output)
                    .filter(|output| output.operation == original)
                    .collect::<Vec<_>>();
                assert_eq!(owned.len(), 1);
                assert_eq!(owned[0].item, interrupted);
                assert_eq!(
                    store
                        .claims_on_branch_lineage(&completion.head_request, &child.0)
                        .unwrap()
                        .iter()
                        .find(|claim| claim.operation == original)
                        .unwrap()
                        .state,
                    ClaimState::Interrupted
                );
                head = completion.head_request;
            }
            histories += 1;
        }
    }
    assert_eq!(histories, 4);
    eprintln!(
        "checkpoint_claim_rewrite: histories=4 consumer_depth_partitions=2 wire_id_partitions=2 fresh_recoveries=8"
    );
}

#[tokio::test]
async fn embedded_compaction_retains_terminal_claims_before_fresh_recovery() {
    use crate::{item::ToolKind, store::EmbeddedRoundOutcome};
    let mut histories = 0;
    for interrupted in [false, true] {
        for operation_count in [1, 2] {
            let store = Arc::new(Store::memory().unwrap());
            let identity = HostIdentity {
                run: "compaction-recovery".into(),
                actor: AgentPath("/root".into()),
                incarnation: "one".into(),
            };
            store.bind_embedded_actor(&identity, None).unwrap();
            let mut source = None;
            let mut expected = Vec::new();
            for index in 0..operation_count {
                let request = RequestId(format!("issuer-{index}"));
                let call = CallId("reused".into());
                store.write_embedded_request(&identity,&request,source.as_ref(),&[Item(json!({"type":"function_call","call_id":call.0,"name":format!("work-{index}"),"arguments":"{}","async":true}))],StoredUsage::default()).unwrap();
                store.set_effort(&request, Effort::Low).unwrap();
                let operation = store.claim(&call, &request).unwrap();
                if interrupted && index == 0 {
                    store
                        .interrupt_operation_claim(&operation, &request)
                        .unwrap();
                } else {
                    store
                        .write_job_output(
                            &operation,
                            ToolKind::Function,
                            &JobOutput::Completed(Ok(json!(format!("result-{index}")))),
                        )
                        .unwrap();
                }
                let canonical = store
                    .append_operation_output(&operation, &request, &request)
                    .unwrap();
                expected.push((
                    operation,
                    canonical.item,
                    if interrupted && index == 0 {
                        ClaimState::Interrupted
                    } else {
                        ClaimState::Settled
                    },
                ));
                source = Some(request);
            }
            let source = source.unwrap();
            let origin = expected[0].0.origin.clone();
            let issued = store.context_request_state(&source, &origin).unwrap();
            let mut items = vec![Item(
                json!({"type":"message","role":"developer","content":"compacted context"}),
            )];
            items.extend(
                issued
                    .history
                    .iter()
                    .filter(|(_, _, item)| !item.is_configuration_update())
                    .map(|(_, _, item)| item.clone()),
            );
            items.push(Item::configuration_update(Effort::Low));
            let boundary = RequestId("compaction-boundary".into());
            store
                .write_compaction_request_with_evidence(
                    &boundary,
                    &source,
                    &identity.actor.0,
                    &items,
                    &[],
                    &issued.occurrences,
                    &[],
                    Some(&identity),
                    None,
                )
                .unwrap();
            let frontier = store.embedded_round_frontier(&identity).unwrap();
            assert!(frontier.settled_head.is_none());
            assert_eq!(frontier.pending_head, Some(boundary.clone()));
            let copied = store.claims_on(&boundary).unwrap();
            assert_eq!(copied.len(), operation_count);
            for (operation, _, state) in &expected {
                assert_eq!(
                    copied
                        .iter()
                        .find(|claim| &claim.operation == operation)
                        .unwrap()
                        .state,
                    *state
                );
            }
            let requests = Arc::new(Mutex::new(vec![]));
            let engine: Engine<Offline, NoTools, FinalResponse> = Engine::with_transport(
                FinalResponse(requests.clone()),
                store.clone(),
                Arc::new(JobScheduler::new(1).unwrap()),
                Arc::new(NoTools),
                EngineConfig {
                    instructions: "recover".into(),
                    tools: vec![],
                    model: "test".into(),
                    effort: Effort::Low,
                    session_id: "compaction-restart".into(),
                    agent: identity.actor.clone(),
                },
            )
            .with_origin(origin);
            let (_cancel, cancel) = watch::channel(false);
            let completion = engine
                .run_recovering_embedded(
                    None,
                    vec![],
                    cancel.clone(),
                    tokio::sync::mpsc::unbounded_channel::<DurableMailboxWake>().1,
                )
                .await
                .unwrap();
            assert_eq!(requests.lock().unwrap().len(), 1);
            let outputs = store
                .recovery_history(&completion.head_request)
                .unwrap()
                .into_iter()
                .filter_map(|record| record.output)
                .collect::<Vec<_>>();
            assert_eq!(outputs.len(), operation_count);
            for (operation, item, _) in &expected {
                let owned = outputs
                    .iter()
                    .filter(|output| &output.operation == operation)
                    .collect::<Vec<_>>();
                assert_eq!(owned.len(), 1);
                assert_eq!(&owned[0].item, item);
            }
            assert!(
                store
                    .settle_embedded_round(
                        &identity,
                        None,
                        &completion.head_request,
                        EmbeddedRoundOutcome::Completed
                    )
                    .unwrap()
            );
            histories += 1;
        }
    }
    assert_eq!(histories, 4);
    eprintln!(
        "compaction_claim_recovery: histories=4 terminal_partitions=2 operation_count_partitions=2"
    );
}
