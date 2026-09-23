use crate::model::AgentPath;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum EnvelopeType {
    NewTask,
    Message,
    FinalAnswer,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum DeliveryClass {
    Steer,
    AtBoundary,
    Hold,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub kind: EnvelopeType,
    pub recipient: AgentPath,
    pub sender: AgentPath,
    pub payload: String,
    pub class: DeliveryClass,
    pub timestamp_ms: i64,
}
