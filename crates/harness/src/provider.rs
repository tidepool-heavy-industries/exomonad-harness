use crate::{
    agents::{is_agent_verb, verb_tool_schemas},
    hooks::{BeforeRequestResult, RequestPlanView},
    model::{AgentPath, CallId, OperationId},
};
use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Maximum number of progress events retained while a provider call runs.
/// When full, the scheduler drops the newest event rather than blocking work.
pub const JOB_PROGRESS_CAPACITY: usize = 64;

/// Stable public identifier for asynchronous provider work.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct JobHandle(pub String);

/// Per-call metadata supplied by the harness. Progress is deliberately
/// out-of-band: it is never appended to the model-visible item list.
#[derive(Clone, Debug)]
pub struct CallContext {
    pub handle: JobHandle,
    /// Exact scheduler operation. Detached direct-provider probes have none.
    pub operation: Option<OperationId>,
    pub call_id: CallId,
    pub agent: AgentPath,
    /// Durable request that emitted this call, when dispatched by Engine.
    pub request: Option<crate::model::RequestId>,
    /// Cooperative cancellation signal. External work remains owned by its
    /// CancellationOwner until that owner confirms cleanup.
    pub cancel: CancellationToken,
    /// Scheduler-created job authority. Providers cannot supply its identity.
    pub verbs: crate::agents::JobVerbs,
    /// Bounded, best-effort out-of-band progress; a full queue rejects sends.
    pub progress: mpsc::Sender<Value>,
}

impl CallContext {
    /// Construct a detached context for provider tests and probes which call
    /// a provider directly, outside `JobScheduler`.
    ///
    /// This deliberately grants no agent-operation backend or request
    /// identity: `verbs.origin()` is `None`, and identity-dependent JobVerbs
    /// return typed refusals. Progress uses the same bounded channel policy
    /// as scheduled calls. Production jobs must receive their context from
    /// `JobScheduler`, which alone can mint job identity.
    pub fn detached_for_test(
        handle: JobHandle,
        call_id: CallId,
        agent: AgentPath,
    ) -> (Self, mpsc::Receiver<Value>) {
        let cancel = CancellationToken::new();
        let (progress, receiver) = mpsc::channel(JOB_PROGRESS_CAPACITY);
        let verbs = crate::agents::JobVerbs::from_scheduler(
            None,
            agent.clone(),
            None,
            cancel.clone(),
            progress.clone(),
        );
        (
            Self {
                handle,
                operation: None,
                call_id,
                agent,
                request: None,
                cancel,
                verbs,
                progress,
            },
            receiver,
        )
    }
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("tool failed: {0}")]
    Tool(String),
}

/// Evidence from the owner of work that can outlive the provider future.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum CancellationAcknowledgment {
    Stopped,
    /// The owner observed the operation's terminal result before cancellation won.
    Completed(Result<Value, String>),
    Unconfirmed(String),
}

#[async_trait]
pub trait CancellationOwner: Send + Sync {
    /// Stop or reconcile the exact admitted operation. A signal alone is not Stopped.
    async fn cancel(
        &self,
        operation: &OperationId,
        handle: &JobHandle,
    ) -> CancellationAcknowledgment;
}

/// A provider owns tool meaning; the harness owns scheduling and history.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Continuation bridges do not hold a scheduler slot while the host runs.
    fn holds_job_capacity(&self) -> bool {
        true
    }

    /// Optional projection of one durable item for model input. The durable
    /// request identity prevents a replacement from affecting another call.
    fn model_visible_item(
        &self,
        _request: &crate::model::RequestId,
        _item: &crate::item::Item,
    ) -> Option<crate::item::Item> {
        None
    }

    /// Acknowledge an exact operation after its real terminal output is durable.
    /// Recovery can repeat this notification; implementations must be idempotent.
    /// Failure leaves the retained output intact and never re-executes the call.
    async fn output_committed(&self, _operation: &OperationId) -> Result<(), ProviderError> {
        Ok(())
    }

    /// A stable view retained for an entire model request, including its later
    /// tool calls. Reloadable providers return an immutable snapshot here.
    fn request_snapshot(&self) -> Result<Option<std::sync::Arc<dyn Provider>>, ProviderError> {
        Ok(None)
    }

    /// None means all work is owned by the call future and stops when it drops.
    /// Providers dispatching external work must supply its cancellation owner.
    fn cancellation_owner(&self) -> Option<std::sync::Arc<dyn CancellationOwner>> {
        None
    }

    /// Immutable host surface version retained with the issuing request.
    fn tool_surface_version(&self) -> Option<&str> {
        None
    }

    /// Validate a call before the Engine interprets any reserved operation.
    fn validate_call(
        &self,
        _name: &str,
        _kind: crate::item::ToolKind,
    ) -> Result<(), ProviderError> {
        Ok(())
    }

    /// Optional runtime backend used by the scheduler to construct the
    /// call-scoped `JobVerbs`. Defaults to no job-side agent operations.
    fn job_agent_service(&self) -> Option<std::sync::Arc<dyn crate::agents::AgentToolService>> {
        None
    }

    /// Invoked once before an Engine transport attempt. Default is pass-through.
    async fn before_request(&self, _plan: &RequestPlanView<'_>) -> BeforeRequestResult {
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

    /// Invoke a freeform custom tool. The input is not JSON: callers must
    /// preserve its exact text rather than parsing or re-serializing it.
    /// Custom calls deliberately do not enter the function-only agent-verb
    /// path in `JobScheduler`.
    async fn call_custom_with_context(
        &self,
        name: &str,
        _input: String,
        _context: CallContext,
    ) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool(format!(
            "custom tool `{name}` is not supported by this provider"
        )))
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

    /// Immutable advertised input for one request; pinned providers share it.
    fn tool_manifest(&self) -> crate::transport::ToolManifest {
        self.all_tools().into()
    }

    fn tools(&self) -> Vec<Value>;
}

pub fn is_harness_tool(name: &str) -> bool {
    is_agent_verb(name)
}
