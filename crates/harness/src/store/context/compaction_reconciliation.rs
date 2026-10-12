//! The external Item-only result crosses into typed canonical occurrences here.
use super::*;
use crate::store::CompactionOwnershipRefusal;

pub(crate) enum CompactionItem {
    Retained(Occurrence),
    Authored(Item),
}

pub(crate) fn reconcile(
    c: &Connection,
    target: &RequestId,
    source: &RequestId,
    items: &[Item],
    retained: &[(usize, Occurrence)],
    issued: &[Option<Occurrence>],
    pending: &[OperationId],
) -> Result<Vec<CompactionItem>> {
    let fail = |position, reason| StoreError::CompactionOwnership {
        request: target.clone(),
        position,
        reason,
    };
    let requests = issued
        .iter()
        .flatten()
        .map(|occurrence| occurrence.request.clone())
        .collect();
    let persisted = occurrences_for_requests(c, &requests)?
        .into_iter()
        .map(|occurrence| {
            (
                (occurrence.request.clone(), occurrence.position),
                occurrence,
            )
        })
        .collect::<HashMap<_, _>>();
    let visible = history(c, source, true)?
        .into_iter()
        .map(|occurrence| (occurrence.request, occurrence.position))
        .collect::<HashSet<_>>();
    let mut sources = HashMap::<Origin, &Occurrence>::new();
    for occurrence in issued.iter().flatten() {
        let key = (occurrence.request.clone(), occurrence.position);
        if !visible.contains(&key) || persisted.get(&key) != Some(occurrence) {
            return Err(fail(0, CompactionOwnershipRefusal::MissingSource));
        }
        if sources
            .insert(occurrence.origin.clone(), occurrence)
            .is_some()
        {
            return Err(fail(0, CompactionOwnershipRefusal::DuplicateSource));
        }
    }
    validate_canonical_history(c, &issued.iter().flatten().cloned().collect::<Vec<_>>())?;
    let mut selections = HashMap::new();
    let mut forced_origins = HashSet::new();
    for (position, selected) in retained {
        let Some(original) = sources.get(&selected.origin) else {
            return Err(fail(*position, CompactionOwnershipRefusal::MissingSource));
        };
        if **original != *selected
            || items.get(*position) != Some(&project_context_note(selected)?)
            || selections.insert(*position, selected).is_some()
            || !forced_origins.insert(selected.origin.clone())
        {
            return Err(fail(*position, CompactionOwnershipRefusal::DuplicateSource));
        }
    }
    let pending_invocations = super::super::validation::invocations_for_operations(
        c,
        &pending.iter().cloned().collect(),
    )?;
    let pending_origins = pending_invocations
        .values()
        .map(|invocation| invocation.occurrence.clone())
        .collect::<HashSet<_>>();
    let mut candidates = HashMap::<ItemHash, Vec<&Occurrence>>::new();
    for source in sources.values().copied() {
        for hash in [
            source.hash.clone(),
            Store::put_item_tx_hash(&project_context_note(source)?)?,
        ] {
            let matches = candidates.entry(hash).or_default();
            if !matches
                .iter()
                .any(|candidate| candidate.origin == source.origin)
            {
                matches.push(source);
            }
        }
    }
    let forced_pending = forced_origins
        .intersection(&pending_origins)
        .cloned()
        .collect::<HashSet<_>>();
    let mut consumed = forced_origins;
    let mut reconciled = Vec::new();
    for (position, item) in items.iter().enumerate() {
        if let Some(source) = selections.get(&position) {
            reconciled.push(CompactionItem::Retained((**source).clone()));
            continue;
        }
        let hash = Store::put_item_tx_hash(item)?;
        match candidates.get(&hash).map(Vec::as_slice).unwrap_or_default() {
            [source] => {
                if forced_pending.contains(&source.origin) {
                    // The exact pending source is already retained by the internal
                    // selection. Its unambiguous external echo adds no second copy.
                    continue;
                }
                if !consumed.insert(source.origin.clone()) {
                    if ownership_bearing(item) {
                        return Err(fail(position, CompactionOwnershipRefusal::DuplicateSource));
                    }
                    reconciled.push(CompactionItem::Authored(item.clone()));
                } else {
                    reconciled.push(CompactionItem::Retained((**source).clone()));
                }
            }
            [] => {
                if ownership_bearing(item) {
                    return Err(fail(position, CompactionOwnershipRefusal::UnownedToolItem));
                }
                reconciled.push(CompactionItem::Authored(item.clone()));
            }
            _ if ownership_bearing(item) => {
                return Err(fail(
                    position,
                    CompactionOwnershipRefusal::AmbiguousSource(hash),
                ));
            }
            _ => reconciled.push(CompactionItem::Authored(item.clone())),
        }
    }
    Ok(reconciled)
}

fn ownership_bearing(item: &Item) -> bool {
    matches!(
        item.0["type"].as_str(),
        Some(
            "function_call"
                | "custom_tool_call"
                | "function_call_output"
                | "custom_tool_call_output"
        )
    )
}
