use super::*;
use crate::{
    embedding::ClientOperationId,
    server::{CommandReceipt, CommandReceiptOutcome, HostCommand},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddedCommandState {
    Queued,
    Dispatching,
    InputAdmitted,
    ControlRequested,
    Refused,
    Unconfirmed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedCommandRecord {
    pub operation_id: ClientOperationId,
    pub command: HostCommand,
    pub state: EmbeddedCommandState,
    pub envelope_id: Option<i64>,
    pub receipt: Option<CommandReceipt>,
}

fn read(
    c: &Connection,
    run: &str,
    operation: ClientOperationId,
) -> Result<Option<EmbeddedCommandRecord>> {
    let row: Option<(String,String,Option<i64>,Option<String>)> = c.query_row(
        "SELECT command,state,envelope_id,outcome FROM embedded_commands WHERE run_id=?1 AND operation_id=?2",
        params![run,operation.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    row.map(|(command, state, envelope_id, outcome)| {
        Ok(EmbeddedCommandRecord {
            operation_id: operation,
            command: serde_json::from_str(&command)?,
            state: serde_json::from_str(&format!("\"{state}\""))?,
            envelope_id,
            receipt: outcome.map(|v| serde_json::from_str(&v)).transpose()?,
        })
    })
    .transpose()
}

pub(super) fn recover_claims(c: &Connection) -> Result<()> {
    let claimed = {
        let mut stmt = c.prepare(
            "SELECT run_id,operation_id FROM embedded_commands WHERE state='dispatching'",
        )?;
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (run, id) in claimed {
        let operation: ClientOperationId = serde_json::from_value(serde_json::Value::String(id))?;
        let record = read(c, &run, operation)?.ok_or(StoreError::InvalidCommandState)?;
        let receipt = CommandReceipt {
            command_id: operation.to_string(),
            outcome: CommandReceiptOutcome::Unconfirmed {
                target: record.command.target().clone(),
                reason: "Host process lost after command claim; dispatch outcome is unknown."
                    .into(),
            },
        };
        c.execute("UPDATE embedded_commands SET state='unconfirmed',outcome=?3 WHERE run_id=?1 AND operation_id=?2 AND state='dispatching'",
            params![run,operation.to_string(),serde_json::to_string(&receipt)?])?;
    }
    Ok(())
}

impl Store {
    pub(crate) fn record_embedded_command_wake_error(
        &self,
        run: &str,
        operation: ClientOperationId,
        error: String,
    ) -> Result<CommandReceipt> {
        let c = self.lock();
        let mut record = read(&c, run, operation)?.ok_or(StoreError::InvalidCommandState)?;
        if record.state != EmbeddedCommandState::InputAdmitted {
            return Err(StoreError::InvalidCommandState);
        }
        let mut receipt = record
            .receipt
            .take()
            .ok_or(StoreError::InvalidCommandState)?;
        let CommandReceiptOutcome::Admitted { wake_error, .. } = &mut receipt.outcome else {
            return Err(StoreError::InvalidCommandState);
        };
        *wake_error = Some(error);
        c.execute("UPDATE embedded_commands SET outcome=?3 WHERE run_id=?1 AND operation_id=?2 AND state='input_admitted'",params![run,operation.to_string(),serde_json::to_string(&receipt)?])?;
        Ok(receipt)
    }

    /// Retain exact command contents before transport acceptance. Duplicate IDs
    /// read the retained record; a conflicting reuse never changes it.
    pub fn enqueue_embedded_command(
        &self,
        operation: ClientOperationId,
        command: &HostCommand,
    ) -> Result<EmbeddedCommandRecord> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let run = &command.target().run;
        if let Some(existing) = read(&tx, run, operation)? {
            if existing.command != *command {
                return Err(StoreError::ConflictingCommand);
            }
            return Ok(existing);
        }
        let (action, payload, round) = match command {
            HostCommand::Input { text, .. } => ("input", text.as_str(), None),
            HostCommand::Interrupt { expected_round, .. } => {
                ("interrupt", "", Some(expected_round.to_string()))
            }
            HostCommand::Retire { .. } => ("retire", "", None),
        };
        tx.execute("INSERT INTO embedded_commands(run_id,operation_id,target,action,payload,expected_round,command,state,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,'queued',?8)",
            params![run,operation.to_string(),serde_json::to_string(command.target())?,action,payload,round,serde_json::to_string(command)?,utc_millis()])?;
        let record = read(&tx, run, operation)?.ok_or(StoreError::InvalidCommandState)?;
        tx.commit()?;
        Ok(record)
    }

    pub fn embedded_command(
        &self,
        run: &str,
        operation: ClientOperationId,
    ) -> Result<Option<EmbeddedCommandRecord>> {
        read(&self.lock(), run, operation)
    }

    /// The existing host command loop drains this list at startup and after a
    /// wake. Channel notifications are hints; durable queued rows are the work.
    pub fn queued_embedded_commands(&self, run: &str) -> Result<Vec<EmbeddedCommandRecord>> {
        let c = self.lock();
        let mut stmt = c.prepare("SELECT operation_id FROM embedded_commands WHERE run_id=?1 AND state='queued' ORDER BY created_at,operation_id")?;
        let ids = stmt
            .query_map([run], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.into_iter()
            .map(|id| {
                let operation = serde_json::from_value(serde_json::Value::String(id))?;
                read(&c, run, operation)?.ok_or(StoreError::InvalidCommandState)
            })
            .collect()
    }

    /// Once-only claim before calling any live host capability.
    pub fn claim_embedded_command(
        &self,
        run: &str,
        operation: ClientOperationId,
    ) -> Result<Option<EmbeddedCommandRecord>> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        if tx.execute("UPDATE embedded_commands SET state='dispatching' WHERE run_id=?1 AND operation_id=?2 AND state='queued'",params![run,operation.to_string()])? == 0 {
            return Ok(None);
        }
        let record = read(&tx, run, operation)?.ok_or(StoreError::InvalidCommandState)?;
        tx.commit()?;
        Ok(Some(record))
    }

    /// Retain a host control/refusal/uncertainty observation. Input admission
    /// uses its own transaction so the envelope and outcome cannot diverge.
    pub fn settle_embedded_command(
        &self,
        run: &str,
        operation: ClientOperationId,
        outcome: CommandReceiptOutcome,
    ) -> Result<CommandReceipt> {
        let state = match &outcome {
            CommandReceiptOutcome::ControlRequested { .. } => "control_requested",
            CommandReceiptOutcome::Refused { .. } => "refused",
            CommandReceiptOutcome::Unconfirmed { .. } => "unconfirmed",
            CommandReceiptOutcome::Admitted { .. } => return Err(StoreError::InvalidCommandState),
        };
        let receipt = CommandReceipt {
            command_id: operation.to_string(),
            outcome,
        };
        let c = self.lock();
        let record = read(&c, run, operation)?.ok_or(StoreError::InvalidCommandState)?;
        if let Some(previous) = record.receipt {
            return if previous == receipt {
                Ok(previous)
            } else {
                Err(StoreError::InvalidCommandState)
            };
        }
        let valid = match &receipt.outcome {
            CommandReceiptOutcome::ControlRequested { target, control } => {
                target == record.command.target()
                    && matches!(
                        (&record.command, control),
                        (
                            HostCommand::Retire { .. },
                            crate::server::CommandControl::Retire
                        ) | (
                            HostCommand::Interrupt { .. },
                            crate::server::CommandControl::Interrupt
                        )
                    )
            }
            CommandReceiptOutcome::Refused { target, .. } => {
                target.as_ref().is_none_or(|t| t == record.command.target())
            }
            CommandReceiptOutcome::Unconfirmed { target, .. } => target == record.command.target(),
            _ => false,
        };
        if !valid || record.state != EmbeddedCommandState::Dispatching {
            return Err(StoreError::InvalidCommandState);
        }
        if c.execute("UPDATE embedded_commands SET state=?3,outcome=?4 WHERE run_id=?1 AND operation_id=?2 AND state='dispatching'",params![run,operation.to_string(),state,serde_json::to_string(&receipt)?])? != 1 { return Err(StoreError::InvalidCommandState); }
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        embedding::{EmbeddedRoundId, HostIdentity},
        model::AgentPath,
        server::CommandControl,
    };
    fn identity() -> HostIdentity {
        HostIdentity {
            run: "run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "first".into(),
        }
    }
    fn operation() -> ClientOperationId {
        ClientOperationId(uuid::Uuid::new_v4())
    }
    fn input(text: &str) -> HostCommand {
        HostCommand::Input {
            target: identity(),
            text: text.into(),
        }
    }

    #[test]
    fn command_duplicate_conflict_and_once_only_claim() {
        let store = Arc::new(Store::memory().unwrap());
        let id = operation();
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store.enqueue_embedded_command(id, &input("héllo")).unwrap()
                })
            })
            .collect();
        for worker in workers {
            assert_eq!(worker.join().unwrap().state, EmbeddedCommandState::Queued);
        }
        assert_eq!(store.queued_embedded_commands("run").unwrap().len(), 1);
        assert!(matches!(
            store.enqueue_embedded_command(id, &input("different")),
            Err(StoreError::ConflictingCommand)
        ));
        let mut foreign = identity();
        foreign.incarnation = "replacement".into();
        assert!(matches!(
            store.enqueue_embedded_command(id, &HostCommand::Retire { target: foreign }),
            Err(StoreError::ConflictingCommand)
        ));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || {
                    store.claim_embedded_command("run", id).unwrap().is_some()
                })
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .filter(|w| w.thread().id() != std::thread::current().id())
                .map(|w| w.join().unwrap() as usize)
                .sum::<usize>(),
            1
        );
    }

    #[test]
    fn input_envelope_and_admission_outcome_commit_together() {
        let store = Store::memory().unwrap();
        store.bind_embedded_actor(&identity(), None).unwrap();
        let id = operation();
        store.enqueue_embedded_command(id, &input("hello")).unwrap();
        assert!(
            store
                .admit_embedded_command_input(&identity(), id, "hello")
                .is_err()
        );
        assert!(store.unread("/root").unwrap().is_empty());
        store.claim_embedded_command("run", id).unwrap().unwrap();
        let first = store
            .admit_embedded_command_input(&identity(), id, "hello")
            .unwrap();
        assert_eq!(
            first,
            store
                .admit_embedded_command_input(&identity(), id, "hello")
                .unwrap()
        );
        assert_eq!(store.unread("/root").unwrap().len(), 1);
        let record = store.embedded_command("run", id).unwrap().unwrap();
        assert_eq!(record.state, EmbeddedCommandState::InputAdmitted);
        assert_eq!(record.receipt, Some(first));
        assert!(record.envelope_id.is_some());
        assert!(
            store
                .admit_embedded_command_input(&identity(), id, "changed")
                .is_err()
        );
    }

    #[test]
    fn reopen_preserves_queued_work_and_marks_claimed_control_unconfirmed() {
        let path =
            std::env::temp_dir().join(format!("harness-commands-{}.sqlite", uuid::Uuid::new_v4()));
        let queued = operation();
        let claimed = operation();
        let settled = operation();
        let command = HostCommand::Interrupt {
            target: identity(),
            expected_round: EmbeddedRoundId(uuid::Uuid::new_v4()),
        };
        {
            let store = Store::open(&path).unwrap();
            store
                .enqueue_embedded_command(queued, &input("gap"))
                .unwrap();
            store.enqueue_embedded_command(claimed, &command).unwrap();
            store
                .claim_embedded_command("run", claimed)
                .unwrap()
                .unwrap();
            store.enqueue_embedded_command(settled, &command).unwrap();
            store
                .claim_embedded_command("run", settled)
                .unwrap()
                .unwrap();
            store
                .settle_embedded_command(
                    "run",
                    settled,
                    CommandReceiptOutcome::ControlRequested {
                        target: identity(),
                        control: CommandControl::Interrupt,
                    },
                )
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .queued_embedded_commands("run")
                .unwrap()
                .iter()
                .map(|r| r.operation_id)
                .collect::<Vec<_>>(),
            vec![queued]
        );
        let record = store.embedded_command("run", claimed).unwrap().unwrap();
        assert_eq!(record.command, command);
        assert_eq!(record.state, EmbeddedCommandState::Unconfirmed);
        assert!(matches!(
            record.receipt.unwrap().outcome,
            CommandReceiptOutcome::Unconfirmed { .. }
        ));
        assert!(
            store
                .claim_embedded_command("run", claimed)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .embedded_command("run", settled)
                .unwrap()
                .unwrap()
                .state,
            EmbeddedCommandState::ControlRequested
        );
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn schema_six_migration_preserves_history_without_fabricating_commands() {
        let mut c = Connection::open_in_memory().unwrap();
        schema::initialize(&mut c).unwrap();
        c.execute_batch("DROP TABLE embedded_commands; UPDATE schema_version SET version=6; INSERT INTO requests(id,branch,created_at) VALUES('old','main',17);").unwrap();
        schema::initialize(&mut c).unwrap();
        assert_eq!(
            c.query_row("SELECT version FROM schema_version", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            7
        );
        assert_eq!(
            c.query_row("SELECT created_at FROM requests WHERE id='old'", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            17
        );
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM embedded_commands", [], |r| r
                .get::<_, u32>(0))
                .unwrap(),
            0
        );
    }
}
