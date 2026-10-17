//! Authored context membership reuses exact invocation and publication evidence.
use super::*;
use crate::context::{
    ContextChange, ContextPatch, RetainedExchangeObservation, RetainedExchangeReference,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangeIdentity {
    version: u32,
    store: String,
    operation: OperationId,
    call: Origin,
    output: Origin,
}

/// The private invocation cut grants read authority to a canonical pre-call
/// occurrence even after its active context representation has been removed.
fn authorized_call(
    c: &Connection,
    snapshot: &ContextSnapshot,
    reference: &ContextReference,
) -> Result<std::result::Result<Occurrence, RetainedExchangeObservation>> {
    let Ok(origins) = serde_json::from_str::<Vec<Origin>>(reference.as_str()) else {
        return Ok(Err(RetainedExchangeObservation::Denied));
    };
    if origins.is_empty() || origins.iter().collect::<HashSet<_>>().len() != origins.len() {
        return Ok(Err(RetainedExchangeObservation::Denied));
    }
    let original = original_call(c, &snapshot.operation)?;
    let ancestry = retained_history(c, &snapshot.operation.request, false)?;
    let Some(cut) = ancestry.iter().position(|item| item.origin == original) else {
        return Err(ContextError::MissingCall.into());
    };
    let mut calls = Vec::new();
    for named in origins {
        let mut matches = ancestry[..cut].iter().filter(|item| item.origin == named);
        let Some(item) = matches.next() else {
            return Ok(Err(RetainedExchangeObservation::Denied));
        };
        if matches
            .any(|other| other.item != item.item || other.output_operation != item.output_operation)
        {
            return Err(ContextError::UnsupportedState.into());
        }
        if item
            .item
            .tool_call()
            .map_err(|_| ContextError::UnsupportedState)?
            .is_some()
        {
            calls.push(item);
        }
    }
    let [call] = calls.as_slice() else {
        return Ok(Err(RetainedExchangeObservation::Missing));
    };
    let mut call = (*call).clone();
    call.overlays.clear();
    Ok(Ok(call))
}

fn resolve(
    c: &Connection,
    snapshot: &ContextSnapshot,
    call_reference: &ContextReference,
) -> Result<std::result::Result<(OperationId, Vec<Occurrence>), RetainedExchangeObservation>> {
    let call = match authorized_call(c, snapshot, call_reference)? {
        Ok(call) => call,
        Err(state) => return Ok(Err(state)),
    };
    let Some(operation) = super::super::validation::operation_for_occurrence(c, &call)? else {
        return Ok(Err(RetainedExchangeObservation::Missing));
    };
    let observed = match terminal::observed_outcome(c, &operation)? {
        None => RetainedExchangeObservation::Pending,
        Some(TerminalOutcome::Success) => {
            let (hash, _, _) =
                terminal::exact_terminal(c, &operation)?.ok_or(ContextError::UnsupportedState)?;
            // Choose the first canonical publication, independent of later
            // context copies. Its retained row identity remains stable.
            let publication: Option<(String, i64)> = c.query_row(
                "SELECT request_id,position FROM request_items WHERE output_operation=?1 AND source_request IS NULL ORDER BY rowid LIMIT 1",
                [serde_json::to_string(&operation)?], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            let Some((request, position)) = publication else {
                return Ok(Err(RetainedExchangeObservation::Unpublished));
            };
            let outputs =
                occurrences_for_requests(c, &HashSet::from([RequestId(request.clone())]))?;
            let Some(output) = outputs
                .into_iter()
                .find(|item| item.position == position && item.request.0 == request)
            else {
                return Err(ContextError::UnsupportedState.into());
            };
            if output.hash != hash
                || output.output_operation.as_ref() != Some(&operation)
                || output.origin.request != output.request
                || output.origin.position != output.position
            {
                return Err(ContextError::UnsupportedState.into());
            }
            let items = vec![call, output];
            super::super::output_publication::validate_history(c, &items)?;
            return Ok(Ok((operation, items)));
        }
        Some(TerminalOutcome::NotStarted) => RetainedExchangeObservation::NotStarted,
        Some(TerminalOutcome::Failure(_)) => RetainedExchangeObservation::Failure,
        Some(TerminalOutcome::Cancelled) => RetainedExchangeObservation::Cancelled,
        Some(
            TerminalOutcome::CancelledWithReceipt(_) | TerminalOutcome::CancelledWithValueReceipt,
        ) => RetainedExchangeObservation::CancelledWithReceipt,
        Some(TerminalOutcome::Interrupted) => RetainedExchangeObservation::Interrupted,
        Some(TerminalOutcome::CancellationUnconfirmed(_)) => {
            RetainedExchangeObservation::CancellationUnconfirmed
        }
    };
    Ok(Err(observed))
}

impl Store {
    pub fn observe_retained_exchange(
        &self,
        snapshot: &ContextSnapshot,
        call: &ContextReference,
    ) -> Result<RetainedExchangeObservation> {
        transactions::validate_snapshot(self, snapshot)?;
        let mut c = self.lock()?;
        c.read_transaction(|tx| {
            validate_identity(tx, &self.store_id, &snapshot.operation.origin)?;
            Ok(match resolve(tx, snapshot, call)? {
                Ok((operation, items)) => RetainedExchangeObservation::Available(
                    RetainedExchangeReference(serde_json::to_string(&ExchangeIdentity {
                        version: 1,
                        store: self.store_id.clone(),
                        operation,
                        call: items[0].origin.clone(),
                        output: items[1].origin.clone(),
                    })?),
                ),
                Err(state) => state,
            })
        })
    }
}

pub(super) fn qualified_items(
    c: &Connection,
    snapshot: &ContextSnapshot,
    source: &RetainedExchangeReference,
) -> Result<Vec<Occurrence>> {
    let identity: ExchangeIdentity =
        serde_json::from_str(&source.0).map_err(|_| ContextError::InvalidReference)?;
    if identity.version != 1 || identity.store != snapshot.store_id {
        return Err(ContextError::InvalidReference.into());
    }
    let call_reference = ContextReference(serde_json::to_string(&vec![identity.call.clone()])?);
    let (operation, items) =
        resolve(c, snapshot, &call_reference)?.map_err(|_| ContextError::Ineligible)?;
    if operation != identity.operation
        || items[0].origin != identity.call
        || items[1].origin != identity.output
    {
        return Err(ContextError::InvalidReference.into());
    }
    Ok(items)
}

pub(super) fn append_changes(
    c: &Connection,
    snapshot: &ContextSnapshot,
    patch: &ContextPatch,
    rewritten: &mut Vec<Occurrence>,
) -> Result<()> {
    for change in &patch.changes {
        let ContextChange::AppendRetainedExchange { source } = change else {
            continue;
        };
        let items = qualified_items(c, snapshot, source)?;
        let operation = items[1]
            .output_operation
            .as_ref()
            .ok_or(ContextError::UnsupportedState)?;
        let matching_calls = rewritten
            .iter()
            .filter(|item| item.origin == items[0].origin)
            .count();
        let matching_outputs = rewritten
            .iter()
            .filter(|item| {
                item.output_operation.as_ref() == Some(operation) && item.hash == items[1].hash
            })
            .count();
        match (matching_calls, matching_outputs) {
            (1, 1) => continue,
            (0, 0) => {}
            _ => return Err(ContextError::InvalidReference.into()),
        }
        // Never rename a conflicting call or pair it with another operation.
        if rewritten
            .iter()
            .any(|item| item.item.0["call_id"] == operation.call.0)
        {
            return Err(ContextError::InvalidReference.into());
        }
        rewritten.extend(items);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::{ContextTransactionDraft, ContextTransactionStatus},
        item::ToolKind,
        model::CallId,
        store::Usage,
    };

    fn invocation(id: &str) -> Item {
        Item(json!({"type":"function_call","call_id":id,"name":"fixture","arguments":"{}"}))
    }
    fn editor(store: &Store, parent: &RequestId, id: &str) -> ContextSnapshot {
        let head = RequestId(id.into());
        store
            .write_request(
                &head,
                Some(parent),
                "/root",
                &[invocation(id)],
                Usage::default(),
            )
            .unwrap();
        let operation = store.claim(&CallId(id.into()), &head).unwrap();
        store.begin_context(&operation, &head).unwrap()
    }
    fn fixture(
        output: Option<JobOutput>,
        published: bool,
    ) -> (Store, ContextSnapshot, ContextReference, OperationId) {
        let store = Store::memory().unwrap();
        let original = RequestId("original".into());
        store
            .write_request(
                &original,
                None,
                "/root",
                &[invocation("source")],
                Usage::default(),
            )
            .unwrap();
        let operation = store.claim(&CallId("source".into()), &original).unwrap();
        store
            .initialize_context_target(
                &operation.origin,
                &ContextProjectionTarget {
                    model: "fixture".into(),
                    route: crate::transport::ResponsesRoute::DeepSeek,
                    protocol: crate::transport::ResponsesProtocol::Standard,
                },
            )
            .unwrap();
        if let Some(output) = output {
            store
                .write_job_output(&operation, ToolKind::Function, &output)
                .unwrap();
            if published {
                store
                    .append_operation_output(&operation, &original, &original)
                    .unwrap();
            }
        }
        let snapshot = editor(&store, &original, "editor");
        let reference = snapshot
            .document
            .blocks
            .iter()
            .find_map(|block| match block {
                ContextBlock::Native { reference, .. } => Some(reference.clone()),
                _ => None,
            })
            .unwrap();
        (store, snapshot, reference, operation)
    }
    fn success() -> JobOutput {
        JobOutput::Completed(Ok(json!({"actual":"retained result"})))
    }
    fn commit(
        store: &Store,
        snapshot: &ContextSnapshot,
        changes: Vec<ContextChange>,
    ) -> crate::context::ContextTransactionReceipt {
        let draft = ContextTransactionDraft {
            patch: ContextPatch { changes },
            next_model: None,
            next_effort: None,
        };
        match store
            .commit_context_transaction_guarded(snapshot, &draft, &success(), &[], || false)
            .unwrap()
        {
            ContextTransactionStatus::Committed(receipt) => receipt,
            status => panic!("unexpected {status:?}"),
        }
    }
    #[test]
    fn observation_distinguishes_pending_unpublished_failure_and_cancellation() {
        let cases = [
            (None, false, RetainedExchangeObservation::Pending),
            (
                Some(success()),
                false,
                RetainedExchangeObservation::Unpublished,
            ),
            (
                Some(JobOutput::Completed(Err(
                    crate::provider::ToolFailure::from("failed"),
                ))),
                true,
                RetainedExchangeObservation::Failure,
            ),
            (
                Some(JobOutput::Cancelled),
                true,
                RetainedExchangeObservation::Cancelled,
            ),
            (
                Some(JobOutput::CancelledWithReceipt(Ok(json!("partial")))),
                true,
                RetainedExchangeObservation::CancelledWithReceipt,
            ),
        ];
        for (output, publish, expected) in cases {
            let (store, snapshot, reference, _) = fixture(output, publish);
            assert_eq!(
                store
                    .observe_retained_exchange(&snapshot, &reference)
                    .unwrap(),
                expected
            );
        }
    }
    #[test]
    fn completed_exchange_is_idempotent_and_can_restore_removed_context_without_execution() {
        let (store, snapshot, reference, operation) = fixture(Some(success()), true);
        let RetainedExchangeObservation::Available(source) = store
            .observe_retained_exchange(&snapshot, &reference)
            .unwrap()
        else {
            panic!("expected retained source")
        };
        let before = store.replay_tool_output_operation(&operation).unwrap();
        let first = commit(
            &store,
            &snapshot,
            vec![ContextChange::AppendRetainedExchange {
                source: source.clone(),
            }],
        );
        assert!(!first.changed);
        let second = editor(&store, &first.head, "second-editor");
        let removed = commit(
            &store,
            &second,
            vec![ContextChange::RemoveContextUnit {
                reference: reference.clone(),
                expected: crate::context::inspect_context(&second.document)
                    .unit_versions
                    .iter()
                    .find(|v| v.reference == reference)
                    .unwrap()
                    .digest
                    .clone(),
            }],
        );
        assert!(
            !store
                .context_history(&removed.head)
                .unwrap()
                .iter()
                .any(|(_, _, item)| item.0["call_id"] == "source")
        );
        let third = editor(&store, &removed.head, "third-editor");
        let RetainedExchangeObservation::Available(restored_source) =
            store.observe_retained_exchange(&third, &reference).unwrap()
        else {
            panic!("authorized removed source unavailable")
        };
        assert_eq!(source, restored_source);
        let restored = commit(
            &store,
            &third,
            vec![
                ContextChange::AppendRetainedExchange {
                    source: source.clone(),
                },
                ContextChange::AppendRetainedExchange { source },
            ],
        );
        let projected = store
            .context_request_state(&restored.head, &third.operation.origin)
            .unwrap();
        let source_items = projected
            .history
            .iter()
            .filter(|(_, _, item)| item.0["call_id"] == "source")
            .map(|(_, _, item)| item.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            source_items,
            vec![
                invocation("source"),
                Item::tool_output(&operation.call, ToolKind::Function, &success())
            ]
        );
        assert_eq!(
            store.replay_tool_output_operation(&operation).unwrap(),
            before
        );
    }
}
