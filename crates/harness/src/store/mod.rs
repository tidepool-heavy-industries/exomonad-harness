//! Durable SQLite event and content-addressed request store.
pub mod schema;

use crate::{
    item::{Item, ItemHash},
    model::{CallId, RequestId},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_micros: i64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingCall {
    pub call_id: CallId,
    pub request: RequestId,
}
#[derive(Clone, Debug, PartialEq)]
pub struct StoredDecision {
    pub id: i64,
    pub request: Option<RequestId>,
    pub decision: Decision,
    pub created_at: i64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionState {
    pub session_id: String,
    pub state: String,
    pub updated_at: i64,
}
fn utc_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
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
        let mut conn = conn;
        schema::initialize(&mut conn)?;
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
            "INSERT INTO requests(id,parent_id,branch,created_at) VALUES (?1,?2,?3,?4)",
            params![id.0, parent.map(|p| p.0.as_str()), branch, utc_millis()],
        )?;
        Ok(Request {
            id: id.clone(),
            parent: parent.cloned(),
            branch: branch.into(),
        })
    }
    /// Create a request and persist its initial items in one commit, so recovery never sees a half-written prefix.
    pub fn write_request(
        &self,
        request: &RequestId,
        parent: Option<&RequestId>,
        branch: &str,
        items: &[Item],
        usage: Usage,
    ) -> Result<Request> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        tx.execute("INSERT INTO requests(id,parent_id,branch,created_at,input_tokens,output_tokens,cost_micros) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![request.0,parent.map(|p|p.0.as_str()),branch,utc_millis(),usage.input_tokens,usage.output_tokens,usage.cost_micros])?;
        for (position, item) in items.iter().enumerate() {
            let hash = Self::put_item_tx(&tx, item)?;
            tx.execute(
                "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                params![request.0, position as i64, hash.0],
            )?;
        }
        tx.commit()?;
        Ok(Request {
            id: request.clone(),
            parent: parent.cloned(),
            branch: branch.into(),
        })
    }
    /// Store a completed output and settle all claimants atomically.
    pub fn write_output(&self, call: &CallId, output: &Item) -> Result<usize> {
        self.settle_claims(call, output)
    }
    /// Crash recovery exposes all still-pending durable claims for the caller's resumption policy.
    pub fn recover_pending(&self) -> Result<Vec<PendingCall>> {
        let c = self.lock();
        let mut q=c.prepare("SELECT call_id,request_id FROM claims WHERE state='pending' ORDER BY call_id,request_id")?;
        q.query_map([], |r| {
            Ok(PendingCall {
                call_id: CallId(r.get(0)?),
                request: RequestId(r.get(1)?),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
    }
    pub fn set_usage(&self, request: &RequestId, usage: Usage) -> Result<()> {
        let n = self.lock().execute(
            "UPDATE requests SET input_tokens=?2,output_tokens=?3,cost_micros=?4 WHERE id=?1",
            params![
                request.0,
                usage.input_tokens,
                usage.output_tokens,
                usage.cost_micros
            ],
        )?;
        if n == 0 {
            return Err(StoreError::MissingRequest(request.0.clone()));
        }
        Ok(())
    }
    pub fn usage_subtree(&self, request: &RequestId) -> Result<Usage> {
        let c = self.lock();
        c.query_row("WITH RECURSIVE subtree(id) AS (SELECT id FROM requests WHERE id=?1 UNION ALL SELECT r.id FROM requests r JOIN subtree s ON r.parent_id=s.id) SELECT COALESCE(SUM(input_tokens),0),COALESCE(SUM(output_tokens),0),COALESCE(SUM(cost_micros),0) FROM subtree JOIN requests USING(id)",[&request.0],|r|Ok(Usage{input_tokens:r.get(0)?,output_tokens:r.get(1)?,cost_micros:r.get(2)?})).map_err(Into::into)
    }
    pub fn siblings(&self, request: &RequestId) -> Result<Vec<Request>> {
        let c = self.lock();
        let mut q=c.prepare("SELECT s.id,s.parent_id,s.branch FROM requests target JOIN requests s ON s.parent_id IS target.parent_id WHERE target.id=?1 AND s.id<>target.id ORDER BY s.branch")?;
        q.query_map([&request.0], |r| {
            Ok(Request {
                id: RequestId(r.get(0)?),
                parent: r.get::<_, Option<String>>(1)?.map(RequestId),
                branch: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
    }
    pub fn pending_at(&self, request: &RequestId) -> Result<Vec<PendingCall>> {
        let c = self.lock();
        let mut q=c.prepare("WITH RECURSIVE lineage(id,parent_id) AS (SELECT id,parent_id FROM requests WHERE id=?1 UNION ALL SELECT r.id,r.parent_id FROM requests r JOIN lineage l ON r.id=l.parent_id) SELECT c.call_id,c.request_id FROM claims c JOIN lineage l ON l.id=c.request_id WHERE c.state='pending' ORDER BY c.call_id,c.request_id")?;
        q.query_map([&request.0], |r| {
            Ok(PendingCall {
                call_id: CallId(r.get(0)?),
                request: RequestId(r.get(1)?),
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
    }
    pub fn save_session_state(&self, session_id: &str, state: &serde_json::Value) -> Result<()> {
        self.lock().execute("INSERT INTO session_state(session_id,state,updated_at) VALUES (?1,?2,?3) ON CONFLICT(session_id) DO UPDATE SET state=excluded.state,updated_at=excluded.updated_at",params![session_id,serde_json::to_string(state)?,utc_millis()])?;
        Ok(())
    }
    pub fn session_state(&self, session_id: &str) -> Result<Option<SessionState>> {
        self.lock()
            .query_row(
                "SELECT session_id,state,updated_at FROM session_state WHERE session_id=?1",
                [session_id],
                |r| {
                    Ok(SessionState {
                        session_id: r.get(0)?,
                        state: r.get(1)?,
                        updated_at: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
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
            "INSERT INTO events(request_id,kind,payload,created_at) VALUES (?1,?2,?3,?4)",
            params![request.map(|r| r.0.as_str()), kind, text, utc_millis()],
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
        tx.execute("INSERT INTO envelopes(sender,recipient,class,item_hash,delivered_request,created_at) VALUES (?1,?2,?3,?4,?5,?6)",params![sender,recipient,class,h.0,delivered.map(|r|r.0.as_str()),utc_millis()])?;
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
    pub fn decisions(&self, request: Option<&RequestId>) -> Result<Vec<StoredDecision>> {
        let c = self.lock();
        let mut q=c.prepare("SELECT id,request_id,hook,event_refs,decision,evidence,latency_ms,created_at FROM decisions WHERE (?1 IS NULL OR request_id=?1) ORDER BY id")?;
        q.query_map([request.map(|r| r.0.as_str())], |r| {
            let decision = Decision {
                hook: r.get(2)?,
                event_refs: serde_json::from_str(&r.get::<_, String>(3)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                decision: serde_json::from_str(&r.get::<_, String>(4)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                evidence: serde_json::from_str(&r.get::<_, String>(5)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                latency_ms: r.get::<_, Option<i64>>(6)?.map(|v| v.max(0) as u64),
            };
            Ok(StoredDecision {
                id: r.get(0)?,
                request: r.get::<_, Option<String>>(1)?.map(RequestId),
                decision,
                created_at: r.get(7)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
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
        c.execute("INSERT INTO decisions(request_id,hook,event_refs,decision,evidence,latency_ms,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",params![request.map(|r|r.0.as_str()),d.hook,serde_json::to_string(&d.event_refs)?,serde_json::to_string(&d.decision)?,serde_json::to_string(&d.evidence)?,d.latency_ms.map(|v|v as i64),utc_millis()])?;
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
            hash = s
                .append_items(&id("root"), std::slice::from_ref(&shared))
                .unwrap()[0]
                .clone();
            assert_eq!(
                s.append_items(&id("left"), std::slice::from_ref(&shared))
                    .unwrap()[0],
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

    #[test]
    fn typed_queries_usage_session_and_crash_recovery_survive_reopen() {
        let path = std::env::temp_dir().join(format!(
            "harness-store-recovery-{}.db",
            uuid::Uuid::new_v4()
        ));
        {
            let s = Store::open(&path).unwrap();
            s.write_request(
                &id("r"),
                None,
                "root",
                &[item(serde_json::json!({"request":1}))],
                Usage {
                    input_tokens: 4,
                    output_tokens: 2,
                    cost_micros: 9,
                },
            )
            .unwrap();
            s.create_request(&id("a"), Some(&id("r")), "a").unwrap();
            s.create_request(&id("b"), Some(&id("r")), "b").unwrap();
            s.set_usage(
                &id("a"),
                Usage {
                    input_tokens: 3,
                    output_tokens: 1,
                    cost_micros: 5,
                },
            )
            .unwrap();
            assert_eq!(s.siblings(&id("a")).unwrap()[0].id, id("b"));
            assert_eq!(
                s.usage_subtree(&id("r")).unwrap(),
                Usage {
                    input_tokens: 7,
                    output_tokens: 3,
                    cost_micros: 14
                }
            );
            let call = CallId("pending".into());
            s.claim(&call, &id("a")).unwrap();
            assert_eq!(s.pending_at(&id("a")).unwrap().len(), 1);
            assert_eq!(s.recover_pending().unwrap().len(), 1);
            s.save_session_state("session", &serde_json::json!({"cursor":7}))
                .unwrap();
            let d = Decision {
                hook: "tool".into(),
                event_refs: vec!["item-1".into()],
                decision: serde_json::json!({"allow":true}),
                evidence: serde_json::json!({"basis":"test"}),
                latency_ms: Some(2),
            };
            s.record_decision(Some(&id("a")), &d).unwrap();
            assert_eq!(s.decisions(Some(&id("a"))).unwrap()[0].decision, d);
        }
        {
            let s = Store::open(&path).unwrap();
            assert_eq!(s.recover_pending().unwrap()[0].request, id("a"));
            assert_eq!(
                s.session_state("session").unwrap().unwrap().state,
                r#"{"cursor":7}"#
            );
            let call = CallId("pending".into());
            assert_eq!(
                s.write_output(&call, &item(serde_json::json!({"ok":true})))
                    .unwrap(),
                1
            );
            assert!(s.recover_pending().unwrap().is_empty());
            assert!(
                s.events(None)
                    .unwrap()
                    .iter()
                    .all(|event| event.created_at > 1_000_000_000_000)
            );
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn migrates_v1_timestamps_and_usage_without_losing_requests() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE schema_version(version INTEGER NOT NULL); INSERT INTO schema_version VALUES(1); CREATE TABLE requests(id TEXT PRIMARY KEY,parent_id TEXT,branch TEXT,created_at INTEGER NOT NULL); INSERT INTO requests VALUES('old',NULL,'main',123); CREATE TABLE events(id INTEGER PRIMARY KEY,request_id TEXT,kind TEXT,payload TEXT,created_at INTEGER); INSERT INTO events VALUES(1,'old','x','{}',124);").unwrap();
        schema::initialize(&mut conn).unwrap();
        let request: (i64, i64) = conn
            .query_row(
                "SELECT created_at,input_tokens FROM requests WHERE id='old'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(request, (123_000, 0));
        let version: u32 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
        assert!(schema::initialize(&mut conn).is_ok());
    }
}
