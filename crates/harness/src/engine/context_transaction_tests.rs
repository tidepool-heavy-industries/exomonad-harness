use super::*;
use crate::turn::JobOutput;
use crate::{
    context::{
        ContextChange, ContextDraft, ContextPatch, ContextProblemCode, ContextTransaction,
        ContextTransactionDraft, ContextTransactionStatus, inspect_context,
    },
    provider::{CallContext, ContextDisposition, ProviderCompletion, ProviderError},
    transport::{ResponsesProtocol, ResponsesRoute},
};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::mpsc;

struct Offline;
impl Auth for Offline {
    fn access(&self) -> Result<String, TransportError> {
        panic!("offline context transaction test");
    }
}
#[derive(Clone, Copy)]
enum Mode {
    Committed,
    Rejected,
    Discarded,
    AbortFailed,
    RejectedWithChoices,
    ChoiceOnly,
    Unconfirmed,
    ForgedProjection,
}
struct Host {
    store: Arc<Store>,
    mode: Mode,
    native: bool,
    retained: AtomicUsize,
    last_operation: Mutex<Option<OperationId>>,
    handle: Mutex<Option<(OperationId, ContextTransaction)>>,
    committed: AtomicUsize,
    aborted: AtomicUsize,
}
impl Host {
    fn authored_output(&self) -> JobOutput {
        let body = if self.native {
            json!({"nativeResult":{"version":1,"semantic":"λ".repeat(50_000)}})
        } else {
            json!({"authored":"success"})
        };
        if matches!(self.mode, Mode::Unconfirmed) {
            JobOutput::CancellationUnconfirmed(
                crate::provider::ToolFailure::from("cleanup unconfirmed").with_result_receipt(body),
            )
        } else {
            JobOutput::Completed(Ok(body))
        }
    }

