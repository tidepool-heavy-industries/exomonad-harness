//! Targeted edits reuse the context commit owner and its durable event.
use super::*;
use crate::context::{
    ContextChange, ContextInspection, ContextPatch, ContextProblem, ContextProblemCode,
    ContextTransaction, ContextTransactionDraft, ContextTransactionReceipt,
    ContextTransactionStatus, check_context_patch, context_digest, context_has_reference,
    inspect_context,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionIdentity {
    version: u32,
    operation: OperationId,
    patch_digest: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TransactionRecord {
    pub patch_digest: String,
    pub status: ContextTransactionStatus,
}

pub(super) fn patch_digest(patch: &ContextPatch) -> String {
    context_digest(&serde_json::to_vec(patch).expect("context patch serialization"))
}

pub(super) fn is_committed(record: &ReceiptRecord) -> bool {
    record.transaction.as_ref().is_none_or(|transaction| {
        matches!(transaction.status, ContextTransactionStatus::Committed(_))
    })
}

pub(super) fn validate_record(c: &Connection, record: &ReceiptRecord) -> Result<()> {
    let Some(transaction) = &record.transaction else {
        return Ok(());
    };
    let draft: ContextTransactionDraft = serde_json::from_str(&record.candidate)?;
    if transaction.patch_digest != patch_digest(&draft.patch) {
        return Err(ContextError::UnsupportedState.into());
    }
    let Some((hash, terminal)) = terminal::exact_terminal(c, &record.operation)? else {
        return Err(ContextError::UnsupportedState.into());
    };
    if hash != record.output {
        return Err(ContextError::UnsupportedState.into());
    }
    let valid = match &transaction.status {
        ContextTransactionStatus::Committed(receipt) => {
            terminal == TerminalOutcome::Success
                && receipt.head == record.receipt.head
                && receipt.generation == record.receipt.generation
                && receipt.changed == record.receipt.changed
        }
        ContextTransactionStatus::Rejected(problems) => {
            !problems.is_empty()
                && matches!(
                    terminal,
                    TerminalOutcome::Success | TerminalOutcome::Failure(_)
                )
        }
        ContextTransactionStatus::Cancelled => matches!(
            terminal,
            TerminalOutcome::Cancelled | TerminalOutcome::CancelledWithReceipt(_)
        ),
        _ => false,
    };
    if !valid {
        return Err(ContextError::UnsupportedState.into());
    }
    Ok(())
}

fn problem(
    code: ContextProblemCode,
    reference: Option<ContextReference>,
    detail: impl Into<String>,
) -> ContextProblem {
    ContextProblem {
        code,
        reference,
        detail: detail.into(),
    }
}

fn validate_snapshot(store: &Store, snapshot: &ContextSnapshot) -> Result<()> {
    if snapshot.store_id != store.store_id
        || snapshot.operation != snapshot.seal.operation
        || snapshot.head != snapshot.seal.head
        || snapshot.generation != snapshot.seal.generation
    {
        return Err(ContextError::InvalidReference.into());
    }
    Ok(())
}

fn authorized_document(snapshot: &ContextSnapshot) -> ContextDocument {
    ContextDocument {
        blocks: snapshot
            .blocks
            .iter()
            .map(|block| block.block.clone())
            .collect(),
    }
}

/// The retained cut grants possible targets; current policy is checked separately.
fn patch_authority(snapshot: &ContextSnapshot, patch: &ContextPatch) -> Vec<ContextProblem> {
    let document = authorized_document(snapshot);
    let inspection = inspect_context(&document);
    let mut problems = Vec::new();
    for change in &patch.changes {
        let denied = match change {
            ContextChange::ReplaceContextText {
                reference,
                selector,
                ..
            } => (!inspection
                .text_versions
                .iter()
                .any(|version| &version.reference == reference && &version.selector == selector))
            .then_some(std::slice::from_ref(reference)),
            ContextChange::RemoveContextUnit { reference, .. } => (!inspection
                .unit_versions
                .iter()
                .any(|version| &version.reference == reference))
            .then_some(std::slice::from_ref(reference)),
            ContextChange::AppendContextNote { sources, .. } => {
                for reference in sources {
                    if !context_has_reference(&document, reference) {
                        problems.push(problem(
                            ContextProblemCode::Denied,
                            Some(reference.clone()),
                            "note source is outside the invocation's authorized before-call cut",
                        ));
                    }
                }
                None
            }
        };
        for reference in denied.into_iter().flatten() {
            problems.push(problem(
                ContextProblemCode::Denied,
                Some(reference.clone()),
                "target is outside the invocation's authorized editable before-call cut",
            ));
        }
    }
    problems
}

fn validate_head(c: &Connection, snapshot: &ContextSnapshot, head: &RequestId) -> Result<()> {
    if branch(c, head)? != snapshot.operation.origin.actor().0 {
        return Err(StoreError::OperationOriginMismatch);
    }
    if let ConversationIdentity::Embedded {
        run,
        actor,
        incarnation,
    } = &snapshot.operation.origin
    {
        let frontier = embedded_round::frontier(
            c,
            &crate::embedding::HostIdentity {
                run: run.clone(),
                actor: actor.clone(),
                incarnation: incarnation.clone(),
            },
        )?;
        if frontier
            .pending_head
            .as_ref()
            .or(frontier.settled_head.as_ref())
            != Some(head)
        {
            return Err(ContextError::Conflict.into());
        }
    }
    let descendant: bool = c.query_row("WITH RECURSIVE lineage(id,parent_id) AS (SELECT id,parent_id FROM requests WHERE id=?1 UNION ALL SELECT r.id,r.parent_id FROM requests r JOIN lineage l ON r.id=l.parent_id) SELECT EXISTS(SELECT 1 FROM lineage WHERE id=?2)", params![head.0, snapshot.head.0], |row| row.get(0))?;
    if !descendant {
        return Err(ContextError::Conflict.into());
    }
    Ok(())
}

fn refresh_snapshot(
    c: &Connection,
    snapshot: &ContextSnapshot,
    head: &RequestId,
) -> Result<ContextSnapshot> {
    if branch(c, head)? != snapshot.operation.origin.actor().0 {
        return Err(StoreError::OperationOriginMismatch);
    }
    let origin = original_call(c, &snapshot.operation)?;
    let all = history(c, head, true)?;
    let cuts = all
        .iter()
        .enumerate()
        .filter(|(_, item)| item.origin == origin)
        .map(|(n, _)| n)
        .collect::<Vec<_>>();
    let [cut] = cuts.as_slice() else {
        return Err(ContextError::MissingCall.into());
    };
    let stored = blocks(c, &all, *cut)?;
    let generation = state(c, &snapshot.operation.origin)?.generation;
    Ok(ContextSnapshot {
        document: ContextDocument {
            blocks: stored.iter().map(|stored| stored.block.clone()).collect(),
        },
        head: head.clone(),
        generation,
        operation: snapshot.operation.clone(),
        prefix: all[..*cut].to_vec(),
        blocks: stored,
        store_id: snapshot.store_id.clone(),
        seal: SnapshotSeal {
            operation: snapshot.operation.clone(),
            head: head.clone(),
            generation,
        },
    })
}

fn semantic_problem(error: &ContextError) -> Option<ContextProblem> {
    let code = match error {
        ContextError::UnsupportedState
        | ContextError::MissingCall
        | ContextError::Replay(_)
        | ContextError::Cancelled => return None,
        ContextError::Conflict => ContextProblemCode::Stale,
        ContextError::InvalidReference => ContextProblemCode::Missing,
        ContextError::Ineligible => ContextProblemCode::Ineligible,
        ContextError::InvalidModel => ContextProblemCode::InvalidPatch,
        _ => ContextProblemCode::Denied,
    };
    Some(problem(code, None, error.to_string()))
}

fn record_unapplied(
    tx: &Transaction<'_>,
    snapshot: &ContextSnapshot,
    head: &RequestId,
    draft: &ContextTransactionDraft,
    output: &JobOutput,
    status: ContextTransactionStatus,
    publish: bool,
) -> Result<ContextTransactionStatus> {
    let call = super::super::validation::invocation_item(
        tx,
        &snapshot.operation.request,
        &snapshot.operation.call,
    )?
    .ok_or(ContextError::MissingCall)?;
    let output_item = Item::tool_output(&snapshot.operation.call, call.input.kind(), output);
    let hash = Store::put_item_tx(tx, &output_item)?;
    let terminal = TerminalOutcome::from(output);
    let previous_terminal = terminal::exact_terminal(tx, &snapshot.operation)?;
    if let Some((old, old_terminal)) = &previous_terminal {
        if *old != hash || *old_terminal != terminal {
            return Err(StoreError::ConflictingReplayOutcome {
                operation: snapshot.operation.clone(),
            });
        }
    }
    let settled = tx.execute("UPDATE claims SET state='settled',output_hash=?4,terminal_json=?5 WHERE origin=?1 AND origin_request_id=?2 AND call_id=?3 AND state='pending'",params![serde_json::to_string(&snapshot.operation.origin)?,snapshot.operation.request.0,snapshot.operation.call.0,hash.0,serde_json::to_string(&terminal)?])?;
    if settled == 0 && previous_terminal.is_none() {
        return Err(StoreError::UnsupportedReplayOutcome {
            operation: snapshot.operation.clone(),
        });
    }
    if publish {
        super::super::output_publication::append_tx(
            tx,
            &snapshot.operation,
            &snapshot.operation.request,
            head,
        )?;
    }
    let state = state(tx, &snapshot.operation.origin)?;
    let record = ReceiptRecord {
        version: 5,
        transaction: Some(TransactionRecord {
            patch_digest: patch_digest(&draft.patch),
            status: status.clone(),
        }),
        next_effort: None,
        deferred_head: head.clone(),
        prefix: Vec::new(),
        candidate: serde_json::to_string(draft)?,
        original_operation: None,
        operation: snapshot.operation.clone(),
        output: hash,
        receipt: ContextCommitReceipt {
            head: head.clone(),
            generation: state.generation,
            changed: false,
            model: state.model,
        },
    };
    tx.execute(
        "INSERT INTO events(request_id,kind,payload,created_at) VALUES(?1,'context_commit',?2,?3)",
        params![head.0, serde_json::to_string(&record)?, utc_millis()],
    )?;
    Ok(status)
}

impl Store {
    pub fn inspect_context(&self, snapshot: &ContextSnapshot) -> Result<ContextInspection> {
        validate_snapshot(self, snapshot)?;
        let c = self.lock()?;
        validate_identity(&c, &self.store_id, &snapshot.operation.origin)?;
        Ok(inspect_context(&authorized_document(snapshot)))
    }

    /// Handles contain exact identity, not authority or a second staging registry.
    pub fn context_transaction(
        &self,
        snapshot: &ContextSnapshot,
        patch: &ContextPatch,
    ) -> Result<ContextTransaction> {
        validate_snapshot(self, snapshot)?;
        let c = self.lock()?;
        validate_identity(&c, &self.store_id, &snapshot.operation.origin)?;
        Ok(ContextTransaction {
            raw: serde_json::to_string(&TransactionIdentity {
                version: 1,
                operation: snapshot.operation.clone(),
                patch_digest: patch_digest(patch),
            })?,
        })
    }

    pub fn context_transaction_status(
        &self,
        identity: &ConversationIdentity,
        handle: &ContextTransaction,
    ) -> Result<ContextTransactionStatus> {
        let c = self.lock()?;
        validate_identity(&c, &self.store_id, identity)?;
        let Ok(handle) = serde_json::from_str::<TransactionIdentity>(&handle.raw) else {
            return Ok(ContextTransactionStatus::Unknown(
                "invalid transaction handle".into(),
            ));
        };
        if handle.version != 1 || handle.operation.origin != *identity {
            return Ok(ContextTransactionStatus::Unknown(
                "transaction is outside this conversation".into(),
            ));
        }
        let Some(record) = receipt(&c, &handle.operation)? else {
            return Ok(ContextTransactionStatus::Unknown(
                "no durable transaction outcome".into(),
            ));
        };
        Ok(match record.transaction {
            Some(transaction) if transaction.patch_digest == handle.patch_digest => {
                transaction.status
            }
            _ => ContextTransactionStatus::Unknown("transaction candidate does not match".into()),
        })
    }

    /// Read canonical visible text at an exact retained occurrence, never an overlay
    /// or a broader history browser. The private invocation cut carries authority.
    pub fn original_context_text(
        &self,
        snapshot: &ContextSnapshot,
        reference: &ContextReference,
        selector: &ContextTextSelector,
    ) -> Result<std::result::Result<String, ContextProblem>> {
        validate_snapshot(self, snapshot)?;
        let c = self.lock()?;
        validate_identity(&c, &self.store_id, &snapshot.operation.origin)?;
        let origins = match serde_json::from_str::<Vec<Origin>>(reference.as_str()) {
            Ok(origins) if origins.len() == 1 => origins,
            _ => {
                return Ok(Err(problem(
                    ContextProblemCode::Missing,
                    Some(reference.clone()),
                    "reference does not name one retained canonical occurrence",
                )));
            }
        };
        let named = &origins[0];
        let owner: Option<String> = c
            .query_row(
                "SELECT branch FROM requests WHERE id=?1",
                [&named.request.0],
                |row| row.get(0),
            )
            .optional()?;
        match owner {
            None => {
                return Ok(Err(problem(
                    ContextProblemCode::Missing,
                    Some(reference.clone()),
                    "canonical source request is unavailable",
                )));
            }
            Some(owner) if owner != snapshot.operation.origin.actor().0 => {
                return Ok(Err(problem(
                    ContextProblemCode::Denied,
                    Some(reference.clone()),
                    "canonical source belongs to another conversation",
                )));
            }
            _ => {}
        }
        let call = original_call(&c, &snapshot.operation)?;
        // An active compaction boundary can hide removed content; canonical
        // ancestry before the exact issuing call remains read-only authority.
        let retained = retained_history(&c, &snapshot.operation.request, false)?;
        let Some(cut) = retained.iter().position(|item| item.origin == call) else {
            return Err(ContextError::MissingCall.into());
        };
        let mut origins = HashSet::new();
        let items = retained[..cut]
            .iter()
            .filter(|item| {
                super::reference(std::slice::from_ref(*item)) == *reference
                    && origins.insert(item.origin.clone())
            })
            .collect::<Vec<_>>();
        let item = match items.as_slice() {
            [] => {
                let ancestry: bool = c.query_row("WITH RECURSIVE lineage(id,parent_id) AS (SELECT id,parent_id FROM requests WHERE id=?1 UNION ALL SELECT r.id,r.parent_id FROM requests r JOIN lineage l ON r.id=l.parent_id) SELECT EXISTS(SELECT 1 FROM lineage WHERE id=?2)", params![snapshot.operation.request.0, named.request.0], |row| row.get(0))?;
                let before_call = named.request != call.request || named.position < call.position;
                let code = if ancestry && before_call {
                    ContextProblemCode::Missing
                } else {
                    ContextProblemCode::Denied
                };
                return Ok(Err(problem(
                    code,
                    Some(reference.clone()),
                    "exact occurrence is unavailable in the canonical before-call ancestry",
                )));
            }
            [item] => *item,
            _ => {
                return Ok(Err(problem(
                    ContextProblemCode::Ambiguous,
                    Some(reference.clone()),
                    "reference names more than one canonical occurrence",
                )));
            }
        };
        Ok(text_body(&item.item, selector)
            .map(str::to_owned)
            .ok_or_else(|| {
                problem(
                    ContextProblemCode::UnsupportedSelector,
                    Some(reference.clone()),
                    "selector does not name supported canonical visible text",
                )
            }))
    }

    pub fn commit_context_transaction_guarded(
        &self,
        snapshot: &ContextSnapshot,
        draft: &ContextTransactionDraft,
        output: &JobOutput,
        pending: &[OperationId],
        cancelled: impl Fn() -> bool,
    ) -> Result<ContextTransactionStatus> {
        self.commit_context_transaction_at_guarded(
            snapshot,
            &snapshot.head,
            draft,
            output,
            pending,
            cancelled,
        )
    }

    pub fn commit_context_transaction_at_guarded(
        &self,
        snapshot: &ContextSnapshot,
        head: &RequestId,
        draft: &ContextTransactionDraft,
        output: &JobOutput,
        pending: &[OperationId],
        cancelled: impl Fn() -> bool,
    ) -> Result<ContextTransactionStatus> {
        self.settle_context_transaction_guarded(
            snapshot, head, draft, output, pending, false, cancelled,
        )
    }

    pub fn context_transaction_head(&self, operation: &OperationId) -> Result<Option<RequestId>> {
        Ok(receipt(&*self.lock()?, operation)?
            .filter(|record| record.transaction.is_some())
            .map(|record| record.receipt.head))
    }

    pub fn discard_context_transaction(
        &self,
        snapshot: &ContextSnapshot,
        draft: &ContextTransactionDraft,
        output: &JobOutput,
    ) -> Result<ContextTransactionStatus> {
        self.discard_context_transaction_at(snapshot, &snapshot.head, draft, output)
    }

    pub fn discard_context_transaction_at(
        &self,
        snapshot: &ContextSnapshot,
        head: &RequestId,
        draft: &ContextTransactionDraft,
        output: &JobOutput,
    ) -> Result<ContextTransactionStatus> {
        self.settle_context_transaction_guarded(snapshot, head, draft, output, &[], true, || false)
    }

    pub(crate) fn retain_context_transaction_output(
        &self,
        operation: &OperationId,
        draft: &ContextTransactionDraft,
        output: &JobOutput,
        head: &RequestId,
    ) -> Result<ContextTransactionStatus> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            validate_identity(tx, &self.store_id, &operation.origin)?;
            let call =
                super::super::validation::invocation_item(tx, &operation.request, &operation.call)?
                    .ok_or(ContextError::MissingCall)?;
            let item = Item::tool_output(&operation.call, call.input.kind(), output);
            let hash = Self::put_item_tx(tx, &item)?;
            if let Some(record) = receipt(tx, operation)? {
                if record.candidate != serde_json::to_string(draft)? || record.output != hash {
                    return Err(StoreError::ConflictingReplayOutcome {
                        operation: operation.clone(),
                    });
                }
                return record
                    .transaction
                    .map(|record| record.status)
                    .ok_or_else(|| ContextError::UnsupportedState.into());
            }
            if matches!(
                output,
                JobOutput::Interrupted | JobOutput::CancellationUnconfirmed(_)
            ) {
                return Err(ContextError::Ineligible.into());
            }
            let generation = state(tx, &operation.origin)?.generation;
            let snapshot = ContextSnapshot {
                document: ContextDocument::default(),
                generation,
                head: head.clone(),
                operation: operation.clone(),
                prefix: Vec::new(),
                blocks: Vec::new(),
                store_id: self.store_id.clone(),
                seal: SnapshotSeal {
                    operation: operation.clone(),
                    head: head.clone(),
                    generation,
                },
            };
            let status = if matches!(
                output,
                JobOutput::Cancelled | JobOutput::CancelledWithReceipt(_)
            ) {
                ContextTransactionStatus::Cancelled
            } else {
                ContextTransactionStatus::Rejected(vec![problem(
                    ContextProblemCode::InvalidPatch,
                    None,
                    "context candidate was discarded before publication",
                )])
            };
            record_unapplied(tx, &snapshot, head, draft, output, status, false)
        })
    }

    fn settle_context_transaction_guarded(
        &self,
        snapshot: &ContextSnapshot,
        head: &RequestId,
        draft: &ContextTransactionDraft,
        output: &JobOutput,
        pending: &[OperationId],
        discarded: bool,
        cancelled: impl Fn() -> bool,
    ) -> Result<ContextTransactionStatus> {
        validate_snapshot(self, snapshot)?;
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            validate_identity(tx, &self.store_id, &snapshot.operation.origin)?;
            let candidate = serde_json::to_string(draft)?;
            let call = super::super::validation::invocation_item(
                tx,
                &snapshot.operation.request,
                &snapshot.operation.call,
            )?
            .ok_or(ContextError::MissingCall)?;
            let output_item =
                Item::tool_output(&snapshot.operation.call, call.input.kind(), output);
            let hash = Store::put_item_tx(tx, &output_item)?;
            // Idempotence precedes current content, cancellation and frontier checks.
            if let Some(record) = receipt(tx, &snapshot.operation)? {
                if record.candidate != candidate || record.output != hash {
                    return Err(StoreError::ConflictingReplayOutcome {
                        operation: snapshot.operation.clone(),
                    });
                }
                return record
                    .transaction
                    .map(|transaction| transaction.status)
                    .ok_or_else(|| ContextError::UnsupportedState.into());
            }
            validate_head(tx, snapshot, head)?;
            if matches!(
                output,
                JobOutput::Cancelled | JobOutput::CancelledWithReceipt(_)
            ) {
                return record_unapplied(
                    tx,
                    snapshot,
                    head,
                    draft,
                    output,
                    ContextTransactionStatus::Cancelled,
                    true,
                );
            }
            if matches!(
                output,
                JobOutput::Interrupted | JobOutput::CancellationUnconfirmed(_)
            ) {
                return Err(ContextError::Ineligible.into());
            }
            if discarded || !matches!(output, JobOutput::Completed(Ok(_))) {
                return record_unapplied(
                    tx,
                    snapshot,
                    head,
                    draft,
                    output,
                    ContextTransactionStatus::Rejected(vec![problem(
                        ContextProblemCode::Ineligible,
                        None,
                        "invocation did not complete with eligible context publication",
                    )]),
                    true,
                );
            }
            if cancelled() {
                return Err(ContextError::Cancelled.into());
            }
            let mut problems = patch_authority(snapshot, &draft.patch);
            let refreshed = refresh_snapshot(tx, snapshot, head);
            let refreshed = match refreshed {
                Ok(snapshot) => snapshot,
                Err(StoreError::Context(error)) => match semantic_problem(&error) {
                    Some(error) => {
                        return record_unapplied(
                            tx,
                            snapshot,
                            head,
                            draft,
                            output,
                            ContextTransactionStatus::Rejected(vec![error]),
                            true,
                        );
                    }
                    None => return Err(error.into()),
                },
                Err(error) => return Err(error),
            };
            let document = match check_context_patch(&refreshed.document, &draft.patch) {
                Ok(document) if problems.is_empty() => document,
                other => {
                    if let Err(errors) = other {
                        problems.extend(errors);
                    }
                    return record_unapplied(
                        tx,
                        snapshot,
                        head,
                        draft,
                        output,
                        ContextTransactionStatus::Rejected(problems),
                        true,
                    );
                }
            };
            let legacy_draft = ContextDraft {
                document,
                next_model: draft.next_model.clone(),
                next_effort: draft.next_effort,
            };
            tx.execute_batch("SAVEPOINT context_transaction_apply")?;
            let applied = self.commit_context_tx(
                tx,
                ContextCommit {
                    snapshot: &refreshed,
                    draft: &legacy_draft,
                    output,
                    pending,
                },
                None,
                &cancelled,
                Some(draft),
            );
            match applied {
                Ok(_) => {
                    tx.execute_batch("RELEASE context_transaction_apply")?;
                    receipt(tx, &snapshot.operation)?
                        .and_then(|record| record.transaction)
                        .map(|transaction| transaction.status)
                        .ok_or_else(|| ContextError::UnsupportedState.into())
                }
                Err(StoreError::Context(ContextError::Cancelled)) => {
                    Err(ContextError::Cancelled.into())
                }
                Err(StoreError::Context(error)) => {
                    let Some(error) = semantic_problem(&error) else {
                        return Err(error.into());
                    };
                    tx.execute_batch(
                        "ROLLBACK TO context_transaction_apply; RELEASE context_transaction_apply",
                    )?;
                    record_unapplied(
                        tx,
                        snapshot,
                        head,
                        draft,
                        output,
                        ContextTransactionStatus::Rejected(vec![error]),
                        true,
                    )
                }
                Err(error) => Err(error),
            }
        })
    }
}

#[cfg(test)]
mod tests;
