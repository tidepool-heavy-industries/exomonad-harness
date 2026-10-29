use super::*;
use crate::engine::{EngineConfig, ResponsesTransport};
use crate::item::FunctionArgumentError;
use crate::model::Effort;
use crate::provider::ProviderCompletion;
use crate::transport::sse::StreamEvent;
use crate::transport::{
    Auth, PreparedProviderRequest, ResponsesProtocol, ResponsesRequest, ResponsesRoute,
    ResponsesTurn, TransportError, Usage,
};
use serde_json::json;
use std::{collections::VecDeque, num::NonZeroU64, sync::Mutex, time::Duration};
use tokio::sync::{Semaphore, mpsc, watch};

struct Offline;
impl Auth for Offline {
    fn admission(&self) -> Result<crate::transport::NativeTransportAdmission, TransportError> {
        crate::transport::NativeTransportAdmission::chatgpt_plan("/offline".into())
    }
    fn access(&self) -> Result<String, TransportError> {
        panic!("offline fixture")
    }
}
struct Guard;
impl AdmissionGuard for Guard {}
struct Host {
    identity: HostIdentity,
    surface: Arc<ToolSurface>,
}
#[async_trait]
impl HostActor for Host {
    fn tool_encoding_profile(&self) -> Option<ToolEncodingProfile> {
        Some(ToolEncodingProfile::FunctionResults)
    }
    fn identity(&self) -> &HostIdentity {
        &self.identity
    }
    async fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError> {
        Ok(Box::new(Guard))
    }
    async fn tool_surface(&self) -> Result<Arc<ToolSurface>, EmbeddedError> {
        Ok(self.surface.clone())
    }
    async fn wake(&self, _: i64) -> Result<(), String> {
        panic!("no inbox")
    }
    async fn control(&self, _: HostControl) -> Result<Value, HostControlError> {
        panic!("no control")
    }
}
struct Dispatcher {
    entered: mpsc::UnboundedSender<(String, ToolInput)>,
    finish: Semaphore,
}
#[async_trait]
impl Provider for Dispatcher {
    fn tools(&self) -> Vec<Value> {
        vec![]
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        panic!("typed dispatch required")
    }
    async fn complete_call(
        &self,
        name: &str,
        input: ToolInput,
        _: CallContext,
    ) -> ProviderCompletion {
        self.entered.send((name.into(), input.clone())).unwrap();
        let result =
            match (name, input) {
                ("work", ToolInput::Function(args))
                    if args.get("value").and_then(Value::as_u64).is_some()
                        && args.as_object().unwrap().len() == 1 =>
                {
                    if args["value"] != 3 {
                        self.finish.acquire().await.unwrap().forget();
                    }
                    Ok(args)
                }
                _ => Err(ProviderError::Tool("owned argument or name refusal".into())
                    .into_tool_failure()),
            };
        ProviderCompletion::unedited(result)
    }
}
struct Script {
    turns: Mutex<VecDeque<ResponsesTurn>>,
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
    release_response: Semaphore,
}
#[async_trait]
impl ResponsesTransport for Arc<Script> {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard))
    }
    async fn create(
        &self,
        request: PreparedProviderRequest,
    ) -> Result<ResponsesTurn, TransportError> {
        self.requests
            .lock()
            .unwrap()
            .push(request.request().clone());
        Ok(self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected inference"))
    }
    async fn create_streaming(
        &self,
        request: PreparedProviderRequest,
        sink: mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        let turn = self.create(request).await?;
        let mut assembly = crate::transport::sse::ResponseAssembly::default();
        for (index, item) in turn.items.iter().enumerate() {
            let event = assembly
                .accept(
                    &json!({"type":"response.output_item.done","output_index":index,"item":item.0})
                        .to_string(),
                )
                .unwrap()
                .unwrap();
            sink.send(event).await.unwrap();
        }
        if turn.response_id == "first" {
            self.release_response.acquire().await.unwrap().forget();
        }
        Ok(turn)
    }
}
fn call(id: &str, name: &str, arguments: Value) -> Item {
    Item(
        json!({"type":"function_call","namespace":"functions","call_id":id,"name":name,"arguments":arguments}),
    )
}
fn host(dispatcher: Arc<Dispatcher>) -> Arc<Host> {
    Arc::new(Host {
        identity: HostIdentity { run: "portable".into(), actor: AgentPath("/root".into()), incarnation: "one".into() },
        surface: Arc::new(ToolSurface::new("portable-v1".into(), vec![json!({"type":"function","name":"work","strict":true,"parameters":{"type":"object","properties":{"value":{"type":"integer"}},"required":["value"],"additionalProperties":false}})], dispatcher).unwrap()),
    })
}
fn config() -> EngineConfig {
    EngineConfig {
        instructions: "fixture".into(),
        tools: vec![],
        model: "offline".into(),
        effort: Effort::Low,
        session_id: "portable".into(),
        agent: AgentPath("/root".into()),
    }
}

