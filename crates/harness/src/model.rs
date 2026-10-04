use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentPath(pub String);

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct CallId(pub String);

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct RequestId(pub String);

/// The conversation authorized to originate a provider operation.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationIdentity {
    Standalone {
        store: String,
        actor: AgentPath,
    },
    Embedded {
        run: String,
        actor: AgentPath,
        incarnation: String,
    },
}

impl ConversationIdentity {
    pub fn actor(&self) -> &AgentPath {
        match self {
            Self::Standalone { actor, .. } | Self::Embedded { actor, .. } => actor,
        }
    }
}

/// Internal identity; `call` is still emitted verbatim in provider wire items.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct OperationId {
    pub origin: ConversationIdentity,
    pub request: RequestId,
    pub call: CallId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
}
