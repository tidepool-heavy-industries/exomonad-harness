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
