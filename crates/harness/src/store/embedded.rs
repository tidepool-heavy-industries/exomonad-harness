use super::*;
use crate::embedding::{EmbeddedError, HostIdentity};

fn matches_binding(
    c: &Connection,
    identity: &HostIdentity,
) -> std::result::Result<bool, EmbeddedError> {
    Ok(c.query_row(
        "SELECT run_id,incarnation FROM embedded_bindings WHERE agent_path=?1",
        [&identity.actor.0],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    )
    .optional()?
    .is_some_and(|(run, incarnation)| run == identity.run && incarnation == identity.incarnation))
}

impl Store {
    pub(crate) fn bind_embedded_actor(
        &self,
        identity: &HostIdentity,
        parent: Option<&AgentPath>,
    ) -> std::result::Result<(), EmbeddedError> {
        if identity.run.is_empty() || identity.incarnation.is_empty() {
            return Err(EmbeddedError::Binding("empty run or incarnation".into()));
        }
        Self::validate_agent_path(&identity.actor.0, parent.map(|p| p.0.as_str()))?;
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let bound: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM embedded_bindings WHERE agent_path=?1)",
            [&identity.actor.0],
            |r| r.get(0),
        )?;
        if bound {
            if !matches_binding(&tx, identity)? {
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
        tx.commit()?;
        Ok(())
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
        if !matches_binding(&tx, identity)? {
            return Err(EmbeddedError::Binding(
                "input target is not bound to this host".into(),
            ));
        }
        let hash = Self::put_item_tx(&tx, item)?;
        let existing: Option<(i64,String,String)> = tx.query_row("SELECT ei.envelope_id,ei.item_hash,e.sender FROM embedded_inputs ei JOIN envelopes e ON e.id=ei.envelope_id WHERE ei.agent_path=?1 AND ei.operation_id=?2", params![identity.actor.0,operation_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some((id, previous_hash, previous_sender)) = existing {
            if hash.0 != previous_hash || sender != previous_sender {
                return Err(EmbeddedError::ConflictingInput);
            }
            return Ok(id);
        }
        tx.execute("INSERT INTO envelopes(sender,recipient,class,item_hash,delivered_request,created_at) VALUES (?1,?2,'user',?3,NULL,?4)", params![sender,identity.actor.0,hash.0,utc_millis()])?;
        let envelope = tx.last_insert_rowid();
        tx.execute("INSERT INTO embedded_inputs(agent_path,operation_id,envelope_id,item_hash) VALUES (?1,?2,?3,?4)", params![identity.actor.0,operation_id,envelope,hash.0])?;
        tx.commit()?;
        Ok(envelope)
    }
}
