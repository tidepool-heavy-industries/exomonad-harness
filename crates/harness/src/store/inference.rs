//! The Store owns inference admission independently of actor liveness and inboxes.
use super::{Connection, Result, Store, StoreError, embedded, embedded_commands, embedded_round};
use crate::{
    embedding::{
        ClientOperationId, HostIdentity, HostInferenceState, InferenceBlock, InferenceBlockReason,
    },
    model::{OperationId, RequestId},
    server::{CommandReceipt, CommandReceiptOutcome, HostCommand},
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum Gate {
    Idle,
    AutoRetry {
        incarnation: String,
        block: InferenceBlock,
        attempt: u8,
        due_ms: i64,
    },
    Running {
        incarnation: String,
        attempt: String,
    },
    Blocked {
        block: InferenceBlock,
    },
    Recovering {
        incarnation: String,
        operation: ClientOperationId,
        block: InferenceBlock,
        resume_head: RequestId,
    },
}

/// Inference authorization, request frontier and mailbox input share one
/// owner-selected transaction. The captured state observes an existing grant;
/// it does not issue one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EmbeddedRoundSnapshot {
    store_id: String,
    identity: HostIdentity,
    pub(crate) frontier: super::EmbeddedRoundFrontier,
    pub(crate) inference: HostInferenceState,
    pub(crate) generation: u64,
    pub(crate) first_envelope: Option<i64>,
}

#[derive(Clone, Copy)]
pub(crate) enum InferenceAttemptDisposition {
    Completed,
    Cancelled,
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
    )
    .optional()?
    .map(|(generation, state)| Ok((generation, serde_json::from_str(&state)?)))
    .transpose()
}

fn write(
    tx: &Transaction<'_>,
    identity: &HostIdentity,
    generation: u64,
    state: &Gate,
) -> Result<()> {
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
    if let Some((generation, state)) = read(tx, identity)? {
        let old_owner = match &state {
            Gate::Running { incarnation, .. }
            | Gate::Recovering { incarnation, .. }
            | Gate::AutoRetry { incarnation, .. } => incarnation != &identity.incarnation,
            _ => false,
        };
        if old_owner {
            let frontier = embedded_round::frontier(tx, identity)?;
            let state = match frontier.pending_head.or(frontier.settled_head) {
                Some(request) => {
                    let generation = generation
                        .checked_add(1)
                        .ok_or(StoreError::StaleInferenceRecovery)?;
                    let state = Gate::Blocked {
                        block: InferenceBlock {
                            continuation_refusal: None,
                            request,
                            generation,
                            reason: InferenceBlockReason::HostInterrupted,
                        },
                    };
                    write(tx, identity, generation, &state)?;
                    return Ok((generation, state));
                }
                None => Gate::Idle,
            };
            write(tx, identity, generation, &state)?;
            return Ok((generation, state));
        }
        return Ok((generation, state));
    }
    let frontier = embedded_round::frontier(tx, identity)?;
    let (generation, state) = match frontier.pending_head {
        Some(request) => (
            1,
            Gate::Blocked {
                block: InferenceBlock {
                    continuation_refusal: None,
                    request,
                    generation: 1,
                    reason: frontier
                        .pending_interruption
                        .map_or(InferenceBlockReason::ContinuityUnproven, |cause| {
                            InferenceBlockReason::StreamInterrupted { cause }
                        }),
                },
            },
        ),
        None => (0, Gate::Idle),
    };
    write(tx, identity, generation, &state)?;
    Ok((generation, state))
}

pub(super) fn block_tx(
    tx: &Transaction<'_>,
    identity: &HostIdentity,
    request: &RequestId,
    reason: InferenceBlockReason,
) -> Result<InferenceBlock> {
    let (generation, state) = initialize(tx, identity)?;
    let frontier = embedded_round::frontier(tx, identity)?;
    if frontier
        .pending_head
        .as_ref()
        .or(frontier.settled_head.as_ref())
        != Some(request)
    {
        return Err(StoreError::StaleInferenceRecovery);
    }
    if let Gate::Blocked { block } = state {
        if block.request == *request && block.reason == reason {
            return Ok(block);
        }
    }
    let generation = generation
        .checked_add(1)
        .ok_or(StoreError::StaleInferenceRecovery)?;
    let block = InferenceBlock {
        continuation_refusal: None,
        request: request.clone(),
        generation,
        reason,
    };
    write(
        tx,
        identity,
        generation,
        &Gate::Blocked {
            block: block.clone(),
        },
    )?;
    Ok(block)
}

fn projection(state: Gate) -> HostInferenceState {
    match state {
        Gate::AutoRetry { block, attempt, .. } => HostInferenceState::AutoRetry {
            block,
            attempt,
            maximum: 2,
            delay_ms: retry_delay(attempt),
        },
        Gate::Idle => HostInferenceState::Idle,
        Gate::Running { .. } => HostInferenceState::Running,
        Gate::Blocked { block } => HostInferenceState::Blocked { block },
        Gate::Recovering {
            operation, block, ..
        } => HostInferenceState::Recovering {
            operation_id: operation,
            block,
        },
    }
}

/// Every retained local operation affected by the interrupted lineage must have
/// terminal evidence or confirmed interruption. Rendering never establishes cleanup.
fn validate_cleanup(c: &Connection, identity: &HostIdentity, head: &RequestId) -> Result<()> {
    let mut statement = c.prepare(
        "WITH RECURSIVE lineage(id) AS (SELECT ?1 UNION ALL SELECT r.parent_id FROM requests r JOIN lineage l ON r.id=l.id WHERE r.parent_id IS NOT NULL) \
         SELECT DISTINCT origin,origin_request_id,call_id FROM claims \
         WHERE request_id IN lineage AND json_extract(origin,'$.actor')=?2 \
         AND (json_extract(origin,'$.run')=?3 OR json_extract(origin,'$.kind')='standalone')",
    )?;
    let rows = statement.query_map(params![head.0, identity.actor.0, identity.run], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (origin, request, call) = row?;
        let operation = OperationId {
            origin: serde_json::from_str(&origin)?,
            request: RequestId(request),
            call: crate::model::CallId(call),
        };
        let safe = match super::terminal::operation_cleanup_evidence(c, &operation)? {
            super::terminal::OperationCleanupEvidence::NotStarted => true,
            super::terminal::OperationCleanupEvidence::Terminal(terminal) => {
                debug_assert!(matches!(
                    terminal,
                    super::TerminalOutcome::Success
                        | super::TerminalOutcome::Failure(_)
                        | super::TerminalOutcome::Cancelled
                        | super::TerminalOutcome::CancelledWithReceipt(_)
                ));
                true
            }
            super::terminal::OperationCleanupEvidence::Uncertain => false,
        };
        if !safe {
            return Err(StoreError::InferenceRecoveryUncertain { operation });
        }
    }
    Ok(())
}

