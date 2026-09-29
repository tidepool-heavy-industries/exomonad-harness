use super::{StoreError, Result, invocation_kind, utc_millis};
use crate::{item::{Item, ToolKind}, model::{AgentPath, CallId, ConversationIdentity, OperationId, RequestId}};
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::{HashMap, HashSet};

pub(super) fn ensure_store_id(tx: &Transaction<'_>) -> Result<String> {
    let existing: Option<String> = tx.query_row(
        "SELECT state FROM session_state WHERE session_id='harness:store-id'", [], |r| r.get(0),
    ).optional()?;
    if let Some(id) = existing { return Ok(id); }
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute("INSERT INTO session_state(session_id,state,updated_at) VALUES ('harness:store-id',?1,?2)", params![id, utc_millis()])?;
    Ok(id)
}

fn ambiguous(request: &str, call: &str, reason: impl Into<String>) -> StoreError {
    StoreError::LegacyProvenance { request: request.into(), call_id: call.into(), reason: reason.into() }
}

fn source_claims(tx: &Transaction<'_>, source: &str, call: &str) -> Result<Vec<String>> {
    let mut q = tx.prepare(
        "WITH RECURSIVE lineage(id,parent_id) AS (
           SELECT id,parent_id FROM requests WHERE id=?1
           UNION ALL SELECT r.id,r.parent_id FROM requests r JOIN lineage l ON r.id=l.parent_id
         ) SELECT c.request_id FROM legacy_claims c JOIN lineage l ON l.id=c.request_id
         WHERE c.call_id=?2 ORDER BY c.request_id")?;
    Ok(q.query_map(params![source, call], |r| r.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()?)
}

fn copy_source(tx: &Transaction<'_>, request: &str) -> Result<Option<String>> {
    // Checkpoint snapshots and their first child requests are copies. The
    // checkpoint row is the durable authority for the original source head.
    let checkpoint_source: Option<String> = tx.query_row(
        "SELECT source_request FROM checkpoints WHERE snapshot_request=?1", [request], |r| r.get(0),
    ).optional()?;
    if checkpoint_source.is_some() { return Ok(checkpoint_source); }
    let parent: Option<String> = tx.query_row(
        "SELECT parent_id FROM requests WHERE id=?1", [request], |r| r.get(0),
    ).optional()?.flatten();
    if let Some(parent) = parent {
        let is_child: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM checkpoints WHERE snapshot_request=?1)", [&parent], |r| r.get(0),
        )?;
        if is_child { return Ok(Some(parent)); }
    }
    let fork: Option<String> = tx.query_row(
        "SELECT fork_source FROM agents WHERE head_request=?1", [request], |r| r.get(0),
    ).optional()?;
    if let Some(fork) = fork {
        let metadata: serde_json::Value = serde_json::from_str(&fork)?;
        if metadata["kind"] == "here" {
            return Ok(metadata["source_head_request"].as_str().map(str::to_owned));
        }
    }
    Ok(None)
}

fn resolve(
    tx: &Transaction<'_>, store_id: &str, request: &str, call: &str,
    cache: &mut HashMap<(String, String), OperationId>,
    active: &mut HashSet<(String, String)>,
) -> Result<OperationId> {
    let key = (request.to_owned(), call.to_owned());
    if let Some(found) = cache.get(&key) { return Ok(found.clone()); }
    if !active.insert(key.clone()) {
        return Err(ambiguous(request, call, "cyclic copied-claim ancestry"));
    }
    let result = if let Some(source) = copy_source(tx, request)? {
        let copied_kind = invocation_kind(tx, &RequestId(request.into()), &CallId(call.into()))
            .map_err(|e| ambiguous(request, call, e.to_string()))?;
        if copied_kind.is_none() {
            return Err(ambiguous(request, call, "copied request lacks the provider call item"));
        }
        let candidates = source_claims(tx, &source, call)?;
        if candidates.is_empty() {
            return Err(ambiguous(request, call, format!("source {source} has no matching claim")));
        }
        let mut origins = HashSet::new();
        for candidate in candidates {
            origins.insert(resolve(tx, store_id, &candidate, call, cache, active)?);
        }
        if origins.len() != 1 {
            return Err(ambiguous(request, call, "source lineage has multiple same-ID operations"));
        }
        origins.into_iter().next().unwrap()
    } else {
        let origin_request = RequestId(request.into());
        if invocation_kind(tx, &origin_request, &CallId(call.into()))
            .map_err(|e| ambiguous(request, call, e.to_string()))?.is_none() {
            return Err(ambiguous(request, call, "claim request has no matching original provider call"));
        }
        let branch: String = tx.query_row("SELECT branch FROM requests WHERE id=?1", [request], |r| r.get(0))?;
        let binding: Option<(String, String)> = tx.query_row(
            "SELECT run_id,incarnation FROM embedded_bindings WHERE agent_path=?1",
            [&branch], |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?;
        let origin = match binding {
            Some((run, incarnation)) => ConversationIdentity::Embedded { run, actor: AgentPath(branch), incarnation },
            None => ConversationIdentity::Standalone { store: store_id.into(), actor: AgentPath(branch) },
        };
        OperationId { origin, request: origin_request, call: CallId(call.into()) }
    };
    active.remove(&key);
    cache.insert(key, result.clone());
    Ok(result)
}

pub(super) fn migrate_claims(tx: &Transaction<'_>, store_id: &str) -> Result<()> {
    let claims: Vec<(String, String, String, Option<String>)> = {
        let mut q = tx.prepare("SELECT call_id,request_id,state,output_hash FROM legacy_claims ORDER BY request_id,call_id")?;
        q.query_map([], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?
            .collect::<std::result::Result<_,_>>()?
    };
    let mut cache = HashMap::new();
    for (call, request, state, output_hash) in claims {
        let operation = resolve(tx, store_id, &request, &call, &mut cache, &mut HashSet::new())?;
        if let Some(hash) = &output_hash {
            let raw: String = tx.query_row("SELECT json FROM items WHERE hash=?1", [hash], |r| r.get(0))?;
            let item: Item = serde_json::from_str(&raw)?;
            let kind = invocation_kind(tx, &operation.request, &operation.call)
                .map_err(|e| ambiguous(&request, &call, e.to_string()))?
                .ok_or_else(|| ambiguous(&request, &call, "origin invocation is missing"))?;
            let expected = match kind { ToolKind::Function => "function_call_output", ToolKind::Custom => "custom_tool_call_output" };
            if item.0["type"] != expected || item.0["call_id"] != call {
                return Err(ambiguous(&request, &call, "settled output does not match origin call kind and wire ID"));
            }
        }
        let origin = serde_json::to_string(&operation.origin)?;
        tx.execute(
            "INSERT INTO claims(origin,origin_request_id,call_id,request_id,state,output_hash) VALUES (?1,?2,?3,?4,?5,?6)",
            params![origin,operation.request.0,call,request,state,output_hash],
        )?;
    }
    Ok(())
}
