use super::*;
use crate::{
    context::{ContextBlock, ContextDraft, ContextRole},
    provider::{CallContext, ContextDisposition, ProviderCompletion, ProviderError},
    transport::{ResponsesProtocol, client::request_body_for_protocol},
};
use std::{sync::Mutex, time::Duration};
use tokio::sync::{Semaphore, mpsc};

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<(String, String), TransportError> {
        panic!("offline regression must not authenticate")
    }
}

struct NativeHost {
    started: mpsc::UnboundedSender<String>,
    cell: Semaphore,
    editor: Semaphore,
}

#[async_trait::async_trait]
impl Provider for NativeHost {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![
            json!({"type":"custom","name":"cell","description":"Execute raw Haskell","format":{"type":"text"}}),
            json!({"type":"function","name":"edit_context","description":"Edit context synchronously","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}}),
        ]
    }

    fn tool_scheduling(&self, name: &str) -> ToolScheduling {
        if name == "edit_context" {
            ToolScheduling::BeforeNextInference
        } else {
            ToolScheduling::Async
        }
    }

    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        unreachable!("regression uses whole-invocation completions")
    }

    async fn complete_call(
        &self,
        name: &str,
        input: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        self.started.send(name.into()).unwrap();
        if name == "cell" {
            assert_eq!(input, ToolInput::Custom("raw λ\ncell".into()));
            assert!(context.context.is_none());
            self.cell.acquire().await.unwrap().forget();
            return ProviderCompletion::unedited(Ok(json!({"real_cell_result":true})));
        }
        let snapshot = context.context.expect("editor has exact Store snapshot");
        assert_eq!(context.operation.as_ref(), Some(&snapshot.operation));
        self.editor.acquire().await.unwrap().forget();
        let mut document = snapshot.document;
        document.blocks.push(ContextBlock::Text {
            reference: None,
            role: ContextRole::Assistant,
            text: "committed editor note".into(),
            sources: vec![],
        });
        ProviderCompletion {
            finalization: crate::provider::FinalizationResponsibility::provider(),
            output: crate::turn::JobOutput::Completed(Ok(json!({"real_edit_result":true}))),
            full_success: true,
            context: ContextDisposition::Draft(ContextDraft {
                document,
                next_model: None,
                next_effort: None,
            }),
        }
    }
}

struct NativeScript {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    issued: mpsc::UnboundedSender<usize>,
    cell: Item,
    editor: Item,
    reasoning: [Item; 2],
}

#[async_trait::async_trait]
impl ResponsesTransport for NativeScript {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request);
            requests.len()
        };
        self.issued.send(index).unwrap();
        Ok(ResponsesTurn {
            response_id: format!("native-response-{index}"),
            items: match index {
                1 => vec![self.reasoning[0].clone(), self.cell.clone()],
                2 => vec![self.reasoning[1].clone(), self.editor.clone()],
                _ => vec![Item(
                    json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}),
                )],
            },
            usage: Usage::default(),
        })
    }
}

async fn issued(receiver: &mut mpsc::UnboundedReceiver<usize>, expected: usize) {
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), receiver.recv())
            .await
            .unwrap(),
        Some(expected)
    );
}

fn has_output(request: &ResponsesRequest, call: &str) -> bool {
    request.input.iter().any(|item| {
        item.0["call_id"] == call
            && matches!(
                item.0["type"].as_str(),
                Some("function_call_output" | "custom_tool_call_output")
            )
    })
}

