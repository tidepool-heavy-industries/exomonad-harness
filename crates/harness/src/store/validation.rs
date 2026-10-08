//! Exact issuing evidence remains Store-owned and independent of live reloads.
use super::{Result, Store, StoreError};
use crate::{
    context::Origin,
    item::{Item, ItemHash, ToolCall},
    model::{CallId, OperationId, RequestId},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};

pub(super) const INDEXES: &str = "
CREATE INDEX IF NOT EXISTS items_call_identity ON items(json_extract(json,'$.call_id'))
 WHERE json_extract(json,'$.type') IN ('function_call','custom_tool_call');
CREATE INDEX IF NOT EXISTS request_items_item_request ON request_items(item_hash,request_id,position);
CREATE INDEX IF NOT EXISTS events_request_kind ON events(request_id,kind,id DESC);
";

const INVOCATION: &str = "SELECT i.json FROM items i INDEXED BY items_call_identity
 CROSS JOIN request_items ri INDEXED BY request_items_item_request ON ri.item_hash=i.hash
 WHERE json_extract(i.json,'$.call_id')=?2
 AND json_extract(i.json,'$.type') IN ('function_call','custom_tool_call')
 AND ri.request_id=?1 ORDER BY ri.position";

pub(super) fn invocation_item(
    c: &Connection,
    request: &RequestId,
    call: &CallId,
) -> Result<Option<ToolCall>> {
    let mut query = c.prepare(INVOCATION)?;
    let rows = query.query_map(params![request.0, call.0], |row| row.get::<_, String>(0))?;
    let mut found = None;
    for raw in rows {
        let item: Item = serde_json::from_str(&raw?)?;
        let parsed = item
            .tool_call()
            .map_err(|reason| StoreError::MalformedReplayCall {
                request: request.0.clone(),
                call_id: call.0.clone(),
                reason: reason.into(),
            })?
            .ok_or_else(|| StoreError::MalformedReplayCall {
                request: request.0.clone(),
                call_id: call.0.clone(),
                reason: "missing invocation".into(),
            })?;
        if found.replace(parsed).is_some() {
            return Err(StoreError::AmbiguousReplayCall {
                call_id: call.0.clone(),
            });
        }
    }
    Ok(found)
}

pub(crate) struct IssuedInvocation {
    pub call: ToolCall,
    pub occurrence: Origin,
}

