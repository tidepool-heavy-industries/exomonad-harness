//! Replay continuation authority is bounded by the next immutable issued window.
use super::{RecordedReplayTurn, Result, Store, StoreError};
use crate::{
    item::{Item, ItemHash},
    model::OperationId,
};

#[derive(Clone, Debug)]
pub(crate) struct RecordedWaitContinuation {
    local: OperationId,
    original: OperationId,
    output_hash: ItemHash,
    issued_cut: Vec<ItemHash>,
    items: Vec<Item>,
}

fn hash(item: &Item) -> Result<ItemHash> {
    Ok(ItemHash(
        blake3::hash(&serde_json::to_vec(item)?)
            .to_hex()
            .to_string(),
    ))
}

impl RecordedWaitContinuation {
    pub(crate) fn into_items(self, local: &OperationId, output: &Item) -> Result<Vec<Item>> {
        if local != &self.local
            || local.call != self.original.call
            || hash(output)? != self.output_hash
        {
            return Err(StoreError::InvalidWaitContinuation {
                operation: local.clone(),
            });
        }
        let mut cut = vec![self.output_hash];
        cut.extend(self.items.iter().map(hash).collect::<Result<Vec<_>>>()?);
        if cut != self.issued_cut {
            return Err(StoreError::InvalidWaitContinuation {
                operation: local.clone(),
            });
        }
        Ok(self.items)
    }
}

