//! Provider issuance owns streamed occurrences before any response termination.
use super::replay::{
    IssuedReplayRequest, IssuedRequestRecord, ResponseCompleteness, ResponseEnvelope,
    ResponseEnvelopeKind,
};
use super::{Result, Store, StoreError};
use crate::{
    context::Origin,
    item::{Item, ItemHash},
    model::RequestId,
    transport::{ResponsesProtocol, ResponsesRoute, sse::CompletedOutputItem},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderResponseState {
    Streaming,
    Completed,
    Interrupted,
    Failed,
    Rejected,
}

/// Only Store issuance creates this handle; restored JSON is not admission authority.
#[derive(Clone, Debug)]
pub(crate) struct IssuedProviderRequest {
    pub(super) request: RequestId,
    store_id: String,
    pub(super) issued: Arc<IssuedReplayRequest>,
}

impl IssuedProviderRequest {
    pub(crate) fn purpose(&self) -> super::ProviderRequestPurpose {
        self.issued
            .record
            .purpose
            .unwrap_or(super::ProviderRequestPurpose::Inference)
    }
}

impl Store {
    pub(crate) fn provider_request_purpose(
        &self,
        request: &RequestId,
    ) -> Result<Option<super::ProviderRequestPurpose>> {
        let purpose: Option<Option<String>> = self.lock()?.query_row("SELECT json_extract(issued_json,'$.purpose') FROM provider_requests WHERE request_id=?1", [&request.0], |row| row.get(0)).optional()?;
        purpose
            .flatten()
            .map(|purpose| serde_json::from_value(serde_json::Value::String(purpose)))
            .transpose()
            .map_err(Into::into)
    }
    pub(crate) fn issue_provider_request(
        &self,
        request: &RequestId,
        issued: IssuedReplayRequest,
        contract: (ResponsesRoute, ResponsesProtocol),
    ) -> Result<IssuedProviderRequest> {
        let encoded = serde_json::to_string(&issued.record)?;
        let route = serde_json::to_string(&contract.0)?;
        let protocol = serde_json::to_string(&contract.1)?;
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let prior: Option<(String, String, String)> = tx.query_row(
                "SELECT issued_json,route,protocol FROM provider_requests WHERE request_id=?1",
                [&request.0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            ).optional()?;
            if let Some(prior) = prior {
                if prior != (encoded.clone(), route.clone(), protocol.clone()) {
                    return Err(StoreError::ConflictingProviderIssuance(request.clone()));
                }
            } else {
                tx.execute("INSERT INTO provider_requests(request_id,issued_json,route,protocol,state) VALUES (?1,?2,?3,?4,'streaming')", params![request.0,encoded,route,protocol])?;
            }
            Ok(IssuedProviderRequest { request: request.clone(), store_id: self.store_id.clone(), issued: Arc::new(issued) })
        })
    }

    pub(crate) fn append_provider_done(
        &self,
        issued: &IssuedProviderRequest,
        done: &CompletedOutputItem,
    ) -> Result<bool> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            self.validate_provider_issuer(tx, issued)?;
            append_item(tx, &issued.request, done.output_index(), done.item())
        })
    }

    pub(super) fn validate_provider_issuer(
        &self,
        tx: &Connection,
        issued: &IssuedProviderRequest,
    ) -> Result<()> {
        if issued.store_id != self.store_id {
            return Err(StoreError::InvalidProviderEvidence(issued.request.clone()));
        }
        // Issuance fields have no mutation API. The private handle binds this
        // immutable request key to its Store; item admission need not encode or
        // compare the complete issued input window again.
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM provider_requests WHERE request_id=?1)",
            [&issued.request.0],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(StoreError::InvalidProviderEvidence(issued.request.clone()));
        }
        Ok(())
    }

    pub(crate) fn stop_provider_response(
        &self,
        request: &RequestId,
        state: ProviderResponseState,
    ) -> Result<()> {
        if matches!(
            state,
            ProviderResponseState::Streaming | ProviderResponseState::Completed
        ) {
            return Err(StoreError::InvalidProviderEvidence(request.clone()));
        }
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            tx.execute(
                "UPDATE provider_requests SET state=?2 WHERE request_id=?1 AND state='streaming'",
                params![request.0, state_name(state)],
            )?;
            Ok(())
        })
    }

    pub(super) fn provider_contract_tx(
        &self,
        c: &Connection,
        request: &RequestId,
    ) -> Result<(ResponsesRoute, ResponsesProtocol)> {
        let (route, protocol): (String, String) = c.query_row(
            "SELECT route,protocol FROM provider_requests WHERE request_id=?1",
            [&request.0],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((
            serde_json::from_str(&route)?,
            serde_json::from_str(&protocol)?,
        ))
    }

    pub fn provider_response_state(
        &self,
        request: &RequestId,
    ) -> Result<Option<ProviderResponseState>> {
        let raw: Option<String> = self
            .lock()?
            .query_row(
                "SELECT state FROM provider_requests WHERE request_id=?1",
                [&request.0],
                |r| r.get(0),
            )
            .optional()?;
        raw.map(|state| parse_state(&state, request)).transpose()
    }
}