    fn assert_retained_before_cleanup(&self, operation: &OperationId) {
        if self.native {
            assert!(self.retained.load(Ordering::SeqCst) > 0);
            let recorded = self
                .store
                .replay_tool_output_operation(operation)
                .unwrap()
                .unwrap();
            assert_eq!(
                recorded.output_format,
                crate::provider::OutputFormat::NativeReceiptV1
            );
            assert!(
                self.store
                    .model_output_projection(operation)
                    .unwrap()
                    .is_some()
            );
        }
    }
}
#[async_trait::async_trait]
impl Provider for Host {
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
    fn tool_scheduling(&self, _: &str) -> ToolScheduling {
        ToolScheduling::BeforeNextInference
    }
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        unreachable!()
    }
    async fn complete_call(
        &self,
        _: &str,
        _: ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        let snapshot = context.context.unwrap();
        *self.last_operation.lock().unwrap() = Some(snapshot.operation.clone());
        if matches!(self.mode, Mode::ChoiceOnly) {
            return ProviderCompletion {
                output: self.authored_output(),
                output_format: if self.native {
                    crate::provider::OutputFormat::NativeReceiptV1
                } else {
                    crate::provider::OutputFormat::LegacyOpaque
                },
                full_success: true,
                context: ContextDisposition::Draft(ContextDraft {
                    document: snapshot.document,
                    next_model: Some("model-b".into()),
                    next_effort: Some(Effort::High),
                }),
                activation_terminal: None,
            };
        }
        let target = inspect_context(&snapshot.document).text_versions[0].clone();
        let patch = ContextPatch {
            changes: vec![ContextChange::ReplaceContextText {
                reference: target.reference,
                selector: target.selector,
                expected: if matches!(
                    self.mode,
                    Mode::Rejected | Mode::AbortFailed | Mode::RejectedWithChoices
                ) {
                    crate::context::context_digest(b"stale text")
                } else {
                    target.digest
                },
                text: "replacement".into(),
            }],
        };
        *self.handle.lock().unwrap() = Some((
            snapshot.operation.clone(),
            self.store.context_transaction(&snapshot, &patch).unwrap(),
        ));
        let draft = ContextTransactionDraft {
            patch,
            next_model: matches!(self.mode, Mode::RejectedWithChoices).then(|| "model-b".into()),
            next_effort: matches!(self.mode, Mode::RejectedWithChoices).then_some(Effort::High),
        };
        ProviderCompletion {
            output: self.authored_output(),
            output_format: if self.native {
                crate::provider::OutputFormat::NativeReceiptV1
            } else {
                crate::provider::OutputFormat::LegacyOpaque
            },
            full_success: !matches!(self.mode, Mode::Discarded | Mode::Unconfirmed),
            context: if matches!(self.mode, Mode::Discarded | Mode::Unconfirmed) {
                ContextDisposition::Discarded(draft)
            } else {
                ContextDisposition::Transaction(draft)
            },
            activation_terminal: None,
        }
    }
    fn model_visible_item(
        &self,
        request: &RequestId,
        item: &Item,
    ) -> Result<Option<crate::provider::ModelVisibleItemCandidate>, ProviderError> {
        if self.native {
            let Some(call) = item.0["call_id"].as_str() else {
                return Ok(None);
            };
            if let Some(operation) = self
                .store
                .recorded_operation_for_request(request, &crate::model::CallId(call.into()))
                .unwrap()
            {
                if self
                    .store
                    .replay_output_operation(&operation)
                    .unwrap()
                    .as_ref()
                    == Some(item)
                {
                    let mut candidate = self.store.model_output_projection(&operation).unwrap();
                    if matches!(self.mode, Mode::ForgedProjection) {
                        candidate.as_mut().unwrap().item.0["output"] =
                            json!("not the issued model presentation");
                    }
                    return Ok(candidate);
                }
            }
        }
        Ok(None)
    }
    async fn output_retained(&self, operation: &OperationId) -> Result<(), ProviderError> {
        if self.native {
            let recorded = self
                .store
                .replay_tool_output_operation(operation)
                .unwrap()
                .unwrap();
            assert_eq!(
                recorded.output_format,
                crate::provider::OutputFormat::NativeReceiptV1
            );
            self.store
                .publish_model_output_projection(
                    operation,
                    json!({"authored":"compact native presentation"}),
                )
                .unwrap();
            self.retained.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn output_committed(&self, operation: &OperationId) -> Result<(), ProviderError> {
        self.assert_retained_before_cleanup(operation);
        self.committed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn output_aborted(&self, operation: &OperationId) -> Result<(), ProviderError> {
        self.assert_retained_before_cleanup(operation);
        self.aborted.fetch_add(1, Ordering::SeqCst);
        if matches!(self.mode, Mode::AbortFailed) {
            return Err(ProviderError::Tool(
                "unpublished continuation cleanup failed".into(),
            ));
        }
        Ok(())
    }
}
struct Script {
    requests: Arc<Mutex<Vec<ResponsesRequest>>>,
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
            requests.len()
        };
        Ok(ResponsesTurn {
            response_id: format!("response-{index}"),
            usage: Usage::default(),
            items: if index == 1 {
                vec![Item(
                    json!({"type":"function_call","call_id":"editor","name":"cell","arguments":"{}"}),
                )]
            } else {
                vec![Item(
                    json!({"type":"message","role":"assistant","phase":"final_answer","content":"done"}),
                )]
            },
        })
    }
}
async fn exercise(mode: Mode) {
    exercise_formatted(mode, false).await;
}

async fn exercise_formatted(mode: Mode, native: bool) {
    exercise_formatted_with_profile(mode, native, native).await;
}

