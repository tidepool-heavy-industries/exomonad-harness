//! Context cuts and terminal publication share the Store connection and transaction.
use super::{Result, Store, StoreError, TerminalOutcome, embedded_round, terminal, utc_millis};
use crate::{
    context::{
        ContextBlock, ContextCommit, ContextCommitEvidence, ContextCommitReceipt, ContextDocument,
        ContextDraft, ContextError, ContextNativeKind, ContextReference, ContextRequestState,
        ContextRole, ContextSnapshot, ContextTextOverlay, ContextTextSelector, ContextVisibleText,
        Occurrence, Origin, SnapshotSeal, StoredBlock,
    },
    item::{Item, ItemHash},
    model::{AgentPath, ConversationIdentity, OperationId, RequestId},
    turn::JobOutput,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};

mod compaction_reconciliation;
mod portability_note;
pub(super) use compaction_reconciliation::{CompactionItem, reconcile as reconcile_compaction};

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

pub(crate) fn history(
    c: &Connection,
    head: &RequestId,
    boundaries: bool,
) -> Result<Vec<Occurrence>> {
    query_occurrences(c, "WITH RECURSIVE lineage(id,parent_id,depth) AS (
        SELECT id,parent_id,0 FROM requests WHERE id=?1
        UNION ALL SELECT r.id,r.parent_id,l.depth+1 FROM requests r JOIN lineage l ON r.id=l.parent_id
        WHERE ?2=0 OR NOT EXISTS(SELECT 1 FROM session_state s WHERE s.session_id='harness:compaction:'||l.id)
    ) SELECT l.id,ri.position,ri.item_hash,i.json,COALESCE(ri.source_request,ri.request_id),COALESCE(ri.source_position,ri.position),ri.context_sources,ri.context_note,ri.context_overlays,ri.output_operation
    FROM lineage l JOIN request_items ri ON ri.request_id=l.id JOIN items i ON i.hash=ri.item_hash ORDER BY l.depth DESC,ri.position", params![head.0, boundaries])
}

pub(super) fn request_occurrences(c: &Connection, request: &RequestId) -> Result<Vec<Occurrence>> {
    query_occurrences(c, "SELECT ri.request_id,ri.position,ri.item_hash,i.json,COALESCE(ri.source_request,ri.request_id),COALESCE(ri.source_position,ri.position),ri.context_sources,ri.context_note,ri.context_overlays,ri.output_operation
        FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id=?1 ORDER BY ri.position", [&request.0])
}

pub(crate) fn validate_canonical_history(c: &Connection, history: &[Occurrence]) -> Result<()> {
    super::output_publication::validate_history(c, history).map(drop)
}

pub(super) fn occurrences_for_requests(
    c: &Connection,
    requests: &HashSet<RequestId>,
) -> Result<Vec<Occurrence>> {
    let requests = requests
        .iter()
        .map(|request| request.0.as_str())
        .collect::<Vec<_>>();
    let mut occurrences = Vec::new();
    for batch in requests.chunks(250) {
        let placeholders = std::iter::repeat_n("?", batch.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT ri.request_id,ri.position,ri.item_hash,i.json,COALESCE(ri.source_request,ri.request_id),COALESCE(ri.source_position,ri.position),ri.context_sources,ri.context_note,ri.context_overlays,ri.output_operation FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id IN ({placeholders}) ORDER BY ri.request_id,ri.position"
        );
        occurrences.extend(query_occurrences(
            c,
            &sql,
            rusqlite::params_from_iter(batch.iter()),
        )?);
    }
    Ok(occurrences)
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
            r.get::<_, Option<String>>(8)?,
            r.get::<_, Option<String>>(9)?,
        ))
    })?;
    rows.map(|row| {
        let (
            request,
            position,
            hash,
            raw,
            origin_request,
            origin_position,
            sources,
            note,
            overlays,
            output_operation,
        ) = row?;
        let occurrence = Occurrence {
            request: RequestId(request),
            position,
            hash: ItemHash(hash.clone()),
            item: serde_json::from_str(&raw)?,
            output_operation: output_operation
                .map(|raw| serde_json::from_str(&raw))
                .transpose()?,
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
            overlays: overlays
                .map(|raw| serde_json::from_str(&raw))
                .transpose()?
                .unwrap_or_default(),
        };
        super::output_publication::validate_occurrence(&occurrence)?;
        Ok(occurrence)
    })
    .collect()
}

