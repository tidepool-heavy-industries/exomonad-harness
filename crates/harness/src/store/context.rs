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

/// Called only by the authorized exact binding transfer, in its transaction.
/// Context seals and historical operations retain their issuing incarnation.
pub(super) fn transfer_embedded_state(
    tx: &Transaction<'_>,
    predecessor: &crate::embedding::HostIdentity,
    successor: &crate::embedding::HostIdentity,
) -> Result<()> {
    let identity = |host: &crate::embedding::HostIdentity| ConversationIdentity::Embedded {
        run: host.run.clone(),
        actor: host.actor.clone(),
        incarnation: host.incarnation.clone(),
    };
    let predecessor = identity(predecessor);
    let successor = identity(successor);
    save_state(tx, &successor, &state(tx, &predecessor)?)?;
    tx.execute(
        "DELETE FROM session_state WHERE session_id=?1",
        [state_key(&predecessor)?],
    )?;
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
    query_occurrences(c, "WITH RECURSIVE lineage(id,parent_id,depth) AS (
        SELECT id,parent_id,0 FROM requests WHERE id=?1
        UNION ALL SELECT r.id,r.parent_id,l.depth+1 FROM requests r JOIN lineage l ON r.id=l.parent_id
        WHERE ?2=0 OR NOT EXISTS(SELECT 1 FROM session_state s WHERE s.session_id='harness:compaction:'||l.id)
    ) SELECT l.id,ri.position,ri.item_hash,i.json,COALESCE(ri.source_request,ri.request_id),COALESCE(ri.source_position,ri.position),ri.context_sources,ri.context_note
    FROM lineage l JOIN request_items ri ON ri.request_id=l.id JOIN items i ON i.hash=ri.item_hash ORDER BY l.depth DESC,ri.position", params![head.0, boundaries])
}

pub(super) fn request_occurrences(c: &Connection, request: &RequestId) -> Result<Vec<Occurrence>> {
    query_occurrences(c, "SELECT ri.request_id,ri.position,ri.item_hash,i.json,COALESCE(ri.source_request,ri.request_id),COALESCE(ri.source_position,ri.position),ri.context_sources,ri.context_note
        FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id=?1 ORDER BY ri.position", [&request.0])
}

fn query_occurrences(
    c: &Connection,
    sql: &str,
    bindings: impl rusqlite::Params,
) -> Result<Vec<Occurrence>> {
    let mut query = c.prepare(sql)?;
    let rows = query.query_map(bindings, |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, i64>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, bool>(7)?,
        ))
    })?;
    rows.map(|row| {
        let (request, position, hash, raw, origin_request, origin_position, sources, note) = row?;
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
            note,
        })
    })
    .collect()
}

