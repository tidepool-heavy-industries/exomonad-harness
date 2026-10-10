//! Exact issued windows reference immutable Store bytes, never later history.
use super::{RecordedReplayTurn, Result, Store, StoreError, utc_millis};
use crate::{
    context::{Occurrence, Origin},
    finalize::{FINALIZE_TOOL_NAME, ValidatedFinalize},
    item::{Item, ItemHash},
    model::{Effort, OperationId, RequestId},
    transport::{ResponsesRequest, ResponsesTurn, Usage},
};
use rusqlite::{Connection, OptionalExtension, params};

use serde::{Deserialize, Serialize};

const FORMAT: u32 = 1;

/// Sealed before transport starts; later output cannot change these references.
#[derive(Clone, Debug, Serialize)]
pub struct IssuedReplayRequest {
    #[serde(flatten)]
    record: IssuedRequestRecord,
    #[cfg(test)]
    #[serde(skip)]
    input_reencodings: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IssuedRequestRecord {
    input: Vec<ItemHash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outputs: Option<Vec<Option<IssuedOutputReference>>>,
    instructions: ItemHash,
    tools: ItemHash,
    tools_allowed: Option<Vec<String>>,
    model: String,
    pinned_effort: Effort,
    session_id: String,
}

/// Issued position owns its immutable source independently of content equality.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IssuedOutputReference {
    pub(crate) origin: Origin,
    pub(crate) operation: OperationId,
}