/// Read immutable issuance for exactly the requested operations in bounded batches.
/// Equal wire IDs in different issuing requests remain separate ledger entries.
pub(super) fn invocations_for_operations(
    c: &Connection,
    operations: &HashSet<OperationId>,
) -> Result<HashMap<OperationId, IssuedInvocation>> {
    let requests = operations
        .iter()
        .map(|operation| operation.request.0.as_str())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut ledger = HashMap::new();
    for batch in requests.chunks(250) {
        let placeholders = std::iter::repeat_n("?", batch.len())
            .collect::<Vec<_>>()
            .join(",");
        let mut query = c.prepare(&format!("SELECT c.origin,c.origin_request_id,c.call_id,ri.position,ri.item_hash,i.json FROM claims c JOIN request_items ri ON ri.request_id=c.origin_request_id JOIN items i ON i.hash=ri.item_hash WHERE c.request_id=c.origin_request_id AND c.origin_request_id IN ({placeholders}) AND json_extract(i.json,'$.call_id')=c.call_id AND json_extract(i.json,'$.type') IN ('function_call','custom_tool_call') ORDER BY c.origin_request_id,ri.position"))?;
        let rows = query.query_map(rusqlite::params_from_iter(batch.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        for row in rows {
            let (origin, request, call_id, position, hash, raw) = row?;
            let operation = OperationId {
                origin: serde_json::from_str(&origin)?,
                request: RequestId(request),
                call: CallId(call_id),
            };
            if !operations.contains(&operation) {
                continue;
            }
            let item: Item = serde_json::from_str(&raw)?;
            let call = item
                .tool_call()
                .map_err(|reason| StoreError::MalformedReplayCall {
                    request: operation.request.0.clone(),
                    call_id: operation.call.0.clone(),
                    reason: reason.into(),
                })?
                .ok_or_else(|| StoreError::MissingCheckpointCall {
                    request: operation.request.0.clone(),
                    call_id: operation.call.0.clone(),
                })?;
            let invocation = IssuedInvocation {
                call,
                occurrence: Origin {
                    request: operation.request.clone(),
                    position,
                    hash: ItemHash(hash),
                },
            };
            if ledger.insert(operation.clone(), invocation).is_some() {
                return Err(StoreError::AmbiguousReplayCall {
                    call_id: operation.call.0,
                });
            }
        }
    }
    Ok(ledger)
}

impl Store {
    pub fn invocation_item(&self, request: &RequestId, call: &CallId) -> Result<Option<ToolCall>> {
        invocation_item(&self.lock(), request, call)
    }

    /// Historical origin is owned by the original claim, never today's binding.
    pub fn recorded_operation_for_request(
        &self,
        request: &RequestId,
        call: &CallId,
    ) -> Result<Option<OperationId>> {
        let c = self.lock();
        if invocation_item(&c, request, call)?.is_none() {
            return Ok(None);
        }
        let mut query = c.prepare(
            "SELECT origin FROM claims WHERE origin_request_id=?1 AND request_id=?1 AND call_id=?2 ORDER BY origin",
        )?;
        let origins = query
            .query_map(params![request.0, call.0], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        match origins.as_slice() {
            [] => Ok(None),
            [origin] => Ok(Some(OperationId {
                origin: serde_json::from_str(origin)?,
                request: request.clone(),
                call: call.clone(),
            })),
            _ => Err(StoreError::AmbiguousReplayCall {
                call_id: call.0.clone(),
            }),
        }
    }

    pub(crate) fn select_operation_occurrences(
        &self,
        history: &[Option<crate::context::Occurrence>],
        operations: &[OperationId],
    ) -> Result<Vec<(usize, crate::context::Occurrence)>> {
        let selected = operations.iter().cloned().collect::<HashSet<_>>();
        if selected.len() != operations.len() {
            return Err(StoreError::DuplicateClaim);
        }
        let ledger = invocations_for_operations(&self.lock(), &selected)?;
        operations
            .iter()
            .map(|operation| {
                let invocation =
                    ledger
                        .get(operation)
                        .ok_or_else(|| StoreError::MissingCheckpointCall {
                            request: operation.request.0.clone(),
                            call_id: operation.call.0.clone(),
                        })?;
                let matches = history
                    .iter()
                    .enumerate()
                    .filter_map(|(index, occurrence)| {
                        occurrence
                            .as_ref()
                            .filter(|occurrence| occurrence.origin == invocation.occurrence)
                            .map(|occurrence| (index, occurrence.clone()))
                    })
                    .collect::<Vec<_>>();
                let [selected] = matches.as_slice() else {
                    return Err(StoreError::AmbiguousReplayCall {
                        call_id: operation.call.0.clone(),
                    });
                };
                Ok(selected.clone())
            })
            .collect()
    }

    pub fn latest_tool_surface(&self, request: &RequestId) -> Result<Option<serde_json::Value>> {
        let payload: Option<String> = self.lock().query_row(
            "SELECT payload FROM events WHERE request_id=?1 AND kind='tool_surface' ORDER BY id DESC LIMIT 1",
            [&request.0], |row| row.get(0),
        ).optional()?;
        payload
            .map(|payload| serde_json::from_str(&payload).map_err(Into::into))
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ToolInput;
    use serde_json::json;

    fn root(store: &Store) -> RequestId {
        let root = RequestId("root".into());
        store.create_request(&root, None, "/root").unwrap();
        root
    }

    #[test]
    fn recorded_origin_survives_current_embedded_rebind() {
        use crate::{
            embedding::HostIdentity,
            model::{AgentPath, ConversationIdentity},
        };
        use std::collections::{HashMap, HashSet};
        let store = Store::memory().unwrap();
        let identity = HostIdentity {
            run: "old-run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "old".into(),
        };
        store.bind_embedded_actor(&identity, None).unwrap();
        let request = root(&store);
        let call = CallId("recorded".into());
        store.append_items(&request,&[Item(json!({"type":"function_call","call_id":call.0,"name":"probe","arguments":"{}"}))]).unwrap();
        let original = store.claim(&call, &request).unwrap();
        store.lock().execute("UPDATE embedded_bindings SET run_id='new-run',incarnation='new' WHERE agent_path='/root'",[]).unwrap();
        assert_ne!(
            store.operation_for_request(&request, &call).unwrap(),
            original
        );
        assert_eq!(
            store
                .recorded_operation_for_request(&request, &call)
                .unwrap(),
            Some(original.clone())
        );
        assert_eq!(
            original.origin,
            ConversationIdentity::Embedded {
                run: identity.run,
                actor: identity.actor,
                incarnation: identity.incarnation,
            }
        );
    }

    #[test]
    fn recorded_origin_refuses_competing_original_claimants() {
        let store = Store::memory().unwrap();
        let request = root(&store);
        let call = CallId("recorded".into());
        store.append_items(&request,&[Item(json!({"type":"function_call","call_id":call.0,"name":"probe","arguments":"{}"}))]).unwrap();
        store.claim(&call, &request).unwrap();
        let foreign = store.standalone_identity(crate::model::AgentPath("/foreign".into()));
        store.lock().execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state) VALUES(?1,?2,?3,?2,'pending')",params![serde_json::to_string(&foreign).unwrap(),request.0,call.0]).unwrap();
        assert!(matches!(
            store.recorded_operation_for_request(&request, &call),
            Err(StoreError::AmbiguousReplayCall { .. })
        ));
    }

    #[test]
    fn exact_call_lookup_preserves_raw_input_and_ignores_unrelated_items() {
        let store = Store::memory().unwrap();
        let root = root(&store);
        let raw = "raw ☃\n\"\\";
        store.append_items(&root,&[
            Item(json!({"type":"message","role":"user","content":"x".repeat(1024*1024)})),
            Item(json!({"type":"custom_tool_call","call_id":"wanted","name":"raw","input":raw,"unknown":true})),
            Item(json!({"type":"function_call","call_id":"other","name":"malformed"})),
        ]).unwrap();
        let call = store
            .invocation_item(&root, &CallId("wanted".into()))
            .unwrap()
            .unwrap();
        assert_eq!(call.input, ToolInput::Custom(raw.into()));
        let plan = store
            .lock()
            .prepare(&format!("EXPLAIN QUERY PLAN {INVOCATION}"))
            .unwrap()
            .query_map(params![root.0, "wanted"], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plan.iter().any(|line| line.contains("items_call_identity")),
            "{plan:?}"
        );
        assert!(
            plan.iter()
                .any(|line| line.contains("request_items_item_request")),
            "{plan:?}"
        );
    }

    #[test]
    fn exact_call_lookup_refuses_duplicate_and_malformed_matching_calls() {
        let store = Store::memory().unwrap();
        let root = root(&store);
        store
            .append_items(
                &root,
                &[
                    Item(
                        json!({"type":"function_call","call_id":"dup","name":"a","arguments":"{}"}),
                    ),
                    Item(
                        json!({"type":"custom_tool_call","call_id":"dup","name":"b","input":"raw"}),
                    ),
                    Item(json!({"type":"custom_tool_call","call_id":"bad","name":"raw"})),
                ],
            )
            .unwrap();
        assert!(matches!(
            store.invocation_item(&root, &CallId("dup".into())),
            Err(StoreError::AmbiguousReplayCall { .. })
        ));
        assert!(matches!(
            store.invocation_item(&root, &CallId("bad".into())),
            Err(StoreError::MalformedReplayCall { .. })
        ));
        assert!(
            store
                .invocation_item(&root, &CallId("missing".into()))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn exact_surface_lookup_selects_latest_without_loading_model_turn() {
        let store = Store::memory().unwrap();
        let root = root(&store);
        store
            .record_event(Some(&root), "tool_surface", &json!({"version":"old"}))
            .unwrap();
        store
            .record_event(
                Some(&root),
                "model_turn",
                &json!({"unrelated":"x".repeat(1024*1024)}),
            )
            .unwrap();
        store
            .record_event(Some(&root), "tool_surface", &json!({"version":"new"}))
            .unwrap();
        assert_eq!(
            store.latest_tool_surface(&root).unwrap(),
            Some(json!({"version":"new"}))
        );
        let other = RequestId("other".into());
        assert!(store.latest_tool_surface(&other).unwrap().is_none());
    }
}
