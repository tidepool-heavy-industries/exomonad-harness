//! Context cuts and terminal publication share the Store connection and transaction.
use super::{Result, Store, StoreError, TerminalOutcome, embedded_round, terminal, utc_millis};
use crate::{
    context::{
        ContextBlock, ContextCommit, ContextCommitEvidence, ContextCommitReceipt, ContextDocument,
        ContextDraft, ContextError, ContextNativeKind, ContextReference, ContextRequestState,
        ContextRole, ContextSnapshot, Occurrence, Origin, SnapshotSeal, StoredBlock,
    },
    item::{Item, ItemHash},
    model::{AgentPath, ConversationIdentity, OperationId, RequestId},
    turn::JobOutput,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};

#[derive(Default, Serialize, Deserialize)]
struct InferenceState {
    generation: u64,
    model: Option<String>,
}

fn state_key(identity: &ConversationIdentity) -> Result<String> {
    Ok(format!(
        "harness:context:{}",
        serde_json::to_string(identity)?
    ))
}

fn state(c: &Connection, identity: &ConversationIdentity) -> Result<InferenceState> {
    let raw: Option<String> = c
        .query_row(
            "SELECT state FROM session_state WHERE session_id=?1",
            [state_key(identity)?],
            |r| r.get(0),
        )
        .optional()?;
    match raw {
        None => Ok(InferenceState::default()),
        Some(raw) => {
            let value: serde_json::Value = serde_json::from_str(&raw)?;
            if value["version"] != 1 {
                return Err(ContextError::UnsupportedState.into());
            }
            Ok(serde_json::from_value(value["state"].clone())?)
        }
    }
}

fn save_state(
    tx: &Transaction<'_>,
    identity: &ConversationIdentity,
    state: &InferenceState,
) -> Result<()> {
    tx.execute("INSERT INTO session_state(session_id,state,updated_at) VALUES(?1,?2,?3) ON CONFLICT(session_id) DO UPDATE SET state=excluded.state,updated_at=excluded.updated_at", params![state_key(identity)?, json!({"version":1,"state":state}).to_string(),utc_millis()])?;
    Ok(())
}

pub(super) fn identity_for_branch(
    c: &Connection,
    store_id: &str,
    branch: &str,
) -> Result<ConversationIdentity> {
    let binding: Option<(String, String)> = c
        .query_row(
            "SELECT run_id,incarnation FROM embedded_bindings WHERE agent_path=?1",
            [branch],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match binding {
        Some((run, incarnation)) => ConversationIdentity::Embedded {
            run,
            incarnation,
            actor: AgentPath(branch.into()),
        },
        None => ConversationIdentity::Standalone {
            store: store_id.into(),
            actor: AgentPath(branch.into()),
        },
    })
}

fn validate_identity(
    c: &Connection,
    store_id: &str,
    identity: &ConversationIdentity,
) -> Result<()> {
    if identity_for_branch(c, store_id, &identity.actor().0)? != *identity {
        return Err(StoreError::OperationOriginMismatch);
    }
    Ok(())
}

pub(super) fn history(
    c: &Connection,
    head: &RequestId,
    boundaries: bool,
) -> Result<Vec<Occurrence>> {
    let mut query = c.prepare("WITH RECURSIVE lineage(id,parent_id,depth) AS (
        SELECT id,parent_id,0 FROM requests WHERE id=?1
        UNION ALL SELECT r.id,r.parent_id,l.depth+1 FROM requests r JOIN lineage l ON r.id=l.parent_id
        WHERE ?2=0 OR NOT EXISTS(SELECT 1 FROM session_state s WHERE s.session_id='harness:compaction:'||l.id)
    ) SELECT l.id,ri.position,ri.item_hash,i.json,COALESCE(ri.source_request,ri.request_id),COALESCE(ri.source_position,ri.position),ri.context_sources
    FROM lineage l JOIN request_items ri ON ri.request_id=l.id JOIN items i ON i.hash=ri.item_hash ORDER BY l.depth DESC,ri.position")?;
    let rows = query.query_map(params![head.0, boundaries], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, Option<String>>(6)?,
        ))
    })?;
    rows.map(|row| {
        let (request, position, hash, raw, origin_request, origin_position, sources) = row?;
        Ok(Occurrence {
            request: RequestId(request),
            position,
            hash: ItemHash(hash.clone()),
            item: serde_json::from_str(&raw)?,
            origin: Origin {
                request: RequestId(origin_request),
                position: origin_position,
                hash: ItemHash(hash),
            },
            sources: sources
                .map(|s| serde_json::from_str(&s))
                .transpose()?
                .unwrap_or_default(),
        })
    })
    .collect()
}

