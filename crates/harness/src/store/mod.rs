//! Durable SQLite event and content-addressed request store.
mod context;
mod embedded;
mod embedded_commands;
mod embedded_round;
pub(crate) use embedded::{CommandInputAdmission, EmbeddedInputState};
pub use embedded_commands::{EmbeddedCommandRecord, EmbeddedCommandState};
pub use embedded_round::{EmbeddedRoundFrontier, EmbeddedRoundOutcome};
pub mod history;
mod replay;
pub mod schema;
mod schema_migration;
mod terminal;
mod validation;
mod wait_continuation;
pub use replay::IssuedReplayRequest;
pub use terminal::{RecordedToolOutput, TerminalOutcome};
pub(crate) use wait_continuation::RecordedWaitContinuation;

use crate::{
    item::{Item, ItemHash, ToolKind},
    lifecycle::{CompletionCommit, CompletionProvenance},
    model::{AgentPath, CallId, ConversationIdentity, Effort, OperationId, RequestId},
    transport::{ResponsesRequest, ResponsesTurn},
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub const VERSION: u32 = schema::VERSION;
pub const SQL: &str = schema::SQL;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Context(#[from] crate::context::ContextError),
    #[error("settled operation {operation:?} has no typed replay outcome; preserve its bytes")]
    UnsupportedReplayOutcome { operation: OperationId },
    #[error("wait replay continuation does not match exact issued operation {operation:?}")]
    InvalidWaitContinuation { operation: OperationId },
    #[error("settled operation {operation:?} has conflicting replay outcomes")]
    ConflictingReplayOutcome { operation: OperationId },
    #[error("replay event {event} has unsupported format; preserve its bytes")]
    UnsupportedReplayFormat { event: i64 },
    #[error("replay event references missing item {0}")]
    MissingReplayItem(String),
    #[error("operation ID already identifies a different host command")]
    ConflictingCommand,
    #[error("command does not have the required durable claim or outcome")]
    InvalidCommandState,
    #[error("embedded request frontier is unavailable or conflicts with the retained branch")]
    InvalidEmbeddedFrontier,
    #[error("embedded pending request has no durable admission provenance")]
    UnownedEmbeddedRequest,
    #[error("embedded conversation does not match its host binding")]
    InvalidEmbeddedBinding,
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("request not found: {0}")]
    MissingRequest(String),
    #[error("history offset exceeds the supported range")]
    InvalidHistoryOffset,
    #[error("request {request} belongs to agent {actual}, not {expected}")]
    RequestAgentMismatch {
        request: String,
        expected: String,
        actual: String,
    },
    #[error("claim already exists for call/request")]
    DuplicateClaim,
    #[error("operation origin does not match its issuing request and Store binding")]
    OperationOriginMismatch,
    #[error("invalid canonical agent path: {0}")]
    InvalidAgentPath(String),
    #[error("agent parent does not exist: {0}")]
    MissingAgentParent(String),
    #[error("active spawn call {call_id} is not persisted in request {request}")]
    MissingActiveSpawnCall { request: String, call_id: String },
    #[error("session state key uses the reserved harness namespace: {0}")]
    ReservedSessionStateNamespace(String),
    #[error("malformed durable tool call `{call_id}` in request {request}: {reason}")]
    MalformedReplayCall {
        call_id: String,
        request: String,
        reason: String,
    },
    #[error("durable output for call `{call_id}` does not match its {expected:?} call kind")]
    ReplayOutputKindMismatch { call_id: String, expected: ToolKind },
    #[error("ambiguous durable invocation for call `{call_id}`")]
    AmbiguousReplayCall { call_id: String },
    #[error("checkpoint boundary call {call_id} is not in request {request}")]
    MissingCheckpointCall { request: String, call_id: String },
    #[error("checkpoint boundary call {call_id} has no durable claim in request {request}")]
    MissingCheckpointClaim { request: String, call_id: String },
    #[error("checkpoint boundary operation is no longer pending: {0:?}")]
    CheckpointBoundaryNotPending(OperationId),
    #[error("checkpoint metadata has unsupported version or missing cut provenance")]
    InvalidCheckpointMetadata,
    #[error("checkpoint request {0} has no recorded effort setting")]
    MissingCheckpointEffort(String),
    #[error("checkpoint belongs to a different Store process")]
    ForeignCheckpoint,
    #[error(
        "schema migration cannot determine origin of legacy claim {call_id} on request {request}: {reason}; preserve this database and repair its provenance before reopening"
    )]
    LegacyProvenance {
        request: String,
        call_id: String,
        reason: String,
    },
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod recovery_tests;
pub type Result<T> = std::result::Result<T, StoreError>;

fn invocation_kind(c: &Connection, request: &RequestId, call: &CallId) -> Result<Option<ToolKind>> {
    Ok(validation::invocation_item(c, request, call)?.map(|item| item.input.kind()))
}