async fn exercise_formatted_with_profile(mode: Mode, native: bool, bounded: bool) {
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let directory = Directory(
        std::env::temp_dir().join(format!("context-native-results-{}", uuid::Uuid::new_v4())),
    );
    std::fs::create_dir(&directory.0).unwrap();
    let path = directory.0.join("context-native-results.sqlite");
    let store = Arc::new(Store::open(&path).unwrap());
    let host = Arc::new(Host {
        store: store.clone(),
        mode,
        native,
        retained: AtomicUsize::new(0),
        last_operation: Mutex::new(None),
        handle: Mutex::new(None),
        committed: AtomicUsize::new(0),
        aborted: AtomicUsize::new(0),
    });
    let requests = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::<Offline, _, _>::with_transport(
        Script {
            requests: requests.clone(),
        },
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        host.clone(),
        EngineConfig {
            instructions: "context transaction".into(),
            tools: vec![],
            model: "model-a".into(),
            effort: Effort::Low,
            session_id: "context-transaction".into(),
            agent: AgentPath("/root".into()),
        },
    );
    let engine = if bounded {
        engine.bounded_invocation()
    } else {
        engine
    };
    let (_cancel, cancel) = watch::channel(false);
    let (_mail, mail) = mpsc::unbounded_channel::<Envelope>();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        engine.run(None, vec![Item::user_text("original")], cancel, mail),
    )
    .await
    .unwrap();
    if matches!(mode, Mode::ForgedProjection) {
        assert!(
            result.is_err(),
            "an unissued presentation must stop inference"
        );
        let error = result.unwrap_err();
        assert!(
            matches!(
                error,
                EngineError::Store(StoreError::Context(crate::context::ContextError::Replay(
                    crate::context::ContextReplayRejection::InvalidMembership
                )))
            ),
            "unexpected projection failure: {error:?}"
        );
        assert_eq!(requests.lock().unwrap().len(), 1);
        let operation = host.last_operation.lock().unwrap().clone().unwrap();
        let canonical = store
            .replay_tool_output_operation(&operation)
            .unwrap()
            .unwrap();
        assert_eq!(
            canonical.item,
            Item::tool_output_with_format(
                &operation.call,
                crate::item::ToolKind::Function,
                &host.authored_output(),
                crate::provider::OutputFormat::NativeReceiptV1,
            )
        );
        let issued = store.model_output_projection(&operation).unwrap().unwrap();
        assert_eq!(
            issued.item.0["output"],
            json!({"authored":"compact native presentation"}).to_string()
        );
        assert_eq!(
            Store::open(&path)
                .unwrap()
                .replay_tool_output_operation(&operation)
                .unwrap(),
            Some(canonical)
        );
        return;
    }
    if matches!(mode, Mode::Unconfirmed) {
        fn unconfirmed(error: &EngineError) -> bool {
            match error {
                EngineError::UnconfirmedCancellation { detail, .. } => {
                    detail == "cleanup unconfirmed"
                }
                EngineError::Secondary(error) => {
                    unconfirmed(error.primary()) || unconfirmed(error.source_error())
                }
                EngineError::BlockedAction(error) => unconfirmed(error.primary()),
                EngineError::Cleanup { primary, .. } => unconfirmed(primary),
                EngineError::CleanupFailures(errors) => errors.iter().any(unconfirmed),
                _ => false,
            }
        }
        let error = result.unwrap_err();
        assert!(unconfirmed(&error), "unexpected failure: {error:?}");
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert_eq!(host.committed.load(Ordering::SeqCst), 0);
        assert_eq!(host.aborted.load(Ordering::SeqCst), 0);
        assert!(host.retained.load(Ordering::SeqCst) >= 1);
        let (operation, handle) = host.handle.lock().unwrap().clone().unwrap();
        assert!(matches!(
            store
                .context_transaction_status(&operation.origin, &handle)
                .unwrap(),
            ContextTransactionStatus::Unknown(_)
        ));
        assert!(
            store
                .context_transaction_head(&operation)
                .unwrap()
                .is_none()
        );
        let canonical = store
            .replay_tool_output_operation(&operation)
            .unwrap()
            .unwrap();
        assert_eq!(
            canonical.output_format,
            crate::provider::OutputFormat::NativeReceiptV1
        );
        assert_eq!(
            canonical.item,
            Item::tool_output_with_format(
                &operation.call,
                crate::item::ToolKind::Function,
                &host.authored_output(),
                canonical.output_format
            )
        );
        assert!(matches!(
            &canonical.terminal,
            crate::store::TerminalOutcome::CancellationUnconfirmed(_)
        ));
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.replay_tool_output_operation(&operation).unwrap(),
            Some(canonical)
        );
        return;
    }
    if matches!(mode, Mode::AbortFailed) {
        fn cleanup_error(error: &EngineError) -> bool {
            match error {
                EngineError::ProviderCall(ProviderError::Tool(message)) => {
                    message.message() == "unpublished continuation cleanup failed"
                }
                EngineError::Secondary(error) => {
                    cleanup_error(error.primary()) || cleanup_error(error.source_error())
                }
                EngineError::BlockedAction(error) => cleanup_error(error.primary()),
                _ => false,
            }
        }
        assert!(cleanup_error(&result.unwrap_err()));
        assert_eq!(
            requests.lock().unwrap().len(),
            1,
            "unconfirmed context cleanup must prevent next inference"
        );
        assert_eq!(host.committed.load(Ordering::SeqCst), 0);
        assert!(host.aborted.load(Ordering::SeqCst) >= 1);
        let (operation, handle) = host.handle.lock().unwrap().clone().unwrap();
        assert!(matches!(
            store
                .context_transaction_status(&operation.origin, &handle)
                .unwrap(),
            ContextTransactionStatus::Rejected(_)
        ));
        assert!(store.has_completed_output(&operation).unwrap());
        return;
    }
    result.unwrap();
    if matches!(mode, Mode::ChoiceOnly) {
        let operation = host.last_operation.lock().unwrap().clone().unwrap();
        assert!(host.handle.lock().unwrap().is_none());
        let receipt = store.context_receipt(&operation).unwrap().unwrap();
        assert_eq!(receipt.model.as_deref(), Some("model-b"));
        assert!(receipt.changed);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].model, "model-a");
        assert_eq!(requests[1].model, "model-b");
        assert_eq!(
            crate::transport::client::request_body(&requests[0]).unwrap()["reasoning"]["effort"],
            "low"
        );
        assert_eq!(
            crate::transport::client::request_body(&requests[1]).unwrap()["reasoning"]["effort"],
            "high"
        );
        assert!(
            requests[1]
                .input
                .iter()
                .any(|item| item == &Item::user_text("original"))
        );
        assert_eq!(host.committed.load(Ordering::SeqCst), 1);
        assert_eq!(host.aborted.load(Ordering::SeqCst), 0);
        return;
    }
    let (operation, handle) = host.handle.lock().unwrap().clone().unwrap();
    let status = store
        .context_transaction_status(&operation.origin, &handle)
        .unwrap();
    match mode {
        Mode::AbortFailed | Mode::ChoiceOnly | Mode::Unconfirmed | Mode::ForgedProjection => {
            unreachable!("special result handled above")
        }
        Mode::Committed => {
            assert!(matches!(status, ContextTransactionStatus::Committed(_)));
            assert_eq!(host.committed.load(Ordering::SeqCst), 1);
            assert_eq!(host.aborted.load(Ordering::SeqCst), 0);
        }
        Mode::Rejected | Mode::Discarded | Mode::RejectedWithChoices => {
            assert!(
                matches!(status, ContextTransactionStatus::Rejected(problems) if problems.iter().any(|problem| problem.code == if matches!(mode, Mode::Rejected | Mode::RejectedWithChoices) { ContextProblemCode::Stale } else { ContextProblemCode::Ineligible }))
            );
            assert_eq!(host.committed.load(Ordering::SeqCst), 0);
            assert_eq!(host.aborted.load(Ordering::SeqCst), 1);
        }
    }
    let requests = requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        2,
        "normal rejection must continue inference"
    );
    assert_eq!(
        requests[1].model, "model-a",
        "patch publication must not reset or publish rejected model choices"
    );
    assert_eq!(
        crate::transport::client::request_body(&requests[1]).unwrap()["reasoning"]["effort"],
        "low",
        "patch publication must preserve the existing effort"
    );
    if native {
        let canonical = store
            .replay_tool_output_operation(&operation)
            .unwrap()
            .unwrap();
        assert_eq!(
            canonical.output_format,
            crate::provider::OutputFormat::NativeReceiptV1
        );
        assert_eq!(
            canonical.item,
            Item::tool_output_with_format(
                &operation.call,
                crate::item::ToolKind::Function,
                &host.authored_output(),
                canonical.output_format
            )
        );
        assert!(!requests[1].input.contains(&canonical.item));
        let projection = store
            .model_output_projection(&operation)
            .unwrap()
            .unwrap()
            .item;
        assert!(requests[1].input.contains(&projection));
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.replay_tool_output_operation(&operation).unwrap(),
            Some(canonical)
        );
        assert_eq!(
            reopened
                .model_output_projection(&operation)
                .unwrap()
                .unwrap()
                .item,
            projection
        );
        assert!(matches!(
            reopened
                .context_transaction_status(&operation.origin, &handle)
                .unwrap(),
            ContextTransactionStatus::Committed(_) | ContextTransactionStatus::Rejected(_)
        ));
    } else {
        assert!(requests[1].input.iter().any(|item| item
            == &Item::tool_output(
                &operation.call,
                crate::item::ToolKind::Function,
                &JobOutput::Completed(Ok(json!({"authored":"success"})))
            )));
    }
    let expected = if matches!(mode, Mode::Committed) {
        "replacement"
    } else {
        "original"
    };
    assert!(requests[1].input.iter().any(|item| {
        item.0["content"].as_str() == Some(expected)
            || item.0["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["text"] == expected))
    }));
}
#[tokio::test]
async fn committed_transaction_acknowledges_and_continues_inference() {
    exercise(Mode::Committed).await;
}
#[tokio::test]
async fn rejected_transaction_aborts_unpublished_context_and_continues_inference() {
    exercise(Mode::Rejected).await;
}
#[tokio::test]
async fn discarded_transaction_retains_candidate_aborts_and_continues_inference() {
    exercise(Mode::Discarded).await;
}