/// Attribution is a request projection; canonical retained bytes and origin
/// references remain unchanged in the Store.
pub(super) fn project_context_note(occurrence: &Occurrence) -> Result<Item> {
    let mut item = project_bodies(occurrence)?;
    if !occurrence.note {
        return Ok(item);
    }
    let Some((_, text)) = message(&item) else {
        return Ok(item);
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
    Ok(item)
}

fn text_body<'a>(item: &'a Item, selector: &ContextTextSelector) -> Option<&'a str> {
    match selector {
        ContextTextSelector::MessageText { part } => {
            if item.0["type"] != "message"
                || !matches!(item.0["role"].as_str(), Some("user" | "assistant"))
            {
                return None;
            }
            match &item.0["content"] {
                serde_json::Value::String(text) if *part == 0 => Some(text),
                serde_json::Value::Array(parts) => {
                    let part = parts.get(*part as usize)?;
                    matches!(part["type"].as_str(), Some("input_text" | "output_text"))
                        .then(|| part["text"].as_str())
                        .flatten()
                }
                _ => None,
            }
        }
        ContextTextSelector::ToolResultText => matches!(
            item.0["type"].as_str(),
            Some("function_call_output" | "custom_tool_call_output")
        )
        .then(|| item.0["output"].as_str())
        .flatten(),
    }
}

fn set_text_body(item: &mut Item, selector: &ContextTextSelector, text: &str) -> Result<()> {
    text_body(item, selector).ok_or(ContextError::InvalidReference)?;
    match selector {
        ContextTextSelector::MessageText { part } => match &mut item.0["content"] {
            serde_json::Value::String(body) => *body = text.into(),
            serde_json::Value::Array(parts) => parts[*part as usize]["text"] = json!(text),
            _ => unreachable!("validated message text"),
        },
        ContextTextSelector::ToolResultText => item.0["output"] = json!(text),
    }
    Ok(())
}

fn project_bodies(occurrence: &Occurrence) -> Result<Item> {
    let mut item = occurrence.item.clone();
    let mut seen = HashSet::new();
    for overlay in &occurrence.overlays {
        if !seen.insert(&overlay.selector) {
            return Err(ContextError::InvalidReference.into());
        }
        set_text_body(&mut item, &overlay.selector, &overlay.text)?;
    }
    Ok(item)
}

fn set_overlay(
    occurrence: &mut Occurrence,
    selector: &ContextTextSelector,
    text: &str,
) -> Result<()> {
    let original = text_body(&occurrence.item, selector).ok_or(ContextError::InvalidReference)?;
    occurrence
        .overlays
        .retain(|overlay| &overlay.selector != selector);
    if original != text {
        occurrence.overlays.push(ContextTextOverlay {
            selector: selector.clone(),
            text: text.into(),
        });
    }
    // Store overlays in selector order so repeated edits have one canonical draft.
    occurrence
        .overlays
        .sort_by_key(|overlay| match overlay.selector {
            ContextTextSelector::MessageText { part } => (0, part),
            ContextTextSelector::ToolResultText => (1, 0),
        });
    Ok(())
}

fn edited_native(stored: &StoredBlock, candidate: &ContextBlock) -> Result<Vec<Occurrence>> {
    let (
        ContextBlock::Native {
            texts: original, ..
        },
        ContextBlock::Native { texts, .. },
    ) = (&stored.block, candidate)
    else {
        return Err(ContextError::NativeEdit.into());
    };
    let mut structural = candidate.clone();
    if let (
        ContextBlock::Native { texts, preview, .. },
        ContextBlock::Native {
            preview: old_preview,
            ..
        },
    ) = (&mut structural, &stored.block)
    {
        *texts = original.clone();
        *preview = old_preview.clone();
    }
    if structural != stored.block || texts.len() != original.len() {
        return Err(ContextError::NativeEdit.into());
    }
    let mut items = stored.items.clone();
    for (field, old) in texts.iter().zip(original) {
        if field.reference != old.reference
            || field.selector != old.selector
            || field.editable != old.editable
        {
            return Err(ContextError::NativeEdit.into());
        }
        if field.text != old.text {
            if !old.editable {
                return Err(ContextError::NativeEdit.into());
            }
            let occurrence = items
                .iter_mut()
                .find(|item| reference(std::slice::from_ref(item)) == field.reference)
                .ok_or(ContextError::InvalidReference)?;
            set_overlay(occurrence, &field.selector, &field.text)?;
        }
    }
    if let (
        ContextBlock::Native {
            preview: submitted, ..
        },
        ContextBlock::Native {
            preview: original, ..
        },
    ) = (candidate, &stored.block)
    {
        if submitted != original && submitted != &preview(&items)? {
            return Err(ContextError::NativeEdit.into());
        }
    }
    Ok(items)
}