fn selected_snapshot(
    c: &Connection,
    store_id: &str,
    identity: &HostIdentity,
    frontier: super::EmbeddedRoundFrontier,
    generation: u64,
    state: &Gate,
) -> Result<EmbeddedRoundSnapshot> {
    Ok(EmbeddedRoundSnapshot {
        store_id: store_id.into(), identity: identity.clone(), frontier,
        inference: projection(state.clone()), generation,
        first_envelope: c.query_row(
            "SELECT id FROM envelopes WHERE recipient=?1 AND delivered_request IS NULL ORDER BY id LIMIT 1",
            [&identity.actor.0], |row| row.get(0),
        ).optional()?,
    })
}

fn retry_delay(attempt: u8) -> u32 {
    if attempt == 1 { 1_000 } else { 3_000 }
}

fn expire_intents(tx: &Transaction<'_>, identity: &HostIdentity, attempt: &str) -> Result<()> {
    tx.execute("UPDATE embedded_input_continuations SET attempt=NULL WHERE run_id=?1 AND agent_path=?2 AND incarnation=?3 AND attempt=?4", params![identity.run,identity.actor.0,identity.incarnation,attempt])?;
    Ok(())
}

fn refusal(error: &StoreError) -> Option<crate::embedding::ContinuationRefusal> {
    use crate::embedding::ContinuationRefusal as R;
    if matches!(
        error,
        StoreError::Context(crate::context::ContextError::OpaqueModel)
            | StoreError::IncompleteProviderPrefix { .. }
    ) {
        return Some(R::ExactProjectionUnavailable);
    }
    if error.is_inference_projection_refusal() {
        return Some(R::ContinuityUnproven);
    }
    match error {
        StoreError::InferenceRecoveryUncertain { .. } => Some(R::UncertainEffects),
        StoreError::StaleInferenceRecovery | StoreError::InvalidEmbeddedBinding => Some(R::Stale),
        StoreError::ConflictingReplayOutcome { .. }
        | StoreError::UnsupportedReplayOutcome { .. } => Some(R::ContinuityUnproven),
        _ => None,
    }
}

fn grant_tx(
    tx: &Transaction<'_>,
    identity: &HostIdentity,
    expected: &InferenceBlock,
    operation: ClientOperationId,
    mode: crate::embedding::RecoveryMode,
    store_id: &str,
) -> Result<crate::context::RecoveryProjectionReceipt> {
    let (generation, state) = initialize(tx, identity)?;
    let block = match state {
        Gate::Blocked { block } | Gate::AutoRetry { block, .. } => block,
        _ => return Err(StoreError::StaleInferenceRecovery),
    };
    let frontier = embedded_round::frontier(tx, identity)?;
    if block != *expected
        || frontier
            .pending_head
            .as_ref()
            .or(frontier.settled_head.as_ref())
            != Some(&block.request)
    {
        return Err(StoreError::StaleInferenceRecovery);
    }
    validate_cleanup(tx, identity, &block.request)?;
    let (resume_head, receipt) =
        super::context::publish_recovery_projection(tx, identity, &block.request, mode, store_id)?;
    write(
        tx,
        identity,
        generation,
        &Gate::Recovering {
            incarnation: identity.incarnation.clone(),
            operation,
            block,
            resume_head,
        },
    )?;
    Ok(receipt)
}

fn mark_continuation_refusal(
    tx: &Transaction<'_>,
    identity: &HostIdentity,
    expected: &InferenceBlock,
    reason: crate::embedding::ContinuationRefusal,
) -> Result<InferenceBlock> {
    let (generation, state) = initialize(tx, identity)?;
    if let Gate::Blocked { mut block } | Gate::AutoRetry { mut block, .. } = state {
        if block == *expected && block.continuation_refusal != Some(reason) {
            block.continuation_refusal = Some(reason);
            block.generation = generation
                .checked_add(1)
                .ok_or(StoreError::StaleInferenceRecovery)?;
            write(
                tx,
                identity,
                block.generation,
                &Gate::Blocked {
                    block: block.clone(),
                },
            )?;
        }
        return Ok(block);
    }
    Ok(expected.clone())
}

fn checked_exact_grant(
    tx: &Transaction<'_>,
    identity: &HostIdentity,
    block: &InferenceBlock,
    operation: ClientOperationId,
    store_id: &str,
) -> Result<crate::embedding::InputContinuation> {
    use crate::embedding::{InputContinuation, RecoveryMode};
    tx.execute_batch("SAVEPOINT input_continuation")?;
    match grant_tx(
        tx,
        identity,
        block,
        operation,
        RecoveryMode::Exact,
        store_id,
    ) {
        Ok(_) => {
            tx.execute_batch("RELEASE input_continuation")?;
            Ok(InputContinuation::Granted {
                block: block.clone(),
            })
        }
        Err(error) => {
            let Some(reason) = refusal(&error) else {
                return Err(error);
            };
            tx.execute_batch("ROLLBACK TO input_continuation; RELEASE input_continuation")?;
            let observed = mark_continuation_refusal(tx, identity, block, reason)?;
            Ok(InputContinuation::Refused {
                block: observed,
                reason,
            })
        }
    }
}

pub(super) fn read_input_continuation(
    c: &Connection,
    envelope: i64,
) -> Result<Option<crate::embedding::InputContinuation>> {
    c.query_row(
        "SELECT outcome FROM embedded_input_continuations WHERE envelope_id=?1",
        [envelope],
        |r| r.get::<_, String>(0),
    )
    .optional()?
    .map(|v| Ok(serde_json::from_str(&v)?))
    .transpose()
}

