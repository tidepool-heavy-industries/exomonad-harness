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
        let c = self.lock();
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
        let mut c = self.lock();
        let tx = c.transaction()?;
        let result = frontier(&tx, identity)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn settle_embedded_round(
        &self,
        identity: &HostIdentity,
        expected: Option<&RequestId>,
        request: &RequestId,
        outcome: EmbeddedRoundOutcome,
    ) -> Result<bool> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let changed = settle_tx(&tx, identity, expected, request, outcome)?;
        if changed {
            tx.commit()?;
        }
        Ok(changed)
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
        let mut c = self.lock();
        let tx = c.transaction()?;
        // Check before INSERT: an unresolved predecessor must never be bypassed.
        let current = frontier(&tx, identity)?;
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
            let hash = Self::put_item_tx(&tx, item)?;
            tx.execute(
                "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                params![request.0, position as i64, hash.0],
            )?;
            super::chat::publish(&tx, request, position as i64, &hash, item)?;
        }
        tx.commit()?;
        Ok(Request {
            id: request.clone(),
            parent: parent.cloned(),
            branch: identity.actor.0.clone(),
        })
    }

    pub(crate) fn recorded_response(
        &self,
        request: &RequestId,
    ) -> Result<Option<crate::transport::ResponsesTurn>> {
        let rows = {
            let c = self.lock();
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