#[tokio::test]
async fn streamed_function_results_isolate_refusals_and_wait_for_actual_results() {
    let (entered, mut entries) = mpsc::unbounded_channel();
    let dispatcher = Arc::new(Dispatcher {
        entered,
        finish: Semaphore::new(0),
    });
    let host = host(dispatcher.clone());
    let store = Arc::new(Store::memory().unwrap());
    let conversation = Conversation::attach(store.clone(), host, None)
        .await
        .unwrap();
    let requests = Arc::new(Mutex::new(vec![]));
    let script = Arc::new(Script { requests: requests.clone(), release_response: Semaphore::new(0), turns: Mutex::new(vec![
        ResponsesTurn { response_id: "first".into(), items: vec![
            call("one", "work", json!({"value":1})),
            call("bad-json", "work", json!("{")),
            call("unknown", "wait_agent", json!({})),
            call("bad-fields", "work", json!({"value":2,"unexpected":true})),
            call("two", "work", json!({"value":2})),
        ], usage: Usage::default() },
        ResponsesTurn { response_id: "second".into(), items: vec![call("three", "work", json!({"value":3}))], usage: Usage::default() },
        ResponsesTurn { response_id: "final".into(), items: vec![Item(json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}))], usage: Usage::default() },
    ].into()) });
    let engine = conversation
        .engine::<Offline, _>(
            script.clone(),
            Arc::new(crate::turn::JobScheduler::new(8).unwrap()),
            config(),
            NonZeroU64::new(100_000).unwrap(),
        )
        .unwrap();
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = mpsc::unbounded_channel();
    let running =
        tokio::spawn(async move { engine.run_embedded(None, vec![], cancel, mail).await });
    let mut observed = Vec::new();
    for _ in 0..5 {
        observed.push(
            tokio::time::timeout(Duration::from_secs(3), entries.recv())
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(observed.iter().any(|(_, input)| matches!(input, ToolInput::InvalidFunction { arguments: Some(Value::String(raw)), reason: FunctionArgumentError::InvalidJson } if raw == "{")));
    // Every call reached its owning dispatcher before the response finished.
    assert_eq!(requests.lock().unwrap().len(), 1);
    script.release_response.add_permits(1);
    tokio::task::yield_now().await;
    assert_eq!(requests.lock().unwrap().len(), 1);
    dispatcher.finish.add_permits(2);
    let completed = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(requests.lock().unwrap().len(), 3);
    for id in ["one", "bad-json", "unknown", "bad-fields", "two", "three"] {
        assert_eq!(
            completed
                .transcript
                .iter()
                .filter(|item| item.0["type"] == "function_call_output" && item.0["call_id"] == id)
                .count(),
            1
        );
    }
    let requests = requests.lock().unwrap();
    for tool in requests[0].tools.iter() {
        assert_eq!(tool["type"], "function");
        assert!(tool.get("async").is_none());
        assert!(tool["parameters"]["properties"].get("delivery").is_none());
    }
    assert_eq!(
        requests[1]
            .input
            .iter()
            .filter(|item| item.0["type"] == "function_call_output")
            .count(),
        5
    );
    conversation
        .provider()
        .validate_resume(&completed.head_request)
        .unwrap();
}

#[tokio::test]
async fn legacy_surface_refuses_resume_without_mutating_history() {
    let (entered, _) = mpsc::unbounded_channel();
    let host = host(Arc::new(Dispatcher {
        entered,
        finish: Semaphore::new(0),
    }));
    let store = Arc::new(Store::memory().unwrap());
    let conversation = Conversation::attach(store.clone(), host, None)
        .await
        .unwrap();
    let head = conversation.seed_context("seed", "hello").await.unwrap();
    store
        .record_event(
            Some(&head),
            "tool_surface",
            &json!({"version":"old","encoding":null}),
        )
        .unwrap();
    let before = store.events(Some(&head)).unwrap().len();
    let error = conversation.provider().validate_resume(&head).unwrap_err();
    assert!(error.to_string().contains("legacy tool contract"));
    assert_eq!(store.events(Some(&head)).unwrap().len(), before);
}

#[tokio::test]
async fn completed_response_usage_survives_later_dispatch_failure() {
    let (entered, _) = mpsc::unbounded_channel();
    let host = host(Arc::new(Dispatcher {
        entered,
        finish: Semaphore::new(0),
    }));
    let store = Arc::new(Store::memory().unwrap());
    let conversation = Conversation::attach(store.clone(), host, None)
        .await
        .unwrap();
    let script = Arc::new(Script {
        requests: Arc::new(Mutex::new(vec![])), release_response: Semaphore::new(0),
        turns: Mutex::new(vec![ResponsesTurn {
            response_id: "kind-conflict".into(),
            items: vec![Item(json!({"type":"custom_tool_call","call_id":"conflict","name":"work","input":"raw"}))],
            usage: Usage { input_tokens: Some(17), output_tokens: Some(4), ..Usage::default() },
        }].into()),
    });
    let engine = conversation
        .engine::<Offline, _>(
            script,
            Arc::new(crate::turn::JobScheduler::new(2).unwrap()),
            config(),
            NonZeroU64::new(100_000).unwrap(),
        )
        .unwrap();
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = mpsc::unbounded_channel();
    assert!(
        engine
            .run_embedded(None, vec![], cancel, mail)
            .await
            .is_err()
    );
    let events = store.events(None).unwrap();
    let usage = events
        .iter()
        .find(|event| event.kind == "responses_usage")
        .unwrap();
    let payload: Value = serde_json::from_str(&usage.payload).unwrap();
    assert_eq!(payload["input_tokens"], 17);
    assert_eq!(payload["output_tokens"], 4);
    let retained = store
        .usage_subtree(usage.request.as_ref().unwrap())
        .unwrap();
    assert_eq!(retained.input_tokens, 17);
    assert_eq!(retained.output_tokens, 4);
}
