//! Hook authorship is distinct from editable agent context notes.
use super::{Result, Store, StoreError, chat, utc_millis, validate_replay_output, validation};
use crate::{
    context::{Occurrence, Origin},
    invocation::HookAnnotation,
    item::{Item, ItemHash},
    model::OperationId,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub(super) const EVENT_KIND: &str = "model_tool_note_publication";

#[derive(Serialize, Deserialize, PartialEq)]
struct Publication {
    operation: OperationId,
    result_event: i64,
    annotation_event: i64,
    origin: Origin,
}

fn invalid(event: i64) -> StoreError {
    StoreError::InvalidReplayOwnership { event }
}

fn annotation_item(
    c: &Connection,
    operation: &OperationId,
    result_event: i64,
    annotation_event: i64,
) -> Result<Option<Item>> {
    let event = |id: i64, kind: &str| -> Result<serde_json::Value> {
        let text: Option<String> = c
            .query_row(
                "SELECT payload FROM events WHERE id=?1 AND request_id=?2 AND kind=?3",
                params![id, operation.request.0, kind],
                |r| r.get(0),
            )
            .optional()?;
        Ok(serde_json::from_str(&text.ok_or_else(|| invalid(id))?)?)
    };
    let result = event(result_event, "model_tool_result")?;
    let recorded: OperationId = serde_json::from_value(result["operation"].clone())?;
    if recorded != *operation {
        return Err(invalid(result_event));
    }
    let invocations =
        validation::invocations_for_operations(c, &HashSet::from([operation.clone()]))?;
    let issued = invocations
        .get(operation)
        .ok_or_else(|| invalid(result_event))?;
    let output: Item = serde_json::from_value(result["output"].clone())?;
    validate_replay_output(&operation.call, issued.call.input.kind(), &output)?;
    // Cleanup may retain a completed terminal without publishing ordinary
    // history. The terminal owner validates all exact claims and formats.
    let (hash, terminal, _) =
        super::terminal::exact_terminal(c, operation)?.ok_or_else(|| invalid(result_event))?;
    if !matches!(
        terminal,
        super::terminal::TerminalOutcome::Success | super::terminal::TerminalOutcome::Failure(_)
    ) {
        return Err(invalid(result_event));
    }
    let retained: String =
        c.query_row("SELECT json FROM items WHERE hash=?1", [&hash.0], |row| {
            row.get(0)
        })?;
    if serde_json::from_str::<Item>(&retained)? != output {
        return Err(invalid(result_event));
    }
    let annotation = event(annotation_event, "model_tool_annotation")?;
    if annotation["ordinal"].as_i64() != Some(result_event) || annotation_event <= result_event {
        return Err(invalid(annotation_event));
    }
    let annotation: HookAnnotation = serde_json::from_value(annotation["annotation"].clone())?;
    let text = match annotation {
        HookAnnotation::Annotated(text)
        | HookAnnotation::Pruned {
            annotation: text, ..
        } => text,
        HookAnnotation::NoAnnotation | HookAnnotation::Abstained(_) => return Ok(None),
    };
    if text.is_empty() {
        return Ok(None);
    }
    Ok(Some(Item(
        serde_json::json!({"type":"message","role":"developer","content":[{"type":"input_text","text":text}]}),
    )))
}

fn validate_publication(c: &Connection, publication: &Publication) -> Result<Item> {
    if publication.origin.request != publication.operation.request {
        return Err(invalid(publication.annotation_event));
    }
    let expected = annotation_item(
        c,
        &publication.operation,
        publication.result_event,
        publication.annotation_event,
    )?
    .ok_or_else(|| invalid(publication.annotation_event))?;
    let raw: Option<String> = c.query_row("SELECT i.json FROM request_items ri JOIN items i ON i.hash=ri.item_hash WHERE ri.request_id=?1 AND ri.position=?2 AND ri.item_hash=?3 AND ri.context_note=0",params![publication.origin.request.0,publication.origin.position,publication.origin.hash.0],|r|r.get(0)).optional()?;
    if raw
        .map(|s| serde_json::from_str::<Item>(&s))
        .transpose()?
        .as_ref()
        != Some(&expected)
    {
        return Err(invalid(publication.annotation_event));
    }
    Ok(expected)
}

impl Store {
    /// Derive the unchanged protected developer message from exact committed hook facts.
    pub(crate) fn append_model_tool_annotation(
        &self,
        operation: &OperationId,
        result_event: i64,
        annotation_event: i64,
    ) -> Result<Option<ItemHash>> {
        let mut c = self.lock()?;
        c.write_transaction(|tx| {
            let item = annotation_item(tx, operation, result_event, annotation_event)?;
            let records = {
                let mut q = tx.prepare(
                    "SELECT payload FROM events WHERE kind=?1 AND request_id=?2 ORDER BY id",
                )?;
                q.query_map(params![EVENT_KIND, operation.request.0], |r| {
                    r.get::<_, String>(0)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
            };
            for raw in records {
                let proof: Publication = serde_json::from_str(&raw)?;
                if proof.annotation_event == annotation_event {
                    if proof.operation != *operation || proof.result_event != result_event {
                        return Err(invalid(annotation_event));
                    }
                    validate_publication(tx, &proof)?;
                    return Ok(Some(proof.origin.hash));
                }
            }
            let Some(item) = item else {
                return Ok(None);
            };
            let position: i64 = tx.query_row(
                "SELECT COALESCE(MAX(position)+1,0) FROM request_items WHERE request_id=?1",
                [&operation.request.0],
                |r| r.get(0),
            )?;
            let hash = Self::put_item_tx(tx, &item)?;
            tx.execute(
                "INSERT INTO request_items(request_id,position,item_hash) VALUES (?1,?2,?3)",
                params![operation.request.0, position, hash.0],
            )?;
            chat::publish(tx, &operation.request, position, &hash, &item)?;
            let proof = Publication {
                operation: operation.clone(),
                result_event,
                annotation_event,
                origin: Origin {
                    request: operation.request.clone(),
                    position,
                    hash: hash.clone(),
                },
            };
            tx.execute(
                "INSERT INTO events(request_id,kind,payload,created_at) VALUES (?1,?2,?3,?4)",
                params![
                    operation.request.0,
                    EVENT_KIND,
                    serde_json::to_string(&proof)?,
                    utc_millis()
                ],
            )?;
            Ok(Some(hash))
        })
    }

    pub(super) fn model_tool_note_origins(
        &self,
        occurrences: &[Option<Occurrence>],
    ) -> Result<HashSet<Origin>> {
        let sources = occurrences
            .iter()
            .flatten()
            .filter(|o| !o.note && o.item.0["type"] == "message" && o.item.0["role"] == "developer")
            .map(|o| o.origin.clone())
            .collect::<HashSet<_>>();
        let requests = sources
            .iter()
            .map(|o| o.request.clone())
            .collect::<HashSet<_>>();
        let c = self.lock()?;
        let mut origins = HashSet::new();
        for request in requests {
            let mut q = c.prepare(
                "SELECT payload FROM events WHERE kind=?1 AND request_id=?2 ORDER BY id",
            )?;
            let records = q
                .query_map(params![EVENT_KIND, request.0], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for raw in records {
                let proof: Publication = serde_json::from_str(&raw)?;
                if sources.contains(&proof.origin) {
                    let expected = validate_publication(&c, &proof)?;
                    if occurrences.iter().flatten().any(|occurrence| {
                        occurrence.origin == proof.origin
                            && occurrence.item == expected
                            && occurrence.overlays.is_empty()
                            && !occurrence.note
                    }) {
                        origins.insert(proof.origin);
                    }
                }
            }
        }
        Ok(origins)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        item::ToolKind,
        model::{CallId, RequestId},
        provider::ToolFailure,
        turn::JobOutput,
    };
    use serde_json::json;

    #[test]
    fn retained_unpublished_completed_outputs_issue_exact_hook_authorship_after_reopen() {
        let path =
            std::env::temp_dir().join(format!("harness-hook-{}.sqlite", uuid::Uuid::new_v4()));
        let mut retained = Vec::new();
        {
            let store = Store::open(&path).unwrap();
            for (index, output) in [
                JobOutput::Completed(Ok(json!("success"))),
                JobOutput::Completed(Err(ToolFailure::from("failure"))),
            ]
            .into_iter()
            .enumerate()
            {
                let request = RequestId(format!("unpublished-{index}"));
                store.create_request(&request, None, "/root").unwrap();
                let call = CallId("same-call".into());
                store.append_items(&request,&[Item(json!({"type":"function_call","call_id":call.0,"name":"probe","arguments":"{}"}))]).unwrap();
                let operation = store.claim(&call, &request).unwrap();
                store
                    .write_job_output(&operation, ToolKind::Function, &output)
                    .unwrap();
                let output = store.replay_output_operation(&operation).unwrap().unwrap();
                let result = store
                    .record_event(
                        Some(&request),
                        "model_tool_result",
                        &json!({"operation":operation,"output":output}),
                    )
                    .unwrap();
                let annotation=store.record_event(Some(&request),"model_tool_annotation",&json!({"ordinal":result,"annotation":HookAnnotation::Annotated("retained cleanup hook".into())})).unwrap();
                let hash = store
                    .append_model_tool_annotation(&operation, result, annotation)
                    .unwrap()
                    .unwrap();
                let history = store.history_occurrences(&request).unwrap();
                assert!(history.iter().all(|o| o.output_operation.is_none()));
                let note = history.iter().find(|o| o.hash == hash).unwrap();
                assert!(!note.note);
                assert_eq!(
                    store
                        .append_model_tool_annotation(&operation, result, annotation)
                        .unwrap(),
                    Some(hash.clone())
                );
                retained.push((operation, result, annotation, hash));
            }
        }
        {
            let store = Store::open(&path).unwrap();
            for (operation, result, annotation, hash) in retained {
                let history = store
                    .history_occurrences(&operation.request)
                    .unwrap()
                    .into_iter()
                    .map(Some)
                    .collect::<Vec<_>>();
                let origins = store.model_tool_note_origins(&history).unwrap();
                assert!(origins.iter().any(|origin| origin.hash == hash));
                assert_eq!(
                    store
                        .append_model_tool_annotation(&operation, result, annotation)
                        .unwrap(),
                    Some(hash)
                );
                assert!(
                    history
                        .iter()
                        .flatten()
                        .all(|o| o.output_operation.is_none())
                );
            }
        }
        std::fs::remove_file(path).unwrap();
    }
}
