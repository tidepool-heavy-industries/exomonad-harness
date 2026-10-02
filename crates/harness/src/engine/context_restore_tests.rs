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
        store.append_items(&original, &[
            completed_output.clone(),
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
