//! Reusable, process-local host checkpoints of a durable conversation boundary.

use crate::{
    item::{Item, ToolKind},
    model::{AgentPath, CallId, ConversationIdentity, OperationId, RequestId},
    store::{Agent, AgentState, Result, Store, StoreError, utc_millis},
};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};

/// Original identity of a call still pending when the checkpoint was captured.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CheckpointClaim {
    pub operation: OperationId,
    pub request: RequestId,
    pub kind: ToolKind,
}

/// A captured host capability. The attachment is retained independently of
/// the issuing call, and cloning the checkpoint retains it for another child.
/// Pending calls retain their original execution owner: each child receives
/// the same terminal outcome, including failure or cancellation.
#[derive(Debug)]
pub struct Checkpoint<T: ?Sized> {
    id: String,
    origin: AgentPath,
    source_request: RequestId,
    snapshot_request: RequestId,
    boundary_call: CallId,
    metadata: Value,
    pending_claims: Vec<CheckpointClaim>,
    attachment: Arc<T>,
    process_identity: Arc<()>,
}

impl<T: ?Sized> Clone for Checkpoint<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            origin: self.origin.clone(),
            source_request: self.source_request.clone(),
            snapshot_request: self.snapshot_request.clone(),
            boundary_call: self.boundary_call.clone(),
            metadata: self.metadata.clone(),
            pending_claims: self.pending_claims.clone(),
            attachment: self.attachment.clone(),
            process_identity: self.process_identity.clone(),
        }
    }
}

impl<T: ?Sized> Checkpoint<T> {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn origin(&self) -> &AgentPath {
        &self.origin
    }
    pub fn source_request(&self) -> &RequestId {
        &self.source_request
    }
    pub fn snapshot_request(&self) -> &RequestId {
        &self.snapshot_request
    }
    pub fn boundary_call(&self) -> &CallId {
        &self.boundary_call
    }
    pub fn metadata(&self) -> &Value {
        &self.metadata
    }
    pub fn pending_claims(&self) -> &[CheckpointClaim] {
        &self.pending_claims
    }
    pub fn attachment(&self) -> &Arc<T> {
        &self.attachment
    }
}

/// Optional first task is committed with the child's conversation registration.
pub struct CheckpointTask<'a> {
    pub sender: &'a str,
    pub class: &'a str,
    pub item: &'a Item,
}

/// Host-selected child identity and checkout. The checkout is recorded per
/// child; it is deliberately absent from the reusable checkpoint itself.
pub struct CheckpointChild<'a> {
    pub path: &'a AgentPath,
    pub parent: &'a AgentPath,
    pub contract: &'a Value,
    pub checkout: &'a Value,
    pub task: Option<CheckpointTask<'a>>,
}

