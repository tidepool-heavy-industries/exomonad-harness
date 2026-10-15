//! Production submission boundaries, including cancellation and raw compaction facts.
use super::*;
use crate::store::{ProviderRequestPurpose, ProviderResponseState};
use crate::transport::{ResponsesProtocol, ResponsesRoute, sse::ResponseAssembly};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<String, TransportError> {
        unreachable!()
    }
}
struct NoEffects(AtomicUsize);
#[async_trait::async_trait]
impl Provider for NoEffects {
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, crate::provider::ProviderError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("compaction must not dispatch raw provider calls")
    }
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
}
struct Script {
    items: Vec<Item>,
    streaming: bool,
    stall: bool,
    cancel: Option<watch::Sender<bool>>,
    request: Arc<Mutex<Option<RequestId>>>,
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
        *self.request.lock().unwrap() = Some(request.request_id().clone());
        Ok(ResponsesTurn {
            response_id: "complete".into(),
            items: self.items.clone(),
            usage: Usage::default(),
        })
    }
    async fn create_streaming(
        &self,
        request: PreparedProviderRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        if !self.streaming {
            return self.create(request).await;
        }
        *self.request.lock().unwrap() = Some(request.request_id().clone());
        let mut assembly = ResponseAssembly::default();
        for (index, item) in self.items.iter().enumerate() {
            let event = assembly.accept(&json!({"type":"response.output_item.done", "output_index":index, "item":item.0}).to_string())?.unwrap();
            sink.send(event).await.unwrap();
        }
        if let Some(cancel) = &self.cancel {
            cancel.send(true).unwrap();
            std::future::pending::<()>().await;
        }
        if self.stall {
            std::future::pending::<()>().await;
        }
        assembly.accept(
            &json!({"type":"response.completed", "response":{"id":"streamed","output":self.items}})
                .to_string(),
        )?;
        assembly.finish()
    }
}
fn engine(
    store: Arc<Store>,
    script: Script,
    effects: Arc<NoEffects>,
) -> Engine<Offline, NoEffects, Script> {
    Engine::with_transport(
        script,
        store,
        Arc::new(JobScheduler::new(1).unwrap()),
        effects,
        EngineConfig {
            instructions: "private submission controls".into(),
            tools: vec![],
            model: "model".into(),
            effort: Effort::Low,
            session_id: "session".into(),
            agent: AgentPath("/test".into()),
        },
    )
}
fn raw_items() -> Vec<Item> {
    vec![
        Item(json!({"type":"configuration_update","reasoning_effort":"high"})),
        Item(json!({"type":"function_call","call_id":"raw-call","name":"never","arguments":"{}"})),
        Item(json!({"type":"function_call_output","call_id":"raw-call","output":"raw"})),
        Item(json!({"type":"additional_tools","tools":[]})),
        Item(json!({"type":"compaction_trigger"})),
        Item(
            json!({"type":"compaction","id":"compact","encrypted_content":"synthetic-test-content"}),
        ),
    ]
}
async fn completed_compaction(streaming: bool) {
    let store = Arc::new(Store::memory().unwrap());
    let source = RequestId("source".into());
    let child = RequestId("compactor".into());
    store.create_request(&source, None, "/test").unwrap();
    store
        .create_request(&child, Some(&source), "/test")
        .unwrap();
    let items = raw_items();
    let request = Arc::new(Mutex::new(None));
    let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
    let script = Script {
        items: items.clone(),
        streaming,
        stall: false,
        cancel: None,
        request,
    };
    let selected = script.prepare_submission().unwrap();
    let prepared = store
        .prepare_provider_request(
            &source,
            &child,
            ResponsesRequest {
                input: vec![],
                instructions: "fixed".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "model".into(),
                pinned_effort: Effort::Low,
                session_id: "session".into(),
                prompt_cache_comparison: None,
            },
            &[],
            None,
            selected,
            ProviderRequestPurpose::ServerCompaction,
        )
        .unwrap();
    let engine = engine(store.clone(), script, effects.clone());
    let (_cancel, receiver) = watch::channel(false);
    let turn = engine
        .execute_compaction_submission(prepared, receiver, &source)
        .await
        .unwrap();
    assert_eq!(turn.items, items);
    assert_eq!(store.items(&child).unwrap(), items);
    assert_eq!(
        store.provider_response_state(&child).unwrap(),
        Some(ProviderResponseState::Completed)
    );
    assert!(store.claims_on(&child).unwrap().is_empty());
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
    let c = store.lock().unwrap();
    let indexes = c
        .prepare(
            "SELECT output_index FROM provider_items WHERE request_id=?1 ORDER BY output_index",
        )
        .unwrap()
        .query_map([&child.0], |row| row.get::<_, u64>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(indexes, (0..items.len() as u64).collect::<Vec<_>>());
    assert_eq!(
        items
            .iter()
            .filter(|item| crate::compaction::server_keeps(item))
            .count(),
        1
    );
}
#[tokio::test]
async fn complete_compaction_retains_raw_facts_without_effect_authority() {
    completed_compaction(false).await;
}
#[tokio::test]
async fn streamed_compaction_retains_raw_facts_without_effect_authority() {
    completed_compaction(true).await;
}

#[tokio::test]
async fn cancelled_compaction_prefix_is_interrupted_after_reopen() {
    let path = std::env::temp_dir().join(format!(
        "prepared-compactor-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let store = Arc::new(Store::open(&path).unwrap());
    let source = RequestId("source".into());
    let child = RequestId("compactor".into());
    store.create_request(&source, None, "/test").unwrap();
    store
        .create_request(&child, Some(&source), "/test")
        .unwrap();
    let items = raw_items();
    let (cancel, receiver) = watch::channel(false);
    let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
    let script = Script {
        items: items.clone(),
        streaming: true,
        stall: false,
        cancel: Some(cancel),
        request: Arc::new(Mutex::new(None)),
    };
    let selected = script.prepare_submission().unwrap();
    let prepared = store
        .prepare_provider_request(
            &source,
            &child,
            ResponsesRequest {
                input: vec![],
                instructions: "fixed".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "model".into(),
                pinned_effort: Effort::Low,
                session_id: "session".into(),
                prompt_cache_comparison: None,
            },
            &[],
            None,
            selected,
            ProviderRequestPurpose::ServerCompaction,
        )
        .unwrap();
    let engine = engine(store.clone(), script, effects.clone());
    let error = engine
        .execute_compaction_submission(prepared, receiver, &source)
        .await
        .unwrap_err();
    assert!(
        matches!(error, EngineError::Cancelled { head_request: Some(ref head) } if head == &source)
    );
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
    drop(engine);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(reopened.items(&child).unwrap(), items);
    assert_eq!(
        reopened.provider_response_state(&child).unwrap(),
        Some(ProviderResponseState::Interrupted)
    );
    assert!(reopened.replay_turns(&source).unwrap().is_empty());
    assert!(reopened.claims_on(&child).unwrap().is_empty());
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn cancelled_inference_retains_buffered_prefix_after_reopen() {
    let path = std::env::temp_dir().join(format!(
        "prepared-inference-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let store = Arc::new(Store::open(&path).unwrap());
    let items = vec![Item(
        json!({"type":"reasoning","id":"prefix","summary":[],"encrypted_content":"synthetic-test-content"}),
    )];
    let (cancel, receiver) = watch::channel(false);
    let request = Arc::new(Mutex::new(None));
    let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
    let engine = engine(
        store.clone(),
        Script {
            items: items.clone(),
            streaming: true,
            stall: false,
            cancel: Some(cancel),
            request: request.clone(),
        },
        effects,
    );
    let (_incoming, incoming) = tokio::sync::mpsc::unbounded_channel();
    let error = engine
        .run(
            None,
            vec![Item(
                json!({"type":"message","role":"user","content":"start"}),
            )],
            receiver,
            incoming,
        )
        .await
        .unwrap_err();
    let head = request.lock().unwrap().clone().unwrap();
    assert!(
        matches!(error, EngineError::Cancelled { head_request: Some(ref actual) } if actual == &head)
    );
    drop(engine);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert!(reopened.items(&head).unwrap().ends_with(&items));
    assert_eq!(
        reopened.provider_response_state(&head).unwrap(),
        Some(ProviderResponseState::Interrupted)
    );
    assert!(reopened.replay_turns(&head).unwrap().is_empty());
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn compaction_live_retention_failure_settles_failed_response() {
    let store = Arc::new(Store::memory().unwrap());
    let source = RequestId("source".into());
    let child = RequestId("compactor".into());
    store.create_request(&source, None, "/test").unwrap();
    store
        .create_request(&child, Some(&source), "/test")
        .unwrap();
    let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
    let script = Script {
        items: raw_items(),
        streaming: true,
        stall: true,
        cancel: None,
        request: Arc::new(Mutex::new(None)),
    };
    let selection = script.prepare_submission().unwrap();
    let prepared = store
        .prepare_provider_request(
            &source,
            &child,
            ResponsesRequest {
                input: vec![],
                instructions: "fixed".into(),
                tools: vec![].into(),
                tools_allowed: None,
                model: "model".into(),
                pinned_effort: Effort::Low,
                session_id: "session".into(),
                prompt_cache_comparison: None,
            },
            &[],
            None,
            selection,
            ProviderRequestPurpose::ServerCompaction,
        )
        .unwrap();
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER refuse_first_provider_item BEFORE INSERT ON provider_items WHEN NEW.output_index=0 BEGIN SELECT RAISE(ABORT,'injected retention refusal'); END;").unwrap();
    let engine = engine(store.clone(), script, effects.clone());
    let (_cancel, receiver) = watch::channel(false);
    assert!(matches!(
        engine
            .execute_compaction_submission(prepared, receiver, &source)
            .await,
        Err(EngineError::Store(_))
    ));
    assert_eq!(
        store.provider_response_state(&child).unwrap(),
        Some(ProviderResponseState::Failed)
    );
    assert!(store.replay_turns(&source).unwrap().is_empty());
    assert!(store.claims_on(&child).unwrap().is_empty());
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn failed_auxiliary_issuance_does_not_pollute_embedded_frontier() {
    let store = Arc::new(Store::memory().unwrap());
    let identity = crate::embedding::HostIdentity {
        run: "run".into(),
        actor: AgentPath("/root".into()),
        incarnation: "issuer".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    let source = RequestId("source".into());
    store
        .write_embedded_request(&identity, &source, None, &[], StoredUsage::default())
        .unwrap();
    let before = store.embedded_round_frontier(&identity).unwrap();
    let effects = Arc::new(NoEffects(AtomicUsize::new(0)));
    let script = Script {
        items: vec![],
        streaming: false,
        stall: false,
        cancel: None,
        request: Arc::new(Mutex::new(None)),
    };
    let mut engine = engine(store.clone(), script, effects.clone()).with_origin(
        ConversationIdentity::Embedded {
            run: identity.run.clone(),
            actor: identity.actor.clone(),
            incarnation: identity.incarnation.clone(),
        },
    );
    engine.config.agent = identity.actor.clone();
    store.lock().unwrap().execute_batch("CREATE TEMP TRIGGER refuse_auxiliary_issuance BEFORE INSERT ON provider_requests BEGIN SELECT RAISE(ABORT,'injected issuance refusal'); END;").unwrap();
    assert!(
        engine
            .prepare_compaction_submission(
                &source,
                ResponsesRequest {
                    input: vec![],
                    instructions: "fixed".into(),
                    tools: vec![].into(),
                    tools_allowed: None,
                    model: "model".into(),
                    pinned_effort: Effort::Low,
                    session_id: "session".into(),
                    prompt_cache_comparison: None,
                },
                vec![],
                ProviderRequestPurpose::TextSummary
            )
            .await
            .is_err()
    );
    assert_eq!(store.embedded_round_frontier(&identity).unwrap(), before);
    assert!(store.replay_turns(&source).unwrap().is_empty());
    assert_eq!(effects.0.load(Ordering::SeqCst), 0);
    let c = store.lock().unwrap();
    let (children, issued): (u64, u64) = c.query_row("SELECT (SELECT COUNT(*) FROM requests WHERE parent_id=?1),(SELECT COUNT(*) FROM provider_requests)", [&source.0], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(children, 1);
    assert_eq!(issued, 0);
}

struct ComparisonCapture(Arc<Mutex<Vec<serde_json::Value>>>);
#[async_trait::async_trait]
impl ResponsesTransport for ComparisonCapture {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard))
    }
    async fn create(
        &self,
        request: PreparedProviderRequest,
    ) -> Result<ResponsesTurn, TransportError> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::from_slice(request.wire_body()).unwrap());
        Ok(ResponsesTurn {
            response_id: "completed-comparison".into(),
            items: vec![Item(
                json!({"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"done"}]}),
            )],
            usage: Usage::default(),
        })
    }
}

#[tokio::test]
async fn cache_comparison_engine_issuance_and_replay_preserve_opt_in() {
    let path =
        std::env::temp_dir().join(format!("cache-comparison-{}.sqlite", uuid::Uuid::new_v4()));
    let store = Arc::new(Store::open(&path).unwrap());
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let comparison = crate::transport::PromptCacheComparison {
        comparison_response_id: "baseline-completed".into(),
    };
    let engine = Engine::<Offline, NoEffects, _>::with_transport(
        ComparisonCapture(bodies.clone()),
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(NoEffects(AtomicUsize::new(0))),
        EngineConfig {
            instructions: "stable".into(),
            tools: vec![],
            model: "gpt-6-luna".into(),
            effort: Effort::Medium,
            session_id: "stable-session".into(),
            agent: AgentPath("/comparison".into()),
        },
    )
    .with_prompt_cache_comparison(comparison.clone());
    let (_cancel, receiver) = watch::channel(false);
    let (_incoming, incoming) = tokio::sync::mpsc::unbounded_channel();
    let completed = engine
        .run(
            None,
            vec![Item(
                json!({"type":"message","role":"user","content":"same prefix"}),
            )],
            receiver,
            incoming,
        )
        .await
        .unwrap();
    assert_eq!(
        bodies.lock().unwrap()[0]["prompt_cache_options"],
        json!({"comparison_response_id":"baseline-completed"})
    );
    drop(engine);
    drop(store);
    let restored = Store::open(&path).unwrap();
    let turns = restored.replay_turns(&completed.head_request).unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0].model_request.prompt_cache_comparison,
        Some(comparison)
    );
    let body = crate::transport::client::request_body(&turns[0].model_request).unwrap();
    assert_eq!(bodies.lock().unwrap()[0], body);
    drop(restored);
    std::fs::remove_file(path).unwrap();
}

struct MetadataThenFailure;
#[async_trait::async_trait]
impl ResponsesTransport for MetadataThenFailure {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard))
    }
    async fn create(&self, _: PreparedProviderRequest) -> Result<ResponsesTurn, TransportError> {
        unreachable!()
    }
    async fn create_streaming(
        &self,
        _: PreparedProviderRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        let summary=serde_json::from_value(json!({"version":1,"body_elapsed_ms":0,"event_kind_tail":[],"sse_event_kind_tail":[],"unknown_event_count":0,"unframed_line_bytes":0,"unframed_data_bytes":0,"unframed_event_name":false,"metadata_truncated":false,"end":"provider_terminal"})).unwrap();
        sink.send(StreamEvent::FailureSummary(summary))
            .await
            .unwrap();
        // Let the observer receive metadata before the native failure. A failing
        // diagnostic write must not replace the imminent provider outcome.
        tokio::task::yield_now().await;
        Err(TransportError::ResponseIncomplete {
            response_id: "primary-incomplete".into(),
            reason: Some(crate::transport::IncompleteReason::MaxOutputTokens),
        })
    }
}
#[tokio::test]
async fn diagnostic_write_failure_preserves_provider_cause_in_inference_and_compaction() {
    for compaction in [false, true] {
        let store = Arc::new(Store::memory().unwrap());
        // Explicit fault injection affects only diagnostic persistence.
        store.lock().unwrap().execute_batch("CREATE TRIGGER diagnostic_failure BEFORE INSERT ON events WHEN NEW.kind='transport_failure_summary' BEGIN SELECT RAISE(FAIL,'injected diagnostic write failure'); END;").unwrap();
        let runner = Engine::<Offline, _, _>::with_transport(
            MetadataThenFailure,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(NoEffects(AtomicUsize::new(0))),
            EngineConfig {
                instructions: "failure composition".into(),
                tools: vec![],
                model: "model".into(),
                effort: Effort::Low,
                session_id: "session".into(),
                agent: AgentPath("/test".into()),
            },
        );
        let (_cancel, cancellation) = watch::channel(false);
        if compaction {
            let source = RequestId("source".into());
            store.create_request(&source, None, "/test").unwrap();
            let prepared = runner
                .prepare_compaction_submission(
                    &source,
                    ResponsesRequest {
                        input: vec![],
                        instructions: "fixed".into(),
                        tools: vec![].into(),
                        tools_allowed: None,
                        model: "model".into(),
                        pinned_effort: Effort::Low,
                        session_id: "session".into(),
                        prompt_cache_comparison: None,
                    },
                    vec![],
                    ProviderRequestPurpose::ServerCompaction,
                )
                .await
                .unwrap();
            let error = runner
                .execute_compaction_submission(prepared, cancellation, &source)
                .await
                .unwrap_err();
            let EngineError::Secondary(failure) = error else {
                panic!("missing retained diagnostic failure: {error:?}")
            };
            assert_eq!(failure.stage(), RoundFailureStage::ProviderTailRecording);
            assert!(
                matches!(failure.primary(),EngineError::Transport(TransportError::ResponseIncomplete{response_id,..}) if response_id=="primary-incomplete")
            );
            assert!(matches!(failure.source_error(), EngineError::Store(_)));
        } else {
            let receipt = runner
                .run_with_finalize_receipt(
                    None,
                    vec![],
                    cancellation,
                    tokio::sync::mpsc::unbounded_channel::<Envelope>().1,
                    None,
                    false,
                )
                .await
                .unwrap();
            assert!(
                matches!(receipt.primary_error(),Some(EngineError::Transport(TransportError::ResponseIncomplete{response_id,..})) if response_id=="primary-incomplete")
            );
            assert_eq!(receipt.secondary_failures().len(), 1);
            assert_eq!(
                receipt.secondary_failures()[0].stage(),
                RoundFailureStage::ProviderTailRecording
            );
        }
    }
}