/// Shared by native input and journaled browser Send. The input envelope and
/// its once-only intent commit together; duplicate admission observes this receipt.
pub(super) fn admit_input_continuation_tx(
    tx: &Transaction<'_>,
    identity: &HostIdentity,
    envelope: i64,
    operation: ClientOperationId,
    store_id: &str,
) -> Result<crate::embedding::InputContinuation> {
    use crate::embedding::InputContinuation;
    if let Some(outcome) = tx
        .query_row(
            "SELECT outcome FROM embedded_input_continuations WHERE envelope_id=?1",
            [envelope],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok(serde_json::from_str(&outcome)?);
    }
    let (_, state) = initialize(tx, identity)?;
    let (attempt, outcome) = match state {
        Gate::Running { attempt, .. } => (Some(attempt), InputContinuation::Queued),
        Gate::Blocked { block } | Gate::AutoRetry { block, .. } => (
            None,
            checked_exact_grant(tx, identity, &block, operation, store_id)?,
        ),
        Gate::Idle | Gate::Recovering { .. } => (None, InputContinuation::Queued),
    };
    tx.execute("INSERT INTO embedded_input_continuations(run_id,agent_path,incarnation,envelope_id,attempt,outcome) VALUES(?1,?2,?3,?4,?5,?6)", params![identity.run,identity.actor.0,identity.incarnation,envelope,attempt,serde_json::to_string(&outcome)?])?;
    Ok(outcome)
}

pub(crate) struct AutomaticRetry {
    pub(crate) block: InferenceBlock,
    pub(crate) attempt: u8,
    pub(crate) delay_ms: u32,
}