fn validate_replay_output(call: &CallId, kind: ToolKind, output: &Item) -> Result<()> {
    let expected = match kind {
        ToolKind::Function => "function_call_output",
        ToolKind::Custom => "custom_tool_call_output",
    };
    if output.0["type"] != expected || output.0["call_id"] != call.0 {
        return Err(StoreError::ReplayOutputKindMismatch {
            call_id: call.0.clone(),
            expected: kind,
        });
    }
    Ok(())
}

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
    pub operation: OperationId,
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
pub(crate) fn utc_millis() -> i64 {
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
    pub operation: OperationId,
    pub state: ClaimState,
    pub output: Option<ItemHash>,
}
fn decode_claim(r: &rusqlite::Row<'_>) -> rusqlite::Result<Claim> {
    let raw: String = r.get(0)?;
    let origin = serde_json::from_str(&raw).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let request = RequestId(r.get(3)?);
    let call_id = CallId(r.get(2)?);
    let state: String = r.get(4)?;
    Ok(Claim {
        operation: OperationId {
            origin,
            request: RequestId(r.get(1)?),
            call: call_id.clone(),
        },
        call_id,
        request,
        state: match state.as_str() {
            "settled" => ClaimState::Settled,
            "interrupted" => ClaimState::Interrupted,
            _ => ClaimState::Pending,
        },
        output: r.get::<_, Option<String>>(5)?.map(ItemHash),
    })
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
    pub(crate) conn: Mutex<Connection>,
    pub(crate) process_identity: Arc<()>,
    store_id: String,
}
impl Store {
    pub(crate) fn validate_agent_path(path: &str, parent: Option<&str>) -> Result<()> {
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
        let history_pairs: Vec<(RequestId, Item)> =
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
                stored_history.into_iter().take(end + 1).collect()
            } else {
                stored_history
            };
        let inherited_operations = if let Some(head) = source_head.as_ref() {
            let mut q = tx.prepare(
                "WITH RECURSIVE lineage(id,parent_id) AS (
                     SELECT id,parent_id FROM requests WHERE id=?1
                     UNION ALL
                     SELECT r.id,r.parent_id FROM requests r JOIN lineage ON r.id=lineage.parent_id
                 )
                 SELECT c.origin,c.origin_request_id,c.call_id,c.request_id FROM claims c
                 JOIN lineage ON lineage.id=c.request_id
                 WHERE c.state='pending' ORDER BY c.request_id,c.call_id",
            )?;
            let rows = q
                .query_map([&head.0], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut same_request_calls =
                std::collections::HashMap::<(String, String), usize>::new();
            for (_, _, call, claim_request) in &rows {
                *same_request_calls
                    .entry((claim_request.clone(), call.clone()))
                    .or_default() += 1;
            }
            for ((claim_request, call), count) in same_request_calls {
                if count <= 1 {
                    continue;
                }
                let visible = history_pairs
                    .iter()
                    .filter(|(request, item)| {
                        request.0 == claim_request
                            && !Self::strip_from_here_snapshot(item)
                            && item
                                .tool_call()
                                .ok()
                                .flatten()
                                .is_some_and(|tool| tool.call_id.0 == call)
                    })
                    .count();
                if visible > 0 && visible < count {
                    return Err(StoreError::AmbiguousReplayCall { call_id: call });
                }
            }
            let mut operations = HashSet::new();
            for (origin, original_request, call, claim_request) in rows {
                if history_pairs.iter().any(|(request, item)| {
                    request.0 == claim_request
                        && !Self::strip_from_here_snapshot(item)
                        && item
                            .tool_call()
                            .ok()
                            .flatten()
                            .is_some_and(|tool| tool.call_id.0 == call)
                }) {
                    operations.insert((origin, original_request, call));
                }
            }
            operations.into_iter().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let history: Vec<Item> = history_pairs.into_iter().map(|(_, item)| item).collect();
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
        if let Some(source_head) = &source_head {
            context::preserve_origins(&tx, source_head, snapshot_request)?;
        }
        for (origin, original_request, call_id) in inherited_operations {
            tx.execute(
                "INSERT INTO claims(origin,origin_request_id,call_id,request_id,state) VALUES (?1,?2,?3,?4,'pending')",
                params![origin, original_request, call_id, snapshot_request.0],
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
    /// Atomically persist a successful agent completion and its optional
    /// parent answer. A lost head CAS cannot publish, and publication failure
    /// rolls the head update back with the transaction.
    pub fn complete_agent_with_publication(
        &self,
        path: &AgentPath,
        expected: Option<&RequestId>,
        current: &RequestId,
        parent_answer: Option<(&AgentPath, &Item)>,
    ) -> Result<CompletionCommit> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let changed = tx.execute(
            "UPDATE agents SET head_request=?3 WHERE path=?1 AND head_request IS ?2",
            params![path.0, expected.map(|x| x.0.as_str()), current.0],
        )?;
        if changed != 1 {
            return Ok(CompletionCommit::HeadMismatch);
        }
        let envelope_id = if let Some((parent, answer)) = parent_answer {
            let hash = Self::put_item_tx(&tx, answer)?;
            tx.execute(
                "INSERT INTO envelopes(sender,recipient,class,item_hash,delivered_request,created_at) \
                 VALUES (?1,?2,'AtBoundary',?3,NULL,?4)",
                params![path.0, parent.0, hash.0, utc_millis()],
            )?;
            Some(tx.last_insert_rowid())
        } else {
            None
        };
        tx.commit()?;
        Ok(CompletionCommit::Committed { envelope_id })
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
        let store_id: String = conn.query_row(
            "SELECT state FROM session_state WHERE session_id='harness:store-id'",
            [],
            |row| row.get(0),
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            process_identity: Arc::new(()),
            store_id,
        })
    }
    pub fn standalone_identity(&self, actor: AgentPath) -> ConversationIdentity {
        ConversationIdentity::Standalone {
            store: self.store_id.clone(),
            actor,
        }
    }
    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
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
    #[cfg(test)]
    pub(crate) fn write_compaction_request(
        &self,
        request: &RequestId,
        parent: &RequestId,
        branch: &str,
        items: &[Item],
    ) -> Result<()> {
        self.write_compaction_request_with_claims(request, parent, branch, items, &[], None)
    }
    pub(crate) fn write_compaction_request_with_claims(
        &self,
        request: &RequestId,
        parent: &RequestId,
        branch: &str,
        items: &[Item],
        pending: &[OperationId],
        identity: Option<&crate::embedding::HostIdentity>,
    ) -> Result<()> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        if let Some(identity) = identity {
            let current = embedded_round::frontier(&tx, identity)?;
            if branch != identity.actor.0
                || current
                    .pending_head
                    .as_ref()
                    .or(current.settled_head.as_ref())
                    != Some(parent)
            {
                return Err(StoreError::InvalidEmbeddedFrontier);
            }
        }
        tx.execute(
            "INSERT INTO requests(id,parent_id,branch,created_at) VALUES (?1,?2,?3,?4)",
            params![request.0, parent.0, branch, utc_millis()],
        )?;
        if let Some(identity) = identity {
            tx.execute("UPDATE requests SET embedded_run=?2,embedded_incarnation=?3,round_phase='pending' WHERE id=?1",
                params![request.0,identity.run,identity.incarnation])?;
        }
        for (position, item) in items.iter().enumerate() {
            let hash = Self::put_item_tx(&tx, item)?;
            tx.execute(
                "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                params![request.0, position as i64, hash.0],
            )?;
        }
        let mut seen = HashSet::new();
        for operation in pending {
            if !seen.insert(operation) {
                return Err(StoreError::DuplicateClaim);
            }
            let matching = items
                .iter()
                .filter_map(|item| item.tool_call().ok().flatten())
                .filter(|call| call.call_id == operation.call)
                .count();
            let required = pending
                .iter()
                .filter(|other| other.call == operation.call)
                .count();
            if matching < required {
                return Err(StoreError::AmbiguousReplayCall {
                    call_id: operation.call.0.clone(),
                });
            }
            let origin = serde_json::to_string(&operation.origin)?;
            tx.execute(
                "INSERT INTO claims(origin,origin_request_id,call_id,request_id,state) VALUES (?1,?2,?3,?4,'pending')",
                params![origin,operation.request.0,operation.call.0,request.0],
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
        context::compaction_generation(&tx, &self.store_id, parent, request, branch)?;
        tx.commit()?;
        Ok(())
    }
    pub(crate) fn is_compaction_boundary(&self, request: &RequestId) -> Result<bool> {
        let key = format!("harness:compaction:{}", request.0);
        Ok(self.session_state(&key)?.is_some())
    }
    /// Store an explicitly classified output and settle all claimants atomically.
    pub fn write_output(
        &self,
        operation: &OperationId,
        output: &Item,
        terminal: TerminalOutcome,
    ) -> Result<usize> {
        self.settle_claims(operation, output, terminal)
    }

    pub(crate) fn write_job_output(
        &self,
        operation: &OperationId,
        kind: ToolKind,
        output: &crate::turn::JobOutput,
    ) -> Result<usize> {
        self.settle_claims(
            operation,
            &Item::tool_output(&operation.call, kind, output),
            TerminalOutcome::from(output),
        )
    }
    /// Crash recovery exposes all still-pending durable claims for the caller's resumption policy.
    pub fn recover_pending(&self) -> Result<Vec<PendingCall>> {
        let c = self.lock();
        let mut q=c.prepare("SELECT origin,origin_request_id,call_id,request_id FROM claims WHERE state='pending' ORDER BY call_id,request_id")?;
        q.query_map([], |r| {
            let raw: String = r.get(0)?;
            let origin = serde_json::from_str(&raw).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let call_id = CallId(r.get(2)?);
            Ok(PendingCall {
                operation: OperationId {
                    origin,
                    request: RequestId(r.get(1)?),
                    call: call_id.clone(),
                },
                call_id,
                request: RequestId(r.get(3)?),
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
        let mut q=c.prepare("WITH RECURSIVE lineage(id,parent_id) AS (SELECT id,parent_id FROM requests WHERE id=?1 UNION ALL SELECT r.id,r.parent_id FROM requests r JOIN lineage l ON r.id=l.parent_id) SELECT c.origin,c.origin_request_id,c.call_id,c.request_id FROM claims c JOIN lineage l ON l.id=c.request_id WHERE c.state='pending' ORDER BY c.call_id,c.request_id")?;
        q.query_map([&request.0], |r| {
            let raw: String = r.get(0)?;
            let origin = serde_json::from_str(&raw).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            let call_id = CallId(r.get(2)?);
            Ok(PendingCall {
                operation: OperationId {
                    origin,
                    request: RequestId(r.get(1)?),
                    call: call_id.clone(),
                },
                call_id,
                request: RequestId(r.get(3)?),
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
    pub(crate) fn put_item_tx(tx: &Transaction<'_>, item: &Item) -> Result<ItemHash> {
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
    pub(crate) fn set_effort_tx(
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
    pub(crate) fn items_with_hashes(&self, request: &RequestId) -> Result<Vec<(ItemHash, Item)>> {
        let c = self.lock();
        let mut q = c.prepare("SELECT ri.item_hash,i.json FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id=?1 ORDER BY ri.position")?;
        q.query_map([&request.0], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .map(|row| {
            let (hash, json) = row?;
            Ok((ItemHash(hash), serde_json::from_str(&json)?))
        })
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
        let issued = self.seal_replay_request(model_request)?;
        self.record_issued_replay_turn(request, issued, model_response)
    }

    /// Completed model turns on this request's branch, from `root` forward.
    /// Fork branches are excluded even when they inherit `root` as an ancestor.
    pub fn replay_turns(&self, root: &RequestId) -> Result<Vec<RecordedReplayTurn>> {
        let records = {
            let c = self.lock();
            let mut q = c.prepare(
                "WITH RECURSIVE chain(id, branch) AS (
                SELECT id, branch FROM requests WHERE id=?1
                UNION ALL
                SELECT child.id, child.branch FROM requests child
                JOIN chain parent ON child.parent_id=parent.id AND child.branch=parent.branch
            )
            SELECT e.id,e.payload FROM events e
            JOIN chain ON chain.id=e.request_id
            WHERE e.kind='model_turn' ORDER BY e.id",
            )?;
            q.query_map([&root.0], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        records
            .into_iter()
            .map(|(event, payload)| self.decode_replay_record(event, &payload))
            .collect()
    }

    /// The durable settled output for one recorded provider call.
    pub fn replay_output_operation(&self, operation: &OperationId) -> Result<Option<Item>> {
        let kind = self.tool_invocation_kind(&operation.request, &operation.call)?;
        let Some(kind) = kind else {
            return Ok(None);
        };
        let origin = serde_json::to_string(&operation.origin)?;
        let c = self.lock();
        let raw: Option<String> = c.query_row(
            "SELECT i.json FROM claims c JOIN items i ON i.hash=c.output_hash WHERE c.origin=?1 AND c.origin_request_id=?2 AND c.call_id=?3 AND c.state='settled' LIMIT 1",
            params![origin,operation.request.0,operation.call.0], |row| row.get(0),
        ).optional()?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let output: Item = serde_json::from_str(&raw)?;
        validate_replay_output(&operation.call, kind, &output)?;
        Ok(Some(output))
    }
    /// Diagnostic lookup; refuses collisions instead of choosing an operation.
    pub fn replay_output(&self, call: &CallId) -> Result<Option<Item>> {
        let c = self.lock();
        // Claims can be inherited by Here/agent requests. A claim alone is
        // therefore not replay evidence: recognize a call only in the exact
        // request that owns a settled claim, and never walk request ancestry.
        let mut claims = c.prepare(
            "SELECT c.request_id,c.output_hash FROM claims c \
             WHERE c.call_id=?1 AND c.state='settled' AND c.output_hash IS NOT NULL \
             ORDER BY c.request_id",
        )?;
        let claim_rows = claims
            .query_map([&call.0], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(claims);

        let mut matched = None;
        for (request, output_hash) in claim_rows {
            let Some(kind) = invocation_kind(&c, &RequestId(request), call)? else {
                continue;
            };
            if matched.is_some() {
                return Err(StoreError::AmbiguousReplayCall {
                    call_id: call.0.clone(),
                });
            }
            let raw: String = c.query_row(
                "SELECT json FROM items WHERE hash=?1",
                [&output_hash],
                |row| row.get(0),
            )?;
            let output: Item = serde_json::from_str(&raw)?;
            let expected_type = match kind {
                ToolKind::Function => "function_call_output",
                ToolKind::Custom => "custom_tool_call_output",
            };
            if output.0.get("type").and_then(serde_json::Value::as_str) != Some(expected_type)
                || output.0.get("call_id").and_then(serde_json::Value::as_str)
                    != Some(call.0.as_str())
            {
                return Err(StoreError::ReplayOutputKindMismatch {
                    call_id: call.0.clone(),
                    expected: kind,
                });
            }
            matched = Some(output);
        }
        Ok(matched)
    }

    /// Resolve output only from the request that persisted this invocation.
    pub fn replay_output_for_request(
        &self,
        request: &RequestId,
        call: &CallId,
    ) -> Result<Option<Item>> {
        let c = self.lock();
        let Some(kind) = invocation_kind(&c, request, call)? else {
            return Ok(None);
        };
        let claim_count: i64 = c.query_row(
            "SELECT COUNT(*) FROM claims WHERE request_id=?1 AND call_id=?2",
            params![request.0, call.0],
            |row| row.get(0),
        )?;
        if claim_count > 1 {
            return Err(StoreError::AmbiguousReplayCall {
                call_id: call.0.clone(),
            });
        }
        let raw: Option<String> = c
            .query_row(
                "SELECT i.json FROM claims c JOIN items i ON i.hash=c.output_hash
             WHERE c.request_id=?1 AND c.call_id=?2 AND c.state='settled'",
                params![request.0, call.0],
                |row| row.get(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let output: Item = serde_json::from_str(&raw)?;
        let expected_type = match kind {
            ToolKind::Function => "function_call_output",
            ToolKind::Custom => "custom_tool_call_output",
        };
        if output.0.get("type").and_then(serde_json::Value::as_str) != Some(expected_type)
            || output.0.get("call_id").and_then(serde_json::Value::as_str) != Some(call.0.as_str())
        {
            return Err(StoreError::ReplayOutputKindMismatch {
                call_id: call.0.clone(),
                expected: kind,
            });
        }
        Ok(Some(output))
    }

    /// Kind of the invocation persisted in this exact request. Call IDs alone
    /// cannot identify a tool across agent or Here branches.
    pub fn tool_invocation_kind(
        &self,
        request: &RequestId,
        call: &CallId,
    ) -> Result<Option<ToolKind>> {
        let c = self.lock();
        invocation_kind(&c, request, call)
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
    /// Resolve a stable envelope reference, including its persisted delivery
    /// request. The reference is the SQLite envelope ID returned at admission.
    pub fn envelope(&self, id: i64) -> Result<Option<Envelope>> {
        let c = self.lock();
        c.query_row(
            "SELECT id,sender,recipient,class,item_hash,delivered_request,created_at \
             FROM envelopes WHERE id=?1",
            [id],
            |r| {
                Ok(Envelope {
                    id: r.get(0)?,
                    sender: r.get(1)?,
                    recipient: r.get(2)?,
                    class: r.get(3)?,
                    item_hash: ItemHash(r.get(4)?),
                    delivered_request: r.get::<_, Option<String>>(5)?.map(RequestId),
                    created_at: r.get(6)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }
    /// Snapshot final input provenance under one serialized connection read.
    /// "Seen" records presentation on the final request's ancestry only, not
    /// acknowledgement or incorporation by the model.
    pub fn completion_provenance(
        &self,
        recipient: &AgentPath,
        final_request: &RequestId,
    ) -> Result<CompletionProvenance> {
        let c = self.lock();
        let branch: Option<String> = c
            .query_row(
                "SELECT branch FROM requests WHERE id=?1",
                [&final_request.0],
                |r| r.get(0),
            )
            .optional()?;
        let Some(branch) = branch else {
            return Err(StoreError::MissingRequest(final_request.0.clone()));
        };
        if branch != recipient.0 {
            return Err(StoreError::RequestAgentMismatch {
                request: final_request.0.clone(),
                expected: recipient.0.clone(),
                actual: branch,
            });
        }
        let mut stmt = c.prepare(
            "WITH RECURSIVE ancestry(id) AS (
                SELECT ?2
                UNION ALL
                SELECT r.parent_id FROM requests r JOIN ancestry a ON r.id=a.id
                WHERE r.parent_id IS NOT NULL
             )
             SELECT e.id, e.delivered_request IS NULL FROM envelopes e
             WHERE e.recipient=?1 AND
               (e.delivered_request IS NULL OR
                e.delivered_request IN (SELECT id FROM ancestry))
             ORDER BY e.id",
        )?;
        let mut seen_envelopes = Vec::new();
        let mut unseen_envelopes = Vec::new();
        for row in stmt.query_map(params![recipient.0, final_request.0], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?))
        })? {
            let (id, unread) = row?;
            if unread {
                unseen_envelopes.push(id);
            } else {
                seen_envelopes.push(id);
            }
        }
        Ok(CompletionProvenance {
            final_request: final_request.clone(),
            seen_envelopes,
            unseen_envelopes,
        })
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
    pub fn operation_for_request(&self, request: &RequestId, call: &CallId) -> Result<OperationId> {
        let c = self.lock();
        let branch: String = c
            .query_row(
                "SELECT branch FROM requests WHERE id=?1",
                [&request.0],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::MissingRequest(request.0.clone()))?;
        let bound: Option<(String, String)> = c
            .query_row(
                "SELECT run_id,incarnation FROM embedded_bindings WHERE agent_path=?1",
                [&branch],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let origin = match bound {
            Some((run, incarnation)) => ConversationIdentity::Embedded {
                run,
                actor: AgentPath(branch),
                incarnation,
            },
            None => self.standalone_identity(AgentPath(branch)),
        };
        Ok(OperationId {
            origin,
            request: request.clone(),
            call: call.clone(),
        })
    }
    /// Admit an original invocation at its exact request.
    pub fn claim(&self, call: &CallId, request: &RequestId) -> Result<OperationId> {
        let operation = self.operation_for_request(request, call)?;
        self.claim_operation(&operation, request)?;
        Ok(operation)
    }
    /// Admit the original claimant under the request's exact conversation binding.
    pub fn claim_operation(&self, operation: &OperationId, request: &RequestId) -> Result<()> {
        if request != &operation.request
            || *operation != self.operation_for_request(request, &operation.call)?
        {
            return Err(StoreError::OperationOriginMismatch);
        }
        let origin = serde_json::to_string(&operation.origin)?;
        self.lock().execute(
            "INSERT INTO claims(origin,origin_request_id,call_id,request_id,state) VALUES (?1,?2,?3,?4,'pending')",
            params![origin,operation.request.0,operation.call.0,request.0],
        ).map(|_|()).map_err(|e| if matches!(e,rusqlite::Error::SqliteFailure(ref x,_) if x.extended_code==rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY) { StoreError::DuplicateClaim } else { e.into() })
    }
    pub fn claims_for_operation(&self, operation: &OperationId) -> Result<Vec<Claim>> {
        let c = self.lock();
        let mut q = c.prepare("SELECT origin,origin_request_id,call_id,request_id,state,output_hash FROM claims WHERE origin=?1 AND origin_request_id=?2 AND call_id=?3 ORDER BY request_id")?;
        let origin = serde_json::to_string(&operation.origin)?;
        Ok(q.query_map(
            params![origin, operation.request.0, operation.call.0],
            decode_claim,
        )?
        .collect::<std::result::Result<_, _>>()?)
    }
    /// Diagnostic enumeration only. A provider ID alone never selects an operation.
    pub fn claims(&self, call: &CallId) -> Result<Vec<Claim>> {
        let c = self.lock();
        let mut q=c.prepare("SELECT origin,origin_request_id,call_id,request_id,state,output_hash FROM claims WHERE call_id=?1 ORDER BY request_id")?;
        q.query_map([&call.0], decode_claim)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
    /// Claims directly attached to one request, without following request
    /// ancestry. Here-fork startup uses this to distinguish a child-owned
    /// inherited claim from a pending ancestor claim visible through lineage.
    pub fn claims_on(&self, request: &RequestId) -> Result<Vec<Claim>> {
        let c = self.lock();
        let mut q = c.prepare(
            "SELECT origin,origin_request_id,call_id,request_id,state,output_hash FROM claims \
             WHERE request_id=?1 ORDER BY call_id",
        )?;
        q.query_map([&request.0], decode_claim)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
    /// Claims on the contiguous same-branch ancestry of `request`.
    ///
    /// This is intentionally distinct from `claims_on`: ordinary Engine runs
    /// (including Here-fork starts) inspect only claims attached to their
    /// supplied head. Process-restart recovery may opt into this query to find
    /// claims on older requests of the same agent branch, without inheriting
    /// claims across a fork boundary. A compacted window owns its copied
    /// pending claims; completed calls discarded by that cut are not inherited.
    pub fn claims_on_branch_lineage(
        &self,
        request: &RequestId,
        branch: &str,
    ) -> Result<Vec<Claim>> {
        let c = self.lock();
        let mut q = c.prepare(
            "WITH RECURSIVE lineage(id,parent_id,branch) AS (
                 SELECT id,parent_id,branch FROM requests WHERE id=?1 AND branch=?2
                 UNION ALL
                 SELECT parent.id,parent.parent_id,parent.branch
                 FROM requests parent JOIN lineage child ON parent.id=child.parent_id
                 WHERE parent.branch=?2 AND NOT EXISTS(
                     SELECT 1 FROM session_state s
                     WHERE s.session_id='harness:compaction:' || child.id
                 )
             )
             SELECT c.origin,c.origin_request_id,c.call_id,c.request_id,c.state,c.output_hash
             FROM claims c JOIN lineage l ON l.id=c.request_id
             ORDER BY c.call_id",
        )?;
        q.query_map(params![request.0, branch], decode_claim)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
    pub fn settle_claims(
        &self,
        operation: &OperationId,
        output: &Item,
        terminal: TerminalOutcome,
    ) -> Result<usize> {
        let kind = self
            .tool_invocation_kind(&operation.request, &operation.call)?
            .ok_or_else(|| StoreError::MissingCheckpointCall {
                request: operation.request.0.clone(),
                call_id: operation.call.0.clone(),
            })?;
        validate_replay_output(&operation.call, kind, &output)?;
        let mut c = self.lock();
        let tx = c.transaction()?;
        let h = Self::put_item_tx(&tx, output)?;
        if let Some((previous_hash, previous_terminal)) = terminal::exact_terminal(&tx, operation)?
        {
            if previous_hash != h || previous_terminal != terminal {
                return Err(StoreError::ConflictingReplayOutcome {
                    operation: operation.clone(),
                });
            }
        }
        let origin = serde_json::to_string(&operation.origin)?;
        let n = tx.execute(
            "UPDATE claims SET state='settled',output_hash=?4,terminal_json=?5 WHERE origin=?1 AND origin_request_id=?2 AND call_id=?3 AND state='pending'",
            params![origin,operation.request.0,operation.call.0,h.0,serde_json::to_string(&terminal)?],
        )?;
        tx.commit()?;
        Ok(n)
    }
    pub fn interrupt_operation_claim(
        &self,
        operation: &OperationId,
        request: &RequestId,
    ) -> Result<usize> {
        let origin = serde_json::to_string(&operation.origin)?;
        Ok(self.lock().execute("UPDATE claims SET state='interrupted' WHERE origin=?1 AND origin_request_id=?2 AND call_id=?3 AND request_id=?4 AND state='pending'",params![origin,operation.request.0,operation.call.0,request.0])?)
    }
    /// Request-scoped legacy caller. Ambiguous same-ID claims are refused.
    pub fn interrupt_claim(&self, call: &CallId, request: &RequestId) -> Result<usize> {
        let candidates: Vec<_> = self
            .claims_on(request)?
            .into_iter()
            .filter(|c| c.call_id == *call)
            .collect();
        if candidates.len() > 1 {
            return Err(StoreError::AmbiguousReplayCall {
                call_id: call.0.clone(),
            });
        }
        match candidates.first() {
            Some(claim) => self.interrupt_operation_claim(&claim.operation, request),
            None => Ok(0),
        }
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
            tools: vec![serde_json::json!({"type":"function","name":"cell"})].into(),
            tools_allowed: None,
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
            s.write_output(
                &s.claims(&call).unwrap()[0].operation,
                &output,
                crate::store::TerminalOutcome::Success,
            )
            .unwrap();
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
    fn custom_replay_preserves_wire_output_and_rejects_kind_mismatch_after_reopen() {
        let path =
            std::env::temp_dir().join(format!("harness-custom-replay-{}.db", uuid::Uuid::new_v4()));
        let request = id("custom-replay-request");
        let custom_call = CallId("custom-replay-call".into());
        let mismatch_call = CallId("mismatch-replay-call".into());
        let call_items = [
            item(serde_json::json!({
                "type":"custom_tool_call","call_id":custom_call.0,"name":"cell",
                "input":"line one\nquotes: \" \\\\ snowman: ☃"
            })),
            item(serde_json::json!({
                "type":"custom_tool_call","call_id":mismatch_call.0,"name":"cell",
                "input":"raw custom input"
            })),
        ];
        let custom_output = item(serde_json::json!({
            "type":"custom_tool_call_output","call_id":custom_call.0,
            "output":"original raw result\nwith \"quotes\" and \\\\ and ☃"
        }));
        let wrong_output = item(serde_json::json!({
            "type":"function_call_output","call_id":mismatch_call.0,"output":"{}"
        }));
        {
            let store = Store::open(&path).unwrap();
            store
                .write_request(&request, None, "/root", &call_items, Usage::default())
                .unwrap();
            store.claim(&custom_call, &request).unwrap();
            store.claim(&mismatch_call, &request).unwrap();
            store
                .write_output(
                    &store.claims(&custom_call).unwrap()[0].operation,
                    &custom_output,
                    crate::store::TerminalOutcome::Success,
                )
                .unwrap();
            assert!(matches!(
                store.write_output(
                    &store.claims(&mismatch_call).unwrap()[0].operation,
                    &wrong_output,
                    crate::store::TerminalOutcome::Success
                ),
                Err(StoreError::ReplayOutputKindMismatch { .. })
            ));
        }
        {
            let store = Store::open(&path).unwrap();
            let retained_claim = store.claims(&custom_call).unwrap();
            assert_eq!(retained_claim.len(), 1);
            assert_eq!(retained_claim[0].request, request);
            assert_eq!(retained_claim[0].state, ClaimState::Settled);
            assert_eq!(
                store.replay_output(&custom_call).unwrap(),
                Some(custom_output),
                "custom output bytes must remain the original persisted item"
            );
            assert_eq!(store.replay_output(&mismatch_call).unwrap(), None);
            assert_eq!(
                store.claims(&mismatch_call).unwrap()[0].state,
                ClaimState::Pending
            );
        }
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let _ = std::fs::remove_file(path.with_extension("db-shm"));
    }

    #[test]
    fn replay_does_not_follow_a_claim_to_ancestor_call_evidence() {
        let store = Store::memory().unwrap();
        let root = id("lineage-root");
        let child = id("lineage-child");
        let call = CallId("lineage-call".into());
        let call_item = item(serde_json::json!({
            "type":"custom_tool_call","call_id":call.0,"name":"cell","input":"root evidence"
        }));
        let output = item(serde_json::json!({
            "type":"custom_tool_call_output","call_id":call.0,"output":"child settled output"
        }));
        store.create_request(&root, None, "/root").unwrap();
        store
            .create_request(&child, Some(&root), "/root/worker")
            .unwrap();
        store.append_items(&root, &[call_item]).unwrap();
        store.claim(&call, &child).unwrap();
        assert!(matches!(
            store.write_output(
                &store.claims(&call).unwrap()[0].operation,
                &output,
                crate::store::TerminalOutcome::Success
            ),
            Err(StoreError::MissingCheckpointCall { .. })
        ));

        assert_eq!(
            store.replay_output(&call).unwrap(),
            None,
            "a child claim cannot borrow its ancestor's tool-call evidence"
        );
    }

    #[test]
    fn replay_ignores_an_unrelated_malformed_tool_call_item() {
        let store = Store::memory().unwrap();
        let request = id("malformed-other-request");
        let call = CallId("valid-requested-call".into());
        let output = item(serde_json::json!({
            "type":"custom_tool_call_output","call_id":call.0,"output":"expected"
        }));
        store.create_request(&request, None, "/root").unwrap();
        store
            .append_items(
                &request,
                &[
                    item(serde_json::json!({
                        "type":"custom_tool_call","call_id":call.0,"name":"cell","input":"valid"
                    })),
                    item(serde_json::json!({
                        "type":"custom_tool_call","call_id":"other-call","input":"malformed"
                    })),
                ],
            )
            .unwrap();
        store.claim(&call, &request).unwrap();
        store
            .write_output(
                &store.claims(&call).unwrap()[0].operation,
                &output,
                crate::store::TerminalOutcome::Success,
            )
            .unwrap();

        assert_eq!(store.replay_output(&call).unwrap(), Some(output));
    }

    #[test]
    fn completion_provenance_uses_ancestry_and_reopen() {
        let path = std::env::temp_dir().join(format!(
            "harness-completion-provenance-{}.db",
            uuid::Uuid::new_v4()
        ));
        let recipient = AgentPath("/root/child".into());
        let first = id("first");
        let final_request = id("final");
        let message = item(serde_json::json!({"type":"message","content":"update"}));
        let (seen, unseen, late);
        {
            let store = Store::open(&path).unwrap();
            store.create_request(&first, None, &recipient.0).unwrap();
            seen = store
                .add_envelope("/root", &recipient.0, "AtBoundary", &message, None)
                .unwrap();
            store.append_unread_envelopes(&recipient, &first).unwrap();
            store
                .create_request(&final_request, Some(&first), &recipient.0)
                .unwrap();
            unseen = store
                .add_envelope("/root", &recipient.0, "AtBoundary", &message, None)
                .unwrap();
            let snapshot = store
                .completion_provenance(&recipient, &final_request)
                .unwrap();
            assert_eq!(snapshot.final_request, final_request);
            assert_eq!(snapshot.seen_envelopes, vec![seen]);
            assert_eq!(snapshot.unseen_envelopes, vec![unseen]);
            late = store
                .add_envelope("/root", &recipient.0, "AtBoundary", &message, None)
                .unwrap();
            assert_eq!(
                store.envelope(seen).unwrap().unwrap().delivered_request,
                Some(first.clone())
            );
        }
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(
                store
                    .completion_provenance(&recipient, &final_request)
                    .unwrap()
                    .seen_envelopes,
                vec![seen]
            );
            assert_eq!(
                store
                    .completion_provenance(&recipient, &final_request)
                    .unwrap()
                    .unseen_envelopes,
                vec![unseen, late]
            );
            let next = id("next");
            store
                .create_request(&next, Some(&final_request), &recipient.0)
                .unwrap();
            assert_eq!(
                store
                    .append_unread_envelopes(&recipient, &next)
                    .unwrap()
                    .len(),
                2
            );
            let snapshot = store.completion_provenance(&recipient, &next).unwrap();
            assert_eq!(snapshot.seen_envelopes, vec![seen, unseen, late]);
            assert!(snapshot.unseen_envelopes.is_empty());
            assert_eq!(
                store.envelope(unseen).unwrap().unwrap().delivered_request,
                Some(next)
            );
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
            s.append_items(&id("root"), &[item(serde_json::json!({"type":"function_call","call_id":"c1","name":"test","arguments":"{}"}))]).unwrap();
            s.claim(&cid, &id("root")).unwrap();
            let output = item(
                serde_json::json!({"type":"function_call_output","call_id":"c1","output":"ok"}),
            );
            assert_eq!(
                s.settle_claims(
                    &s.claims(&cid).unwrap()[0].operation,
                    &output,
                    crate::store::TerminalOutcome::Success
                )
                .unwrap(),
                1
            );
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
            s.append_items(&id("a"), &[item(serde_json::json!({"type":"function_call","call_id":"pending","name":"test","arguments":"{}"}))]).unwrap();
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
                s.write_output(&s.claims(&call).unwrap()[0].operation, &item(serde_json::json!({"type":"function_call_output","call_id":"pending","output":"{\"ok\":true}"})), crate::store::TerminalOutcome::Success)
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
    fn before_request_decision_roundtrips_after_reopen() {
        let path = std::env::temp_dir().join(format!(
            "harness-before-request-{}.db",
            uuid::Uuid::new_v4()
        ));
        let typed_send = crate::hooks::BeforeRequestDecision::Send;
        let expected = Decision {
            hook: "before-request".into(),
            event_refs: vec!["item-1".into()],
            decision: serde_json::to_value(typed_send).unwrap(),
            evidence: serde_json::json!({"consumer":"standalone-browser"}),
            latency_ms: Some(3),
        };
        assert_eq!(expected.decision, serde_json::json!("Send"));
        // Existing generic decision rows must remain readable alongside the
        // newly typed before-request decision.
        let legacy = Decision {
            hook: "tool-call-admission".into(),
            event_refs: vec!["legacy-item".into()],
            decision: serde_json::json!({"allow":true}),
            evidence: serde_json::json!({"basis":"historical"}),
            latency_ms: None,
        };
        {
            let store = Store::open(&path).unwrap();
            store
                .write_request(&id("request-1"), None, "/root", &[], Usage::default())
                .unwrap();
            store
                .record_decision(Some(&id("request-1")), &expected)
                .unwrap();
            store
                .record_decision(Some(&id("request-1")), &legacy)
                .unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            let rows = store.decisions(Some(&id("request-1"))).unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0].request, Some(id("request-1")));
            assert_eq!(rows[0].decision, expected);
            assert_eq!(rows[1].request, Some(id("request-1")));
            assert_eq!(rows[1].decision, legacy);
            let request = store.request(&id("request-1")).unwrap().unwrap();
            assert_eq!(request.branch, "/root");
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn restricted_before_request_decision_roundtrips_after_reopen() {
        let path = std::env::temp_dir().join(format!(
            "harness-before-request-restricted-{}.db",
            uuid::Uuid::new_v4()
        ));
        let expected_decision = crate::hooks::BeforeRequestDecision::SendRestricted {
            tools_allowed: vec!["sleep".into()],
        };
        let expected = Decision {
            hook: "before-request".into(),
            event_refs: vec!["echo-item".into()],
            decision: serde_json::to_value(&expected_decision).unwrap(),
            evidence: serde_json::json!({"consumer":"standalone-browser","selection":"echo-sleep"}),
            latency_ms: None,
        };
        {
            let store = Store::open(&path).unwrap();
            store
                .write_request(
                    &id("restricted-request"),
                    None,
                    "/root",
                    &[],
                    Usage::default(),
                )
                .unwrap();
            store
                .record_decision(Some(&id("restricted-request")), &expected)
                .unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            let rows = store.decisions(Some(&id("restricted-request"))).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].decision, expected);
            let typed: crate::hooks::BeforeRequestDecision =
                serde_json::from_value(rows[0].decision.decision.clone()).unwrap();
            assert_eq!(typed, expected_decision);
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn inject_before_request_decision_roundtrips_after_reopen() {
        let path = std::env::temp_dir().join(format!(
            "harness-before-request-inject-{}.db",
            uuid::Uuid::new_v4()
        ));
        let expected_decision = crate::hooks::BeforeRequestDecision::Inject {
            item: crate::item::Item(serde_json::json!({
                "type":"message","role":"user","content":"standalone injected context"
            })),
            tools_allowed: Some(vec!["sleep".into()]),
        };
        let expected = Decision {
            hook: "before-request".into(),
            event_refs: vec!["echo-item".into()],
            decision: serde_json::to_value(&expected_decision).unwrap(),
            evidence: serde_json::json!({
                "consumer":"standalone-browser","selection":"echo-inject-sleep"
            }),
            latency_ms: None,
        };
        {
            let store = Store::open(&path).unwrap();
            store
                .write_request(&id("inject-request"), None, "/root", &[], Usage::default())
                .unwrap();
            store
                .record_decision(Some(&id("inject-request")), &expected)
                .unwrap();
        }
        {
            let store = Store::open(&path).unwrap();
            let rows = store.decisions(Some(&id("inject-request"))).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].decision, expected);
            let typed: crate::hooks::BeforeRequestDecision =
                serde_json::from_value(rows[0].decision.decision.clone()).unwrap();
            assert_eq!(typed, expected_decision);
        }
        let _ = std::fs::remove_file(path);
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
        assert_eq!(version, VERSION);
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
        assert_eq!(version, VERSION);
    }

    #[test]
    fn standalone_operation_origin_survives_store_reopen() {
        let path = std::env::temp_dir().join(format!(
            "harness-operation-origin-{}.db",
            uuid::Uuid::new_v4()
        ));
        let request = id("stable-origin-request");
        let call = CallId("stable-wire-call".into());
        let original = {
            let store = Store::open(&path).unwrap();
            store.create_request(&request, None, "/root").unwrap();
            store.operation_for_request(&request, &call).unwrap()
        };
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.operation_for_request(&request, &call).unwrap(),
            original
        );
        let _ = std::fs::remove_file(path);
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
    fn here_boundary_refuses_partial_same_id_copied_claims() {
        let store = Store::memory().unwrap();
        let first = id("first-origin");
        let second = id("second-origin");
        let source = id("copied-source");
        let snapshot = id("partial-here-snapshot");
        let call = CallId("reused-wire-id".into());
        let tool = item(serde_json::json!({
            "type":"function_call", "call_id":call.0, "name":"slow", "arguments":"{}"
        }));
        store.create_request(&first, None, "/root").unwrap();
        store
            .append_items(&first, std::slice::from_ref(&tool))
            .unwrap();
        let first_op = store.claim(&call, &first).unwrap();
        store
            .create_request(&second, Some(&first), "/root")
            .unwrap();
        store
            .append_items(&second, std::slice::from_ref(&tool))
            .unwrap();
        let second_op = store.claim(&call, &second).unwrap();
        let spawn = item(serde_json::json!({
            "type":"function_call", "call_id":"spawn-boundary", "name":"spawn_agent", "arguments":"{}"
        }));
        store
            .write_compaction_request_with_claims(
                &source,
                &second,
                "/root",
                &[tool.clone(), spawn, tool],
                &[first_op, second_op],
                None,
            )
            .unwrap();
        let root = AgentPath("/root".into());
        store
            .admit_agent(
                &root,
                None,
                Some(&source),
                &serde_json::json!({}),
                &serde_json::json!({}),
            )
            .unwrap();
        let child = AgentPath("/root/child".into());
        assert!(matches!(
            store.admit_here_agent_from_invocation(
                &child,
                &root,
                &snapshot,
                &source,
                &CallId("spawn-boundary".into()),
                &serde_json::json!({}),
                "/root",
                &child.0,
                "AtBoundary",
                &item(
                    serde_json::json!({"type":"message","role":"assistant","content":"NEW_TASK"})
                ),
            ),
            Err(StoreError::AmbiguousReplayCall { .. })
        ));
        assert!(store.request(&snapshot).unwrap().is_none());
        assert!(store.claims_on(&snapshot).unwrap().is_empty());
    }

    #[test]
    fn complete_agent_with_publication_is_atomic_and_idempotent() {
        let store = Store::memory().unwrap();
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        store
            .admit_agent(
                &root,
                None,
                None,
                &serde_json::json!({}),
                &serde_json::json!({}),
            )
            .unwrap();
        store
            .admit_agent(
                &child,
                Some(&root),
                None,
                &serde_json::json!({}),
                &serde_json::json!({}),
            )
            .unwrap();
        let first = id("first-completed");
        let second = id("second-completed");
        store.create_request(&first, None, &child.0).unwrap();
        store
            .create_request(&second, Some(&first), &child.0)
            .unwrap();
        let answer =
            item(serde_json::json!({"type":"message","role":"assistant","content":"typed"}));
        let outcome = store
            .complete_agent_with_publication(&child, None, &first, Some((&root, &answer)))
            .unwrap();
        let envelope_id = match outcome {
            CompletionCommit::Committed {
                envelope_id: Some(id),
            } => id,
            other => panic!("expected published completion, got {other:?}"),
        };
        assert_eq!(
            store.agent(&child).unwrap().unwrap().head_request,
            Some(first.clone())
        );
        let envelope = store.envelope(envelope_id).unwrap().unwrap();
        assert_eq!(envelope.sender, child.0);
        assert_eq!(envelope.recipient, root.0);
        assert_eq!(envelope.delivered_request, None);
        assert_eq!(
            store.get_item(&envelope.item_hash).unwrap(),
            Some(answer.clone())
        );
        let items_before_retry: i64 = store
            .lock()
            .query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap();
        let rejected_answer =
            item(serde_json::json!({"type":"message","content":"must-not-publish"}));
        assert_eq!(
            store
                .complete_agent_with_publication(
                    &child,
                    None,
                    &first,
                    Some((&root, &rejected_answer))
                )
                .unwrap(),
            CompletionCommit::HeadMismatch
        );
        assert_eq!(store.inbox(&root.0).unwrap().len(), 1);
        assert_eq!(
            store
                .lock()
                .query_row("SELECT COUNT(*) FROM items", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            items_before_retry
        );
        assert_eq!(
            store
                .complete_agent_with_publication(&child, Some(&first), &second, None)
                .unwrap(),
            CompletionCommit::Committed { envelope_id: None }
        );
        assert_eq!(
            store.agent(&child).unwrap().unwrap().head_request,
            Some(second)
        );
        assert_eq!(store.inbox(&root.0).unwrap().len(), 1);
    }

    #[test]
    fn complete_agent_with_publication_rolls_back_head_on_envelope_failure() {
        let store = Store::memory().unwrap();
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        store
            .admit_agent(
                &root,
                None,
                None,
                &serde_json::json!({}),
                &serde_json::json!({}),
            )
            .unwrap();
        store
            .admit_agent(
                &child,
                Some(&root),
                None,
                &serde_json::json!({}),
                &serde_json::json!({}),
            )
            .unwrap();
        let current = id("final");
        store.create_request(&current, None, &child.0).unwrap();
        store
            .lock()
            .execute_batch(
                "CREATE TRIGGER reject_completion_answer BEFORE INSERT ON envelopes \
                 BEGIN SELECT RAISE(ABORT, 'injected envelope failure'); END;",
            )
            .unwrap();
        let answer = item(serde_json::json!({"type":"message","content":"answer"}));
        assert!(
            store
                .complete_agent_with_publication(&child, None, &current, Some((&root, &answer)))
                .is_err()
        );
        assert_eq!(store.agent(&child).unwrap().unwrap().head_request, None);
        assert!(store.inbox(&root.0).unwrap().is_empty());
        assert_eq!(
            store
                .lock()
                .query_row("SELECT COUNT(*) FROM items", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_completion_answer")
            .unwrap();
        assert!(matches!(
            store
                .complete_agent_with_publication(&child, None, &current, Some((&root, &answer)))
                .unwrap(),
            CompletionCommit::Committed {
                envelope_id: Some(_)
            }
        ));
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
