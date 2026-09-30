use super::{Result, StoreError, invocation_kind, utc_millis};
use crate::{
    item::{Item, ToolKind},
    model::{AgentPath, CallId, ConversationIdentity, OperationId, RequestId},
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::{HashMap, HashSet};

pub(super) fn ensure_store_id(tx: &Transaction<'_>) -> Result<String> {
    let existing: Option<String> = tx
        .query_row(
            "SELECT state FROM session_state WHERE session_id='harness:store-id'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO session_state(session_id,state,updated_at) VALUES ('harness:store-id',?1,?2)",
        params![id, utc_millis()],
    )?;
    Ok(id)
}

fn ambiguous(request: &str, call: &str, reason: impl Into<String>) -> StoreError {
    StoreError::LegacyProvenance {
        request: request.into(),
        call_id: call.into(),
        reason: reason.into(),
    }
}

fn source_claims(
    tx: &Transaction<'_>,
    source: &str,
    call: &str,
    lineage: bool,
) -> Result<Vec<String>> {
    if !lineage {
        let exact: Option<String> = tx
            .query_row(
                "SELECT request_id FROM legacy_claims WHERE request_id=?1 AND call_id=?2",
                params![source, call],
                |r| r.get(0),
            )
            .optional()?;
        return Ok(exact.into_iter().collect());
    }
    let mut q = tx.prepare(
        "WITH RECURSIVE lineage(id,parent_id) AS (
           SELECT id,parent_id FROM requests WHERE id=?1
           UNION ALL SELECT r.id,r.parent_id FROM requests r JOIN lineage l ON r.id=l.parent_id
         ) SELECT c.request_id FROM legacy_claims c JOIN lineage l ON l.id=c.request_id
         WHERE c.call_id=?2 ORDER BY c.request_id",
    )?;
    Ok(q.query_map(params![source, call], |r| r.get(0))?
        .collect::<std::result::Result<Vec<String>, _>>()?)
}

struct CopySource {
    request: String,
    requires_call_item: bool,
}

