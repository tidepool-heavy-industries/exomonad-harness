//! Operation ownership is issued with the durable output occurrence.
use super::{Result, Store, StoreError, terminal};
use crate::{
    context::Occurrence,
    item::{Item, ItemHash},
    model::{OperationId, RequestId},
};
use rusqlite::{Connection, Transaction, params};
use std::collections::{HashMap, HashSet};

pub(crate) struct PublishedOperationOutput {
    pub operation: OperationId,
    pub item: Item,
}

pub(crate) struct RecoveryItem {
    pub request: RequestId,
    pub source_request: RequestId,
    pub item: Item,
    pub output: Option<PublishedOperationOutput>,
}

pub(crate) struct AppendedOperationOutput {
    pub item: Item,
    pub hash: ItemHash,
    pub appended: bool,
}

enum ItemOwnership<'a> {
    Ordinary,
    OperationOutput(&'a OperationId),
    HistoricalUnboundOutput,
}

fn invalid(operation: &OperationId, claimant: &RequestId) -> StoreError {
    StoreError::InvalidOutputPublication {
        operation: operation.clone(),
        claimant: claimant.clone(),
    }
}

fn inconsistent(request: &RequestId, position: i64) -> StoreError {
    StoreError::InconsistentOutputPublication {
        request: request.clone(),
        position,
    }
}

pub(super) fn is_tool_output(item: &Item) -> bool {
    matches!(
        item.0["type"].as_str(),
        Some("function_call_output" | "custom_tool_call_output")
    )
}

pub(super) fn require_ordinary(item: &Item, request: &RequestId, position: i64) -> Result<()> {
    if is_tool_output(item) {
        return Err(StoreError::UnboundOutputPublication {
            request: request.clone(),
            position,
        });
    }
    Ok(())
}

fn ownership<'a>(
    item: &Item,
    operation: Option<&'a OperationId>,
    request: &RequestId,
    position: i64,
) -> Result<ItemOwnership<'a>> {
    match (is_tool_output(item), operation) {
        (false, None) => Ok(ItemOwnership::Ordinary),
        (false, Some(_)) => Err(inconsistent(request, position)),
        (true, None) => Ok(ItemOwnership::HistoricalUnboundOutput),
        (true, Some(operation)) if item.0["call_id"] == operation.call.0 => {
            Ok(ItemOwnership::OperationOutput(operation))
        }
        (true, Some(_)) => Err(inconsistent(request, position)),
    }
}

pub(super) fn validate_occurrence(occurrence: &Occurrence) -> Result<()> {
    ownership(
        &occurrence.item,
        occurrence.output_operation.as_ref(),
        &occurrence.request,
        occurrence.position,
    )
    .map(drop)
}

pub(super) struct RecoveryLedger {
    invocations: HashMap<OperationId, super::validation::IssuedInvocation>,
    sources: HashMap<(RequestId, i64), Occurrence>,
}

impl RecoveryLedger {
    fn read(c: &Connection, history: &[Occurrence]) -> Result<Self> {
        let operations = history
            .iter()
            .filter_map(|occurrence| occurrence.output_operation.as_ref())
            .cloned()
            .collect::<HashSet<_>>();
        let invocations = super::validation::invocations_for_operations(c, &operations)?;
        let requests = history
            .iter()
            .filter(|occurrence| occurrence.output_operation.is_some())
            .map(|occurrence| occurrence.origin.request.clone())
            .collect::<HashSet<_>>();
        let sources = super::context::occurrences_for_requests(c, &requests)?
            .into_iter()
            .map(|occurrence| {
                (
                    (occurrence.request.clone(), occurrence.position),
                    occurrence,
                )
            })
            .collect();
        Ok(Self {
            invocations,
            sources,
        })
    }
}