#[tokio::test]
async fn native_async_custom_cell_survives_synchronous_context_commit() {
    let cell = Item(
        json!({"type":"custom_tool_call","id":"custom-native-item","namespace":"functions","call_id":"original-cell","name":"cell","async":true,"input":"raw λ\ncell","provider_extension":{"preserve":true}}),
    );
    // Host policy remains authoritative even if the native editor says async.
    let editor = Item(
        json!({"type":"function_call","id":"editor-native-item","call_id":"original-editor","name":"edit_context","async":true,"arguments":"{}"}),
    );
    let reasoning = [
        Item(
            json!({"type":"reasoning","id":"native-reasoning-cell","encrypted_content":"opaque-cell-reasoning","summary":[]}),
        ),
        Item(
            json!({"type":"reasoning","id":"native-reasoning-editor","encrypted_content":"opaque-editor-reasoning","summary":[]}),
        ),
    ];
    let requests = Arc::new(Mutex::new(vec![]));
    let (issued_tx, mut issued_rx) = mpsc::unbounded_channel();
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let host = Arc::new(NativeHost {
        started: started_tx,
        cell: Semaphore::new(0),
        editor: Semaphore::new(0),
    });
    let store = Arc::new(Store::memory().unwrap());
    let engine = Engine::<Offline, _, _>::with_transport(
        NativeScript {
            requests: requests.clone(),
            issued: issued_tx,
            cell: cell.clone(),
            editor: editor.clone(),
            reasoning: reasoning.clone(),
        },
        store.clone(),
        Arc::new(JobScheduler::new(2).unwrap()),
        host.clone(),
        EngineConfig {
            instructions: "shared instructions".into(),
            tools: vec![],
            model: "native-model".into(),
            effort: Effort::Low,
            session_id: "native-boundary".into(),
            agent: AgentPath("/root".into()),
        },
    );
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = mpsc::unbounded_channel::<Envelope>();
    let running = tokio::spawn(async move { engine.run(None, vec![], cancel, mail).await });
    issued(&mut issued_rx, 1).await;
    assert_eq!(started_rx.recv().await.as_deref(), Some("cell"));
    issued(&mut issued_rx, 2).await;
    assert_eq!(started_rx.recv().await.as_deref(), Some("edit_context"));
    assert!(
        tokio::time::timeout(Duration::from_millis(40), issued_rx.recv())
            .await
            .is_err()
    );
    let pending_claims = store.claims(&CallId("original-cell".into())).unwrap();
    assert_eq!(pending_claims.len(), 1);
    assert_eq!(pending_claims[0].state, crate::store::ClaimState::Pending);
    host.editor.add_permits(1);
    issued(&mut issued_rx, 3).await;
    {
        let requests = requests.lock().unwrap();
        assert!(requests[1].input.contains(&cell));
        assert!(!has_output(&requests[1], "original-cell"));
        assert!(requests[2].input.contains(&cell));
        assert!(requests[2].input.contains(&editor));
        assert!(!has_output(&requests[2], "original-cell"));
        assert!(has_output(&requests[2], "original-editor"));
        assert!(requests[2].input.iter().any(|item| {
            item.0["content"]
                .as_str()
                .is_some_and(|text| text.ends_with("committed editor note"))
        }));
        for native in &reasoning {
            assert!(requests[2].input.contains(native));
        }
        let wire = request_body_for_protocol(&requests[2], ResponsesProtocol::Standard).unwrap();
        let tools = wire["tools"].as_array().unwrap();
        assert_eq!(
            tools.iter().find(|tool| tool["name"] == "cell").unwrap()["async"],
            true
        );
        assert_eq!(
            tools
                .iter()
                .find(|tool| tool["name"] == "edit_context")
                .unwrap()["async"],
            false
        );
        assert_eq!(
            wire["input"],
            serde_json::to_value(&requests[2].input).unwrap()
        );
    }
    host.cell.add_permits(1);
    let completion = tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // Settlement can race the final response's processing. Both histories keep
    // the real terminal; another model request is needed only after a late wake.
    assert!((3..=4).contains(&requests.lock().unwrap().len()));
    assert!(completion.transcript.contains(&cell));
    for native in &reasoning {
        assert!(completion.transcript.contains(native));
    }
    let settled_claims = store.claims(&CallId("original-cell".into())).unwrap();
    assert!(!settled_claims.is_empty());
    for claim in settled_claims {
        assert_eq!(claim.operation, pending_claims[0].operation);
        assert_eq!(claim.state, crate::store::ClaimState::Settled);
    }
    let outputs = completion
        .transcript
        .iter()
        .filter(|item| {
            item.0["type"] == "custom_tool_call_output" && item.0["call_id"] == "original-cell"
        })
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(outputs[0].0["output"].as_str().unwrap())
            .unwrap(),
        json!({"real_cell_result":true})
    );
}