fn reference(items: &[Occurrence]) -> ContextReference {
    ContextReference(
        serde_json::to_string(&items.iter().map(|i| &i.origin).collect::<Vec<_>>())
            .expect("origin serialization"),
    )
}

fn message(item: &Item) -> Option<(ContextRole, String)> {
    if item.0["type"] != "message" {
        return None;
    }
    let role = ContextRole::parse(item.0["role"].as_str()?)?;
    let text = match &item.0["content"] {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts)
            if parts.len() == 1
                && matches!(
                    parts[0]["type"].as_str(),
                    Some("input_text" | "output_text")
                ) =>
        {
            parts[0]["text"].as_str()?.into()
        }
        _ => return None,
    };
    Some((role, text))
}

fn opaque(item: &Item) -> bool {
    fn encrypted(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(m) => m
                .iter()
                .any(|(k, v)| k.starts_with("encrypted") || encrypted(v)),
            serde_json::Value::Array(a) => a.iter().any(encrypted),
            _ => false,
        }
    }
    encrypted(&item.0)
        || !matches!(
            item.0["type"].as_str(),
            Some(
                "message"
                    | "function_call"
                    | "custom_tool_call"
                    | "function_call_output"
                    | "custom_tool_call_output"
                    | "configuration_update"
            )
        )
}