fn visible_texts(
    c: &Connection,
    owners: &HashMap<Origin, &Occurrence>,
    items: &[Occurrence],
) -> Result<Vec<ContextVisibleText>> {
    let mut fields = Vec::new();
    for occurrence in items {
        let selectors = match occurrence.item.0["type"].as_str() {
            Some("message") => match &occurrence.item.0["content"] {
                serde_json::Value::String(_) => vec![ContextTextSelector::MessageText { part: 0 }],
                serde_json::Value::Array(parts) => (0..parts.len())
                    .filter_map(|part| {
                        u32::try_from(part)
                            .ok()
                            .map(|part| ContextTextSelector::MessageText { part })
                    })
                    .collect(),
                _ => Vec::new(),
            },
            Some("function_call_output" | "custom_tool_call_output") => {
                vec![ContextTextSelector::ToolResultText]
            }
            _ => Vec::new(),
        };
        let projected = project_bodies(occurrence)?;
        for selector in selectors {
            let Some(text) = text_body(&projected, &selector) else {
                continue;
            };
            let editable = if selector == ContextTextSelector::ToolResultText {
                output_editable(c, owners.get(&occurrence.origin).copied(), occurrence)?
            } else {
                true
            };
            fields.push(ContextVisibleText {
                reference: reference(std::slice::from_ref(occurrence)),
                selector,
                text: text.into(),
                editable,
            });
        }
    }
    Ok(fields)
}

