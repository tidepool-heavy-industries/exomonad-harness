//! Bounded, ordered reads of one durable request's exact Items.

use super::{Request, Result, Store, StoreError};
use crate::{item::Item, model::RequestId};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

pub const MAX_HISTORY_ITEMS: usize = 100;
pub const MAX_HISTORY_BYTES: usize = 256 * 1024;
const ITEM_BUDGET: usize = MAX_HISTORY_BYTES - 16 * 1024;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub position: u64,
    pub hash: String,
    pub byte_len: usize,
    pub item: Item,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OversizedItem {
    pub position: u64,
    pub hash: String,
    pub byte_len: usize,
    /// The client must explicitly request this offset to omit the large Item.
    pub skip_offset: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub request_id: String,
    pub parent_id: Option<String>,
    pub branch: String,
    pub items: Vec<HistoryItem>,
    pub next_offset: Option<u64>,
    pub oversized_item: Option<OversizedItem>,
}

impl Store {
    /// Read only durable completion metadata, without loading model inputs or Items.
    pub(crate) fn completed_model_requests(&self, limit: usize) -> Result<Vec<Request>> {
        let c = self.lock();
        let mut query = c.prepare(
            "WITH recent AS (
                SELECT request_id,MAX(id) AS sequence FROM (
                    SELECT id,request_id FROM events WHERE kind='model_turn'
                    ORDER BY id DESC LIMIT ?1
                ) GROUP BY request_id
             ) SELECT r.id,r.parent_id,r.branch FROM recent
             JOIN requests r ON r.id=recent.request_id ORDER BY recent.sequence",
        )?;
        let rows = query.query_map([limit.clamp(1, 128) as i64], |row| {
            Ok(Request {
                id: RequestId(row.get(0)?),
                parent: row.get::<_, Option<String>>(1)?.map(RequestId),
                branch: row.get(2)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
    /// Read at most `MAX_HISTORY_ITEMS` and `MAX_HISTORY_BYTES` from one request.
    /// An oversized Item is identified by its content hash and position. It is
    /// never silently skipped, and a caller can explicitly request `skip_offset`.
    pub fn history_page(&self, id: &RequestId, offset: u64, limit: usize) -> Result<HistoryPage> {
        let limit = limit.clamp(1, MAX_HISTORY_ITEMS);
        let offset = i64::try_from(offset).map_err(|_| StoreError::InvalidHistoryOffset)?;
        let c = self.lock();
        let request: Request = c
            .query_row(
                "SELECT id,parent_id,branch FROM requests WHERE id=?1",
                [&id.0],
                |row| {
                    Ok(Request {
                        id: RequestId(row.get(0)?),
                        parent: row.get::<_, Option<String>>(1)?.map(RequestId),
                        branch: row.get(2)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::MissingRequest(id.0.clone()))?;
        let metadata = {
            let mut query = c.prepare(
                "SELECT ri.position,ri.item_hash,length(CAST(i.json AS BLOB)) \
                 FROM request_items ri JOIN items i ON i.hash=ri.item_hash \
                 WHERE ri.request_id=?1 AND ri.position>=?2 ORDER BY ri.position LIMIT ?3",
            )?;
            query
                .query_map(
                    params![id.0, offset, i64::try_from(limit + 1).unwrap()],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut page = HistoryPage {
            request_id: request.id.0,
            parent_id: request.parent.map(|id| id.0),
            branch: request.branch,
            items: Vec::new(),
            next_offset: None,
            oversized_item: None,
        };
        let mut used = 0usize;
        for (position, hash, byte_len) in metadata.iter().take(limit) {
            let position =
                u64::try_from(*position).map_err(|_| StoreError::InvalidHistoryOffset)?;
            let byte_len =
                usize::try_from(*byte_len).map_err(|_| StoreError::InvalidHistoryOffset)?;
            if byte_len > ITEM_BUDGET.saturating_sub(used) {
                page.next_offset = Some(position);
                if byte_len > ITEM_BUDGET {
                    page.oversized_item = Some(OversizedItem {
                        position,
                        hash: hash.clone(),
                        byte_len,
                        skip_offset: position + 1,
                    });
                }
                break;
            }
            let raw: String =
                c.query_row("SELECT json FROM items WHERE hash=?1", [hash], |row| {
                    row.get(0)
                })?;
            let item: Item = serde_json::from_str(&raw)?;
            used += byte_len;
            page.items.push(HistoryItem {
                position,
                hash: hash.clone(),
                byte_len,
                item,
            });
        }
        if page.next_offset.is_none() && metadata.len() > limit {
            page.next_offset = u64::try_from(metadata[limit].0).ok();
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pages_exact_items_and_identifies_large_item_without_skipping_it() {
        let store = Store::memory().unwrap();
        let request = RequestId("history-page".into());
        store.create_request(&request, None, "/root").unwrap();
        let first = Item(json!({"type":"message","role":"user","content":"first"}));
        let large =
            Item(json!({"type":"message","role":"user","content":"z".repeat(MAX_HISTORY_BYTES)}));
        let last = Item(json!({"type":"message","role":"user","content":"last"}));
        store
            .append_items(&request, &[first.clone(), large, last.clone()])
            .unwrap();

        let page = store.history_page(&request, 0, 100).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].item, first);
        assert_eq!(page.next_offset, Some(1));
        let oversized = page.oversized_item.unwrap();
        assert_eq!(oversized.position, 1);
        assert_eq!(oversized.skip_offset, 2);
        assert_eq!(oversized.hash.len(), 64);
        assert!(oversized.byte_len > ITEM_BUDGET);

        let blocked = store.history_page(&request, 1, 100).unwrap();
        assert!(blocked.items.is_empty());
        assert_eq!(blocked.next_offset, Some(1));
        assert_eq!(blocked.oversized_item.unwrap().hash, oversized.hash);
        let skipped = store.history_page(&request, 2, 100).unwrap();
        assert_eq!(skipped.items.len(), 1);
        assert_eq!(skipped.items[0].item, last);
        assert_eq!(skipped.next_offset, None);
    }

    #[test]
    fn count_limit_and_missing_request_are_explicit() {
        let store = Store::memory().unwrap();
        let request = RequestId("count-page".into());
        store.create_request(&request, None, "/root").unwrap();
        store
            .append_items(
                &request,
                &[
                    Item(json!({"type":"message","content":"one"})),
                    Item(json!({"type":"message","content":"two"})),
                ],
            )
            .unwrap();
        let page = store.history_page(&request, 0, 1).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.next_offset, Some(1));
        assert!(matches!(
            store.history_page(&RequestId("missing".into()), 0, 1),
            Err(StoreError::MissingRequest(_))
        ));
    }

    #[test]
    fn page_limits_serialized_response_bytes() {
        let store = Store::memory().unwrap();
        let request = RequestId("byte-page".into());
        store.create_request(&request, None, "/root").unwrap();
        let items: Vec<_> = (0..100).map(|index| Item(json!({
            "type":"message", "role":"user", "content":format!("{index}:{}", "x".repeat(3000))
        }))).collect();
        store.append_items(&request, &items).unwrap();
        let page = store.history_page(&request, 0, 100).unwrap();
        assert!(page.items.len() < 100);
        assert!(page.next_offset.is_some());
        assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_HISTORY_BYTES);
    }

    #[test]
    fn item_that_fits_a_fresh_page_is_not_marked_oversized() {
        let store = Store::memory().unwrap();
        let request = RequestId("fresh-page".into());
        store.create_request(&request, None, "/root").unwrap();
        let first = Item(json!({"type":"message","content":"a".repeat(150_000)}));
        let second = Item(json!({"type":"message","content":"b".repeat(150_000)}));
        store
            .append_items(&request, &[first.clone(), second.clone()])
            .unwrap();
        let page = store.history_page(&request, 0, 100).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].item, first);
        assert_eq!(page.next_offset, Some(1));
        assert!(page.oversized_item.is_none());

        let next = store.history_page(&request, 1, 100).unwrap();
        assert_eq!(next.items.len(), 1);
        assert_eq!(next.items[0].item, second);
        assert_eq!(next.next_offset, None);
        assert!(next.oversized_item.is_none());
    }
}
