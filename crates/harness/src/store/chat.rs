//! Journal publication order. Body bytes stay in Items, never in reference events.
use super::{
    Result, Store, StoreError,
    actor_output::{self, ActorOutputOrigin, StoredActorOutput, validate_origin},
    forms::StoredActorForm,
    history::{MAX_HISTORY_BYTES, MAX_HISTORY_ITEMS},
};
use crate::{
    item::{Item, ItemHash},
    model::{CallId, OperationId, RequestId},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Debug, Serialize)]
pub struct ChatTool {
    pub operation: OperationId,
    pub invocation: ChatInvocation,
}
#[derive(Debug, Serialize)]
pub struct ChatInvocation {
    pub request: String,
    pub position: i64,
    pub hash: String,
}
pub(super) fn initialize_cutover(tx: &Transaction<'_>, legacy: bool) -> Result<()> {
    let sequence: i64 = if legacy {
        tx.query_row("SELECT COALESCE(MAX(id),0) FROM events", [], |r| r.get(0))?
    } else {
        0
    };
    tx.execute("INSERT OR IGNORE INTO session_state(session_id,state,updated_at) VALUES('harness:chat-cutover',?1,?2)",params![sequence.to_string(),super::utc_millis()])?;
    Ok(())
}
pub(super) fn publish(
    tx: &Transaction<'_>,
    request: &RequestId,
    position: i64,
    hash: &ItemHash,
    item: &Item,
) -> Result<()> {
    if item.is_configuration_update() {
        return Ok(());
    }
    tx.execute(
        "INSERT INTO events(request_id,kind,payload,created_at) VALUES(?1,'chat_message',?2,?3)",
        params![
            request.0,
            serde_json::to_string(&json!({"position":position,"hash":hash.0}))?,
            super::utc_millis()
        ],
    )?;
    Ok(())
}
#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ChatEntry {
    Message {
        sequence: i64,
        request_id: String,
        position: i64,
        hash: String,
        item: Value,
        tool: Option<ChatTool>,
    },
    Output {
        sequence: i64,
        output: StoredActorOutput,
    },
    Form {
        sequence: i64,
        form: StoredActorForm,
    },
    Oversized {
        sequence: i64,
        request_id: String,
        position: i64,
        hash: String,
        byte_len: usize,
    },
}
impl ChatEntry {
    fn sequence(&self) -> i64 {
        match self {
            Self::Message { sequence, .. }
            | Self::Output { sequence, .. }
            | Self::Form { sequence, .. }
            | Self::Oversized { sequence, .. } => *sequence,
        }
    }
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatPage {
    pub origin: ActorOutputOrigin,
    pub cutover_sequence: i64,
    pub legacy_history: bool,
    pub entries: Vec<ChatEntry>,
    pub next_after: Option<i64>,
    pub previous_before: Option<i64>,
}
#[derive(Clone, Copy)]
enum Cursor {
    After(i64),
    Before(Option<i64>),
}
impl Store {
    pub fn chat_page(
        &self,
        origin: &ActorOutputOrigin,
        head: Option<&RequestId>,
        after: i64,
        limit: usize,
    ) -> Result<ChatPage> {
        self.chat_page_at(origin, head, Cursor::After(after), limit)
    }
    /// Latest or strictly older publications, returned in publication order.
    pub fn chat_page_before(
        &self,
        origin: &ActorOutputOrigin,
        head: Option<&RequestId>,
        before: Option<i64>,
        limit: usize,
    ) -> Result<ChatPage> {
        self.chat_page_at(origin, head, Cursor::Before(before), limit)
    }
    fn chat_page_at(
        &self,
        origin: &ActorOutputOrigin,
        head: Option<&RequestId>,
        cursor: Cursor,
        limit: usize,
    ) -> Result<ChatPage> {
        validate_origin(origin)?;
        let boundary = match cursor {
            Cursor::After(value) => Some(value),
            Cursor::Before(value) => value,
        };
        if boundary.is_some_and(|value| value < 0) || limit == 0 || limit > MAX_HISTORY_ITEMS {
            return Err(StoreError::InvalidHistoryOffset);
        }
        let c = self.lock()?;
        if let Some(head) = head {
            let exists: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM requests WHERE id=?1)",
                [&head.0],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(StoreError::MissingRequest(head.0.clone()));
            }
        }
        let cutover:i64=c.query_row("SELECT CAST(state AS INTEGER) FROM session_state WHERE session_id='harness:chat-cutover'",[],|r|r.get(0))?;
        // Source occurrence edges are proven by the context owner, not content equality.
        let lineage = "WITH RECURSIVE lineage(id) AS (SELECT id FROM requests WHERE id=?1 UNION SELECT r.parent_id FROM requests r JOIN lineage l ON r.id=l.id WHERE r.parent_id IS NOT NULL UNION SELECT ri.source_request FROM request_items ri JOIN lineage l ON ri.request_id=l.id WHERE ri.source_request IS NOT NULL)";
        let legacy:bool=c.query_row(&format!("{lineage} SELECT EXISTS(SELECT 1 FROM request_items ri JOIN lineage l ON l.id=ri.request_id JOIN items i ON i.hash=ri.item_hash WHERE json_extract(i.json,'$.type')!='configuration_update' AND NOT EXISTS(SELECT 1 FROM events e WHERE e.kind='chat_message' AND e.request_id=COALESCE(ri.source_request,ri.request_id) AND json_extract(e.payload,'$.position')=COALESCE(ri.source_position,ri.position) AND json_extract(e.payload,'$.hash')=ri.item_hash))"),[head.map(|h|h.0.as_str())],|r|r.get(0))?;
        let (selection, order) = match cursor {
            Cursor::After(_) => ("e.id>?2", "ASC"),
            Cursor::Before(_) => ("(?2 IS NULL OR e.id<?2)", "DESC"),
        };
        let sql = format!(
            "{lineage} SELECT e.id,e.kind,e.request_id,e.payload,e.created_at FROM events e WHERE {selection} AND ((e.kind='chat_message' AND e.request_id IN (SELECT id FROM lineage)) OR (e.kind IN ('actor_output','actor_form_open') AND json_extract(e.payload,'$.origin.run')=?3 AND json_extract(e.payload,'$.origin.nativeActor')=?4 AND json_extract(e.payload,'$.origin.incarnation')=?5)) ORDER BY e.id {order} LIMIT ?6"
        );
        let mut q = c.prepare(&sql)?;
        let candidates = q
            .query_map(
                params![
                    head.map(|h| h.0.as_str()),
                    boundary,
                    origin.run,
                    origin.native_actor,
                    origin.incarnation,
                    limit + 1
                ],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut page = ChatPage {
            origin: origin.clone(),
            cutover_sequence: cutover,
            legacy_history: legacy,
            entries: vec![],
            next_after: None,
            previous_before: None,
        };
        for (sequence, kind, request, payload, created_at) in candidates {
            if page.entries.len() == limit {
                set_cursor(&mut page, cursor);
                break;
            }
            let entry = match kind.as_str() {
                "chat_message" => {
                    let reference: Value = serde_json::from_str(&payload)?;
                    let hash = reference["hash"]
                        .as_str()
                        .ok_or(StoreError::InvalidActorOutput)?
                        .to_owned();
                    let position = reference["position"]
                        .as_i64()
                        .ok_or(StoreError::InvalidActorOutput)?;
                    let raw: String =
                        c.query_row("SELECT json FROM items WHERE hash=?1", [&hash], |r| {
                            r.get(0)
                        })?;
                    let request_id = request.ok_or(StoreError::InvalidActorOutput)?;
                    if raw.len() > MAX_HISTORY_BYTES - 8192 {
                        ChatEntry::Oversized {
                            sequence,
                            request_id,
                            position,
                            hash,
                            byte_len: raw.len(),
                        }
                    } else {
                        ChatEntry::Message {
                            sequence,
                            request_id,
                            position,
                            hash,
                            item: serde_json::from_str(&raw)?,
                            tool: None,
                        }
                    }
                }
                "actor_output" => ChatEntry::Output {
                    sequence,
                    output: actor_output::row(sequence, payload, created_at)?,
                },
                "actor_form_open" => {
                    let raw: String = c.query_row(
                        "SELECT presentation FROM actor_forms WHERE opening_sequence=?1",
                        [sequence],
                        |r| r.get(0),
                    )?;
                    ChatEntry::Form {
                        sequence,
                        form: StoredActorForm::decode(&raw)?,
                    }
                }
                _ => unreachable!(),
            };
            page.entries.push(entry);
            if serde_json::to_vec(&page)?.len() > MAX_HISTORY_BYTES {
                page.entries.pop();
                set_cursor(&mut page, cursor);
                if page.entries.is_empty() {
                    return Err(StoreError::InvalidActorOutput);
                }
                break;
            }
        }
        if matches!(cursor, Cursor::Before(_)) {
            page.entries.reverse();
        }
        project_tools(&c, &mut page.entries)?;
        while serde_json::to_vec(&page)?.len() > MAX_HISTORY_BYTES {
            match cursor {
                Cursor::After(_) => {
                    page.entries.pop();
                }
                Cursor::Before(_) => {
                    page.entries.remove(0);
                }
            }
            if page.entries.is_empty() {
                return Err(StoreError::InvalidActorOutput);
            }
            match cursor {
                Cursor::After(_) => page.next_after = page.entries.last().map(ChatEntry::sequence),
                Cursor::Before(_) => {
                    page.previous_before = page.entries.first().map(ChatEntry::sequence)
                }
            }
        }
        Ok(page)
    }
}