fn copy_source(tx: &Transaction<'_>, request: &str, call: &str) -> Result<Option<CopySource>> {
    // Checkpoint snapshots and their first child requests are copies. The
    // checkpoint row is the durable authority for the original source head.
    let checkpoint_source: Option<String> = tx
        .query_row(
            "SELECT source_request FROM checkpoints WHERE snapshot_request=?1",
            [request],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(source) = checkpoint_source {
        return Ok(Some(CopySource {
            request: source,
            requires_call_item: true,
        }));
    }
    let parent: Option<String> = tx
        .query_row(
            "SELECT parent_id FROM requests WHERE id=?1",
            [request],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if let Some(parent) = parent {
        let is_child: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM checkpoints WHERE snapshot_request=?1)",
            [&parent],
            |r| r.get(0),
        )?;
        if is_child {
            return Ok(Some(CopySource {
                request: parent,
                requires_call_item: false,
            }));
        }
    }
    let fork: Option<String> = tx
        .query_row(
            "SELECT a.fork_source FROM agents a JOIN requests r ON r.branch=a.path WHERE r.id=?1",
            [request],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(fork) = fork {
        let metadata: serde_json::Value = serde_json::from_str(&fork)?;
        if metadata["kind"] == "here" && metadata["snapshot_request"] == request {
            return metadata["source_head_request"]
                .as_str()
                .map(str::to_owned)
                .map(|source| {
                    Some(CopySource {
                        request: source,
                        requires_call_item: true,
                    })
                })
                .ok_or_else(|| ambiguous(request, call, "Here copy has no source head request"));
        }
    }
    Ok(None)
}

fn resolve(
    tx: &Transaction<'_>,
    store_id: &str,
    request: &str,
    call: &str,
    cache: &mut HashMap<(String, String), OperationId>,
    active: &mut HashSet<(String, String)>,
) -> Result<OperationId> {
    let key = (request.to_owned(), call.to_owned());
    if let Some(found) = cache.get(&key) {
        return Ok(found.clone());
    }
    if !active.insert(key.clone()) {
        return Err(ambiguous(request, call, "cyclic copied-claim ancestry"));
    }
    let result = if let Some(source) = copy_source(tx, request, call)? {
        if source.requires_call_item {
            let copied_kind = invocation_kind(tx, &RequestId(request.into()), &CallId(call.into()))
                .map_err(|e| ambiguous(request, call, e.to_string()))?;
            if copied_kind.is_none() {
                return Err(ambiguous(
                    request,
                    call,
                    "copied request lacks the provider call item",
                ));
            }
        }
        let candidates = source_claims(tx, &source.request, call, source.requires_call_item)?;
        if candidates.is_empty() {
            return Err(ambiguous(
                request,
                call,
                format!("source {} has no matching claim", source.request),
            ));
        }
        let mut origins = HashSet::new();
        for candidate in candidates {
            origins.insert(resolve(tx, store_id, &candidate, call, cache, active)?);
        }
        if origins.len() != 1 {
            return Err(ambiguous(
                request,
                call,
                "source lineage has multiple same-ID operations",
            ));
        }
        origins.into_iter().next().unwrap()
    } else {
        let origin_request = RequestId(request.into());
        if invocation_kind(tx, &origin_request, &CallId(call.into()))
            .map_err(|e| ambiguous(request, call, e.to_string()))?
            .is_none()
        {
            return Err(ambiguous(
                request,
                call,
                "claim request has no matching original provider call",
            ));
        }
        let branch: String =
            tx.query_row("SELECT branch FROM requests WHERE id=?1", [request], |r| {
                r.get(0)
            })?;
        let binding: Option<(String, String)> = tx
            .query_row(
                "SELECT run_id,incarnation FROM embedded_bindings WHERE agent_path=?1",
                [&branch],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let origin = match binding {
            Some((run, incarnation)) => ConversationIdentity::Embedded {
                run,
                actor: AgentPath(branch),
                incarnation,
            },
            None => ConversationIdentity::Standalone {
                store: store_id.into(),
                actor: AgentPath(branch),
            },
        };
        OperationId {
            origin,
            request: origin_request,
            call: CallId(call.into()),
        }
    };
    active.remove(&key);
    cache.insert(key, result.clone());
    Ok(result)
}

pub(super) fn migrate_claims(tx: &Transaction<'_>, store_id: &str) -> Result<()> {
    let claims: Vec<(String, String, String, Option<String>)> = {
        let mut q = tx.prepare("SELECT call_id,request_id,state,output_hash FROM legacy_claims ORDER BY request_id,call_id")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<std::result::Result<_, _>>()?
    };
    let mut cache = HashMap::new();
    for (call, request, state, output_hash) in claims {
        let operation = resolve(
            tx,
            store_id,
            &request,
            &call,
            &mut cache,
            &mut HashSet::new(),
        )?;
        if let Some(hash) = &output_hash {
            let raw: String =
                tx.query_row("SELECT json FROM items WHERE hash=?1", [hash], |r| r.get(0))?;
            let item: Item = serde_json::from_str(&raw)?;
            let kind = invocation_kind(tx, &operation.request, &operation.call)
                .map_err(|e| ambiguous(&request, &call, e.to_string()))?
                .ok_or_else(|| ambiguous(&request, &call, "origin invocation is missing"))?;
            let expected = match kind {
                ToolKind::Function => "function_call_output",
                ToolKind::Custom => "custom_tool_call_output",
            };
            if item.0["type"] != expected || item.0["call_id"] != call {
                return Err(ambiguous(
                    &request,
                    &call,
                    "settled output does not match origin call kind and wire ID",
                ));
            }
        }
        let origin = serde_json::to_string(&operation.origin)?;
        tx.execute(
            "INSERT INTO claims(origin,origin_request_id,call_id,request_id,state,output_hash) VALUES (?1,?2,?3,?4,?5,?6)",
            params![origin,operation.request.0,call,request,state,output_hash],
        )?;
    }
    // Checkpoint records retain the pending operations that a reusable
    // attachment would have inherited. Upgrade their archived shape in the
    // same transaction as the claim rows.
    let checkpoints: Vec<(String, String)> = {
        let mut q = tx.prepare("SELECT id,pending_claims FROM checkpoints")?;
        q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?
    };
    for (id, raw) in checkpoints {
        let old: Vec<serde_json::Value> = serde_json::from_str(&raw)?;
        let mut updated = Vec::new();
        for value in old {
            let call = value["call_id"].as_str().ok_or_else(|| {
                ambiguous(&id, "<unknown>", "checkpoint pending call has no wire ID")
            })?;
            let request = value["request"].as_str().ok_or_else(|| {
                ambiguous(&id, call, "checkpoint pending call has no source request")
            })?;
            let (origin, original_request): (String, String) = tx.query_row(
                "SELECT origin,origin_request_id FROM claims WHERE request_id=?1 AND call_id=?2",
                params![request, call], |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?.ok_or_else(|| ambiguous(&id, call, "checkpoint pending call has no migrated source claim"))?;
            let kind: ToolKind = serde_json::from_value(value["kind"].clone())?;
            updated.push(crate::checkpoint::CheckpointClaim {
                operation: OperationId {
                    origin: serde_json::from_str(&origin)?,
                    request: RequestId(original_request),
                    call: CallId(call.into()),
                },
                request: RequestId(request.into()),
                kind,
            });
        }
        tx.execute(
            "UPDATE checkpoints SET pending_claims=?1 WHERE id=?2",
            params![serde_json::to_string(&updated)?, id],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::schema;
    use rusqlite::Connection;

    fn legacy_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        schema::initialize(&mut conn).unwrap();
        conn.execute_batch("DROP TABLE claims; DROP INDEX IF EXISTS claims_request; DROP INDEX IF EXISTS claims_operation;
            CREATE TABLE claims(call_id TEXT NOT NULL,request_id TEXT NOT NULL,state TEXT NOT NULL,output_hash TEXT,PRIMARY KEY(call_id,request_id));
            CREATE INDEX claims_request ON claims(request_id,state);
            UPDATE schema_version SET version=4;
            INSERT INTO agents(path,parent_path,head_request,contract,fork_source,state,created_at)
            VALUES('/root',NULL,NULL,'{}','{}','active',1);") .unwrap();
        conn
    }

    fn request(conn: &Connection, id: &str, parent: Option<&str>) {
        conn.execute(
            "INSERT INTO requests(id,parent_id,branch) VALUES(?1,?2,'/root')",
            params![id, parent],
        )
        .unwrap();
    }

    fn call(conn: &Connection, request: &str) {
        let hash = format!("call-{request}");
        let item = serde_json::json!({"type":"function_call","call_id":"same","name":"echo","arguments":{}}).to_string();
        conn.execute(
            "INSERT INTO items(hash,json) VALUES(?1,?2)",
            params![hash, item],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO request_items(request_id,position,item_hash) VALUES(?1,0,?2)",
            params![request, hash],
        )
        .unwrap();
    }

    fn claim(conn: &Connection, request: &str) {
        conn.execute(
            "INSERT INTO claims(call_id,request_id,state) VALUES('same',?1,'pending')",
            [request],
        )
        .unwrap();
    }

    #[test]
    fn schema_four_claims_keep_distinct_original_requests_and_checkpoint_inheritance() {
        let mut conn = legacy_db();
        request(&conn, "first", None);
        request(&conn, "second", Some("first"));
        request(&conn, "snapshot", Some("second"));
        call(&conn, "first");
        call(&conn, "second");
        call(&conn, "snapshot");
        claim(&conn, "first");
        claim(&conn, "second");
        // This checkpoint was captured from the first request, so its copied
        // claim must retain that origin despite an equal wire ID later.
        conn.execute("INSERT INTO checkpoints(id,origin_agent,source_request,snapshot_request,boundary_call,metadata,pending_claims,created_at)
            VALUES('checkpoint','/root','first','snapshot','same','{}',?1,1)",
            [r#"[{"call_id":"same","request":"first","kind":"function"}]"#]).unwrap();
        claim(&conn, "snapshot");
        conn.execute("INSERT INTO requests(id,parent_id,branch) VALUES('child-snapshot','snapshot','/root/child')", []).unwrap();
        claim(&conn, "child-snapshot");
        schema::initialize(&mut conn).unwrap();
        let records: Vec<(String, String, String)> = conn
            .prepare("SELECT origin_request_id,call_id,request_id FROM claims ORDER BY request_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            records,
            vec![
                ("first".into(), "same".into(), "child-snapshot".into()),
                ("first".into(), "same".into(), "first".into()),
                ("second".into(), "same".into(), "second".into()),
                ("first".into(), "same".into(), "snapshot".into())
            ]
        );
        let indexed: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name='claims_operation')", [], |r| r.get(0)).unwrap();
        assert!(indexed);
        let pending: String = conn
            .query_row(
                "SELECT pending_claims FROM checkpoints WHERE id='checkpoint'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let pending: Vec<crate::checkpoint::CheckpointClaim> =
            serde_json::from_str(&pending).unwrap();
        assert_eq!(pending[0].operation.request.0, "first");
    }

    #[test]
    fn ambiguous_legacy_checkpoint_rolls_back_schema_and_claims() {
        let mut conn = legacy_db();
        request(&conn, "first", None);
        request(&conn, "second", Some("first"));
        request(&conn, "snapshot", Some("second"));
        for id in ["first", "second", "snapshot"] {
            call(&conn, id);
            claim(&conn, id);
        }
        conn.execute("INSERT INTO checkpoints(id,origin_agent,source_request,snapshot_request,boundary_call,metadata,pending_claims,created_at)
            VALUES('checkpoint','/root','second','snapshot','same','{}','[]',1)", []).unwrap();
        assert!(matches!(
            schema::initialize(&mut conn),
            Err(StoreError::LegacyProvenance { .. })
        ));
        let version: u32 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM claims", [], |r| r.get(0))
            .unwrap();
        assert_eq!((version, count), (4, 3));
        assert!(conn.prepare("SELECT origin FROM claims").is_err());
    }

    #[test]
    fn here_copy_keeps_source_origin_after_child_head_advances() {
        let mut conn = legacy_db();
        request(&conn, "source", None);
        call(&conn, "source");
        claim(&conn, "source");
        conn.execute(
            "INSERT INTO requests(id,parent_id,branch) VALUES('here-snapshot',NULL,'/root/child')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO requests(id,parent_id,branch) VALUES('child-head','here-snapshot','/root/child')", []).unwrap();
        call(&conn, "here-snapshot");
        claim(&conn, "here-snapshot");
        let source = serde_json::json!({
            "kind":"here", "source_head_request":"source", "snapshot_request":"here-snapshot"
        })
        .to_string();
        conn.execute("INSERT INTO agents(path,parent_path,head_request,contract,fork_source,state,created_at)
            VALUES('/root/child','/root','child-head','{}',?1,'active',1)", [source]).unwrap();
        schema::initialize(&mut conn).unwrap();
        let original: String = conn
            .query_row(
                "SELECT origin_request_id FROM claims WHERE request_id='here-snapshot'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(original, "source");
    }
}

/// Schema 5 checkpoint metadata is host data; its cut is always deferred.
pub(super) fn migrate_checkpoint_metadata(tx: &Transaction<'_>) -> Result<()> {
    let rows: Vec<(String, String)> = {
        let mut q = tx.prepare("SELECT id,metadata FROM checkpoints")?;
        q.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?
    };
    for (id, raw) in rows {
        let host = serde_json::from_str(&raw)?;
        let metadata = crate::checkpoint::CheckpointMetadata::legacy(host);
        tx.execute(
            "UPDATE checkpoints SET metadata=?1 WHERE id=?2",
            params![serde_json::to_string(&metadata)?, id],
        )?;
    }
    Ok(())
}

pub(super) fn validate_checkpoint_metadata(conn: &rusqlite::Connection) -> Result<()> {
    let mut q = conn.prepare("SELECT metadata FROM checkpoints")?;
    let rows = q.query_map([], |row| row.get::<_, String>(0))?;
    for row in rows {
        crate::checkpoint::CheckpointMetadata::decode(&row?)?;
    }
    Ok(())
}