fn state_name(state: ProviderResponseState) -> &'static str {
    match state {
        ProviderResponseState::Streaming => "streaming",
        ProviderResponseState::Completed => "completed",
        ProviderResponseState::Interrupted => "interrupted",
        ProviderResponseState::Failed => "failed",
        ProviderResponseState::Rejected => "rejected",
    }
}
fn parse_state(state: &str, request: &RequestId) -> Result<ProviderResponseState> {
    match state {
        "streaming" => Ok(ProviderResponseState::Streaming),
        "completed" => Ok(ProviderResponseState::Completed),
        "interrupted" => Ok(ProviderResponseState::Interrupted),
        "failed" => Ok(ProviderResponseState::Failed),
        "rejected" => Ok(ProviderResponseState::Rejected),
        _ => Err(StoreError::InvalidProviderEvidence(request.clone())),
    }
}

pub(super) fn append_item(
    tx: &Transaction<'_>,
    request: &RequestId,
    index: u64,
    item: &Item,
) -> Result<bool> {
    let purpose: Option<String> = tx.query_row(
        "SELECT json_extract(issued_json,'$.purpose') FROM provider_requests WHERE request_id=?1",
        [&request.0],
        |row| row.get(0),
    )?;
    let purpose = purpose
        .map(|purpose| {
            serde_json::from_value::<super::ProviderRequestPurpose>(serde_json::Value::String(
                purpose,
            ))
        })
        .transpose()?
        .unwrap_or(super::ProviderRequestPurpose::Inference);
    let inference = purpose == super::ProviderRequestPurpose::Inference;
    let index_i64 =
        i64::try_from(index).map_err(|_| StoreError::InvalidProviderEvidence(request.clone()))?;
    if inference && item.is_configuration_update() {
        return Err(StoreError::InvalidProviderEvidence(request.clone()));
    }
    let hash = Store::put_item_tx(tx, item)?;
    let prior: Option<(i64, String)> = tx.query_row("SELECT occurrence_position,item_hash FROM provider_items WHERE request_id=?1 AND output_index=?2", params![request.0,index_i64], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((position, prior_hash)) = prior {
        if prior_hash != hash.0 {
            return Err(StoreError::ConflictingProviderItem {
                request: request.clone(),
                index,
            });
        }
        let occurrence_hash: String = tx.query_row(
            "SELECT item_hash FROM request_items WHERE request_id=?1 AND position=?2",
            params![request.0, position],
            |r| r.get(0),
        )?;
        if occurrence_hash != hash.0 {
            return Err(StoreError::InvalidProviderEvidence(request.clone()));
        }
        return Ok(false);
    }
    let state: String = tx.query_row(
        "SELECT state FROM provider_requests WHERE request_id=?1",
        [&request.0],
        |r| r.get(0),
    )?;
    if state != "streaming" {
        return Err(StoreError::InvalidProviderEvidence(request.clone()));
    }
    let position: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position)+1,0) FROM request_items WHERE request_id=?1",
        [&request.0],
        |r| r.get(0),
    )?;
    if inference {
        super::output_publication::require_ordinary(item, request, position)?;
    }
    tx.execute(
        "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
        params![request.0, position, hash.0],
    )?;
    tx.execute("INSERT INTO provider_items(request_id,output_index,occurrence_position,item_hash) VALUES (?1,?2,?3,?4)", params![request.0,index_i64,position,hash.0])?;
    if inference {
        super::chat::publish(tx, request, position, &hash, item)?;
    }
    Ok(true)
}