impl Store {
    pub fn embedded_inference_state(&self, identity: &HostIdentity) -> Result<HostInferenceState> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| Ok(projection(initialize(tx, identity)?.1)))
    }

    pub(crate) fn select_embedded_round(
        &self,
        identity: &HostIdentity,
    ) -> Result<EmbeddedRoundSnapshot> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let (generation, state) = initialize(tx, identity)?;
            let frontier = embedded_round::frontier(tx, identity)?;
            selected_snapshot(tx, &self.store_id, identity, frontier, generation, &state)
        })
    }

    pub(crate) fn begin_selected_embedded_inference(
        &self,
        identity: &HostIdentity,
        snapshot: &EmbeddedRoundSnapshot,
    ) -> Result<InferenceAttempt> {
        self.begin_inference(
            identity,
            snapshot.frontier.settled_head.as_ref(),
            Some(snapshot),
        )
    }

    pub(crate) fn begin_embedded_inference(
        &self,
        identity: &HostIdentity,
        expected: Option<&RequestId>,
    ) -> Result<InferenceAttempt> {
        self.begin_inference(identity, expected, None)
    }

    fn begin_inference(
        &self,
        identity: &HostIdentity,
        expected: Option<&RequestId>,
        selected: Option<&EmbeddedRoundSnapshot>,
    ) -> Result<InferenceAttempt> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let (generation, state) = initialize(tx, identity)?;
            let frontier = embedded_round::frontier(tx, identity)?;
            if let Some(selected) = selected {
                let current = selected_snapshot(
                    tx,
                    &self.store_id,
                    identity,
                    frontier.clone(),
                    generation,
                    &state,
                )?;
                if &current != selected {
                    return Err(StoreError::StaleInferenceRecovery);
                }
            }
            if frontier.settled_head.as_ref() != expected {
                return Err(StoreError::StaleInferenceRecovery);
            }
            let recovering = matches!(state, Gate::Recovering { .. });
            match state {
                Gate::Idle => {}
                Gate::Blocked { block } | Gate::AutoRetry { block, .. } => {
                    return Err(StoreError::InferenceBlocked { block });
                }
                Gate::Running { .. } => return Err(StoreError::InferenceRunning),
                Gate::Recovering {
                    incarnation,
                    resume_head,
                    ..
                } => {
                    if incarnation != identity.incarnation
                        || frontier
                            .pending_head
                            .as_ref()
                            .or(frontier.settled_head.as_ref())
                            != Some(&resume_head)
                    {
                        return Err(StoreError::StaleInferenceRecovery);
                    }
                    validate_cleanup(tx, identity, &resume_head)?;
                }
            }
            // This private attempt fences completion; it is not an actor/model round ID.
            let attempt = uuid::Uuid::new_v4().to_string();
            write(
                tx,
                identity,
                generation,
                &Gate::Running {
                    incarnation: identity.incarnation.clone(),
                    attempt: attempt.clone(),
                },
            )?;
            Ok(InferenceAttempt {
                identity: identity.clone(),
                attempt,
                recovering,
            })
        })
    }

    pub(crate) fn finish_embedded_inference(
        &self,
        attempt: &InferenceAttempt,
        disposition: InferenceAttemptDisposition,
    ) -> Result<()> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let (generation, state) = initialize(tx, &attempt.identity)?;
            match state {
                Gate::Running {
                    incarnation,
                    attempt: current,
                } if incarnation == attempt.identity.incarnation && current == attempt.attempt => {
                    if matches!(disposition, InferenceAttemptDisposition::Completed) {
                        tx.execute(
                            "DELETE FROM embedded_retry_lineage WHERE run_id=?1 AND agent_path=?2",
                            params![attempt.identity.run, attempt.identity.actor.0],
                        )?;
                    }
                    expire_intents(tx, &attempt.identity, &attempt.attempt)?;
                    write(tx, &attempt.identity, generation, &Gate::Idle)
                }
                Gate::Blocked { .. } => Ok(()),
                _ => Err(StoreError::StaleInferenceRecovery),
            }
        })
    }

    pub(crate) fn block_embedded_inference(
        &self,
        identity: &HostIdentity,
        request: &RequestId,
        reason: InferenceBlockReason,
    ) -> Result<InferenceBlock> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            validate_cleanup(tx, identity, request)?;
            block_tx(tx, identity, request, reason)
        })
    }

    /// The exclusive host-owner startup path fences old attempts and grants.
    /// Passive Store reopen never performs takeover or authorizes continuation.
    pub fn recover_embedded_inference_after_host_loss(&self, run: &str) -> Result<usize> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let identities = {
                let mut query = tx.prepare(
                    "SELECT agent_path,incarnation FROM embedded_bindings WHERE run_id=?1",
                )?;
                query
                    .query_map([run], |r| {
                        Ok(HostIdentity {
                            run: run.into(),
                            actor: crate::model::AgentPath(r.get(0)?),
                            incarnation: r.get(1)?,
                        })
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };
            let mut count = 0;
            for identity in identities {
                let (_, state) = initialize(tx, &identity)?;
                let frontier = embedded_round::frontier(tx, &identity)?;
                if matches!(
                    state,
                    Gate::Running { .. } | Gate::Recovering { .. } | Gate::AutoRetry { .. }
                ) || (matches!(state, Gate::Idle) && frontier.pending_head.is_some())
                {
                    if let Some(request) = frontier.pending_head.or(frontier.settled_head) {
                        block_tx(
                            tx,
                            &identity,
                            &request,
                            InferenceBlockReason::HostInterrupted,
                        )?;
                    } else {
                        // No request, provider issuance or effect could have escaped.
                        let (generation, _) =
                            read(tx, &identity)?.ok_or(StoreError::InvalidEmbeddedFrontier)?;
                        write(tx, &identity, generation, &Gate::Idle)?;
                    }
                    count += 1;
                }
            }
            Ok(count)
        })
    }

    pub(crate) fn embedded_input_continuation(
        &self,
        envelope: i64,
    ) -> Result<Option<crate::embedding::InputContinuation>> {
        let c = self.lock()?;
        read_input_continuation(&c, envelope)
    }

    /// Consume only intent bound to this attempt. Included or successfully
    /// completed input can never grant a later unrelated interruption.
    pub(crate) fn continue_queued_input(&self, attempt: &InferenceAttempt) -> Result<bool> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let identity = &attempt.identity;
            let (_, state) = initialize(tx, identity)?;
            let eligible: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM embedded_input_continuations i JOIN envelopes e ON e.id=i.envelope_id WHERE i.run_id=?1 AND i.agent_path=?2 AND i.incarnation=?3 AND i.attempt=?4 AND e.delivered_request IS NULL)", params![identity.run,identity.actor.0,identity.incarnation,attempt.attempt], |r| r.get(0))?;
            expire_intents(tx, identity, &attempt.attempt)?;
            if !eligible { return Ok(false); }
            let Gate::Blocked { block } = state else { return Ok(false); };
            Ok(matches!(checked_exact_grant(tx, identity, &block, ClientOperationId(uuid::Uuid::new_v4()), &self.store_id)?, crate::embedding::InputContinuation::Granted { .. }))
        })
    }

    /// Called only after Engine has drained the provider tail and confirmed all
    /// cleanup and persistence obligations. Store independently vetoes escaped
    /// completed items/claims and persists reservation before any wait.
    pub(crate) fn reserve_automatic_retry(
        &self,
        identity: &HostIdentity,
        expected: &InferenceBlock,
    ) -> Result<Option<AutomaticRetry>> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let (generation, state) = initialize(tx, identity)?;
            let Gate::Blocked { block } = state else { return Ok(None); };
            if block != *expected || !matches!(block.reason, InferenceBlockReason::StreamInterrupted { .. }) || block.continuation_refusal.is_some() { return Ok(None); }
            let escaped: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM provider_items WHERE request_id=?1) OR EXISTS(SELECT 1 FROM claims WHERE request_id=?1) OR NOT EXISTS(SELECT 1 FROM provider_requests WHERE request_id=?1 AND state='interrupted')", [&block.request.0], |r| r.get(0))?;
            if escaped { return Ok(None); }
            validate_cleanup(tx, identity, &block.request)?;
            let used: u8 = tx.query_row("SELECT attempts FROM embedded_retry_lineage WHERE run_id=?1 AND agent_path=?2", params![identity.run,identity.actor.0], |r| r.get(0)).optional()?.unwrap_or(0);
            if used >= 2 { return Ok(None); }
            // Preflight through the same exact projection owner; no new grant.
            tx.execute_batch("SAVEPOINT retry_preflight")?;
            let preflight = super::context::publish_recovery_projection(tx, identity, &block.request, crate::embedding::RecoveryMode::Exact, &self.store_id);
            tx.execute_batch("ROLLBACK TO retry_preflight; RELEASE retry_preflight")?;
            if let Err(error) = preflight {
                if let Some(reason) = refusal(&error) {
                    let mut refused = block.clone(); refused.continuation_refusal = Some(reason);
                    refused.generation = generation.checked_add(1).ok_or(StoreError::StaleInferenceRecovery)?;
                    write(tx, identity, refused.generation, &Gate::Blocked { block: refused })?;
                    return Ok(None);
                }
                return Err(error);
            }
            let attempt = used + 1;
            let delay_ms = retry_delay(attempt);
            tx.execute("INSERT INTO embedded_retry_lineage(run_id,agent_path,lineage,attempts) VALUES(?1,?2,?3,?4) ON CONFLICT(run_id,agent_path) DO UPDATE SET attempts=excluded.attempts", params![identity.run,identity.actor.0,block.request.0,attempt])?;
            write(tx, identity, generation, &Gate::AutoRetry { incarnation: identity.incarnation.clone(), block: block.clone(), attempt, due_ms: super::utc_millis() + i64::from(delay_ms) })?;
            Ok(Some(AutomaticRetry { block, attempt, delay_ms }))
        })
    }

    pub(crate) fn activate_automatic_retry(
        &self,
        identity: &HostIdentity,
        retry: &AutomaticRetry,
    ) -> Result<bool> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let (_, state) = initialize(tx, identity)?;
            if matches!(&state, Gate::Recovering { incarnation, block, .. } if incarnation == &identity.incarnation && block == &retry.block) { return Ok(true); }
            if !matches!(&state, Gate::AutoRetry { incarnation, block, attempt, .. } if incarnation == &identity.incarnation && block == &retry.block && *attempt == retry.attempt) { return Ok(false); }
            Ok(matches!(checked_exact_grant(tx, identity, &retry.block, ClientOperationId(uuid::Uuid::new_v4()), &self.store_id)?, crate::embedding::InputContinuation::Granted { .. }))
        })
    }

    pub(crate) fn cancel_automatic_retry(
        &self,
        identity: &HostIdentity,
        retry: &AutomaticRetry,
    ) -> Result<()> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let (_, state) = initialize(tx, identity)?;
            if matches!(&state, Gate::AutoRetry { block, attempt, .. } if block == &retry.block && *attempt == retry.attempt)
                || matches!(&state, Gate::Recovering { incarnation, block, .. } if incarnation == &identity.incarnation && block == &retry.block) {
                block_tx(tx, identity, &retry.block.request, InferenceBlockReason::HostInterrupted)?;
            }
            Ok(())
        })
    }

    /// Projection publication, once-only authorization and command receipt share
    /// one existing Store transaction. A duplicate observes its prior outcome.
    pub(crate) fn authorize_embedded_recovery(
        &self,
        identity: &HostIdentity,
        operation: ClientOperationId,
    ) -> Result<CommandReceipt> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let record = embedded_commands::read(tx, &identity.run, operation)?.ok_or(StoreError::InvalidCommandState)?;
            let HostCommand::Recover { target, expected_block, mode } = &record.command else { return Err(StoreError::InvalidCommandState); };
            if target != identity { return Err(StoreError::ConflictingCommand); }
            if let Some(receipt) = record.receipt { return Ok(receipt); }
            if record.state != super::EmbeddedCommandState::Dispatching { return Err(StoreError::InvalidCommandState); }
            tx.execute_batch("SAVEPOINT inference_recovery")?;
            let result = (|| {
                if !matches!(initialize(tx, identity)?.1, Gate::Blocked { .. }) { return Err(StoreError::StaleInferenceRecovery); }
                let block = expected_block.clone();
                let receipt = grant_tx(tx, identity, &block, operation, *mode, &self.store_id)?;
                Ok(CommandReceiptOutcome::RecoveryAuthorized { target: identity.clone(), block, mode: *mode, projection: receipt })
            })();
            let (state, outcome) = match result {
                Ok(outcome) => {
                    tx.execute_batch("RELEASE inference_recovery")?;
                    ("recovery_authorized", outcome)
                },
                Err(error) if refusal(&error).is_some() => {
                    tx.execute_batch("ROLLBACK TO inference_recovery; RELEASE inference_recovery")?;
                    let reason = refusal(&error).expect("classified refusal");
                    if reason != crate::embedding::ContinuationRefusal::Stale {
                        mark_continuation_refusal(tx, identity, expected_block, reason)?;
                    }
                    ("refused", CommandReceiptOutcome::ContinuationRefused { target: identity.clone(), block: expected_block.clone(), reason })
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
    use crate::{
        embedding::RecoveryMode,
        model::{AgentPath, CallId},
        store::Usage,
        transport::StreamInterruption,
    };
    fn identity() -> HostIdentity {
        HostIdentity {
            run: "gate-run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "first".into(),
        }
    }
    fn ready(store: &Store, identity: &HostIdentity) {
        store.bind_embedded_actor(identity, None).unwrap();
        store
            .initialize_context_target(
                &crate::model::ConversationIdentity::Embedded {
                    run: identity.run.clone(),
                    actor: identity.actor.clone(),
                    incarnation: identity.incarnation.clone(),
                },
                &crate::context::ContextProjectionTarget {
                    model: "test".into(),
                    route: crate::transport::ResponsesRoute::ChatGptPlan,
                    protocol: crate::transport::ResponsesProtocol::Standard,
                },
            )
            .unwrap();
        assert_eq!(
            store.embedded_inference_state(identity).unwrap(),
            HostInferenceState::Idle
        );
    }
    fn interrupt(
        store: &Store,
        identity: &HostIdentity,
        parent: Option<&RequestId>,
        request: &RequestId,
    ) -> InferenceBlock {
        let _attempt = store.begin_embedded_inference(identity, parent).unwrap();
        store
            .write_embedded_request(identity, request, parent, &[], Usage::default())
            .unwrap();
        assert!(
            store
                .record_interrupted_model_request(
                    request,
                    StreamInterruption::MissingCompletion,
                    Some((identity, parent))
                )
                .unwrap()
                .was_recorded()
        );
        let HostInferenceState::Blocked { block } =
            store.embedded_inference_state(identity).unwrap()
        else {
            panic!("blocked");
        };
        block
    }
    fn recovery(
        store: &Store,
        identity: &HostIdentity,
        block: &InferenceBlock,
    ) -> ClientOperationId {
        let operation = ClientOperationId(uuid::Uuid::new_v4());
        store
            .enqueue_embedded_command(
                operation,
                &HostCommand::Recover {
                    target: identity.clone(),
                    expected_block: block.clone(),
                    mode: RecoveryMode::Exact,
                },
            )
            .unwrap();
        store
            .claim_embedded_command(&identity.run, operation)
            .unwrap()
            .unwrap();
        operation
    }
    #[test]
    fn selected_snapshot_checks_store_mailbox_frontier_and_gate_in_existing_admission() {
        let store = Store::memory().unwrap();
        let identity = identity();
        ready(&store, &identity);
        let selected = store.select_embedded_round(&identity).unwrap();
        let foreign_actor = HostIdentity {
            actor: AgentPath("/root/other".into()),
            ..identity.clone()
        };
        store
            .bind_embedded_actor(&foreign_actor, Some(&identity.actor))
            .unwrap();
        assert!(matches!(
            store.begin_selected_embedded_inference(&foreign_actor, &selected),
            Err(StoreError::StaleInferenceRecovery)
        ));
        assert_eq!(
            store.embedded_inference_state(&foreign_actor).unwrap(),
            HostInferenceState::Idle
        );
        let foreign = Store::memory().unwrap();
        ready(&foreign, &identity);
        assert!(matches!(
            foreign.begin_selected_embedded_inference(&identity, &selected),
            Err(StoreError::StaleInferenceRecovery)
        ));
        store
            .admit_embedded_input(
                &identity,
                "queued",
                "human",
                &crate::item::Item(
                    serde_json::json!({"type":"message","role":"user","content":"new input"}),
                ),
            )
            .unwrap();
        assert!(matches!(
            store.begin_selected_embedded_inference(&identity, &selected),
            Err(StoreError::StaleInferenceRecovery)
        ));
        let selected = store.select_embedded_round(&identity).unwrap();
        store
            .write_embedded_request(
                &identity,
                &RequestId("pending".into()),
                None,
                &[],
                Usage::default(),
            )
            .unwrap();
        assert!(matches!(
            store.begin_selected_embedded_inference(&identity, &selected),
            Err(StoreError::StaleInferenceRecovery)
        ));
        assert_eq!(
            store.embedded_inference_state(&identity).unwrap(),
            HostInferenceState::Idle
        );
        let selected = store.select_embedded_round(&identity).unwrap();
        let attempt = store
            .begin_selected_embedded_inference(&identity, &selected)
            .unwrap();
        assert!(!attempt.recovering);
        assert!(matches!(
            store.begin_selected_embedded_inference(&identity, &selected),
            Err(StoreError::StaleInferenceRecovery)
        ));
        let running = store.select_embedded_round(&identity).unwrap();
        assert!(matches!(
            store.begin_selected_embedded_inference(&identity, &running),
            Err(StoreError::InferenceRunning)
        ));
    }

    #[test]
    fn eof_wakes_cannot_authorize_and_duplicate_recovery_is_once_only() {
        let store = Store::memory().unwrap();
        let identity = identity();
        ready(&store, &identity);
        let request = RequestId("request".into());
        let block = interrupt(&store, &identity, None, &request);
        for _ in 0..3 {
            assert!(matches!(
                store.begin_embedded_inference(&identity, None),
                Err(StoreError::InferenceBlocked { .. })
            ));
        }
        let operation = recovery(&store, &identity, &block);
        let receipt = store
            .authorize_embedded_recovery(&identity, operation)
            .unwrap();
        assert!(matches!(
            receipt.outcome,
            CommandReceiptOutcome::RecoveryAuthorized { .. }
        ));
        let attempt = store.begin_embedded_inference(&identity, None).unwrap();
        assert!(attempt.recovering);
        assert_eq!(
            store
                .authorize_embedded_recovery(&identity, operation)
                .unwrap(),
            receipt
        );
        assert!(matches!(
            store.begin_embedded_inference(&identity, None),
            Err(StoreError::InferenceRunning)
        ));
    }
    #[test]
    fn stale_block_wrong_incarnation_and_uncertain_terminal_refuse_atomically() {
        let store = Store::memory().unwrap();
        let identity = identity();
        ready(&store, &identity);
        let request = RequestId("request".into());
        let block = interrupt(&store, &identity, None, &request);
        let mut stale = block.clone();
        stale.generation += 1;
        let operation = recovery(&store, &identity, &stale);
        assert!(matches!(
            store
                .authorize_embedded_recovery(&identity, operation)
                .unwrap()
                .outcome,
            CommandReceiptOutcome::ContinuationRefused { .. }
        ));
        let foreign = HostIdentity {
            incarnation: "other".into(),
            ..identity.clone()
        };
        let operation = recovery(&store, &foreign, &block);
        assert!(matches!(
            store
                .authorize_embedded_recovery(&foreign, operation)
                .unwrap()
                .outcome,
            CommandReceiptOutcome::ContinuationRefused { .. }
        ));
        store.append_items(&request, &[crate::item::Item(serde_json::json!({"type":"function_call","call_id":"uncertain","name":"effect","arguments":"{}"}))]).unwrap();
        let operation = store.claim(&CallId("uncertain".into()), &request).unwrap();
        store
            .interrupt_operation_claim(&operation, &request)
            .unwrap();
        let operation = recovery(&store, &identity, &block);
        let receipt = store
            .authorize_embedded_recovery(&identity, operation)
            .unwrap();
        assert!(matches!(
            receipt.outcome,
            CommandReceiptOutcome::ContinuationRefused { .. }
        ));
        let HostInferenceState::Blocked { block: observed } =
            store.embedded_inference_state(&identity).unwrap()
        else {
            panic!("blocked");
        };
        assert_eq!(observed.request, block.request);
        assert_eq!(observed.reason, block.reason);
        assert_eq!(observed.generation, block.generation + 1);
        assert_eq!(
            observed.continuation_refusal,
            Some(crate::embedding::ContinuationRefusal::UncertainEffects)
        );
        assert_eq!(
            store
                .authorize_embedded_recovery(&identity, operation)
                .unwrap(),
            receipt
        );
    }
    #[test]
    fn restart_fences_old_grants_and_second_block_retains_monotonic_generation() {
        let path =
            std::env::temp_dir().join(format!("harness-gate-{}.sqlite", uuid::Uuid::new_v4()));
        let identity = identity();
        let request = RequestId("request".into());
        let first;
        let operation;
        let receipt;
        {
            let store = Store::open(&path).unwrap();
            ready(&store, &identity);
            first = interrupt(&store, &identity, None, &request);
            operation = recovery(&store, &identity, &first);
            receipt = store
                .authorize_embedded_recovery(&identity, operation)
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert!(matches!(
            store.embedded_inference_state(&identity).unwrap(),
            HostInferenceState::Recovering { .. }
        ));
        assert_eq!(
            store
                .recover_embedded_inference_after_host_loss(&identity.run)
                .unwrap(),
            1
        );
        let HostInferenceState::Blocked { block: second } =
            store.embedded_inference_state(&identity).unwrap()
        else {
            panic!("blocked");
        };
        assert!(second.generation > first.generation);
        assert_eq!(
            store
                .authorize_embedded_recovery(&identity, operation)
                .unwrap(),
            receipt
        );
        assert!(matches!(
            store.begin_embedded_inference(&identity, None),
            Err(StoreError::InferenceBlocked { .. })
        ));
        let fresh = recovery(&store, &identity, &second);
        assert!(matches!(
            store
                .authorize_embedded_recovery(&identity, fresh)
                .unwrap()
                .outcome,
            CommandReceiptOutcome::RecoveryAuthorized { .. }
        ));
        let _attempt = store.begin_embedded_inference(&identity, None).unwrap();
        assert_eq!(
            store
                .recover_embedded_inference_after_host_loss(&identity.run)
                .unwrap(),
            1
        );
        let HostInferenceState::Blocked { block: third } =
            store.embedded_inference_state(&identity).unwrap()
        else {
            panic!("blocked");
        };
        assert!(third.generation > second.generation);
        assert_eq!(
            store
                .recover_embedded_inference_after_host_loss(&identity.run)
                .unwrap(),
            0
        );
        drop(store);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn legacy_pending_frontier_new_send_refuses_without_projection_authority() {
        for interrupted in [false, true] {
            let store = Store::memory().unwrap();
            let identity = identity();
            store.bind_embedded_actor(&identity, None).unwrap();
            let request = RequestId("legacy-pending".into());
            store
                .write_embedded_request(&identity, &request, None, &[], Usage::default())
                .unwrap();
            if interrupted {
                store
                    .record_interrupted_model_request(
                        &request,
                        StreamInterruption::ReadFailed,
                        Some((&identity, None)),
                    )
                    .unwrap();
                store
                    .lock()
                    .unwrap()
                    .execute("DELETE FROM embedded_inference", [])
                    .unwrap();
            }
            store
                .admit_embedded_input(
                    &identity,
                    "queued",
                    "human",
                    &crate::item::Item(
                        serde_json::json!({"type":"message","role":"user","content":"queued"}),
                    ),
                )
                .unwrap();
            store
                .recover_embedded_inference_after_host_loss(&identity.run)
                .unwrap();
            let HostInferenceState::Blocked { block } =
                store.embedded_inference_state(&identity).unwrap()
            else {
                panic!("legacy pending must block");
            };
            assert_eq!(block.request, request);
            assert_eq!(block.generation, 2);
            assert_eq!(
                block.continuation_refusal,
                Some(crate::embedding::ContinuationRefusal::ContinuityUnproven)
            );
            assert_eq!(
                block.reason,
                if interrupted {
                    InferenceBlockReason::StreamInterrupted {
                        cause: StreamInterruption::ReadFailed,
                    }
                } else {
                    InferenceBlockReason::ContinuityUnproven
                }
            );
            assert!(matches!(
                store.begin_embedded_inference(&identity, None),
                Err(StoreError::InferenceBlocked { .. })
            ));
            assert_eq!(store.unread("/root").unwrap().len(), 1);
        }
    }
    #[test]
    fn bare_interruption_and_settled_interruption_are_uncertain_but_not_started_is_proven() {
        for proof in 0..3 {
            let store = Store::memory().unwrap();
            let identity = identity();
            ready(&store, &identity);
            let request = RequestId("cleanup".into());
            let _attempt = store.begin_embedded_inference(&identity, None).unwrap();
            store.write_embedded_request(&identity, &request, None, &[crate::item::Item(serde_json::json!({"type":"function_call","call_id":"effect","name":"effect","arguments":"{}"}))], Usage::default()).unwrap();
            let operation = store.claim(&CallId("effect".into()), &request).unwrap();
            match proof {
                0 => {
                    store
                        .interrupt_operation_claim(&operation, &request)
                        .unwrap();
                }
                1 => {
                    store
                        .write_job_output(
                            &operation,
                            crate::item::ToolKind::Function,
                            &crate::turn::JobOutput::Interrupted,
                        )
                        .unwrap();
                }
                _ => {
                    store
                        .interrupt_unstarted_operation_claim(&operation, &request)
                        .unwrap();
                }
            }
            store
                .record_interrupted_model_request(
                    &request,
                    StreamInterruption::MissingCompletion,
                    Some((&identity, None)),
                )
                .unwrap();
            let HostInferenceState::Blocked { block } =
                store.embedded_inference_state(&identity).unwrap()
            else {
                panic!("blocked");
            };
            let command = recovery(&store, &identity, &block);
            let receipt = store
                .authorize_embedded_recovery(&identity, command)
                .unwrap();
            assert_eq!(
                matches!(
                    receipt.outcome,
                    CommandReceiptOutcome::RecoveryAuthorized { .. }
                ),
                proof == 2
            );
            assert_eq!(store.claims_for_operation(&operation).unwrap().len(), 1);
        }
    }
    #[test]
    fn operation_wide_terminal_closes_cleanup_without_rewriting_claim_local_interruption() {
        let store = Store::memory().unwrap();
        let identity = identity();
        ready(&store, &identity);
        let source = RequestId("original".into());
        let snapshot = RequestId("snapshot".into());
        let item = crate::item::Item(
            serde_json::json!({"type":"function_call","call_id":"effect","name":"effect","arguments":"{}"}),
        );
        let _attempt = store.begin_embedded_inference(&identity, None).unwrap();
        store
            .write_embedded_request(
                &identity,
                &source,
                None,
                std::slice::from_ref(&item),
                Usage::default(),
            )
            .unwrap();
        let operation = store.claim(&CallId("effect".into()), &source).unwrap();
        store
            .write_compaction_request_with_claims(
                &snapshot,
                &source,
                "/root",
                std::slice::from_ref(&item),
                std::slice::from_ref(&operation),
                Some(&identity),
            )
            .unwrap();
        store
            .interrupt_operation_claim(&operation, &source)
            .unwrap();
        store
            .write_job_output(
                &operation,
                crate::item::ToolKind::Function,
                &crate::turn::JobOutput::Completed(Ok(serde_json::json!(42))),
            )
            .unwrap();
        store
            .record_interrupted_model_request(
                &snapshot,
                StreamInterruption::MissingCompletion,
                Some((&identity, None)),
            )
            .unwrap();
        let HostInferenceState::Blocked { block } =
            store.embedded_inference_state(&identity).unwrap()
        else {
            panic!("blocked");
        };
        let command = recovery(&store, &identity, &block);
        assert!(matches!(
            store
                .authorize_embedded_recovery(&identity, command)
                .unwrap()
                .outcome,
            CommandReceiptOutcome::RecoveryAuthorized { .. }
        ));
        assert_eq!(
            store
                .replay_tool_output_claim(&operation, &source)
                .unwrap()
                .unwrap()
                .terminal,
            super::super::TerminalOutcome::Interrupted
        );
        assert_eq!(
            store
                .replay_tool_output_claim(&operation, &snapshot)
                .unwrap()
                .unwrap()
                .terminal,
            super::super::TerminalOutcome::Success
        );
    }
    #[test]
    fn recovery_commit_failure_fences_without_publishing_snapshot_or_authorization() {
        let path = std::env::temp_dir().join(format!(
            "harness-gate-commit-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let identity = identity();
        let request = RequestId("request".into());
        let store = std::sync::Arc::new(Store::open(&path).unwrap());
        ready(&store, &identity);
        let block = interrupt(&store, &identity, None, &request);
        let command = ClientOperationId(uuid::Uuid::new_v4());
        store
            .enqueue_embedded_command(
                command,
                &HostCommand::Recover {
                    target: identity.clone(),
                    expected_block: block.clone(),
                    mode: RecoveryMode::VisibleHistory,
                },
            )
            .unwrap();
        store
            .claim_embedded_command(&identity.run, command)
            .unwrap()
            .unwrap();
        store.lock().unwrap().execute_batch("CREATE TABLE fault_parent(id INTEGER PRIMARY KEY); CREATE TABLE fault_child(id INTEGER REFERENCES fault_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER recovery_fault AFTER UPDATE OF outcome ON embedded_commands BEGIN INSERT INTO fault_child VALUES(1); END;").unwrap();
        assert!(matches!(
            store.authorize_embedded_recovery(&identity, command),
            Err(StoreError::RecoveryRequired(_))
        ));
        assert!(matches!(
            store.embedded_inference_state(&identity),
            Err(StoreError::AdmissionFenced)
        ));
        let reopened = Store::open(&path).unwrap();
        assert_eq!(
            reopened.embedded_inference_state(&identity).unwrap(),
            HostInferenceState::Blocked { block }
        );
        assert_eq!(
            reopened
                .embedded_round_frontier(&identity)
                .unwrap()
                .pending_head,
            Some(request)
        );
        assert_eq!(
            reopened
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM requests", [], |r| r
                    .get::<_, usize>(0))
                .unwrap(),
            1
        );
        let retained = reopened
            .embedded_command(&identity.run, command)
            .unwrap()
            .unwrap();
        assert_eq!(
            retained.state,
            super::super::EmbeddedCommandState::Dispatching
        );
        assert!(retained.receipt.is_none());
        reopened
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER recovery_fault")
            .unwrap();
        reopened
            .recover_embedded_command_claims(&identity.run)
            .unwrap();
        assert!(matches!(
            reopened
                .authorize_embedded_recovery(&identity, command)
                .unwrap()
                .outcome,
            CommandReceiptOutcome::Unconfirmed { .. }
        ));
        drop(reopened);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn genuine_schema18_reopen_retains_old_inputs_without_issuing_continuation() {
        let fixtures: [(&str, &[u8]); 3] = [
            ("idle", include_bytes!("fixtures/schema18-idle.sqlite.zst")),
            (
                "running",
                include_bytes!("fixtures/schema18-running.sqlite.zst"),
            ),
            (
                "blocked",
                include_bytes!("fixtures/schema18-blocked.sqlite.zst"),
            ),
        ];
        for (phase, bytes) in fixtures {
            let path = std::env::temp_dir().join(format!(
                "schema18-reopen-{}-{}.sqlite",
                phase,
                uuid::Uuid::new_v4()
            ));
            std::fs::write(&path, zstd::stream::decode_all(bytes).unwrap()).unwrap();
            let store = Store::open(&path).unwrap();
            let identity = identity();
            let before = store.embedded_inference_state(&identity).unwrap();
            match (&before, phase) {
                (HostInferenceState::Idle, "idle") | (HostInferenceState::Running, "running") => {}
                (HostInferenceState::Blocked { block }, "blocked") => {
                    assert_eq!(block.continuation_refusal, None)
                }
                _ => panic!("migration changed old gate: {before:?}"),
            }
            let old_operation = "00000000-0000-0000-0000-000000000111";
            let old_item = crate::item::Item::user_text("legacy input");
            let envelope = store
                .admit_embedded_input(&identity, old_operation, "operator", &old_item)
                .unwrap();
            assert_eq!(store.embedded_input_continuation(envelope).unwrap(), None);
            assert_eq!(store.embedded_inference_state(&identity).unwrap(), before);
            // A newly journaled browser command can refer to the same native
            // input operation; retained issuance must remain retained here too.
            let operation = ClientOperationId(uuid::Uuid::parse_str(old_operation).unwrap());
            store
                .enqueue_embedded_command(
                    operation,
                    &HostCommand::Input {
                        target: identity.clone(),
                        text: "legacy input".into(),
                    },
                )
                .unwrap();
            store
                .claim_embedded_command(&identity.run, operation)
                .unwrap()
                .unwrap();
            let crate::store::CommandInputAdmission::New(receipt) = store
                .admit_embedded_command_input(&identity, operation, "legacy input")
                .unwrap()
            else {
                panic!("new journal record");
            };
            assert!(matches!(
                receipt.outcome,
                CommandReceiptOutcome::Admitted {
                    continuation: None,
                    ..
                }
            ));
            assert_eq!(store.embedded_inference_state(&identity).unwrap(), before);
            store
                .recover_embedded_inference_after_host_loss(&identity.run)
                .unwrap();
            let recovered = store.embedded_inference_state(&identity).unwrap();
            assert_eq!(
                store
                    .admit_embedded_input(&identity, old_operation, "operator", &old_item)
                    .unwrap(),
                envelope
            );
            assert_eq!(
                store.embedded_inference_state(&identity).unwrap(),
                recovered
            );
            let c = store.lock().unwrap();
            let intents: u64 = c
                .query_row(
                    "SELECT COUNT(*) FROM embedded_input_continuations",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let budgets: u64 = c
                .query_row("SELECT COUNT(*) FROM embedded_retry_lineage", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!((intents, budgets), (0, 0));
            drop(c);
            drop(store);
            std::fs::remove_file(path).unwrap();
        }
    }
}
