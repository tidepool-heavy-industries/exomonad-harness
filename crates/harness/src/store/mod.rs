//! Durable SQLite event and content-addressed request store.
pub mod schema;

use crate::{
    item::{Item, ItemHash},
    model::{CallId, RequestId},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Mutex};

pub const VERSION: u32 = schema::VERSION;
pub const SQL: &str = schema::SQL;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("request not found: {0}")]
    MissingRequest(String),
    #[error("claim already exists for call/request")]
    DuplicateClaim,
}
pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub id: RequestId,
    pub parent: Option<RequestId>,
    pub branch: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Event {
    pub id: i64,
    pub request: Option<RequestId>,
    pub kind: String,
    pub payload: String,
    pub created_at: i64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Envelope {
    pub id: i64,
    pub sender: String,
    pub recipient: String,
    pub class: String,
    pub item_hash: ItemHash,
    pub delivered_request: Option<RequestId>,
    pub created_at: i64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Claim {
    pub call_id: CallId,
    pub request: RequestId,
    pub state: ClaimState,
    pub output: Option<ItemHash>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimState {
    Pending,
    Settled,
    Interrupted,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub hook: String,
    pub event_refs: Vec<String>,
    pub decision: serde_json::Value,
    pub evidence: serde_json::Value,
    pub latency_ms: Option<u64>,
}

/// Clones share one serialized SQLite connection. Use separate Store::open handles for read concurrency.
pub struct Store {
    conn: Mutex<Connection>,
}
impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }
    pub fn memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }
    fn from_connection(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
        )?;
        schema::initialize(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }
    pub fn create_request(
        &self,
        id: &RequestId,
        parent: Option<&RequestId>,
        branch: &str,
    ) -> Result<Request> {
        let c = self.lock();
        c.execute(
            "INSERT INTO requests(id,parent_id,branch) VALUES (?1,?2,?3)",
            params![id.0, parent.map(|p| p.0.as_str()), branch],
        )?;
        Ok(Request {
            id: id.clone(),
            parent: parent.cloned(),
            branch: branch.into(),
        })
    }
    pub fn request(&self, id: &RequestId) -> Result<Option<Request>> {
        self.lock()
            .query_row(
                "SELECT id,parent_id,branch FROM requests WHERE id=?1",
                [&id.0],
                |r| {
                    Ok(Request {
                        id: RequestId(r.get(0)?),
                        parent: r.get::<_, Option<String>>(1)?.map(RequestId),
                        branch: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }
    fn put_item_tx(tx: &Transaction<'_>, item: &Item) -> Result<ItemHash> {
        let bytes = serde_json::to_vec(item)?;
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let json = String::from_utf8(bytes).expect("serde_json emits UTF-8");
        tx.execute(
            "INSERT OR IGNORE INTO items(hash,json) VALUES (?1,?2)",
            params![hash, json],
        )?;
        Ok(ItemHash(hash))
    }
    pub fn put_item(&self, item: &Item) -> Result<ItemHash> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let h = Self::put_item_tx(&tx, item)?;
        tx.commit()?;
        Ok(h)
    }
    pub fn get_item(&self, hash: &ItemHash) -> Result<Option<Item>> {
        self.lock()
            .query_row("SELECT json FROM items WHERE hash=?1", [&hash.0], |r| {
                r.get::<_, String>(0)
            })
            .optional()?
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }
    /// Append is atomic; identical item bytes share storage, while positions remain request-local.
    pub fn append_items(&self, request: &RequestId, items: &[Item]) -> Result<Vec<ItemHash>> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM requests WHERE id=?1)",
            [&request.0],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(StoreError::MissingRequest(request.0.clone()));
        }
        let mut pos: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position)+1,0) FROM request_items WHERE request_id=?1",
            [&request.0],
            |r| r.get(0),
        )?;
        let mut hashes = Vec::new();
        for item in items {
            let h = Self::put_item_tx(&tx, item)?;
            tx.execute(
                "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                params![request.0, pos, h.0],
            )?;
            pos += 1;
            hashes.push(h);
        }
        tx.commit()?;
        Ok(hashes)
    }
    pub fn items(&self, request: &RequestId) -> Result<Vec<Item>> {
        let c = self.lock();
        let mut q=c.prepare("SELECT i.json FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id=?1 ORDER BY ri.position")?;
        q.query_map([&request.0], |r| r.get::<_, String>(0))?
            .map(|x| Ok(serde_json::from_str(&x?)?))
            .collect()
    }
    pub fn seen_by(&self, request: &RequestId) -> Result<Vec<ItemHash>> {
        let c = self.lock();
        let mut q=c.prepare("WITH RECURSIVE lineage(id,parent_id) AS (SELECT id,parent_id FROM requests WHERE id=?1 UNION ALL SELECT r.id,r.parent_id FROM requests r JOIN lineage l ON r.id=l.parent_id) SELECT DISTINCT ri.item_hash FROM lineage JOIN request_items ri ON ri.request_id=lineage.id ORDER BY ri.item_hash")?;
        q.query_map([&request.0], |r| Ok(ItemHash(r.get(0)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
    pub fn children_of(&self, parent: &RequestId) -> Result<Vec<Request>> {
        let c = self.lock();
        let mut q = c.prepare(
            "SELECT id,parent_id,branch FROM requests WHERE parent_id=?1 ORDER BY branch",
        )?;
        q.query_map([&parent.0], |r| {
            Ok(Request {
                id: RequestId(r.get(0)?),
                parent: r.get::<_, Option<String>>(1)?.map(RequestId),
                branch: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
    }
    pub fn record_event(
        &self,
        request: Option<&RequestId>,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Result<i64> {
        let text = serde_json::to_string(payload)?;
        let c = self.lock();
        c.execute(
            "INSERT INTO events(request_id,kind,payload) VALUES (?1,?2,?3)",
            params![request.map(|r| r.0.as_str()), kind, text],
        )?;
        Ok(c.last_insert_rowid())
    }
    pub fn events(&self, request: Option<&RequestId>) -> Result<Vec<Event>> {
        let c = self.lock();
        let mut q=c.prepare("SELECT id,request_id,kind,payload,created_at FROM events WHERE (?1 IS NULL OR request_id=?1) ORDER BY id")?;
        q.query_map([request.map(|r| r.0.as_str())], |r| {
            Ok(Event {
                id: r.get(0)?,
                request: r.get::<_, Option<String>>(1)?.map(RequestId),
                kind: r.get(2)?,
                payload: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
    }
    pub fn add_envelope(
        &self,
        sender: &str,
        recipient: &str,
        class: &str,
        item: &Item,
        delivered: Option<&RequestId>,
    ) -> Result<i64> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let h = Self::put_item_tx(&tx, item)?;
        tx.execute("INSERT INTO envelopes(sender,recipient,class,item_hash,delivered_request) VALUES (?1,?2,?3,?4,?5)",params![sender,recipient,class,h.0,delivered.map(|r|r.0.as_str())])?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        Ok(id)
    }
    pub fn inbox(&self, path: &str) -> Result<Vec<Envelope>> {
        self.envelopes(path, false)
    }
    pub fn unread(&self, path: &str) -> Result<Vec<Envelope>> {
        self.envelopes(path, true)
    }
    fn envelopes(&self, path: &str, unread: bool) -> Result<Vec<Envelope>> {
        let c = self.lock();
        let mut q=c.prepare("SELECT id,sender,recipient,class,item_hash,delivered_request,created_at FROM envelopes WHERE recipient=?1 AND (?2=0 OR delivered_request IS NULL) ORDER BY id")?;
        q.query_map(params![path, unread as i32], |r| {
            Ok(Envelope {
                id: r.get(0)?,
                sender: r.get(1)?,
                recipient: r.get(2)?,
                class: r.get(3)?,
                item_hash: ItemHash(r.get(4)?),
                delivered_request: r.get::<_, Option<String>>(5)?.map(RequestId),
                created_at: r.get(6)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
    }
    pub fn claim(&self, call: &CallId, request: &RequestId) -> Result<()> {
        self.lock().execute("INSERT INTO claims(call_id,request_id,state) VALUES (?1,?2,'pending')",params![call.0,request.0]).map(|_|()).map_err(|e|if matches!(e,rusqlite::Error::SqliteFailure(ref x,_) if x.extended_code==rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY){StoreError::DuplicateClaim}else{e.into()})
    }
    pub fn claims(&self, call: &CallId) -> Result<Vec<Claim>> {
        let c = self.lock();
        let mut q=c.prepare("SELECT call_id,request_id,state,output_hash FROM claims WHERE call_id=?1 ORDER BY request_id")?;
        q.query_map([&call.0], |r| {
            let s: String = r.get(2)?;
            Ok(Claim {
                call_id: CallId(r.get(0)?),
                request: RequestId(r.get(1)?),
                state: match s.as_str() {
                    "settled" => ClaimState::Settled,
                    "interrupted" => ClaimState::Interrupted,
                    _ => ClaimState::Pending,
                },
                output: r.get::<_, Option<String>>(3)?.map(ItemHash),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
    }
    pub fn settle_claims(&self, call: &CallId, output: &Item) -> Result<usize> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let h = Self::put_item_tx(&tx, output)?;
        let n = tx.execute(
            "UPDATE claims SET state='settled',output_hash=?2 WHERE call_id=?1 AND state='pending'",
            params![call.0, h.0],
        )?;
        tx.commit()?;
        Ok(n)
    }
    pub fn interrupt_claim(&self, call: &CallId, request: &RequestId) -> Result<usize> {
        Ok(self.lock().execute("UPDATE claims SET state='interrupted' WHERE call_id=?1 AND request_id=?2 AND state='pending'",params![call.0,request.0])?)
    }
    pub fn record_decision(&self, request: Option<&RequestId>, d: &Decision) -> Result<i64> {
        let c = self.lock();
        c.execute("INSERT INTO decisions(request_id,hook,event_refs,decision,evidence,latency_ms) VALUES (?1,?2,?3,?4,?5,?6)",params![request.map(|r|r.0.as_str()),d.hook,serde_json::to_string(&d.event_refs)?,serde_json::to_string(&d.decision)?,serde_json::to_string(&d.evidence)?,d.latency_ms.map(|v|v as i64)])?;
        Ok(c.last_insert_rowid())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(v: serde_json::Value) -> Item {
        Item(v)
    }
    fn id(s: &str) -> RequestId {
        RequestId(s.into())
    }
    #[test]
    fn durable_dag_content_address_and_queries() {
        let path = std::env::temp_dir().join(format!("harness-store-{}.db", uuid::Uuid::new_v4()));
        let shared = item(serde_json::json!({"type":"message","role":"user","content":"hi"}));
        let hash;
        {
            let s = Store::open(&path).unwrap();
            s.create_request(&id("root"), None, "root").unwrap();
            s.create_request(&id("left"), Some(&id("root")), "left")
                .unwrap();
            s.create_request(&id("right"), Some(&id("root")), "right")
                .unwrap();
            hash = s.append_items(&id("root"), &[shared.clone()]).unwrap()[0].clone();
            assert_eq!(
                s.append_items(&id("left"), &[shared.clone()]).unwrap()[0],
                hash
            );
            s.append_items(&id("right"), &[item(serde_json::json!({"x":1}))])
                .unwrap();
            assert_eq!(s.items(&id("root")).unwrap(), vec![shared.clone()]);
            assert_eq!(s.children_of(&id("root")).unwrap().len(), 2);
            assert_eq!(s.seen_by(&id("left")).unwrap().len(), 1);
            let cid = CallId("c1".into());
            s.claim(&cid, &id("root")).unwrap();
            let output = item(serde_json::json!({"output":"ok"}));
            assert_eq!(s.settle_claims(&cid, &output).unwrap(), 1);
            assert_eq!(s.claims(&cid).unwrap()[0].state, ClaimState::Settled);
            s.add_envelope("a", "b", "AtBoundary", &shared, None)
                .unwrap();
            assert_eq!(s.unread("b").unwrap().len(), 1);
            s.record_event(Some(&id("root")), "created", &serde_json::json!({}))
                .unwrap();
            assert_eq!(s.events(Some(&id("root"))).unwrap().len(), 1);
        }
        {
            let s = Store::open(&path).unwrap();
            assert_eq!(s.get_item(&hash).unwrap(), Some(shared));
            assert_eq!(s.items(&id("left")).unwrap().len(), 1);
            assert_eq!(s.inbox("b").unwrap().len(), 1);
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }
}