fn validated_output<'a>(
    occurrence: &'a Occurrence,
    ledger: &'a RecoveryLedger,
) -> Result<Option<(&'a OperationId, &'a Item)>> {
    match ownership(
        &occurrence.item,
        occurrence.output_operation.as_ref(),
        &occurrence.request,
        occurrence.position,
    )? {
        ItemOwnership::Ordinary => Ok(None),
        ItemOwnership::HistoricalUnboundOutput => Err(StoreError::UnboundOutputPublication {
            request: occurrence.request.clone(),
            position: occurrence.position,
        }),
        ItemOwnership::OperationOutput(operation) => {
            let invocation = ledger
                .invocations
                .get(operation)
                .ok_or_else(|| inconsistent(&occurrence.request, occurrence.position))?;
            let kind = invocation.call.input.kind();
            super::validate_replay_output(&operation.call, kind, &occurrence.item)?;
            let source = ledger
                .sources
                .get(&(
                    occurrence.origin.request.clone(),
                    occurrence.origin.position,
                ))
                .ok_or_else(|| inconsistent(&occurrence.request, occurrence.position))?;
            if source.hash != occurrence.origin.hash {
                return Err(inconsistent(&occurrence.request, occurrence.position));
            }
            match ownership(
                &source.item,
                source.output_operation.as_ref(),
                &source.request,
                source.position,
            )? {
                ItemOwnership::OperationOutput(owner) if owner == operation => {}
                _ => return Err(inconsistent(&occurrence.request, occurrence.position)),
            }
            super::validate_replay_output(&operation.call, kind, &source.item)?;
            Ok(Some((operation, &source.item)))
        }
    }
}

fn decode(occurrence: Occurrence, ledger: &RecoveryLedger) -> Result<RecoveryItem> {
    let output =
        validated_output(&occurrence, ledger)?.map(|(operation, item)| PublishedOperationOutput {
            operation: operation.clone(),
            item: item.clone(),
        });
    Ok(RecoveryItem {
        request: occurrence.request,
        source_request: occurrence.origin.request,
        item: occurrence.item,
        output,
    })
}

impl RecoveryLedger {
    pub(super) fn issuing_call(&self, operation: &OperationId) -> Option<&crate::context::Origin> {
        self.invocations
            .get(operation)
            .map(|invocation| &invocation.occurrence)
    }
}

pub(super) fn append_tx(
    tx: &Transaction<'_>,
    operation: &OperationId,
    claimant: &RequestId,
    request: &RequestId,
) -> Result<AppendedOperationOutput> {
    // Claimant authority is checked here, in the same transaction as publication.
    // The persistent fact is the issuing operation, shared by inherited claimants.
    let retained = terminal::replay_claim(tx, operation, claimant)?
        .ok_or_else(|| invalid(operation, claimant))?;
    let hash = Store::put_item_tx(tx, &retained.item)?;
    let owner = serde_json::to_string(operation)?;
    let previous = {
        let mut query = tx.prepare("SELECT item_hash FROM request_items WHERE request_id=?1 AND output_operation=?2 ORDER BY position LIMIT 2")?;
        query
            .query_map(params![request.0, owner], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    match previous.as_slice() {
        [] => {}
        [previous] if previous == &hash.0 => {
            return Ok(AppendedOperationOutput {
                item: retained.item,
                hash,
                appended: false,
            });
        }
        _ => return Err(invalid(operation, claimant)),
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM requests WHERE id=?1)",
        [&request.0],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(StoreError::MissingRequest(request.0.clone()));
    }
    let position: i64 = tx.query_row(
        "SELECT COALESCE(MAX(position)+1,0) FROM request_items WHERE request_id=?1",
        [&request.0],
        |row| row.get(0),
    )?;
    tx.execute("INSERT INTO request_items(request_id,position,item_hash,output_operation) VALUES (?1,?2,?3,?4)", params![request.0, position, hash.0, owner])?;
    super::chat::publish(tx, request, position, &hash, &retained.item)?;
    Ok(AppendedOperationOutput {
        item: retained.item,
        hash,
        appended: true,
    })
}

pub(super) fn validate_history(c: &Connection, history: &[Occurrence]) -> Result<RecoveryLedger> {
    let ledger = RecoveryLedger::read(c, history)?;
    for occurrence in history {
        validated_output(occurrence, &ledger)?;
    }
    Ok(ledger)
}

impl Store {
    /// Canonical retained output and its exact operation are one occurrence.
    /// An exact retry returns that occurrence instead of duplicating it.
    pub(crate) fn append_operation_output(
        &self,
        operation: &OperationId,
        claimant: &RequestId,
        request: &RequestId,
    ) -> Result<AppendedOperationOutput> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let output = append_tx(&tx, operation, claimant, request)?;
        tx.commit()?;
        Ok(output)
    }

    pub(crate) fn recovery_history(&self, head: &RequestId) -> Result<Vec<RecoveryItem>> {
        let c = self.lock();
        let history = super::context::history(&c, head, true)?;
        let ledger = RecoveryLedger::read(&c, &history)?;
        history
            .into_iter()
            .map(|occurrence| decode(occurrence, &ledger))
            .collect()
    }
}

#[cfg(test)]
mod tests;
