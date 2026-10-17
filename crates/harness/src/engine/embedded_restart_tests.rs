//! Process-reopen cuts through the embedded Store frontier and real Engine.
use super::*;
use crate::transport::{ResponsesProtocol, ResponsesRoute};
use crate::{
    embedding::{BindingSuccessorAuthority, HostIdentity},
    store::EmbeddedRoundOutcome,
};
use std::{collections::VecDeque, sync::Mutex};

struct Offline;
impl Auth for Offline {
    fn admission(
        &self,
    ) -> Result<crate::transport::NativeTransportAdmission, crate::transport::TransportError> {
        crate::transport::NativeTransportAdmission::chatgpt_plan(std::path::PathBuf::from(
            "/offline/synthetic-credential",
        ))
    }
    fn access(&self) -> Result<String, TransportError> {
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
    ) -> Result<serde_json::Value, crate::provider::ProviderError> {
        panic!("recovery must never redispatch inherited tools")
    }
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
}
struct Script {
    inputs: Arc<Mutex<Vec<ResponsesRequest>>>,
    turns: Mutex<VecDeque<Result<ResponsesTurn, TransportError>>>,
}
#[async_trait::async_trait]
impl ResponsesTransport for Script {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard))
    }

    async fn create(
        &self,
        prepared: PreparedProviderRequest,
    ) -> Result<ResponsesTurn, TransportError> {
        self.inputs.lock().unwrap().push(prepared.request().clone());
        self.turns
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected provider request")
    }
}
fn final_turn() -> ResponsesTurn {
    ResponsesTurn {
        response_id: "durable-final".into(),
        items: vec![Item(
            json!({"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"done"}]}),
        )],
        usage: Usage::default(),
    }
}
fn identity() -> HostIdentity {
    HostIdentity {
        run: "restart-run".into(),
        actor: AgentPath("/root".into()),
        incarnation: "old".into(),
    }
}
fn path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("engine-restart-{}.sqlite", uuid::Uuid::new_v4()))
}
fn engine(
    store: Arc<Store>,
    identity: &HostIdentity,
    turns: Vec<Result<ResponsesTurn, TransportError>>,
) -> (
    Engine<Offline, NoTools, Script>,
    Arc<Mutex<Vec<ResponsesRequest>>>,
) {
    let inputs = Arc::new(Mutex::new(vec![]));
    let engine = Engine::with_transport(
        Script {
            inputs: inputs.clone(),
            turns: Mutex::new(turns.into()),
        },
        store,
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(NoTools),
        EngineConfig {
            instructions: "offline restart".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "restart".into(),
            agent: identity.actor.clone(),
        },
    )
    .with_origin(ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    });
    (engine, inputs)
}
pub(super) fn authorize_restart(
    store: &Store,
    identity: &HostIdentity,
    mode: crate::embedding::RecoveryMode,
) -> crate::server::CommandReceipt {
    store
        .initialize_context_target(
            &ConversationIdentity::Embedded {
                run: identity.run.clone(),
                actor: identity.actor.clone(),
                incarnation: identity.incarnation.clone(),
            },
            &crate::context::ContextProjectionTarget {
                model: "test".into(),
                route: ResponsesRoute::ChatGptPlan,
                protocol: ResponsesProtocol::Standard,
            },
        )
        .unwrap();
    store
        .recover_embedded_inference_after_host_loss(&identity.run)
        .unwrap();
    let crate::embedding::HostInferenceState::Blocked { block } =
        store.embedded_inference_state(identity).unwrap()
    else {
        panic!("restart requires an explicit durable block");
    };
    let command = crate::embedding::ClientOperationId(uuid::Uuid::new_v4());
    store
        .enqueue_embedded_command(
            command,
            &crate::server::HostCommand::Recover {
                target: identity.clone(),
                expected_block: block,
                mode,
            },
        )
        .unwrap();
    store
        .claim_embedded_command(&identity.run, command)
        .unwrap()
        .unwrap();
    store
        .authorize_embedded_recovery(identity, command)
        .unwrap()
}
pub(super) fn authorize_exact_restart(store: &Store, identity: &HostIdentity) {
    let receipt = authorize_restart(store, identity, crate::embedding::RecoveryMode::Exact);
    assert!(
        matches!(
            receipt.outcome,
            crate::server::CommandReceiptOutcome::RecoveryAuthorized { .. }
        ),
        "{receipt:?}"
    );
}
fn incoming() -> tokio::sync::mpsc::UnboundedReceiver<DurableMailboxWake> {
    tokio::sync::mpsc::unbounded_channel().1
}

