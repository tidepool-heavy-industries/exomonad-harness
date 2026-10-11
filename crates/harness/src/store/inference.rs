//! The Store owns inference admission independently of actor liveness and inboxes.
use super::{Connection, Result, Store, StoreError, embedded, embedded_commands, embedded_round};
use crate::{
    embedding::{ClientOperationId, HostIdentity, HostInferenceState, InferenceBlock, InferenceBlockReason},
    model::{OperationId, RequestId},
    server::{CommandReceipt, CommandReceiptOutcome, HostCommand},
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Gate {
    Idle,
    Running { incarnation: String, attempt: String },
    Blocked { block: InferenceBlock },
    Recovering {
        incarnation: String,
        operation: ClientOperationId,
        block: InferenceBlock,
        resume_head: RequestId,
    },
}

pub(crate) struct InferenceAttempt {
    identity: HostIdentity,
    attempt: String,
    pub(crate) recovering: bool,
}

fn read(c: &Connection, identity: &HostIdentity) -> Result<Option<(u64, Gate)>> {
    c.query_row(
        "SELECT generation,state FROM embedded_inference WHERE run_id=?1 AND agent_path=?2",
        params![identity.run, identity.actor.0],
        |r| Ok((r.get::<_, u64>(0)?, r.get::<_, String>(1)?)),
    ).optional()?.map(|(generation, state)| Ok((generation, serde_json::from_str(&state)?))).transpose()
}

fn write(tx: &Transaction<'_>, identity: &HostIdentity, generation: u64, state: &Gate) -> Result<()> {
    tx.execute(
        "INSERT INTO embedded_inference(run_id,agent_path,generation,state) VALUES (?1,?2,?3,?4) \
         ON CONFLICT(run_id,agent_path) DO UPDATE SET generation=excluded.generation,state=excluded.state",
        params![identity.run, identity.actor.0, generation, serde_json::to_string(state)?],
    )?;
    Ok(())
}

fn initialize(tx: &Transaction<'_>, identity: &HostIdentity) -> Result<(u64, Gate)> {
    if !embedded::matches_binding(tx, identity)? {
        return Err(StoreError::InvalidEmbeddedBinding);
    }
    if let Some(state) = read(tx, identity)? {
        return Ok(state);
    }
    let frontier = embedded_round::frontier(tx, identity)?;
    let (generation, state) = match frontier.pending_head {
        Some(request) => (1, Gate::Blocked { block: InferenceBlock {
            request,
            generation: 1,
            reason: frontier.pending_interruption.map_or(InferenceBlockReason::ContinuityUnproven,
                |cause| InferenceBlockReason::StreamInterrupted { cause }),
        }}),
        None => (0, Gate::Idle),
    };
    write(tx, identity, generation, &state)?;
    Ok((generation, state))
}

pub(super) fn block_tx(
    tx: &Transaction<'_>, identity: &HostIdentity, request: &RequestId,
    reason: InferenceBlockReason,
) -> Result<InferenceBlock> {
    let (generation, state) = initialize(tx, identity)?;
    let frontier = embedded_round::frontier(tx, identity)?;
    if frontier.pending_head.as_ref().or(frontier.settled_head.as_ref()) != Some(request) {
        return Err(StoreError::StaleInferenceRecovery);
    }
    if let Gate::Blocked { block } = state {
        if block.request == *request && block.reason == reason {
            return Ok(block);
        }
    }
    let generation = generation.checked_add(1).ok_or(StoreError::StaleInferenceRecovery)?;
    let block = InferenceBlock { request: request.clone(), generation, reason };
    write(tx, identity, generation, &Gate::Blocked { block: block.clone() })?;
    Ok(block)
}

fn projection(state: Gate) -> HostInferenceState {
    match state {
        Gate::Idle => HostInferenceState::Idle,
        Gate::Running { .. } => HostInferenceState::Running,
        Gate::Blocked { block } => HostInferenceState::Blocked { block },
        Gate::Recovering { operation, block, .. } => HostInferenceState::Recovering { operation_id: operation, block },
    }
}

/// Every retained local operation affected by the interrupted lineage must have
/// terminal evidence or confirmed interruption. Rendering never establishes cleanup.
fn validate_cleanup(c: &Connection, identity: &HostIdentity, head: &RequestId) -> Result<()> {
    let mut statement = c.prepare(
        "WITH RECURSIVE lineage(id) AS (SELECT ?1 UNION ALL SELECT r.parent_id FROM requests r JOIN lineage l ON r.id=l.id WHERE r.parent_id IS NOT NULL) \
         SELECT origin,origin_request_id,call_id,state,terminal_json FROM claims \
         WHERE request_id IN lineage AND json_extract(origin,'$.actor')=?2 \
         AND (json_extract(origin,'$.run')=?3 OR json_extract(origin,'$.kind')='standalone')",
    )?;
    let rows = statement.query_map(params![head.0, identity.actor.0, identity.run], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?))
    })?;
    for row in rows {
        let (origin, request, call, state, terminal) = row?;
        let operation = OperationId { origin: serde_json::from_str(&origin)?, request: RequestId(request), call: crate::model::CallId(call) };
        let safe = match state.as_str() {
            "interrupted" => true,
            "settled" => terminal.map(|raw| serde_json::from_str::<super::TerminalOutcome>(&raw))
                .transpose()?.is_some_and(|outcome| !matches!(outcome, super::TerminalOutcome::CancellationUnconfirmed(_))),
            _ => false,
        };
        if !safe { return Err(StoreError::InferenceRecoveryUncertain { operation }); }
    }
    Ok(())
}

