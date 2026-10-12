//! Server compaction provenance shares the boundary's atomic Store event.
use super::{
    Result, StoreError, context,
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
    pub(crate) provider_request: RequestId,
}

#[derive(Serialize, Deserialize)]
struct Record {
    source: RequestId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    evidence: Option<Evidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exact: Option<ExactEvidence>,
}

#[derive(Serialize, Deserialize)]
struct Evidence {
    format: u32,
    request: RequestId,
    model: String,
    raw_response: Vec<ItemHash>,
    origins: Vec<Origin>,
}

#[derive(Serialize, Deserialize)]
struct ExactEvidence {
    format: u32,
    provider_request: RequestId,
    selected: Vec<(usize, u64)>,
}

/// Exact provider indexes select raw occurrences; equal bytes never select an owner.
pub(super) fn generated_sources(
    c: &Connection,
    request: &RequestId,
    source: &RequestId,
    items: &[Item],
    selected: &[(usize, u64)],
    response: Option<&ServerCompactionResponse>,
) -> Result<Vec<(usize, crate::context::Occurrence)>> {
    let Some(response) = response else {
        return if selected.is_empty() {
            Ok(vec![])
        } else {
            Err(StoreError::InvalidProviderEvidence(request.clone()))
        };
    };
    let raw = raw_response(c, &response.provider_request, source)?;
    let expected = raw
        .iter()
        .enumerate()
        .filter(|(_, occurrence)| crate::compaction::server_keeps(&occurrence.item))
        .map(|(index, _)| index as u64)
        .collect::<Vec<_>>();
    if selected.iter().map(|(_, index)| *index).collect::<Vec<_>>() != expected {
        return Err(StoreError::InvalidProviderEvidence(
            response.provider_request.clone(),
        ));
    }
    let mut positions = HashSet::new();
    selected
        .iter()
        .map(|(position, index)| {
            let occurrence = raw.get(*index as usize).ok_or_else(|| {
                StoreError::InvalidProviderEvidence(response.provider_request.clone())
            })?;
            if !positions.insert(*position) || items.get(*position) != Some(&occurrence.item) {
                return Err(StoreError::InvalidProviderEvidence(
                    response.provider_request.clone(),
                ));
            }
            Ok((*position, occurrence.clone()))
        })
        .collect()
}

