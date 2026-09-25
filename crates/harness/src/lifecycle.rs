//! Durable completion/publication contract shared by Store, Engine and hosts.
//!
//! Envelope IDs are the stable references already allocated by Store. A final
//! request sees exactly envelopes delivered into that request or its ancestors.
//! Completion snapshots are taken under Store's serialized connection after
//! the final response is persisted. Later arrivals remain unread and wake the
//! next request; they do not invalidate an already valid completion.

use crate::model::RequestId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