#[tokio::test]
async fn embedded_restart_retains_validated_finalize_without_provider_or_job() {
    let path = path();
    let identity = identity();
    let schema = json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false});
    let final_call = Item(json!({
        "type":"function_call", "call_id":"final-restart", "name":"finalize",
        "arguments":{"result":{"answer":"ready"}}
    }));
    let head;
    let retained;
    {
        let store = Arc::new(Store::open(&path).unwrap());
        store.bind_embedded_actor(&identity, None).unwrap();
        let (engine, inputs) = engine(
            store.clone(),
            &identity,
            vec![Ok(ResponsesTurn {
                response_id: "validated-final-restart".into(),
                items: vec![final_call.clone()],
                usage: Usage::default(),
            })],
        );
        let (_cancel, cancel) = watch::channel(false);
        let completion = engine
            .run_with_reply_schema(
                None,
                vec![Item(
                    json!({"type":"message","role":"user","content":"finish"}),
                )],
                cancel,
                tokio::sync::mpsc::unbounded_channel().1,
                schema.clone(),
            )
            .await
            .unwrap();
        head = completion.head_request;
        assert_eq!(inputs.lock().unwrap().len(), 1);
        assert_eq!(completion.turn.items, vec![final_call.clone()]);
        assert!(store.claims_on(&head).unwrap().is_empty());
        retained = store.items(&head).unwrap();
        // Engine has sealed completion; the host has not published its frontier.
        assert_eq!(
            store
                .embedded_round_frontier(&identity)
                .unwrap()
                .pending_head,
            Some(head.clone())
        );
        assert!(
            store
                .agent(&identity.actor)
                .unwrap()
                .unwrap()
                .head_request
                .is_none()
        );
    }
    let store = Arc::new(Store::open(&path).unwrap());
    authorize_exact_restart(&store, &identity);
    let (resumed, inputs) = engine(store.clone(), &identity, vec![]);
    let (_cancel, cancel) = watch::channel(false);
    let completion = resumed
        .run_with_reply_schema(
            None,
            vec![],
            cancel,
            tokio::sync::mpsc::unbounded_channel().1,
            schema,
        )
        .await
        .unwrap();
    assert_eq!(completion.head_request, head);
    assert_eq!(completion.turn.items, vec![final_call]);
    assert_eq!(store.items(&head).unwrap(), retained);
    assert!(store.claims_on(&head).unwrap().is_empty());
    assert!(inputs.lock().unwrap().is_empty());
    assert!(
        store
            .settle_embedded_round(&identity, None, &head, EmbeddedRoundOutcome::Completed)
            .unwrap()
    );
    drop(resumed);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn embedded_restart_refuses_unvalidated_finalize_with_same_name() {
    let store = Arc::new(Store::memory().unwrap());
    let identity = identity();
    store.bind_embedded_actor(&identity, None).unwrap();
    let final_call = Item(json!({
        "type":"function_call", "call_id":"unvalidated-final", "name":"finalize",
        "arguments":{"result":{"answer":7}}
    }));
    let (first_engine, inputs) = engine(
        store.clone(),
        &identity,
        vec![Ok(ResponsesTurn {
            response_id: "unvalidated-final-restart".into(),
            items: vec![final_call.clone()],
            usage: Usage::default(),
        })],
    );
    let (_cancel, cancel) = watch::channel(false);
    let error = first_engine.run_with_reply_schema(
        None,
        vec![Item(json!({"type":"message","role":"user","content":"finish"}))],
        cancel,
        tokio::sync::mpsc::unbounded_channel().1,
        json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"],"additionalProperties":false}),
    ).await.unwrap_err();
    assert!(matches!(
        error,
        EngineError::Finalize(FinalizeError::ResultSchemaMismatch(_))
    ));
    assert_eq!(inputs.lock().unwrap().len(), 1);
    let head = store
        .embedded_round_frontier(&identity)
        .unwrap()
        .pending_head
        .unwrap();
    let retained = store.items(&head).unwrap();
    assert!(retained.contains(&final_call));
    assert!(store.claims_on(&head).unwrap().is_empty());
    let receipt = authorize_restart(&store, &identity, crate::embedding::RecoveryMode::Exact);
    assert!(
        matches!(
            receipt.outcome,
            crate::server::CommandReceiptOutcome::ContinuationRefused { .. }
        ),
        "{receipt:?}"
    );
    let (resumed, resumed_inputs) = engine(store.clone(), &identity, vec![]);
    let (_cancel, cancel) = watch::channel(false);
    let error = resumed
        .run_recovering_embedded(None, vec![], cancel, incoming())
        .await
        .unwrap_err();
    assert!(matches!(error, EngineError::InferenceBlocked { .. }));
    assert!(resumed_inputs.lock().unwrap().is_empty());
    assert_eq!(store.items(&head).unwrap(), retained);
}