/// Attribution is a request projection; canonical retained bytes and origin
/// references remain unchanged in the Store.
fn project_context_note(occurrence: &Occurrence) -> Item {
    let mut item = occurrence.item.clone();
    if !occurrence.note {
        return item;
    }
    let Some((_, text)) = message(&item) else {
        return item;
    };
    let sources = occurrence
        .sources
        .iter()
        .flat_map(|r| serde_json::from_str::<Vec<Origin>>(r.as_str()).unwrap_or_default())
        .map(|o| format!("{}:{}", o.request.0, o.position))
        .collect::<Vec<_>>();
    let attribution = if sources.is_empty() {
        "[Agent-authored context note]".to_string()
    } else {
        format!(
            "[Agent-authored context note; sources: {}]",
            sources.join(", ")
        )
    };
    let projected = format!("{attribution}\n{text}");
    match &mut item.0["content"] {
        serde_json::Value::String(content) => *content = projected,
        serde_json::Value::Array(parts) => parts[0]["text"] = json!(projected),
        _ => {}
    }
    item
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
    // Index the immutable history once for the grouping pass. Calls still pair
    // with the first matching output after their own position, including when
    // duplicate call IDs appear in the same history.
    let mut outputs = HashMap::<String, Vec<usize>>::new();
    for (index, occurrence) in all.iter().enumerate() {
        if matches!(
            occurrence.item.0["type"].as_str(),
            Some("function_call_output" | "custom_tool_call_output")
        ) && let Some(call_id) = occurrence.item.0["call_id"].as_str()
        {
            outputs.entry(call_id.into()).or_default().push(index);
        }
    }

    // A model_turn payload describes opaque items by hash. Load each source
    // request once, then reduce its membership to the first and last history
    // positions in one pass. This keeps per-item work independent of history
    // length while preserving repeated occurrences of a listed hash.
    let opaque_items = all
        .iter()
        .take(cut)
        .map(|occurrence| opaque(&occurrence.item))
        .collect::<Vec<_>>();
    let opaque_requests = all
        .iter()
        .take(cut)
        .enumerate()
        .filter(|(index, occurrence)| {
            opaque_items[*index]
                && !occurrence.item.is_configuration_update()
                && !(occurrence.item.0["type"] == "message"
                    && matches!(
                        occurrence.item.0["role"].as_str(),
                        Some("system" | "developer")
                    ))
        })
        .map(|(_, occurrence)| occurrence.origin.request.clone())
        .collect::<HashSet<_>>();
    let mut opaque_hashes = HashMap::<RequestId, HashSet<String>>::new();
    for request in opaque_requests {
        let raw: Option<String> = c
            .query_row(
                "SELECT payload FROM events WHERE request_id=?1 AND kind='model_turn' ORDER BY id DESC LIMIT 1",
                [&request.0],
                |r| r.get(0),
            )
            .optional()?;
        let hashes = raw
            .map(|raw| serde_json::from_str::<serde_json::Value>(&raw))
            .transpose()?
            .and_then(|r| r["response"]["items"].as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        opaque_hashes.insert(request, hashes);
    }
    let mut opaque_bounds = HashMap::<RequestId, (usize, usize)>::new();
    for (index, occurrence) in all.iter().enumerate() {
        if opaque_hashes
            .get(&occurrence.origin.request)
            .is_some_and(|hashes| hashes.contains(&occurrence.origin.hash.0))
        {
            opaque_bounds
                .entry(occurrence.origin.request.clone())
                .and_modify(|bounds| bounds.1 = index)
                .or_insert((index, index));
        }
    }

    let mut ranges = Vec::<(usize, usize, bool, bool)>::new();
    let mut prior_calls = HashSet::<String>::new();
    for (index, occurrence) in all.iter().enumerate().take(cut) {
        let item = &occurrence.item;
        if item.is_configuration_update()
            || (item.0["type"] == "message"
                && matches!(item.0["role"].as_str(), Some("system" | "developer")))
        {
            ranges.push((index, index + 1, false, true));
            continue;
        }
        let call = item
            .tool_call()
            .map_err(|_| ContextError::InvalidReference)?;
        if let Some(call) = &call {
            let result = outputs.get(&call.call_id.0).and_then(|positions| {
                let next = positions.partition_point(|position| *position <= index);
                positions.get(next).copied()
            });
            let end = result.map_or(index + 1, |n| n + 1);
            ranges.push((
                index,
                end.min(cut),
                opaque_items[index],
                result.is_none() || end > cut,
            ));
        } else if message(item).is_none() {
            let orphan = matches!(
                item.0["type"].as_str(),
                Some("function_call_output" | "custom_tool_call_output")
            ) && !item.0["call_id"]
                .as_str()
                .is_some_and(|call_id| prior_calls.contains(call_id));
            ranges.push((index, index + 1, opaque_items[index], orphan));
        }
        if let Some(call) = call {
            prior_calls.insert(call.call_id.0);
        }
        if opaque_items[index] {
            if let Some((start, end)) = opaque_bounds.get(&occurrence.origin.request) {
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

pub(super) fn insert_occurrence(
    tx: &Transaction<'_>,
    request: &RequestId,
    position: i64,
    occurrence: &Occurrence,
) -> Result<()> {
    tx.execute("INSERT INTO request_items(request_id,position,item_hash,source_request,source_position,context_sources,context_note) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![request.0,position,occurrence.hash.0,occurrence.origin.request.0,occurrence.origin.position,serde_json::to_string(&occurrence.sources)?,occurrence.note])?;
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
        Ok(receipt(tx, operation)?.map(|r| r.deferred_head))
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
            .map(|i| {
                if i.note {
                    let projected = project_context_note(&i);
                    let hash = Self::put_item_tx(&tx, &projected)?;
                    Ok((i.request, hash, projected))
                } else {
                    Ok((i.request, i.hash, i.item))
                }
            })
            .collect::<Result<Vec<_>>>()?;
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
        self.commit_context_inner(commit, None, &|| false)
    }

    pub fn commit_context_guarded(
        &self,
        commit: ContextCommit<'_>,
        cancelled: impl Fn() -> bool,
    ) -> Result<ContextCommitReceipt> {
        self.commit_context_inner(commit, None, &cancelled)
    }

    pub fn restore_context_commit_guarded(
        &self,
        snapshot: &ContextSnapshot,
        evidence: &ContextCommitEvidence,
        output: &JobOutput,
        pending: &[OperationId],
        cancelled: impl Fn() -> bool,
    ) -> Result<ContextCommitReceipt> {
        let draft = ContextDraft {
            document: snapshot.document.clone(),
            next_model: evidence.model.clone(),
            next_effort: evidence.next_effort,
        };
        self.commit_context_inner(
            ContextCommit {
                snapshot,
                draft: &draft,
                output,
                pending,
            },
            Some(evidence),
            &cancelled,
        )
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
            next_effort: evidence.next_effort,
        };
        self.commit_context_inner(
            ContextCommit {
                snapshot,
                draft: &draft,
                output,
                pending,
            },
            Some(evidence),
            &|| false,
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
        if record.version != 2
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
            .map(|p| Ok((load(&p.hash)?, p.sources.clone(), p.note)))
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(ContextCommitEvidence {
            original_operation: record.original_operation.unwrap_or(record.operation),
            prefix,
            output,
            invocation: load(&original_call(&c, operation)?.hash)?,
            model: record.receipt.model,
            next_effort: record.next_effort,
        }))
    }

    fn commit_context_inner(
        &self,
        commit: ContextCommit<'_>,
        replay: Option<&ContextCommitEvidence>,
        cancelled: &dyn Fn() -> bool,
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
                    .map(|(i, s, note)| (i, s, note))
                    .collect::<Vec<_>>(),
                &evidence.model,
                evidence.next_effort,
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
        if cancelled() {
            return Err(ContextError::Cancelled.into());
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
        // Saved documents can retain content dropped by an earlier rewrite.
        // Resolve those references only from immutable ancestry before the
        // original issuing call; post-call envelopes never become editable.
        let current_refs = snapshot
            .blocks
            .iter()
            .filter_map(|b| match &b.block {
                ContextBlock::Text {
                    reference: Some(r), ..
                }
                | ContextBlock::Native { reference: r, .. } => Some(r),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let current_sources = snapshot
            .prefix
            .iter()
            .flat_map(|i| i.sources.iter())
            .collect::<HashSet<_>>();
        let historical_needed = if let Some(evidence) = replay {
            evidence.prefix.iter().any(|(item, _, _)| {
                message(item).is_none() && !snapshot.prefix.iter().any(|i| i.item == *item)
            })
        } else {
            draft.document.blocks.iter().any(|b| match b {
                ContextBlock::Native { reference, .. }
                | ContextBlock::Text {
                    reference: Some(reference),
                    ..
                } => !current_refs.contains(reference),
                ContextBlock::Text {
                    reference: None,
                    sources,
                    ..
                } => sources
                    .iter()
                    .any(|r| !current_refs.contains(r) && !current_sources.contains(r)),
            })
        };
        let mut historic_blocks = Vec::new();
        if historical_needed {
            let mut q = tx.prepare("WITH RECURSIVE lineage(id,parent_id,depth) AS (SELECT id,parent_id,0 FROM requests WHERE id=?1 UNION ALL SELECT r.id,r.parent_id,lineage.depth+1 FROM requests r JOIN lineage ON r.id=lineage.parent_id) SELECT id FROM lineage ORDER BY depth DESC")?;
            let ancestors = q
                .query_map([&snapshot.operation.request.0], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(q);
            for ancestor in ancestors {
                let historic = history(&tx, &RequestId(ancestor), true)?;
                let cut = historic
                    .iter()
                    .position(|i| i.origin == origin)
                    .unwrap_or(historic.len());
                historic_blocks.extend(blocks(&tx, &historic, cut)?);
            }
        }
        let mut available = snapshot.blocks.clone();
        for block in historic_blocks {
            let identity = match &block.block {
                ContextBlock::Text {
                    reference: Some(r), ..
                }
                | ContextBlock::Native { reference: r, .. } => r,
                _ => continue,
            };
            if !available.iter().any(|b| match &b.block {
                ContextBlock::Text {
                    reference: Some(r), ..
                }
                | ContextBlock::Native { reference: r, .. } => r == identity,
                _ => false,
            }) {
                available.push(block)
            }
        }
        let mut rewritten = Vec::<Occurrence>::new();
        let mut seen = HashSet::new();
        let mut last_native = None;
        let mut last_historical_native = None;
        let current_origins = snapshot
            .prefix
            .iter()
            .map(|i| &i.origin)
            .collect::<HashSet<_>>();
        let mut retained_current_opaque = false;
        if let Some(evidence) = replay {
            let mut used_occurrences = HashSet::new();
            let mut used_origins = HashSet::new();
            for (item, sources, note) in &evidence.prefix {
                let hash = Self::put_item_tx(&tx, item)?;
                if let Some((index, existing)) =
                    snapshot.prefix.iter().enumerate().find(|(index, i)| {
                        !used_occurrences.contains(index) && i.hash == hash && i.item == *item
                    })
                {
                    used_occurrences.insert(index);
                    used_origins.insert(existing.origin.clone());
                    let mut occurrence = existing.clone();
                    occurrence.sources = sources.clone();
                    occurrence.note = *note;
                    rewritten.push(occurrence);
                } else if let Some(existing) =
                    available.iter().flat_map(|block| &block.items).find(|i| {
                        !used_origins.contains(&i.origin) && i.hash == hash && i.item == *item
                    })
                {
                    // Restored native groups reuse bounded local occurrences,
                    // including any text inside their provider envelope.
                    used_origins.insert(existing.origin.clone());
                    let mut occurrence = existing.clone();
                    occurrence.sources = sources.clone();
                    occurrence.note = *note;
                    rewritten.push(occurrence);
                } else {
                    // Native replay bytes alone cannot manufacture authority.
                    if message(item).is_none() {
                        return Err(ContextError::ProtectedGroup.into());
                    }
                    rewritten.push(Occurrence {
                        request: RequestId(String::new()),
                        position: 0,
                        hash: hash.clone(),
                        item: item.clone(),
                        sources: sources.clone(),
                        note: *note,
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
            retained_current_opaque = rewritten
                .iter()
                .any(|i| opaque(&i.item) && current_origins.contains(&i.origin));
        } else {
            for block in &draft.document.blocks {
                match block {
                    ContextBlock::Native { reference, .. } => {
                        let (index,stored)=available.iter().enumerate().find(|(_,b)|matches!(&b.block,ContextBlock::Native{reference:r,..} if r==reference)).ok_or(ContextError::InvalidReference)?;
                        if *block != stored.block {
                            return Err(ContextError::NativeEdit.into());
                        }
                        if index >= snapshot.blocks.len() && stored.mandatory {
                            // A past pending call cannot be resurrected without
                            // its now-completed protocol companion, nor can a
                            // historical owner instruction replace today's spine.
                            return Err(ContextError::ProtectedGroup.into());
                        }
                        if !seen.insert(reference.clone()) {
                            return Err(ContextError::InvalidReference.into());
                        }
                        let ordering = if index < snapshot.blocks.len() {
                            &mut last_native
                        } else {
                            &mut last_historical_native
                        };
                        if ordering.is_some_and(|previous| index < previous) {
                            return Err(ContextError::NativeOrder.into());
                        }
                        *ordering = Some(index);
                        retained_current_opaque |= index < snapshot.blocks.len() && stored.opaque;
                        rewritten.extend(stored.items.clone());
                    }
                    ContextBlock::Text {
                        reference: Some(reference),
                        role,
                        text,
                        sources,
                    } => {
                        let stored=available.iter().find(|b|matches!(&b.block,ContextBlock::Text{reference:Some(r),..} if r==reference)).ok_or(ContextError::InvalidReference)?;
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
                            if !occurrence.sources.contains(reference) {
                                occurrence.sources.push(reference.clone());
                            }
                            occurrence.note = true;
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
                            if !available.iter().any(|b| match &b.block {
                                ContextBlock::Text {
                                    reference: Some(r), ..
                                }
                                | ContextBlock::Native { reference: r, .. } => r == source,
                                _ => false,
                            }) && !available
                                .iter()
                                .any(|b| b.items.iter().any(|i| i.sources.contains(source)))
                            {
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
                            note: true,
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
                && (retained_current_opaque
                    || all[snapshot.prefix.len()..].iter().any(|i| opaque(&i.item)))
            {
                return Err(ContextError::OpaqueModel.into());
            }
        }
        let selected_model = draft.next_model.as_deref().or(current.model.as_deref());
        for occurrence in rewritten
            .iter()
            .filter(|i| opaque(&i.item) && !current_origins.contains(&i.origin))
        {
            let issuing_model = super::replay::response_item_model(
                &tx,
                &occurrence.origin.request,
                &occurrence.origin.hash,
            )?;
            if issuing_model
                .as_deref()
                .is_none_or(|model| Some(model) != selected_model)
            {
                return Err(ContextError::OpaqueModel.into());
            }
        }
        if cancelled() {
            return Err(ContextError::Cancelled.into());
        }
        let text_changed = rewritten
            .iter()
            .map(|i| (&i.hash, &i.sources, i.note))
            .collect::<Vec<_>>()
            != snapshot
                .prefix
                .iter()
                .map(|i| (&i.hash, &i.sources, i.note))
                .collect::<Vec<_>>();
        let model_changed = draft
            .next_model
            .as_ref()
            .is_some_and(|model| current.model.as_ref() != Some(model));
        let changed = text_changed || model_changed || draft.next_effort.is_some();
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
                note: i.note,
            })
            .collect();
        if text_changed {
            tx.execute("INSERT INTO requests(id,parent_id,branch,created_at,embedded_run,embedded_incarnation,round_phase) SELECT ?1,id,branch,?2,embedded_run,embedded_incarnation,round_phase FROM requests WHERE id=?3",params![head.0,utc_millis(),snapshot.head.0])?;
            rewritten.extend_from_slice(&all[snapshot.prefix.len()..]);
            for (position, occurrence) in rewritten.iter_mut().enumerate() {
                if occurrence.origin.request.0.is_empty() {
                    occurrence.origin.request = head.clone();
                    occurrence.origin.position = position as i64;
                }
                insert_occurrence(&tx, &head, position as i64, occurrence)?;
            }
            tx.execute(
                "INSERT INTO session_state(session_id,state,updated_at) VALUES(?1,'true',?2)",
                params![format!("harness:compaction:{}", head.0), utc_millis()],
            )?;
            // Saved native groups can come from ancestry hidden by a prior cut.
            // Carry authority from every retained invocation's immutable origin.
            let carried = carry_native_claims(&tx, &rewritten, &head, &self.store_id)?;
            for operation in pending.iter().chain(std::iter::once(&snapshot.operation)) {
                if !carried.contains(operation) {
                    return Err(ContextError::ProtectedGroup.into());
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
        if let Some(effort) = draft.next_effort {
            // The successful output separates this update from every earlier
            // setting, preserving the already-issued request's input prefix.
            Self::set_effort_tx(&tx, &head, effort)?;
            // Publication supersedes every choice still pending at commit
            // admission; later choices keep the ordinary pending boundary.
            tx.execute(
                "DELETE FROM session_state WHERE session_id=?1",
                [Self::pending_effort_key(snapshot.operation.origin.actor())],
            )?;
        }
        let receipt = ContextCommitReceipt {
            head,
            generation: current.generation,
            changed,
            model: current.model,
        };
        let deferred_head = freeze_committed_context(&tx, &receipt.head, &self.store_id)?;
        let record = ReceiptRecord {
            deferred_head,
            version: 2,
            next_effort: draft.next_effort,
            prefix: prefix_evidence,
            candidate,
            original_operation: replay.map(|e| e.original_operation.clone()),
            operation: snapshot.operation.clone(),
            output: output_hash,
            receipt: receipt.clone(),
        };
        tx.execute("INSERT INTO events(request_id,kind,payload,created_at) VALUES(?1,'context_commit',?2,?3)",params![receipt.head.0,serde_json::to_string(&record)?,utc_millis()])?;
        if cancelled() {
            return Err(ContextError::Cancelled.into());
        }
        tx.commit()?;
        Ok(receipt)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptRecord {
    version: u32,
    next_effort: Option<crate::model::Effort>,
    deferred_head: RequestId,
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
    note: bool,
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
            if record.version != 2 || record.operation != *operation {
                return Err(ContextError::UnsupportedState.into());
            }
            Ok(Some(record))
        }
        _ => Err(ContextError::UnsupportedState.into()),
    }
}

/// Deferred children inherit a bounded immutable cut, never the continuing
/// request to which future arrivals can still be appended.
fn freeze_committed_context(
    tx: &Transaction<'_>,
    head: &RequestId,
    store_id: &str,
) -> Result<RequestId> {
    let snapshot = RequestId(uuid::Uuid::new_v4().to_string());
    tx.execute(
        "INSERT INTO requests(id,parent_id,branch,created_at) VALUES(?1,NULL,?2,?3)",
        params![
            snapshot.0,
            format!("harness:context-release:{}", snapshot.0),
            utc_millis()
        ],
    )?;
    let all = history(tx, head, true)?;
    for (position, occurrence) in all.iter().enumerate() {
        insert_occurrence(tx, &snapshot, position as i64, occurrence)?;
    }
    carry_native_claims(tx, &all, &snapshot, store_id)?;
    Ok(snapshot)
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

/// A copied occurrence retains its original claimant, independent of today's
/// conversation binding and of other invocations sharing a provider call ID.
fn carry_native_claims(
    tx: &Transaction<'_>,
    occurrences: &[Occurrence],
    head: &RequestId,
    store_id: &str,
) -> Result<HashSet<OperationId>> {
    let mut carried = HashSet::new();
    for occurrence in occurrences {
        let Some(call) = occurrence
            .item
            .tool_call()
            .map_err(|_| ContextError::UnsupportedState)?
        else {
            continue;
        };
        let mut query = tx.prepare(
            "SELECT origin FROM claims WHERE request_id=?1 AND origin_request_id=?1 AND call_id=?2",
        )?;
        let origins = query
            .query_map(
                params![occurrence.origin.request.0, call.call_id.0],
                |row| row.get::<_, String>(0),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if origins.is_empty() && super::replay::is_validated_completion(tx, &occurrence.origin)? {
            continue;
        }
        let [origin] = origins.as_slice() else {
            return Err(ContextError::ProtectedGroup.into());
        };
        let operation = OperationId {
            origin: serde_json::from_str(origin)?,
            request: occurrence.origin.request.clone(),
            call: call.call_id,
        };
        if original_call(tx, &operation)? != occurrence.origin {
            return Err(ContextError::InvalidReference.into());
        }
        let (branch, run, incarnation): (String, Option<String>, Option<String>) = tx.query_row(
            "SELECT branch,embedded_run,embedded_incarnation FROM requests WHERE id=?1",
            [&operation.request.0],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let principal_matches = match &operation.origin {
            ConversationIdentity::Standalone { store, actor } => {
                store == store_id && actor.0 == branch && run.is_none() && incarnation.is_none()
            }
            ConversationIdentity::Embedded {
                run: owner_run,
                incarnation: owner_incarnation,
                actor,
            } => {
                actor.0 == branch
                    && !owner_run.is_empty()
                    && !owner_incarnation.is_empty()
                    && run.as_ref().is_none_or(|run| run == owner_run)
                    && incarnation
                        .as_ref()
                        .is_none_or(|incarnation| incarnation == owner_incarnation)
            }
        };
        if !principal_matches {
            return Err(StoreError::OperationOriginMismatch);
        }
        if carried.insert(operation.clone()) {
            carry_claim(tx, &operation, head)?;
        }
    }
    Ok(carried)
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
            tx.execute("UPDATE request_items SET source_request=?3,source_position=?4,context_sources=?5,context_note=?6 WHERE request_id=?1 AND position=?2",params![target.0,item.position,source.origin.request.0,source.origin.position,serde_json::to_string(&source.sources)?,source.note])?;
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
mod opaque_model_tests;
#[cfg(test)]
mod tests;