#[derive(Serialize, Deserialize)]
struct ReplayRecord {
    format: u32,
    request: RequestId,
    issued: IssuedRequestRecord,
    response: ReplayResponse,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    completion: Option<CompletionMarker>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CompletionMarker {
    Finalize { origin: Origin, schema: ItemHash },
}

#[derive(Serialize, Deserialize)]
struct ReplayResponse {
    response_id: String,
    items: Vec<ItemHash>,
    usage: Usage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ResponseEnvelopeKind {
    ModelTurn,
    ServerCompaction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ResponseEnvelope {
    pub(super) model: String,
    pub(super) origins: Vec<Origin>,
    pub(super) kind: ResponseEnvelopeKind,
}

/// Presence of a completed record is independent of provider identity availability.
pub(super) struct RecordedResponseIdentity {
    pub(super) response_id: Option<String>,
}

pub(super) fn recorded_response_identity(
    c: &Connection,
    request: &RequestId,
) -> Result<Option<RecordedResponseIdentity>> {
    let mut query = c.prepare(
        "SELECT id,payload FROM events WHERE request_id=?1 AND kind='model_turn' ORDER BY id LIMIT 2",
    )?;
    let records = query
        .query_map([&request.0], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if records.len() > 1 {
        return Err(StoreError::InvalidEmbeddedFrontier);
    }
    let Some((event, payload)) = records.into_iter().next() else {
        return Ok(None);
    };
    let record: ReplayRecord = serde_json::from_str(&payload)?;
    if record.format != FORMAT {
        return Err(StoreError::UnsupportedReplayFormat { event });
    }
    if record.request != *request {
        return Err(StoreError::InvalidEmbeddedFrontier);
    }
    Ok(Some(RecordedResponseIdentity {
        response_id: (!record.response.response_id.is_empty())
            .then_some(record.response.response_id),
    }))
}

/// Group membership and the issuing model share one evidence reader. Hashes
/// identify bytes, so response hashes must resolve to a unique ordered sequence
/// of original occurrences before they authorize an envelope.
pub(super) fn response_envelopes(
    c: &Connection,
    request: &RequestId,
) -> Result<std::collections::HashMap<Origin, std::sync::Arc<ResponseEnvelope>>> {
    use std::collections::{HashMap, HashSet};
    let mut query =
        c.prepare("SELECT payload FROM events WHERE request_id=?1 AND kind='model_turn'")?;
    let payloads = query
        .query_map([&request.0], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let original = super::context::request_occurrences(c, request)?
        .into_iter()
        .filter(|i| i.request == i.origin.request && i.position == i.origin.position)
        .collect::<Vec<_>>();
    let mut envelopes = Vec::new();
    let mut unknown = HashSet::new();
    for payload in payloads {
        let raw: serde_json::Value = serde_json::from_str(&payload)?;
        let hashes = raw["response"]["items"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|h| h.as_str().map(|h| ItemHash(h.into())))
            .collect::<Vec<_>>();
        let Ok(record) = serde_json::from_str::<ReplayRecord>(&payload) else {
            unknown.extend(hashes);
            continue;
        };
        if record.format != FORMAT
            || record.request != *request
            || record.issued.model.trim().is_empty()
        {
            unknown.extend(hashes);
            continue;
        }
        let hashes = &record.response.items;
        let first = ordered_membership(&original, hashes, false);
        let last = ordered_membership(&original, hashes, true);
        let Some(origins) = first.filter(|first| Some(first) == last.as_ref()) else {
            unknown.extend(hashes.iter().cloned());
            continue;
        };
        envelopes.push(ResponseEnvelope {
            model: record.issued.model,
            origins,
            kind: ResponseEnvelopeKind::ModelTurn,
        });
    }
    envelopes.extend(super::compaction::response_envelopes(c, request)?);
    let mut matching = HashMap::new();
    let mut ambiguous = HashSet::new();
    for envelope in envelopes {
        let envelope = std::sync::Arc::new(envelope);
        for origin in &envelope.origins {
            if matching
                .insert(origin.clone(), std::sync::Arc::clone(&envelope))
                .is_some()
            {
                ambiguous.insert(origin.clone());
            }
        }
    }
    matching.retain(|origin, _| !unknown.contains(&origin.hash) && !ambiguous.contains(origin));
    Ok(matching)
}

fn ordered_membership(
    original: &[crate::context::Occurrence],
    hashes: &[ItemHash],
    reverse: bool,
) -> Option<Vec<Origin>> {
    let mut positions = Vec::with_capacity(hashes.len());
    if reverse {
        let mut end = original.len();
        for hash in hashes.iter().rev() {
            let index = original[..end].iter().rposition(|i| &i.hash == hash)?;
            positions.push(original[index].origin.clone());
            end = index;
        }
        positions.reverse();
    } else {
        let mut start = 0;
        for hash in hashes {
            let index = start + original[start..].iter().position(|i| &i.hash == hash)?;
            positions.push(original[index].origin.clone());
            start = index + 1;
        }
    }
    Some(positions)
}

fn read_item(tx: &Connection, hash: &ItemHash) -> Result<Item> {
    let json: String = tx
        .query_row("SELECT json FROM items WHERE hash=?1", [&hash.0], |row| {
            row.get(0)
        })
        .optional()?
        .ok_or_else(|| StoreError::MissingReplayItem(hash.0.clone()))?;
    Ok(serde_json::from_str(&json)?)
}

fn seal_output_references(
    c: &Connection,
    head: &RequestId,
    input: &[Item],
    occurrences: &[Option<Occurrence>],
) -> Result<Vec<Option<IssuedOutputReference>>> {
    if input.len() != occurrences.len() {
        return Err(StoreError::OperationOriginMismatch);
    }
    let selected = occurrences
        .iter()
        .flatten()
        .filter(|occurrence| occurrence.output_operation.is_some())
        .cloned()
        .collect::<Vec<_>>();
    let requests = selected
        .iter()
        .map(|occurrence| occurrence.request.clone())
        .collect();
    let persisted = super::context::occurrences_for_requests(c, &requests)?
        .into_iter()
        .map(|occurrence| {
            (
                (occurrence.request.clone(), occurrence.position),
                occurrence,
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let visible = super::context::history(c, head, true)?
        .into_iter()
        .map(|occurrence| (occurrence.request, occurrence.position))
        .collect::<std::collections::HashSet<_>>();
    super::context::validate_canonical_history(c, &selected)?;
    let mut seen = std::collections::HashSet::new();
    input
        .iter()
        .zip(occurrences)
        .map(|(item, occurrence)| {
            let Some(occurrence) = occurrence.as_ref().filter(|o| o.output_operation.is_some())
            else {
                return Ok(None);
            };
            let key = (occurrence.request.clone(), occurrence.position);
            if !visible.contains(&key) || persisted.get(&key) != Some(occurrence) {
                return Err(StoreError::OperationOriginMismatch);
            }
            // Model projection may turn a native output into a readable message.
            // That message carries no output publication authority.
            if !super::output_publication::is_tool_output(item) {
                return Ok(None);
            }
            let operation = occurrence.output_operation.as_ref().unwrap();
            if item.0["type"] != occurrence.item.0["type"]
                || item.0["call_id"] != operation.call.0
                || !seen.insert(occurrence.origin.clone())
            {
                return Err(StoreError::OperationOriginMismatch);
            }
            Ok(Some(IssuedOutputReference {
                origin: occurrence.origin.clone(),
                operation: operation.clone(),
            }))
        })
        .collect()
}

fn validate_output_references(
    c: &Connection,
    event: i64,
    input: &[Item],
    outputs: &[Option<IssuedOutputReference>],
) -> Result<()> {
    let invalid = || StoreError::InvalidReplayOwnership { event };
    if outputs.len() != input.len() {
        return Err(invalid());
    }
    let requests = outputs
        .iter()
        .flatten()
        .map(|owner| owner.origin.request.clone())
        .collect();
    let sources = super::context::occurrences_for_requests(c, &requests)?
        .into_iter()
        .map(|occurrence| {
            (
                (occurrence.request.clone(), occurrence.position),
                occurrence,
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (item, owner) in input.iter().zip(outputs) {
        let Some(owner) = owner else {
            continue;
        };
        let source = sources
            .get(&(owner.origin.request.clone(), owner.origin.position))
            .ok_or_else(invalid)?;
        if source.origin != owner.origin
            || source.output_operation.as_ref() != Some(&owner.operation)
            || item.0["type"] != source.item.0["type"]
            || item.0["call_id"] != owner.operation.call.0
            || !seen.insert(&owner.origin)
        {
            return Err(invalid());
        }
        selected.push(source.clone());
    }
    super::context::validate_canonical_history(c, &selected)?;
    Ok(())
}

impl Store {
    /// Intern exact bytes without output occurrence authority. Engine seals its
    /// aligned occurrence cut through `seal_replay_request_with_occurrences`.
    pub fn seal_replay_request(&self, request: &ResponsesRequest) -> Result<IssuedReplayRequest> {
        self.seal_replay_request_with_hashes(request, &vec![None; request.input.len()], None)
    }

    pub(crate) fn intern_replay_instructions(&self, instructions: &str) -> Result<ItemHash> {
        self.put_item(&Item(serde_json::Value::String(instructions.to_owned())))
    }

    pub(crate) fn seal_replay_request_with_hashes(
        &self,
        request: &ResponsesRequest,
        hashes: &[Option<ItemHash>],
        instructions: Option<&ItemHash>,
    ) -> Result<IssuedReplayRequest> {
        self.seal_replay_request_inner(request, hashes, instructions, None)
    }

    pub(crate) fn seal_replay_request_with_occurrences(
        &self,
        head: &RequestId,
        request: &ResponsesRequest,
        hashes: &[Option<ItemHash>],
        instructions: Option<&ItemHash>,
        occurrences: &[Option<Occurrence>],
    ) -> Result<IssuedReplayRequest> {
        self.seal_replay_request_inner(request, hashes, instructions, Some((head, occurrences)))
    }

    fn seal_replay_request_inner(
        &self,
        request: &ResponsesRequest,
        hashes: &[Option<ItemHash>],
        instructions: Option<&ItemHash>,
        occurrences: Option<(&RequestId, &[Option<Occurrence>])>,
    ) -> Result<IssuedReplayRequest> {
        let _seal = tracing::debug_span!(target: "harness::runtime_cost", "seal_replay_request_with_hashes",
            session_id = %request.session_id, input_items = request.input.len(),
            tool_count = request.tools.len()).entered();
        assert_eq!(
            hashes.len(),
            request.input.len(),
            "Engine input provenance must align"
        );
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let outputs = occurrences
            .map(|(head, occurrences)| {
                seal_output_references(&tx, head, &request.input, occurrences)
            })
            .transpose()?;
        #[cfg(test)]
        let mut input_reencodings = 0;
        let input = request
            .input
            .iter()
            .zip(hashes)
            .enumerate()
            .map(|(position, (item, hash))| {
                if outputs
                    .as_ref()
                    .is_some_and(|owners| owners[position].is_some())
                {
                    if let Some(hash) = hash {
                        if Self::put_item_tx_hash(item)? != *hash {
                            return Err(StoreError::OperationOriginMismatch);
                        }
                    }
                }
                hash.clone().map_or_else(
                    || {
                        #[cfg(test)]
                        {
                            input_reencodings += 1;
                        }
                        Self::put_item_tx(&tx, item)
                    },
                    Ok,
                )
            })
            .collect::<Result<_>>()?;
        let instructions = match instructions {
            Some(hash) => hash.clone(),
            None => Self::put_item_tx(
                &tx,
                &Item(serde_json::Value::String(request.instructions.clone())),
            )?,
        };
        let (tools, json) = request.tools.encoded();
        let present: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM items WHERE hash=?1)",
            [&tools.0],
            |row| row.get(0),
        )?;
        if !present {
            tx.execute(
                "INSERT INTO items(hash,json) VALUES (?1,?2)",
                params![tools.0, json],
            )?;
        }
        let issued = IssuedReplayRequest {
            record: IssuedRequestRecord {
                input,
                outputs,
                instructions,
                tools: tools.clone(),
                tools_allowed: request.tools_allowed.clone(),
                model: request.model.clone(),
                pinned_effort: request.pinned_effort,
                session_id: request.session_id.clone(),
            },
            #[cfg(test)]
            input_reencodings,
        };
        tx.commit()?;
        Ok(issued)
    }

    /// Completion and references commit atomically; no request payload is copied.
    pub fn record_issued_replay_turn(
        &self,
        request: &RequestId,
        issued: IssuedReplayRequest,
        response: &ResponsesTurn,
    ) -> Result<i64> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let items = response
            .items
            .iter()
            .map(|item| Self::put_item_tx(&tx, item))
            .collect::<Result<_>>()?;
        let record = ReplayRecord {
            format: FORMAT,
            request: request.clone(),
            issued: issued.record,
            completion: None,
            response: ReplayResponse {
                response_id: response.response_id.clone(),
                items,
                usage: response.usage.clone(),
            },
        };
        let payload = serde_json::to_string(&record)?;
        tx.execute(
            "INSERT INTO events(request_id,kind,payload,created_at) VALUES (?1,'model_turn',?2,?3)",
            params![request.0, payload, utc_millis()],
        )?;
        let sequence = tx.last_insert_rowid();
        tx.commit()?;
        Ok(sequence)
    }

    /// Bind parser-validated Engine completion to the exact persisted response
    /// occurrence. Provider tools and copied item bytes cannot mint this marker.
    pub(crate) fn record_validated_finalize(
        &self,
        event: i64,
        request: &RequestId,
        completion: &ValidatedFinalize,
    ) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let payload: Option<String> = tx
            .query_row(
                "SELECT payload FROM events WHERE id=?1 AND request_id=?2 AND kind='model_turn'",
                params![event, request.0],
                |row| row.get(0),
            )
            .optional()?;
        let mut record: ReplayRecord =
            serde_json::from_str(&payload.ok_or(StoreError::InvalidCompletionMarker { event })?)?;
        let hash = Self::put_item_tx(&tx, completion.item())?;
        let mut query = tx.prepare(
            "SELECT position FROM request_items WHERE request_id=?1 AND item_hash=?2 AND (source_request IS NULL OR (source_request=request_id AND source_position=position))",
        )?;
        let positions = query
            .query_map(params![request.0, hash.0], |row| row.get::<_, i64>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let [position] = positions.as_slice() else {
            return Err(StoreError::InvalidCompletionMarker { event });
        };
        let origin = Origin {
            request: request.clone(),
            position: *position,
            hash,
        };
        let schema = Self::put_item_tx(&tx, &Item(completion.schema().clone()))?;
        let marker = CompletionMarker::Finalize { origin, schema };
        if record.completion.as_ref().is_some_and(|old| old != &marker) {
            return Err(StoreError::InvalidCompletionMarker { event });
        }
        record.completion = Some(marker);
        validate_completion_record(&tx, event, &record)?;
        tx.execute(
            "UPDATE events SET payload=?2 WHERE id=?1",
            params![event, serde_json::to_string(&record)?],
        )?;
        drop(query);
        tx.commit()?;
        Ok(())
    }

    pub(super) fn decode_replay_record(
        &self,
        event: i64,
        payload: &str,
    ) -> Result<RecordedReplayTurn> {
        let value: serde_json::Value = serde_json::from_str(payload)?;
        if value.get("format").and_then(serde_json::Value::as_u64) != Some(u64::from(FORMAT)) {
            return Err(StoreError::UnsupportedReplayFormat { event });
        }
        let record: ReplayRecord = serde_json::from_value(value)?;
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let instructions = read_item(&tx, &record.issued.instructions)?;
        let tools = read_item(&tx, &record.issued.tools)?;
        let request = ResponsesRequest {
            input: record
                .issued
                .input
                .iter()
                .map(|hash| read_item(&tx, hash))
                .collect::<Result<_>>()?,
            instructions: serde_json::from_value(instructions.0)?,
            tools: serde_json::from_value(tools.0)?,
            tools_allowed: record.issued.tools_allowed,
            model: record.issued.model,
            pinned_effort: record.issued.pinned_effort,
            session_id: record.issued.session_id,
        };
        if let Some(outputs) = &record.issued.outputs {
            validate_output_references(&tx, event, &request.input, outputs)?;
        }
        let response = ResponsesTurn {
            response_id: record.response.response_id,
            items: record
                .response
                .items
                .iter()
                .map(|hash| read_item(&tx, hash))
                .collect::<Result<_>>()?,
            usage: record.response.usage,
        };
        tx.commit()?;
        Ok(RecordedReplayTurn {
            request: record.request,
            model_request: request,
            model_response: response,
            issued_outputs: record.issued.outputs,
            replay_event: Some(event),
        })
    }
}

/// Context copies retain the immutable source occurrence, so the same sealed
/// response evidence authenticates a marker after a rewrite or frozen cut.
pub(super) fn is_validated_completion(c: &Connection, origin: &Origin) -> Result<bool> {
    let mut query = c.prepare(
        "SELECT id,payload FROM events WHERE request_id=?1 AND kind='model_turn' AND json_extract(payload,'$.completion.origin.position')=?2 AND json_extract(payload,'$.completion.origin.hash')=?3",
    )?;
    let records = query
        .query_map(
            params![origin.request.0, origin.position, origin.hash.0],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let [(event, payload)] = records.as_slice() else {
        return if records.is_empty() {
            Ok(false)
        } else {
            Err(StoreError::InvalidCompletionMarker {
                event: records[0].0,
            })
        };
    };
    let record: ReplayRecord = serde_json::from_str(payload)?;
    let validated = validate_completion_record(c, *event, &record)?;
    Ok(validated == *origin)
}

fn validate_completion_record(c: &Connection, event: i64, record: &ReplayRecord) -> Result<Origin> {
    let invalid = || StoreError::InvalidCompletionMarker { event };
    let Some(CompletionMarker::Finalize { origin, schema }) = &record.completion else {
        return Err(invalid());
    };
    if record.format != FORMAT || record.request != origin.request {
        return Err(invalid());
    }
    let exact: Option<String> = c.query_row(
        "SELECT item_hash FROM request_items WHERE request_id=?1 AND position=?2 AND (source_request IS NULL OR (source_request=request_id AND source_position=position))",
        params![origin.request.0, origin.position], |row| row.get(0),
    ).optional()?;
    if exact.as_deref() != Some(origin.hash.0.as_str()) {
        return Err(invalid());
    }
    let item = read_item(c, &origin.hash)?;
    let schema_item = read_item(c, schema)?;
    if ItemHash(
        blake3::hash(&serde_json::to_vec(&item)?)
            .to_hex()
            .to_string(),
    ) != origin.hash
        || ItemHash(
            blake3::hash(&serde_json::to_vec(&schema_item)?)
                .to_hex()
                .to_string(),
        ) != *schema
    {
        return Err(invalid());
    }
    let tools = read_item(c, &record.issued.tools)?;
    if tools
        .0
        .as_array()
        .is_none_or(|tools| tools.iter().filter(|tool| **tool == schema_item.0).count() != 1)
        || record
            .issued
            .tools_allowed
            .as_ref()
            .is_some_and(|allowed| !allowed.iter().any(|name| name == FINALIZE_TOOL_NAME))
    {
        return Err(invalid());
    }
    let calls = record
        .response
        .items
        .iter()
        .map(|hash| Ok((hash, read_item(c, hash)?)))
        .collect::<Result<Vec<_>>>()?;
    let finals = calls
        .iter()
        .filter(|(_, item)| {
            item.0["type"] == "function_call" && item.0["name"] == FINALIZE_TOOL_NAME
        })
        .collect::<Vec<_>>();
    let [final_call] = finals.as_slice() else {
        return Err(invalid());
    };
    if *final_call.0 != origin.hash || final_call.1 != item {
        return Err(invalid());
    }
    ValidatedFinalize::parse(&item, &schema_item.0).map_err(|_| invalid())?;
    Ok(origin.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Effort, transport::client::request_body};
    use serde_json::json;

    fn request(input: Vec<Item>) -> ResponsesRequest {
        ResponsesRequest {
            input,
            instructions: "exact immutable instructions".into(),
            tools: vec![json!({"type":"function","name":"a","strict":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false},"future":{"opaque":7}})]
                .into(),
            tools_allowed: Some(vec!["a".into()]),
            model: "offline".into(),
            pinned_effort: Effort::Medium,
            session_id: "session".into(),
        }
    }

    fn response(items: Vec<Item>) -> ResponsesTurn {
        ResponsesTurn {
            response_id: "response".into(),
            items,
            usage: Usage::default(),
        }
    }

    #[test]
    fn exact_issued_items_survive_late_output_and_reopen() {
        let path = std::env::temp_dir().join(format!(
            "harness-issued-replay-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let root = RequestId("root".into());
        let original = Item(json!({"type":"message","role":"user","content":"original"}));
        let projected = Item(
            json!({"type":"function_call_output","call_id":"same-id","output":"projected","unknown":{"keep":[1,2]}}),
        );
        let injected =
            Item(json!({"type":"message","role":"user","content":"ephemeral hook input"}));
        let input = request(vec![original.clone(), projected, injected]);
        let done = response(vec![Item(
            json!({"type":"custom_tool_call","name":"raw","call_id":"same-id","input":"☃\n\\\"","future_field":42}),
        )]);
        {
            let store = Store::open(&path).unwrap();
            store.create_request(&root, None, "/root").unwrap();
            store.append_items(&root, &[original]).unwrap();
            let sealed = store.seal_replay_request(&input).unwrap();
            store
                .append_items(
                    &root,
                    &[Item(
                        json!({"type":"message","role":"user","content":"late actual input"}),
                    )],
                )
                .unwrap();
            store
                .record_issued_replay_turn(&root, sealed, &done)
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        let replay = store.replay_turns(&root).unwrap();
        assert_eq!(replay.len(), 1);
        assert_eq!(
            request_body(&replay[0].model_request).unwrap(),
            request_body(&input).unwrap()
        );
        assert_eq!(replay[0].model_response.items, done.items);
        assert_eq!(store.items(&root).unwrap().len(), 2);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn old_replay_format_is_refused_without_changing_event_bytes() {
        let store = Store::memory().unwrap();
        let root = RequestId("root".into());
        store.create_request(&root, None, "/root").unwrap();
        let old = RecordedReplayTurn {
            request: root.clone(),
            model_request: request(vec![]),
            model_response: response(vec![]),
            issued_outputs: None,
            replay_event: None,
        };
        let sequence = store
            .record_event(
                Some(&root),
                "model_turn",
                &serde_json::to_value(old).unwrap(),
            )
            .unwrap();
        let before = store.events(Some(&root)).unwrap()[0].payload.clone();
        assert!(
            matches!(store.replay_turns(&root),Err(StoreError::UnsupportedReplayFormat { event }) if event==sequence)
        );
        assert_eq!(store.events(Some(&root)).unwrap()[0].payload, before);
    }

    #[test]
    fn replay_completion_failure_rolls_back_response_references() {
        let store = Store::memory().unwrap();
        let root = RequestId("root".into());
        store.create_request(&root, None, "/root").unwrap();
        let sealed = store.seal_replay_request(&request(vec![])).unwrap();
        store.lock().execute_batch("CREATE TRIGGER reject_replay BEFORE INSERT ON events WHEN NEW.kind='model_turn' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
        let output = Item(json!({"type":"message","role":"assistant","content":"new output"}));
        let hash = ItemHash(
            blake3::hash(&serde_json::to_vec(&output).unwrap())
                .to_hex()
                .to_string(),
        );
        assert!(
            store
                .record_issued_replay_turn(&root, sealed, &response(vec![output]))
                .is_err()
        );
        assert!(store.get_item(&hash).unwrap().is_none());
        assert!(store.events(Some(&root)).unwrap().is_empty());
    }

    #[test]
    fn replay_reference_bytes_at_two_history_sizes() {
        for count in [100, 1000] {
            let store = Store::memory().unwrap();
            let root = RequestId("root".into());
            store.create_request(&root, None, "/root").unwrap();
            let items = (0..count).map(|index| Item(json!({"type":"message","role":"user","content":format!("{index}:{}","x".repeat(4096))}))).collect::<Vec<_>>();
            store.append_items(&root, &items).unwrap();
            let hashes = store
                .items_with_hashes(&root)
                .unwrap()
                .into_iter()
                .map(|(hash, _)| Some(hash))
                .collect::<Vec<_>>();
            let issued = request(items);
            let instructions = store
                .intern_replay_instructions(&issued.instructions)
                .unwrap();
            let mut input_reencodings = 0;
            for _ in 0..4 {
                let sealed = store
                    .seal_replay_request_with_hashes(&issued, &hashes, Some(&instructions))
                    .unwrap();
                input_reencodings += sealed.input_reencodings;
                store
                    .record_issued_replay_turn(&root, sealed, &response(vec![]))
                    .unwrap();
            }
            let history_bytes = serde_json::to_vec(&issued.input).unwrap().len();
            let previous_event_bytes = serde_json::to_vec(&RecordedReplayTurn {
                request: root.clone(),
                model_request: issued.clone(),
                model_response: response(vec![]),
                issued_outputs: None,
                replay_event: None,
            })
            .unwrap()
            .len()
                * 4;
            let events = store.events(Some(&root)).unwrap();
            let replay_bytes = events
                .iter()
                .map(|event| event.payload.len())
                .sum::<usize>();
            assert!(replay_bytes < history_bytes / 8);
            assert_eq!(store.replay_turns(&root).unwrap().len(), 4);
            let stored_items: usize = store
                .lock()
                .query_row("SELECT COUNT(*) FROM items", [], |row| row.get(0))
                .unwrap();
            assert_eq!(stored_items, count + 2);
            assert_eq!(input_reencodings, 0);
            eprintln!(
                "harness-request-cost {}",
                json!({"history_items":count,"history_bytes":history_bytes,"rounds":4,"previous_full_payload_bytes":previous_event_bytes,"replay_event_bytes":replay_bytes,"stored_items":stored_items,"immutable_schema_bytes":issued.tools.encoded().1.len(),"unchanged_history_reencodings":input_reencodings})
            );
        }
    }
}

#[cfg(test)]
#[path = "replay/ownership_tests.rs"]
mod ownership_tests;
