//! Claim settlement owns terminal state independently of rendered tool payloads.
use super::{Result, Store, StoreError};
use crate::{
    item::{Item, ItemHash},
    model::OperationId,
    provider::ToolFailure,
    turn::JobOutput,
};
use rusqlite::{Connection, params};
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

fn exact_terminal(
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

impl Store {
    /// Exact original operation; inherited claimants must retain identical evidence.
    pub fn replay_tool_output_operation(
        &self,
        operation: &OperationId,
    ) -> Result<Option<RecordedToolOutput>> {
        let terminal = exact_terminal(&self.lock(), operation)?;
        let Some((hash, terminal)) = terminal else {
            return Ok(None);
        };
        let item = self
            .replay_output_operation(operation)?
            .ok_or_else(|| StoreError::MissingReplayItem(hash.0))?;
        Ok(Some(RecordedToolOutput { item, terminal }))
    }

    pub(crate) fn has_completed_output(&self, operation: &OperationId) -> Result<bool> {
        Ok(
            exact_terminal(&self.lock(), operation)?.is_some_and(|(_, outcome)| {
                matches!(
                    outcome,
                    TerminalOutcome::Success | TerminalOutcome::Failure(_)
                )
            }),
        )
    }
}