fn preview(items: &[Occurrence]) -> String {
    items
        .iter()
        .filter_map(|i| {
            if let Some((_, text)) = message(&i.item) {
                return Some(text);
            }
            for field in ["arguments", "input", "output"] {
                if let Some(text) = i.item.0[field].as_str() {
                    return Some(text.into());
                }
            }
            None
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn blocks(c: &Connection, all: &[Occurrence], cut: usize) -> Result<Vec<StoredBlock>> {
    let mut ranges = Vec::<(usize, usize, bool, bool)>::new();
    for (index, occurrence) in all.iter().enumerate().take(cut) {
        let item = &occurrence.item;
        if item.is_configuration_update()
            || (item.0["type"] == "message"
                && matches!(item.0["role"].as_str(), Some("system" | "developer")))
        {
            ranges.push((index, index + 1, false, true));
            continue;
        }
        if let Some(call) = item
            .tool_call()
            .map_err(|_| ContextError::InvalidReference)?
        {
            let result = all.iter().enumerate().skip(index + 1).find(|(_, i)| {
                i.item.0["call_id"] == call.call_id.0
                    && matches!(
                        i.item.0["type"].as_str(),
                        Some("function_call_output" | "custom_tool_call_output")
                    )
            });
            let end = result.map_or(index + 1, |(n, _)| n + 1);
            ranges.push((
                index,
                end.min(cut),
                opaque(item),
                result.is_none() || end > cut,
            ));
        } else if message(item).is_none() {
            let orphan = matches!(
                item.0["type"].as_str(),
                Some("function_call_output" | "custom_tool_call_output")
            ) && !all[..index].iter().any(|i| {
                i.item
                    .tool_call()
                    .ok()
                    .flatten()
                    .is_some_and(|call| item.0["call_id"] == call.call_id.0)
            });
            ranges.push((index, index + 1, opaque(item), orphan));
        }
        if opaque(item) {
            let raw:Option<String>=c.query_row("SELECT payload FROM events WHERE request_id=?1 AND kind='model_turn' ORDER BY id DESC LIMIT 1",[&occurrence.origin.request.0],|r|r.get(0)).optional()?;
            let hashes: HashSet<String> = raw
                .map(|raw| serde_json::from_str::<serde_json::Value>(&raw))
                .transpose()?
                .and_then(|r| r["response"]["items"].as_array().cloned())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            let members = all
                .iter()
                .enumerate()
                .filter(|(_, i)| {
                    i.origin.request == occurrence.origin.request
                        && hashes.contains(&i.origin.hash.0)
                })
                .map(|(n, _)| n)
                .collect::<Vec<_>>();
            if let (Some(start), Some(end)) = (members.first(), members.last()) {
                ranges.push((*start, (*end + 1).min(cut), true, *end >= cut));
            } else {
                ranges.push((index, index + 1, true, true));
            }
        }
    }
    ranges.sort_by_key(|r| r.0);
    let mut merged = Vec::<(usize, usize, bool, bool)>::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.0 < last.1
        {
            last.1 = last.1.max(range.1);
            last.2 |= range.2;
            last.3 |= range.3;
        } else {
            merged.push(range);
        }
    }
    let mut result = Vec::new();
    let mut index = 0;
    let mut ranges = merged.into_iter().peekable();
    while index < cut {
        if let Some(range) = ranges.peek()
            && range.0 == index
        {
            let (_, end, is_opaque, mandatory) = ranges.next().unwrap();
            let items = all[index..end].to_vec();
            let kind = if mandatory {
                ContextNativeKind::Pending
            } else if is_opaque {
                ContextNativeKind::Opaque
            } else {
                ContextNativeKind::CompletedExchange
            };
            result.push(StoredBlock {
                block: ContextBlock::Native {
                    reference: reference(&items),
                    kind,
                    preview: preview(&items),
                    protected: mandatory,
                },
                items,
                opaque: is_opaque,
                mandatory,
            });
            index = end;
        } else {
            let items = vec![all[index].clone()];
            let (role, text) = message(&items[0].item).ok_or(ContextError::InvalidReference)?;
            result.push(StoredBlock {
                block: ContextBlock::Text {
                    reference: Some(reference(&items)),
                    role,
                    text,
                    sources: items[0].sources.clone(),
                },
                items,
                opaque: false,
                mandatory: false,
            });
            index += 1;
        }
    }
    Ok(result)
}

fn insert_occurrence(
    tx: &Transaction<'_>,
    request: &RequestId,
    position: i64,
    occurrence: &Occurrence,
) -> Result<()> {
    tx.execute("INSERT INTO request_items(request_id,position,item_hash,source_request,source_position,context_sources) VALUES(?1,?2,?3,?4,?5,?6)",params![request.0,position,occurrence.hash.0,occurrence.origin.request.0,occurrence.origin.position,serde_json::to_string(&occurrence.sources)?])?;
    Ok(())
}

fn original_call(c: &Connection, operation: &OperationId) -> Result<Origin> {
    super::validation::invocation_item(c, &operation.request, &operation.call)?
        .ok_or(ContextError::MissingCall)?;
    let(position,hash):(i64,String)=c.query_row("SELECT ri.position,ri.item_hash FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id=?1 AND json_extract(i.json,'$.call_id')=?2 AND json_extract(i.json,'$.type') IN ('function_call','custom_tool_call')",params![operation.request.0,operation.call.0],|r|Ok((r.get(0)?,r.get(1)?)))?;
    Ok(Origin {
        request: operation.request.clone(),
        position,
        hash: ItemHash(hash),
    })
}

fn branch(c: &Connection, head: &RequestId) -> Result<String> {
    c.query_row("SELECT branch FROM requests WHERE id=?1", [&head.0], |r| {
        r.get(0)
    })
    .optional()?
    .ok_or_else(|| StoreError::MissingRequest(head.0.clone()))
}

impl Store {
    pub(crate) fn context_call_cut_tx(
        tx: &Transaction<'_>,
        operation: &OperationId,
        head: &RequestId,
    ) -> Result<usize> {
        let origin = original_call(tx, operation)?;
        let matches = history(tx, head, true)?
            .iter()
            .enumerate()
            .filter(|(_, i)| i.origin == origin)
            .map(|(n, _)| n)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [cut] => Ok(*cut),
            _ => Err(ContextError::MissingCall.into()),
        }
    }

    pub(crate) fn context_committed_head_tx(
        tx: &Transaction<'_>,
        operation: &OperationId,
    ) -> Result<Option<RequestId>> {
        Ok(receipt(tx, operation)?.map(|r| r.receipt.head))
    }

    pub(crate) fn preserve_context_origins_tx(
        tx: &Transaction<'_>,
        source: &RequestId,
        target: &RequestId,
    ) -> Result<()> {
        preserve_origins(tx, source, target)
    }

    pub fn initialize_context_model(
        &self,
        identity: &ConversationIdentity,
        model: &str,
    ) -> Result<()> {
        if model.trim().is_empty() {
            return Err(ContextError::InvalidModel.into());
        }
        let mut c = self.lock();
        let tx = c.transaction()?;
        validate_identity(&tx, &self.store_id, identity)?;
        let mut current = state(&tx, identity)?;
        if current.model.is_none() {
            current.model = Some(model.into());
            save_state(&tx, identity, &current)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn context_model(&self, identity: &ConversationIdentity) -> Result<Option<String>> {
        let c = self.lock();
        validate_identity(&c, &self.store_id, identity)?;
        Ok(state(&c, identity)?.model)
    }

    pub fn context_request_state(
        &self,
        head: &RequestId,
        identity: &ConversationIdentity,
    ) -> Result<ContextRequestState> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        validate_identity(&tx, &self.store_id, identity)?;
        if branch(&tx, head)? != identity.actor().0 {
            return Err(StoreError::OperationOriginMismatch);
        }
        let current = state(&tx, identity)?;
        let history = history(&tx, head, true)?
            .into_iter()
            .map(|i| (i.request, i.hash, i.item))
            .collect();
        tx.commit()?;
        Ok(ContextRequestState {
            history,
            model: current.model,
            generation: current.generation,
        })
    }

    pub fn context_history(&self, head: &RequestId) -> Result<Vec<(RequestId, ItemHash, Item)>> {
        let c = self.lock();
        Ok(history(&c, head, true)?
            .into_iter()
            .map(|i| (i.request, i.hash, i.item))
            .collect())
    }

    pub fn read_context(&self, head: &RequestId) -> Result<ContextDocument> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        let all = history(&tx, head, true)?;
        let document = ContextDocument {
            blocks: blocks(&tx, &all, all.len())?
                .into_iter()
                .map(|b| b.block)
                .collect(),
        };
        tx.commit()?;
        Ok(document)
    }

    pub fn begin_context(
        &self,
        operation: &OperationId,
        head: &RequestId,
    ) -> Result<ContextSnapshot> {
        let mut c = self.lock();
        let tx = c.transaction()?;
        validate_identity(&tx, &self.store_id, &operation.origin)?;
        if branch(&tx, head)? != operation.origin.actor().0 {
            return Err(StoreError::OperationOriginMismatch);
        }
        let claim:Option<String>=tx.query_row("SELECT state FROM claims WHERE origin=?1 AND origin_request_id=?2 AND call_id=?3 AND request_id=?2",params![serde_json::to_string(&operation.origin)?,operation.request.0,operation.call.0],|r|r.get(0)).optional()?;
        if claim.as_deref() != Some("pending") {
            return Err(StoreError::CheckpointBoundaryNotPending(operation.clone()));
        }
        let origin = original_call(&tx, operation)?;
        let all = history(&tx, head, true)?;
        let matches = all
            .iter()
            .enumerate()
            .filter(|(_, i)| i.origin == origin)
            .map(|(n, _)| n)
            .collect::<Vec<_>>();
        let [cut] = matches.as_slice() else {
            return Err(ContextError::MissingCall.into());
        };
        let stored = blocks(&tx, &all, *cut)?;
        let document = ContextDocument {
            blocks: stored.iter().map(|b| b.block.clone()).collect(),
        };
        let generation = state(&tx, &operation.origin)?.generation;
        let snapshot = ContextSnapshot {
            document,
            generation,
            head: head.clone(),
            operation: operation.clone(),
            prefix: all[..*cut].to_vec(),
            blocks: stored,
            store_id: self.store_id.clone(),
            seal: SnapshotSeal {
                operation: operation.clone(),
                head: head.clone(),
                generation,
            },
        };
        tx.commit()?;
        Ok(snapshot)
    }

    pub fn context_receipt(&self, operation: &OperationId) -> Result<Option<ContextCommitReceipt>> {
        receipt(&self.lock(), operation).map(|r| r.map(|r| r.receipt))
    }

    pub fn commit_context(&self, commit: ContextCommit<'_>) -> Result<ContextCommitReceipt> {
        self.commit_context_inner(commit, None)
    }

    pub fn restore_context_commit(
        &self,
        snapshot: &ContextSnapshot,
        evidence: &ContextCommitEvidence,
        output: &JobOutput,
        pending: &[OperationId],
    ) -> Result<ContextCommitReceipt> {
        let draft = ContextDraft {
            document: snapshot.document.clone(),
            next_model: evidence.model.clone(),
        };
        self.commit_context_inner(
            ContextCommit {
                snapshot,
                draft: &draft,
                output,
                pending,
            },
            Some(evidence),
        )
    }

    pub fn context_commit_evidence(
        &self,
        operation: &OperationId,
    ) -> Result<Option<ContextCommitEvidence>> {
        let c = self.lock();
        let Some(record) = receipt(&c, operation)? else {
            return Ok(None);
        };
        if record.version != 1
            || terminal::exact_terminal(&c, operation)?
                != Some((record.output.clone(), TerminalOutcome::Success))
        {
            return Err(ContextError::UnsupportedState.into());
        }
        let load = |hash: &ItemHash| -> Result<Item> {
            let raw: String =
                c.query_row("SELECT json FROM items WHERE hash=?1", [&hash.0], |r| {
                    r.get(0)
                })?;
            let item = Item(serde_json::from_str(&raw)?);
            if ItemHash(
                blake3::hash(&serde_json::to_vec(&item)?)
                    .to_hex()
                    .to_string(),
            ) != *hash
            {
                return Err(ContextError::UnsupportedState.into());
            }
            Ok(item)
        };
        let output = load(&record.output)?;
        if output.0["call_id"].as_str() != Some(operation.call.0.as_str()) {
            return Err(ContextError::UnsupportedState.into());
        }
        let prefix = record
            .prefix
            .iter()
            .map(|p| Ok((load(&p.hash)?, p.sources.clone())))
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(ContextCommitEvidence {
            original_operation: record.original_operation.unwrap_or(record.operation),
            prefix,
            output,
            invocation: load(&original_call(&c, operation)?.hash)?,
            model: record.receipt.model,
        }))
    }

    fn commit_context_inner(
        &self,
        commit: ContextCommit<'_>,
        replay: Option<&ContextCommitEvidence>,
    ) -> Result<ContextCommitReceipt> {
        let ContextCommit {
            snapshot,
            draft,
            output,
            pending,
        } = commit;
        if snapshot.store_id != self.store_id
            || snapshot.operation != snapshot.seal.operation
            || snapshot.head != snapshot.seal.head
            || snapshot.generation != snapshot.seal.generation
        {
            return Err(ContextError::InvalidReference.into());
        }
        let JobOutput::Completed(Ok(_)) = output else {
            return Err(ContextError::Ineligible.into());
        };
        let mut c = self.lock();
        let tx = c.transaction()?;
        validate_identity(&tx, &self.store_id, &snapshot.operation.origin)?;
        let call = super::validation::invocation_item(
            &tx,
            &snapshot.operation.request,
            &snapshot.operation.call,
        )?
        .ok_or(ContextError::MissingCall)?;
        let output_item = Item::tool_output(&snapshot.operation.call, call.input.kind(), output);
        let output_hash = Self::put_item_tx(&tx, &output_item)?;
        if let Some(evidence) = replay {
            let original = original_call(&tx, &snapshot.operation)?;
            let raw: String = tx.query_row(
                "SELECT json FROM items WHERE hash=?1",
                [&original.hash.0],
                |r| r.get(0),
            )?;
            let local_invocation: Item = serde_json::from_str(&raw)?;
            let mut expected_invocation = evidence.invocation.clone();
            expected_invocation.0["call_id"] = json!(snapshot.operation.call.0);
            if expected_invocation != local_invocation {
                return Err(ContextError::Ineligible.into());
            }
            let mut expected = evidence.output.clone();
            expected.0["call_id"] = json!(snapshot.operation.call.0);
            if expected != output_item {
                return Err(ContextError::Ineligible.into());
            }
        }
        let candidate = if let Some(evidence) = replay {
            serde_json::to_string(&(
                evidence
                    .prefix
                    .iter()
                    .map(|(i, s)| (i, s))
                    .collect::<Vec<_>>(),
                &evidence.model,
            ))?
        } else {
            serde_json::to_string(draft)?
        };
        if let Some(record) = receipt(&tx, &snapshot.operation)? {
            if record.output != output_hash || record.candidate != candidate {
                return Err(StoreError::ConflictingReplayOutcome {
                    operation: snapshot.operation.clone(),
                });
            }
            tx.commit()?;
            return Ok(record.receipt);
        }
        let mut current = state(&tx, &snapshot.operation.origin)?;
        if current.generation != snapshot.generation {
            return Err(ContextError::Conflict.into());
        }
        if let ConversationIdentity::Embedded {
            run,
            actor,
            incarnation,
        } = &snapshot.operation.origin
        {
            let identity = crate::embedding::HostIdentity {
                run: run.clone(),
                actor: actor.clone(),
                incarnation: incarnation.clone(),
            };
            let frontier = embedded_round::frontier(&tx, &identity)?;
            if frontier
                .pending_head
                .as_ref()
                .or(frontier.settled_head.as_ref())
                != Some(&snapshot.head)
            {
                return Err(ContextError::Conflict.into());
            }
        }
        let all = history(&tx, &snapshot.head, true)?;
        if all.len() < snapshot.prefix.len() || all[..snapshot.prefix.len()] != snapshot.prefix {
            return Err(ContextError::Conflict.into());
        }
        let origin = original_call(&tx, &snapshot.operation)?;
        if all
            .get(snapshot.prefix.len())
            .is_none_or(|i| i.origin != origin)
        {
            return Err(ContextError::Conflict.into());
        }
        let mut rewritten = Vec::<Occurrence>::new();
        let mut seen = HashSet::new();
        let mut last_native = None;
        let mut retained_opaque = false;
        if let Some(evidence) = replay {
            let mut used_occurrences = HashSet::new();
            for (item, sources) in &evidence.prefix {
                let hash = Self::put_item_tx(&tx, item)?;
                if let Some((index, existing)) =
                    snapshot.prefix.iter().enumerate().find(|(index, i)| {
                        !used_occurrences.contains(index) && i.hash == hash && i.item == *item
                    })
                {
                    used_occurrences.insert(index);
                    let mut occurrence = existing.clone();
                    occurrence.sources = sources.clone();
                    rewritten.push(occurrence);
                } else {
                    rewritten.push(Occurrence {
                        request: RequestId(String::new()),
                        position: 0,
                        hash: hash.clone(),
                        item: item.clone(),
                        sources: sources.clone(),
                        origin: Origin {
                            request: RequestId(String::new()),
                            position: 0,
                            hash,
                        },
                    });
                }
            }
            // Replay must preserve every protected local envelope as an intact
            // group; foreign evidence cannot replace local pending operations.
            for stored in &snapshot.blocks {
                if stored.mandatory
                    && !rewritten.windows(stored.items.len()).any(|w| {
                        w.iter()
                            .map(|i| &i.item)
                            .eq(stored.items.iter().map(|i| &i.item))
                    })
                {
                    return Err(ContextError::ProtectedGroup.into());
                }
            }
            retained_opaque = rewritten.iter().any(|i| opaque(&i.item));
        } else {
            for block in &draft.document.blocks {
                match block {
                    ContextBlock::Native { reference, .. } => {
                        let (index,stored)=snapshot.blocks.iter().enumerate().find(|(_,b)|matches!(&b.block,ContextBlock::Native{reference:r,..} if r==reference)).ok_or(ContextError::InvalidReference)?;
                        if *block != stored.block {
                            return Err(ContextError::NativeEdit.into());
                        }
                        if !seen.insert(reference.clone()) {
                            return Err(ContextError::InvalidReference.into());
                        }
                        if last_native.is_some_and(|previous| index < previous) {
                            return Err(ContextError::NativeOrder.into());
                        }
                        last_native = Some(index);
                        retained_opaque |= stored.opaque;
                        rewritten.extend(stored.items.clone());
                    }
                    ContextBlock::Text {
                        reference: Some(reference),
                        role,
                        text,
                        sources,
                    } => {
                        let stored=snapshot.blocks.iter().find(|b|matches!(&b.block,ContextBlock::Text{reference:Some(r),..} if r==reference)).ok_or(ContextError::InvalidReference)?;
                        if !seen.insert(reference.clone()) {
                            return Err(ContextError::InvalidReference.into());
                        }
                        let ContextBlock::Text {
                            role: old_role,
                            sources: old_sources,
                            ..
                        } = &stored.block
                        else {
                            unreachable!()
                        };
                        if role != old_role || sources != old_sources {
                            return Err(ContextError::InvalidReference.into());
                        }
                        let mut occurrence = stored.items[0].clone();
                        if message(&occurrence.item).is_none_or(|(_, old)| old != *text) {
                            match &mut occurrence.item.0["content"] {
                                serde_json::Value::String(old) => *old = text.clone(),
                                serde_json::Value::Array(parts) => parts[0]["text"] = json!(text),
                                _ => return Err(ContextError::InvalidReference.into()),
                            }
                            occurrence.hash = Self::put_item_tx(&tx, &occurrence.item)?;
                            occurrence.origin = Origin {
                                request: RequestId(String::new()),
                                position: 0,
                                hash: occurrence.hash.clone(),
                            };
                        }
                        rewritten.push(occurrence);
                    }
                    ContextBlock::Text {
                        reference: None,
                        role,
                        text,
                        sources,
                    } => {
                        for source in sources {
                            if !snapshot.blocks.iter().any(|b| match &b.block {
                                ContextBlock::Text {
                                    reference: Some(r), ..
                                }
                                | ContextBlock::Native { reference: r, .. } => r == source,
                                _ => false,
                            }) {
                                return Err(ContextError::InvalidReference.into());
                            }
                        }
                        let item =
                            Item(json!({"type":"message","role":role.text(),"content":text}));
                        let hash = Self::put_item_tx(&tx, &item)?;
                        rewritten.push(Occurrence {
                            request: RequestId(String::new()),
                            position: 0,
                            origin: Origin {
                                request: RequestId(String::new()),
                                position: 0,
                                hash: hash.clone(),
                            },
                            hash,
                            item,
                            sources: sources.clone(),
                        });
                    }
                }
            }
            for stored in &snapshot.blocks {
                if stored.mandatory && !draft.document.blocks.contains(&stored.block) {
                    return Err(ContextError::ProtectedGroup.into());
                }
            }
        }
        if let Some(model) = &draft.next_model {
            if model.trim().is_empty() {
                return Err(ContextError::InvalidModel.into());
            }
            if current.model.as_ref() != Some(model)
                && (retained_opaque || all[snapshot.prefix.len()..].iter().any(|i| opaque(&i.item)))
            {
                return Err(ContextError::OpaqueModel.into());
            }
        }
        let text_changed = rewritten.iter().map(|i| &i.hash).collect::<Vec<_>>()
            != snapshot.prefix.iter().map(|i| &i.hash).collect::<Vec<_>>();
        let model_changed = draft
            .next_model
            .as_ref()
            .is_some_and(|model| current.model.as_ref() != Some(model));
        let changed = text_changed || model_changed;
        let head = if text_changed {
            RequestId(uuid::Uuid::new_v4().to_string())
        } else {
            snapshot.head.clone()
        };
        let prefix_evidence = rewritten
            .iter()
            .map(|i| PrefixEvidence {
                hash: i.hash.clone(),
                sources: i.sources.clone(),
            })
            .collect();
        if text_changed {
            tx.execute("INSERT INTO requests(id,parent_id,branch,created_at,embedded_run,embedded_incarnation,round_phase) SELECT ?1,id,branch,?2,embedded_run,embedded_incarnation,round_phase FROM requests WHERE id=?3",params![head.0,utc_millis(),snapshot.head.0])?;
            rewritten.extend_from_slice(&all[snapshot.prefix.len()..]);
            for (position, mut occurrence) in rewritten.into_iter().enumerate() {
                if occurrence.origin.request.0.is_empty() {
                    occurrence.origin.request = head.clone();
                    occurrence.origin.position = position as i64;
                }
                insert_occurrence(&tx, &head, position as i64, &occurrence)?;
            }
            tx.execute(
                "INSERT INTO session_state(session_id,state,updated_at) VALUES(?1,'true',?2)",
                params![format!("harness:compaction:{}", head.0), utc_millis()],
            )?;
            let mut carried = HashSet::<OperationId>::new();
            for pending in pending.iter().chain(std::iter::once(&snapshot.operation)) {
                if carried.insert(pending.clone()) {
                    carry_claim(&tx, pending, &head)?;
                }
            }
            // A completed exchange can remain native across several rewrites.
            // Keep its existing terminal claim attached to the copied occurrence.
            for occurrence in &all {
                let Some(call) = occurrence
                    .item
                    .tool_call()
                    .map_err(|_| ContextError::UnsupportedState)?
                else {
                    continue;
                };
                let mut q = tx.prepare("SELECT origin,origin_request_id FROM claims WHERE request_id=?1 AND call_id=?2")?;
                let claims = q
                    .query_map(params![occurrence.request.0, call.call_id.0], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                for (origin, request) in claims {
                    let operation = OperationId {
                        origin: serde_json::from_str(&origin)?,
                        request: RequestId(request),
                        call: call.call_id.clone(),
                    };
                    let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id=?1 AND json_extract(i.json,'$.call_id')=?2 AND json_extract(i.json,'$.type') IN ('custom_tool_call','function_call'))",params![head.0,operation.call.0],|r|r.get(0))?;
                    if exists && carried.insert(operation.clone()) {
                        carry_claim(&tx, &operation, &head)?;
                    }
                }
            }
        }
        if changed {
            current.generation = current
                .generation
                .checked_add(1)
                .ok_or(ContextError::UnsupportedState)?;
        }
        if let Some(model) = &draft.next_model {
            current.model = Some(model.clone());
        }
        save_state(&tx, &snapshot.operation.origin, &current)?;
        settle_success(&tx, &snapshot.operation, &output_hash)?;
        let position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position)+1,0) FROM request_items WHERE request_id=?1",
            [&head.0],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO request_items(request_id,position,item_hash) VALUES(?1,?2,?3)",
            params![head.0, position, output_hash.0],
        )?;
        let receipt = ContextCommitReceipt {
            head,
            generation: current.generation,
            changed,
            model: current.model,
        };
        let record = ReceiptRecord {
            version: 1,
            prefix: prefix_evidence,
            candidate,
            original_operation: replay.map(|e| e.original_operation.clone()),
            operation: snapshot.operation.clone(),
            output: output_hash,
            receipt: receipt.clone(),
        };
        tx.execute("INSERT INTO events(request_id,kind,payload,created_at) VALUES(?1,'context_commit',?2,?3)",params![receipt.head.0,serde_json::to_string(&record)?,utc_millis()])?;
        tx.commit()?;
        Ok(receipt)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptRecord {
    version: u32,
    prefix: Vec<PrefixEvidence>,
    candidate: String,
    original_operation: Option<OperationId>,
    operation: OperationId,
    output: ItemHash,
    receipt: ContextCommitReceipt,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrefixEvidence {
    hash: ItemHash,
    sources: Vec<ContextReference>,
}

fn receipt(c: &Connection, operation: &OperationId) -> Result<Option<ReceiptRecord>> {
    let mut q=c.prepare("SELECT payload FROM events WHERE kind='context_commit' AND json_extract(payload,'$.operation')=?1 ORDER BY id DESC LIMIT 2")?;
    let rows = q
        .query_map([serde_json::to_string(operation)?], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match rows.as_slice() {
        [] => Ok(None),
        [raw] => {
            let record: ReceiptRecord = serde_json::from_str(raw)?;
            if record.version != 1 || record.operation != *operation {
                return Err(ContextError::UnsupportedState.into());
            }
            Ok(Some(record))
        }
        _ => Err(ContextError::UnsupportedState.into()),
    }
}

fn settle_success(tx: &Transaction<'_>, operation: &OperationId, hash: &ItemHash) -> Result<()> {
    if let Some((old, terminal)) = terminal::exact_terminal(tx, operation)?
        && (old != *hash || terminal != TerminalOutcome::Success)
    {
        return Err(StoreError::ConflictingReplayOutcome {
            operation: operation.clone(),
        });
    }
    let n=tx.execute("UPDATE claims SET state='settled',output_hash=?4,terminal_json=?5 WHERE origin=?1 AND origin_request_id=?2 AND call_id=?3 AND state='pending'",params![serde_json::to_string(&operation.origin)?,operation.request.0,operation.call.0,hash.0,serde_json::to_string(&TerminalOutcome::Success)?])?;
    if n == 0 {
        return Err(ContextError::Conflict.into());
    }
    Ok(())
}

fn carry_claim(tx: &Transaction<'_>, operation: &OperationId, head: &RequestId) -> Result<()> {
    let origin = serde_json::to_string(&operation.origin)?;
    let matching:i64=tx.query_row("SELECT COUNT(*) FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id=?1 AND json_extract(i.json,'$.call_id')=?2 AND json_extract(i.json,'$.type') IN ('function_call','custom_tool_call')",params![head.0,operation.call.0],|r|r.get(0))?;
    if matching != 1 {
        return Err(ContextError::ProtectedGroup.into());
    }
    let n=tx.execute("INSERT INTO claims(origin,origin_request_id,call_id,request_id,state,output_hash,terminal_json) SELECT origin,origin_request_id,call_id,?4,state,output_hash,terminal_json FROM claims WHERE origin=?1 AND origin_request_id=?2 AND call_id=?3 ORDER BY CASE WHEN request_id=origin_request_id THEN 0 ELSE 1 END LIMIT 1",params![origin,operation.request.0,operation.call.0,head.0])?;
    if n != 1 {
        return Err(ContextError::ProtectedGroup.into());
    }
    Ok(())
}

/// Replacements preserve occurrence identity whenever an exact input item is reused.
pub(super) fn preserve_origins(
    tx: &Transaction<'_>,
    source: &RequestId,
    target: &RequestId,
) -> Result<()> {
    let old = history(tx, source, true)?;
    let target_items = history(tx, target, false)?
        .into_iter()
        .filter(|i| i.request == *target)
        .collect::<Vec<_>>();
    let mut candidates = HashMap::<String, Vec<&Occurrence>>::new();
    for item in &old {
        candidates
            .entry(item.hash.0.clone())
            .or_default()
            .push(item);
    }
    let mut used = HashMap::<String, usize>::new();
    for item in target_items {
        let count = used.entry(item.hash.0.clone()).or_default();
        if let Some(source) = candidates.get(&item.hash.0).and_then(|v| v.get(*count)) {
            tx.execute("UPDATE request_items SET source_request=?3,source_position=?4,context_sources=?5 WHERE request_id=?1 AND position=?2",params![target.0,item.position,source.origin.request.0,source.origin.position,serde_json::to_string(&source.sources)?])?;
            *count += 1;
        }
    }
    Ok(())
}

pub(super) fn compaction_generation(
    tx: &Transaction<'_>,
    store_id: &str,
    source: &RequestId,
    target: &RequestId,
    branch: &str,
) -> Result<()> {
    preserve_origins(tx, source, target)?;
    let before = history(tx, source, true)?
        .into_iter()
        .map(|i| i.hash)
        .collect::<Vec<_>>();
    let after = history(tx, target, false)?
        .into_iter()
        .filter(|i| i.request == *target)
        .map(|i| i.hash)
        .collect::<Vec<_>>();
    if before != after {
        let identity = identity_for_branch(tx, store_id, branch)?;
        let mut current = state(tx, &identity)?;
        current.generation = current
            .generation
            .checked_add(1)
            .ok_or(ContextError::UnsupportedState)?;
        save_state(tx, &identity, &current)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
