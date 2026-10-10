//! Request-owned admission and settlement for embedded Engine continuations.
use super::{Request, Result, Store, StoreError, Usage, embedded};
use crate::{
    embedding::HostIdentity,
    item::Item,
    model::{AgentPath, ConversationIdentity, RequestId},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbeddedRoundFrontier {
    /// Only settled history is published through agents.head_request.
    pub settled_head: Option<RequestId>,
    /// A durably admitted continuation exists even when its inbox is empty.
    pub pending_head: Option<RequestId>,
    /// Cleanup-confirmed interruption of the pending provider attempt. It
    /// requires fresh explicit input before another model request is admitted.
    pub pending_interruption: Option<crate::transport::StreamInterruption>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmbeddedRoundOutcome {
    Completed,
    /// The model continuation stopped; this does not prove external cleanup.
    Cancelled,
    Rejected,
}
impl EmbeddedRoundOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Rejected => "rejected",
        }
    }
}

pub(super) fn frontier(c: &Connection, identity: &HostIdentity) -> Result<EmbeddedRoundFrontier> {
    if !embedded::matches_binding(c, identity)? {
        return Err(StoreError::InvalidEmbeddedBinding);
    }
    let settled_head: Option<String> = c.query_row(
        "SELECT head_request FROM agents WHERE path=?1",
        [&identity.actor.0],
        |r| r.get(0),
    )?;
    let settled_head = settled_head.map(RequestId);
    let mut tip = settled_head.clone();
    let mut seen = std::collections::HashSet::new();
    loop {
        let rows = {
            let mut query = c.prepare(
                "SELECT id,embedded_run,embedded_incarnation,round_phase FROM requests \
                 WHERE parent_id IS ?1 AND branch=?2 ORDER BY id LIMIT 2",
            )?;
            query
                .query_map(
                    params![tip.as_ref().map(|id| &id.0), identity.actor.0],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, Option<String>>(3)?,
                        ))
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if rows.is_empty() {
            break;
        }
        if rows.len() != 1 {
            return Err(StoreError::InvalidEmbeddedFrontier);
        }
        let (id, run, incarnation, phase) = &rows[0];
        // Successor binding authorizes the same run's retained conversation,
        // while each admitted request keeps its original issuer incarnation.
        if run.as_deref() != Some(identity.run.as_str())
            || incarnation.as_deref().is_none_or(str::is_empty)
        {
            return Err(StoreError::UnownedEmbeddedRequest);
        }
        if phase.as_deref() != Some("pending") || !seen.insert(id.clone()) {
            return Err(StoreError::InvalidEmbeddedFrontier);
        }
        tip = Some(RequestId(id.clone()));
    }
    let pending_head = if tip == settled_head { None } else { tip };
    let pending_interruption = match &pending_head {
        Some(request) => c.query_row(
            "SELECT payload FROM events WHERE request_id=?1 AND kind='model_interrupted' ORDER BY id DESC LIMIT 1",
            [&request.0], |row| row.get::<_, String>(0),
        ).optional()?.map(|payload| serde_json::from_str(&payload)).transpose()?,
        None => None,
    };
    Ok(EmbeddedRoundFrontier {
        pending_head,
        pending_interruption,
        settled_head,
    })
}

pub(super) fn settle_tx(
    tx: &Transaction<'_>,
    identity: &HostIdentity,
    expected: Option<&RequestId>,
    request: &RequestId,
    outcome: EmbeddedRoundOutcome,
) -> Result<bool> {
    let current = frontier(tx, identity)?;
    if current.settled_head.as_ref() != expected || current.pending_head.as_ref() != Some(request) {
        return Ok(false);
    }
    if tx.execute(
        "UPDATE agents SET head_request=?3 WHERE path=?1 AND head_request IS ?2",
        params![identity.actor.0, expected.map(|id| &id.0), request.0],
    )? != 1
    {
        return Ok(false);
    }
    if tx.execute(
        "UPDATE requests SET round_phase=?2 WHERE id=?1 AND round_phase='pending'",
        params![request.0, outcome.as_str()],
    )? != 1
    {
        return Err(StoreError::InvalidEmbeddedFrontier);
    }
    Ok(true)
}