#[tokio::test]
async fn rejected_transaction_cleanup_failure_blocks_next_inference() {
    exercise(Mode::AbortFailed).await;
}

#[tokio::test]
async fn rejected_patch_does_not_publish_explicit_model_or_effort_choices() {
    exercise(Mode::RejectedWithChoices).await;
}

#[tokio::test]
async fn choice_only_legacy_draft_publishes_independently_of_transactions() {
    exercise(Mode::ChoiceOnly).await;
}

#[tokio::test]
async fn native_results_survive_context_commit_rejection_and_discard_before_cleanup() {
    for mode in [
        Mode::Committed,
        Mode::Rejected,
        Mode::Discarded,
        Mode::ChoiceOnly,
    ] {
        exercise_formatted(mode, true).await;
    }
}

#[tokio::test]
async fn unbounded_native_results_use_the_issued_model_projection() {
    for mode in [
        Mode::Committed,
        Mode::Rejected,
        Mode::Discarded,
        Mode::ChoiceOnly,
    ] {
        exercise_formatted_with_profile(mode, true, false).await;
    }
}

#[tokio::test]
async fn unbounded_native_model_projection_refuses_an_unissued_replacement() {
    exercise_formatted_with_profile(Mode::ForgedProjection, true, false).await;
}

#[tokio::test]
async fn unconfirmed_context_cleanup_retains_native_receipt_without_confirming_context() {
    exercise_formatted(Mode::Unconfirmed, true).await;
}

