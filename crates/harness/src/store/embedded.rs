use super::*;
use crate::embedding::{
    BindingInitialAuthority, BindingSuccessorAuthority, BindingSuccessorCommit, EmbeddedError,
    HostIdentity,
};

pub(crate) enum CommandInputAdmission {
    New(crate::server::CommandReceipt),
    Retained(crate::server::CommandReceipt),
}

pub(crate) enum EmbeddedInputState {
    Missing,
    Admitted,
    Included(crate::model::RequestId),
}

pub(super) fn matches_binding(c: &Connection, identity: &HostIdentity) -> rusqlite::Result<bool> {
    Ok(c.query_row(
        "SELECT run_id,incarnation FROM embedded_bindings WHERE agent_path=?1",
        [&identity.actor.0],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .optional()?
    .is_some_and(|(run, incarnation)| run == identity.run && incarnation == identity.incarnation))
}

/// ApplicationBound may be fsynced immediately after this owner returns. Confirm
/// the binding's SQLite durability even when an exact retry writes no new rows.
/// Ordinary request/event transactions retain the connection's original mode.
fn durable_binding_transaction<T>(
    connection: &mut Connection,
    action: impl FnOnce(&Transaction<'_>) -> std::result::Result<T, EmbeddedError>,
) -> std::result::Result<T, EmbeddedError> {
    let original: i64 = connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    let result = (|| {
        let tx = connection.transaction()?;
        let result = action(&tx)?;
        tx.commit()?;
        let (busy, pages, checkpointed): (i64, i64, i64) =
            connection.query_row("PRAGMA wal_checkpoint(FULL)", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
        if busy != 0 || pages != checkpointed {
            return Err(EmbeddedError::Binding(
                "binding committed but WAL durability confirmation is unavailable".into(),
            ));
        }
        Ok(result)
    })();
    let restored = connection.pragma_update(None, "synchronous", original);
    match (result, restored) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error.into()),
        (Err(primary), Err(error)) => Err(EmbeddedError::Binding(format!(
            "{primary}; restoring binding durability mode failed: {error}"
        ))),
    }
}

impl Store {
    pub(crate) fn embedded_input_state(
        &self,
        identity: &HostIdentity,
        operation_id: &str,
    ) -> std::result::Result<EmbeddedInputState, EmbeddedError> {
        let connection = self.lock();
        let observation: Option<(i64, Option<i64>, Option<String>)> = connection
            .query_row(
                "SELECT ei.envelope_id,e.id,e.delivered_request FROM embedded_inputs ei \
             LEFT JOIN envelopes e ON e.id=ei.envelope_id AND e.recipient=ei.agent_path \
             WHERE ei.run_id=?1 AND ei.agent_path=?2 AND ei.incarnation=?3 AND ei.operation_id=?4",
                params![
                    identity.run,
                    identity.actor.0,
                    identity.incarnation,
                    operation_id
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let (operation_envelope, envelope, delivered_request) = match observation {
            Some((operation, envelope, request)) => (Some(operation), envelope, request),
            None if matches_binding(&connection, identity)? => {
                return Ok(EmbeddedInputState::Missing);
            }
            None => {
                return Err(EmbeddedError::Binding(
                    "input target has no retained operation or current binding".into(),
                ));
            }
        };
        match (operation_envelope, envelope) {
            (None, None) => Ok(EmbeddedInputState::Missing),
            (Some(_), None) => Err(EmbeddedError::Binding(
                "input operation is not owned by this host".into(),
            )),
            (Some(_), Some(_)) => Ok(match delivered_request {
                None => EmbeddedInputState::Admitted,
                Some(request) => EmbeddedInputState::Included(crate::model::RequestId(request)),
            }),
            (None, Some(_)) => Err(EmbeddedError::Binding(
                "input envelope has no matching host operation".into(),
            )),
        }
    }

    /// Transfer only the existing exact binding. Historical inputs and command
    /// receipts keep their original identities; this does not enqueue work.
    /// FULL commit and WAL checkpoint confirmation precede success, including
    /// exact retries. An error can leave the successor visible; retry this same
    /// authorized transition to confirm durability before publishing attachment.
    pub fn transfer_embedded_binding(
        &self,
        predecessor: &HostIdentity,
        successor: &HostIdentity,
        authority: &dyn BindingSuccessorAuthority,
    ) -> std::result::Result<BindingSuccessorCommit, EmbeddedError> {
        if predecessor.run.is_empty()
            || predecessor.incarnation.is_empty()
            || successor.incarnation.is_empty()
            || predecessor.run != successor.run
            || predecessor.actor != successor.actor
            || predecessor.incarnation == successor.incarnation
        {
            return Err(EmbeddedError::Binding(
                "invalid exact successor binding transition".into(),
            ));
        }
        let mut connection = self.lock();
        durable_binding_transaction(&mut connection, |tx| {
            if !authority
                .validate_successor(predecessor, successor)
                .map_err(EmbeddedError::Host)?
            {
                return Err(EmbeddedError::Binding(
                    "successor binding lacks retained run and journal authority".into(),
                ));
            }
            let outcome = if matches_binding(tx, successor)? {
                BindingSuccessorCommit::AlreadyInstalled
            } else if matches_binding(tx, predecessor)? {
                let changed = tx.execute("UPDATE embedded_bindings SET incarnation=?1 WHERE agent_path=?2 AND run_id=?3 AND incarnation=?4",
                params![successor.incarnation, predecessor.actor.0, predecessor.run, predecessor.incarnation])?;
                if changed != 1 {
                    return Err(EmbeddedError::Binding(
                        "successor binding compare-and-swap lost its exact predecessor".into(),
                    ));
                }
                BindingSuccessorCommit::Installed
            } else {
                return Err(EmbeddedError::Binding(
                    "successor binding conflicts with its exact predecessor".into(),
                ));
            };
            Ok(outcome)
        })
    }

    /// Readback is evidence, not admission or authority to create a host.
    pub fn embedded_binding_matches(
        &self,
        identity: &HostIdentity,
    ) -> std::result::Result<bool, EmbeddedError> {
        matches_binding(&self.lock(), identity).map_err(Into::into)
    }

    /// Observe the logical agent head only under its exact current host binding.
    /// Binding and head share the connection lock, preventing successor races.
    pub fn embedded_agent_head(&self, identity: &HostIdentity) -> Result<Option<RequestId>> {
        let connection = self.lock();
        if !matches_binding(&connection, identity)? {
            return Err(StoreError::InvalidEmbeddedBinding);
        }
        Ok(connection
            .query_row(
                "SELECT head_request FROM agents WHERE path=?1",
                [&identity.actor.0],
                |row| row.get::<_, Option<String>>(0),
            )?
            .map(RequestId))
    }

    pub(crate) fn bind_embedded_actor(
        &self,
        identity: &HostIdentity,
        parent: Option<&AgentPath>,
    ) -> std::result::Result<(), EmbeddedError> {
        self.bind_embedded_actor_authorized(identity, parent, None)
    }

    /// Bind retained startup intent without attaching a Conversation or admitting
    /// live actor work. Exact retries validate authority again and preserve history.
    /// Binding writes use FULL commit and WAL checkpoint confirmation, then restore
    /// the ordinary connection mode. A visible binding after error is insufficient
    /// attachment evidence: repeat this exact authorized call until it succeeds.
    pub fn bind_initial_embedded_binding(
        &self,
        identity: &HostIdentity,
        parent: Option<&AgentPath>,
        authority: &dyn BindingInitialAuthority,
    ) -> std::result::Result<(), EmbeddedError> {
        self.bind_embedded_actor_authorized(identity, parent, Some(authority))
    }

    fn bind_embedded_actor_authorized(
        &self,
        identity: &HostIdentity,
        parent: Option<&AgentPath>,
        authority: Option<&dyn BindingInitialAuthority>,
    ) -> std::result::Result<(), EmbeddedError> {
        if identity.run.is_empty() || identity.incarnation.is_empty() {
            return Err(EmbeddedError::Binding("empty run or incarnation".into()));
        }
        Self::validate_agent_path(&identity.actor.0, parent.map(|p| p.0.as_str()))?;
        let mut connection = self.lock();
        durable_binding_transaction(&mut connection, |tx| {
            if let Some(authority) = authority {
                if !authority
                    .validate_initial_binding(identity)
                    .map_err(EmbeddedError::Host)?
                {
                    return Err(EmbeddedError::Binding(
                        "initial binding lacks retained run and startup authority".into(),
                    ));
                }
            }
            let bound: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM embedded_bindings WHERE agent_path=?1)",
                [&identity.actor.0],
                |r| r.get(0),
            )?;
            if bound {
                if !matches_binding(tx, identity)? {
                    return Err(EmbeddedError::Binding(
                        "actor path belongs to another run/incarnation".into(),
                    ));
                }
            } else {
                let existing: Option<String> = tx
                    .query_row(
                        "SELECT fork_source FROM agents WHERE path=?1",
                        [&identity.actor.0],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(source) = existing {
                    let source: serde_json::Value = serde_json::from_str(&source)?;
                    if source["kind"] != "checkpoint" {
                        return Err(EmbeddedError::Binding(
                            "existing standalone actor cannot become an embedded actor".into(),
                        ));
                    }
                } else {
                    tx.execute("INSERT INTO agents(path,parent_path,head_request,contract,fork_source,state,created_at) VALUES (?1,?2,NULL,'{}',?3,'active',?4)", params![identity.actor.0,parent.map(|p|p.0.as_str()),serde_json::to_string(&serde_json::json!({"kind":"embedded"}))?,utc_millis()])?;
                }
                tx.execute(
                "INSERT INTO embedded_bindings(agent_path,run_id,incarnation) VALUES (?1,?2,?3)",
                params![identity.actor.0, identity.run, identity.incarnation],
            )?;
            }
            Ok(())
        })
    }

    pub(crate) fn admit_embedded_input(
        &self,
        identity: &HostIdentity,
        operation_id: &str,
        sender: &str,
        item: &Item,
    ) -> std::result::Result<i64, EmbeddedError> {
        if operation_id.is_empty() {
            return Err(EmbeddedError::Binding("empty input operation ID".into()));
        }
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let envelope = Self::admit_embedded_input_tx(&tx, identity, operation_id, sender, item)?;
        tx.commit()?;
        Ok(envelope)
    }
    pub(super) fn admit_embedded_input_tx(
        tx: &Transaction<'_>,
        identity: &HostIdentity,
        operation_id: &str,
        sender: &str,
        item: &Item,
    ) -> std::result::Result<i64, EmbeddedError> {
        if !matches_binding(tx, identity)? {
            return Err(EmbeddedError::Binding(
                "input target is not bound to this host".into(),
            ));
        }
        let hash = Self::put_item_tx(tx, item)?;
        let existing: Option<(i64,String,String)> = tx.query_row("SELECT ei.envelope_id,ei.item_hash,e.sender FROM embedded_inputs ei JOIN envelopes e ON e.id=ei.envelope_id WHERE ei.run_id=?1 AND ei.agent_path=?2 AND ei.incarnation=?3 AND ei.operation_id=?4", params![identity.run,identity.actor.0,identity.incarnation,operation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((id, previous_hash, previous_sender)) = existing {
            if hash.0 != previous_hash || sender != previous_sender {
                return Err(EmbeddedError::ConflictingInput);
            }
            return Ok(id);
        }
        tx.execute("INSERT INTO envelopes(sender,recipient,class,item_hash,delivered_request,created_at) VALUES (?1,?2,'user',?3,NULL,?4)", params![sender,identity.actor.0,hash.0,utc_millis()])?;
        let envelope = tx.last_insert_rowid();
        tx.execute("INSERT INTO embedded_inputs(run_id,agent_path,incarnation,operation_id,envelope_id,item_hash) VALUES (?1,?2,?3,?4,?5,?6)", params![identity.run,identity.actor.0,identity.incarnation,operation_id,envelope,hash.0])?;
        Ok(envelope)
    }

    pub(crate) fn admit_embedded_command_input(
        &self,
        identity: &HostIdentity,
        operation: crate::embedding::ClientOperationId,
        text: &str,
    ) -> std::result::Result<CommandInputAdmission, EmbeddedError> {
        use crate::server::{CommandReceipt, CommandReceiptOutcome, HostCommand};
        let mut c = self.lock();
        let tx = c.transaction()?;
        let expected = HostCommand::Input {
            target: identity.clone(),
            text: text.into(),
        };
        let row: Option<(String,String,Option<String>)> = tx.query_row(
            "SELECT command,state,outcome FROM embedded_commands WHERE run_id=?1 AND operation_id=?2",
            params![identity.run,operation.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((command, state, outcome)) = row else {
            return Err(StoreError::InvalidCommandState.into());
        };
        if serde_json::from_str::<HostCommand>(&command)? != expected {
            return Err(StoreError::ConflictingCommand.into());
        }
        if state == "input_admitted" {
            return Ok(CommandInputAdmission::Retained(serde_json::from_str(
                &outcome.ok_or(StoreError::InvalidCommandState)?,
            )?));
        }
        if state != "dispatching" {
            return Err(StoreError::InvalidCommandState.into());
        }
        let item = Item(serde_json::json!({"type":"message","role":"user","content":text}));
        let envelope = Self::admit_embedded_input_tx(
            &tx,
            identity,
            &operation.to_string(),
            "operator",
            &item,
        )?;
        let receipt = CommandReceipt {
            command_id: operation.to_string(),
            outcome: CommandReceiptOutcome::Admitted {
                target: Some(identity.clone()),
                envelope_id: envelope.to_string(),
                wake_error: None,
            },
        };
        tx.execute("UPDATE embedded_commands SET state='input_admitted',envelope_id=?3,outcome=?4 WHERE run_id=?1 AND operation_id=?2 AND state='dispatching'",
            params![identity.run,operation.to_string(),envelope,serde_json::to_string(&receipt)?])?;
        tx.commit()?;
        Ok(CommandInputAdmission::New(receipt))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct InitialAuthority(HostIdentity);
    impl BindingInitialAuthority for InitialAuthority {
        fn validate_initial_binding(
            &self,
            identity: &HostIdentity,
        ) -> std::result::Result<bool, String> {
            Ok(identity == &self.0)
        }
    }

    fn root_identity() -> HostIdentity {
        HostIdentity {
            run: "run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "1".into(),
        }
    }

    #[test]
    fn initial_binding_requires_exact_authority_and_retries_without_live_work() {
        let root =
            std::env::temp_dir().join(format!("harness-binding-initial-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("store.sqlite");
        let store = Store::open(&path).unwrap();
        let identity = root_identity();
        let foreign = HostIdentity {
            run: "foreign".into(),
            ..identity.clone()
        };
        assert!(
            store
                .bind_initial_embedded_binding(&identity, None, &InitialAuthority(foreign.clone()))
                .is_err()
        );
        assert!(!store.embedded_binding_matches(&identity).unwrap());
        store
            .bind_initial_embedded_binding(&identity, None, &InitialAuthority(identity.clone()))
            .unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        store
            .bind_initial_embedded_binding(&identity, None, &InitialAuthority(identity.clone()))
            .unwrap();
        for other in [
            foreign,
            HostIdentity {
                incarnation: "2".into(),
                ..identity.clone()
            },
        ] {
            assert!(
                store
                    .bind_initial_embedded_binding(&other, None, &InitialAuthority(other.clone()))
                    .is_err()
            );
        }
        assert!(
            store
                .bind_initial_embedded_binding(
                    &identity,
                    None,
                    &InitialAuthority(HostIdentity {
                        incarnation: "revoked".into(),
                        ..identity.clone()
                    })
                )
                .is_err()
        );
        assert!(store.embedded_binding_matches(&identity).unwrap());
        let c = store.lock();
        for table in ["requests", "envelopes", "embedded_commands"] {
            let count: i64 = c
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "binding created live work in {table}");
        }
        drop(c);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
        let standalone = Store::memory().unwrap();
        standalone.lock().execute("INSERT INTO agents(path,parent_path,contract,fork_source,state,created_at) VALUES('/root',NULL,'{}','{}','active',0)",[]).unwrap();
        assert!(
            standalone
                .bind_initial_embedded_binding(&identity, None, &InitialAuthority(identity.clone()))
                .is_err()
        );
        assert!(!standalone.embedded_binding_matches(&identity).unwrap());
    }

    #[test]
    fn exact_head_read_refuses_stale_binding_and_preserves_retained_history() {
        let store = Store::memory().unwrap();
        let identity = root_identity();
        store.bind_embedded_actor(&identity, None).unwrap();
        assert_eq!(store.embedded_agent_head(&identity).unwrap(), None);
        let head = RequestId("old-head".into());
        store
            .create_request(&head, None, &identity.actor.0)
            .unwrap();
        store
            .append_items(
                &head,
                &[Item(
                    serde_json::json!({"type":"message","role":"assistant","content":"old worker"}),
                )],
            )
            .unwrap();
        assert!(
            store
                .advance_agent_head(&identity.actor, None, Some(&head))
                .unwrap()
        );
        assert_eq!(
            store.embedded_agent_head(&identity).unwrap(),
            Some(head.clone())
        );
        let successor = HostIdentity {
            incarnation: "2".into(),
            ..identity.clone()
        };
        store
            .transfer_embedded_binding(&identity, &successor, &TransferAuthority(true))
            .unwrap();
        let new_head = RequestId("new-head".into());
        store
            .create_request(&new_head, Some(&head), &identity.actor.0)
            .unwrap();
        assert!(
            store
                .advance_agent_head(&identity.actor, Some(&head), Some(&new_head))
                .unwrap()
        );
        assert!(matches!(
            store.embedded_agent_head(&identity),
            Err(StoreError::InvalidEmbeddedBinding)
        ));
        assert_eq!(
            store.embedded_agent_head(&successor).unwrap(),
            Some(new_head)
        );
        assert_eq!(store.history_page(&head, 0, 100).unwrap().items.len(), 1);
        let foreign = HostIdentity {
            run: "foreign".into(),
            ..successor
        };
        assert!(matches!(
            store.embedded_agent_head(&foreign),
            Err(StoreError::InvalidEmbeddedBinding)
        ));
    }

    #[test]
    fn binding_durability_mode_restores_after_success_refusal_and_sql_failure() {
        let store = Store::memory().unwrap();
        let mut c = store.lock();
        for original in [1, 2] {
            c.pragma_update(None, "synchronous", original).unwrap();
            for failure in [0, 1, 2] {
                let result = durable_binding_transaction(&mut c, |tx| {
                    let current: i64 =
                        tx.pragma_query_value(None, "synchronous", |row| row.get(0))?;
                    assert_eq!(current, 2);
                    match failure {
                        0 => Ok(()),
                        1 => Err(EmbeddedError::Binding("authority refused".into())),
                        _ => {
                            tx.execute("INSERT INTO missing_binding_table VALUES(1)", [])?;
                            Ok(())
                        }
                    }
                });
                assert_eq!(result.is_ok(), failure == 0);
                let restored: i64 = c
                    .pragma_query_value(None, "synchronous", |row| row.get(0))
                    .unwrap();
                assert_eq!(restored, original);
            }
        }
    }

    #[test]
    fn busy_binding_confirmation_requires_exact_retry_and_restores_normal_mode() {
        let root =
            std::env::temp_dir().join(format!("harness-binding-busy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("store.sqlite");
        let store = Store::open(&path).unwrap();
        store
            .lock()
            .busy_timeout(std::time::Duration::ZERO)
            .unwrap();
        let old = root_identity();
        let new = HostIdentity {
            incarnation: "2".into(),
            ..old.clone()
        };
        store
            .bind_initial_embedded_binding(&old, None, &InitialAuthority(old.clone()))
            .unwrap();
        let reader = Connection::open(&path).unwrap();
        reader
            .execute_batch("BEGIN; SELECT COUNT(*) FROM embedded_bindings;")
            .unwrap();
        assert!(
            store
                .transfer_embedded_binding(&old, &new, &TransferAuthority(true))
                .is_err()
        );
        assert!(store.embedded_binding_matches(&new).unwrap());
        let mode: i64 = store
            .lock()
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, 1);
        drop(reader);
        assert_eq!(
            store
                .transfer_embedded_binding(&old, &new, &TransferAuthority(true))
                .unwrap(),
            BindingSuccessorCommit::AlreadyInstalled
        );
        assert_eq!(
            store
                .lock()
                .pragma_query_value(None, "synchronous", |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        assert!(store.embedded_binding_matches(&new).unwrap());
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    struct TransferAuthority(bool);
    impl BindingSuccessorAuthority for TransferAuthority {
        fn validate_successor(
            &self,
            _: &HostIdentity,
            _: &HostIdentity,
        ) -> std::result::Result<bool, String> {
            Ok(self.0)
        }
    }

    #[test]
    fn successor_binding_is_exact_idempotent_and_preserves_historical_observation() {
        let root = std::env::temp_dir().join(format!(
            "harness-binding-successor-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("store.sqlite");
        let store = Store::open(&path).unwrap();
        let old = HostIdentity {
            run: "run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "1".into(),
        };
        let new = HostIdentity {
            incarnation: "2".into(),
            ..old.clone()
        };
        store.bind_embedded_actor(&old, None).unwrap();
        let item = Item(serde_json::json!({"type":"message","role":"user","content":[]}));
        let envelope = store
            .admit_embedded_input(&old, "operation", "operator", &item)
            .unwrap();
        assert!(
            store
                .transfer_embedded_binding(&old, &new, &TransferAuthority(false))
                .is_err()
        );
        assert!(store.embedded_binding_matches(&old).unwrap());
        assert_eq!(
            store
                .transfer_embedded_binding(&old, &new, &TransferAuthority(true))
                .unwrap(),
            BindingSuccessorCommit::Installed
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .transfer_embedded_binding(&old, &new, &TransferAuthority(true))
                .unwrap(),
            BindingSuccessorCommit::AlreadyInstalled
        );
        assert!(matches!(
            store.embedded_input_state(&old, "operation").unwrap(),
            EmbeddedInputState::Admitted
        ));
        assert!(
            store
                .admit_embedded_input(&old, "later", "operator", &item)
                .is_err()
        );
        assert!(
            store
                .transfer_embedded_binding(
                    &old,
                    &HostIdentity {
                        incarnation: "3".into(),
                        ..new.clone()
                    },
                    &TransferAuthority(true)
                )
                .is_err()
        );
        assert!(
            store
                .transfer_embedded_binding(
                    &old,
                    &HostIdentity {
                        run: "other".into(),
                        ..new.clone()
                    },
                    &TransferAuthority(true)
                )
                .is_err()
        );
        let connection = store.lock();
        connection
            .execute(
                "INSERT INTO requests(id,branch) VALUES ('retained-request','main')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE envelopes SET delivered_request='retained-request' WHERE id=?1",
                [envelope],
            )
            .unwrap();
        drop(connection);
        assert!(
            matches!(store.embedded_input_state(&old, "operation").unwrap(), EmbeddedInputState::Included(request) if request.0 == "retained-request")
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    use crate::{item::Item, model::AgentPath};
    use serde_json::json;

    #[test]
    fn input_identity_dedupe_is_exact_run_incarnation_and_operation() {
        let store = Store::memory().unwrap();
        let first = HostIdentity {
            run: "old-run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "old".into(),
        };
        store.bind_embedded_actor(&first, None).unwrap();
        let item = Item(json!({"type":"message","role":"user","content":"same text"}));
        let first_envelope = store
            .admit_embedded_input(&first, "same-operation", "operator", &item)
            .unwrap();
        // Model a deliberately authorized binding transition in the fixture.
        // Input history remains immutable and cannot become a new admission.
        store.lock().execute("UPDATE embedded_bindings SET run_id='new-run',incarnation='new' WHERE agent_path='/root'",[]).unwrap();
        let second = HostIdentity {
            run: "new-run".into(),
            actor: first.actor.clone(),
            incarnation: "new".into(),
        };
        assert!(matches!(
            store
                .embedded_input_state(&second, "same-operation")
                .unwrap(),
            EmbeddedInputState::Missing
        ));
        let second_envelope = store
            .admit_embedded_input(&second, "same-operation", "operator", &item)
            .unwrap();
        assert_ne!(first_envelope, second_envelope);
        assert_eq!(
            store
                .admit_embedded_input(&second, "same-operation", "operator", &item)
                .unwrap(),
            second_envelope
        );
        assert_eq!(store.unread("/root").unwrap().len(), 2);
        store
            .lock()
            .execute(
                "UPDATE embedded_bindings SET incarnation='third' WHERE agent_path='/root'",
                [],
            )
            .unwrap();
        let third = HostIdentity {
            incarnation: "third".into(),
            ..second
        };
        let third_envelope = store
            .admit_embedded_input(&third, "same-operation", "operator", &item)
            .unwrap();
        assert_ne!(second_envelope, third_envelope);
    }

    #[test]
    fn schema_six_input_migration_preserves_exact_binding_and_envelope() {
        let mut c = Connection::open_in_memory().unwrap();
        crate::store::schema::initialize(&mut c).unwrap();
        c.execute_batch("DROP TABLE embedded_commands; DROP TABLE embedded_inputs; CREATE TABLE embedded_inputs(agent_path TEXT NOT NULL REFERENCES embedded_bindings(agent_path),operation_id TEXT NOT NULL,envelope_id INTEGER NOT NULL REFERENCES envelopes(id),item_hash TEXT NOT NULL REFERENCES items(hash),PRIMARY KEY(agent_path,operation_id)); UPDATE schema_version SET version=6; INSERT INTO agents(path,parent_path,contract,fork_source,state,created_at) VALUES('/root',NULL,'{}','{}','active',17); INSERT INTO embedded_bindings VALUES('/root','legacy-run','legacy-incarnation'); INSERT INTO items VALUES('legacy-item','{}'); INSERT INTO envelopes(id,sender,recipient,class,item_hash,created_at) VALUES(7,'operator','/root','user','legacy-item',19); INSERT INTO embedded_inputs VALUES('/root','legacy-operation',7,'legacy-item');").unwrap();
        crate::store::schema::initialize(&mut c).unwrap();
        let row: (String, String, String, i64) = c
            .query_row(
                "SELECT run_id,incarnation,operation_id,envelope_id FROM embedded_inputs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "legacy-run".into(),
                "legacy-incarnation".into(),
                "legacy-operation".into(),
                7
            )
        );
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM embedded_commands", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            c.query_row("SELECT created_at FROM envelopes WHERE id=7", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            19
        );
    }

    #[test]
    fn embedded_input_state_requires_exact_incarnation_and_distinguishes_missing() {
        let store = Store::memory().unwrap();
        let identity = HostIdentity {
            run: "run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "first".into(),
        };
        store.bind_embedded_actor(&identity, None).unwrap();
        assert!(matches!(
            store.embedded_input_state(&identity, "absent").unwrap(),
            EmbeddedInputState::Missing
        ));

        store
            .admit_embedded_input(
                &identity,
                "operation-1",
                "operator",
                &Item(json!({"type":"message","role":"user","content":"hello"})),
            )
            .unwrap();
        assert!(matches!(
            store
                .embedded_input_state(&identity, "operation-1")
                .unwrap(),
            EmbeddedInputState::Admitted
        ));

        let replacement = HostIdentity {
            incarnation: "replacement".into(),
            ..identity.clone()
        };
        assert!(matches!(
            store.embedded_input_state(&replacement, "operation-1"),
            Err(EmbeddedError::Binding(_))
        ));

        store
            .lock()
            .execute(
                "UPDATE envelopes SET recipient='/root/other' \
                 WHERE id=(SELECT envelope_id FROM embedded_inputs \
                           WHERE agent_path='/root' AND operation_id='operation-1')",
                [],
            )
            .unwrap();
        assert!(matches!(
            store.embedded_input_state(&identity, "operation-1"),
            Err(EmbeddedError::Binding(_))
        ));
    }
}
