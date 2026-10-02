//! Server compaction provenance shares the boundary's atomic Store event.
use super::{
    Result, Store, context,
    replay::{ResponseEnvelope, ResponseEnvelopeKind},
    utc_millis,
};
use crate::{
    context::Origin,
    item::{Item, ItemHash},
    model::RequestId,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const FORMAT: u32 = 1;

/// Captured from the actual transport request and unfiltered server response.
pub(crate) struct ServerCompactionResponse {
    pub(crate) model: String,
    pub(crate) items: Vec<Item>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    source: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    evidence: Option<Evidence>,
}

#[derive(Serialize, Deserialize)]
struct Evidence {
    format: u32,
    request: RequestId,
    model: String,
    raw_response: Vec<ItemHash>,
    origins: Vec<Origin>,
}

/// Only unambiguous fresh occurrences are attributed to this response. Equal
/// bytes reused from history keep their earlier provenance, including overflow
/// duplicates for which hash-based preservation could not establish an origin.
fn generated_origins(
    c: &Connection,
    request: &RequestId,
    source: &RequestId,
    raw_response: &[ItemHash],
) -> Result<Vec<Origin>> {
    let source_hashes = context::history(c, source, true)?
        .into_iter()
        .map(|item| item.hash)
        .collect::<HashSet<_>>();
    let boundary = context::request_occurrences(c, request)?;
    let mut raw_counts = HashMap::<&ItemHash, usize>::new();
    for hash in raw_response {
        *raw_counts.entry(hash).or_default() += 1;
    }
    let mut boundary_counts = HashMap::<&ItemHash, usize>::new();
    for occurrence in &boundary {
        *boundary_counts.entry(&occurrence.hash).or_default() += 1;
    }
    Ok(boundary
        .iter()
        .enumerate()
        .filter(|(index, occurrence)| {
            // Server inserts the opening notice and final effort pin itself.
            *index > 0
                && *index + 1 < boundary.len()
                && occurrence.origin.request == *request
                && occurrence.origin.position == occurrence.position
                && !source_hashes.contains(&occurrence.hash)
                && raw_counts.get(&occurrence.hash) == Some(&1)
                && boundary_counts.get(&occurrence.hash) == Some(&1)
                && !occurrence.item.is_configuration_update()
                && !(occurrence.item.0["type"] == "message" && occurrence.item.0["role"] == "user")
        })
        .map(|(_, occurrence)| occurrence.origin.clone())
        .collect())
}

pub(super) fn record(
    tx: &Transaction<'_>,
    request: &RequestId,
    source: &RequestId,
    response: Option<&ServerCompactionResponse>,
) -> Result<()> {
    let evidence = response
        .map(|response| {
            let raw_response = response
                .items
                .iter()
                .map(|item| Store::put_item_tx(tx, item))
                .collect::<Result<Vec<_>>>()?;
            let origins = generated_origins(tx, request, source, &raw_response)?;
            Ok::<_, super::StoreError>(Evidence {
                format: FORMAT,
                request: request.clone(),
                model: response.model.clone(),
                raw_response,
                origins,
            })
        })
        .transpose()?;
    let record = Record {
        source: source.clone(),
        evidence,
    };
    tx.execute(
        "INSERT INTO events(request_id,kind,payload,created_at) VALUES (?1,'compaction',?2,?3)",
        params![request.0, serde_json::to_string(&record)?, utc_millis()],
    )?;
    Ok(())
}

/// Legacy, malformed, and conflicting compaction events establish no model.
pub(super) fn response_envelopes(
    c: &Connection,
    request: &RequestId,
) -> Result<Vec<ResponseEnvelope>> {
    let mut query = c.prepare(
        "SELECT payload FROM events WHERE request_id=?1 AND kind='compaction' ORDER BY id",
    )?;
    let payloads = query
        .query_map([&request.0], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let [payload] = payloads.as_slice() else {
        return Ok(vec![]);
    };
    let Ok(record) = serde_json::from_str::<Record>(payload) else {
        return Ok(vec![]);
    };
    let Some(evidence) = record.evidence else {
        return Ok(vec![]);
    };
    if evidence.format != FORMAT
        || evidence.request != *request
        || evidence.model.trim().is_empty()
        || evidence.origins.is_empty()
    {
        return Ok(vec![]);
    }
    let boundary_source: Option<String> = c.query_row(
        "SELECT parent_id FROM requests WHERE id=?1 AND EXISTS(SELECT 1 FROM session_state WHERE session_id='harness:compaction:'||?1)",
        [&request.0], |row| row.get(0),
    ).optional()?.flatten();
    if boundary_source.as_deref() != Some(&record.source.0) {
        return Ok(vec![]);
    }
    // Raw response hashes refer to immutable, interned bytes even when Server
    // filtered those items out of the successor window.
    for hash in &evidence.raw_response {
        let json: Option<String> = c
            .query_row("SELECT json FROM items WHERE hash=?1", [&hash.0], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(json) = json else {
            return Ok(vec![]);
        };
        if blake3::hash(json.as_bytes()).to_hex().as_str() != hash.0 {
            return Ok(vec![]);
        }
    }
    // Membership was sealed at boundary installation. Later response items
    // appended to this request cannot invalidate it or enter this envelope.
    let mut seen = HashSet::new();
    for origin in &evidence.origins {
        if origin.request != *request
            || origin.position <= 0
            || !seen.insert(&origin.hash)
            || evidence
                .raw_response
                .iter()
                .filter(|hash| **hash == origin.hash)
                .count()
                != 1
        {
            return Ok(vec![]);
        }
        let installed: Option<(String, String, i64)> = c.query_row(
            "SELECT item_hash,COALESCE(source_request,request_id),COALESCE(source_position,position) FROM request_items WHERE request_id=?1 AND position=?2",
            params![request.0, origin.position],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).optional()?;
        if installed != Some((origin.hash.0.clone(), request.0.clone(), origin.position)) {
            return Ok(vec![]);
        }
    }
    Ok(vec![ResponseEnvelope {
        model: evidence.model,
        origins: evidence.origins,
        kind: ResponseEnvelopeKind::ServerCompaction,
    }])
}

#[cfg(test)]
mod tests;
