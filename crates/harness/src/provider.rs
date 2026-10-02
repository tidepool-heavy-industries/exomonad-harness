use crate::{
    agents::{is_agent_verb, verb_tool_schemas},
    hooks::{BeforeRequestResult, RequestPlanView},
    model::{AgentPath, CallId, OperationId},
};
use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

mod failure;
pub use failure::{MetadataOmission, ToolFailure};

/// Maximum number of progress events retained while a provider call runs.
/// When full, the scheduler drops the newest event rather than blocking work.
pub const JOB_PROGRESS_CAPACITY: usize = 64;

/// Host policy pinned to the issuing tool installation and operation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolScheduling {
    #[default]
    Async,
    BeforeNextInference,
}

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
    /// Store-issued prefix lease for this exact synchronous invocation. Async
    /// calls and detached probes never receive editing authority.
    pub context: Option<crate::context::ContextSnapshot>,
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
                context: None,
            },
            receiver,
        )
    }
}

#[derive(Debug)]
pub enum ProviderError {
    Tool(ToolFailure),
    NonValueTerminal(NonValueTerminal),
}

/// A value-only direct call cannot faithfully return these recorded terminals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NonValueTerminal {
    Cancelled,
    Interrupted,
    CancellationUnconfirmed(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tool(failure) => failure.fmt_provider(output),
            Self::NonValueTerminal(NonValueTerminal::Cancelled) => {
                write!(
                    output,
                    "recorded cancellation requires terminal-aware scheduling"
                )
            }
            Self::NonValueTerminal(NonValueTerminal::Interrupted) => {
                write!(
                    output,
                    "recorded interruption requires terminal-aware scheduling"
                )
            }
            Self::NonValueTerminal(NonValueTerminal::CancellationUnconfirmed(detail)) => {
                write!(
                    output,
                    "recorded unconfirmed cancellation requires terminal-aware scheduling: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for ProviderError {}

impl ProviderError {
    pub fn into_tool_failure(self) -> ToolFailure {
        match self {
            Self::Tool(failure) => failure.with_tool_prefix(),
            error @ Self::NonValueTerminal(_) => error.to_string().into(),
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct WaitReplayBarrier {
    pub(crate) before: Vec<OperationId>,
    pub(crate) after: Vec<OperationId>,
}

impl WaitReplayBarrier {
    pub(crate) fn is_empty(&self) -> bool {
        self.before.is_empty() && self.after.is_empty()
    }
}

/// Exact retained terminal output. Only the replay owner can attach a sealed
/// builtin continuation; ordinary providers can supply terminal output alone.
#[derive(Debug)]
pub struct RetainedOutput {
    output: crate::turn::JobOutput,
    wait_operation: Option<OperationId>,
    continuation: Option<crate::store::RecordedWaitContinuation>,
    wait_barrier: WaitReplayBarrier,
    wait_commit: Option<crate::replay::ReplayWaitCommit>,
}

impl RetainedOutput {
    pub fn terminal(output: crate::turn::JobOutput) -> Self {
        Self {
            output,
            wait_operation: None,
            continuation: None,
            wait_barrier: WaitReplayBarrier::default(),
            wait_commit: None,
        }
    }

    pub fn output(&self) -> &crate::turn::JobOutput {
        &self.output
    }

    pub(crate) fn recorded_wait(
        operation: OperationId,
        output: crate::turn::JobOutput,
        continuation: Option<crate::store::RecordedWaitContinuation>,
        wait_barrier: WaitReplayBarrier,
        wait_commit: crate::replay::ReplayWaitCommit,
    ) -> Self {
        Self {
            output,
            wait_operation: Some(operation),
            continuation,
            wait_barrier,
            wait_commit: Some(wait_commit),
        }
    }

    pub(crate) fn into_parts(
        self,
        operation: &OperationId,
    ) -> Result<
        (
            crate::turn::JobOutput,
            Option<crate::store::RecordedWaitContinuation>,
            WaitReplayBarrier,
            Option<crate::replay::ReplayWaitCommit>,
        ),
        ProviderError,
    > {
        if self
            .wait_operation
            .as_ref()
            .is_some_and(|expected| expected != operation)
        {
            return Err(ProviderError::Tool(
                "retained builtin output belongs to another operation".into(),
            ));
        }
        Ok((
            self.output,
            self.continuation,
            self.wait_barrier,
            self.wait_commit,
        ))
    }
}

/// Evidence from the owner of work that can outlive the provider future.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "snake_case")]
pub enum CancellationAcknowledgment {
    Stopped,
    /// The owner observed the operation's terminal result before cancellation won.
    Completed(Result<Value, ToolFailure>),
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

/// Draft disposition belongs to the complete invocation, never streamed output.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextDisposition {
    Unedited,
    Draft(crate::context::ContextDraft),
    Replay(crate::context::ContextCommitEvidence),
}

/// Exact provider-future completion. Embedded runtimes derive `full_success`
/// from their typed exit and confirmed cleanup, independently from value JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderCompletion {
    pub result: Result<Value, ToolFailure>,
    pub full_success: bool,
    pub context: ContextDisposition,
}

impl ProviderCompletion {
    pub fn unedited(result: Result<Value, ToolFailure>) -> Self {
        Self {
            full_success: result.is_ok(),
            result,
            context: ContextDisposition::Unedited,
        }
    }
}

/// A provider owns tool meaning; the harness owns scheduling and history.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Continuation bridges do not hold a scheduler slot while the host runs.
    fn holds_job_capacity(&self) -> bool {
        true
    }

    /// Read-only lookup of an exact retained call. Returning None permits live
    /// dispatch only after the scheduler or builtin owner rechecks cancellation.
    /// A retained cancellation is historical output, not new cleanup authority.
    async fn retained_output(
        &self,
        _name: &str,
        _input: &crate::item::ToolInput,
        _operation: &OperationId,
    ) -> Result<Option<RetainedOutput>, ProviderError> {
        Ok(None)
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

    /// Execution policy supplied by the host, independent of transport projection.
    fn tool_scheduling(&self, _name: &str) -> ToolScheduling {
        ToolScheduling::Async
    }

    /// Replay resolves the recorded operation policy instead of consulting a
    /// later installation. Ordinary providers use their immutable host policy.
    fn operation_scheduling(
        &self,
        _name: &str,
        _operation: &OperationId,
        declared: ToolScheduling,
    ) -> Result<ToolScheduling, ProviderError> {
        Ok(declared)
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

    /// Whole-invocation completion, returned before Store settlement. Hosts
    /// overriding this method must never await their own output acknowledgment.
    async fn complete_call(
        &self,
        name: &str,
        input: crate::item::ToolInput,
        context: CallContext,
    ) -> ProviderCompletion {
        let result = match input {
            crate::item::ToolInput::Function(args) if is_harness_tool(name) => {
                self.call_agent_verb(name, args, context).await
            }
            crate::item::ToolInput::Function(args) => {
                self.call_with_context(name, args, context).await
            }
            crate::item::ToolInput::Custom(raw) => {
                self.call_custom_with_context(name, raw, context).await
            }
        };
        ProviderCompletion::unedited(result.map_err(ProviderError::into_tool_failure))
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
        Err(ProviderError::Tool(
            format!("custom tool `{name}` is not supported by this provider").into(),
        ))
    }

    /// Runtime hook for crate-provided agent verbs. A provider integrating
    /// durable sessions should route this to `dispatch_agent_verb`.
    async fn call_agent_verb(
        &self,
        name: &str,
        _args: Value,
        _context: CallContext,
    ) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool(
            format!("agent verb `{name}` has no AgentToolService runtime").into(),
        ))
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
                    let name = object.get("name").and_then(Value::as_str).unwrap_or("");
                    let asynchronous = self.tool_scheduling(name) == ToolScheduling::Async;
                    object.insert("async".to_owned(), Value::Bool(asynchronous));
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
