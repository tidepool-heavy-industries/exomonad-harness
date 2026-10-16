//! Canonical tool results and their separately certified model presentation.

use super::*;
use crate::{embedding::HostIdentity, provider::ModelVisibleItemCandidate};

pub(super) fn output_kind(item: &Item) -> Result<crate::item::ToolKind> {
    match item.0["type"].as_str() {
        Some("function_call_output") => Ok(crate::item::ToolKind::Function),
        Some("custom_tool_call_output") => Ok(crate::item::ToolKind::Custom),
        _ => Err(StoreError::OperationOriginMismatch),
    }
}

impl Store {
    /// Publish an alternate model body for one already durable exact operation.
    /// The canonical output remains the operation's immutable settlement.
    pub fn publish_model_output_projection(
        &self,
        operation: &OperationId,
        replacement: serde_json::Value,
    ) -> Result<ModelVisibleItemCandidate> {
        let original = self
            .replay_output_operation(operation)?
            .ok_or(StoreError::OperationOriginMismatch)?;
        let kind = output_kind(&original)?;
        let candidate = Item::tool_output(
            &operation.call,
            kind,
            &crate::turn::JobOutput::Completed(Ok(replacement.clone())),
        );
        let result = serde_json::to_string(&serde_json::json!({
            "operation": operation, "output": original,
        }))?;
        let mut connection = self.lock()?;
        connection.write_transaction(|tx| {
            if let Some(existing) = retained_candidates(tx, operation, &original)?.into_iter().next() {
                if existing.item == candidate {
                    return Ok(existing);
                }
                return Err(StoreError::OperationOriginMismatch);
            }
            tx.execute(
                "INSERT INTO events(request_id,kind,payload,created_at) VALUES (?1,'model_tool_result',?2,?3)",
                params![operation.request.0, result, utc_millis()],
            )?;
            let result_event = tx.last_insert_rowid();
            let annotation = crate::invocation::HookAnnotation::Pruned {
                replacement, annotation: String::new(),
            };
            tx.execute(
                "INSERT INTO events(request_id,kind,payload,created_at) VALUES (?1,'model_tool_annotation',?2,?3)",
                params![operation.request.0, serde_json::to_string(&serde_json::json!({
                    "ordinal": result_event, "annotation": annotation,
                }))?, utc_millis()],
            )?;
            Ok(ModelVisibleItemCandidate {
                item: candidate, result_event, annotation_event: tx.last_insert_rowid(),
            })
        })
    }

    /// Read canonical output only through the caller's current exact binding.
    /// Membership follows retained original calls through certified binding
    /// transfers; a matching path or reused call ID alone never authorizes it.
    pub fn inspect_embedded_result(
        &self,
        host: &HostIdentity,
        operation: &OperationId,
    ) -> Result<Option<RecordedToolOutput>> {
        let mut connection = self.lock()?;
        connection.write_transaction(|tx| {
            match &operation.origin {
                crate::model::ConversationIdentity::Embedded { run, actor, .. }
                    if run == &host.run && actor == &host.actor => {},
                _ => return Err(StoreError::OperationOriginMismatch),
            }
            let frontier = embedded_round::frontier(tx, host)?;
            let head = frontier.pending_head.as_ref().or(frontier.settled_head.as_ref())
                .ok_or(StoreError::OperationOriginMismatch)?;
            let call = context::original_call(tx, operation)?;
            // Inspect only exact retained membership. Decoding every historical
            // result here would make one terminal publication grow with all prior
            // semantic payloads in the conversation.
            let membership = context::query_occurrences(tx, "WITH RECURSIVE lineage(id,parent_id) AS (
                SELECT id,parent_id FROM requests WHERE id=?1
                UNION ALL SELECT r.id,r.parent_id FROM requests r JOIN lineage l ON r.id=l.parent_id
                WHERE NOT EXISTS(SELECT 1 FROM session_state s WHERE s.session_id='harness:compaction:'||l.id)
            ) SELECT ri.request_id,ri.position,ri.item_hash,i.json,COALESCE(ri.source_request,ri.request_id),COALESCE(ri.source_position,ri.position),ri.context_sources,ri.context_note,ri.context_overlays,ri.output_operation
            FROM lineage l JOIN request_items ri ON ri.request_id=l.id JOIN items i ON i.hash=ri.item_hash
            WHERE (COALESCE(ri.source_request,ri.request_id)=?2 AND COALESCE(ri.source_position,ri.position)=?3 AND ri.item_hash=?4)
               OR ri.output_operation=?5", params![head.0, call.request.0, call.position, call.hash.0, serde_json::to_string(operation)?])?;
            context::validate_canonical_history(tx, &membership)?;
            if membership.is_empty() {
                return Err(StoreError::OperationOriginMismatch);
            }
            let raw: Option<String> = tx.query_row(
                "SELECT i.json FROM claims c JOIN items i ON i.hash=c.output_hash WHERE c.origin=?1 AND c.origin_request_id=?2 AND c.call_id=?3 AND c.state='settled' LIMIT 1",
                params![serde_json::to_string(&operation.origin)?, operation.request.0, operation.call.0],
                |row| row.get(0),
            ).optional()?;
            raw.map(|raw| {
                let item: Item = serde_json::from_str(&raw)?;
                let kind = invocation_kind(tx, &operation.request, &operation.call)?
                    .ok_or(StoreError::OperationOriginMismatch)?;
                validate_replay_output(&operation.call, kind, &item)?;
                let (_, terminal, output_format) = terminal::exact_terminal(tx, operation)?
                    .ok_or(StoreError::OperationOriginMismatch)?;
                if item.output_format()? != output_format {
                    return Err(StoreError::ConflictingReplayOutcome { operation: operation.clone() });
                }
                Ok(RecordedToolOutput { item, terminal, output_format })
            }).transpose()
        })
    }
}

