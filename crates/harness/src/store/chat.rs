//! Journal publication order. Body bytes stay in Items, never in reference events.
use super::{
    Result, Store, StoreError,
    actor_output::{ActorOutputEmission, ActorOutputOrigin, validate_origin},
    forms::StoredActorForm,
    history::{MAX_HISTORY_BYTES, MAX_HISTORY_ITEMS},
};
use crate::{
    item::{Item, ItemHash},
    model::RequestId,
};
use rusqlite::{Transaction, params};
use serde::Serialize;
use serde_json::{Value, json};
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
    },
    Output {
        sequence: i64,
        output: Value,
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
}
impl Store {
    pub fn chat_page(
        &self,
        origin: &ActorOutputOrigin,
        head: Option<&RequestId>,
        after: i64,
        limit: usize,
    ) -> Result<ChatPage> {
        validate_origin(origin)?;
        if after < 0 || limit == 0 || limit > MAX_HISTORY_ITEMS {
            return Err(StoreError::InvalidHistoryOffset);
        }
        let c = self.lock();
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
        let sql = format!(
            "{lineage} SELECT e.id,e.kind,e.request_id,e.payload,e.created_at FROM events e WHERE e.id>?2 AND ((e.kind='chat_message' AND e.request_id IN (SELECT id FROM lineage)) OR (e.kind IN ('actor_output','actor_form_open') AND json_extract(e.payload,'$.origin.run')=?3 AND json_extract(e.payload,'$.origin.nativeActor')=?4 AND json_extract(e.payload,'$.origin.incarnation')=?5)) ORDER BY e.id LIMIT ?6"
        );
        let mut q = c.prepare(&sql)?;
        let candidates = q
            .query_map(
                params![
                    head.map(|h| h.0.as_str()),
                    after,
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
        };
        for (sequence, kind, request, payload, created_at) in candidates {
            if page.entries.len() == limit {
                page.next_after = page.entries.last().map(ChatEntry::sequence);
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
                        }
                    }
                }
                "actor_output" => {
                    let emission: ActorOutputEmission = serde_json::from_str(&payload)?;
                    ChatEntry::Output {
                        sequence,
                        output: json!({"reference":{"origin":emission.origin,"sequence":sequence},"emission":emission,"createdAt":created_at}),
                    }
                }
                "actor_form_open" => {
                    let raw: String = c.query_row(
                        "SELECT presentation FROM actor_forms WHERE opening_sequence=?1",
                        [sequence],
                        |r| r.get(0),
                    )?;
                    ChatEntry::Form {
                        sequence,
                        form: serde_json::from_str(&raw)?,
                    }
                }
                _ => unreachable!(),
            };
            page.entries.push(entry);
            if serde_json::to_vec(&page)?.len() > MAX_HISTORY_BYTES {
                page.entries.pop();
                page.next_after = page.entries.last().map(ChatEntry::sequence);
                if page.entries.is_empty() {
                    return Err(StoreError::InvalidActorOutput);
                }
                break;
            }
        }
        Ok(page)
    }
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
        store.lock().execute_batch("CREATE TRIGGER refuse_chat BEFORE INSERT ON events WHEN NEW.kind='chat_message' BEGIN SELECT RAISE(ABORT,'refuse'); END;").unwrap();
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