impl Store {
    /// Only the replay owner supplies a next turn read through typed Store replay.
    /// Later history messages cannot extend the immutable issued input cut.
    pub(crate) fn replay_wait_continuation(
        &self,
        local: &OperationId,
        original: &OperationId,
        next: Option<&RecordedReplayTurn>,
    ) -> Result<Option<RecordedWaitContinuation>> {
        let invocation = self.invocation_item(&original.request, &original.call)?;
        if invocation.as_ref().map(|call| call.name.as_str()) != Some("wait_agent")
            || local.call != original.call
        {
            return Err(StoreError::InvalidWaitContinuation {
                operation: local.clone(),
            });
        }
        let Some(recorded) = self.replay_tool_output_operation(original)? else {
            return Err(StoreError::InvalidWaitContinuation {
                operation: local.clone(),
            });
        };
        let history = self.items(&original.request)?;
        let outputs = history
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.0["call_id"].as_str() == Some(&original.call.0)
                    && matches!(
                        item.0["type"].as_str(),
                        Some("function_call_output" | "custom_tool_call_output")
                    )
            })
            .collect::<Vec<_>>();
        let [(position, output)] = outputs.as_slice() else {
            // An interrupted claim can be retained before its synthetic Item is
            // attached; that tombstone has no message continuation authority.
            if outputs.is_empty() && recorded.terminal == super::TerminalOutcome::Interrupted {
                return Ok(None);
            }
            return Err(StoreError::InvalidWaitContinuation {
                operation: local.clone(),
            });
        };
        if **output != recorded.item {
            return Err(StoreError::InvalidWaitContinuation {
                operation: local.clone(),
            });
        }
        let following = history[position + 1..]
            .iter()
            .take_while(|item| item.0["type"] == "message")
            .collect::<Vec<_>>();
        if following.is_empty() {
            return Ok(None);
        }
        let Some(next) = next else {
            return Err(StoreError::InvalidWaitContinuation {
                operation: local.clone(),
            });
        };
        // A caller cannot turn a reconstructed or edited view into authority.
        // Match the cut against the immutable reference event owned by Store.
        let records = {
            let c = self.lock();
            let descendant: bool = c.query_row(
                "WITH RECURSIVE ancestry(id) AS (SELECT ?1 UNION ALL SELECT r.parent_id FROM requests r JOIN ancestry a ON r.id=a.id WHERE r.parent_id IS NOT NULL) SELECT EXISTS(SELECT 1 FROM ancestry WHERE id=?2) AND (SELECT branch FROM requests WHERE id=?1)=(SELECT branch FROM requests WHERE id=?2)",
                rusqlite::params![next.request.0,original.request.0], |row| row.get(0),
            )?;
            if !descendant {
                return Err(StoreError::InvalidWaitContinuation {
                    operation: local.clone(),
                });
            }
            let mut query = c.prepare("SELECT id,payload FROM events WHERE request_id=?1 AND kind='model_turn' ORDER BY id")?;
            query
                .query_map([&next.request.0], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut authorized = false;
        for (event, payload) in records {
            let issued = self.decode_replay_record(event, &payload)?;
            if issued.model_request.input == next.model_request.input {
                authorized = true;
                break;
            }
        }
        if !authorized {
            return Err(StoreError::InvalidWaitContinuation {
                operation: local.clone(),
            });
        }
        let positions = next
            .model_request
            .input
            .iter()
            .enumerate()
            .filter_map(|(index, item)| (item == &recorded.item).then_some(index))
            .collect::<Vec<_>>();
        let [cut_start] = positions.as_slice() else {
            return Err(StoreError::InvalidWaitContinuation {
                operation: local.clone(),
            });
        };
        let issued_messages = next.model_request.input[cut_start + 1..]
            .iter()
            .take_while(|item| item.0["type"] == "message");
        let items = following
            .into_iter()
            .zip(issued_messages)
            .take_while(|(history, issued)| *history == *issued)
            .map(|(item, _)| item.clone())
            .collect::<Vec<_>>();
        if items.is_empty() {
            return Err(StoreError::InvalidWaitContinuation {
                operation: local.clone(),
            });
        }
        let output_hash = hash(&recorded.item)?;
        let mut issued_cut = vec![output_hash.clone()];
        issued_cut.extend(items.iter().map(hash).collect::<Result<Vec<_>>>()?);
        Ok(Some(RecordedWaitContinuation {
            local: local.clone(),
            original: original.clone(),
            output_hash,
            issued_cut,
            items,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        item::ToolKind,
        model::{CallId, Effort, RequestId},
        transport::{ResponsesRequest, ResponsesTurn, Usage},
        turn::JobOutput,
    };
    use serde_json::json;

    fn fixture() -> (
        Store,
        OperationId,
        OperationId,
        RecordedReplayTurn,
        Item,
        Item,
    ) {
        let store = Store::memory().unwrap();
        let source = RequestId("wait-source".into());
        let next = RequestId("wait-next".into());
        let local_request = RequestId("replay-local".into());
        store.create_request(&source, None, "/root").unwrap();
        store.create_request(&next, Some(&source), "/root").unwrap();
        store
            .create_request(&local_request, None, "/replay")
            .unwrap();
        let call = CallId("wait-id".into());
        let invocation = Item(
            json!({"type":"function_call","name":"wait_agent","call_id":call.0,"arguments":"{}"}),
        );
        store.append_items(&source, &[invocation.clone()]).unwrap();
        let original = store.claim(&call, &source).unwrap();
        let local = store.operation_for_request(&local_request, &call).unwrap();
        let terminal = JobOutput::Completed(Ok(json!({"resumed_by":"user_input"})));
        let output = Item::tool_output(&call, ToolKind::Function, &terminal);
        store
            .write_job_output(&original, ToolKind::Function, &terminal)
            .unwrap();
        let message = Item(json!({"type":"message","role":"user","content":"recorded wake"}));
        store
            .append_items(&source, &[output.clone(), message.clone()])
            .unwrap();
        let request = ResponsesRequest {
            input: vec![invocation, output.clone(), message.clone()],
            instructions: String::new(),
            tools: vec![].into(),
            tools_allowed: None,
            model: "test".into(),
            pinned_effort: Effort::Low,
            session_id: "test".into(),
        };
        store
            .record_replay_turn(
                &next,
                &request,
                &ResponsesTurn {
                    response_id: "next".into(),
                    items: vec![],
                    usage: Usage::default(),
                },
            )
            .unwrap();
        let next = store.replay_turns(&next).unwrap().remove(0);
        (store, local, original, next, output, message)
    }

    #[test]
    fn immutable_issued_cut_excludes_later_history_messages() {
        let (store, local, original, next, output, message) = fixture();
        let late = Item(json!({"type":"message","role":"user","content":"late mutation"}));
        store.append_items(&original.request, &[late]).unwrap();
        let witness = store
            .replay_wait_continuation(&local, &original, Some(&next))
            .unwrap()
            .unwrap();
        assert_eq!(witness.into_items(&local, &output).unwrap(), vec![message]);
    }

    #[test]
    fn wait_continuation_stops_at_first_non_message() {
        let (store, local, original, next, output, message) = fixture();
        let late = Item(json!({"type":"message","role":"user","content":"after parallel output"}));
        store
            .append_items(
                &original.request,
                &[
                    Item(json!({"type":"function_call_output","call_id":"other","output":"{}"})),
                    late.clone(),
                ],
            )
            .unwrap();
        let witness = store
            .replay_wait_continuation(&local, &original, Some(&next))
            .unwrap()
            .unwrap();
        assert_eq!(witness.into_items(&local, &output).unwrap(), vec![message]);
    }

    #[test]
    fn edited_next_request_cannot_issue_continuation_authority() {
        let (store, local, original, mut next, _, _) = fixture();
        next.model_request.input.push(Item(
            json!({"type":"message","role":"user","content":"forged cut"}),
        ));
        assert!(matches!(
            store.replay_wait_continuation(&local, &original, Some(&next)),
            Err(StoreError::InvalidWaitContinuation { .. })
        ));
    }

    #[test]
    fn wait_continuation_refuses_duplicate_output_or_missing_cut() {
        let (store, local, original, next, output, _) = fixture();
        assert!(
            store
                .replay_wait_continuation(&local, &original, None)
                .is_err()
        );
        store.append_items(&original.request, &[output]).unwrap();
        assert!(
            store
                .replay_wait_continuation(&local, &original, Some(&next))
                .is_err()
        );
    }

    #[test]
    fn wait_continuation_is_bound_to_local_operation_and_exact_output() {
        let (store, local, original, next, output, _) = fixture();
        let witness = store
            .replay_wait_continuation(&local, &original, Some(&next))
            .unwrap()
            .unwrap();
        let mut foreign = local.clone();
        foreign.request = RequestId("foreign".into());
        assert!(witness.clone().into_items(&foreign, &output).is_err());
        assert!(
            witness
                .into_items(&local, &Item(json!({"changed":true})))
                .is_err()
        );
    }
}
