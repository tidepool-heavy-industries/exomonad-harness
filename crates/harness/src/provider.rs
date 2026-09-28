use crate::{
    agents::{is_agent_verb, verb_tool_schemas},
    hooks::{BeforeRequestResult, RequestPlan},
    model::{AgentPath, CallId},
};
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;

/// Stable public identifier for asynchronous provider work.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct JobHandle(pub String);

// TODO(adoption C0/H1; docs/daily-driver-plan.md): reconcile CallContext with
// accepted bounded-progress/cancellation work before wiring a resident evaluator.
// Extend the existing job owner; do not add a parallel provider-side job registry.
/// Per-call metadata supplied by the harness. Progress is deliberately
/// out-of-band: it is never appended to the model-visible item list.
#[derive(Clone, Debug)]
pub struct CallContext {
    pub handle: JobHandle,
    pub call_id: CallId,
    pub agent: AgentPath,
    /// Durable request that emitted this call, when dispatched by Engine.
    pub request: Option<crate::model::RequestId>,
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
    /// Invoked once before an Engine transport attempt. Default is pass-through.
    async fn before_request(&self, _plan: &RequestPlan) -> BeforeRequestResult {
        BeforeRequestResult::default()
    }
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

    // TODO(adoption C0/H2; docs/daily-driver-plan.md): decide the sole agent
    // transition authority with the embedding host; route verbs to that owner
    // rather than duplicating lifecycle state in each Provider implementation.
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
        for tool in &mut tools {
            if let Some(object) = tool.as_object_mut() {
                let is_wait_agent =
                    object.get("name").and_then(Value::as_str) == Some("wait_agent");
                if is_wait_agent {
                    object.remove("async");
                } else {
                    object.insert("async".to_owned(), Value::Bool(true));
                }
            }
        }
        tools
    }

    fn tools(&self) -> Vec<Value>;
}

pub fn is_harness_tool(name: &str) -> bool {
    is_agent_verb(name)
}