impl Store {
    /// Freeze the prefix ending at an actual call item in `source_request`.
    /// The caller's Arc is provisional until this transaction commits; a
    /// failed capture drops it without leaving checkpoint metadata behind.
    pub fn capture_checkpoint<T: ?Sized + Send + Sync + 'static>(
        &self,
        origin: &AgentPath,
        source_request: &RequestId,
        boundary_call: &CallId,
        metadata: &Value,
        attachment: Arc<T>,
    ) -> Result<Checkpoint<T>> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let source_branch: Option<String> = tx
            .query_row(
                "SELECT branch FROM requests WHERE id=?1",
                [&source_request.0],
                |row| row.get(0),
            )
            .optional()?;
        let Some(source_branch) = source_branch else {
            return Err(StoreError::MissingRequest(source_request.0.clone()));
        };
        if source_branch != origin.0 {
            return Err(StoreError::RequestAgentMismatch {
                request: source_request.0.clone(),
                expected: origin.0.clone(),
                actual: source_branch,
            });
        }
        let origin_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE path=?1)",
            [&origin.0],
            |row| row.get(0),
        )?;
        if !origin_exists {
            return Err(StoreError::MissingAgentParent(origin.0.clone()));
        }

        let stored: Vec<(RequestId, Item)> = {
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
            q.query_map([&source_request.0], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (request, raw) = row?;
                Ok((RequestId(request), serde_json::from_str::<Item>(&raw)?))
            })
            .collect::<Result<Vec<_>>>()?
        };
        let boundary = stored
            .iter()
            .position(|(request, item)| {
                request == source_request
                    && matches!(
                        item.0["type"].as_str(),
                        Some("function_call" | "custom_tool_call")
                    )
                    && item.0["call_id"].as_str() == Some(&boundary_call.0)
            })
            .ok_or_else(|| StoreError::MissingCheckpointCall {
                request: source_request.0.clone(),
                call_id: boundary_call.0.clone(),
            })?;
        let prefix = &stored[..=boundary];
        let mut call_kinds = HashMap::<(RequestId, CallId), ToolKind>::new();
        for (request, item) in prefix {
            if let Some(call) =
                item.tool_call()
                    .map_err(|reason| StoreError::MalformedReplayCall {
                        request: request.0.clone(),
                        call_id: item.0["call_id"].as_str().unwrap_or("").into(),
                        reason: reason.into(),
                    })?
            {
                let key = (request.clone(), call.call_id.clone());
                if call_kinds.insert(key, call.input.kind()).is_some() {
                    return Err(StoreError::AmbiguousReplayCall {
                        call_id: call.call_id.0,
                    });
                }
            }
        }
        let effort = prefix
            .iter()
            .rev()
            .find_map(|(_, item)| item.configuration_effort())
            .ok_or_else(|| StoreError::MissingCheckpointEffort(source_request.0.clone()))?;
        let id = uuid::Uuid::new_v4().to_string();
        let snapshot_request = RequestId(uuid::Uuid::new_v4().to_string());
        tx.execute(
            "INSERT INTO requests(id,parent_id,branch,created_at,input_tokens,output_tokens,cost_micros) VALUES (?1,NULL,?2,?3,0,0,0)",
            params![snapshot_request.0, format!("harness:checkpoint:{id}"), utc_millis()],
        )?;
        for (position, (_, item)) in prefix
            .iter()
            .filter(|(_, item)| !item.is_configuration_update())
            .enumerate()
        {
            let hash = Self::put_item_tx(&tx, item)?;
            tx.execute(
                "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                params![snapshot_request.0, position as i64, hash.0],
            )?;
        }
        let claims: Vec<(String, String, String, String, String, Option<String>)> = {
            let mut q = tx.prepare(
                "WITH RECURSIVE lineage(id,parent_id,depth) AS (
                     SELECT id,parent_id,0 FROM requests WHERE id=?1
                     UNION ALL SELECT r.id,r.parent_id,lineage.depth+1 FROM requests r JOIN lineage ON r.id=lineage.parent_id
                 ) SELECT c.origin,c.origin_request_id,c.call_id,c.request_id,c.state,c.output_hash FROM claims c
                   JOIN lineage ON lineage.id=c.request_id ORDER BY lineage.depth ASC",
            )?;
            q.query_map([&source_request.0], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut pending_claims = Vec::new();
        let mut copied = std::collections::HashSet::new();
        let mut boundary_claimed = false;
        for (origin, original_request, call_id, claim_request, state, output_hash) in claims {
            let key = (RequestId(claim_request.clone()), CallId(call_id.clone()));
            if let Some(kind) = call_kinds.get(&key) {
                let terminal = match state.as_str() {
                    "settled" => {
                        if let Some(hash) = &output_hash {
                            let raw: String = tx.query_row(
                                "SELECT json FROM items WHERE hash=?1",
                                [hash],
                                |row| row.get(0),
                            )?;
                            Some(serde_json::from_str::<Item>(&raw)?)
                        } else {
                            None
                        }
                    }
                    "interrupted" => Some(Item::tool_output(
                        &CallId(call_id.clone()),
                        *kind,
                        &crate::turn::JobOutput::Interrupted,
                    )),
                    _ => None,
                };
                if let Some(terminal) = terminal {
                    if let Some(start) = prefix.iter().position(|(request, item)| {
                        request.0 == claim_request
                            && item
                                .tool_call()
                                .ok()
                                .flatten()
                                .is_some_and(|call| call.call_id.0 == call_id)
                    }) {
                        let before_next_call =
                            prefix[start + 1..].iter().take_while(|(_, item)| {
                                !item
                                    .tool_call()
                                    .ok()
                                    .flatten()
                                    .is_some_and(|call| call.call_id.0 == call_id)
                            });
                        if before_next_call
                            .into_iter()
                            .any(|(_, item)| *item == terminal)
                        {
                            continue;
                        }
                    }
                }
                if call_id == boundary_call.0 && claim_request == source_request.0 {
                    boundary_claimed = true;
                }
                let identity: ConversationIdentity = serde_json::from_str(&origin)?;
                let operation = OperationId {
                    origin: identity,
                    request: RequestId(original_request.clone()),
                    call: CallId(call_id.clone()),
                };
                if !copied.insert(operation.clone()) {
                    continue;
                }
                if state == "pending" {
                    pending_claims.push(CheckpointClaim {
                        operation,
                        request: RequestId(claim_request),
                        kind: *kind,
                    });
                }
                tx.execute(
                    "INSERT INTO claims(origin,origin_request_id,call_id,request_id,state,output_hash) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![origin,original_request,call_id,snapshot_request.0,state,output_hash],
                )?;
            }
        }
        if !boundary_claimed {
            return Err(StoreError::MissingCheckpointClaim {
                request: source_request.0.clone(),
                call_id: boundary_call.0.clone(),
            });
        }
        pending_claims.sort_by(|a, b| {
            a.operation
                .request
                .0
                .cmp(&b.operation.request.0)
                .then_with(|| a.operation.call.0.cmp(&b.operation.call.0))
        });
        Self::set_effort_tx(&tx, &snapshot_request, effort)?;
        tx.execute(
            "INSERT INTO checkpoints(id,origin_agent,source_request,snapshot_request,boundary_call,metadata,pending_claims,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![id,origin.0,source_request.0,snapshot_request.0,boundary_call.0,serde_json::to_string(metadata)?,serde_json::to_string(&pending_claims)?,utc_millis()],
        )?;
        tx.commit()?;
        Ok(Checkpoint {
            id,
            origin: origin.clone(),
            source_request: source_request.clone(),
            snapshot_request,
            boundary_call: boundary_call.clone(),
            metadata: metadata.clone(),
            pending_claims,
            attachment,
            process_identity: self.process_identity.clone(),
        })
    }

    /// Register one host-authorized child from a reusable checkpoint. This
    /// copies the frozen prefix and the current settlement state of its claims.
    /// It never waits for the checkpointing call to return. It does not start
    /// or own another execution of a pending call.
    pub fn attach_checkpoint_child<T: ?Sized + Send + Sync + 'static>(
        &self,
        checkpoint: &Checkpoint<T>,
        child: CheckpointChild<'_>,
    ) -> Result<(Agent, Option<i64>)> {
        self.attach_checkpoint_inner(checkpoint, child, None)
    }

    pub(crate) fn attach_bound_checkpoint_child<T: ?Sized + Send + Sync + 'static>(
        &self,
        checkpoint: &Checkpoint<T>,
        child: CheckpointChild<'_>,
        binding: &crate::embedding::HostIdentity,
    ) -> Result<(Agent, Option<i64>)> {
        if binding.actor != *child.path || binding.run.is_empty() || binding.incarnation.is_empty()
        {
            return Err(StoreError::InvalidEmbeddedBinding);
        }
        self.attach_checkpoint_inner(checkpoint, child, Some(binding))
    }

    fn attach_checkpoint_inner<T: ?Sized + Send + Sync + 'static>(
        &self,
        checkpoint: &Checkpoint<T>,
        child: CheckpointChild<'_>,
        binding: Option<&crate::embedding::HostIdentity>,
    ) -> Result<(Agent, Option<i64>)> {
        if !Arc::ptr_eq(&self.process_identity, &checkpoint.process_identity) {
            return Err(StoreError::ForeignCheckpoint);
        }
        Self::validate_agent_path(&child.path.0, Some(&child.parent.0))?;
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let parent_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM agents WHERE path=?1)",
            [&child.parent.0],
            |row| row.get(0),
        )?;
        if !parent_exists {
            return Err(StoreError::MissingAgentParent(child.parent.0.clone()));
        }
        let snapshot_request = RequestId(uuid::Uuid::new_v4().to_string());
        tx.execute(
            "INSERT INTO requests(id,parent_id,branch,created_at,input_tokens,output_tokens,cost_micros) VALUES (?1,?2,?3,?4,0,0,0)",
            params![snapshot_request.0,checkpoint.snapshot_request.0,child.path.0,utc_millis()],
        )?;
        let inherited_claims: Vec<(String, String, String, String, Option<String>)> = {
            let mut q =
                tx.prepare("SELECT origin,origin_request_id,call_id,state,output_hash FROM claims WHERE request_id=?1")?;
            q.query_map([&checkpoint.snapshot_request.0], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (origin, original_request, call_id, state, output_hash) in inherited_claims {
            tx.execute(
                "INSERT INTO claims(origin,origin_request_id,call_id,request_id,state,output_hash) VALUES (?1,?2,?3,?4,?5,?6)",
                params![origin,original_request,call_id,snapshot_request.0,state,output_hash],
            )?;
        }
        let source = json!({
            "kind":"checkpoint",
            "checkpoint_id":checkpoint.id,
            "origin_agent":checkpoint.origin,
            "source_request":checkpoint.source_request,
            "snapshot_request":checkpoint.snapshot_request,
            "boundary_call":checkpoint.boundary_call,
            "checkout":child.checkout,
        });
        let created_at = utc_millis();
        tx.execute(
            "INSERT INTO agents(path,parent_path,head_request,contract,fork_source,state,created_at) VALUES (?1,?2,?3,?4,?5,'active',?6)",
            params![child.path.0,child.parent.0,snapshot_request.0,serde_json::to_string(child.contract)?,source.to_string(),created_at],
        )?;
        if let Some(binding) = binding {
            tx.execute(
                "INSERT INTO embedded_bindings(agent_path,run_id,incarnation) VALUES (?1,?2,?3)",
                params![binding.actor.0, binding.run, binding.incarnation],
            )?;
        }
        let envelope_id = if let Some(task) = child.task {
            let hash = Self::put_item_tx(&tx, task.item)?;
            tx.execute(
                "INSERT INTO envelopes(sender,recipient,class,item_hash,delivered_request,created_at) VALUES (?1,?2,?3,?4,NULL,?5)",
                params![task.sender,child.path.0,task.class,hash.0,utc_millis()],
            )?;
            Some(tx.last_insert_rowid())
        } else {
            None
        };
        tx.commit()?;
        Ok((
            Agent {
                path: child.path.clone(),
                parent: Some(child.parent.clone()),
                head_request: Some(snapshot_request),
                contract: child.contract.clone(),
                fork_source: source,
                state: AgentState::Active,
                created_at,
            },
            envelope_id,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Effort;
    use crate::store::ClaimState;

    fn item(value: Value) -> Item {
        Item(value)
    }

    #[test]
    fn checkpoint_omits_completed_earlier_equal_wire_id() {
        let store = Store::memory().unwrap();
        let root = AgentPath("/root".into());
        let first = RequestId("first-equal-call".into());
        let second = RequestId("second-equal-call".into());
        let call = CallId("same-wire-id".into());
        let invocation = item(json!({
            "type":"function_call", "call_id":call.0, "name":"slow", "arguments":"{}"
        }));
        store.create_request(&first, None, &root.0).unwrap();
        store.set_effort(&first, Effort::Medium).unwrap();
        store
            .append_items(&first, std::slice::from_ref(&invocation))
            .unwrap();
        let first_op = store.claim(&call, &first).unwrap();
        let output = item(json!({
            "type":"function_call_output", "call_id":call.0, "output":"\"first\""
        }));
        store.write_output(&first_op, &output).unwrap();
        store.append_items(&first, &[output]).unwrap();
        store
            .create_request(&second, Some(&first), &root.0)
            .unwrap();
        store.append_items(&second, &[invocation]).unwrap();
        let second_op = store.claim(&call, &second).unwrap();
        store
            .admit_agent(&root, None, Some(&second), &json!({}), &json!({}))
            .unwrap();
        let checkpoint = store
            .capture_checkpoint(&root, &second, &call, &json!({}), Arc::new(()))
            .unwrap();
        let inherited = store.claims_on(checkpoint.snapshot_request()).unwrap();
        assert_eq!(inherited.len(), 1);
        assert_eq!(inherited[0].operation, second_op);
        assert_eq!(checkpoint.pending_claims().len(), 1);
    }

    #[test]
    fn checkpoint_freezes_exact_prefix_and_supports_two_immediate_children() {
        let store = Store::memory().unwrap();
        let root = AgentPath("/root".into());
        let workflow = AgentPath("/root/workflow".into());
        let source = RequestId("source".into());
        let raw = CallId("raw".into());
        let typed = CallId("typed".into());
        let later = CallId("later".into());
        store.create_request(&source, None, &root.0).unwrap();
        store.set_effort(&source, Effort::Medium).unwrap();
        store
            .admit_agent(&root, None, Some(&source), &json!({}), &json!({}))
            .unwrap();
        store
            .admit_agent(&workflow, Some(&root), None, &json!({}), &json!({}))
            .unwrap();
        let raw_text = "literal $() `ticks` \\n newline";
        let raw_item =
            item(json!({"type":"custom_tool_call","call_id":raw.0,"name":"cell","input":raw_text}));
        let typed_item = item(
            json!({"type":"function_call","call_id":typed.0,"name":"work","arguments":"{\"value\":2}"}),
        );
        let later_item =
            item(json!({"type":"function_call","call_id":later.0,"name":"late","arguments":"{}"}));
        store
            .append_items(
                &source,
                &[
                    item(json!({"type":"message","role":"user","content":"req"})),
                    raw_item.clone(),
                    typed_item.clone(),
                    later_item,
                ],
            )
            .unwrap();
        for call in [&raw, &typed, &later] {
            store.claim(call, &source).unwrap();
        }
        let attachment = Arc::new(String::from("source lease"));
        let checkpoint = store
            .capture_checkpoint(
                &root,
                &source,
                &typed,
                &json!({"source_version":"v1","private_bindings":["x"]}),
                attachment.clone(),
            )
            .unwrap();
        assert_eq!(checkpoint.attachment().as_str(), "source lease");
        assert_eq!(Arc::strong_count(&attachment), 2);
        let frozen = store.items(checkpoint.snapshot_request()).unwrap();
        assert!(frozen.contains(&raw_item));
        assert!(frozen.contains(&typed_item));
        assert!(!frozen.iter().any(|item| item.0["call_id"] == later.0));
        assert_eq!(
            frozen
                .iter()
                .find(|item| item.0["call_id"] == raw.0)
                .unwrap()
                .0["input"],
            raw_text
        );
        let captured_claims = store.claims_on(checkpoint.snapshot_request()).unwrap();
        assert_eq!(captured_claims.len(), 2);
        assert_eq!(
            checkpoint.pending_claims(),
            &[
                CheckpointClaim {
                    operation: store.operation_for_request(&source, &raw).unwrap(),
                    request: source.clone(),
                    kind: ToolKind::Custom
                },
                CheckpointClaim {
                    operation: store.operation_for_request(&source, &typed).unwrap(),
                    request: source.clone(),
                    kind: ToolKind::Function
                },
            ]
        );
        assert!(
            captured_claims
                .iter()
                .all(|claim| claim.state == ClaimState::Pending)
        );

        let worker = AgentPath("/root/worker".into());
        let review = AgentPath("/root/workflow/review".into());
        let (first, _) = store
            .attach_checkpoint_child(
                &checkpoint,
                CheckpointChild {
                    path: &worker,
                    parent: &root,
                    contract: &json!({"task":"implement"}),
                    checkout: &json!({"revision":"a1"}),
                    task: None,
                },
            )
            .unwrap();
        let (second, _) = store
            .attach_checkpoint_child(
                &checkpoint,
                CheckpointChild {
                    path: &review,
                    parent: &workflow,
                    contract: &json!({"task":"review"}),
                    checkout: &json!({"revision":"b2"}),
                    task: None,
                },
            )
            .unwrap();
        assert_eq!(first.fork_source["kind"], "checkpoint");
        assert_eq!(first.fork_source["checkout"]["revision"], "a1");
        assert_eq!(second.fork_source["checkout"]["revision"], "b2");
        assert_eq!(second.parent, Some(workflow));
        assert_eq!(
            store
                .request(first.head_request.as_ref().unwrap())
                .unwrap()
                .unwrap()
                .parent,
            Some(checkpoint.snapshot_request().clone())
        );
        for child in [&first, &second] {
            let claims = store
                .claims_on(child.head_request.as_ref().unwrap())
                .unwrap();
            assert_eq!(claims.len(), 2);
            assert!(
                claims
                    .iter()
                    .all(|claim| claim.state == ClaimState::Pending)
            );
        }
        store
            .append_items(
                &source,
                &[item(
                    json!({"type":"message","role":"assistant","content":"later mutation"}),
                )],
            )
            .unwrap();
        assert_eq!(store.items(checkpoint.snapshot_request()).unwrap(), frozen);
        store.interrupt_claim(&typed, &source).unwrap();
        assert!(
            store
                .claims_on(first.head_request.as_ref().unwrap())
                .unwrap()
                .iter()
                .any(|claim| claim.call_id == typed && claim.state == ClaimState::Pending)
        );
        let output =
            item(json!({"type":"custom_tool_call_output","call_id":raw.0,"output":"done"}));
        store
            .settle_claims(&store.claims(&raw).unwrap()[0].operation, &output)
            .unwrap();
        assert!(
            store
                .claims_on(second.head_request.as_ref().unwrap())
                .unwrap()
                .iter()
                .any(|claim| claim.call_id == raw && claim.state == ClaimState::Settled)
        );
        drop(checkpoint);
        assert_eq!(Arc::strong_count(&attachment), 1);
    }

    #[test]
    fn failed_capture_releases_attachment_and_foreign_store_rejects_handle() {
        let path =
            std::env::temp_dir().join(format!("harness-checkpoint-{}.db", uuid::Uuid::new_v4()));
        let store = Store::open(&path).unwrap();
        let root = AgentPath("/root".into());
        let source = RequestId("source".into());
        let call = CallId("real".into());
        store.create_request(&source, None, &root.0).unwrap();
        store.set_effort(&source, Effort::Low).unwrap();
        store
            .admit_agent(&root, None, Some(&source), &json!({}), &json!({}))
            .unwrap();
        store
            .append_items(
                &source,
                &[item(
                    json!({"type":"function_call","call_id":call.0,"name":"f","arguments":"{}"}),
                )],
            )
            .unwrap();
        let attachment = Arc::new(String::from("lease"));
        assert!(matches!(
            store.capture_checkpoint(
                &root,
                &source,
                &CallId("missing".into()),
                &json!({}),
                attachment.clone()
            ),
            Err(StoreError::MissingCheckpointCall { .. })
        ));
        assert_eq!(Arc::strong_count(&attachment), 1);
        assert!(matches!(
            store.capture_checkpoint(&root, &source, &call, &json!({}), attachment.clone()),
            Err(StoreError::MissingCheckpointClaim { .. })
        ));
        assert_eq!(Arc::strong_count(&attachment), 1);
        store.claim(&call, &source).unwrap();
        let checkpoint = store
            .capture_checkpoint(&root, &source, &call, &json!({}), attachment)
            .unwrap();
        let reopened = Store::open(&path).unwrap();
        assert!(matches!(
            reopened.attach_checkpoint_child(
                &checkpoint,
                CheckpointChild {
                    path: &AgentPath("/root/child".into()),
                    parent: &root,
                    contract: &json!({}),
                    checkout: &json!({}),
                    task: None,
                }
            ),
            Err(StoreError::ForeignCheckpoint)
        ));
        assert!(
            reopened
                .request(checkpoint.snapshot_request())
                .unwrap()
                .is_some()
        );
        assert_eq!(reopened.list_agents().unwrap().len(), 1);
        drop(reopened);
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn checkpoint_child_engine_runs_while_origin_call_is_pending() {
        use crate::{
            engine::{Engine, EngineConfig, ResponsesTransport},
            provider::{Provider, ProviderError},
            transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, Usage},
            turn::JobScheduler,
        };
        use async_trait::async_trait;
        use std::sync::Mutex;
        use tokio::sync::{mpsc, oneshot, watch};

        #[derive(Clone)]
        struct FakeAuth;
        impl Auth for FakeAuth {
            fn access(&self) -> std::result::Result<(String, String), TransportError> {
                Ok(("unused".into(), "unused".into()))
            }
        }
        struct Replay {
            sent: Arc<Mutex<Vec<ResponsesRequest>>>,
            turns: Mutex<std::collections::VecDeque<ResponsesTurn>>,
        }
        #[async_trait]
        impl ResponsesTransport for Replay {
            async fn create(
                &self,
                request: ResponsesRequest,
            ) -> std::result::Result<ResponsesTurn, TransportError> {
                self.sent.lock().unwrap().push(request);
                self.turns
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or_else(|| TransportError::Stream("replay exhausted".into()))
            }
        }
        struct Delayed {
            release: tokio::sync::Mutex<Option<oneshot::Receiver<()>>>,
        }
        #[async_trait]
        impl Provider for Delayed {
            async fn call(
                &self,
                name: &str,
                _args: Value,
            ) -> std::result::Result<Value, ProviderError> {
                if name == "slow" {
                    self.release.lock().await.take().unwrap().await.unwrap();
                }
                Ok(json!({"name":name,"done":true}))
            }
            fn tools(&self) -> Vec<Value> {
                vec![]
            }
        }
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/child".into());
        let source = RequestId("parent-pending".into());
        let call = CallId("original-slow-call".into());
        store.create_request(&source, None, &root.0).unwrap();
        store.set_effort(&source, Effort::Low).unwrap();
        store
            .admit_agent(&root, None, Some(&source), &json!({}), &json!({}))
            .unwrap();
        store
            .append_items(
                &source,
                &[item(json!({
                    "type":"function_call","call_id":call.0,"name":"slow","arguments":"{}"
                }))],
            )
            .unwrap();
        store.claim(&call, &source).unwrap();
        let (release_tx, release_rx) = oneshot::channel();
        let provider = Arc::new(Delayed {
            release: tokio::sync::Mutex::new(Some(release_rx)),
        });
        let scheduler = Arc::new(JobScheduler::new(2).unwrap());
        scheduler
            .start_operation(
                provider.clone(),
                store.operation_for_request(&source, &call).unwrap(),
                root.clone(),
                Some(source.clone()),
                "slow".into(),
                json!({}),
            )
            .await
            .unwrap();
        scheduler
            .claim_exact(
                &store.operation_for_request(&source, &call).unwrap(),
                store.standalone_identity(root.clone()),
            )
            .await
            .unwrap();
        let checkpoint = store
            .capture_checkpoint(&root, &source, &call, &json!({}), Arc::new(()))
            .unwrap();
        let (attached, _) = store
            .attach_checkpoint_child(
                &checkpoint,
                CheckpointChild {
                    path: &child,
                    parent: &root,
                    contract: &json!({}),
                    checkout: &json!({"revision":"exact"}),
                    task: None,
                },
            )
            .unwrap();
        let sent = Arc::new(Mutex::new(Vec::new()));
        let replay = Replay {
            sent:sent.clone(),
            turns:Mutex::new([
                ResponsesTurn {response_id:"first".into(),items:vec![item(json!({
                    "type":"function_call","call_id":"child-quick","name":"quick","arguments":"{}"
                }))],usage:Usage::default()},
                ResponsesTurn {response_id:"final".into(),items:vec![item(json!({
                    "type":"message","role":"assistant","phase":"final_answer","content":"done"
                }))],usage:Usage::default()},
            ].into()),
        };
        let engine = Engine::<FakeAuth, Delayed, _>::with_transport(
            replay,
            store.clone(),
            scheduler,
            provider,
            EngineConfig {
                instructions: "child".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "checkpoint-child".into(),
                agent: child,
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_mail_tx, mail_rx) = mpsc::unbounded_channel();
        let head = attached.head_request.unwrap();
        let run =
            tokio::spawn(async move { engine.run(Some(head), vec![], cancel_rx, mail_rx).await });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if !sent.lock().unwrap().is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("child's first request starts before original call returns");
        assert!(
            sent.lock().unwrap()[0]
                .input
                .iter()
                .any(|item| item.0["call_id"] == call.0)
        );
        assert!(!sent.lock().unwrap()[0].input.iter().any(|item| item.0["type"] == "function_call_output" && item.0["call_id"] == call.0));
        release_tx.send(()).unwrap();
        let completion = tokio::time::timeout(std::time::Duration::from_secs(2), run)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(completion.turn.response_id, "final");
        assert_eq!(
            completion
                .transcript
                .iter()
                .filter(
                    |item| item.0["type"] == "function_call_output" && item.0["call_id"] == call.0
                )
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn cancelled_origin_call_replays_to_later_child_without_reexecution() {
        use crate::{
            engine::{Engine, EngineConfig, ResponsesTransport},
            provider::{Provider, ProviderError},
            transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, Usage},
            turn::{JobOutput, JobScheduler},
        };
        use async_trait::async_trait;
        use std::sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        };
        use tokio::sync::{Notify, mpsc, watch};

        #[derive(Clone)]
        struct FakeAuth;
        impl Auth for FakeAuth {
            fn access(&self) -> std::result::Result<(String, String), TransportError> {
                Ok(("unused".into(), "unused".into()))
            }
        }
        struct Replay {
            sent: Arc<Mutex<Vec<ResponsesRequest>>>,
        }
        #[async_trait]
        impl ResponsesTransport for Replay {
            async fn create(
                &self,
                request: ResponsesRequest,
            ) -> std::result::Result<ResponsesTurn, TransportError> {
                self.sent.lock().unwrap().push(request);
                Ok(ResponsesTurn {
                    response_id: "final".into(),
                    items: vec![item(json!({
                        "type":"message","role":"assistant","phase":"final_answer","content":"continued"
                    }))],
                    usage: Usage::default(),
                })
            }
        }
        struct PendingProvider {
            started: Arc<Notify>,
            calls: Arc<AtomicUsize>,
        }
        #[async_trait]
        impl Provider for PendingProvider {
            async fn call(
                &self,
                _name: &str,
                _args: Value,
            ) -> std::result::Result<Value, ProviderError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.started.notify_one();
                std::future::pending().await
            }
            fn tools(&self) -> Vec<Value> {
                vec![]
            }
        }
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/later".into());
        let source = RequestId("cancelled-origin".into());
        let call = CallId("shared-call".into());
        store.create_request(&source, None, &root.0).unwrap();
        store.set_effort(&source, Effort::Low).unwrap();
        store
            .admit_agent(&root, None, Some(&source), &json!({}), &json!({}))
            .unwrap();
        store
            .append_items(
                &source,
                &[item(json!({
                    "type":"function_call","call_id":call.0,"name":"slow","arguments":"{}"
                }))],
            )
            .unwrap();
        store.claim(&call, &source).unwrap();
        let started = Arc::new(Notify::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = Arc::new(PendingProvider {
            started: started.clone(),
            calls: calls.clone(),
        });
        let scheduler = Arc::new(JobScheduler::new(1).unwrap());
        scheduler
            .start_operation(
                provider.clone(),
                store.operation_for_request(&source, &call).unwrap(),
                root.clone(),
                Some(source.clone()),
                "slow".into(),
                json!({}),
            )
            .await
            .unwrap();
        scheduler
            .claim_exact(
                &store.operation_for_request(&source, &call).unwrap(),
                store.standalone_identity(root.clone()),
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .unwrap();
        let attachment = Arc::new(String::from("kept source"));
        let checkpoint = store
            .capture_checkpoint(&root, &source, &call, &json!({}), attachment.clone())
            .unwrap();
        assert_eq!(
            scheduler
                .cancel(&store.claims(&call).unwrap()[0].operation)
                .await
                .unwrap()
                .unwrap()
                .output,
            JobOutput::Cancelled
        );
        let cancelled = Item::tool_output(&call, ToolKind::Function, &JobOutput::Cancelled);
        assert_eq!(
            store
                .write_output(&store.claims(&call).unwrap()[0].operation, &cancelled)
                .unwrap(),
            2
        );
        assert_eq!(
            store.claims_on(checkpoint.snapshot_request()).unwrap()[0].state,
            ClaimState::Settled
        );
        let (attached, _) = store
            .attach_checkpoint_child(
                &checkpoint,
                CheckpointChild {
                    path: &child,
                    parent: &root,
                    contract: &json!({}),
                    checkout: &json!({"revision":"later"}),
                    task: None,
                },
            )
            .unwrap();
        assert_eq!(
            store
                .claims_on(attached.head_request.as_ref().unwrap())
                .unwrap()[0]
                .state,
            ClaimState::Settled
        );
        let sent = Arc::new(Mutex::new(Vec::new()));
        let engine = Engine::<FakeAuth, PendingProvider, _>::with_transport(
            Replay { sent: sent.clone() },
            store.clone(),
            scheduler,
            provider,
            EngineConfig {
                instructions: "child".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Low,
                session_id: "cancelled-child".into(),
                agent: child,
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_mail_tx, mail_rx) = mpsc::unbounded_channel();
        let completion = engine
            .run(attached.head_request, vec![], cancel_rx, mail_rx)
            .await
            .unwrap();
        assert_eq!(completion.turn.response_id, "final");
        let received = sent.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(
            received[0]
                .input
                .iter()
                .filter(|item| *item == &cancelled)
                .count(),
            1
        );
        assert_eq!(
            completion
                .transcript
                .iter()
                .filter(|item| *item == &cancelled)
                .count(),
            1
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the child never reruns the source call"
        );
        assert_eq!(
            Arc::strong_count(&attachment),
            2,
            "cancellation leaves the checkpoint attachment live"
        );
    }

    #[tokio::test]
    async fn provider_captures_checkpoint_at_its_live_call_boundary() {
        use crate::{
            engine::{Engine, EngineConfig, ResponsesTransport},
            provider::{CallContext, Provider, ProviderError},
            transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, Usage},
            turn::JobScheduler,
        };
        use async_trait::async_trait;
        use std::sync::Mutex;
        use tokio::sync::{mpsc, watch};

        #[derive(Clone)]
        struct FakeAuth;
        impl Auth for FakeAuth {
            fn access(&self) -> std::result::Result<(String, String), TransportError> {
                Ok(("unused".into(), "unused".into()))
            }
        }
        struct Replay {
            turns: Mutex<std::collections::VecDeque<ResponsesTurn>>,
        }
        #[async_trait]
        impl ResponsesTransport for Replay {
            async fn create(
                &self,
                _request: ResponsesRequest,
            ) -> std::result::Result<ResponsesTurn, TransportError> {
                self.turns
                    .lock()
                    .unwrap()
                    .pop_front()
                    .ok_or_else(|| TransportError::Stream("replay exhausted".into()))
            }
        }
        struct CaptureProvider {
            store: Arc<Store>,
            captured: Mutex<Option<Checkpoint<()>>>,
            error: Mutex<Option<String>>,
        }
        #[async_trait]
        impl Provider for CaptureProvider {
            async fn call(
                &self,
                _name: &str,
                _args: Value,
            ) -> std::result::Result<Value, ProviderError> {
                unreachable!("Engine must supply the original call context")
            }
            async fn call_with_context(
                &self,
                _name: &str,
                _args: Value,
                context: CallContext,
            ) -> std::result::Result<Value, ProviderError> {
                let request = context.request.as_ref().expect("request identity is bound");
                match self.store.capture_checkpoint(
                    &context.agent,
                    request,
                    &context.call_id,
                    &json!({"source_version":"issued-inside-call"}),
                    Arc::new(()),
                ) {
                    Ok(handle) => {
                        *self.captured.lock().unwrap() = Some(handle);
                        Ok(json!({"checkpoint":"captured"}))
                    }
                    Err(error) => {
                        *self.error.lock().unwrap() = Some(error.to_string());
                        Err(ProviderError::Tool(error.to_string()))
                    }
                }
            }
            fn tools(&self) -> Vec<Value> {
                vec![]
            }
        }
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({}))
            .unwrap();
        let provider = Arc::new(CaptureProvider {
            store: store.clone(),
            captured: Mutex::new(None),
            error: Mutex::new(None),
        });
        let replay = Replay { turns: Mutex::new([
            ResponsesTurn {response_id:"call".into(),items:vec![item(json!({
                "type":"function_call","call_id":"capture-call","name":"capture","arguments":"{}"
            }))],usage:Usage::default()},
            ResponsesTurn {response_id:"final".into(),items:vec![item(json!({
                "type":"message","role":"assistant","phase":"final_answer","content":"done"
            }))],usage:Usage::default()},
        ].into()) };
        let engine = Engine::<FakeAuth, CaptureProvider, _>::with_transport(
            replay,
            store.clone(),
            Arc::new(JobScheduler::new(1).unwrap()),
            provider.clone(),
            EngineConfig {
                instructions: "root".into(),
                tools: vec![],
                model: "test".into(),
                effort: Effort::Medium,
                session_id: "provider-checkpoint".into(),
                agent: root.clone(),
            },
        );
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_mail_tx, mail_rx) = mpsc::unbounded_channel();
        engine.run(None, vec![], cancel_rx, mail_rx).await.unwrap();
        let captured = provider.captured.lock().unwrap();
        assert!(
            captured.is_some(),
            "capture inside provider failed: {:?}",
            provider.error.lock().unwrap()
        );
        let checkpoint = captured.as_ref().unwrap();
        assert_eq!(
            checkpoint.pending_claims(),
            &[CheckpointClaim {
                operation: store
                    .operation_for_request(
                        checkpoint.source_request(),
                        &CallId("capture-call".into())
                    )
                    .unwrap(),
                request: checkpoint.source_request().clone(),
                kind: ToolKind::Function,
            }]
        );
        assert!(
            store
                .items(checkpoint.snapshot_request())
                .unwrap()
                .iter()
                .any(|item| item.0["call_id"] == "capture-call")
        );
    }
}
