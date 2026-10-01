//! Claim settlement owns terminal state independently of rendered tool payloads.
use super::{Result, Store, StoreError};
use crate::{
    item::{Item, ItemHash},
    model::{OperationId, RequestId},
    provider::ToolFailure,
    turn::JobOutput,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum TerminalOutcome {
    Success,
    Failure(ToolFailure),
    Cancelled,
    Interrupted,
    CancellationUnconfirmed(String),
}

impl From<&JobOutput> for TerminalOutcome {
    fn from(output: &JobOutput) -> Self {
        match output {
            JobOutput::Completed(Ok(_)) => Self::Success,
            JobOutput::Completed(Err(failure)) => Self::Failure(failure.clone()),
            JobOutput::Cancelled => Self::Cancelled,
            JobOutput::Interrupted => Self::Interrupted,
            JobOutput::CancellationUnconfirmed(detail) => {
                Self::CancellationUnconfirmed(detail.clone())
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RecordedToolOutput {
    pub item: Item,
    pub terminal: TerminalOutcome,
}

pub(super) fn exact_terminal(
    c: &Connection,
    operation: &OperationId,
) -> Result<Option<(ItemHash, TerminalOutcome)>> {
    let origin = serde_json::to_string(&operation.origin)?;
    let mut query = c.prepare(
        "SELECT output_hash,terminal_json FROM claims WHERE origin=?1 AND origin_request_id=?2 AND call_id=?3 AND state='settled' ORDER BY request_id",
    )?;
    let rows = query.query_map(
        params![origin, operation.request.0, operation.call.0],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    )?;
    let mut found = None;
    for row in rows {
        let (hash, terminal) = row?;
        let (Some(hash), Some(terminal)) = (hash, terminal) else {
            return Err(StoreError::UnsupportedReplayOutcome {
                operation: operation.clone(),
            });
        };
        let candidate = (
            ItemHash(hash),
            serde_json::from_str::<TerminalOutcome>(&terminal)?,
        );
        if found
            .as_ref()
            .is_some_and(|previous| previous != &candidate)
        {
            return Err(StoreError::ConflictingReplayOutcome {
                operation: operation.clone(),
            });
        }
        found = Some(candidate);
    }
    Ok(found)
}

fn exact_claim_state(
    c: &Connection,
    operation: &OperationId,
    claimant: &RequestId,
) -> Result<Option<super::ClaimState>> {
    let origin = serde_json::to_string(&operation.origin)?;
    let state: Option<String> = c.query_row(
        "SELECT state FROM claims WHERE origin=?1 AND origin_request_id=?2 AND call_id=?3 AND request_id=?4",
        params![origin,operation.request.0,operation.call.0,claimant.0], |row| row.get(0),
    ).optional()?;
    match state.as_deref() {
        Some("pending") => Ok(Some(super::ClaimState::Pending)),
        Some("settled") => Ok(Some(super::ClaimState::Settled)),
        Some("interrupted") => Ok(Some(super::ClaimState::Interrupted)),
        None => Ok(None),
        _ => Err(StoreError::UnsupportedReplayOutcome {
            operation: operation.clone(),
        }),
    }
}

impl Store {
    /// Exact original operation; inherited claimants must retain identical evidence.
    pub fn replay_tool_output_operation(
        &self,
        operation: &OperationId,
    ) -> Result<Option<RecordedToolOutput>> {
        self.replay_tool_output_claim(operation, &operation.request)
    }

    /// Claim-local interruption never substitutes for another claimant's result.
    pub fn replay_tool_output_claim(
        &self,
        operation: &OperationId,
        claimant: &RequestId,
    ) -> Result<Option<RecordedToolOutput>> {
        let c = self.lock();
        let Some(kind) = super::invocation_kind(&c, &operation.request, &operation.call)? else {
            return Ok(None);
        };
        match exact_claim_state(&c, operation, claimant)? {
            Some(super::ClaimState::Interrupted) => {
                return Ok(Some(RecordedToolOutput {
                    item: Item::tool_output(&operation.call, kind, &JobOutput::Interrupted),
                    terminal: TerminalOutcome::Interrupted,
                }));
            }
            Some(super::ClaimState::Settled) => {}
            Some(super::ClaimState::Pending) | None => return Ok(None),
        }
        let terminal = exact_terminal(&c, operation)?;
        let Some((hash, terminal)) = terminal else {
            return Ok(None);
        };
        let raw: Option<String> = c
            .query_row("SELECT json FROM items WHERE hash=?1", [&hash.0], |row| {
                row.get(0)
            })
            .optional()?;
        let raw = raw.ok_or_else(|| StoreError::MissingReplayItem(hash.0))?;
        let item: Item = serde_json::from_str(&raw)?;
        super::validate_replay_output(&operation.call, kind, &item)?;
        Ok(Some(RecordedToolOutput { item, terminal }))
    }

    pub(crate) fn has_completed_output(&self, operation: &OperationId) -> Result<bool> {
        let c = self.lock();
        if exact_claim_state(&c, operation, &operation.request)? != Some(super::ClaimState::Settled)
        {
            return Ok(false);
        }
        Ok(exact_terminal(&c, operation)?.is_some_and(|(_, outcome)| {
            matches!(
                outcome,
                TerminalOutcome::Success | TerminalOutcome::Failure(_)
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        item::ToolKind,
        model::{CallId, RequestId},
    };
    use serde_json::json;

    fn operation(store: &Store, request: &str) -> OperationId {
        let request = RequestId(request.into());
        store.create_request(&request, None, "/root").unwrap();
        let call = CallId("same-wire-id".into());
        store.append_items(&request, &[Item(json!({"type":"function_call","call_id":call.0,"name":"probe","arguments":"{}"}))]).unwrap();
        store.claim(&call, &request).unwrap()
    }

    fn path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("harness-terminal-{}.sqlite", uuid::Uuid::new_v4()))
    }

    #[test]
    fn exact_terminal_outcomes_survive_reopen_without_interpreting_success_payloads() {
        let path = path();
        let outputs = [
            JobOutput::Completed(Ok(
                json!({"error":"successful business data","failure":{"opaque":true}}),
            )),
            JobOutput::Completed(Err(ToolFailure::with_metadata(
                "failed",
                json!({"class":"input_rejected","phase":"compile","cause":{"kind":"parser"}}),
            ))),
            JobOutput::Cancelled,
            JobOutput::Interrupted,
            JobOutput::CancellationUnconfirmed("owner still active".into()),
        ];
        let operations = {
            let store = Store::open(&path).unwrap();
            outputs
                .iter()
                .enumerate()
                .map(|(index, output)| {
                    let operation = operation(&store, &format!("request-{index}"));
                    assert_eq!(
                        store
                            .write_job_output(&operation, ToolKind::Function, output)
                            .unwrap(),
                        1
                    );
                    operation
                })
                .collect::<Vec<_>>()
        };
        let store = Store::open(&path).unwrap();
        for (operation, output) in operations.iter().zip(&outputs) {
            let recorded = store
                .replay_tool_output_operation(operation)
                .unwrap()
                .unwrap();
            assert_eq!(recorded.terminal, TerminalOutcome::from(output));
            assert_eq!(
                recorded.item,
                Item::tool_output(&operation.call, ToolKind::Function, output)
            );
            assert_eq!(
                store.has_completed_output(operation).unwrap(),
                matches!(output, JobOutput::Completed(_))
            );
        }
        let success = store
            .replay_tool_output_operation(&operations[0])
            .unwrap()
            .unwrap();
        assert_eq!(success.terminal, TerminalOutcome::Success);
        let wire: serde_json::Value =
            serde_json::from_str(success.item.0["output"].as_str().unwrap()).unwrap();
        assert_eq!(wire["error"], "successful business data");
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn interrupted_projection_is_claim_local_with_competing_settlement() {
        for interrupt_original in [true, false] {
            let store = Store::memory().unwrap();
            let operation = operation(&store, "original");
            let child = RequestId("child-claim".into());
            store
                .create_request(&child, Some(&operation.request), "/root/child")
                .unwrap();
            store.lock().execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state) SELECT origin,origin_request_id,call_id,?1,'pending' FROM claims WHERE request_id=?2", params![child.0,operation.request.0]).unwrap();
            let interrupted = if interrupt_original {
                &operation.request
            } else {
                &child
            };
            let settled = if interrupt_original {
                &child
            } else {
                &operation.request
            };
            store
                .interrupt_operation_claim(&operation, interrupted)
                .unwrap();
            store
                .write_job_output(
                    &operation,
                    ToolKind::Function,
                    &JobOutput::Completed(Ok(json!({"answer":42}))),
                )
                .unwrap();
            assert_eq!(
                store
                    .replay_tool_output_claim(&operation, interrupted)
                    .unwrap()
                    .unwrap()
                    .terminal,
                TerminalOutcome::Interrupted
            );
            assert_eq!(
                store
                    .replay_tool_output_claim(&operation, settled)
                    .unwrap()
                    .unwrap()
                    .terminal,
                TerminalOutcome::Success
            );
            assert_eq!(
                store
                    .replay_tool_output_operation(&operation)
                    .unwrap()
                    .unwrap()
                    .terminal,
                if interrupt_original {
                    TerminalOutcome::Interrupted
                } else {
                    TerminalOutcome::Success
                }
            );
            assert_eq!(
                store.has_completed_output(&operation).unwrap(),
                !interrupt_original
            );
            assert!(
                store
                    .replay_tool_output_claim(&operation, &RequestId("absent".into()))
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn pending_claim_cannot_replay_a_siblings_settlement() {
        let store = Store::memory().unwrap();
        let operation = operation(&store, "original");
        store
            .write_job_output(&operation, ToolKind::Function, &JobOutput::Cancelled)
            .unwrap();
        let child = RequestId("late-pending-child".into());
        store
            .create_request(&child, Some(&operation.request), "/root/child")
            .unwrap();
        store.lock().execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state) SELECT origin,origin_request_id,call_id,?1,'pending' FROM claims WHERE request_id=?2", params![child.0,operation.request.0]).unwrap();
        assert!(
            store
                .replay_tool_output_claim(&operation, &child)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn schema_seven_unmarked_settlement_is_refused_without_changing_bytes() {
        let path = path();
        let (operation, before, hash) = {
            let store = Store::open(&path).unwrap();
            let operation = operation(&store, "old");
            let output = JobOutput::Completed(Ok(json!({"error":"unclassified legacy payload"})));
            store
                .write_job_output(&operation, ToolKind::Function, &output)
                .unwrap();
            let item = store.replay_output_operation(&operation).unwrap().unwrap();
            let hash = store.claims_for_operation(&operation).unwrap()[0]
                .output
                .clone()
                .unwrap();
            let before: String = store
                .lock()
                .query_row("SELECT json FROM items WHERE hash=?1", [&hash.0], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(item.0["call_id"], operation.call.0);
            store.lock().execute_batch("ALTER TABLE claims DROP COLUMN terminal_json; UPDATE schema_version SET version=7;").unwrap();
            (operation, before, hash)
        };
        let store = Store::open(&path).unwrap();
        assert!(
            matches!(store.replay_tool_output_operation(&operation),Err(StoreError::UnsupportedReplayOutcome {operation: missing}) if missing==operation)
        );
        assert!(store.replay_output_operation(&operation).unwrap().is_some());
        let after: String = store
            .lock()
            .query_row("SELECT json FROM items WHERE hash=?1", [&hash.0], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(before, after);
        let version: u32 = store
            .lock()
            .query_row("SELECT version FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 8);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn terminal_settlement_failure_rolls_back_hash_state_and_outcome() {
        let store = Store::memory().unwrap();
        let operation = operation(&store, "rollback");
        store.lock().execute_batch("CREATE TRIGGER reject_terminal BEFORE UPDATE ON claims WHEN NEW.terminal_json IS NOT NULL BEGIN SELECT RAISE(ABORT,'refuse terminal'); END;").unwrap();
        let output = JobOutput::Completed(Err(ToolFailure::with_metadata(
            "new failure",
            json!({"class":"offline"}),
        )));
        let item = Item::tool_output(&operation.call, ToolKind::Function, &output);
        let hash = ItemHash(
            blake3::hash(&serde_json::to_vec(&item).unwrap())
                .to_hex()
                .to_string(),
        );
        assert!(
            store
                .write_job_output(&operation, ToolKind::Function, &output)
                .is_err()
        );
        assert!(store.get_item(&hash).unwrap().is_none());
        assert!(
            store
                .replay_tool_output_operation(&operation)
                .unwrap()
                .is_none()
        );
        let claim = &store.claims_for_operation(&operation).unwrap()[0];
        assert_eq!(claim.state, super::super::ClaimState::Pending);
        assert!(claim.output.is_none());
    }

    #[test]
    fn conflicting_inherited_outcomes_refuse_instead_of_choosing_one_row() {
        let store = Store::memory().unwrap();
        let operation = operation(&store, "original");
        store
            .write_job_output(&operation, ToolKind::Function, &JobOutput::Cancelled)
            .unwrap();
        let child = RequestId("inherited".into());
        store
            .create_request(&child, Some(&operation.request), "/root/child")
            .unwrap();
        store.lock().execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state,output_hash,terminal_json) SELECT origin,origin_request_id,call_id,?1,state,output_hash,terminal_json FROM claims WHERE request_id=?2",params![child.0,operation.request.0]).unwrap();
        assert_eq!(
            store
                .replay_tool_output_operation(&operation)
                .unwrap()
                .unwrap()
                .terminal,
            TerminalOutcome::Cancelled
        );
        store
            .lock()
            .execute(
                "UPDATE claims SET terminal_json=?1 WHERE request_id=?2",
                params![
                    serde_json::to_string(&TerminalOutcome::Success).unwrap(),
                    child.0
                ],
            )
            .unwrap();
        assert!(
            matches!(store.replay_tool_output_operation(&operation),Err(StoreError::ConflictingReplayOutcome {operation: conflicting}) if conflicting==operation)
        );
    }

    #[test]
    fn settlement_refuses_same_hash_different_terminal_before_updating_inherited_claims() {
        let store = Store::memory().unwrap();
        let operation = operation(&store, "atomic-original");
        let output = JobOutput::Cancelled;
        store
            .write_job_output(&operation, ToolKind::Function, &output)
            .unwrap();
        let item = Item::tool_output(&operation.call, ToolKind::Function, &output);
        let child = RequestId("atomic-inherited".into());
        store
            .create_request(&child, Some(&operation.request), "/root/child")
            .unwrap();
        store.lock().execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state) SELECT origin,origin_request_id,call_id,?1,'pending' FROM claims WHERE request_id=?2",params![child.0,operation.request.0]).unwrap();
        assert!(
            matches!(store.write_output(&operation,&item,TerminalOutcome::Success),Err(StoreError::ConflictingReplayOutcome {operation: conflicting}) if conflicting==operation)
        );
        let inherited = store.claims_on(&child).unwrap();
        assert_eq!(inherited[0].state, super::super::ClaimState::Pending);
        assert!(inherited[0].output.is_none());
        let marker: Option<String> = store
            .lock()
            .query_row(
                "SELECT terminal_json FROM claims WHERE request_id=?1",
                [&child.0],
                |row| row.get(0),
            )
            .unwrap();
        assert!(marker.is_none());
        assert_eq!(
            store
                .write_output(&operation, &item, TerminalOutcome::Cancelled)
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .write_output(&operation, &item, TerminalOutcome::Cancelled)
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .replay_tool_output_operation(&operation)
                .unwrap()
                .unwrap()
                .terminal,
            TerminalOutcome::Cancelled
        );
    }
}