impl Store {
    /// Read immutable request issuer provenance, rather than its successor binding.
    pub(crate) fn request_output_origin(&self, id: &RequestId) -> Result<ConversationIdentity> {
        let c = self.lock()?;
        let (branch, run, incarnation): (String, Option<String>, Option<String>) = c.query_row(
            "SELECT branch,embedded_run,embedded_incarnation FROM requests WHERE id=?1",
            [&id.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        match (run, incarnation) {
            (Some(run), Some(incarnation)) => Ok(ConversationIdentity::Embedded {
                run,
                actor: AgentPath(branch),
                incarnation,
            }),
            (None, None) => Ok(self.standalone_identity(AgentPath(branch))),
            _ => Err(StoreError::UnownedEmbeddedRequest),
        }
    }

    /// Exact binding and the entire branch frontier share a SQLite snapshot.
    pub fn embedded_round_frontier(
        &self,
        identity: &HostIdentity,
    ) -> Result<EmbeddedRoundFrontier> {
        let mut c = self.lock()?;
        c.read_transaction(|tx| frontier(tx, identity))
    }

    pub fn settle_embedded_round(
        &self,
        identity: &HostIdentity,
        expected: Option<&RequestId>,
        request: &RequestId,
        outcome: EmbeddedRoundOutcome,
    ) -> Result<bool> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| settle_tx(tx, identity, expected, request, outcome))
    }