fn output_editable(c: &Connection, call: Option<&Occurrence>, output: &Occurrence) -> Result<bool> {
    let Some(call) = call else { return Ok(false) };
    let Some(id) = output.item.0["call_id"].as_str() else {
        return Ok(false);
    };
    let call_id = crate::model::CallId(id.into());
    match validate_portable_output(c, call, output, &call_id, None) {
        Ok(()) => Ok(true),
        Err(StoreError::Context(ContextError::OpaqueModel)) => Ok(false),
        Err(error) => Err(error),
    }
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

fn preview(items: &[Occurrence]) -> Result<String> {
    let mut visible = Vec::new();
    for occurrence in items {
        let item = project_bodies(occurrence)?;
        if let Some((_, text)) = message(&item) {
            visible.push(text);
        } else if let Some(parts) = item.0["content"].as_array() {
            visible.extend(
                parts
                    .iter()
                    .filter_map(|part| part["text"].as_str().map(str::to_owned)),
            );
        } else {
            for field in ["arguments", "input", "output"] {
                if let Some(text) = item.0[field].as_str() {
                    visible.push(text.into());
                    break;
                }
            }
        }
    }
    Ok(visible.join("\n"))
}

fn occurrence_positions(all: &[Occurrence]) -> HashMap<Origin, Vec<usize>> {
    let mut positions = HashMap::<Origin, Vec<usize>>::new();
    for (index, occurrence) in all.iter().enumerate() {
        positions
            .entry(occurrence.origin.clone())
            .or_default()
            .push(index);
    }
    positions
}

fn response_evidence(
    c: &Connection,
    all: &[Occurrence],
) -> Result<HashMap<Origin, std::sync::Arc<super::replay::ResponseEnvelope>>> {
    let requests = all
        .iter()
        .filter(|i| {
            opaque(&i.item)
                && !i.item.is_configuration_update()
                && !(i.item.0["type"] == "message"
                    && matches!(i.item.0["role"].as_str(), Some("system" | "developer")))
        })
        .map(|i| i.origin.request.clone())
        .collect::<HashSet<_>>();
    let mut evidence = HashMap::new();
    for request in requests {
        evidence.extend(super::replay::response_envelopes(c, &request)?);
    }
    Ok(evidence)
}

fn blocks(c: &Connection, all: &[Occurrence], cut: usize) -> Result<Vec<StoredBlock>> {
    let publication_ledger = super::output_publication::validate_history(c, all)?;
    // Group by the issuing occurrence. Wire call IDs can repeat, and an older
    // operation's output can arrive after a newer invocation.
    let mut outputs = HashMap::<Origin, Vec<usize>>::new();
    let calls = all
        .iter()
        .filter(|occurrence| {
            matches!(
                occurrence.item.0["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        })
        .map(|occurrence| (occurrence.origin.clone(), occurrence))
        .collect::<HashMap<_, _>>();
    let mut output_owners = HashMap::<Origin, &Occurrence>::new();
    for (index, occurrence) in all.iter().enumerate() {
        if matches!(
            occurrence.item.0["type"].as_str(),
            Some("function_call_output" | "custom_tool_call_output")
        ) {
            let operation = occurrence.output_operation.as_ref().ok_or_else(|| {
                StoreError::UnboundOutputPublication {
                    request: occurrence.request.clone(),
                    position: occurrence.position,
                }
            })?;
            let owner = publication_ledger
                .issuing_call(operation)
                .ok_or(ContextError::InvalidReference)?
                .clone();
            if let Some(call) = calls.get(&owner) {
                output_owners.insert(occurrence.origin.clone(), *call);
            }
            outputs.entry(owner).or_default().push(index);
        }
    }

    // The same strict evidence reader supplies request portability and editor
    // groups. Equal bytes do not enlarge a provider response's membership.
    let opaque_items = all
        .iter()
        .take(cut)
        .map(|occurrence| opaque(&occurrence.item))
        .collect::<Vec<_>>();
    let opaque_origins = all
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
        .map(|(_, occurrence)| occurrence.origin.clone())
        .collect::<HashSet<_>>();
    let positions = occurrence_positions(all);
    let evidence = response_evidence(c, all)?;
    let mut opaque_bounds = HashMap::<Origin, (usize, usize, bool)>::new();
    let mut grouped_envelopes = HashSet::new();
    for origin in opaque_origins {
        if let Some(envelope) = evidence.get(&origin) {
            if !grouped_envelopes.insert(envelope.origins[0].clone()) {
                continue;
            }
            let members = envelope
                .origins
                .iter()
                .filter_map(|o| positions.get(o))
                .collect::<Vec<_>>();
            let complete =
                members.len() == envelope.origins.len() && members.iter().all(|v| v.len() == 1);
            let bounds = members.iter().flat_map(|v| v.iter()).fold(
                None::<(usize, usize)>,
                |bounds, index| {
                    Some(bounds.map_or((*index, *index), |(start, end)| {
                        (start.min(*index), end.max(*index))
                    }))
                },
            );
            if let Some((start, end)) = bounds {
                for member in &envelope.origins {
                    opaque_bounds.insert(member.clone(), (start, end, complete));
                }
            }
        }
    }

    let mut ranges = Vec::<(usize, usize, bool, bool)>::new();
    let mut prior_calls = HashSet::<Origin>::new();
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
        if call.is_some() {
            let completion = super::replay::is_validated_completion(c, &occurrence.origin)?;
            let result = outputs.get(&occurrence.origin).and_then(|positions| {
                let next = positions.partition_point(|position| *position <= index);
                positions.get(next).copied()
            });
            let end = result.map_or(index + 1, |n| n + 1);
            ranges.push((
                index,
                end.min(cut),
                opaque_items[index],
                !completion && (result.is_none() || end > cut),
            ));
        } else if message(item).is_none() {
            let orphan = matches!(
                item.0["type"].as_str(),
                Some("function_call_output" | "custom_tool_call_output")
            ) && !occurrence
                .output_operation
                .as_ref()
                .map(|operation| original_call(c, operation))
                .transpose()?
                .is_some_and(|origin| prior_calls.contains(&origin));
            ranges.push((index, index + 1, opaque_items[index], orphan));
        }
        if call.is_some() {
            prior_calls.insert(occurrence.origin.clone());
        }
        if opaque_items[index] {
            if let Some((start, end, complete)) = opaque_bounds.get(&occurrence.origin) {
                ranges.push((*start, (*end + 1).min(cut), true, !complete || *end >= cut));
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
            let texts = visible_texts(c, &output_owners, &items)?;
            result.push(StoredBlock {
                block: ContextBlock::Native {
                    reference: reference(&items),
                    kind,
                    preview: preview(&items)?,
                    protected: mandatory,
                    texts,
                },
                items,
                opaque: is_opaque,
                mandatory,
            });
            index = end;
        } else {
            let items = vec![all[index].clone()];
            let (role, text) =
                message(&project_bodies(&items[0])?).ok_or(ContextError::InvalidReference)?;
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

/// Construct the only provider-facing projection. Stored history, native
/// references, claims and receipts continue to refer to the original bytes.
fn portable_request(
    c: &Connection,
    all: &[Occurrence],
    model: Option<&str>,
    completing: Option<&OperationId>,
) -> Result<Vec<(RequestId, Option<ItemHash>, Item, Option<Occurrence>)>> {
    let evidence = response_evidence(c, all)?;
    let positions = occurrence_positions(all);
    let operations = all
        .iter()
        .filter_map(|occurrence| occurrence.output_operation.clone())
        .collect::<HashSet<_>>();
    let invocations = super::validation::invocations_for_operations(c, &operations)?;
    let mut outputs = HashMap::<Origin, Vec<usize>>::new();
    for (index, occurrence) in all
        .iter()
        .enumerate()
        .filter(|(_, occurrence)| super::output_publication::is_tool_output(&occurrence.item))
    {
        let operation = occurrence.output_operation.as_ref().ok_or_else(|| {
            StoreError::UnboundOutputPublication {
                request: occurrence.request.clone(),
                position: occurrence.position,
            }
        })?;
        let invocation = invocations
            .get(operation)
            .ok_or(ContextError::OpaqueModel)?;
        super::validate_replay_output(
            &operation.call,
            invocation.call.input.kind(),
            &occurrence.item,
        )?;
        outputs
            .entry(invocation.occurrence.clone())
            .or_default()
            .push(index);
    }
    let mut seen = HashSet::new();
    let mut removed = HashSet::new();
    let mut notes = HashMap::new();
    for occurrence in all.iter().filter(|i| opaque(&i.item)) {
        if occurrence.item.is_configuration_update()
            || (occurrence.item.0["type"] == "message"
                && matches!(
                    occurrence.item.0["role"].as_str(),
                    Some("system" | "developer")
                ))
        {
            continue;
        }
        let envelope = evidence
            .get(&occurrence.origin)
            .ok_or(ContextError::OpaqueModel)?;
        let selected = model.ok_or(ContextError::OpaqueModel)?;
        if envelope.model == selected || !seen.insert(envelope.origins[0].clone()) {
            continue;
        }
        // A visible edit preserves opaque continuity; changing models must not
        // silently discard that continuity through the portability renderer.
        if all.iter().any(|occurrence| !occurrence.overlays.is_empty()) {
            return Err(ContextError::OpaqueModel.into());
        }
        if envelope.kind == super::replay::ResponseEnvelopeKind::ServerCompaction {
            return Err(ContextError::OpaqueModel.into());
        }
        let mut members = Vec::with_capacity(envelope.origins.len());
        for origin in &envelope.origins {
            let Some(indices) = positions.get(origin) else {
                return Err(ContextError::OpaqueModel.into());
            };
            let [index] = indices.as_slice() else {
                return Err(ContextError::OpaqueModel.into());
            };
            if members.last().is_some_and(|previous| previous >= index) {
                return Err(ContextError::OpaqueModel.into());
            }
            members.push(*index);
        }
        let mut paired_outputs = HashSet::new();
        let mut membership = members.iter().copied().collect::<HashSet<_>>();
        for index in members.clone() {
            let occurrence = &all[index];
            let Some(call) = occurrence
                .item
                .tool_call()
                .map_err(|_| ContextError::OpaqueModel)?
            else {
                continue;
            };
            if super::replay::is_validated_completion(c, &occurrence.origin)? {
                continue;
            }
            let expected_kind = match call.input.kind() {
                crate::item::ToolKind::Function => "function_call_output",
                crate::item::ToolKind::Custom => "custom_tool_call_output",
            };
            let results = outputs
                .get(&occurrence.origin)
                .ok_or(ContextError::OpaqueModel)?;
            let first = results.partition_point(|position| *position <= index);
            let [output_index] = &results[first..] else {
                return Err(ContextError::OpaqueModel.into());
            };
            let output = &all[*output_index];
            if output.item.0["type"] != expected_kind || !paired_outputs.insert(*output_index) {
                return Err(ContextError::OpaqueModel.into());
            }
            validate_portable_output(c, occurrence, output, &call.call_id, completing)?;
            if membership.insert(*output_index) {
                members.push(*output_index);
            }
        }
        members.sort_unstable();
        for index in &members {
            if matches!(
                all[*index].item.0["type"].as_str(),
                Some("function_call_output" | "custom_tool_call_output")
            ) && !paired_outputs.contains(index)
            {
                return Err(ContextError::OpaqueModel.into());
            }
            if !removed.insert(*index) {
                return Err(ContextError::OpaqueModel.into());
            }
        }
        let items = members
            .iter()
            .map(|index| all[*index].clone())
            .collect::<Vec<_>>();
        notes.insert(members[0], portability_note::render(&items)?);
    }
    let mut projected = Vec::with_capacity(all.len());
    for (index, occurrence) in all.iter().enumerate() {
        if let Some(note) = notes.remove(&index) {
            projected.push((occurrence.request.clone(), None, note, None));
        } else if !removed.contains(&index) {
            projected.push((
                occurrence.request.clone(),
                (!occurrence.note && occurrence.overlays.is_empty())
                    .then(|| occurrence.hash.clone()),
                project_context_note(occurrence)?,
                Some(occurrence.clone()),
            ));
        }
    }
    Ok(projected)
}

/// A retained output closes its original invocation only when Store owns that
/// exact result. The committing call's real successful output is settled in
/// this same transaction and is the sole prospective exception.
fn validate_portable_output(
    c: &Connection,
    occurrence: &Occurrence,
    output: &Occurrence,
    call: &crate::model::CallId,
    completing: Option<&OperationId>,
) -> Result<()> {
    let operation =
        output
            .output_operation
            .as_ref()
            .ok_or_else(|| StoreError::UnboundOutputPublication {
                request: output.request.clone(),
                position: output.position,
            })?;
    if operation.call != *call {
        return Err(ContextError::OpaqueModel.into());
    }
    if original_call(c, &operation)? != occurrence.origin {
        return Err(ContextError::OpaqueModel.into());
    }
    if Some(operation) != completing
        && terminal::exact_terminal(c, &operation)?.is_none_or(|(hash, _)| hash != output.hash)
    {
        return Err(ContextError::OpaqueModel.into());
    }
    Ok(())
}

pub(crate) fn insert_occurrence(
    tx: &Transaction<'_>,
    request: &RequestId,
    position: i64,
    occurrence: &Occurrence,
) -> Result<()> {
    super::output_publication::validate_occurrence(occurrence)?;
    if super::output_publication::is_tool_output(&occurrence.item)
        && occurrence.output_operation.is_none()
    {
        return Err(StoreError::UnboundOutputPublication {
            request: occurrence.request.clone(),
            position: occurrence.position,
        });
    }
    tx.execute("INSERT INTO request_items(request_id,position,item_hash,source_request,source_position,context_sources,context_note,context_overlays,output_operation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![request.0,position,occurrence.hash.0,occurrence.origin.request.0,occurrence.origin.position,serde_json::to_string(&occurrence.sources)?,occurrence.note,serde_json::to_string(&occurrence.overlays)?,occurrence.output_operation.as_ref().map(serde_json::to_string).transpose()?])?;
    Ok(())
}

pub(super) fn original_call(c: &Connection, operation: &OperationId) -> Result<Origin> {
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
        let _projection =
            tracing::debug_span!(target: "harness::runtime_cost", "context_request_state",
            request_id = %head.0, actor = %identity.actor().0)
            .entered();
        let mut c = {
            let _wait = tracing::debug_span!(target: "harness::runtime_cost", "sqlite_mutex_wait")
                .entered();
            self.lock()
        };
        let transaction_span = tracing::debug_span!(target: "harness::runtime_cost", "projected_history_transaction", output_items = tracing::field::Empty, committed = false);
        let _transaction = transaction_span.enter();
        let tx = c.transaction()?;
        validate_identity(&tx, &self.store_id, identity)?;
        if branch(&tx, head)? != identity.actor().0 {
            return Err(StoreError::OperationOriginMismatch);
        }
        let current = state(&tx, identity)?;
        let canonical = {
            let _read =
                tracing::debug_span!(target: "harness::runtime_cost", "lineage_query_and_decode")
                    .entered();
            history(&tx, head, true)?
        };
        validate_canonical_history(&tx, &canonical)?;
        let projected = {
            let _projection = tracing::debug_span!(target: "harness::runtime_cost", "portable_history_projection", input_items = canonical.len()).entered();
            portable_request(&tx, &canonical, current.model.as_deref(), None)?
        };
        drop(canonical);
        let mut history = Vec::with_capacity(projected.len());
        let mut occurrences = Vec::with_capacity(projected.len());
        for (request, hash, item, occurrence) in projected {
            let hash = match hash {
                Some(hash) => hash,
                None => Self::put_item_tx(&tx, &item)?,
            };
            history.push((request, hash, item));
            occurrences.push(occurrence);
        }
        tx.commit()?;
        transaction_span.record("output_items", history.len());
        transaction_span.record("committed", true);
        Ok(ContextRequestState {
            history,
            occurrences,
            model: current.model,
            generation: current.generation,
        })
    }

    pub(crate) fn history_occurrences(&self, head: &RequestId) -> Result<Vec<Occurrence>> {
        history(&self.lock(), head, true)
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
        if !matches!(record.version, 2 | 3)
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
            .map(|p| {
                Ok((
                    load(&p.hash)?,
                    p.sources.clone(),
                    p.note,
                    p.overlays.clone(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Some(ContextCommitEvidence {
            version: record.version,
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
                    .map(|(i, s, note, overlays)| (i, s, note, overlays))
                    .collect::<Vec<_>>(),
                &evidence.model,
                evidence.next_effort,
            ))?
        } else {
            serde_json::to_string(draft)?
        };
        if let Some(record) = receipt(&tx, &snapshot.operation)? {
            let equivalent = record.candidate == candidate
                || (record.version == 2
                    && legacy_candidate_matches(&record.candidate, replay, draft, snapshot)?);
            if record.output != output_hash || !equivalent {
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
            // Equal current bytes cannot prove a native segment's owner.
            // Include bounded ancestry before checking whole-group uniqueness.
            evidence
                .prefix
                .iter()
                .any(|(item, _, _, _)| message(item).is_none())
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
        if let Some(evidence) = replay {
            let mut native = HashMap::<ItemHash, Vec<&StoredBlock>>::new();
            let mut text = HashMap::<ItemHash, Vec<&Occurrence>>::new();
            for stored in &available {
                match &stored.block {
                    ContextBlock::Native { .. } => {
                        native
                            .entry(stored.items[0].hash.clone())
                            .or_default()
                            .push(stored);
                    }
                    ContextBlock::Text { .. } => {
                        text.entry(stored.items[0].hash.clone())
                            .or_default()
                            .push(&stored.items[0]);
                    }
                }
            }
            let mut used_origins = HashSet::new();
            let mut cursor = 0;
            while cursor < evidence.prefix.len() {
                let (item, sources, note, overlays) = &evidence.prefix[cursor];
                let hash = Self::put_item_tx(&tx, item)?;
                let matching = native
                    .get(&hash)
                    .into_iter()
                    .flatten()
                    .filter(|stored| {
                        let end = cursor + stored.items.len();
                        let current = matches!(&stored.block, ContextBlock::Native {reference,..} if current_refs.contains(reference));
                        (!stored.mandatory || current)
                            && end <= evidence.prefix.len()
                            && stored.items.iter().zip(&evidence.prefix[cursor..end]).all(
                                |(local, (item, _, _, _))| {
                                    local.item == *item && !used_origins.contains(&local.origin)
                                },
                            )
                    })
                    .copied()
                    .collect::<Vec<_>>();
                match matching.as_slice() {
                    [stored] => {
                        // A native segment has one local owner. Resolve every
                        // occurrence together so equal result bytes cannot splice
                        // another invocation's companion into the restored group.
                        for occurrence in &stored.items {
                            used_origins.insert(occurrence.origin.clone());
                        }
                        let mut candidate = stored.block.clone();
                        let mut restored = stored.items.clone();
                        for (local, (_, _, _, overlays)) in
                            restored.iter_mut().zip(&evidence.prefix[cursor..])
                        {
                            local.overlays = overlays.clone();
                            project_bodies(local)?;
                        }
                        if let ContextBlock::Native { texts, .. } = &mut candidate {
                            for field in texts {
                                let local = restored
                                    .iter()
                                    .find(|item| {
                                        reference(std::slice::from_ref(item)) == field.reference
                                    })
                                    .ok_or(ContextError::InvalidReference)?;
                                field.text = text_body(&project_bodies(local)?, &field.selector)
                                    .ok_or(ContextError::InvalidReference)?
                                    .into();
                            }
                        }
                        rewritten.extend(edited_native(stored, &candidate)?);
                        cursor += stored.items.len();
                        continue;
                    }
                    [] => {}
                    _ => return Err(ContextError::ProtectedGroup.into()),
                }
                if message(item).is_none() {
                    return Err(ContextError::ProtectedGroup.into());
                }
                if let Some(existing) = text
                    .get(&hash)
                    .into_iter()
                    .flatten()
                    .find(|i| !used_origins.contains(&i.origin) && i.item == *item)
                {
                    used_origins.insert(existing.origin.clone());
                    let mut occurrence = (*existing).clone();
                    occurrence.sources = sources.clone();
                    occurrence.note = *note;
                    occurrence.overlays = overlays.clone();
                    project_bodies(&occurrence)?;
                    rewritten.push(occurrence);
                } else {
                    rewritten.push(Occurrence {
                        request: RequestId(String::new()),
                        position: 0,
                        hash: hash.clone(),
                        item: item.clone(),
                        output_operation: None,
                        sources: sources.clone(),
                        note: *note,
                        overlays: overlays.clone(),
                        origin: Origin {
                            request: RequestId(String::new()),
                            position: 0,
                            hash,
                        },
                    });
                }
                cursor += 1;
            }
            // Replay must preserve every protected local envelope as an intact
            // group; foreign evidence cannot replace local pending operations.
            for stored in &snapshot.blocks {
                if (stored.mandatory || (stored.opaque && evidence.version >= 3))
                    && !rewritten.windows(stored.items.len()).any(|w| {
                        w.iter()
                            .map(|i| (&i.item, &i.origin))
                            .eq(stored.items.iter().map(|i| (&i.item, &i.origin)))
                    })
                {
                    return Err(ContextError::ProtectedGroup.into());
                }
            }
        } else {
            for block in &draft.document.blocks {
                match block {
                    ContextBlock::Native { reference, .. } => {
                        let (index,stored)=available.iter().enumerate().find(|(_,b)|matches!(&b.block,ContextBlock::Native{reference:r,..} if r==reference)).ok_or(ContextError::InvalidReference)?;
                        let edited = edited_native(stored, block)?;
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
                        rewritten.extend(edited);
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
                        if message(&project_bodies(&occurrence)?)
                            .is_none_or(|(_, old)| old != *text)
                        {
                            set_overlay(
                                &mut occurrence,
                                &ContextTextSelector::MessageText { part: 0 },
                                text,
                            )?;
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
                            output_operation: None,
                            sources: sources.clone(),
                            note: true,
                            overlays: Vec::new(),
                        });
                    }
                }
            }
            for stored in &snapshot.blocks {
                if (stored.mandatory || stored.opaque)
                    && !matches!(&stored.block, ContextBlock::Native {reference, ..} if seen.contains(reference))
                {
                    return Err(ContextError::ProtectedGroup.into());
                }
            }
        }
        if let Some(model) = &draft.next_model {
            if model.trim().is_empty() {
                return Err(ContextError::InvalidModel.into());
            }
        }
        let selected_model = draft.next_model.as_deref().or(current.model.as_deref());
        let mut prospective = rewritten.clone();
        prospective.extend_from_slice(&all[snapshot.prefix.len()..]);
        prospective.push(Occurrence {
            request: snapshot.head.clone(),
            position: i64::MAX,
            hash: output_hash.clone(),
            item: output_item,
            output_operation: Some(snapshot.operation.clone()),
            origin: Origin {
                request: snapshot.head.clone(),
                position: i64::MAX,
                hash: output_hash.clone(),
            },
            sources: Vec::new(),
            note: false,
            overlays: Vec::new(),
        });
        portable_request(&tx, &prospective, selected_model, Some(&snapshot.operation))?;
        if cancelled() {
            return Err(ContextError::Cancelled.into());
        }
        let text_changed = rewritten
            .iter()
            .map(|i| (&i.hash, &i.sources, i.note, &i.overlays))
            .collect::<Vec<_>>()
            != snapshot
                .prefix
                .iter()
                .map(|i| (&i.hash, &i.sources, i.note, &i.overlays))
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
                overlays: i.overlays.clone(),
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
        super::output_publication::append_tx(
            &tx,
            &snapshot.operation,
            &snapshot.operation.request,
            &head,
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
            version: 3,
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
    #[serde(default)]
    overlays: Vec<ContextTextOverlay>,
}

fn legacy_candidate_matches(
    candidate: &str,
    replay: Option<&ContextCommitEvidence>,
    draft: &ContextDraft,
    snapshot: &ContextSnapshot,
) -> Result<bool> {
    if let Some(evidence) = replay {
        if evidence
            .prefix
            .iter()
            .any(|(_, _, _, overlays)| !overlays.is_empty())
        {
            return Ok(false);
        }
        return Ok(candidate
            == serde_json::to_string(&(
                evidence
                    .prefix
                    .iter()
                    .map(|(item, sources, note, _)| (item, sources, note))
                    .collect::<Vec<_>>(),
                &evidence.model,
                evidence.next_effort,
            ))?);
    }
    let Ok(mut old) = serde_json::from_str::<ContextDraft>(candidate) else {
        return Ok(false);
    };
    let mut current = draft.clone();
    for block in &mut old.document.blocks {
        if let ContextBlock::Native { texts, .. } = block {
            texts.clear();
        }
    }
    for block in &mut current.document.blocks {
        if let ContextBlock::Native {
            reference, texts, ..
        } = block
        {
            if !texts.is_empty()
                && !snapshot.blocks.iter().any(|original| {
                    matches!(&original.block,
                ContextBlock::Native {reference: original_reference, texts: original_texts, ..}
                    if original_reference == reference && original_texts == texts)
                })
            {
                return Ok(false);
            }
            texts.clear();
        }
    }
    Ok(old == current)
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
            if !matches!(record.version, 2 | 3)
                || record.operation != *operation
                || (record.version == 2
                    && record.prefix.iter().any(|entry| !entry.overlays.is_empty()))
            {
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
    validate_canonical_history(tx, &all)?;
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
            carry_claim(tx, &operation, &occurrence.request, head, occurrences)?;
        }
    }
    Ok(carried)
}

pub(super) fn carry_operation_claim(
    tx: &Transaction<'_>,
    operation: &OperationId,
    source: &RequestId,
    target: &RequestId,
) -> Result<()> {
    // Copy the nearest claimant's terminal state inside the publication transaction.
    // Settlement before a copy must not create a fresh pending claimant.
    let n = tx.execute("WITH RECURSIVE lineage(id,depth) AS (SELECT ?4,0 UNION ALL SELECT r.parent_id,l.depth+1 FROM requests r JOIN lineage l ON r.id=l.id WHERE r.parent_id IS NOT NULL), candidates AS (SELECT c.*,l.depth FROM claims c JOIN lineage l ON c.request_id=l.id WHERE c.origin=?1 AND c.origin_request_id=?2 AND c.call_id=?3 UNION ALL SELECT c.*,9223372036854775807 FROM claims c WHERE c.origin=?1 AND c.origin_request_id=?2 AND c.call_id=?3 AND c.request_id=c.origin_request_id) INSERT INTO claims(origin,origin_request_id,call_id,request_id,state,output_hash,terminal_json) SELECT origin,origin_request_id,call_id,?5,state,output_hash,terminal_json FROM candidates ORDER BY depth LIMIT 1", params![serde_json::to_string(&operation.origin)?,operation.request.0,operation.call.0,source.0,target.0])?;
    if n != 1 {
        return Err(ContextError::ProtectedGroup.into());
    }
    Ok(())
}

fn carry_claim(
    tx: &Transaction<'_>,
    operation: &OperationId,
    source: &RequestId,
    head: &RequestId,
    occurrences: &[Occurrence],
) -> Result<()> {
    let issuing_call = original_call(tx, operation)?;
    if occurrences
        .iter()
        .filter(|occurrence| occurrence.origin == issuing_call)
        .count()
        != 1
    {
        return Err(ContextError::ProtectedGroup.into());
    }
    carry_operation_claim(tx, operation, source, head)
}

pub(super) fn compaction_generation(
    tx: &Transaction<'_>,
    store_id: &str,
    source: &RequestId,
    target: &RequestId,
    branch: &str,
) -> Result<()> {
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
mod overlay_tests;
#[cfg(test)]
mod portability_tests;
#[cfg(test)]
mod tests;