fn raw_response(
    c: &Connection,
    request: &RequestId,
    source: &RequestId,
) -> Result<Vec<crate::context::Occurrence>> {
    let (parent, issued, state): (Option<String>, String, String) = c.query_row("SELECT r.parent_id,p.issued_json,p.state FROM provider_requests p JOIN requests r ON r.id=p.request_id WHERE p.request_id=?1", [&request.0], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
    let issued: super::replay::IssuedRequestRecord = serde_json::from_str(&issued)?;
    if parent.as_deref() != Some(&source.0)
        || state != "completed"
        || issued.model.trim().is_empty()
        || issued.purpose != Some(super::ProviderRequestPurpose::ServerCompaction)
    {
        return Err(StoreError::InvalidProviderEvidence(request.clone()));
    }
    let mut occurrences = context::request_occurrences(c, request)?
        .into_iter()
        .map(|occurrence| (occurrence.position, occurrence))
        .collect::<HashMap<_, _>>();
    let mut query = c.prepare("SELECT output_index,occurrence_position,item_hash FROM provider_items WHERE request_id=?1 ORDER BY output_index")?;
    let indices = query
        .query_map([&request.0], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    indices
        .into_iter()
        .enumerate()
        .map(|(expected, (index, position, hash))| {
            let occurrence = occurrences
                .remove(&position)
                .ok_or_else(|| StoreError::InvalidProviderEvidence(request.clone()))?;
            if index != expected as u64
                || occurrence.hash.0 != hash
                || occurrence.origin.request != *request
                || occurrence.origin.position != position
            {
                return Err(StoreError::InvalidProviderEvidence(request.clone()));
            }
            Ok(occurrence)
        })
        .collect()
}

pub(super) fn record(
    tx: &Transaction<'_>,
    request: &RequestId,
    source: &RequestId,
    selected: &[(usize, u64)],
    response: Option<&ServerCompactionResponse>,
) -> Result<()> {
    let record = Record {
        source: source.clone(),
        evidence: None,
        exact: response.map(|response| ExactEvidence {
            format: 2,
            provider_request: response.provider_request.clone(),
            selected: selected.to_vec(),
        }),
    };
    tx.execute(
        "INSERT INTO events(request_id,kind,payload,created_at) VALUES (?1,'compaction',?2,?3)",
        params![request.0, serde_json::to_string(&record)?, utc_millis()],
    )?;
    if let Some(response) = response {
        tx.execute("INSERT INTO events(request_id,kind,payload,created_at) VALUES (?1,'compaction_selection',?2,?3)", params![response.provider_request.0,serde_json::to_string(request)?,utc_millis()])?;
    }
    Ok(())
}

/// Installed selections point back to their original, indexed response owner.
pub(super) fn source_response_envelope(
    c: &Connection,
    request: &RequestId,
) -> Result<Option<ResponseEnvelope>> {
    let mut query = c.prepare(
        "SELECT payload FROM events WHERE request_id=?1 AND kind='compaction_selection'",
    )?;
    let payloads = query
        .query_map([&request.0], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let [payload] = payloads.as_slice() else {
        return if payloads.is_empty() {
            Ok(None)
        } else {
            Err(StoreError::InvalidProviderEvidence(request.clone()))
        };
    };
    let boundary: RequestId = serde_json::from_str(payload)?;
    let record: String = c.query_row(
        "SELECT payload FROM events WHERE request_id=?1 AND kind='compaction'",
        [&boundary.0],
        |row| row.get(0),
    )?;
    let record: Record = serde_json::from_str(&record)?;
    let exact = record
        .exact
        .ok_or_else(|| StoreError::InvalidProviderEvidence(request.clone()))?;
    if exact.format != 2 || exact.provider_request != *request {
        return Err(StoreError::InvalidProviderEvidence(request.clone()));
    }
    let parent: Option<String> = c.query_row("SELECT parent_id FROM requests WHERE id=?1 AND EXISTS(SELECT 1 FROM session_state WHERE session_id='harness:compaction:'||?1)", [&boundary.0], |row| row.get(0))?;
    if parent.as_deref() != Some(&record.source.0) {
        return Err(StoreError::InvalidProviderEvidence(request.clone()));
    }
    let installed = context::request_occurrences(c, &boundary)?;
    let items = installed
        .iter()
        .map(|occurrence| occurrence.item.clone())
        .collect::<Vec<_>>();
    let selected = generated_sources(
        c,
        &boundary,
        &record.source,
        &items,
        &exact.selected,
        Some(&ServerCompactionResponse {
            provider_request: request.clone(),
        }),
    )?;
    for (position, occurrence) in &selected {
        if installed.get(*position).is_none_or(|actual| {
            actual.origin != occurrence.origin
                || actual.output_operation != occurrence.output_operation
                || !actual.overlays.is_empty()
                || actual.note
        }) {
            return Err(StoreError::InvalidProviderEvidence(request.clone()));
        }
    }
    let (issued, route, protocol): (String, String, String) = c.query_row(
        "SELECT issued_json,route,protocol FROM provider_requests WHERE request_id=?1",
        [&request.0],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let issued: super::replay::IssuedRequestRecord = serde_json::from_str(&issued)?;
    Ok(Some(ResponseEnvelope {
        model: issued.model,
        origins: selected
            .into_iter()
            .map(|(_, occurrence)| occurrence.origin)
            .collect(),
        kind: ResponseEnvelopeKind::ServerCompaction,
        completeness: super::replay::ResponseCompleteness::Completed,
        route: Some(serde_json::from_str(&route)?),
        protocol: Some(serde_json::from_str(&protocol)?),
    }))
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
        completeness: super::replay::ResponseCompleteness::Completed,
        route: None,
        protocol: None,
    }])
}

#[cfg(test)]
mod tests;
