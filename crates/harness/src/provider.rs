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

// TODO(correction-wave, allowed if a-c need it): PRD `provider trait` shape.
// `CallContext` should carry `cancel: CancellationToken` (tokio-util) and
// `verbs: JobVerbs`; `progress` must be a BOUNDED channel with an explicit
// overflow policy (PRD `fast !`). Tool identity by `&str` and JSON in/out is
// the wire boundary leaking into the trait; typed `Tools` with
// `output_schema` is the target. Hooks (eleven typed points, one closed
// Decision enum each) are wave1, not this wave.
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

    // FIXME(correction-wave): agent verbs are crate-owned (PRD `agent verbs`);
    // routing them through the provider inverts the dependency and forces every
    // consumer to wire `StoreAgentToolService` by hand. The engine should
    // dispatch verbs itself; the provider supplies tools and hooks only.
    // Nudge `verbs_via_provider`.
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
    // TODO(correction-wave b): this is the ONE place to make
    // `tests/correction_wave.rs` green: stamp `"async": true` on every tool
    // here, crate verbs and provider tools alike, except `wait_agent`. a
    // provider-supplied `async` is overridden, not trusted (PRD `provider
    // trait`; fix the class, not each schema).
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
