//! Durable SQLite event and content-addressed request store.
pub mod schema;

use crate::{
    item::{Item, ItemHash},
    model::{AgentPath, CallId, Effort, RequestId},
    transport::{ResponsesRequest, ResponsesTurn},
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
    #[error("request {request} belongs to agent {actual}, not {expected}")]
    RequestAgentMismatch {
        request: String,
        expected: String,
        actual: String,
    },
    #[error("claim already exists for call/request")]
    DuplicateClaim,
    #[error("invalid canonical agent path: {0}")]
    InvalidAgentPath(String),
    #[error("agent parent does not exist: {0}")]
    MissingAgentParent(String),
    #[error("active spawn call {call_id} is not persisted in request {request}")]
    MissingActiveSpawnCall { request: String, call_id: String },
    #[error("session state key uses the reserved harness namespace: {0}")]
    ReservedSessionStateNamespace(String),
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentState {
    Active,
    Idle,
    Completed,
    Cancelled,
}
impl AgentState {
    fn parse(s: &str) -> Self {
        match s {
            "idle" => Self::Idle,
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            _ => Self::Active,
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub path: AgentPath,
    pub parent: Option<AgentPath>,
    pub head_request: Option<RequestId>,
    pub contract: serde_json::Value,
    pub fork_source: serde_json::Value,
    pub state: AgentState,
    pub created_at: i64,
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
/// The exact model boundary: input and response are captured together before
/// later provider outputs can be appended to request history.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RecordedReplayTurn {
    pub request: RequestId,
    pub model_request: ResponsesRequest,
    pub model_response: ResponsesTurn,
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
// TODO(wave1): `hook: String` + `decision: Value` is a blob, not the PRD shape.
// Each of the eleven hooks gets its own closed decision enum; the row keeps the
// typed decision plus a provider-opaque `evidence` blob. Keep this table's
// columns; type the values.
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
    fn validate_agent_path(path: &str, parent: Option<&str>) -> Result<()> {
        let valid = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        };
        let segs: Vec<_> = path.strip_prefix('/').unwrap_or("").split('/').collect();
        let relation = match parent {
            None => path == "/root",
            Some(p) => path.strip_prefix(p).is_some_and(|suffix| {
                suffix.starts_with('/') && !suffix[1..].contains('/') && valid(&suffix[1..])
            }),
        };
        if !path.starts_with('/') || segs.iter().any(|s| !valid(s)) || !relation {
            return Err(StoreError::InvalidAgentPath(path.into()));
        }
        Ok(())
    }
    fn decode_agent(
        r: (
            String,
            Option<String>,
            Option<String>,
            String,
            String,
            String,
            i64,
        ),
    ) -> Result<Agent> {
        Ok(Agent {
            path: AgentPath(r.0.clone()),
            // `/operator` is a virtual parent: it is the human mailbox, not a
            // schedulable agent row. Keep old SQLite roots readable.
            parent: r
                .1
                .map(AgentPath)
                .or_else(|| (r.0 == "/root").then(|| AgentPath("/operator".into()))),
            head_request: r.2.map(RequestId),
            contract: serde_json::from_str(&r.3)?,
            fork_source: serde_json::from_str(&r.4)?,
            state: AgentState::parse(&r.5),
            created_at: r.6,
        })
    }
    pub fn admit_agent(
        &self,
        path: &AgentPath,
        parent: Option<&AgentPath>,
        head: Option<&RequestId>,
        contract: &serde_json::Value,
        source: &serde_json::Value,
    ) -> Result<Agent> {
        Self::validate_agent_path(&path.0, parent.map(|p| p.0.as_str()))?;
        let mut c = self.lock();
        let tx = c.transaction()?;
        if let Some(p) = parent {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM agents WHERE path=?1)",
                [&p.0],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(StoreError::MissingAgentParent(p.0.clone()));
            }
        }
        if let Some(h) = head {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM requests WHERE id=?1)",
                [&h.0],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(StoreError::MissingRequest(h.0.clone()));
            }
        }
        let created_at = utc_millis();
        tx.execute("INSERT INTO agents(path,parent_path,head_request,contract,fork_source,state,created_at) VALUES (?1,?2,?3,?4,?5,'active',?6)",
            params![path.0,parent.map(|p|p.0.as_str()),head.map(|h|h.0.as_str()),serde_json::to_string(contract)?,serde_json::to_string(source)?,created_at])?;
        tx.commit()?;
        Ok(Agent {
            path: path.clone(),
            parent: parent.cloned(),
            head_request: head.cloned(),
            contract: contract.clone(),
            fork_source: source.clone(),
            state: AgentState::Active,
            created_at,
        })
    }
    /// Atomically admit an agent and persist its initial task envelope.
    /// Failure of either insert rolls back both the agent and item/envelope writes.
    #[allow(clippy::too_many_arguments)]
    pub fn admit_agent_with_envelope(
        &self,
        path: &AgentPath,
        parent: Option<&AgentPath>,
        head: Option<&RequestId>,
        contract: &serde_json::Value,
        source: &serde_json::Value,
        sender: &str,
        recipient: &str,
        class: &str,
        item: &Item,
    ) -> Result<(Agent, i64)> {
        Self::validate_agent_path(&path.0, parent.map(|p| p.0.as_str()))?;
        let mut c = self.lock();
        let tx = c.transaction()?;
        if let Some(p) = parent {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM agents WHERE path=?1)",
                [&p.0],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(StoreError::MissingAgentParent(p.0.clone()));
            }
        }
        if let Some(h) = head {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM requests WHERE id=?1)",
                [&h.0],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(StoreError::MissingRequest(h.0.clone()));
            }
        }
        let created_at = utc_millis();
        tx.execute("INSERT INTO agents(path,parent_path,head_request,contract,fork_source,state,created_at) VALUES (?1,?2,?3,?4,?5,'active',?6)",
            params![path.0,parent.map(|p|p.0.as_str()),head.map(|h|h.0.as_str()),serde_json::to_string(contract)?,serde_json::to_string(source)?,created_at])?;
        let hash = Self::put_item_tx(&tx, item)?;
        tx.execute("INSERT INTO envelopes(sender,recipient,class,item_hash,delivered_request,created_at) VALUES (?1,?2,?3,?4,NULL,?5)",
            params![sender, recipient, class, hash.0, utc_millis()])?;
        let envelope_id = tx.last_insert_rowid();
        tx.commit()?;
        Ok((
            Agent {
                path: path.clone(),
                parent: parent.cloned(),
                head_request: head.cloned(),
                contract: contract.clone(),
                fork_source: source.clone(),
                state: AgentState::Active,
                created_at,
            },
            envelope_id,
        ))
    }
    /// Atomically fork a flattened `here` snapshot, admit its child, and put
    /// the initial NEW_TASK envelope in the child's mailbox. The snapshot
    /// request deliberately has no request-parent edge: `source_head` is
    /// retained only in the agent's fork metadata.
    #[allow(clippy::too_many_arguments)]
    pub fn admit_here_agent_with_snapshot(
        &self,
        path: &AgentPath,
        parent: &AgentPath,
        snapshot_request: &RequestId,
        contract: &serde_json::Value,
        sender: &str,
        recipient: &str,
        class: &str,
        task_item: &Item,
    ) -> Result<(Agent, i64)> {
        self.admit_here_agent_from_source(
            path,
            parent,
            snapshot_request,
            None,
            None,
            contract,
            sender,
            recipient,
            class,
            task_item,
        )
    }

    /// Atomically fork from an in-flight spawn invocation. The request must
    /// already contain the actual `spawn_agent` call item; its output is added
    /// by the child Engine only after that output is durable in the parent's
    /// request history.
    #[allow(clippy::too_many_arguments)]
    pub fn admit_here_agent_from_invocation(
        &self,
        path: &AgentPath,
        parent: &AgentPath,
        snapshot_request: &RequestId,
        invocation_request: &RequestId,
        invocation_call_id: &CallId,
        contract: &serde_json::Value,
        sender: &str,
        recipient: &str,
        class: &str,
        task_item: &Item,
    ) -> Result<(Agent, i64)> {
        self.admit_here_agent_from_source(
            path,
            parent,
            snapshot_request,
            Some(invocation_request),
            Some(invocation_call_id),
            contract,
            sender,
            recipient,
            class,
            task_item,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn admit_here_agent_from_source(
        &self,
        path: &AgentPath,
        parent: &AgentPath,
        snapshot_request: &RequestId,
        invocation_request: Option<&RequestId>,
        invocation_call_id: Option<&CallId>,
        contract: &serde_json::Value,
        sender: &str,
        recipient: &str,
        class: &str,
        task_item: &Item,
    ) -> Result<(Agent, i64)> {
        Self::validate_agent_path(&path.0, Some(&parent.0))?;
        let mut c = self.lock();
        let tx = c.transaction()?;
        let parent_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE path=?1)",
            [&parent.0],
            |r| r.get(0),
        )?;
        if !parent_exists {
            return Err(StoreError::MissingAgentParent(parent.0.clone()));
        }

        let source_head = if let Some(request) = invocation_request {
            let branch: Option<String> = tx
                .query_row(
                    "SELECT branch FROM requests WHERE id=?1",
                    [&request.0],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(branch) = branch else {
                return Err(StoreError::MissingRequest(request.0.clone()));
            };
            if branch != parent.0 {
                return Err(StoreError::RequestAgentMismatch {
                    request: request.0.clone(),
                    expected: parent.0.clone(),
                    actual: branch,
                });
            }
            Some(request.clone())
        } else {
            tx.query_row(
                "SELECT head_request FROM agents WHERE path=?1",
                [&parent.0],
                |row| Ok(row.get::<_, Option<String>>(0)?.map(RequestId)),
            )?
        };
        let stored_history: Vec<(RequestId, Item)> = if let Some(head) = source_head.as_ref() {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM requests WHERE id=?1)",
                [&head.0],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(StoreError::MissingRequest(head.0.clone()));
            }
            let mut q = tx.prepare(
                "WITH RECURSIVE lineage(id,parent_id,depth) AS (
                     SELECT id,parent_id,0 FROM requests WHERE id=?1
                     UNION ALL
                     SELECT r.id,r.parent_id,lineage.depth+1
                     FROM requests r JOIN lineage ON r.id=lineage.parent_id
                     WHERE NOT EXISTS(
                         SELECT 1 FROM session_state s
                         WHERE s.session_id='harness:compaction:' || lineage.id
                     )
                 )
                 SELECT lineage.id,i.json FROM lineage
                 JOIN request_items ri ON ri.request_id=lineage.id
                 JOIN items i ON i.hash=ri.item_hash
                 ORDER BY lineage.depth DESC,ri.position",
            )?;
            q.query_map([&head.0], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (request, raw) = row?;
                Ok((RequestId(request), serde_json::from_str::<Item>(&raw)?))
            })
            .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let history: Vec<Item> =
            if let (Some(request), Some(call_id)) = (invocation_request, invocation_call_id) {
                let end = stored_history.iter().position(|(item_request, item)| {
                    item_request == request
                        && item.0["type"] == "function_call"
                        && item.0["call_id"].as_str() == Some(&call_id.0)
                        && item.0["name"].as_str() == Some("spawn_agent")
                });
                let Some(end) = end else {
                    return Err(StoreError::MissingActiveSpawnCall {
                        request: request.0.clone(),
                        call_id: call_id.0.clone(),
                    });
                };
                stored_history
                    .into_iter()
                    .take(end + 1)
                    .map(|(_, item)| item)
                    .collect()
            } else {
                stored_history.into_iter().map(|(_, item)| item).collect()
            };
        let inherited_calls = if let Some(head) = source_head.as_ref() {
            let mut q = tx.prepare(
                "WITH RECURSIVE lineage(id,parent_id) AS (
                     SELECT id,parent_id FROM requests WHERE id=?1
                     UNION ALL
                     SELECT r.id,r.parent_id FROM requests r JOIN lineage ON r.id=lineage.parent_id
                 )
                 SELECT DISTINCT c.call_id FROM claims c
                 JOIN lineage ON lineage.id=c.request_id
                 WHERE c.state='pending' ORDER BY c.call_id",
            )?;
            q.query_map([&head.0], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        let child_effort = history
            .iter()
            .rev()
            .find_map(Item::configuration_effort)
            .unwrap_or(Effort::Low);
        let snapshot: Vec<_> = history
            .into_iter()
            .filter(|item| !Self::strip_from_here_snapshot(item))
            .collect();
        let source = serde_json::json!({
            "kind":"here",
            "source_head_request":source_head,
            "snapshot_request":snapshot_request,
            "invocation_request":invocation_request,
            "invocation_call_id":invocation_call_id
        });

        tx.execute(
            "INSERT INTO requests(id,parent_id,branch,created_at,input_tokens,output_tokens,cost_micros) VALUES (?1,NULL,?2,?3,0,0,0)",
            params![snapshot_request.0, path.0, utc_millis()],
        )?;
        for (position, item) in snapshot.iter().enumerate() {
            let hash = Self::put_item_tx(&tx, item)?;
            tx.execute(
                "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                params![snapshot_request.0, position as i64, hash.0],
            )?;
        }
        for call_id in inherited_calls {
            tx.execute(
                "INSERT INTO claims(call_id,request_id,state) VALUES (?1,?2,'pending')",
                params![call_id, snapshot_request.0],
            )?;
        }
        // The existing trusted writer is factored into a transaction helper,
        // so this is exactly one fresh pin in the same admission transaction.
        Self::set_effort_tx(&tx, snapshot_request, child_effort)?;
        let created_at = utc_millis();
        tx.execute(
            "INSERT INTO agents(path,parent_path,head_request,contract,fork_source,state,created_at) VALUES (?1,?2,?3,?4,?5,'active',?6)",
            params![
                path.0,
                parent.0,
                snapshot_request.0,
                serde_json::to_string(contract)?,
                serde_json::to_string(&source)?,
                created_at
            ],
        )?;
        let hash = Self::put_item_tx(&tx, task_item)?;
        tx.execute(
            "INSERT INTO envelopes(sender,recipient,class,item_hash,delivered_request,created_at) VALUES (?1,?2,?3,?4,NULL,?5)",
            params![sender, recipient, class, hash.0, utc_millis()],
        )?;
        let envelope_id = tx.last_insert_rowid();
        tx.commit()?;
        Ok((
            Agent {
                path: path.clone(),
                parent: Some(parent.clone()),
                head_request: Some(snapshot_request.clone()),
                contract: contract.clone(),
                fork_source: source,
                state: AgentState::Active,
                created_at,
            },
            envelope_id,
        ))
    }

    fn strip_from_here_snapshot(item: &Item) -> bool {
        item.is_configuration_update()
            || matches!(
                item.0.get("type").and_then(serde_json::Value::as_str),
                Some("annotation" | "watchdog_annotation" | "dropped_claim")
            )
    }
    pub fn agent(&self, path: &AgentPath) -> Result<Option<Agent>> {
        self.lock().query_row("SELECT path,parent_path,head_request,contract,fork_source,state,created_at FROM agents WHERE path=?1",[&path.0],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?.map(Self::decode_agent).transpose()
    }
    pub fn children_agents(&self, path: &AgentPath) -> Result<Vec<Agent>> {
        if path.0 == "/operator" {
            return self
                .agent(&AgentPath("/root".into()))
                .map(|root| root.into_iter().collect());
        }
        self.query_agents("SELECT path,parent_path,head_request,contract,fork_source,state,created_at FROM agents WHERE parent_path=?1 ORDER BY path",Some(&path.0))
    }
    pub fn list_agents(&self) -> Result<Vec<Agent>> {
        self.query_agents("SELECT path,parent_path,head_request,contract,fork_source,state,created_at FROM agents ORDER BY path",None)
    }
    fn query_agents(&self, sql: &str, arg: Option<&str>) -> Result<Vec<Agent>> {
        let c = self.lock();
        let mut q = c.prepare(sql)?;
        let decode = |r: &rusqlite::Row<'_>| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
            ))
        };
        let rows = if let Some(arg) = arg {
            q.query_map([arg], decode)?.collect::<std::result::Result<
                Vec<(
                    String,
                    Option<String>,
                    Option<String>,
                    String,
                    String,
                    String,
                    i64,
                )>,
                _,
            >>()?
        } else {
            q.query_map([], decode)?.collect::<std::result::Result<
                Vec<(
                    String,
                    Option<String>,
                    Option<String>,
                    String,
                    String,
                    String,
                    i64,
                )>,
                _,
            >>()?
        };
        rows.into_iter().map(Self::decode_agent).collect()
    }
    pub fn advance_agent_head(
        &self,
        path: &AgentPath,
        expected: Option<&RequestId>,
        current: Option<&RequestId>,
    ) -> Result<bool> {
        Ok(self.lock().execute(
            "UPDATE agents SET head_request=?3 WHERE path=?1 AND head_request IS ?2",
            params![path.0, expected.map(|x| &x.0), current.map(|x| &x.0)],
        )? == 1)
    }
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
        for (position, item) in items
            .iter()
            .filter(|item| !item.is_configuration_update())
            .enumerate()
        {
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
    /// Install a trusted compacted window atomically. The parent edge keeps the
    /// source request and its claims queryable; history replay stops here.
    pub(crate) fn write_compaction_request(
        &self,
        request: &RequestId,
        parent: &RequestId,
        branch: &str,
        items: &[Item],
    ) -> Result<()> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        tx.execute(
            "INSERT INTO requests(id,parent_id,branch,created_at) VALUES (?1,?2,?3,?4)",
            params![request.0, parent.0, branch, utc_millis()],
        )?;
        for (position, item) in items.iter().enumerate() {
            let hash = Self::put_item_tx(&tx, item)?;
            tx.execute(
                "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                params![request.0, position as i64, hash.0],
            )?;
        }
        let key = format!("harness:compaction:{}", request.0);
        tx.execute(
            "INSERT INTO session_state(session_id,state,updated_at) VALUES (?1,'true',?2)",
            params![key, utc_millis()],
        )?;
        tx.execute(
            "INSERT INTO events(request_id,kind,payload,created_at) VALUES (?1,'compaction',?2,?3)",
            params![
                request.0,
                serde_json::json!({"source":parent.0}).to_string(),
                utc_millis()
            ],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn is_compaction_boundary(&self, request: &RequestId) -> Result<bool> {
        let key = format!("harness:compaction:{}", request.0);
        Ok(self.session_state(&key)?.is_some())
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
        if session_id.starts_with("harness:") {
            return Err(StoreError::ReservedSessionStateNamespace(
                session_id.to_owned(),
            ));
        }
        self.save_session_state_inner(session_id, state)
    }

    fn save_session_state_inner(&self, session_id: &str, state: &serde_json::Value) -> Result<()> {
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
    // Model/client input cannot author settings. Trusted settings enter only
    // through set_effort (also used by fork and compaction re-pins).
    pub fn append_items(&self, request: &RequestId, items: &[Item]) -> Result<Vec<ItemHash>> {
        let items: Vec<_> = items
            .iter()
            .filter(|item| !item.is_configuration_update())
            .collect();
        self.append_items_inner(request, &items)
    }
    fn append_items_inner(&self, request: &RequestId, items: &[&Item]) -> Result<Vec<ItemHash>> {
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

    /// Append the harness-owned effort item, replacing an immediately adjacent
    /// update so a second change before the next response does not grow history.
    pub(crate) fn set_effort(&self, request: &RequestId, effort: Effort) -> Result<ItemHash> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let hash = Self::set_effort_tx(&tx, request, effort)?;
        tx.commit()?;
        Ok(hash)
    }
    // The single trusted positional-setting writer. Both the ordinary
    // Store::set_effort API and atomic pending-setting consumption route here;
    // model-authored items still cannot reach it through append_items.
    fn set_effort_tx(
        tx: &Transaction<'_>,
        request: &RequestId,
        effort: Effort,
    ) -> Result<ItemHash> {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM requests WHERE id=?1)",
            [&request.0],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(StoreError::MissingRequest(request.0.clone()));
        }
        let last: Option<(i64, String)> = tx
            .query_row(
                "SELECT ri.position,i.json FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id=?1 ORDER BY ri.position DESC LIMIT 1",
                [&request.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (position, replace) = match last {
            Some((position, raw)) => {
                let previous: Item = serde_json::from_str(&raw)?;
                (position, previous.is_configuration_update())
            }
            None => (0, false),
        };
        let position = if replace {
            tx.execute(
                "DELETE FROM request_items WHERE request_id=?1 AND position=?2",
                params![request.0, position],
            )?;
            position
        } else {
            position
                + i64::from(tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM request_items WHERE request_id=?1)",
                    [&request.0],
                    |r| r.get::<_, bool>(0),
                )?)
        };
        let hash = Self::put_item_tx(tx, &Item::configuration_update(effort))?;
        tx.execute(
            "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
            params![request.0, position, hash.0],
        )?;
        Ok(hash)
    }
    fn pending_effort_key(agent: &AgentPath) -> String {
        format!("harness:pending_effort:{}", agent.0)
    }

    /// Persist the latest model-facing effort change until the agent's next
    /// request boundary.
    pub(crate) fn save_pending_effort(&self, agent: &AgentPath, effort: Effort) -> Result<()> {
        self.save_session_state_inner(
            &Self::pending_effort_key(agent),
            &serde_json::to_value(effort)?,
        )
    }

    /// Atomically consume a pending effort into the new request. A failure
    /// rolls back both its deletion and the positional setting item.
    pub(crate) fn apply_pending_effort(
        &self,
        agent: &AgentPath,
        request: &RequestId,
    ) -> Result<Option<ItemHash>> {
        let key = Self::pending_effort_key(agent);
        let mut c = self.lock();
        let tx = c.transaction()?;
        let pending: Option<String> = tx
            .query_row(
                "SELECT state FROM session_state WHERE session_id=?1",
                [&key],
                |r| r.get(0),
            )
            .optional()?;
        let Some(pending) = pending else {
            return Ok(None);
        };
        let effort: Effort = serde_json::from_str(&pending)?;
        let hash = Self::set_effort_tx(&tx, request, effort)?;
        tx.execute("DELETE FROM session_state WHERE session_id=?1", [&key])?;
        tx.commit()?;
        Ok(Some(hash))
    }

    /// Atomically attach every unread envelope for `recipient` to `request` and
    /// mark those envelopes delivered. A retry after commit returns an empty
    /// vector; a failed transaction leaves the envelopes unread.
    pub fn append_unread_envelopes(
        &self,
        recipient: &AgentPath,
        request: &RequestId,
    ) -> Result<Vec<Item>> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let branch: Option<String> = tx
            .query_row(
                "SELECT branch FROM requests WHERE id=?1",
                [&request.0],
                |r| r.get(0),
            )
            .optional()?;
        let Some(branch) = branch else {
            return Err(StoreError::MissingRequest(request.0.clone()));
        };
        if branch != recipient.0 {
            return Err(StoreError::RequestAgentMismatch {
                request: request.0.clone(),
                expected: recipient.0.clone(),
                actual: branch,
            });
        }

        let envelopes = {
            let mut q = tx.prepare(
                "SELECT e.id,e.item_hash,i.json FROM envelopes e JOIN items i ON i.hash=e.item_hash \
                 WHERE e.recipient=?1 AND e.delivered_request IS NULL ORDER BY e.id",
            )?;
            q.query_map([&recipient.0], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position)+1,0) FROM request_items WHERE request_id=?1",
            [&request.0],
            |r| r.get(0),
        )?;
        let mut items = Vec::with_capacity(envelopes.len());
        for (_, hash, json) in &envelopes {
            let item = serde_json::from_str::<Item>(json)?;
            // Envelopes are untrusted ingress, not a trusted settings writer.
            // Mark configuration updates delivered below, but never attach
            // them to model history through this path.
            if item.is_configuration_update() {
                continue;
            }
            tx.execute(
                "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                params![request.0, position, hash],
            )?;
            position += 1;
            items.push(item);
        }
        for (envelope_id, _, _) in &envelopes {
            tx.execute(
                "UPDATE envelopes SET delivered_request=?2 WHERE id=?1 AND delivered_request IS NULL",
                params![envelope_id, request.0],
            )?;
        }
        tx.commit()?;
        Ok(items)
    }
    pub fn items(&self, request: &RequestId) -> Result<Vec<Item>> {
        let c = self.lock();
        let mut q=c.prepare("SELECT i.json FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id=?1 ORDER BY ri.position")?;
        q.query_map([&request.0], |r| r.get::<_, String>(0))?
            .map(|x| Ok(serde_json::from_str(&x?)?))
            .collect()
    }
    /// Copy a spawn tool's actual durable output from its parent request into
    /// the Here snapshot. Claim settlement alone is insufficient: this returns
    /// `false` until the function_call_output Item is in request history.
    pub(crate) fn copy_call_output_if_persisted(
        &self,
        source: &RequestId,
        target: &RequestId,
        call_id: &CallId,
    ) -> Result<bool> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let source_items = {
            let mut q = tx.prepare(
                "SELECT i.json FROM request_items ri JOIN items i ON i.hash=ri.item_hash
                 WHERE ri.request_id=?1 ORDER BY ri.position",
            )?;
            q.query_map([&source.0], |row| row.get::<_, String>(0))?
                .map(|raw| Ok(serde_json::from_str::<Item>(&raw?)?))
                .collect::<Result<Vec<_>>>()?
        };
        let Some(output) = source_items.into_iter().find(|item| {
            item.0["type"] == "function_call_output"
                && item.0["call_id"].as_str() == Some(&call_id.0)
        }) else {
            return Ok(false);
        };
        let target_items = {
            let mut q = tx.prepare(
                "SELECT i.json FROM request_items ri JOIN items i ON i.hash=ri.item_hash
                 WHERE ri.request_id=?1 ORDER BY ri.position",
            )?;
            q.query_map([&target.0], |row| row.get::<_, String>(0))?
                .map(|raw| Ok(serde_json::from_str::<Item>(&raw?)?))
                .collect::<Result<Vec<_>>>()?
        };
        if target_items.iter().any(|item| {
            item.0["type"] == "function_call_output"
                && item.0["call_id"].as_str() == Some(&call_id.0)
        }) {
            tx.commit()?;
            return Ok(true);
        }
        let Some(spawn_position) = target_items.iter().position(|item| {
            item.0["type"] == "function_call"
                && item.0["call_id"].as_str() == Some(&call_id.0)
                && item.0["name"].as_str() == Some("spawn_agent")
        }) else {
            return Ok(false);
        };
        let mut ordered = target_items;
        ordered.insert(spawn_position + 1, output);
        tx.execute("DELETE FROM request_items WHERE request_id=?1", [&target.0])?;
        for (position, item) in ordered.iter().enumerate() {
            let hash = Self::put_item_tx(&tx, item)?;
            tx.execute(
                "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                params![target.0, position as i64, hash.0],
            )?;
        }
        tx.commit()?;
        Ok(true)
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
    /// Persist the completed model batch separately from later tool outputs
    /// appended to the same request. The event is written only after Engine
    /// has persisted all completed response items.
    pub fn record_replay_turn(
        &self,
        request: &RequestId,
        model_request: &ResponsesRequest,
        model_response: &ResponsesTurn,
    ) -> Result<i64> {
        let record = RecordedReplayTurn {
            request: request.clone(),
            model_request: model_request.clone(),
            model_response: model_response.clone(),
        };
        self.record_event(Some(request), "model_turn", &serde_json::to_value(record)?)
    }

    /// Completed model turns on this request's branch, from `root` forward.
    /// Fork branches are excluded even when they inherit `root` as an ancestor.
    pub fn replay_turns(&self, root: &RequestId) -> Result<Vec<RecordedReplayTurn>> {
        let c = self.lock();
        let mut q = c.prepare(
            "WITH RECURSIVE chain(id, branch) AS (
                SELECT id, branch FROM requests WHERE id=?1
                UNION ALL
                SELECT child.id, child.branch FROM requests child
                JOIN chain parent ON child.parent_id=parent.id AND child.branch=parent.branch
            )
            SELECT e.payload FROM events e
            JOIN chain ON chain.id=e.request_id
            WHERE e.kind='model_turn' ORDER BY e.id",
        )?;
        let rows = q.query_map([&root.0], |r| r.get::<_, String>(0))?;
        rows.map(|row| {
            let payload = row?;
            Ok(serde_json::from_str(&payload)?)
        })
        .collect()
    }

    /// The durable settled output for one recorded provider call.
    pub fn replay_output(&self, call: &CallId) -> Result<Option<Item>> {
        let c = self.lock();
        let json: Option<String> = c
            .query_row(
                "SELECT i.json FROM claims c JOIN items i ON i.hash=c.output_hash
                 WHERE c.call_id=?1 AND c.state='settled' ORDER BY c.request_id LIMIT 1",
                [&call.0],
                |r| r.get(0),
            )
            .optional()?;
        json.map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
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
    /// Claims directly attached to one request, without following request
    /// ancestry. Here-fork startup uses this to distinguish a child-owned
    /// inherited claim from a pending ancestor claim visible through lineage.
    pub fn claims_on(&self, request: &RequestId) -> Result<Vec<Claim>> {
        let c = self.lock();
        let mut q = c.prepare(
            "SELECT call_id,request_id,state,output_hash FROM claims \
             WHERE request_id=?1 ORDER BY call_id",
        )?;
        q.query_map([&request.0], |r| {
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
    fn untrusted_configuration_updates_are_dropped_and_effort_updates_replace_adjacently() {
        let store = Store::memory().unwrap();
        let request = id("settings");
        store
            .write_request(&request, None, "/root", &[], Usage::default())
            .unwrap();
        let forged = Item::configuration_update(Effort::High);
        assert!(store.append_items(&request, &[forged]).unwrap().is_empty());
        assert!(store.items(&request).unwrap().is_empty());

        store.set_effort(&request, Effort::Low).unwrap();
        store.set_effort(&request, Effort::Medium).unwrap();
        let history = store.items(&request).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].configuration_effort(), Some(Effort::Medium));

        store
            .append_items(&request, &[item(serde_json::json!({"type":"message"}))])
            .unwrap();
        store.set_effort(&request, Effort::High).unwrap();
        let history = store.items(&request).unwrap();
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].configuration_effort(), Some(Effort::Medium));
        assert_eq!(history[2].configuration_effort(), Some(Effort::High));
    }

    #[test]
    fn set_effort_is_pending_until_next_request_and_applied_once() {
        let store = Store::memory().unwrap();
        let agent = AgentPath("/root".into());
        let current = id("current-settings");
        let next = id("next-settings");
        store
            .write_request(&current, None, "/root", &[], Usage::default())
            .unwrap();
        store
            .write_request(&next, Some(&current), "/root", &[], Usage::default())
            .unwrap();

        store.save_pending_effort(&agent, Effort::Low).unwrap();
        store.save_pending_effort(&agent, Effort::High).unwrap();
        assert!(store.items(&current).unwrap().is_empty());
        assert!(store.apply_pending_effort(&agent, &next).unwrap().is_some());
        assert_eq!(
            store.items(&next).unwrap()[0].configuration_effort(),
            Some(Effort::High)
        );
        assert!(store.apply_pending_effort(&agent, &next).unwrap().is_none());
    }

    #[test]
    fn public_session_state_reserves_harness_namespace_but_accepts_demo_key() {
        let store = Store::memory().unwrap();
        let demo_key = "harness-demo-server:/root";
        let state = serde_json::json!({"cursor":7});
        store.save_session_state(demo_key, &state).unwrap();
        assert_eq!(
            store.session_state(demo_key).unwrap().unwrap().state,
            state.to_string()
        );
        assert!(matches!(
            store.save_session_state("harness:pending_effort:/root", &state),
            Err(StoreError::ReservedSessionStateNamespace(_))
        ));

        let agent = AgentPath("/root".into());
        store.save_pending_effort(&agent, Effort::High).unwrap();
        assert_eq!(
            store
                .session_state("harness:pending_effort:/root")
                .unwrap()
                .unwrap()
                .state,
            "\"high\""
        );
    }

    #[test]
    fn initial_request_input_cannot_forge_configuration_updates() {
        let store = Store::memory().unwrap();
        let request = id("initial-settings");
        store
            .write_request(
                &request,
                None,
                "/root",
                &[
                    item(serde_json::json!({"type":"message","content":"keep"})),
                    Item::configuration_update(Effort::High),
                ],
                Usage::default(),
            )
            .unwrap();
        let history = store.items(&request).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].0["type"], "message");
    }

    #[test]
    fn atomic_agent_task_admission_commits_or_rolls_back_as_one_unit() {
        let store = Store::memory().unwrap();
        let root = AgentPath("/root".into());
        store
            .admit_agent(
                &root,
                None,
                None,
                &serde_json::json!({}),
                &serde_json::json!({}),
            )
            .unwrap();
        let child = AgentPath("/root/worker".into());
        let task = item(serde_json::json!({"type":"message","content":"new task"}));
        let (agent, envelope_id) = store
            .admit_agent_with_envelope(
                &child,
                Some(&root),
                None,
                &serde_json::json!({"contract":1}),
                &serde_json::json!({"source":"test"}),
                "/root",
                "/root/worker",
                "NEW_TASK",
                &task,
            )
            .unwrap();
        assert_eq!(agent.path, child);
        let inbox = store.inbox("/root/worker").unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].id, envelope_id);
        assert_eq!(inbox[0].class, "NEW_TASK");

        // Force the second insert to fail after the agent and content write.
        store.lock().execute_batch(
            "CREATE TRIGGER reject_task BEFORE INSERT ON envelopes BEGIN SELECT RAISE(ABORT, 'rejected'); END;"
        ).unwrap();
        let rollback_path = AgentPath("/root/rejected".into());
        assert!(
            store
                .admit_agent_with_envelope(
                    &rollback_path,
                    Some(&root),
                    None,
                    &serde_json::json!({}),
                    &serde_json::json!({}),
                    "/root",
                    "/root/rejected",
                    "NEW_TASK",
                    &item(serde_json::json!({"rollback":true})),
                )
                .is_err()
        );
        assert!(store.agent(&rollback_path).unwrap().is_none());
        let item_count: i64 = store
            .lock()
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        assert_eq!(item_count, 1);

        // A duplicate path is rejected without adding another envelope.
        assert!(
            store
                .admit_agent_with_envelope(
                    &child,
                    Some(&root),
                    None,
                    &serde_json::json!({}),
                    &serde_json::json!({}),
                    "/root",
                    "/root/worker",
                    "NEW_TASK",
                    &task,
                )
                .is_err()
        );
        assert_eq!(store.inbox("/root/worker").unwrap().len(), 1);
        assert!(matches!(
            store.admit_agent_with_envelope(
                &AgentPath("/root/no_parent/child".into()),
                Some(&AgentPath("/root/no_parent".into())),
                None,
                &serde_json::json!({}),
                &serde_json::json!({}),
                "/root",
                "/root/no_parent/child",
                "NEW_TASK",
                &task,
            ),
            Err(StoreError::MissingAgentParent(_))
        ));
    }
    #[test]
    fn replay_turns_and_outputs_survive_reopen_without_flat_history_guessing() {
        let path = std::env::temp_dir().join(format!("harness-replay-{}.db", uuid::Uuid::new_v4()));
        let root = id("replay-root");
        let next = id("replay-next");
        let fork = id("replay-fork");
        let call = CallId("replay-call".into());
        let call_item = item(serde_json::json!({
            "type":"function_call","call_id":call.0,"name":"cell","arguments":"{}"
        }));
        let output = item(serde_json::json!({
            "type":"function_call_output","call_id":call.0,"output":"{\"value\":\"done\"}"
        }));
        let model_request = ResponsesRequest {
            input: vec![item(
                serde_json::json!({"role":"user","content":"run cell"}),
            )],
            instructions: "reply by tool".into(),
            tools: vec![serde_json::json!({"type":"function","name":"cell"})],
            model: "offline-recording".into(),
            pinned_effort: Effort::Low,
            session_id: "recorded-session".into(),
        };
        {
            let s = Store::open(&path).unwrap();
            s.create_request(&root, None, "/root").unwrap();
            s.create_request(&next, Some(&root), "/root").unwrap();
            s.create_request(&fork, Some(&root), "/root/child").unwrap();
            s.append_items(&root, std::slice::from_ref(&call_item))
                .unwrap();
            s.record_replay_turn(
                &root,
                &model_request,
                &ResponsesTurn {
                    response_id: "recorded-call".into(),
                    items: vec![call_item.clone()],
                    usage: Default::default(),
                },
            )
            .unwrap();
            s.claim(&call, &root).unwrap();
            s.write_output(&call, &output).unwrap();
            s.append_items(&next, std::slice::from_ref(&output))
                .unwrap();
            s.record_replay_turn(
                &fork,
                &model_request,
                &ResponsesTurn {
                    response_id: "fork-only".into(),
                    items: vec![],
                    usage: Default::default(),
                },
            )
            .unwrap();
        }
        {
            let s = Store::open(&path).unwrap();
            let turns = s.replay_turns(&root).unwrap();
            assert_eq!(turns.len(), 1);
            assert_eq!(turns[0].request, root);
            assert_eq!(
                serde_json::to_value(&turns[0].model_request).unwrap(),
                serde_json::to_value(&model_request).unwrap()
            );
            assert_eq!(turns[0].model_response.response_id, "recorded-call");
            assert_eq!(turns[0].model_response.items, vec![call_item]);
            assert_eq!(s.replay_output(&call).unwrap(), Some(output));
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
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
    fn unread_envelopes_commit_once_in_order_and_survive_reopen() {
        let path = std::env::temp_dir().join(format!("harness-inbox-{}.db", uuid::Uuid::new_v4()));
        let first = item(serde_json::json!({"n":1}));
        let second = item(serde_json::json!({"n":2}));
        {
            let s = Store::open(&path).unwrap();
            s.create_request(&id("request"), None, "/root/worker")
                .unwrap();
            s.add_envelope("a", "/root/worker", "message", &first, None)
                .unwrap();
            s.add_envelope("b", "/root/worker", "message", &second, None)
                .unwrap();
            s.add_envelope("b", "/root/other", "message", &first, None)
                .unwrap();
            let recipient = AgentPath("/root/worker".into());
            assert_eq!(
                s.append_unread_envelopes(&recipient, &id("request"))
                    .unwrap(),
                vec![first.clone(), second.clone()]
            );
            assert!(s.unread("/root/worker").unwrap().is_empty());
            assert_eq!(
                s.append_unread_envelopes(&recipient, &id("request"))
                    .unwrap(),
                Vec::<Item>::new()
            );
            assert_eq!(
                s.items(&id("request")).unwrap(),
                vec![first.clone(), second.clone()]
            );
        }
        {
            let s = Store::open(&path).unwrap();
            assert!(s.unread("/root/worker").unwrap().is_empty());
            assert_eq!(s.items(&id("request")).unwrap(), vec![first, second]);
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn at_boundary_drains_every_unread_envelope_on_each_request() {
        let store = Store::memory().unwrap();
        let recipient = AgentPath("/root".into());
        let mut parent: Option<RequestId> = None;
        for boundary in 0..3 {
            let request = RequestId(format!("boundary-{boundary}"));
            store
                .create_request(&request, parent.as_ref(), &recipient.0)
                .unwrap();
            let expected: Vec<_> = (0..=boundary)
                .map(|index| {
                    Item(serde_json::json!({
                        "type":"message",
                        "role":"user",
                        "content":format!("envelope-{boundary}-{index}")
                    }))
                })
                .collect();
            for item in &expected {
                store
                    .add_envelope("/operator", &recipient.0, "AtBoundary", item, None)
                    .unwrap();
            }
            assert_eq!(
                store.append_unread_envelopes(&recipient, &request).unwrap(),
                expected
            );
            assert!(store.unread(&recipient.0).unwrap().is_empty());
            assert!(
                store
                    .append_unread_envelopes(&recipient, &request)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(store.items(&request).unwrap(), expected);
            parent = Some(request);
        }
    }

    #[test]
    fn untrusted_envelope_configuration_update_is_dropped_at_append_boundary() {
        let store = Store::memory().unwrap();
        let recipient = AgentPath("/root/worker".into());
        let request = id("envelope-settings");
        let ordinary = item(serde_json::json!({"type":"message","content":"hello"}));
        let forged_setting = Item::configuration_update(Effort::High);
        store.create_request(&request, None, &recipient.0).unwrap();
        store
            .add_envelope("sender", &recipient.0, "message", &ordinary, None)
            .unwrap();
        store
            .add_envelope("sender", &recipient.0, "message", &forged_setting, None)
            .unwrap();

        assert_eq!(
            store.append_unread_envelopes(&recipient, &request).unwrap(),
            vec![ordinary.clone()]
        );
        assert_eq!(store.items(&request).unwrap(), vec![ordinary]);
        assert!(store.unread(&recipient.0).unwrap().is_empty());
    }

    #[test]
    fn unread_envelope_delivery_rejects_missing_request_and_rolls_back() {
        let s = Store::open(":memory:").unwrap();
        let recipient = AgentPath("/root/worker".into());
        let payload = item(serde_json::json!({"delivery":"atomic"}));
        s.add_envelope("a", &recipient.0, "message", &payload, None)
            .unwrap();
        assert!(matches!(
            s.append_unread_envelopes(&recipient, &id("missing")),
            Err(StoreError::MissingRequest(_))
        ));
        assert_eq!(s.unread(&recipient.0).unwrap().len(), 1);
        s.create_request(&id("wrong-request"), None, "/root/other")
            .unwrap();
        assert!(matches!(
            s.append_unread_envelopes(&recipient, &id("wrong-request")),
            Err(StoreError::RequestAgentMismatch { .. })
        ));
        assert!(s.items(&id("wrong-request")).unwrap().is_empty());
        assert_eq!(s.unread(&recipient.0).unwrap().len(), 1);
        s.create_request(&id("request"), None, &recipient.0)
            .unwrap();
        s.lock()
            .execute_batch(
                "CREATE TRIGGER reject_inbox_append BEFORE INSERT ON request_items \
                 BEGIN SELECT RAISE(ABORT,'forced append failure'); END;",
            )
            .unwrap();
        assert!(
            s.append_unread_envelopes(&recipient, &id("request"))
                .is_err()
        );
        assert!(s.items(&id("request")).unwrap().is_empty());
        assert_eq!(s.unread(&recipient.0).unwrap().len(), 1);
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
        assert_eq!(version, 3);
        assert!(schema::initialize(&mut conn).is_ok());
    }

    #[test]
    fn migrates_v2_without_rewriting_request_timestamp() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE schema_version(version INTEGER NOT NULL); INSERT INTO schema_version VALUES(2); CREATE TABLE requests(id TEXT PRIMARY KEY,parent_id TEXT,branch TEXT,created_at INTEGER NOT NULL,input_tokens INTEGER NOT NULL DEFAULT 0,output_tokens INTEGER NOT NULL DEFAULT 0,cost_micros INTEGER NOT NULL DEFAULT 0); INSERT INTO requests VALUES('kept',NULL,'main',123456,7,8,9);").unwrap();
        schema::initialize(&mut conn).unwrap();
        let preserved: (i64, i64, i64, i64) = conn
            .query_row("SELECT created_at,input_tokens,output_tokens,cost_micros FROM requests WHERE id='kept'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))
            .unwrap();
        assert_eq!(preserved, (123456, 7, 8, 9));
        let agents: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agents')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(agents);
        let version: u32 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 3);
    }

    #[test]
    fn here_snapshot_admission_rolls_back_request_claim_and_envelope_on_agent_conflict() {
        let store = Store::memory().unwrap();
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        let source = id("here-parent");
        let snapshot = id("here-child-snapshot");
        let call = CallId("here-pending-call".into());
        store.create_request(&source, None, &root.0).unwrap();
        store
            .append_items(
                &source,
                &[item(serde_json::json!({
                    "type":"function_call","call_id":call.0,"name":"slow","arguments":"{}"
                }))],
            )
            .unwrap();
        store.set_effort(&source, Effort::Medium).unwrap();
        store
            .admit_agent(
                &root,
                None,
                Some(&source),
                &serde_json::json!({}),
                &serde_json::json!({}),
            )
            .unwrap();
        store.claim(&call, &source).unwrap();
        store
            .admit_agent(
                &child,
                Some(&root),
                Some(&source),
                &serde_json::json!({}),
                &serde_json::json!({"kind":"preexisting"}),
            )
            .unwrap();

        assert!(
            store
                .admit_here_agent_with_snapshot(
                    &child,
                    &root,
                    &snapshot,
                    &serde_json::json!({}),
                    &root.0,
                    &child.0,
                    "AtBoundary",
                    &item(serde_json::json!({
                        "type":"message","role":"assistant","content":"NEW_TASK"
                    })),
                )
                .is_err()
        );
        assert_eq!(store.request(&snapshot).unwrap(), None);
        assert!(store.claims_on(&snapshot).unwrap().is_empty());
        assert!(store.unread(&child.0).unwrap().is_empty());
        assert_eq!(store.claims(&call).unwrap()[0].state, ClaimState::Pending);
    }

    #[test]
    fn fresh_agent_has_no_head_and_head_cas_handles_null() {
        let store = Store::memory().unwrap();
        let root = AgentPath("/root".into());
        let fresh = store
            .admit_agent(
                &root,
                None,
                None,
                &serde_json::json!({"task":"fresh"}),
                &serde_json::json!({"source":"branch"}),
            )
            .unwrap();
        assert_eq!(fresh.head_request, None);
        let child = AgentPath("/root/child_1".into());
        assert!(
            store
                .admit_agent(
                    &child,
                    Some(&root),
                    None,
                    &serde_json::json!({}),
                    &serde_json::json!({})
                )
                .is_ok()
        );
        assert_eq!(store.children_agents(&root).unwrap().len(), 1);
        assert_eq!(store.list_agents().unwrap().len(), 2);
        let request = RequestId("r1".into());
        store.create_request(&request, None, "main").unwrap();
        assert!(
            store
                .advance_agent_head(&root, None, Some(&request))
                .unwrap()
        );
        assert!(!store.advance_agent_head(&root, None, None).unwrap());
        assert_eq!(
            store.agent(&root).unwrap().unwrap().head_request,
            Some(request)
        );
        assert!(matches!(
            store.admit_agent(
                &AgentPath("/root/a-b".into()),
                Some(&root),
                None,
                &serde_json::json!({}),
                &serde_json::json!({})
            ),
            Err(StoreError::InvalidAgentPath(_))
        ));
        assert!(matches!(
            store.admit_agent(
                &AgentPath("/root/Upper".into()),
                Some(&root),
                None,
                &serde_json::json!({}),
                &serde_json::json!({})
            ),
            Err(StoreError::InvalidAgentPath(_))
        ));
    }
}
