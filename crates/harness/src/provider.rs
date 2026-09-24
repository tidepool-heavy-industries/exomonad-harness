use crate::{
    agents::{is_agent_verb, verb_tool_schemas},
    model::{AgentPath, CallId},
};
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;

/// Stable public identifier for asynchronous provider work.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct JobHandle(pub String);

/// Per-call metadata supplied by the harness. Progress is deliberately
/// out-of-band: it is never appended to the model-visible item list.
#[derive(Clone, Debug)]
pub struct CallContext {
    pub handle: JobHandle,
    pub call_id: CallId,
    pub agent: AgentPath,
    pub progress: mpsc::UnboundedSender<Value>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("tool failed: {0}")]
    Tool(String),
}

/// A provider owns tool meaning; the harness owns scheduling and history.
#[async_trait]
pub trait Provider: Send + Sync {
    async fn call(&self, name: &str, args: Value) -> Result<Value, ProviderError>;
    /// Context-aware entry point. Existing providers remain source-compatible;
    /// providers which emit progress may override this method.
    async fn call_with_context(
        &self,
        name: &str,
        args: Value,
        _context: CallContext,
    ) -> Result<Value, ProviderError> {
        self.call(name, args).await
    }

    /// Runtime hook for crate-provided agent verbs. A provider integrating
    /// durable sessions should route this to `dispatch_agent_verb`.
    async fn call_agent_verb(
        &self,
        name: &str,
        _args: Value,
        _context: CallContext,
    ) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool(format!(
            "agent verb `{name}` has no AgentToolService runtime"
        )))
    }

    /// Complete stable model tool list (harness verbs plus provider-owned
    /// tools). The provider's existing `tools` method remains unchanged.
    fn all_tools(&self) -> Vec<Value> {
        let mut tools = verb_tool_schemas();
        tools.extend(self.tools());
        tools
    }

    fn tools(&self) -> Vec<Value>;
}

pub fn is_harness_tool(name: &str) -> bool {
    is_agent_verb(name)
}