/// Recover a durable projection and authenticate it through the same owner used
/// by provider requests. GUI and model readers share this exact projection.
fn retained_candidates(
    connection: &Connection,
    operation: &OperationId,
    canonical: &Item,
) -> Result<Vec<ModelVisibleItemCandidate>> {
    let mut query = connection.prepare(
        "SELECT r.id,a.id,r.payload,a.payload FROM events a JOIN events r ON r.id=json_extract(a.payload,'$.ordinal') WHERE a.kind='model_tool_annotation' AND r.kind='model_tool_result' AND r.request_id=?1 AND a.request_id=r.request_id AND json_extract(r.payload,'$.operation.call')=?2 ORDER BY a.id DESC"
    )?;
    let rows = query.query_map(params![operation.request.0, operation.call.0], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    #[derive(serde::Deserialize)]
    struct Original {
        operation: OperationId,
        output: Item,
    }
    #[derive(serde::Deserialize)]
    struct Annotation {
        ordinal: i64,
        annotation: crate::invocation::HookAnnotation,
    }
    let mut candidates = Vec::new();
    for row in rows {
        let (result_event, annotation_event, original, annotation) = row?;
        let original: Original = serde_json::from_str(&original)?;
        let annotation: Annotation = serde_json::from_str(&annotation)?;
        if original.operation != *operation
            || original.output != *canonical
            || annotation.ordinal != result_event
        {
            continue;
        }
        if let crate::invocation::HookAnnotation::Pruned { replacement, .. } = annotation.annotation
        {
            candidates.push(ModelVisibleItemCandidate {
                item: Item::tool_output(
                    &operation.call,
                    output_kind(canonical)?,
                    &crate::turn::JobOutput::Completed(Ok(replacement)),
                ),
                result_event,
                annotation_event,
            });
        }
    }
    Ok(candidates)
}

pub(super) fn model_output_projection(
    connection: &Connection,
    operation: &OperationId,
    canonical: &Item,
) -> Result<Option<ModelVisibleItemCandidate>> {
    let unprojected = || {
        if canonical.output_format()? == crate::provider::OutputFormat::NativeReceiptV1 {
            Err(StoreError::MissingModelOutputProjection {
                operation: operation.clone(),
            })
        } else {
            Ok(None)
        }
    };
    let candidates = retained_candidates(connection, operation, canonical)?;
    if candidates.is_empty() {
        return unprojected();
    }
    let published_request: Option<String> = connection.query_row(
        "SELECT request_id FROM request_items WHERE output_operation=?1 ORDER BY request_id,position LIMIT 1",
        [serde_json::to_string(operation)?], |row| row.get(0),
    ).optional()?;
    let Some(published_request) = published_request else {
        return unprojected();
    };
    let occurrence = context::request_occurrences(connection, &RequestId(published_request))?
        .into_iter()
        .find(|item| item.output_operation.as_ref() == Some(operation) && item.item == *canonical);
    let Some(occurrence) = occurrence else {
        return Err(StoreError::OperationOriginMismatch);
    };
    for candidate in candidates {
        match context::qualify_model_visible_item(connection, &occurrence, &candidate) {
            Ok(_) => return Ok(Some(candidate)),
            Err(StoreError::Context(crate::context::ContextError::Replay(
                crate::context::ContextReplayRejection::InvalidMembership,
            ))) => continue,
            Err(error) => return Err(error),
        }
    }
    unprojected()
}

impl Store {
    /// The existing exact operation's certified ordinary presentation, if any.
    pub fn model_output_projection(
        &self,
        operation: &OperationId,
    ) -> Result<Option<ModelVisibleItemCandidate>> {
        let Some(canonical) = self.replay_output_operation(operation)? else {
            return Ok(None);
        };
        let connection = self.lock()?;
        model_output_projection(&connection, operation, &canonical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        embedding::BindingSuccessorAuthority,
        model::{AgentPath, CallId},
    };

    struct Transfer;
    impl BindingSuccessorAuthority for Transfer {
        fn validate_successor(
            &self,
            _: &HostIdentity,
            _: &HostIdentity,
        ) -> std::result::Result<bool, String> {
            Ok(true)
        }
    }

    fn settled(store: &Store, attach: bool) -> (HostIdentity, OperationId, Item) {
        let host = HostIdentity {
            run: "result-test".into(),
            actor: AgentPath("/root".into()),
            incarnation: "1".into(),
        };
        store.bind_embedded_actor(&host, None).unwrap();
        let request = RequestId("result-request".into());
        let call = CallId("reused-call".into());
        store
            .write_request(
                &request,
                None,
                "/root",
                &[Item(serde_json::json!({
                    "type":"function_call", "call_id":call.0, "name":"read", "arguments":"{}"
                }))],
                Usage::default(),
            )
            .unwrap();
        let operation = store.claim(&call, &request).unwrap();
        store
            .write_job_output(
                &operation,
                crate::item::ToolKind::Function,
                &crate::turn::JobOutput::Completed(Ok(
                    serde_json::json!({"semantic":"λ".repeat(50_000)}),
                )),
            )
            .unwrap();
        if attach {
            store
                .append_operation_output(&operation, &request, &request)
                .unwrap();
        }
        assert!(
            store
                .advance_agent_head(&host.actor, None, Some(&request))
                .unwrap()
        );
        let original = store.replay_output_operation(&operation).unwrap().unwrap();
        (host, operation, original)
    }

    #[test]
    fn canonical_result_and_projection_survive_reopen_and_exact_binding_transfer() {
        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = Directory(std::env::temp_dir().join(format!(
            "harness-result-inspection-{}",
            uuid::Uuid::new_v4()
        )));
        std::fs::create_dir(&directory.0).unwrap();
        let path = directory.0.join("store.sqlite");
        let store = Store::open(&path).unwrap();
        let (host, operation, original) = settled(&store, true);
        store
            .publish_model_output_projection(&operation, serde_json::json!({"output":"done"}))
            .unwrap();
        assert_eq!(
            store
                .inspect_embedded_result(&host, &operation)
                .map(|record| record.map(|record| record.item))
                .unwrap(),
            Some(original.clone())
        );
        let before = store
            .model_output_projection(&operation)
            .unwrap()
            .unwrap()
            .item;
        let reflected = store.embedded_conversation_history(&host).unwrap();
        assert_eq!(reflected.history.last().unwrap().2, before);
        assert_eq!(
            store
                .inspect_embedded_result(&host, &operation)
                .map(|record| record.map(|record| record.item))
                .unwrap(),
            Some(original.clone())
        );
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .model_output_projection(&operation)
                .unwrap()
                .unwrap()
                .item,
            before
        );
        let successor = HostIdentity {
            incarnation: "2".into(),
            ..host.clone()
        };
        store
            .transfer_embedded_binding(&host, &successor, &Transfer)
            .unwrap();
        assert_eq!(
            store
                .inspect_embedded_result(&successor, &operation)
                .map(|record| record.map(|record| record.item))
                .unwrap(),
            Some(original)
        );
        assert!(matches!(
            store
                .inspect_embedded_result(&host, &operation)
                .map(|record| record.map(|record| record.item)),
            Err(StoreError::InvalidEmbeddedBinding)
        ));
    }

    #[test]
    fn projection_is_retained_before_history_and_retry_is_idempotent() {
        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = Directory(std::env::temp_dir().join(format!(
            "harness-result-inspection-{}",
            uuid::Uuid::new_v4()
        )));
        std::fs::create_dir(&directory.0).unwrap();
        let path = directory.0.join("store.sqlite");
        let store = Store::open(&path).unwrap();
        let (host, operation, original) = settled(&store, false);
        let replacement = serde_json::json!({"output":"authored presentation"});
        let first = store
            .publish_model_output_projection(&operation, replacement.clone())
            .unwrap();
        let retry = store
            .publish_model_output_projection(&operation, replacement.clone())
            .unwrap();
        assert_eq!(first.result_event, retry.result_event);
        assert!(store.model_output_projection(&operation).unwrap().is_none());
        drop(store);
        let store = Store::open(&path).unwrap();
        let successor = HostIdentity {
            incarnation: "2".into(),
            ..host
        };
        let predecessor = HostIdentity {
            incarnation: "1".into(),
            ..successor.clone()
        };
        store
            .transfer_embedded_binding(&predecessor, &successor, &Transfer)
            .unwrap();
        assert_eq!(
            store
                .inspect_embedded_result(&successor, &operation)
                .map(|record| record.map(|record| record.item))
                .unwrap(),
            Some(original)
        );
        let resumed = store
            .publish_model_output_projection(&operation, replacement)
            .unwrap();
        assert_eq!(resumed.result_event, first.result_event);
        store
            .append_operation_output(&operation, &operation.request, &operation.request)
            .unwrap();
        assert_eq!(
            store
                .model_output_projection(&operation)
                .unwrap()
                .unwrap()
                .item,
            first.item
        );
    }

    #[test]
    fn result_inspection_refuses_foreign_origins_and_reused_call_ids() {
        let store = Store::memory().unwrap();
        let (host, operation, original) = settled(&store, true);
        for origin in [
            crate::model::ConversationIdentity::Embedded {
                run: "foreign".into(),
                actor: host.actor.clone(),
                incarnation: "1".into(),
            },
            crate::model::ConversationIdentity::Embedded {
                run: host.run.clone(),
                actor: AgentPath("/other".into()),
                incarnation: "1".into(),
            },
        ] {
            let foreign = OperationId {
                origin,
                ..operation.clone()
            };
            assert!(matches!(
                store
                    .inspect_embedded_result(&host, &foreign)
                    .map(|record| record.map(|record| record.item)),
                Err(StoreError::OperationOriginMismatch)
            ));
        }
        // A genuine second issuing request may reuse the call ID, but its
        // operation is not a member of this actor's current retained lineage.
        let other_request = RequestId("unretained-request".into());
        store
            .write_request(
                &other_request,
                None,
                &host.actor.0,
                &[Item(
                    serde_json::json!({"type":"function_call", "call_id":operation.call.0,
                "name":"read", "arguments":"{}"}),
                )],
                Usage::default(),
            )
            .unwrap();
        let other = store.claim(&operation.call, &other_request).unwrap();
        store
            .write_job_output(
                &other,
                crate::item::ToolKind::Function,
                &crate::turn::JobOutput::Completed(Ok(serde_json::json!({"other":true}))),
            )
            .unwrap();
        assert!(matches!(
            store
                .inspect_embedded_result(&host, &other)
                .map(|record| record.map(|record| record.item)),
            Err(StoreError::OperationOriginMismatch)
        ));
        let absent = OperationId {
            request: RequestId("absent".into()),
            ..operation.clone()
        };
        assert!(
            store
                .inspect_embedded_result(&host, &absent)
                .map(|record| record.map(|record| record.item))
                .is_err()
        );
        assert_eq!(
            store
                .inspect_embedded_result(&host, &operation)
                .map(|record| record.map(|record| record.item))
                .unwrap(),
            Some(original)
        );
    }

    #[test]
    fn marked_native_output_requires_certified_projection_without_losing_inspection() {
        for kind in [
            crate::item::ToolKind::Function,
            crate::item::ToolKind::Custom,
        ] {
            let store = Store::memory().unwrap();
            let host = HostIdentity {
                run: "guard".into(),
                actor: AgentPath("/root".into()),
                incarnation: "1".into(),
            };
            store.bind_embedded_actor(&host, None).unwrap();
            let request = RequestId("guard-request".into());
            let call = CallId("guard-call".into());
            let invocation = match kind {
                crate::item::ToolKind::Function => {
                    serde_json::json!({"type":"function_call","call_id":call.0,"name":"read","arguments":"{}"})
                }
                crate::item::ToolKind::Custom => {
                    serde_json::json!({"type":"custom_tool_call","call_id":call.0,"name":"read","input":"read once"})
                }
            };
            store
                .write_request(
                    &request,
                    None,
                    &host.actor.0,
                    &[Item(invocation)],
                    Usage::default(),
                )
                .unwrap();
            let operation = store.claim(&call, &request).unwrap();
            store
                .write_job_output_with_format(
                    &operation,
                    kind,
                    &crate::turn::JobOutput::Completed(Ok(
                        serde_json::json!({"canonical":"λ".repeat(50000)}),
                    )),
                    crate::provider::OutputFormat::NativeReceiptV1,
                )
                .unwrap();
            store
                .append_operation_output(&operation, &request, &request)
                .unwrap();
            store
                .advance_agent_head(&host.actor, None, Some(&request))
                .unwrap();
            assert!(matches!(
                store.model_output_projection(&operation),
                Err(StoreError::MissingModelOutputProjection { .. })
            ));
            assert!(
                store
                    .inspect_embedded_result(&host, &operation)
                    .map(|record| record.map(|record| record.item))
                    .unwrap()
                    .is_some()
            );
            let candidate = store
                .publish_model_output_projection(&operation, serde_json::json!("done"))
                .unwrap();
            assert_eq!(
                store
                    .model_output_projection(&operation)
                    .unwrap()
                    .unwrap()
                    .item,
                candidate.item
            );
        }
    }

    #[test]
    fn exact_inspection_keeps_real_settlement_when_original_claim_is_interrupted() {
        let store = Store::memory().unwrap();
        let host = HostIdentity {
            run: "inherited".into(),
            actor: AgentPath("/root".into()),
            incarnation: "1".into(),
        };
        store.bind_embedded_actor(&host, None).unwrap();
        let request = RequestId("original-result".into());
        let call = CallId("inherited-call".into());
        store.write_request(&request,None,&host.actor.0,&[Item(serde_json::json!({"type":"function_call","call_id":call.0,"name":"read","arguments":"{}"}))],Usage::default()).unwrap();
        store
            .set_effort(&request, crate::model::Effort::Medium)
            .unwrap();
        let operation = store.claim(&call, &request).unwrap();
        store
            .advance_agent_head(&host.actor, None, Some(&request))
            .unwrap();
        let cuts = store
            .capture_checkpoint_cuts(&operation, &serde_json::json!({}), std::sync::Arc::new(()))
            .unwrap();
        let (child, _) = store
            .attach_checkpoint_child(
                cuts.deferred(),
                crate::checkpoint::CheckpointChild {
                    path: &AgentPath("/root/reader".into()),
                    parent: &host.actor,
                    contract: &serde_json::json!({}),
                    checkout: &serde_json::json!({}),
                    task: None,
                    target: &crate::context::ContextProjectionTarget {
                        model: "test".into(),
                        route: crate::transport::ResponsesRoute::ChatGptPlan,
                        protocol: crate::transport::ResponsesProtocol::Standard,
                    },
                },
            )
            .unwrap();
        let child_request = child.head_request.unwrap();
        assert_eq!(
            store.claims_on(&child_request).unwrap()[0].operation,
            operation
        );
        store
            .interrupt_operation_claim(&operation, &request)
            .unwrap();
        let output =
            crate::turn::JobOutput::Completed(Ok(serde_json::json!({"semantic":"committed once"})));
        store
            .write_job_output_with_format(
                &operation,
                crate::item::ToolKind::Function,
                &output,
                crate::provider::OutputFormat::NativeReceiptV1,
            )
            .unwrap();
        assert_eq!(
            store
                .replay_tool_output_operation(&operation)
                .unwrap()
                .unwrap()
                .terminal,
            TerminalOutcome::Interrupted
        );
        let inspected = store
            .inspect_embedded_result(&host, &operation)
            .unwrap()
            .unwrap();
        assert_eq!(inspected.terminal, TerminalOutcome::Success);
        assert_eq!(
            inspected.output_format,
            crate::provider::OutputFormat::NativeReceiptV1
        );
        assert_eq!(
            inspected.item,
            Item::tool_output_with_format(
                &call,
                crate::item::ToolKind::Function,
                &output,
                crate::provider::OutputFormat::NativeReceiptV1
            )
        );
    }

    #[test]
    fn result_inspection_refuses_corrupt_canonical_output() {
        let store = Store::memory().unwrap();
        let (host, operation, original) = settled(&store, false);
        let mut corrupt = original.0;
        corrupt["call_id"] = serde_json::json!("different-call");
        // Corrupt the retained Item only; no replacement authority is issued.
        store.lock().unwrap().execute(
            "UPDATE items SET json=?1 WHERE hash=(SELECT output_hash FROM claims WHERE origin_request_id=?2 AND call_id=?3 AND state='settled')",
            params![serde_json::to_string(&corrupt).unwrap(), operation.request.0, operation.call.0],
        ).unwrap();
        assert!(
            store
                .inspect_embedded_result(&host, &operation)
                .map(|record| record.map(|record| record.item))
                .is_err()
        );
    }
}
