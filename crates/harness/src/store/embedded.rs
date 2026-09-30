use super::*;
use crate::embedding::{EmbeddedError, HostIdentity};

pub(crate) enum EmbeddedInputState {
    Missing,
    Admitted,
    Included(crate::model::RequestId),
}

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
    pub(crate) fn embedded_input_state(
        &self,
        identity: &HostIdentity,
        operation_id: &str,
    ) -> std::result::Result<EmbeddedInputState, EmbeddedError> {
        let connection = self.lock();
        let binding: Option<(String, String, Option<i64>, Option<i64>, Option<String>)> =
            connection
                .query_row(
                    "SELECT b.run_id,b.incarnation,ei.envelope_id,e.id,e.delivered_request \
                 FROM embedded_bindings b \
                 LEFT JOIN embedded_inputs ei \
                   ON ei.agent_path=b.agent_path AND ei.operation_id=?2 \
                 LEFT JOIN envelopes e \
                   ON e.id=ei.envelope_id AND e.recipient=b.agent_path \
                 WHERE b.agent_path=?1",
                    params![identity.actor.0, operation_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .optional()?;
        let Some((run, incarnation, operation_envelope, envelope, delivered_request)) = binding
        else {
            return Err(EmbeddedError::Binding(
                "input target is not bound to this host".into(),
            ));
        };
        if run != identity.run || incarnation != identity.incarnation {
            return Err(EmbeddedError::Binding(
                "input target is not bound to this host".into(),
            ));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{item::Item, model::AgentPath};
    use serde_json::json;

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
