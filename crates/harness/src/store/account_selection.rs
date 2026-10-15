//! Run-wide account selection and its terminal browser receipt commit together.
use super::*;
use crate::{
    embedding::ClientOperationId,
    server::{CommandReceipt, CommandReceiptOutcome, HostCommand},
    transport::auth::chatgpt::{SavedAccountId, SavedAccounts},
};

fn key(run: &str) -> String {
    format!("harness:run-account:{run}")
}
fn read(c: &Connection, run: &str) -> Result<Option<SavedAccountId>> {
    let state: Option<String> = c
        .query_row(
            "SELECT state FROM session_state WHERE session_id=?1",
            [key(run)],
            |r| r.get(0),
        )
        .optional()?;
    state
        .map(|state| serde_json::from_str(&state).map_err(Into::into))
        .transpose()
}
impl Store {
    pub fn initialize_run_account(
        &self,
        run: &str,
        launch: &SavedAccountId,
    ) -> Result<SavedAccountId> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            tx.execute("INSERT OR IGNORE INTO session_state(session_id,state,updated_at) VALUES (?1,?2,?3)",params![key(run),serde_json::to_string(launch)?,utc_millis()])?;
            read(tx, run)?.ok_or(StoreError::InvalidCommandState)
        })
    }
    pub fn run_account(&self, run: &str) -> Result<Option<SavedAccountId>> {
        read(&*self.lock()?, run)
    }

    /// Caller verifies the exact live root identity through host authority. The
    /// inventory owner supplies membership, never a browser-controlled path.
    pub fn select_run_account(
        &self,
        run: &str,
        operation: ClientOperationId,
        accounts: &SavedAccounts,
    ) -> Result<CommandReceipt> {
        // Validate current credential bytes before acquiring the Store writer.
        // No refresh or network work is done by this observation.
        let retained = self
            .embedded_command(run, operation)?
            .ok_or(StoreError::InvalidCommandState)?;
        let HostCommand::SelectAccount { account_id, .. } = &retained.command else {
            return Err(StoreError::InvalidCommandState);
        };
        let available = accounts.is_available(account_id);
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let record = super::embedded_commands::read(tx, run, operation)?.ok_or(StoreError::InvalidCommandState)?;
            let HostCommand::SelectAccount { target, account_id } = &record.command else { return Err(StoreError::InvalidCommandState); };
            if let Some(receipt) = record.receipt { return Ok(receipt); }
            if target.run != run || record.state != super::EmbeddedCommandState::Dispatching { return Err(StoreError::InvalidCommandState); }
            let (state, outcome) = if available && accounts.contains(account_id) && read(tx, run)?.is_some() {
                tx.execute("UPDATE session_state SET state=?2,updated_at=?3 WHERE session_id=?1",params![key(run),serde_json::to_string(account_id)?,utc_millis()])?;
                ("account_selected", CommandReceiptOutcome::AccountSelected { target: target.clone(), account_id: account_id.clone() })
            } else {
                ("refused", CommandReceiptOutcome::Refused { target: Some(target.clone()), reason: "Saved account is unavailable in this run.".into() })
            };
            let receipt = CommandReceipt { command_id: operation.to_string(), outcome };
            if tx.execute("UPDATE embedded_commands SET state=?3,outcome=?4 WHERE run_id=?1 AND operation_id=?2 AND state='dispatching'",params![run,operation.to_string(),state,serde_json::to_string(&receipt)?])? != 1 { return Err(StoreError::InvalidCommandState); }
            Ok(receipt)
        })
    }
}
