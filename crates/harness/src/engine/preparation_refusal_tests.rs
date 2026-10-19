#[tokio::test]
async fn embedded_preparation_refusal_is_durable_without_dispatch_and_waits_for_recovery() {
    use crate::embedding::{HostIdentity, HostInferenceState, InferenceBlockReason};
    use crate::engine::embedded_round::{EmbeddedRoundReadiness, EmbeddedRoundWait};
    use crate::transport::{ResponsesRoute, UnsupportedSemantics};
    let path = std::env::temp_dir().join(format!(
        "harness-preparation-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    let store = Arc::new(Store::open(&path).unwrap());
    let identity = HostIdentity {
        run: "preparation".into(),
        actor: AgentPath("/root".into()),
        incarnation: "first".into(),
    };
    store.bind_embedded_actor(&identity, None).unwrap();
    store
        .admit_embedded_input(&identity, "initial", "operator", &Item::user_text("go"))
        .unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let engine = Engine::<FakeAuth, Echo, _>::with_transport(
        Replay {
            requests: requests.clone(),
            turns: Mutex::new(Default::default()),
        },
        store.clone(),
        Arc::new(JobScheduler::new(1).unwrap()),
        Arc::new(Echo),
        EngineConfig {
            instructions: "instruction".into(),
            tools: vec![],
            model: "test".into(),
            effort: Effort::Max,
            session_id: "preparation".into(),
            agent: identity.actor.clone(),
        },
    )
    .with_origin(ConversationIdentity::Embedded {
        run: identity.run.clone(),
        actor: identity.actor.clone(),
        incarnation: identity.incarnation.clone(),
    });
    let mut rounds = engine.into_embedded_rounds().unwrap();
    let EmbeddedRoundReadiness::Selected(selection) = rounds.next_round().unwrap() else {
        panic!("input selects round")
    };
    let (_cancel, cancellation) = watch::channel(false);
    let (_incoming, incoming) = tokio::sync::mpsc::unbounded_channel();
    let settlement = rounds
        .run_round(selection, cancellation, incoming)
        .await
        .unwrap();
    assert!(matches!(
        settlement.issue(),
        Some(EngineError::RequestPreparationRefused {
            route: ResponsesRoute::ChatGptPlan,
            semantics: UnsupportedSemantics::ReasoningEffort,
            ..
        })
    ));
    assert_eq!(settlement.assessment().execution(), RoundExecution::Failed);
    assert_eq!(settlement.assessment().cleanup(), RoundCleanup::Confirmed);
    assert!(
        requests.lock().unwrap().is_empty(),
        "normalization precedes transport dispatch"
    );
    let HostInferenceState::Blocked { block } = store.embedded_inference_state(&identity).unwrap()
    else {
        panic!("durable block")
    };
    assert_eq!(
        block.reason,
        InferenceBlockReason::RequestPreparationRefused {
            route: ResponsesRoute::ChatGptPlan,
            semantics: UnsupportedSemantics::ReasoningEffort
        }
    );
    assert_eq!(
        store
            .embedded_round_frontier(&identity)
            .unwrap()
            .pending_head
            .as_ref(),
        Some(&block.request)
    );
    store
        .admit_embedded_input(
            &identity,
            "after",
            "operator",
            &Item::user_text("queued after refusal"),
        )
        .unwrap();
    assert!(
        matches!(rounds.next_round().unwrap(), EmbeddedRoundReadiness::Waiting(EmbeddedRoundWait::RecoveryRequired { block: waiting }) if waiting == block)
    );
    drop(rounds);
    drop(store);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(
        reopened.embedded_inference_state(&identity).unwrap(),
        HostInferenceState::Blocked { block }
    );
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn preparation_settlement_preserves_fatal_cleanup_contract_and_commit_failures() {
    use crate::embedding::{HostIdentity, HostInferenceState};
    use crate::engine::embedded_round::{CleanupReceipt, EngineRunReceipt};
    use crate::transport::{ResponsesRoute, UnsupportedSemantics};
    for control in ["cleanup", "secondary", "contract", "panic", "commit"] {
        let store = Arc::new(Store::memory().unwrap());
        let identity = HostIdentity {
            run: "fatal-controls".into(),
            actor: AgentPath("/root".into()),
            incarnation: control.into(),
        };
        store.bind_embedded_actor(&identity, None).unwrap();
        let attempt = store.begin_embedded_inference(&identity, None).unwrap();
        let head = RequestId(format!("{control}-head"));
        store
            .write_embedded_request(&identity, &head, None, &[], StoredUsage::default())
            .unwrap();
        let engine = Engine::<FakeAuth, Echo, _>::with_transport(
            Replay {
                requests: Arc::new(Mutex::new(Vec::new())),
                turns: Mutex::new(Default::default()),
            },
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            Arc::new(Echo),
            EngineConfig {
                instructions: "instruction".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "controls".into(),
                agent: identity.actor.clone(),
            },
        )
        .with_origin(ConversationIdentity::Embedded {
            run: identity.run.clone(),
            actor: identity.actor.clone(),
            incarnation: identity.incarnation.clone(),
        });
        let cause = match control {
            "contract" => EngineError::Store(StoreError::ProviderAdmission(
                TransportError::InvalidAdmission,
            )),
            "panic" => EngineError::StoreTask,
            _ => EngineError::RequestPreparationRefused {
                head_request: head.clone(),
                route: ResponsesRoute::DeepSeek,
                semantics: UnsupportedSemantics::Message,
            },
        };
        let mut receipt = EngineRunReceipt {
            result: Err(cause),
            head: Some(head.clone()),
            cleanup: if control == "cleanup" {
                CleanupReceipt::Unobserved
            } else {
                CleanupReceipt::Confirmed
            },
            secondary: vec![],
            inference: vec![],
            retained: vec![],
            head_settlements: vec![],
        };
        if control == "secondary" {
            receipt.add_failure(
                RoundFailureStage::FollowupObservation,
                EngineError::StoreTask,
            );
        }
        if control == "commit" {
            store.lock().unwrap().execute_batch("CREATE TABLE fault_parent(id INTEGER PRIMARY KEY); CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER preparation_fault AFTER UPDATE ON embedded_inference BEGIN INSERT INTO fault_child VALUES(1); END;").unwrap();
        }
        engine.settle_inference_attempt(&attempt, &mut receipt);
        assert!(
            !receipt
                .assessment()
                .inference_obligations()
                .iter()
                .any(|obligation| matches!(obligation, InferenceObligation::RecoverBlock { .. })),
            "{control}"
        );
        if control == "commit" {
            assert_eq!(
                receipt.assessment().store_continuation(),
                StoreContinuation::ReopenRequired
            );
            assert_eq!(
                receipt.assessment().secondary_failure_stages(),
                vec![RoundFailureStage::InferenceBlockRecording]
            );
            assert!(matches!(
                receipt.primary_error(),
                Some(EngineError::RequestPreparationRefused { .. })
            ));
        } else {
            assert_eq!(
                store.embedded_inference_state(&identity).unwrap(),
                HostInferenceState::Running,
                "{control}"
            );
        }
        let mut rounds = engine.into_embedded_rounds().unwrap();
        assert!(
            rounds.finish(None, receipt).is_err(),
            "{control} must remain fatal"
        );
    }
}
