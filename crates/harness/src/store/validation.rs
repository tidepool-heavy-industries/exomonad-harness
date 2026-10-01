//! Exact issuing evidence remains Store-owned and independent of live reloads.
use super::{Result, Store, StoreError};
use crate::{
    item::{Item, ToolCall},
    model::{CallId, RequestId},
};
use rusqlite::{Connection, OptionalExtension, params};

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

impl Store {
    pub fn invocation_item(&self, request: &RequestId, call: &CallId) -> Result<Option<ToolCall>> {
        invocation_item(&self.lock(), request, call)
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