fn set_cursor(page: &mut ChatPage, cursor: Cursor) {
    let sequence = page.entries.last().map(ChatEntry::sequence);
    match cursor {
        Cursor::After(_) => page.next_after = sequence,
        Cursor::Before(_) => page.previous_before = sequence,
    }
}

fn project_tools(c: &Connection, entries: &mut [ChatEntry]) -> Result<()> {
    let mut operations = Vec::with_capacity(entries.len());
    for entry in entries.iter() {
        let operation = if let ChatEntry::Message {
            request_id,
            position,
            hash,
            item,
            ..
        } = entry
        {
            match item["type"].as_str() {
                Some("function_call_output" | "custom_tool_call_output") => {
                    let raw: Option<String> = c.query_row("SELECT output_operation FROM request_items WHERE request_id=?1 AND position=?2 AND item_hash=?3", params![request_id, position, hash], |r| r.get(0)).optional()?.flatten();
                    raw.map(|raw| serde_json::from_str(&raw)).transpose()?
                }
                Some("function_call" | "custom_tool_call") => {
                    if let Some(call) = item["call_id"].as_str() {
                        let mut q = c.prepare("SELECT origin FROM claims WHERE origin_request_id=?1 AND request_id=?1 AND call_id=?2")?;
                        let origins = q
                            .query_map(params![request_id, call], |r| r.get::<_, String>(0))?
                            .collect::<std::result::Result<Vec<_>, _>>()?;
                        match origins.as_slice() {
                            [origin] => Some(OperationId {
                                origin: serde_json::from_str(origin)?,
                                request: RequestId(request_id.clone()),
                                call: CallId(call.to_owned()),
                            }),
                            [] => None,
                            _ => {
                                return Err(StoreError::AmbiguousReplayCall {
                                    call_id: call.to_owned(),
                                });
                            }
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        operations.push(operation);
    }
    let selected: HashSet<_> = operations.iter().flatten().cloned().collect();
    let ledger = super::validation::invocations_for_operations(c, &selected)?;
    for (entry, operation) in entries.iter_mut().zip(operations) {
        if let (
            ChatEntry::Message {
                request_id,
                position,
                hash,
                item,
                tool,
                ..
            },
            Some(operation),
        ) = (entry, operation)
        {
            let Some(issued) = ledger.get(&operation) else {
                return Err(StoreError::InvalidActorOutput);
            };
            if matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            ) && (issued.occurrence.request.0 != *request_id
                || issued.occurrence.position != *position
                || issued.occurrence.hash.0 != *hash)
            {
                return Err(StoreError::InvalidActorOutput);
            }
            *tool = Some(ChatTool {
                operation,
                invocation: ChatInvocation {
                    request: issued.occurrence.request.0.clone(),
                    position: issued.occurrence.position,
                    hash: issued.occurrence.hash.0.clone(),
                },
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Usage, actor_output::*};
    struct Authority;
    impl ActorOutputAuthority for Authority {
        fn validate_output(&self, _: &ActorOutputEmission) -> std::result::Result<bool, String> {
            Ok(true)
        }
    }
    fn origin() -> ActorOutputOrigin {
        ActorOutputOrigin {
            run: "run".into(),
            native_actor: 1,
            incarnation: 1,
        }
    }
    #[test]
    fn backward_pages_start_at_latest_and_follow_exact_publication_boundaries() {
        let store = Store::memory().unwrap();
        let request = RequestId("history".into());
        store.create_request(&request, None, "/root").unwrap();
        for n in 0..7 {
            store
                .append_items(
                    &request,
                    &[Item(
                        json!({"type":"message","role":"user","content":format!("message {n}")}),
                    )],
                )
                .unwrap();
            store
                .record_event(Some(&request), "not_chat", &json!({}))
                .unwrap();
        }
        let all = store.chat_page(&origin(), Some(&request), 0, 100).unwrap();
        let mut before = None;
        let mut restored = vec![];
        loop {
            let page = store
                .chat_page_before(&origin(), Some(&request), before, 2)
                .unwrap();
            assert!(page.next_after.is_none());
            assert!(
                page.entries
                    .windows(2)
                    .all(|pair| pair[0].sequence() < pair[1].sequence())
            );
            assert!(
                page.entries
                    .iter()
                    .all(|entry| before.is_none_or(|boundary| entry.sequence() < boundary))
            );
            if let Some(cursor) = page.previous_before {
                assert_eq!(cursor, page.entries[0].sequence());
            }
            restored.splice(0..0, page.entries.iter().map(ChatEntry::sequence));
            before = page.previous_before;
            if before.is_none() {
                break;
            }
        }
        assert_eq!(
            restored,
            all.entries
                .iter()
                .map(ChatEntry::sequence)
                .collect::<Vec<_>>()
        );
        assert!(
            store
                .chat_page_before(&origin(), Some(&request), Some(-1), 2)
                .is_err()
        );
        let empty = store.chat_page_before(&origin(), None, None, 2).unwrap();
        assert!(empty.entries.is_empty());
        assert!(empty.previous_before.is_none());
    }
    #[test]
    fn chat_projects_output_issuance_without_joining_reused_wire_ids() {
        let store = Store::memory().unwrap();
        let first = RequestId("first".into());
        let second = RequestId("second".into());
        let call = CallId("reused".into());
        let item =
            Item(json!({"type":"function_call","name":"run","call_id":call.0,"arguments":"{}"}));
        store
            .write_request(&first, None, "/root", &[item.clone()], Usage::default())
            .unwrap();
        let operation_a = store.claim(&call, &first).unwrap();
        store
            .write_request(&second, Some(&first), "/root", &[item], Usage::default())
            .unwrap();
        let operation_b = store.claim(&call, &second).unwrap();
        assert_ne!(operation_a, operation_b);
        store
            .write_job_output(
                &operation_b,
                crate::item::ToolKind::Function,
                &crate::turn::JobOutput::Completed(Ok(json!({"answer":42}))),
            )
            .unwrap();
        store
            .append_operation_output(&operation_b, &second, &second)
            .unwrap();
        let page = store
            .chat_page_before(&origin(), Some(&second), None, 100)
            .unwrap();
        let owners = page
            .entries
            .iter()
            .filter_map(|entry| match entry {
                ChatEntry::Message {
                    tool: Some(tool), ..
                } => Some(&tool.operation),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(owners, vec![&operation_a, &operation_b, &operation_b]);
        let result = store
            .chat_page_before(&origin(), Some(&second), None, 1)
            .unwrap();
        assert!(
            matches!(&result.entries[0], ChatEntry::Message { tool: Some(tool), .. } if tool.operation == operation_b && tool.invocation.request == second.0)
        );
    }
    #[test]
    fn publication_interleaves_exact_occurrences_and_pages_without_hash_dedup() {
        let store = Store::memory().unwrap();
        let a = RequestId("a".into());
        let b = RequestId("b".into());
        let item = Item(json!({"type":"message","role":"user","content":"same"}));
        store
            .write_request(&a, None, "/root", &[item.clone()], Usage::default())
            .unwrap();
        let emission = ActorOutputEmission {
            origin: origin(),
            id: ActorOutputId {
                display_slot: 1,
                page_ordinal: 0,
            },
            execution: ActorOutputExecution::ActorProgram,
            conversation: None,
            page: ActorDisplayPage {
                text: "display".into(),
                expansions: vec![],
                unavailable: false,
                view: None,
            },
        };
        store.append_actor_output(&Authority, &emission).unwrap();
        store
            .write_request(&b, Some(&a), "/root", &[item.clone()], Usage::default())
            .unwrap();
        store.set_effort(&b, crate::model::Effort::Low).unwrap();
        let first = store.chat_page(&origin(), Some(&b), 0, 2).unwrap();
        assert_eq!(first.entries.len(), 2);
        assert!(matches!(first.entries[0],ChatEntry::Message{ref request_id,..}if request_id=="a"));
        assert!(matches!(first.entries[1], ChatEntry::Output { .. }));
        let next = store
            .chat_page(&origin(), Some(&b), first.next_after.unwrap(), 2)
            .unwrap();
        assert_eq!(next.entries.len(), 1);
        assert!(
            matches!(&next.entries[0],ChatEntry::Message{request_id,item:body,..}if request_id=="b"&&*body==item.0)
        );
        assert!(!next.legacy_history);
        assert_eq!(next.next_after, None);
    }
    #[test]
    fn reconstruction_has_no_new_publications_and_legacy_cutover_stays_explicit() {
        let store = Store::memory().unwrap();
        let source = RequestId("source".into());
        let compacted = RequestId("compacted".into());
        let item = Item(json!({"type":"message","role":"user","content":"source"}));
        store
            .write_request(&source, None, "/root", &[item.clone()], Usage::default())
            .unwrap();
        let count = store.events(None).unwrap().len();
        store
            .write_compaction_request(&compacted, &source, "/root", &[item])
            .unwrap();
        assert_eq!(
            store
                .events(None)
                .unwrap()
                .iter()
                .filter(|e| e.kind == "chat_message")
                .count(),
            count
        );
        let page = store
            .chat_page(&origin(), Some(&compacted), 0, 100)
            .unwrap();
        assert_eq!(page.entries.len(), 1);
        store
            .lock()
            .unwrap()
            .execute("DELETE FROM events WHERE kind='chat_message'", [])
            .unwrap();
        let page = store
            .chat_page(&origin(), Some(&compacted), 0, 100)
            .unwrap();
        assert!(page.entries.is_empty());
        assert!(page.legacy_history);
    }
    #[test]
    fn body_bounds_are_explicit_and_reference_insert_failure_rolls_back_items() {
        let store = Store::memory().unwrap();
        let request = RequestId("r".into());
        store.create_request(&request, None, "/root").unwrap();
        store.lock().unwrap().execute_batch("CREATE TRIGGER refuse_chat BEFORE INSERT ON events WHEN NEW.kind='chat_message' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
        assert!(
            store
                .append_items(
                    &request,
                    &[Item(json!({"type":"message","content":"atomic"}))]
                )
                .is_err()
        );
        assert!(store.items(&request).unwrap().is_empty());
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER refuse_chat")
            .unwrap();
        store
            .append_items(
                &request,
                &[Item(
                    json!({"type":"message","content":"x".repeat(MAX_HISTORY_BYTES)}),
                )],
            )
            .unwrap();
        let page = store.chat_page(&origin(), Some(&request), 0, 100).unwrap();
        assert!(matches!(page.entries[0], ChatEntry::Oversized { .. }));
        assert!(serde_json::to_vec(&page).unwrap().len() < MAX_HISTORY_BYTES);
    }
}
#[cfg(test)]
mod publication_boundary_tests {
    use super::*;
    use crate::{model::AgentPath, store::Usage};
    #[test]
    fn here_snapshot_reuses_proven_occurrences_and_publishes_only_new_delivered_task() {
        let store = Store::memory().unwrap();
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        let source = RequestId("source".into());
        let snapshot = RequestId("snapshot".into());
        let original =
            Item(json!({"type":"message","role":"user","content":"same at two positions"}));
        store
            .write_request(
                &source,
                None,
                &root.0,
                &[original.clone(), original],
                Usage::default(),
            )
            .unwrap();
        store
            .admit_agent(&root, None, Some(&source), &json!({}), &json!({}))
            .unwrap();
        store
            .admit_here_agent_with_snapshot(
                &child,
                &root,
                &snapshot,
                &json!({}),
                &root.0,
                &child.0,
                "AtBoundary",
                &Item(json!({"type":"message","role":"user","content":"new child task"})),
            )
            .unwrap();
        let origin = ActorOutputOrigin {
            run: "run".into(),
            native_actor: 7,
            incarnation: 1,
        };
        let page = store.chat_page(&origin, Some(&snapshot), 0, 100).unwrap();
        assert_eq!(page.entries.len(), 2);
        for (i, entry) in page.entries.iter().enumerate() {
            assert!(
                matches!(entry,ChatEntry::Message{request_id,position,..}if request_id=="source"&&*position==i as i64)
            );
        }
        store.append_unread_envelopes(&child, &snapshot).unwrap();
        let page = store.chat_page(&origin, Some(&snapshot), 0, 100).unwrap();
        assert_eq!(page.entries.len(), 3);
        assert!(
            matches!(&page.entries[2],ChatEntry::Message{request_id,item,..}if request_id=="snapshot"&&item["content"]=="new child task")
        );
    }
    #[test]
    fn migrated_legacy_history_has_explicit_cutover_without_fabricated_order() {
        let path = std::env::temp_dir().join(format!(
            "harness-chat-cutover-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let head = RequestId("head".into());
        let marker;
        {
            let store = Store::open(&path).unwrap();
            store
                .write_request(
                    &head,
                    None,
                    "/root",
                    &[Item(
                        json!({"type":"message","role":"user","content":"old"}),
                    )],
                    Usage::default(),
                )
                .unwrap();
            marker = store
                .record_event(Some(&head), "legacy_marker", &json!({}))
                .unwrap();
            let c = store.lock().unwrap();
            c.execute_batch("DELETE FROM events WHERE kind='chat_message'; DELETE FROM session_state WHERE session_id='harness:chat-cutover'; UPDATE schema_version SET version=13;").unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            store
                .append_items(
                    &head,
                    &[Item(
                        json!({"type":"message","role":"assistant","content":"new"}),
                    )],
                )
                .unwrap();
            let origin = ActorOutputOrigin {
                run: "run".into(),
                native_actor: 7,
                incarnation: 1,
            };
            let page = store.chat_page(&origin, Some(&head), 0, 100).unwrap();
            assert_eq!(page.cutover_sequence, marker);
            assert!(page.legacy_history);
            assert_eq!(page.entries.len(), 1);
            assert!(
                matches!(&page.entries[0],ChatEntry::Message{item,sequence,..}if item["content"]=="new"&&*sequence>marker)
            );
        }
        std::fs::remove_file(path).unwrap();
    }
}