#[tokio::test]
async fn embedded_seed_is_included_once_when_explicit_input_starts_inference() {
    let store = Arc::new(Store::memory().unwrap());
    let identity = identity();
    store.bind_embedded_actor(&identity, None).unwrap();
    let prompt = Item(json!({"type":"message","role":"user","content":"  seed\nλ\n"}));
    let seed = store
        .seed_embedded_context(&identity, "spawn", &prompt)
        .unwrap();
    assert!(store.model_request_outcomes(128).unwrap().is_empty());
    assert_eq!(
        store.usage_subtree(&seed).unwrap(),
        crate::store::Usage::default()
    );
    let usage = Usage {
        input_tokens: 17,
        output_tokens: 5,
        reported: true,
        ..Usage::default()
    };
    let mut turn = final_turn();
    turn.usage = usage.clone();
    let (engine, inputs) = engine(store.clone(), &identity, vec![Ok(turn)]);
    assert!(inputs.lock().unwrap().is_empty());
    let wake = Item(json!({"type":"message","role":"user","content":"start work"}));
    let (_cancel, cancel) = watch::channel(false);
    let completion = engine
        .run_embedded(Some(seed.clone()), vec![wake.clone()], cancel, incoming())
        .await
        .unwrap();
    let requests = inputs.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let outcomes = store.model_request_outcomes(128).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].request.id, completion.head_request);
    assert!(outcomes[0].failure.is_none());
    assert!(outcomes[0].interruption.is_none());
    assert_eq!(
        store.usage_subtree(&seed).unwrap(),
        crate::store::Usage {
            input_tokens: 17,
            output_tokens: 5,
            cost_micros: 0,
        }
    );
    assert_eq!(
        requests[0]
            .input
            .iter()
            .filter(|item| **item == prompt)
            .count(),
        1
    );
    assert_eq!(
        requests[0]
            .input
            .iter()
            .filter(|item| **item == wake)
            .count(),
        1
    );
    assert!(
        requests[0]
            .input
            .iter()
            .position(|item| *item == prompt)
            .unwrap()
            < requests[0]
                .input
                .iter()
                .position(|item| *item == wake)
                .unwrap()
    );
    assert_eq!(
        store
            .request(&completion.head_request)
            .unwrap()
            .unwrap()
            .parent,
        Some(seed)
    );
}

#[tokio::test]
async fn embedded_restart_recovers_admission_and_delivery_cuts_without_duplicate_parent() {
    for has_head in [false, true] {
        for delivered in [false, true] {
            let path = path();
            let identity = identity();
            let pending = RequestId("pending".into());
            let settled = has_head.then(|| RequestId("settled".into()));
            let input = Item(json!({"type":"message","role":"user","content":"retained input"}));
            {
                let store = Store::open(&path).unwrap();
                store.bind_embedded_actor(&identity, None).unwrap();
                if let Some(head) = &settled {
                    store
                        .write_embedded_request(&identity, head, None, &[], StoredUsage::default())
                        .unwrap();
                    store.set_effort(head, Effort::Low).unwrap();
                    assert!(
                        store
                            .settle_embedded_round(
                                &identity,
                                None,
                                head,
                                EmbeddedRoundOutcome::Completed
                            )
                            .unwrap()
                    );
                }
                store
                    .add_envelope("operator", "/root", "user", &input, None)
                    .unwrap();
                store
                    .write_embedded_request(
                        &identity,
                        &pending,
                        settled.as_ref(),
                        &[],
                        StoredUsage::default(),
                    )
                    .unwrap();
                if delivered {
                    store
                        .append_unread_envelopes(&identity.actor, &pending)
                        .unwrap();
                }
            }
            let store = Arc::new(Store::open(&path).unwrap());
            let (engine, inputs) = engine(store.clone(), &identity, vec![Ok(final_turn())]);
            let (_cancel, cancel) = watch::channel(false);
            authorize_exact_restart(&store, &identity);
            let completion = engine
                .run_recovering_embedded(settled.clone(), vec![], cancel, incoming())
                .await
                .unwrap();
            assert_eq!(inputs.lock().unwrap().len(), 1);
            assert_eq!(
                inputs.lock().unwrap()[0]
                    .input
                    .iter()
                    .filter(|item| **item == input)
                    .count(),
                1
            );
            assert_eq!(
                store
                    .request(&completion.head_request)
                    .unwrap()
                    .unwrap()
                    .parent,
                Some(pending)
            );
            assert_eq!(
                store.agent(&identity.actor).unwrap().unwrap().head_request,
                settled
            );
            assert!(store.unread("/root").unwrap().is_empty());
            assert!(
                store
                    .settle_embedded_round(
                        &identity,
                        settled.as_ref(),
                        &completion.head_request,
                        EmbeddedRoundOutcome::Completed
                    )
                    .unwrap()
            );
            assert!(
                store
                    .embedded_round_frontier(&identity)
                    .unwrap()
                    .pending_head
                    .is_none()
            );
            drop(engine);
            drop(store);
            std::fs::remove_file(path).unwrap();
        }
    }
}