impl Store {
    pub fn embedded_inference_state(&self, identity: &HostIdentity) -> Result<HostInferenceState> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| Ok(projection(initialize(tx, identity)?.1)))
    }

    pub(crate) fn begin_embedded_inference(&self, identity: &HostIdentity, expected: Option<&RequestId>) -> Result<InferenceAttempt> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let (generation, state) = initialize(tx, identity)?;
            let frontier = embedded_round::frontier(tx, identity)?;
            if frontier.settled_head.as_ref() != expected {
                return Err(StoreError::StaleInferenceRecovery);
            }
            let recovering = matches!(state, Gate::Recovering { .. });
            match state {
                Gate::Idle => {},
                Gate::Blocked { block } => return Err(StoreError::InferenceBlocked { block }),
                Gate::Running { .. } => return Err(StoreError::InferenceRunning),
                Gate::Recovering { incarnation, resume_head, .. } => {
                    if incarnation != identity.incarnation || frontier.pending_head.as_ref()
                        .or(frontier.settled_head.as_ref()) != Some(&resume_head) {
                        return Err(StoreError::StaleInferenceRecovery);
                    }
                    validate_cleanup(tx, identity, &resume_head)?;
                },
            }
            // This private attempt fences completion; it is not an actor/model round ID.
            let attempt = uuid::Uuid::new_v4().to_string();
            write(tx, identity, generation, &Gate::Running { incarnation: identity.incarnation.clone(), attempt: attempt.clone() })?;
            Ok(InferenceAttempt { identity: identity.clone(), attempt, recovering })
        })
    }

    pub(crate) fn finish_embedded_inference(&self, attempt: &InferenceAttempt) -> Result<()> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let (generation, state) = initialize(tx, &attempt.identity)?;
            match state {
                Gate::Running { incarnation, attempt: current } if incarnation == attempt.identity.incarnation && current == attempt.attempt =>
                    write(tx, &attempt.identity, generation, &Gate::Idle),
                Gate::Blocked { .. } => Ok(()),
                _ => Err(StoreError::StaleInferenceRecovery),
            }
        })
    }

    pub(crate) fn block_embedded_inference(&self, identity: &HostIdentity, request: &RequestId, reason: InferenceBlockReason) -> Result<InferenceBlock> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| block_tx(tx, identity, request, reason))
    }

    /// The exclusive host-owner startup path fences old attempts and grants.
    /// Passive Store reopen never performs takeover or authorizes continuation.
    pub fn recover_embedded_inference_after_host_loss(&self, run: &str) -> Result<usize> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let identities = {
                let mut query = tx.prepare("SELECT agent_path,incarnation FROM embedded_bindings WHERE run_id=?1")?;
                query.query_map([run], |r| Ok(HostIdentity { run: run.into(), actor: crate::model::AgentPath(r.get(0)?), incarnation: r.get(1)? }))?
                    .collect::<std::result::Result<Vec<_>,_>>()?
            };
            let mut count = 0;
            for identity in identities {
                let (_, state) = initialize(tx, &identity)?;
                let frontier = embedded_round::frontier(tx, &identity)?;
                if matches!(state, Gate::Running { .. } | Gate::Recovering { .. })
                    || (matches!(state, Gate::Idle) && frontier.pending_head.is_some()) {
                    if let Some(request) = frontier.pending_head.or(frontier.settled_head) {
                        block_tx(tx, &identity, &request, InferenceBlockReason::HostInterrupted)?;
                    } else {
                        // No request, provider issuance or effect could have escaped.
                        let (generation, _) = read(tx, &identity)?.ok_or(StoreError::InvalidEmbeddedFrontier)?;
                        write(tx, &identity, generation, &Gate::Idle)?;
                    }
                    count += 1;
                }
            }
            Ok(count)
        })
    }

    /// Projection publication, once-only authorization and command receipt share
    /// one existing Store transaction. A duplicate observes its prior outcome.
    pub(crate) fn authorize_embedded_recovery(&self, identity: &HostIdentity, operation: ClientOperationId) -> Result<CommandReceipt> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let record = embedded_commands::read(tx, &identity.run, operation)?.ok_or(StoreError::InvalidCommandState)?;
            let HostCommand::Recover { target, expected_block, mode } = &record.command else { return Err(StoreError::InvalidCommandState); };
            if target != identity { return Err(StoreError::ConflictingCommand); }
            if let Some(receipt) = record.receipt { return Ok(receipt); }
            if record.state != super::EmbeddedCommandState::Dispatching { return Err(StoreError::InvalidCommandState); }
            tx.execute_batch("SAVEPOINT inference_recovery")?;
            let result = (|| {
                let (generation, state) = initialize(tx, identity)?;
                let Gate::Blocked { block } = state else { return Err(StoreError::StaleInferenceRecovery); };
                let frontier = embedded_round::frontier(tx, identity)?;
                if block != *expected_block || frontier.pending_head.as_ref().or(frontier.settled_head.as_ref()) != Some(&block.request) {
                    return Err(StoreError::StaleInferenceRecovery);
                }
                validate_cleanup(tx, identity, &block.request)?;
                let (resume_head, receipt) = super::context::publish_recovery_projection(tx, identity, &block.request, *mode, &self.store_id)?;
                write(tx, identity, generation, &Gate::Recovering { incarnation: identity.incarnation.clone(), operation, block: block.clone(), resume_head })?;
                Ok(CommandReceiptOutcome::RecoveryAuthorized { target: identity.clone(), block, mode: *mode, projection: receipt })
            })();
            let (state, outcome) = match result {
                Ok(outcome) => {
                    tx.execute_batch("RELEASE inference_recovery")?;
                    ("recovery_authorized", outcome)
                },
                Err(error @ (StoreError::StaleInferenceRecovery | StoreError::InvalidEmbeddedBinding | StoreError::InferenceRecoveryUncertain { .. } | StoreError::Context(_))) => {
                    tx.execute_batch("ROLLBACK TO inference_recovery; RELEASE inference_recovery")?;
                    ("refused", CommandReceiptOutcome::Refused { target: Some(identity.clone()), reason: error.to_string() })
                },
                Err(error) => return Err(error),
            };
            let receipt = CommandReceipt { command_id: operation.to_string(), outcome };
            if tx.execute("UPDATE embedded_commands SET state=?3,outcome=?4 WHERE run_id=?1 AND operation_id=?2 AND state='dispatching'",
                params![identity.run, operation.to_string(), state, serde_json::to_string(&receipt)?])? != 1 { return Err(StoreError::InvalidCommandState); }
            Ok(receipt)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{embedding::RecoveryMode, model::{AgentPath, CallId}, store::Usage, transport::StreamInterruption};
    fn identity() -> HostIdentity { HostIdentity { run: "gate-run".into(), actor: AgentPath("/root".into()), incarnation: "first".into() } }
    fn ready(store: &Store, identity: &HostIdentity) {
        store.bind_embedded_actor(identity, None).unwrap();
        store.initialize_context_target(&crate::model::ConversationIdentity::Embedded {
            run: identity.run.clone(), actor: identity.actor.clone(), incarnation: identity.incarnation.clone(),
        }, &crate::context::ContextProjectionTarget {
            model: "test".into(), route: crate::transport::ResponsesRoute::Codex,
            protocol: crate::transport::ResponsesProtocol::Standard,
        }).unwrap();
        assert_eq!(store.embedded_inference_state(identity).unwrap(), HostInferenceState::Idle);
    }
    fn interrupt(store: &Store, identity: &HostIdentity, parent: Option<&RequestId>, request: &RequestId) -> InferenceBlock {
        let _attempt = store.begin_embedded_inference(identity, parent).unwrap();
        store.write_embedded_request(identity, request, parent, &[], Usage::default()).unwrap();
        assert!(store.record_interrupted_model_request(request, StreamInterruption::MissingCompletion, Some((identity, parent))).unwrap());
        let HostInferenceState::Blocked { block } = store.embedded_inference_state(identity).unwrap() else { panic!("blocked"); };
        block
    }
    fn recovery(store: &Store, identity: &HostIdentity, block: &InferenceBlock) -> ClientOperationId {
        let operation = ClientOperationId(uuid::Uuid::new_v4());
        store.enqueue_embedded_command(operation, &HostCommand::Recover { target: identity.clone(), expected_block: block.clone(), mode: RecoveryMode::Exact }).unwrap();
        store.claim_embedded_command(&identity.run, operation).unwrap().unwrap();
        operation
    }
    #[test]
    fn eof_input_and_wake_cannot_authorize_and_duplicate_recovery_is_once_only() {
        let store = Store::memory().unwrap(); let identity = identity(); ready(&store, &identity);
        let request = RequestId("request".into()); let block = interrupt(&store, &identity, None, &request);
        store.admit_embedded_input(&identity, "queued", "human", &crate::item::Item(serde_json::json!({"type":"message","role":"user","content":"new input"}))).unwrap();
        for _ in 0..3 { assert!(matches!(store.begin_embedded_inference(&identity, None), Err(StoreError::InferenceBlocked { .. }))); }
        assert_eq!(store.unread("/root").unwrap().len(), 1);
        let operation = recovery(&store, &identity, &block);
        let receipt = store.authorize_embedded_recovery(&identity, operation).unwrap();
        assert!(matches!(receipt.outcome, CommandReceiptOutcome::RecoveryAuthorized { .. }));
        let attempt = store.begin_embedded_inference(&identity, None).unwrap();
        assert!(attempt.recovering);
        assert_eq!(store.authorize_embedded_recovery(&identity, operation).unwrap(), receipt);
        assert!(matches!(store.begin_embedded_inference(&identity, None), Err(StoreError::InferenceRunning)));
        assert_eq!(store.unread("/root").unwrap().len(), 1);
    }
    #[test]
    fn stale_block_wrong_incarnation_and_uncertain_terminal_refuse_atomically() {
        let store = Store::memory().unwrap(); let identity = identity(); ready(&store, &identity);
        let request = RequestId("request".into()); let block = interrupt(&store, &identity, None, &request);
        let mut stale = block.clone(); stale.generation += 1;
        let operation = recovery(&store, &identity, &stale);
        assert!(matches!(store.authorize_embedded_recovery(&identity, operation).unwrap().outcome, CommandReceiptOutcome::Refused { .. }));
        let foreign = HostIdentity { incarnation:"other".into(), ..identity.clone() };
        let operation = recovery(&store, &foreign, &block);
        assert!(matches!(store.authorize_embedded_recovery(&foreign, operation).unwrap().outcome, CommandReceiptOutcome::Refused { .. }));
        let op = OperationId { origin: crate::model::ConversationIdentity::Embedded { run:identity.run.clone(), actor:identity.actor.clone(), incarnation:identity.incarnation.clone() }, request:request.clone(), call:CallId("uncertain".into()) };
        store.lock().unwrap().execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state,terminal_json) VALUES(?1,?2,?3,?2,'settled',?4)", params![serde_json::to_string(&op.origin).unwrap(), request.0, op.call.0, serde_json::to_string(&super::super::TerminalOutcome::CancellationUnconfirmed("lost cleanup".into())).unwrap()]).unwrap();
        let operation = recovery(&store, &identity, &block);
        let receipt = store.authorize_embedded_recovery(&identity, operation).unwrap();
        assert!(matches!(receipt.outcome, CommandReceiptOutcome::Refused { .. }));
        assert_eq!(store.embedded_inference_state(&identity).unwrap(), HostInferenceState::Blocked { block });
        assert_eq!(store.authorize_embedded_recovery(&identity, operation).unwrap(), receipt);
    }
    #[test]
    fn restart_fences_old_grants_and_second_block_retains_monotonic_generation() {
        let path = std::env::temp_dir().join(format!("harness-gate-{}.sqlite",uuid::Uuid::new_v4()));
        let identity = identity(); let request = RequestId("request".into());
        let first; let operation; let receipt;
        {
            let store = Store::open(&path).unwrap(); ready(&store, &identity);
            first = interrupt(&store, &identity, None, &request);
            operation = recovery(&store, &identity, &first);
            receipt = store.authorize_embedded_recovery(&identity, operation).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert!(matches!(store.embedded_inference_state(&identity).unwrap(), HostInferenceState::Recovering { .. }));
        assert_eq!(store.recover_embedded_inference_after_host_loss(&identity.run).unwrap(), 1);
        let HostInferenceState::Blocked { block:second } = store.embedded_inference_state(&identity).unwrap() else { panic!("blocked"); };
        assert!(second.generation > first.generation);
        assert_eq!(store.authorize_embedded_recovery(&identity, operation).unwrap(), receipt);
        assert!(matches!(store.begin_embedded_inference(&identity, None), Err(StoreError::InferenceBlocked { .. })));
        let fresh = recovery(&store, &identity, &second);
        assert!(matches!(store.authorize_embedded_recovery(&identity, fresh).unwrap().outcome, CommandReceiptOutcome::RecoveryAuthorized { .. }));
        let _attempt = store.begin_embedded_inference(&identity, None).unwrap();
        assert_eq!(store.recover_embedded_inference_after_host_loss(&identity.run).unwrap(), 1);
        let HostInferenceState::Blocked { block:third } = store.embedded_inference_state(&identity).unwrap() else { panic!("blocked"); };
        assert!(third.generation > second.generation);
        assert_eq!(store.recover_embedded_inference_after_host_loss(&identity.run).unwrap(), 0);
        drop(store); std::fs::remove_file(path).unwrap();
    }
}
