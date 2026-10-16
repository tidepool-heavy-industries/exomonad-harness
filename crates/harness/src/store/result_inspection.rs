//! Canonical tool results and their separately certified model presentation.

use super::*;
use crate::{embedding::HostIdentity, provider::ModelVisibleItemCandidate};

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
        if original.0["type"] != "function_call_output" {
            return Err(StoreError::OperationOriginMismatch);
        }
        let candidate = Item::tool_output(
            &operation.call,
            crate::item::ToolKind::Function,
            &crate::turn::JobOutput::Completed(Ok(replacement.clone())),
        );
        if let Some(existing) = self.model_output_projection(operation)? {
            if existing.item == candidate {
                return Ok(existing);
            }
            return Err(StoreError::OperationOriginMismatch);
        }
        let result = serde_json::to_string(&serde_json::json!({
            "operation": operation, "output": original,
        }))?;
        let mut connection = self.lock()?;
        connection.write_transaction(|tx| {
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
    ) -> Result<Option<Item>> {
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
            let history = context::history(tx, head, true)?;
            context::validate_canonical_history(tx, &history)?;
            let call = context::original_call(tx, operation)?;
            let owned = history.iter().any(|item| {
                item.origin == call || item.output_operation.as_ref() == Some(operation)
            });
            if !owned {
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
                Ok(item)
            }).transpose()
        })
    }
}

/// Recover a durable projection and authenticate it through the same owner used
/// by provider requests. GUI and model readers share this exact projection.
pub(super) fn model_output_projection(
    connection: &Connection,
    operation: &OperationId,
    canonical: &Item,
) -> Result<Option<ModelVisibleItemCandidate>> {
    let mut query = connection.prepare(
        "SELECT r.id,a.id,a.payload FROM events a JOIN events r ON r.id=json_extract(a.payload,'$.ordinal') WHERE a.kind='model_tool_annotation' AND r.kind='model_tool_result' AND r.request_id=?1 AND a.request_id=r.request_id AND json_extract(r.payload,'$.operation.call')=?2 ORDER BY a.id DESC"
    )?;
    let rows = query.query_map(params![operation.request.0, operation.call.0], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let published_request: Option<String> = connection.query_row(
        "SELECT request_id FROM request_items WHERE output_operation=?1 ORDER BY request_id,position LIMIT 1",
        [serde_json::to_string(operation)?], |row| row.get(0),
    ).optional()?;
    let Some(published_request) = published_request else {
        return Ok(None);
    };
    let occurrence = context::request_occurrences(connection, &RequestId(published_request))?
        .into_iter()
        .find(|item| item.output_operation.as_ref() == Some(operation) && item.item == *canonical);
    let Some(occurrence) = occurrence else {
        return Ok(None);
    };
    for row in rows {
        let (result_event, annotation_event, raw) = row?;
        #[derive(serde::Deserialize)]
        struct Annotation {
            annotation: crate::invocation::HookAnnotation,
        }
        let annotation: Annotation = serde_json::from_str(&raw)?;
        if let crate::invocation::HookAnnotation::Pruned { replacement, .. } = annotation.annotation
        {
            let candidate = ModelVisibleItemCandidate {
                item: Item::tool_output(
                    &operation.call,
                    crate::item::ToolKind::Function,
                    &crate::turn::JobOutput::Completed(Ok(replacement)),
                ),
                result_event,
                annotation_event,
            };
            match context::qualify_model_visible_item(connection, &occurrence, &candidate) {
                Ok(_) => return Ok(Some(candidate)),
                Err(StoreError::Context(crate::context::ContextError::Replay(
                    crate::context::ContextReplayRejection::InvalidMembership,
                ))) => continue,
                Err(error) => return Err(error),
            }
        }
    }
    Ok(None)
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
        model_output_projection(&self.lock()?, operation, &canonical)
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

    fn settled(store: &Store) -> (HostIdentity, OperationId, Item) {
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
        store
            .append_operation_output(&operation, &request, &request)
            .unwrap();
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.sqlite");
        let store = Store::open(&path).unwrap();
        let (host, operation, original) = settled(&store);
        store
            .publish_model_output_projection(&operation, serde_json::json!({"output":"done"}))
            .unwrap();
        assert_eq!(
            store.inspect_embedded_result(&host, &operation).unwrap(),
            Some(original.clone())
        );
        let before = store
            .model_output_projection(&operation)
            .unwrap()
            .unwrap()
            .item;
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
                .unwrap(),
            Some(original)
        );
        assert!(matches!(
            store.inspect_embedded_result(&host, &operation),
            Err(StoreError::InvalidEmbeddedBinding)
        ));
    }

    #[test]
    fn result_inspection_refuses_foreign_origins_and_reused_call_ids() {
        let store = Store::memory().unwrap();
        let (host, operation, original) = settled(&store);
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
                store.inspect_embedded_result(&host, &foreign),
                Err(StoreError::OperationOriginMismatch)
            ));
        }
        let absent = OperationId {
            request: RequestId("absent".into()),
            ..operation.clone()
        };
        assert!(store.inspect_embedded_result(&host, &absent).is_err());
        assert_eq!(
            store.inspect_embedded_result(&host, &operation).unwrap(),
            Some(original)
        );
    }
}