struct EmbeddedHost {
    identity: crate::embedding::HostIdentity,
    dispatcher: Arc<Host>,
}
struct EmbeddedAdmission;
impl crate::embedding::AdmissionGuard for EmbeddedAdmission {}
#[async_trait::async_trait]
impl crate::embedding::HostActor for EmbeddedHost {
    fn identity(&self) -> &crate::embedding::HostIdentity {
        &self.identity
    }
    async fn admit(
        &self,
    ) -> Result<Box<dyn crate::embedding::AdmissionGuard>, crate::embedding::EmbeddedError> {
        Ok(Box::new(EmbeddedAdmission))
    }
    async fn tool_surface(
        &self,
    ) -> Result<Arc<crate::embedding::ToolSurface>, crate::embedding::EmbeddedError> {
        Ok(Arc::new(crate::embedding::ToolSurface::new(
            "native-projection".into(),
            vec![json!({"type":"function","name":"cell","strict":true,
                "parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}})],
            self.dispatcher.clone(),
        )?))
    }
    async fn wake(&self, _: i64) -> Result<(), String> {
        Ok(())
    }
    async fn control(
        &self,
        _: crate::embedding::HostControl,
    ) -> Result<serde_json::Value, crate::embedding::HostControlError> {
        unreachable!("projection does not control the host")
    }
    async fn output_retained(&self, operation: &OperationId) -> Result<(), String> {
        self.dispatcher
            .output_retained(operation)
            .await
            .map_err(|e| e.to_string())
    }
    async fn output_committed(&self, operation: &OperationId) -> Result<(), String> {
        self.dispatcher
            .output_committed(operation)
            .await
            .map_err(|e| e.to_string())
    }
    async fn output_aborted(&self, operation: &OperationId) -> Result<(), String> {
        self.dispatcher
            .output_aborted(operation)
            .await
            .map_err(|e| e.to_string())
    }
}
struct EmbeddedScript {
    script: Script,
    bodies: Arc<Mutex<Vec<serde_json::Value>>>,
}
#[async_trait::async_trait]
impl ResponsesTransport for EmbeddedScript {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        self.script.response_contract()
    }
    async fn create(
        &self,
        request: PreparedProviderRequest,
    ) -> Result<ResponsesTurn, TransportError> {
        self.bodies
            .lock()
            .unwrap()
            .push(serde_json::from_slice(request.wire_body()).unwrap());
        self.script.create(request).await
    }
}

#[tokio::test]
async fn embedded_round_projects_native_receipt_in_prepared_input_and_wire_body() {
    exercise_embedded_projection(Mode::Committed).await;
}

#[tokio::test]
async fn embedded_round_refuses_unissued_projection_before_next_provider_request() {
    exercise_embedded_projection(Mode::ForgedProjection).await;
}

async fn exercise_embedded_projection(mode: Mode) {
    let store = Arc::new(Store::memory().unwrap());
    let dispatcher = Arc::new(Host {
        store: store.clone(),
        mode,
        native: true,
        retained: AtomicUsize::new(0),
        last_operation: Mutex::new(None),
        handle: Mutex::new(None),
        committed: AtomicUsize::new(0),
        aborted: AtomicUsize::new(0),
    });
    let host = Arc::new(EmbeddedHost {
        identity: crate::embedding::HostIdentity {
            run: "native-projection".into(),
            actor: AgentPath("/root".into()),
            incarnation: "original".into(),
        },
        dispatcher: dispatcher.clone(),
    });
    let conversation = crate::embedding::Conversation::attach(store.clone(), host.clone(), None)
        .await
        .unwrap();
    store
        .admit_embedded_input(
            &host.identity,
            "first",
            "human",
            &Item::user_text("original"),
        )
        .unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let engine = conversation
        .engine::<Offline, _>(
            EmbeddedScript {
                script: Script {
                    requests: requests.clone(),
                },
                bodies: bodies.clone(),
            },
            Arc::new(JobScheduler::new(1).unwrap()),
            EngineConfig {
                instructions: "embedded native projection".into(),
                tools: vec![],
                model: "model-a".into(),
                effort: Effort::Low,
                session_id: "embedded-native-projection".into(),
                agent: host.identity.actor.clone(),
            },
            std::num::NonZeroU64::new(1_000_000).unwrap(),
        )
        .unwrap();
    assert!(!engine.bounded_invocation);
    let mut driver = engine.into_embedded_rounds().unwrap();
    let EmbeddedRoundReadiness::Selected(selection) = driver.next_round().unwrap() else {
        panic!("admitted input selects the embedded round");
    };
    let (_cancel, cancel) = watch::channel(false);
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        driver.run_round(selection, cancel, mpsc::unbounded_channel().1),
    )
    .await
    .unwrap();
    let operation = dispatcher.last_operation.lock().unwrap().clone().unwrap();
    let canonical = store
        .replay_tool_output_operation(&operation)
        .unwrap()
        .unwrap();
    assert_eq!(
        canonical.output_format,
        crate::provider::OutputFormat::NativeReceiptV1
    );
    assert_eq!(
        canonical.item,
        Item::tool_output_with_format(
            &operation.call,
            crate::item::ToolKind::Function,
            &dispatcher.authored_output(),
            canonical.output_format
        )
    );
    let projection = store
        .model_output_projection(&operation)
        .unwrap()
        .unwrap()
        .item;
    if matches!(mode, Mode::ForgedProjection) {
        let failure = result.unwrap_err();
        assert!(
            matches!(
                failure.primary_error(),
                Some(EngineError::Store(StoreError::Context(
                    crate::context::ContextError::Replay(
                        crate::context::ContextReplayRejection::InvalidMembership
                    )
                )))
            ),
            "unexpected failure: {failure:?}"
        );
        assert_eq!(requests.lock().unwrap().len(), 1);
        assert_eq!(bodies.lock().unwrap().len(), 1);
    } else {
        result.unwrap();
        let requests = requests.lock().unwrap();
        let bodies = bodies.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(bodies.len(), 2);
        assert!(requests[1].input.contains(&projection));
        assert!(!requests[1].input.contains(&canonical.item));
        let wire_output = bodies[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "function_call_output")
            .unwrap();
        assert_eq!(wire_output, &projection.0);
        assert_ne!(wire_output["output"], canonical.item.0["output"]);
        assert!(wire_output.get("output_format").is_none());
    }
    assert_eq!(
        store.replay_tool_output_operation(&operation).unwrap(),
        Some(canonical)
    );
    assert_eq!(
        store
            .model_output_projection(&operation)
            .unwrap()
            .unwrap()
            .item,
        projection
    );
}