#[tokio::test]
async fn embedded_restart_settles_durable_final_without_provider_then_waits_for_new_input() {
    let path = path();
    let identity = identity();
    let head;
    {
        let store = Arc::new(Store::open(&path).unwrap());
        store.bind_embedded_actor(&identity, None).unwrap();
        let (engine, inputs) = engine(store.clone(), &identity, vec![Ok(final_turn())]);
        let (_cancel, cancel) = watch::channel(false);
        head = engine
            .run_embedded(
                None,
                vec![Item(
                    json!({"type":"message","role":"user","content":"first"}),
                )],
                cancel,
                incoming(),
            )
            .await
            .unwrap()
            .head_request;
        assert_eq!(inputs.lock().unwrap().len(), 1);
        assert!(
            store
                .agent(&identity.actor)
                .unwrap()
                .unwrap()
                .head_request
                .is_none()
        );
    }
    let store = Arc::new(Store::open(&path).unwrap());
    let (resumed, inputs) = engine(store.clone(), &identity, vec![]);
    let (_cancel, cancel) = watch::channel(false);
    authorize_exact_restart(&store, &identity);
    let completion = resumed
        .run_recovering_embedded(None, vec![], cancel.clone(), incoming())
        .await
        .unwrap();
    assert_eq!(completion.head_request, head);
    assert!(inputs.lock().unwrap().is_empty());
    assert!(
        store
            .settle_embedded_round(&identity, None, &head, EmbeddedRoundOutcome::Completed)
            .unwrap()
    );
    assert!(
        store
            .embedded_round_frontier(&identity)
            .unwrap()
            .pending_head
            .is_none()
    );
    let (next, inputs) = engine(store.clone(), &identity, vec![Ok(final_turn())]);
    let completion = next
        .run_embedded(
            Some(head.clone()),
            vec![Item(
                json!({"type":"message","role":"user","content":"explicit followup"}),
            )],
            cancel,
            incoming(),
        )
        .await
        .unwrap();
    assert_eq!(inputs.lock().unwrap().len(), 1);
    assert_eq!(
        store
            .request(&completion.head_request)
            .unwrap()
            .unwrap()
            .parent,
        Some(head)
    );
    drop(next);
    drop(resumed);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn embedded_restart_refuses_orphan_streamed_call_before_provider_or_claim() {
    let store = Arc::new(Store::memory().unwrap());
    let identity = identity();
    store.bind_embedded_actor(&identity, None).unwrap();
    let head = RequestId("orphan".into());
    let call = CallId("unclaimed".into());
    store
        .write_embedded_request(
            &identity,
            &head,
            None,
            &[Item(
                json!({"type":"function_call","call_id":call.0,"name":"effect","arguments":"{}"}),
            )],
            StoredUsage::default(),
        )
        .unwrap();
    let (engine, inputs) = engine(store.clone(), &identity, vec![]);
    let (_cancel, cancel) = watch::channel(false);
    assert!(
        matches!(engine.run_recovering_embedded(None,vec![],cancel,incoming()).await,Err(EngineError::InferenceBlocked{block}) if block.request==head)
    );
    assert!(inputs.lock().unwrap().is_empty());
    assert!(store.claims(&call).unwrap().is_empty());
    assert_eq!(
        store
            .embedded_round_frontier(&identity)
            .unwrap()
            .pending_head,
        Some(head)
    );
}

#[tokio::test]
async fn embedded_restart_recovers_atomic_completion_cut_without_inventing_missing_items() {
    let store = Arc::new(Store::memory().unwrap());
    let identity = identity();
    store.bind_embedded_actor(&identity, None).unwrap();
    // Refuse final publication and retain the actual atomic Store rollback cut.
    store.lock().unwrap().execute_batch("CREATE TRIGGER cut_final BEFORE INSERT ON request_items WHEN (SELECT json FROM items WHERE hash=NEW.item_hash) LIKE '%final_answer%' BEGIN SELECT RAISE(ABORT,'cut'); END;").unwrap();
    let (first, _) = engine(store.clone(), &identity, vec![Ok(final_turn())]);
    let (_cancel, cancel) = watch::channel(false);
    assert!(
        first
            .run_embedded(None, vec![], cancel.clone(), incoming())
            .await
            .is_err()
    );
    let head = store
        .embedded_round_frontier(&identity)
        .unwrap()
        .pending_head
        .unwrap();
    assert!(
        !store
            .events(Some(&head))
            .unwrap()
            .iter()
            .any(|event| event.kind == "model_turn")
    );
    store
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER cut_final")
        .unwrap();
    let (resumed, inputs) = engine(store.clone(), &identity, vec![Ok(final_turn())]);
    authorize_exact_restart(&store, &identity);
    let completed = resumed
        .run_recovering_embedded(None, vec![], cancel, incoming())
        .await
        .unwrap();
    assert_eq!(inputs.lock().unwrap().len(), 1);
    assert!(
        !inputs.lock().unwrap()[0]
            .input
            .iter()
            .any(|item| item.0["phase"] == "final_answer")
    );
    assert_eq!(
        store
            .request(&completed.head_request)
            .unwrap()
            .unwrap()
            .parent,
        Some(head)
    );
}

struct Successor;
impl BindingSuccessorAuthority for Successor {
    fn validate_successor(&self, _: &HostIdentity, _: &HostIdentity) -> Result<bool, String> {
        Ok(true)
    }
}

#[tokio::test]
async fn embedded_restart_preserves_committed_model_for_retained_opaque_history() {
    use crate::context::{ContextCommit, ContextDraft};

    let path = path();
    let old = identity();
    let mut next = old.clone();
    next.incarnation = "successor".into();
    let source = RequestId("model-switch".into());
    let opaque_head = RequestId("model-b-output".into());
    let opaque = Item(json!({"type":"reasoning","encrypted_content":"model-b-only"}));
    let (snapshot, draft, output, receipt, old_origin) = {
        let store = Store::open(&path).unwrap();
        store.bind_embedded_actor(&old, None).unwrap();
        store
            .write_embedded_request(
                &old,
                &source,
                None,
                &[
                    Item(json!({"type":"message","role":"user","content":"switch models"})),
                    Item(json!({"type":"custom_tool_call","call_id":"switch","name":"haskell_sync","input":"modifyContext"})),
                ],
                StoredUsage::default(),
            )
            .unwrap();
        store.set_effort(&source, Effort::Low).unwrap();
        let operation = store.claim(&CallId("switch".into()), &source).unwrap();
        store
            .initialize_context_model(&operation.origin, "test")
            .unwrap();
        let snapshot = store.begin_context(&operation, &source).unwrap();
        let draft = ContextDraft {
            document: snapshot.document.clone(),
            next_model: Some("model-b".into()),
            next_effort: None,
        };
        let output = crate::turn::JobOutput::Completed(Ok(json!("switched")));
        let receipt = store
            .commit_context(ContextCommit {
                snapshot: &snapshot,
                draft: &draft,
                output: &output,
                pending: &[],
            })
            .unwrap();
        assert_eq!(receipt.generation, 1);
        assert!(
            store
                .settle_embedded_round(&old, None, &receipt.head, EmbeddedRoundOutcome::Completed)
                .unwrap()
        );
        store
            .write_embedded_request(
                &old,
                &opaque_head,
                Some(&receipt.head),
                std::slice::from_ref(&opaque),
                StoredUsage::default(),
            )
            .unwrap();
        store.set_effort(&opaque_head, Effort::Low).unwrap();
        store
            .record_replay_turn(
                &opaque_head,
                &ResponsesRequest {
                    input: vec![],
                    instructions: "instructions".into(),
                    tools: vec![].into(),
                    tools_allowed: None,
                    model: "model-b".into(),
                    pinned_effort: Effort::Low,
                    session_id: "embedded-restart".into(),
                    prompt_cache_comparison: None,
                },
                &ResponsesTurn {
                    response_id: "model-b-output".into(),
                    items: vec![opaque.clone()],
                    usage: Usage::default(),
                },
            )
            .unwrap();
        assert!(
            store
                .settle_embedded_round(
                    &old,
                    Some(&receipt.head),
                    &opaque_head,
                    EmbeddedRoundOutcome::Completed
                )
                .unwrap()
        );
        let old_origin = operation.origin;
        store
            .transfer_embedded_binding(&old, &next, &Successor)
            .unwrap();
        (snapshot, draft, output, receipt, old_origin)
    };
    let store = Arc::new(Store::open(&path).unwrap());
    let next_origin = ConversationIdentity::Embedded {
        run: next.run.clone(),
        actor: next.actor.clone(),
        incarnation: next.incarnation.clone(),
    };
    assert!(store.context_model(&old_origin).is_err());
    assert!(store.initialize_context_model(&old_origin, "test").is_err());
    assert!(
        store
            .context_request_state(&opaque_head, &old_origin)
            .is_err()
    );
    assert!(
        store
            .commit_context(ContextCommit {
                snapshot: &snapshot,
                draft: &draft,
                output: &output,
                pending: &[],
            })
            .is_err()
    );
    assert_eq!(
        store
            .transfer_embedded_binding(&old, &next, &Successor)
            .unwrap(),
        crate::embedding::BindingSuccessorCommit::AlreadyInstalled
    );
    let (engine, inputs) = engine(store.clone(), &next, vec![Ok(final_turn())]);
    let (_cancel, cancel) = watch::channel(false);
    assert!(matches!(
        engine
            .run_recovering_embedded(
                Some(opaque_head.clone()),
                vec![],
                cancel.clone(),
                incoming()
            )
            .await,
        Err(EngineError::InferenceBlocked { .. })
    ));
    let original_items = store.items(&opaque_head).unwrap();
    let migration = authorize_restart(
        &store,
        &next,
        crate::embedding::RecoveryMode::VisibleHistory,
    );
    let crate::server::CommandReceiptOutcome::RecoveryAuthorized { projection, .. } =
        migration.outcome
    else {
        panic!("{migration:?}");
    };
    assert_eq!(projection.omitted.len(), 1);
    let completion = engine
        .run_recovering_embedded(Some(opaque_head.clone()), vec![], cancel, incoming())
        .await
        .unwrap();
    let requests = inputs.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].input.contains(&opaque));
    assert_eq!(store.items(&opaque_head).unwrap(), original_items);
    assert_eq!(requests[0].model, "model-b");
    assert!(matches!(
        store.context_request_state_for_transport(
            &opaque_head,
            &next_origin,
            (ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard),
        ),
        Err(StoreError::Context(crate::context::ContextError::Replay(
            crate::context::ContextReplayRejection::IncompatibleTransport,
        )))
    ));
    let state = store
        .context_request_state_for_transport(
            &completion.head_request,
            &next_origin,
            (ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard),
        )
        .unwrap();
    assert_eq!(state.model.as_deref(), Some("model-b"));
    assert!(state.generation > receipt.generation);
    drop(requests);
    drop(engine);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn embedded_restart_replays_compacted_settled_claim_with_original_incarnation_once() {
    let path = path();
    let old = identity();
    let source = RequestId("tool-source".into());
    let compact = RequestId("compacted".into());
    let call = CallId("effect".into());
    let item =
        Item(json!({"type":"function_call","call_id":call.0,"name":"effect","arguments":"{}"}));
    let operation;
    {
        let store = Store::open(&path).unwrap();
        store.bind_embedded_actor(&old, None).unwrap();
        store
            .write_embedded_request(
                &old,
                &source,
                None,
                std::slice::from_ref(&item),
                StoredUsage::default(),
            )
            .unwrap();
        store.set_effort(&source, Effort::Low).unwrap();
        operation = store.claim(&call, &source).unwrap();
        store
            .write_compaction_request_with_claims(
                &compact,
                &source,
                "/root",
                std::slice::from_ref(&item),
                std::slice::from_ref(&operation),
                Some(&old),
            )
            .unwrap();
        store.set_effort(&compact, Effort::Low).unwrap();
        store
            .write_job_output(
                &operation,
                ToolKind::Function,
                &crate::turn::JobOutput::Completed(Ok(json!(42))),
            )
            .unwrap();
    }
    let store = Arc::new(Store::open(&path).unwrap());
    let mut next = old.clone();
    next.incarnation = "successor".into();
    store
        .transfer_embedded_binding(&old, &next, &Successor)
        .unwrap();
    let (engine, inputs) = engine(store.clone(), &next, vec![Ok(final_turn())]);
    let (_cancel, cancel) = watch::channel(false);
    authorize_exact_restart(&store, &next);
    let completion = engine
        .run_recovering_embedded(None, vec![], cancel, incoming())
        .await
        .unwrap();
    assert_eq!(inputs.lock().unwrap().len(), 1);
    assert_eq!(
        inputs.lock().unwrap()[0]
            .input
            .iter()
            .filter(|item| item.0["type"] == "function_call_output" && item.0["call_id"] == call.0)
            .count(),
        1
    );
    assert!(
        store
            .claims_for_operation(&operation)
            .unwrap()
            .iter()
            .all(|claim| claim.operation.origin == operation.origin)
    );
    assert_eq!(
        store
            .request(&completion.head_request)
            .unwrap()
            .unwrap()
            .parent,
        Some(compact)
    );
    drop(engine);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

struct CompletedTool {
    calls: std::sync::atomic::AtomicUsize,
    acknowledged: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl Provider for CompletedTool {
    async fn call(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<serde_json::Value, crate::provider::ProviderError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(json!("retained large result ".repeat(1000)))
    }
    fn tools(&self) -> Vec<serde_json::Value> {
        vec![]
    }
    async fn output_committed(
        &self,
        _: &OperationId,
    ) -> Result<(), crate::provider::ProviderError> {
        self.acknowledged.notify_one();
        Ok(())
    }
}
struct CutAfterCompaction {
    provider: Arc<CompletedTool>,
    rounds: std::sync::atomic::AtomicUsize,
    compacted: std::sync::atomic::AtomicUsize,
    parked: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl ResponsesTransport for Arc<CutAfterCompaction> {
    fn response_contract(&self) -> Result<(ResponsesRoute, ResponsesProtocol), TransportError> {
        Ok((ResponsesRoute::ChatGptPlan, ResponsesProtocol::Standard))
    }

    async fn create(
        &self,
        prepared: PreparedProviderRequest,
    ) -> Result<ResponsesTurn, TransportError> {
        let request = prepared.request();
        use std::sync::atomic::Ordering;
        if request.tools_allowed.as_ref().is_some_and(Vec::is_empty) {
            assert!(
                request
                    .input
                    .iter()
                    .any(|item| item.0["type"] == "function_call_output")
            );
            self.compacted.fetch_add(1, Ordering::SeqCst);
            return Ok(ResponsesTurn {
                response_id: "summary".into(),
                items: vec![Item(json!({
                    "type":"message","role":"assistant","content":[{"type":"output_text","text":"The effect completed once and returned its large result."}]
                }))],
                usage: Usage::default(),
            });
        }
        if self.rounds.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(ResponsesTurn {
                response_id: "tool-round".into(),
                items: vec![Item(json!({
                    "type":"function_call","call_id":"completed-effect","name":"effect","arguments":"{}"
                }))],
                usage: Usage {
                    input_tokens: 1000,
                    ..Usage::default()
                },
            });
        }
        self.parked.notify_one();
        std::future::pending().await
    }
    async fn create_streaming(
        &self,
        request: PreparedProviderRequest,
        sink: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<ResponsesTurn, TransportError> {
        let turn = self.create(request).await?;
        for (index, item) in turn.items.iter().enumerate() {
            sink.send(
                crate::transport::sse::ResponseAssembly::default()
                    .accept(
                        &serde_json::json!({
                            "type": "response.output_item.done",
                            "output_index": index as u64,
                            "item": (item.clone()).0,
                        })
                        .to_string(),
                    )
                    .expect("authored indexed done event is valid")
                    .expect("done event publishes a completed item"),
            )
            .await
            .unwrap();
        }
        self.provider.acknowledged.notified().await;
        Ok(turn)
    }
}

#[tokio::test]
async fn embedded_restart_recovers_actual_internal_compaction_without_reexecuting_completed_tool() {
    use std::sync::atomic::Ordering;
    let path = path();
    let old = identity();
    let pending;
    let provider = Arc::new(CompletedTool {
        calls: std::sync::atomic::AtomicUsize::new(0),
        acknowledged: tokio::sync::Notify::new(),
    });
    {
        let store = Arc::new(Store::open(&path).unwrap());
        store.bind_embedded_actor(&old, None).unwrap();
        let transport = Arc::new(CutAfterCompaction {
            provider: provider.clone(),
            rounds: std::sync::atomic::AtomicUsize::new(0),
            compacted: std::sync::atomic::AtomicUsize::new(0),
            parked: tokio::sync::Notify::new(),
        });
        let config = EngineConfig {
            instructions: "offline".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Low,
            session_id: "before-loss".into(),
            agent: old.actor.clone(),
        };
        let runtime = Engine::<Offline, _, _>::with_transport(
            transport.clone(),
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            provider.clone(),
            config,
        )
        .with_origin(ConversationIdentity::Embedded {
            run: old.run.clone(),
            actor: old.actor.clone(),
            incarnation: old.incarnation.clone(),
        })
        .with_plain_text_compaction(NonZeroU64::new(100).unwrap());
        let (_cancel, cancel) = watch::channel(false);
        let mut task = tokio::spawn(async move {
            runtime
                .run_embedded(
                    None,
                    vec![Item(
                        json!({"type":"message","role":"user","content":"keep this input"}),
                    )],
                    cancel,
                    incoming(),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::select! {
                biased;
                result = &mut task => panic!("Engine ended before internal compaction: {result:?}"),
                () = transport.parked.notified() => (),
            }
        })
        .await
        .expect("Engine parks after internal compaction");
        pending = store
            .embedded_round_frontier(&old)
            .unwrap()
            .pending_head
            .unwrap();
        assert!(store.is_compaction_boundary(&pending).unwrap());
        assert_eq!(transport.compacted.load(Ordering::SeqCst), 1);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert!(
            store
                .agent(&old.actor)
                .unwrap()
                .unwrap()
                .head_request
                .is_none()
        );
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }
    let store = Arc::new(Store::open(&path).unwrap());
    let mut next = old.clone();
    next.incarnation = "after-loss".into();
    store
        .transfer_embedded_binding(&old, &next, &Successor)
        .unwrap();
    let (engine, inputs) = engine(store.clone(), &next, vec![Ok(final_turn())]);
    let (_cancel, cancel) = watch::channel(false);
    authorize_exact_restart(&store, &next);
    let completion = engine
        .run_recovering_embedded(None, vec![], cancel, incoming())
        .await
        .unwrap();
    assert_eq!(inputs.lock().unwrap().len(), 1);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(
        inputs.lock().unwrap()[0]
            .input
            .iter()
            .any(|item| item.0.to_string().contains("effect completed once"))
    );
    assert_eq!(
        store
            .request(&completion.head_request)
            .unwrap()
            .unwrap()
            .parent,
        Some(pending)
    );
    assert!(
        store
            .settle_embedded_round(
                &next,
                None,
                &completion.head_request,
                EmbeddedRoundOutcome::Completed
            )
            .unwrap()
    );
    assert!(
        store
            .embedded_round_frontier(&next)
            .unwrap()
            .pending_head
            .is_none()
    );
    drop(engine);
    drop(store);
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn embedded_restart_retains_direct_followup_after_durable_final_response() {
    let store = Arc::new(Store::memory().unwrap());
    let identity = identity();
    store.bind_embedded_actor(&identity, None).unwrap();
    let first_input = Item(json!({"type":"message","role":"user","content":"first direct input"}));
    let next_input =
        Item(json!({"type":"message","role":"user","content":"explicit direct followup"}));
    let (first, _) = engine(store.clone(), &identity, vec![Ok(final_turn())]);
    let (_cancel, cancel) = watch::channel(false);
    let pending = first
        .run_embedded(None, vec![first_input.clone()], cancel.clone(), incoming())
        .await
        .unwrap()
        .head_request;
    let (resumed, inputs) = engine(store.clone(), &identity, vec![Ok(final_turn())]);
    authorize_exact_restart(&store, &identity);
    let completion = resumed
        .run_recovering_embedded(None, vec![next_input.clone()], cancel, incoming())
        .await
        .unwrap();
    assert_eq!(inputs.lock().unwrap().len(), 1);
    let inputs = inputs.lock().unwrap();
    assert_eq!(
        inputs[0]
            .input
            .iter()
            .filter(|item| **item == first_input)
            .count(),
        1
    );
    assert_eq!(
        inputs[0]
            .input
            .iter()
            .filter(|item| **item == next_input)
            .count(),
        1
    );
    assert_eq!(
        store
            .request(&completion.head_request)
            .unwrap()
            .unwrap()
            .parent,
        Some(pending)
    );
}