    /// Admission closes the request-row cut before any input is marked delivered.
    pub(crate) fn write_embedded_request(
        &self,
        identity: &HostIdentity,
        request: &RequestId,
        parent: Option<&RequestId>,
        items: &[Item],
        usage: Usage,
    ) -> Result<Request> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            // Check before INSERT: an unresolved predecessor must never be bypassed.
            let current = frontier(tx, identity)?;
            if current
                .pending_head
                .as_ref()
                .or(current.settled_head.as_ref())
                != parent
            {
                return Err(StoreError::InvalidEmbeddedFrontier);
            }
            tx.execute("INSERT INTO requests(id,parent_id,branch,created_at,input_tokens,output_tokens,cost_micros,embedded_run,embedded_incarnation,round_phase) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'pending')",
            params![request.0,parent.map(|id| &id.0),identity.actor.0,super::utc_millis(),usage.input_tokens,usage.output_tokens,usage.cost_micros,identity.run,identity.incarnation])?;
            for (position, item) in items
                .iter()
                .filter(|item| !item.is_configuration_update())
                .enumerate()
            {
                super::output_publication::require_ordinary(item, request, position as i64)?;
                let hash = Self::put_item_tx(tx, item)?;
                tx.execute(
                    "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                    params![request.0, position as i64, hash.0],
                )?;
                super::chat::publish(tx, request, position as i64, &hash, item)?;
            }
            Ok(Request {
                id: request.clone(),
                parent: parent.cloned(),
                branch: identity.actor.0.clone(),
            })
        })
    }

    pub(crate) fn recorded_response(
        &self,
        request: &RequestId,
    ) -> Result<Option<crate::transport::ResponsesTurn>> {
        let rows = {
            let c = self.lock()?;
            let mut q=c.prepare("SELECT id,payload FROM events WHERE request_id=?1 AND kind='model_turn' ORDER BY id LIMIT 2")?;
            q.query_map([&request.0], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        if rows.len() > 1 {
            return Err(StoreError::InvalidEmbeddedFrontier);
        }
        rows.into_iter()
            .next()
            .map(|(event, payload)| {
                self.decode_replay_record(event, &payload)
                    .map(|turn| turn.model_response)
            })
            .transpose()
    }
}

#[cfg(test)]
#[path = "embedded_round/history_model_tests.rs"]
mod history_model_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentPath, CallId, OperationId};
    fn identity() -> HostIdentity {
        HostIdentity {
            run: "frontier-run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "first".into(),
        }
    }
    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("embedded-frontier-{}.sqlite", uuid::Uuid::new_v4()))
    }
    struct TransferAuthority;
    impl crate::embedding::BindingSuccessorAuthority for TransferAuthority {
        fn validate_successor(
            &self,
            _: &HostIdentity,
            _: &HostIdentity,
        ) -> std::result::Result<bool, String> {
            Ok(true)
        }
    }
    #[test]
    fn request_and_settlement_commit_faults_fence_siblings_until_reopen() {
        for settlement in [false, true] {
            let file = path();
            let store = std::sync::Arc::new(Store::open(&file).unwrap());
            let sibling = store.clone();
            let identity = identity();
            store.bind_embedded_actor(&identity, None).unwrap();
            let request = RequestId("commit-fault".into());
            if settlement {
                store
                    .write_embedded_request(&identity, &request, None, &[], Usage::default())
                    .unwrap();
            }
            let trigger = if settlement {
                "CREATE TRIGGER round_commit_fault AFTER UPDATE OF head_request ON agents BEGIN INSERT INTO commit_fault_child VALUES(1); END;"
            } else {
                "CREATE TRIGGER round_commit_fault AFTER INSERT ON requests BEGIN INSERT INTO commit_fault_child VALUES(1); END;"
            };
            store.lock().unwrap().execute_batch(&format!("CREATE TABLE commit_fault_parent(id INTEGER PRIMARY KEY); CREATE TABLE commit_fault_child(id INTEGER REFERENCES commit_fault_parent(id) DEFERRABLE INITIALLY DEFERRED); {trigger}")).unwrap();
            let result = if settlement {
                store
                    .settle_embedded_round(
                        &identity,
                        None,
                        &request,
                        EmbeddedRoundOutcome::Completed,
                    )
                    .map(|_| ())
            } else {
                store
                    .write_embedded_request(&identity, &request, None, &[], Usage::default())
                    .map(|_| ())
            };
            assert!(
                matches!(result, Err(StoreError::RecoveryRequired(error)) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation))
            );
            assert!(matches!(
                sibling.embedded_round_frontier(&identity),
                Err(StoreError::AdmissionFenced)
            ));
            let recovered = Store::open(&file).unwrap();
            let frontier = recovered.embedded_round_frontier(&identity).unwrap();
            assert_eq!(frontier.settled_head, None);
            assert_eq!(frontier.pending_head, settlement.then(|| request.clone()));
            recovered
                .lock()
                .unwrap()
                .execute_batch("DROP TRIGGER round_commit_fault")
                .unwrap();
            if !settlement {
                recovered
                    .write_embedded_request(&identity, &request, None, &[], Usage::default())
                    .unwrap();
            }
            assert!(
                recovered
                    .settle_embedded_round(
                        &identity,
                        None,
                        &request,
                        EmbeddedRoundOutcome::Completed
                    )
                    .unwrap()
            );
            assert!(
                !recovered
                    .settle_embedded_round(
                        &identity,
                        None,
                        &request,
                        EmbeddedRoundOutcome::Completed
                    )
                    .unwrap()
            );
            assert_eq!(
                recovered
                    .embedded_round_frontier(&identity)
                    .unwrap()
                    .settled_head,
                Some(request)
            );
            let count: i64 = recovered
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM requests", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 1);
            drop(recovered);
            drop(sibling);
            drop(store);
            std::fs::remove_file(file).unwrap();
        }
    }
    #[test]
    fn conversation_history_distinguishes_completion_without_identity_from_missing_response() {
        use crate::store::EmbeddedModelResponseState;
        let store = Store::memory().unwrap();
        let identity = identity();
        store.bind_embedded_actor(&identity, None).unwrap();
        let request = RequestId("empty-response-id".into());
        store
            .write_embedded_request(&identity, &request, None, &[], Usage::default())
            .unwrap();
        let active = store.embedded_conversation_history(&identity).unwrap();
        assert_eq!(active.head, Some(request.clone()));
        assert!(active.history.is_empty());
        assert_eq!(
            active.responses[&request],
            EmbeddedModelResponseState::InProgress
        );
        let model_request = crate::transport::ResponsesRequest {
            input: vec![],
            instructions: "test".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "mock".into(),
            pinned_effort: crate::model::Effort::Low,
            session_id: "test".into(),
        };
        let response = crate::transport::ResponsesTurn {
            response_id: String::new(),
            items: vec![],
            usage: Default::default(),
        };
        store
            .record_replay_turn(&request, &model_request, &response)
            .unwrap();
        assert_eq!(
            store
                .embedded_conversation_history(&identity)
                .unwrap()
                .responses[&request],
            EmbeddedModelResponseState::Completed { response_id: None }
        );
        assert!(
            store
                .settle_embedded_round(&identity, None, &request, EmbeddedRoundOutcome::Completed)
                .unwrap()
        );
        assert_eq!(
            store
                .embedded_conversation_history(&identity)
                .unwrap()
                .responses[&request],
            EmbeddedModelResponseState::Completed { response_id: None }
        );
        // A synthetic or retained request can be settled without a model response.
        let retained = RequestId("retained-history".into());
        store
            .write_embedded_request(&identity, &retained, Some(&request), &[], Usage::default())
            .unwrap();
        assert!(
            store
                .settle_embedded_round(
                    &identity,
                    Some(&request),
                    &retained,
                    EmbeddedRoundOutcome::Completed
                )
                .unwrap()
        );
        assert_eq!(
            store
                .embedded_conversation_history(&identity)
                .unwrap()
                .responses[&retained],
            EmbeddedModelResponseState::Unknown
        );
        let stale = HostIdentity {
            incarnation: "stale".into(),
            ..identity
        };
        assert!(matches!(
            store.embedded_conversation_history(&stale),
            Err(StoreError::InvalidEmbeddedBinding)
        ));
    }

    #[test]
    fn conversation_history_does_not_treat_a_pending_compaction_cut_as_a_model_response() {
        let store = Store::memory().unwrap();
        let identity = identity();
        store.bind_embedded_actor(&identity, None).unwrap();
        let request = RequestId("original".into());
        store
            .write_embedded_request(&identity, &request, None, &[], Usage::default())
            .unwrap();
        let cut = RequestId("compaction".into());
        store
            .write_compaction_request_with_claims(
                &cut,
                &request,
                &identity.actor.0,
                &[],
                &[],
                Some(&identity),
            )
            .unwrap();
        let snapshot = store.embedded_conversation_history(&identity).unwrap();
        assert_eq!(snapshot.head, Some(cut.clone()));
        assert_eq!(
            snapshot.responses[&cut],
            crate::store::EmbeddedModelResponseState::Unknown
        );
    }

    #[test]
    fn conversation_history_refuses_conflicting_completed_response_records() {
        let store = Store::memory().unwrap();
        let identity = identity();
        store.bind_embedded_actor(&identity, None).unwrap();
        let request = RequestId("conflicting-response".into());
        store
            .write_embedded_request(&identity, &request, None, &[], Usage::default())
            .unwrap();
        let model_request = crate::transport::ResponsesRequest {
            input: vec![],
            instructions: "test".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "mock".into(),
            pinned_effort: crate::model::Effort::Low,
            session_id: "test".into(),
        };
        for response_id in ["first", "second"] {
            store
                .record_replay_turn(
                    &request,
                    &model_request,
                    &crate::transport::ResponsesTurn {
                        response_id: response_id.into(),
                        items: vec![],
                        usage: Default::default(),
                    },
                )
                .unwrap();
        }
        assert!(matches!(
            store.embedded_conversation_history(&identity),
            Err(StoreError::InvalidEmbeddedFrontier)
        ));
    }

    #[test]
    fn live_output_origin_keeps_request_issuer_after_binding_transfer() {
        let store = Store::memory().unwrap();
        let first = identity();
        store.bind_embedded_actor(&first, None).unwrap();
        let request = RequestId("pending".into());
        store
            .write_embedded_request(&first, &request, None, &[], Usage::default())
            .unwrap();
        let successor = HostIdentity {
            incarnation: "second".into(),
            ..first.clone()
        };
        store
            .transfer_embedded_binding(&first, &successor, &TransferAuthority)
            .unwrap();
        assert!(store.embedded_binding_matches(&successor).unwrap());
        assert_eq!(
            store.request_output_origin(&request).unwrap(),
            ConversationIdentity::Embedded {
                run: first.run,
                actor: first.actor,
                incarnation: first.incarnation
            }
        );
    }
    #[test]
    fn frontier_survives_request_and_delivery_cuts_with_null_and_settled_heads() {
        for has_head in [false, true] {
            for delivered in [false, true] {
                let path = path();
                let identity = identity();
                let pending = RequestId("pending".into());
                let settled = has_head.then(|| RequestId("settled".into()));
                let input = Item(serde_json::json!({"role":"user","content":"retained input"}));
                {
                    let store = Store::open(&path).unwrap();
                    store.bind_embedded_actor(&identity, None).unwrap();
                    if let Some(head) = &settled {
                        store
                            .write_embedded_request(&identity, head, None, &[], Usage::default())
                            .unwrap();
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
                            Usage::default(),
                        )
                        .unwrap();
                    if delivered {
                        store
                            .append_unread_envelopes(&identity.actor, &pending)
                            .unwrap();
                    }
                }
                let store = Store::open(&path).unwrap();
                assert_eq!(
                    store.embedded_round_frontier(&identity).unwrap(),
                    EmbeddedRoundFrontier {
                        settled_head: settled.clone(),
                        pending_head: Some(pending.clone()),
                        pending_interruption: None
                    }
                );
                assert_eq!(
                    store.unread("/root").unwrap().len(),
                    usize::from(!delivered)
                );
                assert!(matches!(
                    store.write_embedded_request(
                        &identity,
                        &RequestId("wrong-child".into()),
                        settled.as_ref(),
                        &[],
                        Usage::default()
                    ),
                    Err(StoreError::InvalidEmbeddedFrontier)
                ));
                assert!(
                    store
                        .request(&RequestId("wrong-child".into()))
                        .unwrap()
                        .is_none()
                );
                drop(store);
                std::fs::remove_file(path).unwrap();
            }
        }
    }
    #[test]
    fn frontier_refuses_legacy_and_ambiguous_null_roots() {
        for count in [1, 2] {
            let store = Store::memory().unwrap();
            let identity = identity();
            store.bind_embedded_actor(&identity, None).unwrap();
            for n in 0..count {
                store
                    .create_request(&RequestId(format!("legacy-{n}")), None, "/root")
                    .unwrap();
            }
            assert!(matches!(
                store.embedded_round_frontier(&identity),
                Err(StoreError::UnownedEmbeddedRequest | StoreError::InvalidEmbeddedFrontier)
            ));
        }
    }
    #[test]
    fn compaction_admission_rolls_back_with_claim_failure_and_fences_binding() {
        let store = Store::memory().unwrap();
        let identity = identity();
        store.bind_embedded_actor(&identity, None).unwrap();
        let source = RequestId("source".into());
        store
            .write_embedded_request(&identity, &source, None, &[], Usage::default())
            .unwrap();
        let compact = RequestId("compact".into());
        let invalid = OperationId {
            origin: crate::model::ConversationIdentity::Embedded {
                run: identity.run.clone(),
                actor: identity.actor.clone(),
                incarnation: identity.incarnation.clone(),
            },
            request: source.clone(),
            call: CallId("missing".into()),
        };
        assert!(
            store
                .write_compaction_request_with_claims(
                    &compact,
                    &source,
                    "/root",
                    &[],
                    &[invalid],
                    Some(&identity)
                )
                .is_err()
        );
        assert!(store.request(&compact).unwrap().is_none());
        assert_eq!(
            store
                .embedded_round_frontier(&identity)
                .unwrap()
                .pending_head,
            Some(source)
        );
        let mut wrong = identity.clone();
        wrong.incarnation = "other".into();
        assert!(matches!(
            store.write_embedded_request(&wrong, &compact, None, &[], Usage::default()),
            Err(StoreError::InvalidEmbeddedBinding)
        ));
        assert!(store.request(&compact).unwrap().is_none());
    }
}