/// New records use exact occurrence ownership. Absence means legacy evidence;
/// a malformed or incomplete new record never falls back to hash matching.
pub(super) fn response_envelopes(
    c: &Connection,
    request: &RequestId,
) -> Result<Option<HashMap<Origin, Arc<ResponseEnvelope>>>> {
    let issuance: Option<(String, String, String, String)> = c
        .query_row(
            "SELECT issued_json,route,protocol,state FROM provider_requests WHERE request_id=?1",
            [&request.0],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((issued, route, protocol, state)) = issuance else {
        return Ok(None);
    };
    let issued: IssuedRequestRecord = serde_json::from_str(&issued)?;
    if issued.model.trim().is_empty() {
        return Err(StoreError::InvalidProviderEvidence(request.clone()));
    }
    if issued.purpose == Some(super::ProviderRequestPurpose::ServerCompaction) {
        let envelope = super::compaction::source_response_envelope(c, request)?.map(Arc::new);
        return Ok(Some(
            envelope
                .into_iter()
                .flat_map(|envelope| {
                    envelope
                        .origins
                        .clone()
                        .into_iter()
                        .map(move |origin| (origin, envelope.clone()))
                })
                .collect(),
        ));
    }
    let route = Some(serde_json::from_str(&route)?);
    let protocol = Some(serde_json::from_str(&protocol)?);
    let state = parse_state(&state, request)?;
    let rows = {
        let mut q = c.prepare("SELECT p.output_index,p.occurrence_position,p.item_hash,i.item_hash FROM provider_items p JOIN request_items i ON i.request_id=p.request_id AND i.position=p.occurrence_position WHERE p.request_id=?1 ORDER BY p.output_index")?;
        q.query_map([&request.0], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut origins = Vec::with_capacity(rows.len());
    for (expected, (index, position, hash, occurrence_hash)) in rows.into_iter().enumerate() {
        if hash != occurrence_hash {
            return Err(StoreError::InvalidProviderEvidence(request.clone()));
        }
        if index != expected as u64 {
            return Err(StoreError::IncompleteProviderPrefix {
                request: request.clone(),
                index: expected as u64,
            });
        }
        origins.push(Origin {
            request: request.clone(),
            position,
            hash: ItemHash(hash),
        });
    }
    let envelope = Arc::new(ResponseEnvelope {
        model: issued.model,
        origins,
        kind: ResponseEnvelopeKind::ModelTurn,
        completeness: if state == ProviderResponseState::Completed {
            ResponseCompleteness::Completed
        } else {
            ResponseCompleteness::ObservedPrefix
        },
        route,
        protocol,
    });
    Ok(Some(
        envelope
            .origins
            .iter()
            .cloned()
            .map(|origin| (origin, envelope.clone()))
            .collect(),
    ))
}

/// Freeze only the exact authenticated prefix preceding a captured invocation.
/// Later response items cannot enlarge this proof or repair an unproven cut.
pub(super) fn captured_prefix(
    c: &Connection,
    request: &RequestId,
    boundary: &Origin,
    selected: &[Origin],
) -> Result<Option<Vec<Origin>>> {
    let owned: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM provider_requests WHERE request_id=?1)",
        [&request.0],
        |r| r.get(0),
    )?;
    if !owned || boundary.request != *request {
        return Ok(None);
    }
    let boundary_index: Option<u64> = c.query_row("SELECT output_index FROM provider_items WHERE request_id=?1 AND occurrence_position=?2 AND item_hash=?3", params![request.0,boundary.position,boundary.hash.0], |r| r.get(0)).optional()?;
    let Some(boundary_index) = boundary_index else {
        return Ok(None);
    };
    let rows = {
        let mut q = c.prepare("SELECT p.output_index,p.occurrence_position,p.item_hash,i.item_hash FROM provider_items p JOIN request_items i ON i.request_id=p.request_id AND i.position=p.occurrence_position WHERE p.request_id=?1 AND p.output_index<?2 ORDER BY p.output_index")?;
        q.query_map(params![request.0, boundary_index], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    if rows.len() as u64 != boundary_index {
        return Ok(None);
    }
    let mut origins = Vec::with_capacity(rows.len());
    for (expected, (index, position, hash, actual)) in rows.into_iter().enumerate() {
        let origin = Origin {
            request: request.clone(),
            position,
            hash: ItemHash(hash.clone()),
        };
        if index != expected as u64
            || actual != hash
            || selected
                .iter()
                .filter(|candidate| **candidate == origin)
                .count()
                != 1
        {
            return Ok(None);
        }
        origins.push(origin);
    }
    Ok(Some(origins))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::Effort,
        transport::{
            ResponsesRequest, ResponsesTurn, Usage,
            sse::{ResponseAssembly, StreamEvent},
        },
    };
    use proptest::prelude::*;
    use serde_json::json;

    fn item(value: u8) -> Item {
        Item(
            json!({"type":"reasoning","id":format!("reasoning-{value}"),"summary":[{"type":"summary_text","text":format!("observed-{value}")}]}),
        )
    }
    fn done(index: u64, item: &Item) -> CompletedOutputItem {
        let packet = json!({"type":"response.output_item.done","output_index":index,"item":item.0});
        match ResponseAssembly::default()
            .accept(&packet.to_string())
            .unwrap()
            .unwrap()
        {
            StreamEvent::CompletedItem(done) => done,
            _ => panic!("expected finalized provider item"),
        }
    }
    fn request() -> ResponsesRequest {
        ResponsesRequest {
            input: vec![],
            instructions: "instructions".into(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "model-original".into(),
            pinned_effort: Effort::Low,
            session_id: "session".into(),
            prompt_cache_comparison: None,
        }
    }
    fn issue(store: &Store) -> IssuedProviderRequest {
        let id = RequestId("provider-request".into());
        store.create_request(&id, None, "/root").unwrap();
        let sealed = store.seal_replay_request(&request()).unwrap();
        store
            .issue_provider_request(
                &id,
                sealed,
                (ResponsesRoute::Codex, ResponsesProtocol::Lite),
            )
            .unwrap()
    }
    fn turn(items: Vec<Item>) -> ResponsesTurn {
        ResponsesTurn {
            response_id: "response".into(),
            items,
            usage: Usage::default(),
        }
    }

    #[test]
    fn done_occurrences_are_owned_before_response_completion_and_duplicates_are_idempotent() {
        let store = Store::memory().unwrap();
        let issued = issue(&store);
        let value = item(1);
        assert!(
            store
                .append_provider_done(&issued, &done(0, &value))
                .unwrap()
        );
        assert!(
            !store
                .append_provider_done(&issued, &done(0, &value))
                .unwrap()
        );
        assert_eq!(
            store.provider_response_state(&issued.request).unwrap(),
            Some(ProviderResponseState::Streaming)
        );
        let c = store.lock().unwrap();
        let owned = response_envelopes(&c, &issued.request).unwrap().unwrap();
        let envelope = owned.values().next().unwrap();
        assert_eq!(envelope.model, "model-original");
        assert_eq!(envelope.route, Some(ResponsesRoute::Codex));
        assert_eq!(envelope.protocol, Some(ResponsesProtocol::Lite));
        assert_eq!(envelope.completeness, ResponseCompleteness::ObservedPrefix);
        assert_eq!(owned.len(), 1);
    }

    #[test]
    fn equal_bytes_at_distinct_provider_indices_keep_distinct_occurrences() {
        let store = Store::memory().unwrap();
        let issued = issue(&store);
        let value = item(1);
        store
            .append_provider_done(&issued, &done(0, &value))
            .unwrap();
        store
            .append_provider_done(&issued, &done(1, &value))
            .unwrap();
        let c = store.lock().unwrap();
        let owned = response_envelopes(&c, &issued.request).unwrap().unwrap();
        assert_eq!(owned.len(), 2);
        let origins = &owned.values().next().unwrap().origins;
        assert_eq!(origins[0].hash, origins[1].hash);
        assert_ne!(origins[0].position, origins[1].position);
    }

    #[test]
    fn out_of_order_done_preserves_arrival_and_authenticated_provider_order() {
        let store = Store::memory().unwrap();
        let issued = issue(&store);
        store
            .append_provider_done(&issued, &done(1, &item(2)))
            .unwrap();
        assert!(matches!(
            response_envelopes(&store.lock().unwrap(), &issued.request),
            Err(StoreError::IncompleteProviderPrefix { index: 0, .. })
        ));
        store
            .append_provider_done(&issued, &done(0, &item(1)))
            .unwrap();
        assert_eq!(
            store.items(&issued.request).unwrap(),
            vec![item(2), item(1)]
        );
        let c = store.lock().unwrap();
        let owned = response_envelopes(&c, &issued.request).unwrap().unwrap();
        assert_eq!(owned.len(), 2);
        assert_eq!(
            owned
                .values()
                .next()
                .unwrap()
                .origins
                .iter()
                .map(|o| o.position)
                .collect::<Vec<_>>(),
            vec![1, 0]
        );
    }

    #[test]
    fn provider_indices_remain_distinct_from_interleaved_operation_output_positions() {
        let store = Store::memory().unwrap();
        let issued = issue(&store);
        let invocation = Item(
            json!({"type":"function_call","call_id":"early-call","name":"work","arguments":"{}"}),
        );
        store
            .append_provider_done(&issued, &done(0, &invocation))
            .unwrap();
        let operation = store
            .claim(&crate::model::CallId("early-call".into()), &issued.request)
            .unwrap();
        let output = crate::turn::JobOutput::Completed(Ok(json!({"result":"effect-once"})));
        store
            .write_job_output(&operation, crate::item::ToolKind::Function, &output)
            .unwrap();
        store
            .append_operation_output(&operation, &issued.request, &issued.request)
            .unwrap();
        store
            .append_provider_done(&issued, &done(1, &item(2)))
            .unwrap();
        store
            .record_provider_turn(&issued, &turn(vec![invocation.clone(), item(2)]))
            .unwrap();
        let history = store.items(&issued.request).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0], invocation);
        assert_eq!(history[1].0["type"], "function_call_output");
        assert_eq!(history[2], item(2));
        let c = store.lock().unwrap();
        let owned = response_envelopes(&c, &issued.request).unwrap().unwrap();
        assert_eq!(
            owned
                .values()
                .next()
                .unwrap()
                .origins
                .iter()
                .map(|o| o.position)
                .collect::<Vec<_>>(),
            vec![0, 2]
        );
    }

    #[test]
    fn conflicting_index_and_foreign_store_refuse_without_mutation() {
        let store = Store::memory().unwrap();
        let issued = issue(&store);
        store
            .append_provider_done(&issued, &done(0, &item(1)))
            .unwrap();
        assert!(matches!(
            store.append_provider_done(&issued, &done(0, &item(2))),
            Err(StoreError::ConflictingProviderItem { index: 0, .. })
        ));
        assert_eq!(store.items(&issued.request).unwrap(), vec![item(1)]);
        let foreign = Store::memory().unwrap();
        issue(&foreign);
        assert!(matches!(
            foreign.append_provider_done(&issued, &done(1, &item(2))),
            Err(StoreError::InvalidProviderEvidence(_))
        ));
        assert!(foreign.items(&issued.request).unwrap().is_empty());
    }

    #[test]
    fn completion_reconciles_all_positions_and_rolls_back_its_entire_publication_on_failure() {
        let store = Store::memory().unwrap();
        let issued = issue(&store);
        store
            .append_provider_done(&issued, &done(0, &item(1)))
            .unwrap();
        store.lock().unwrap().execute_batch("CREATE TRIGGER reject_completion BEFORE INSERT ON events WHEN NEW.kind='model_turn' BEGIN SELECT RAISE(ABORT,'rejected'); END;").unwrap();
        assert!(
            store
                .record_provider_turn(&issued, &turn(vec![item(1), item(2)]))
                .is_err()
        );
        assert_eq!(store.items(&issued.request).unwrap(), vec![item(1)]);
        assert_eq!(
            store.provider_response_state(&issued.request).unwrap(),
            Some(ProviderResponseState::Streaming)
        );
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER reject_completion;")
            .unwrap();
        let event = store
            .record_provider_turn(&issued, &turn(vec![item(1), item(2)]))
            .unwrap();
        assert_eq!(
            store
                .record_provider_turn(&issued, &turn(vec![item(1), item(2)]))
                .unwrap(),
            event
        );
        assert_eq!(
            store.items(&issued.request).unwrap(),
            vec![item(1), item(2)]
        );
        assert_eq!(
            store.provider_response_state(&issued.request).unwrap(),
            Some(ProviderResponseState::Completed)
        );
        let decoded = store
            .decode_replay_record(
                event,
                &store
                    .events(Some(&issued.request))
                    .unwrap()
                    .into_iter()
                    .find(|e| e.id == event)
                    .unwrap()
                    .payload
                    .to_string(),
            )
            .unwrap();
        assert_eq!(
            decoded.provider_contract,
            Some((ResponsesRoute::Codex, ResponsesProtocol::Lite))
        );
    }

    #[test]
    fn interrupted_prefix_survives_reopen_without_becoming_completed() {
        let path =
            std::env::temp_dir().join(format!("provider-response-{}.sqlite", uuid::Uuid::new_v4()));
        let request;
        {
            let store = Store::open(&path).unwrap();
            let issued = issue(&store);
            request = issued.request.clone();
            store
                .append_provider_done(&issued, &done(0, &item(1)))
                .unwrap();
            store
                .stop_provider_response(&request, ProviderResponseState::Interrupted)
                .unwrap();
            assert!(
                store
                    .record_provider_turn(&issued, &turn(vec![item(1)]))
                    .is_err()
            );
        }
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.items(&request).unwrap(), vec![item(1)]);
            assert_eq!(
                store.provider_response_state(&request).unwrap(),
                Some(ProviderResponseState::Interrupted)
            );
            let c = store.lock().unwrap();
            let owned = response_envelopes(&c, &request).unwrap().unwrap();
            assert_eq!(
                owned.values().next().unwrap().completeness,
                ResponseCompleteness::ObservedPrefix
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn indexed_history_matches_independent_map_under_duplicates_conflicts_and_reordering(history in prop::collection::vec((0_u8..7,0_u8..4),1..55)) {
            let store=Store::memory().unwrap();let issued=issue(&store);
            let mut expected=std::collections::BTreeMap::<u8,u8>::new();
            let mut arrival=Vec::new();
            for (index,value) in history {
                let result=store.append_provider_done(&issued,&done(index as u64,&item(value)));
                match expected.get(&index) {
                    Some(previous) if *previous!=value => prop_assert!(matches!(result,Err(StoreError::ConflictingProviderItem{..})), "typed index conflict expected"),
                    Some(_) => prop_assert!(!result.unwrap()),
                    None => { prop_assert!(result.unwrap());expected.insert(index,value);arrival.push(item(value)); }
                }
                prop_assert_eq!(store.items(&issued.request).unwrap(),arrival.clone());
                let continuous=expected.keys().copied().eq(0..expected.len() as u8);
                let evidence=response_envelopes(&store.lock().unwrap(),&issued.request);
                if continuous { prop_assert_eq!(evidence.unwrap().unwrap().len(),expected.len()); }
                else { prop_assert!(matches!(evidence,Err(StoreError::IncompleteProviderPrefix{..})), "incomplete prefix expected"); }
            }
        }
    }
}

/// Order retained facts without authorizing continuity or certifying absence.
/// Provider indices and immutable durable occurrence positions are distinct.
pub(super) fn output_order(
    c: &Connection,
    requests: &std::collections::HashSet<RequestId>,
) -> Result<HashMap<Origin, u64>> {
    let mut result = HashMap::new();
    for request in requests {
        let mut q=c.prepare("SELECT p.output_index,p.occurrence_position,p.item_hash,i.item_hash FROM provider_items p LEFT JOIN request_items i ON i.request_id=p.request_id AND i.position=p.occurrence_position WHERE p.request_id=?1 ORDER BY p.output_index")?;
        let rows = q.query_map([&request.0], |r| {
            Ok((
                r.get::<_, u64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (index, position, hash, actual) = row?;
            if actual.as_deref() != Some(hash.as_str()) {
                return Err(StoreError::InvalidProviderEvidence(request.clone()));
            }
            let origin = Origin {
                request: request.clone(),
                position,
                hash: ItemHash(hash),
            };
            if result.insert(origin, index).is_some() {
                return Err(StoreError::InvalidProviderEvidence(request.clone()));
            }
        }
    }
    Ok(result)
}
