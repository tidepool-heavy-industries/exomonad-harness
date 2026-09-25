//! Durable completion/publication contract shared by Store, Engine and hosts.
//!
//! Envelope IDs are the stable references already allocated by Store. A final
//! request sees exactly envelopes delivered into that request or its ancestors.
//! Completion snapshots are taken under Store's serialized connection after
//! the final response is persisted. Later arrivals remain unread and wake the
//! next request; they do not invalidate an already valid completion.

use crate::{item::Item, model::RequestId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompletionProvenance {
    pub final_request: RequestId,
    pub seen_envelopes: Vec<i64>,
    pub unseen_envelopes: Vec<i64>,
}

/// Structured payload of the parent's FINAL_ANSWER envelope. Never infer
/// incorporation from `seen_envelopes`: it means only delivery to model input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PublishedAnswer {
    pub sender: String,
    pub result: Value,
    pub provenance: CompletionProvenance,
}

#[derive(Debug, thiserror::Error)]
pub enum PublishedAnswerError {
    #[error("published answer is not a standard single-text assistant message")]
    InvalidMessage,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl PublishedAnswer {
    /// The persisted envelope item is also a valid Responses input item.
    /// Typed consumers decode the JSON payload; the model sees wire-safe text.
    pub fn to_message_item(&self) -> Result<Item, PublishedAnswerError> {
        let text = serde_json::to_string(self)?;
        Ok(Item(json!({
            "type": "message",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text}]
        })))
    }

    pub fn from_message_item(item: &Item) -> Result<Self, PublishedAnswerError> {
        let fields = &item.0;
        let parts = fields["content"]
            .as_array()
            .ok_or(PublishedAnswerError::InvalidMessage)?;
        if fields["type"] != "message"
            || fields["role"] != "assistant"
            || parts.len() != 1
            || parts[0]["type"] != "output_text"
        {
            return Err(PublishedAnswerError::InvalidMessage);
        }
        let text = parts[0]["text"]
            .as_str()
            .ok_or(PublishedAnswerError::InvalidMessage)?;
        Ok(serde_json::from_str(text)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_answer_item_is_wire_safe_and_round_trips() {
        let answer = PublishedAnswer {
            sender: "/root/child".into(),
            result: json!({"answer": "done", "count": 2}),
            provenance: CompletionProvenance {
                final_request: RequestId("r1".into()),
                seen_envelopes: vec![2],
                unseen_envelopes: vec![4],
            },
        };
        let item = answer.to_message_item().unwrap();
        let object = item.0.as_object().unwrap();
        assert_eq!(object.len(), 3);
        assert_eq!(PublishedAnswer::from_message_item(&item).unwrap(), answer);
    }
}
