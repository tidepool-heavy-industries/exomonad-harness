//! Conversations attached to an external actor owner. The host admits actors;
//! this module binds their model history without creating a second supervisor.
use crate::{
    item::{Item, ToolInput, ToolKind},
    model::{AgentPath, ConversationIdentity, RequestId},
    provider::{CallContext, CancellationOwner, Provider, ProviderError, ToolScheduling},
    store::{EmbeddedInputState, Store, StoreError},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

/// Opaque identity of one browser operation. Possession grants no host authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientOperationId(pub uuid::Uuid);

/// Opaque identity of the exact host execution round targeted by an interrupt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EmbeddedRoundId(pub uuid::Uuid);

// Match UUID's human-readable Serde parser, while outbound values always use
// the canonical lowercase hyphenated spelling.
fn browser_uuid_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let digits = if generator.contract().is_serialize() {
        "[0-9a-f]"
    } else {
        "[0-9a-fA-F]"
    };
    let hyphenated =
        format!("{digits}{{8}}-{digits}{{4}}-{digits}{{4}}-{digits}{{4}}-{digits}{{12}}");
    let pattern = if generator.contract().is_serialize() {
        format!(r"^{hyphenated}$(?![\s\S])")
    } else {
        format!(
            "^(?:{digits}{{32}}|{hyphenated}|\\{{{hyphenated}\\}}|urn:uuid:{hyphenated})$(?![\\s\\S])"
        )
    };
    schemars::json_schema!({"type": "string", "pattern": pattern})
}
impl schemars::JsonSchema for ClientOperationId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ClientOperationId".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        browser_uuid_schema(generator)
    }
}
impl schemars::JsonSchema for EmbeddedRoundId {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "EmbeddedRoundId".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        browser_uuid_schema(generator)
    }
}

impl std::fmt::Display for ClientOperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::fmt::Display for EmbeddedRoundId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HostIdentity {
    pub run: String,
    pub actor: AgentPath,
    pub incarnation: String,
}

/// Historical evidence of one atomic startup publication. References identify
/// original occurrences; later context edits preserve their identities and do
/// not change this receipt's recorded text. Possession grants no edit authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitialContextPublication {
    pub issuer: HostIdentity,
    pub operation_id: String,
    pub request: RequestId,
    pub caller_seed: Option<String>,
    pub authored_seed: Option<String>,
    pub caller_reference: Option<crate::context::ContextReference>,
    pub authored_reference: Option<crate::context::ContextReference>,
}

/// A durable interruption is independent of the host actor's lifetime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InferenceBlock {
    pub request: RequestId,
    pub generation: u64,
    pub reason: InferenceBlockReason,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InferenceBlockReason {
    StreamInterrupted {
        cause: crate::transport::StreamInterruption,
    },
    ProviderStreamFailure {
        event: crate::transport::ProviderStreamFailureEvent,
    },
    RequestRejected {
        failure: crate::transport::RequestFailure,
    },
    ContinuityUnproven,
    HostInterrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMode {
    Exact,
    VisibleHistory,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum HostInferenceState {
    Idle,
    Running,
    Blocked {
        block: InferenceBlock,
    },
    Recovering {
        operation_id: ClientOperationId,
        block: InferenceBlock,
    },
}

/// Native composition retains an admitted startup intent and the run lease.
/// Validation runs under the Store binding transaction and must not reenter Store.
pub trait BindingInitialAuthority: Send + Sync {
    fn validate_initial_binding(&self, identity: &HostIdentity) -> Result<bool, String>;
}

/// Native composition retains the actual run lease and latest journal proof.
/// Serialized identity fields alone never authorize replacing a live binding.
/// Validation runs under the Store binding transaction and must not reenter Store.
pub trait BindingSuccessorAuthority: Send + Sync {
    fn validate_successor(
        &self,
        predecessor: &HostIdentity,
        successor: &HostIdentity,
    ) -> Result<bool, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingSuccessorCommit {
    Installed,
    AlreadyInstalled,
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddedError {
    #[error(transparent)]
    Transport(#[from] crate::transport::TransportError),
    #[error(transparent)]
    Store(StoreError),
    #[error("host refused: {0}")]
    Host(String),
    #[error("host request admission closed")]
    AdmissionClosed,
    #[error("invalid embedded binding: {0}")]
    Binding(String),
    /// The embedding must fence its run store and discard its admission handles.
    /// Authenticated reopen is required before attempting another operation.
    #[error("embedded admission requires recovery; fence the run store and reopen: {0}")]
    RecoveryRequired(#[source] rusqlite::Error),
    #[error("invalid tool surface: {0}")]
    Surface(String),
    #[error("input operation was already admitted with different content")]
    ConflictingInput,
    #[error("context seed operation was already committed with different content")]
    ConflictingSeed,
    #[error("context seed requires a fresh conversation without history or inputs")]
    SeedRequiresFreshConversation,
}
impl From<StoreError> for EmbeddedError {
    fn from(value: StoreError) -> Self {
        match value {
            StoreError::RecoveryRequired(error) => Self::RecoveryRequired(error),
            other => Self::Store(other),
        }
    }
}
impl From<rusqlite::Error> for EmbeddedError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Store(value.into())
    }
}
impl From<serde_json::Error> for EmbeddedError {
    fn from(value: serde_json::Error) -> Self {
        Self::Store(value.into())
    }
}

/// A host-owned lease preventing retirement from overtaking admission. Dropping
/// it releases that protection through the host's existing lifecycle owner.
pub trait AdmissionGuard: Send {}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostControl {
    Interrupt { expected_round: EmbeddedRoundId },
    Retire,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum HostControlError {
    #[error("host control refused: {0}")]
    Refused(String),
    #[error("host control outcome unconfirmed: {0}")]
    Unconfirmed(String),
}

/// Capability supplied by the embedding, never selected by model arguments.
#[async_trait]
pub trait HostActor: Send + Sync {
    fn identity(&self) -> &HostIdentity;
    fn active_round(&self) -> Option<EmbeddedRoundId> {
        None
    }
    /// Implementations must release any issued guard when this future is dropped.
    async fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError>;
    /// Capture one immutable installation under the host owner's admission authority.
    async fn tool_surface(&self) -> Result<Arc<ToolSurface>, EmbeddedError>;
    /// Wake after the input transaction commits. Failure leaves the envelope
    /// admitted and retryable by its original operation ID.
    async fn wake(&self, envelope_id: i64) -> Result<(), String>;
    /// A notification after the Store commits an explicit recovery grant.
    /// The notification itself never authorizes inference.
    async fn wake_recovery(&self) -> Result<(), String> {
        Ok(())
    }
    /// A request to the host owner, not proof that retirement has completed.
    async fn control(&self, control: HostControl) -> Result<Value, HostControlError>;
    /// The owning conversation has durably retained this operation's real result.
    /// Retried acknowledgments must not repeat execution or lifecycle admission.
    async fn output_committed(&self, _operation: &crate::model::OperationId) -> Result<(), String> {
        Ok(())
    }
    /// Retained terminal did not publish its staged context transaction.
    async fn output_aborted(&self, _operation: &crate::model::OperationId) -> Result<(), String> {
        Ok(())
    }
}

/// Immutable manifest and dispatcher published together by the host. Reload
/// replaces this value; requests retain the old value until their calls end.
pub struct ToolSurface {
    version: String,
    manifest: Arc<EmbeddedToolManifest>,
    dispatcher: Arc<dyn Provider>,
}

/// Validated declarations shared by requests from one immutable installation.
pub struct EmbeddedToolManifest {
    tools: crate::transport::ToolManifest,
    kinds: HashMap<String, ToolKind>,
    scheduling: HashMap<String, ToolScheduling>,
}

impl EmbeddedToolManifest {
    pub fn new(tools: Vec<Value>) -> Result<Self, EmbeddedError> {
        Self::with_scheduling(tools, HashMap::new())
    }

    pub fn with_scheduling(
        mut tools: Vec<Value>,
        scheduling: HashMap<String, ToolScheduling>,
    ) -> Result<Self, EmbeddedError> {
        let mut kinds = HashMap::new();
        for tool in &mut tools {
            let name = tool["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| EmbeddedError::Surface("missing tool name".into()))?;
            let kind = match tool["type"].as_str() {
                Some("function") if tool["strict"] == true => ToolKind::Function,
                Some("custom") => ToolKind::Custom,
                _ => {
                    return Err(EmbeddedError::Surface(format!(
                        "unsupported declaration for {name}"
                    )));
                }
            };
            if crate::provider::is_harness_tool(name) || name == crate::finalize::FINALIZE_TOOL_NAME
            {
                return Err(EmbeddedError::Surface(format!(
                    "reserved standalone tool name {name}"
                )));
            }
            if kinds.insert(name.to_owned(), kind).is_some() {
                return Err(EmbeddedError::Surface(format!("duplicate tool {name}")));
            }
        }
        if scheduling.keys().any(|name| !kinds.contains_key(name)) {
            return Err(EmbeddedError::Surface(
                "scheduling policy names an undeclared tool".into(),
            ));
        }
        for tool in &mut tools {
            let name = tool["name"].as_str().expect("validated name");
            let mode = scheduling.get(name).copied().unwrap_or_default();
            tool.as_object_mut()
                .expect("validated tool")
                .insert("async".into(), Value::Bool(mode == ToolScheduling::Async));
        }
        let tools: crate::transport::ToolManifest = tools.into();
        tools
            .strict_tools()
            .map_err(|error| EmbeddedError::Surface(error.to_string()))?;
        Ok(Self {
            tools,
            kinds,
            scheduling,
        })
    }

    pub fn tools(&self) -> &crate::transport::ToolManifest {
        &self.tools
    }
}

impl ToolSurface {
    pub fn new(
        version: String,
        tools: Vec<Value>,
        dispatcher: Arc<dyn Provider>,
    ) -> Result<Self, EmbeddedError> {
        Self::from_manifest(
            version,
            Arc::new(EmbeddedToolManifest::new(tools)?),
            dispatcher,
        )
    }

    pub fn from_manifest(
        version: String,
        manifest: Arc<EmbeddedToolManifest>,
        dispatcher: Arc<dyn Provider>,
    ) -> Result<Self, EmbeddedError> {
        if version.is_empty() {
            return Err(EmbeddedError::Surface("empty version".into()));
        }
        Ok(Self {
            version,
            manifest,
            dispatcher,
        })
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn tools(&self) -> &[Value] {
        self.manifest.tools()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputReceipt {
    pub envelope_id: i64,
    /// Delivery failure does not revoke the durable admission.
    pub wake_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", content = "request", rename_all = "snake_case")]
pub enum InputObservation {
    Admitted,
    Included(RequestId),
}

/// Read-only access to durable input observations for one exact host binding.
/// The observer does not retain the actor capability or perform admission.
pub struct InputObserver {
    store: Arc<Store>,
    identity: HostIdentity,
}
impl InputObserver {
    /// Read whether this exact host operation has been admitted or included
    /// in a request. Historical operations remain readable after successor transfer. Missing
    /// operations require the current binding and return `None`.
    pub fn input_observation_by_operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<InputObservation>, EmbeddedError> {
        Ok(
            match self
                .store
                .embedded_input_state(&self.identity, operation_id)?
            {
                EmbeddedInputState::Missing => None,
                EmbeddedInputState::Admitted => Some(InputObservation::Admitted),
                EmbeddedInputState::Included(request) => Some(InputObservation::Included(request)),
            },
        )
    }
}

pub struct Conversation {
    store: Arc<Store>,
    host: Arc<dyn HostActor>,
}
impl Conversation {
    /// Register model history for an already admitted host actor. Existing
    /// durable metadata is checked, never used to resurrect a live host.
    pub async fn attach(
        store: Arc<Store>,
        host: Arc<dyn HostActor>,
        parent: Option<&AgentPath>,
    ) -> Result<Self, EmbeddedError> {
        let _admission = host.admit().await?;
        store.bind_embedded_actor(host.identity(), parent)?;
        Ok(Self { store, host })
    }
    /// Attach checkpoint history and host identity in one Store transaction.
    /// Host admission precedes every write; this does not start or supervise an actor.
    pub async fn from_checkpoint<T: ?Sized + Send + Sync + 'static>(
        store: Arc<Store>,
        host: Arc<dyn HostActor>,
        parent: &AgentPath,
        checkpoint: &crate::checkpoint::Checkpoint<T>,
        contract: &Value,
        checkout: &Value,
        target: &crate::context::ContextProjectionTarget,
    ) -> Result<Self, EmbeddedError> {
        let _admission = host.admit().await?;
        store.attach_bound_checkpoint_child(
            checkpoint,
            crate::checkpoint::CheckpointChild {
                path: &host.identity().actor,
                parent,
                contract,
                checkout,
                task: None,
                target,
            },
            host.identity(),
        )?;
        Ok(Self { store, host })
    }

    /// Construct the model loop for this binding. The host manifest supplies
    /// provider tools; Engine adds its reserved continuation intrinsic. The
    /// caller supplies shared Store/scheduler infrastructure.
    pub fn engine<A: crate::transport::Auth, C: crate::engine::ResponsesTransport>(
        &self,
        transport: C,
        scheduler: Arc<crate::turn::JobScheduler>,
        config: crate::engine::EngineConfig,
        context_capacity: std::num::NonZeroU64,
    ) -> Result<crate::engine::Engine<A, BoundProvider, C>, EmbeddedError> {
        if config.agent != self.identity().actor || !config.tools.is_empty() {
            return Err(EmbeddedError::Binding(
                "Engine identity/tools must come from its bound host".into(),
            ));
        }
        let origin = ConversationIdentity::Embedded {
            run: self.identity().run.clone(),
            actor: self.identity().actor.clone(),
            incarnation: self.identity().incarnation.clone(),
        };
        let (route, protocol) = transport.response_contract()?;
        self.store.initialize_context_target(
            &origin,
            &crate::context::ContextProjectionTarget {
                model: config.model.clone(),
                route,
                protocol,
            },
        )?;
        Ok(crate::engine::Engine::with_transport(
            transport,
            self.store.clone(),
            scheduler,
            self.provider(),
            config,
        )
        .with_origin(origin)
        .with_plain_text_compaction(context_capacity))
    }
    pub fn active_round(&self) -> Option<EmbeddedRoundId> {
        self.host.active_round()
    }
    pub fn identity(&self) -> &HostIdentity {
        self.host.identity()
    }
    /// Derive a read-only observer for this exact host incarnation. It can
    /// outlive the conversation without retaining the actor capability.
    pub fn input_observer(&self) -> InputObserver {
        InputObserver {
            store: self.store.clone(),
            identity: self.identity().clone(),
        }
    }
    pub fn provider(&self) -> Arc<BoundProvider> {
        Arc::new(BoundProvider {
            store: self.store.clone(),
            host: self.host.clone(),
        })
    }
    /// Persist the initial user prompt as settled history without admitting an
    /// input envelope or waking the actor. Exact operation retries return the
    /// original seed even after subsequent model history has advanced.
    pub async fn seed_context(
        &self,
        operation_id: &str,
        prompt: &str,
    ) -> Result<RequestId, EmbeddedError> {
        let _admission = self.host.admit().await?;
        let item = Item(serde_json::json!({"type":"message","role":"user","content":prompt}));
        self.store
            .seed_embedded_context(self.identity(), operation_id, &item)
    }

    /// Publish caller context followed by one editable authored context block
    /// without waking inference. An empty authored result has no block. Exact
    /// retries return historical evidence and never restore previously edited text.
    pub async fn seed_initial_context(
        &self,
        operation_id: &str,
        caller_seed: Option<&str>,
        authored_seed: Option<&str>,
    ) -> Result<InitialContextPublication, EmbeddedError> {
        let _admission = self.host.admit().await?;
        self.store
            .seed_initial_context(self.identity(), operation_id, caller_seed, authored_seed)
    }

    /// Observe the issuing operation after reattachment, including an earlier
    /// incarnation of this same conversation. This never republishes context.
    pub fn initial_context_publication(
        &self,
        issuer: &HostIdentity,
        operation_id: &str,
    ) -> Result<Option<InitialContextPublication>, EmbeddedError> {
        self.store
            .initial_context_publication(self.identity(), issuer, operation_id)
    }

    pub async fn input(
        &self,
        operation_id: &str,
        sender: &str,
        text: &str,
    ) -> Result<InputReceipt, EmbeddedError> {
        let _admission = self.host.admit().await?;
        let item = Item(serde_json::json!({"type":"message","role":"user","content":text}));
        let envelope_id =
            self.store
                .admit_embedded_input(self.identity(), operation_id, sender, &item)?;
        drop(_admission);
        let wake_error = match self.input_observation(envelope_id)? {
            InputObservation::Admitted => self.host.wake(envelope_id).await.err(),
            InputObservation::Included(_) => None,
        };
        Ok(InputReceipt {
            envelope_id,
            wake_error,
        })
    }
    /// Admit a previously claimed browser command atomically with its input
    /// envelope. Exact admitted retries read Store without live admission/wake.
    pub async fn command_input(
        &self,
        operation: ClientOperationId,
        text: &str,
    ) -> Result<crate::server::CommandReceipt, EmbeddedError> {
        let record = self
            .store
            .embedded_command(&self.identity().run, operation)?
            .ok_or(StoreError::InvalidCommandState)?;
        let expected = crate::server::HostCommand::Input {
            target: self.identity().clone(),
            text: text.into(),
        };
        if record.command != expected {
            return Err(StoreError::ConflictingCommand.into());
        }
        if record.state == crate::store::EmbeddedCommandState::InputAdmitted {
            return record
                .receipt
                .ok_or_else(|| StoreError::InvalidCommandState.into());
        }
        if record.state != crate::store::EmbeddedCommandState::Dispatching {
            return Err(StoreError::InvalidCommandState.into());
        }
        let admission = self.host.admit().await?;
        let admitted = self
            .store
            .admit_embedded_command_input(self.identity(), operation, text)?;
        drop(admission);
        let receipt = match admitted {
            crate::store::CommandInputAdmission::New(receipt) => receipt,
            crate::store::CommandInputAdmission::Retained(receipt) => return Ok(receipt),
        };
        let envelope = match &receipt.outcome {
            crate::server::CommandReceiptOutcome::Admitted { envelope_id, .. } => envelope_id.get(),
            _ => return Err(StoreError::InvalidCommandState.into()),
        };
        // Wake is a best-effort hint after the durable atomic commit. The host's
        // existing loop also drains admitted inputs on recovery.
        if let Err(error) = self.host.wake(envelope).await {
            return Ok(self.store.record_embedded_command_wake_error(
                &self.identity().run,
                operation,
                error,
            )?);
        }
        Ok(receipt)
    }

    pub fn input_observation(&self, envelope_id: i64) -> Result<InputObservation, EmbeddedError> {
        let envelope = self
            .store
            .envelope(envelope_id)?
            .filter(|e| e.recipient == self.identity().actor.0)
            .ok_or_else(|| {
                EmbeddedError::Binding("input does not belong to this conversation".into())
            })?;
        Ok(envelope
            .delivered_request
            .map_or(InputObservation::Admitted, InputObservation::Included))
    }
    pub async fn command_recover(
        &self,
        operation: ClientOperationId,
    ) -> Result<crate::server::CommandReceipt, EmbeddedError> {
        let record = self
            .store
            .embedded_command(&self.identity().run, operation)?
            .ok_or(StoreError::InvalidCommandState)?;
        if !matches!(&record.command, crate::server::HostCommand::Recover { target, .. } if target == self.identity())
        {
            return Err(StoreError::ConflictingCommand.into());
        }
        if let Some(receipt) = record.receipt {
            return Ok(receipt);
        }
        let admission = self.host.admit().await?;
        let receipt = self
            .store
            .authorize_embedded_recovery(self.identity(), operation)?;
        drop(admission);
        if matches!(
            receipt.outcome,
            crate::server::CommandReceiptOutcome::RecoveryAuthorized { .. }
        ) {
            let _ = self.host.wake_recovery().await;
        }
        Ok(receipt)
    }
    /// Read whether this exact host operation has been admitted or included
    /// in a request. Observation remains available after actor retirement and
    /// never performs admission, wake, or redispatch.
    pub fn input_observation_by_operation(
        &self,
        operation_id: &str,
    ) -> Result<Option<InputObservation>, EmbeddedError> {
        self.input_observer()
            .input_observation_by_operation(operation_id)
    }
    pub async fn control(&self, control: HostControl) -> Result<Value, HostControlError> {
        self.host.control(control).await
    }
}

pub struct BoundProvider {
    store: Arc<Store>,
    host: Arc<dyn HostActor>,
}
struct PinnedProvider {
    store: Arc<Store>,
    host: Arc<dyn HostActor>,
    surface: Arc<ToolSurface>,
}
impl PinnedProvider {
    async fn validate(
        &self,
        name: &str,
        input: &ToolInput,
        context: &CallContext,
    ) -> Result<(), ProviderError> {
        let fail = |message: &str| ProviderError::Tool(message.to_owned().into());
        let _admission = self.host.admit().await.map_err(|e| fail(&e.to_string()))?;
        if context.agent != self.host.identity().actor {
            return Err(fail("foreign actor call"));
        }
        let operation = context
            .operation
            .as_ref()
            .ok_or_else(|| fail("embedded call requires operation identity"))?;
        let expected_origin = ConversationIdentity::Embedded {
            run: self.host.identity().run.clone(),
            actor: self.host.identity().actor.clone(),
            incarnation: self.host.identity().incarnation.clone(),
        };
        if operation.origin != expected_origin
            || operation.call != context.call_id
            || context.request.as_ref() != Some(&operation.request)
        {
            return Err(fail("foreign operation call"));
        }
        let request = context
            .request
            .as_ref()
            .ok_or_else(|| fail("embedded call requires request identity"))?;
        let stored = self
            .store
            .request(request)
            .map_err(|e| fail(&e.to_string()))?
            .ok_or_else(|| fail("unknown request"))?;
        if stored.branch != context.agent.0 {
            return Err(fail("foreign request call"));
        }
        if self.surface.manifest.kinds.get(name) != Some(&input.kind()) {
            return Err(fail("call does not match issuing tool surface"));
        }
        let recorded = self
            .store
            .invocation_item(request, &context.call_id)
            .map_err(|e| fail(&e.to_string()))?;
        if !recorded.is_some_and(|call| call.name == name && &call.input == input) {
            return Err(fail("call does not match durable invocation"));
        }
        let version = self
            .store
            .latest_tool_surface(request)
            .map_err(|e| fail(&e.to_string()))?;
        if version.as_ref().and_then(|value| value["version"].as_str())
            != Some(self.surface.version())
        {
            return Err(fail("request did not publish this tool surface"));
        }
        Ok(())
    }
}
#[async_trait]
impl Provider for BoundProvider {
    async fn output_committed(
        &self,
        operation: &crate::model::OperationId,
    ) -> Result<(), ProviderError> {
        let identity = self.host.identity();
        let expected = ConversationIdentity::Embedded {
            run: identity.run.clone(),
            actor: identity.actor.clone(),
            incarnation: identity.incarnation.clone(),
        };
        if operation.origin != expected {
            return Err(ProviderError::Tool("foreign output acknowledgment".into()));
        }
        self.host
            .output_committed(operation)
            .await
            .map_err(|error| ProviderError::Tool(error.into()))
    }

    async fn output_aborted(
        &self,
        operation: &crate::model::OperationId,
    ) -> Result<(), ProviderError> {
        let identity = self.host.identity();
        let expected = ConversationIdentity::Embedded {
            run: identity.run.clone(),
            actor: identity.actor.clone(),
            incarnation: identity.incarnation.clone(),
        };
        if operation.origin != expected {
            return Err(ProviderError::Tool("foreign output abort".into()));
        }
        self.host
            .output_aborted(operation)
            .await
            .map_err(|error| ProviderError::Tool(error.into()))
    }

    async fn request_snapshot(&self) -> Result<Option<Arc<dyn Provider>>, ProviderError> {
        let surface = self
            .host
            .tool_surface()
            .await
            .map_err(|error| match error {
                EmbeddedError::AdmissionClosed => ProviderError::RequestAdmissionClosed,
                error => ProviderError::Tool(error.to_string().into()),
            })?;
        Ok(Some(Arc::new(PinnedProvider {
            store: self.store.clone(),
            host: self.host.clone(),
            surface,
        })))
    }
    fn tools(&self) -> Vec<Value> {
        // Embedded tools are obtained from the asynchronous request snapshot.
        Vec::new()
    }
    fn all_tools(&self) -> Vec<Value> {
        self.tools()
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool(
            "embedded dispatch requires a request snapshot".into(),
        ))
    }
}
#[async_trait]
impl Provider for PinnedProvider {
    fn holds_job_capacity(&self) -> bool {
        self.surface.dispatcher.holds_job_capacity()
    }
    fn tool_scheduling(&self, name: &str) -> ToolScheduling {
        self.surface
            .manifest
            .scheduling
            .get(name)
            .copied()
            .unwrap_or_default()
    }
    fn tool_manifest(&self) -> crate::transport::ToolManifest {
        self.surface.manifest.tools.clone()
    }
    fn tool_surface_version(&self) -> Option<&str> {
        Some(self.surface.version())
    }
    fn validate_call(&self, name: &str, kind: ToolKind) -> Result<(), ProviderError> {
        if self.surface.manifest.kinds.get(name) == Some(&kind) {
            Ok(())
        } else {
            Err(ProviderError::Tool(
                "call does not match issuing tool surface".into(),
            ))
        }
    }
    fn tools(&self) -> Vec<Value> {
        self.surface.tools().to_vec()
    }
    fn all_tools(&self) -> Vec<Value> {
        self.tools()
    }
    fn cancellation_owner(&self) -> Option<Arc<dyn CancellationOwner>> {
        self.surface.dispatcher.cancellation_owner()
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool(
            "embedded dispatch requires call identity".into(),
        ))
    }
    async fn before_request(
        &self,
        plan: &crate::hooks::RequestPlanView<'_>,
    ) -> crate::hooks::BeforeRequestResult {
        self.surface.dispatcher.before_request(plan).await
    }
    async fn complete_call(
        &self,
        name: &str,
        input: ToolInput,
        context: CallContext,
    ) -> crate::provider::ProviderCompletion {
        if let Err(error) = self.validate(name, &input, &context).await {
            return crate::provider::ProviderCompletion::unedited(Err(error.into_tool_failure()));
        }
        self.surface
            .dispatcher
            .complete_call(name, input, context)
            .await
    }
    async fn call_with_context(
        &self,
        name: &str,
        args: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        self.validate(name, &ToolInput::Function(args.clone()), &context)
            .await?;
        self.surface
            .dispatcher
            .call_with_context(name, args, context)
            .await
    }
    async fn call_custom_with_context(
        &self,
        name: &str,
        input: String,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        self.validate(name, &ToolInput::Custom(input.clone()), &context)
            .await?;
        self.surface
            .dispatcher
            .call_custom_with_context(name, input, context)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct SeedHost(HostIdentity);
    struct SeedGuard;
    impl AdmissionGuard for SeedGuard {}
    #[async_trait]
    impl HostActor for SeedHost {
        fn identity(&self) -> &HostIdentity {
            &self.0
        }
        async fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError> {
            Ok(Box::new(SeedGuard))
        }
        async fn tool_surface(&self) -> Result<Arc<ToolSurface>, EmbeddedError> {
            panic!("seeding must not construct a model request")
        }
        async fn wake(&self, _: i64) -> Result<(), String> {
            panic!("seeding must not wake the actor")
        }
        async fn control(&self, _: HostControl) -> Result<Value, HostControlError> {
            panic!("seeding must not control the actor")
        }
    }
    fn seed_host() -> Arc<SeedHost> {
        Arc::new(SeedHost(HostIdentity {
            run: "seed-run".into(),
            actor: AgentPath("/root".into()),
            incarnation: "seed-incarnation".into(),
        }))
    }

    struct PendingAdmissionHost {
        identity: HostIdentity,
        started: tokio::sync::Notify,
        live_guards: Arc<std::sync::atomic::AtomicUsize>,
    }
    struct CountedGuard(Arc<std::sync::atomic::AtomicUsize>);
    impl AdmissionGuard for CountedGuard {}
    impl Drop for CountedGuard {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    #[async_trait]
    impl HostActor for PendingAdmissionHost {
        fn identity(&self) -> &HostIdentity {
            &self.identity
        }
        async fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError> {
            self.live_guards
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _guard = CountedGuard(self.live_guards.clone());
            self.started.notify_one();
            std::future::pending().await
        }
        async fn tool_surface(&self) -> Result<Arc<ToolSurface>, EmbeddedError> {
            panic!("attachment must not capture tools")
        }
        async fn wake(&self, _: i64) -> Result<(), String> {
            panic!("attachment must not wake")
        }
        async fn control(&self, _: HostControl) -> Result<Value, HostControlError> {
            panic!("attachment must not control")
        }
    }
    #[tokio::test]
    async fn cancelled_attachment_releases_pending_admission_without_binding() {
        let store = Arc::new(Store::memory().unwrap());
        let host = Arc::new(PendingAdmissionHost {
            identity: seed_host().identity().clone(),
            started: tokio::sync::Notify::new(),
            live_guards: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let attachment = tokio::spawn(Conversation::attach(store.clone(), host.clone(), None));
        host.started.notified().await;
        assert_eq!(
            host.live_guards.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        attachment.abort();
        assert!(matches!(attachment.await, Err(error) if error.is_cancelled()));
        assert_eq!(
            host.live_guards.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert!(!store.embedded_binding_matches(host.identity()).unwrap());
    }

    #[tokio::test]
    async fn initial_context_empty_and_absent_preserve_caller_and_do_not_wake() {
        for authored in [None, Some("")] {
            let store = Arc::new(Store::memory().unwrap());
            let host = seed_host();
            let conversation = Conversation::attach(store.clone(), host.clone(), None)
                .await
                .unwrap();
            let receipt = conversation
                .seed_initial_context("startup", Some("caller"), authored)
                .await
                .unwrap();
            assert_eq!(receipt.authored_seed.as_deref(), authored);
            assert!(receipt.authored_reference.is_none());
            let document = store.read_context(&receipt.request).unwrap();
            assert!(
                matches!(&document.blocks[..], [crate::context::ContextBlock::Text { reference, text, .. }]
                if reference == &receipt.caller_reference && text == "caller")
            );
            assert_eq!(
                conversation
                    .seed_initial_context("startup", Some("caller"), authored)
                    .await
                    .unwrap(),
                receipt
            );
            assert!(matches!(
                conversation
                    .seed_initial_context(
                        "startup",
                        Some("caller"),
                        if authored.is_some() { None } else { Some("") }
                    )
                    .await,
                Err(EmbeddedError::ConflictingSeed)
            ));
            assert!(store.inbox("/root").unwrap().is_empty());
            assert!(store.model_request_outcomes(128).unwrap().is_empty());
        }
        let store = Arc::new(Store::memory().unwrap());
        let conversation = Conversation::attach(store.clone(), seed_host(), None)
            .await
            .unwrap();
        let receipt = conversation
            .seed_initial_context("empty", None, Some(""))
            .await
            .unwrap();
        assert!(
            store
                .read_context(&receipt.request)
                .unwrap()
                .blocks
                .is_empty()
        );
        assert!(receipt.caller_reference.is_none());
        assert!(receipt.authored_reference.is_none());
    }

    fn initial_context_edit(
        store: &Store,
        receipt: &InitialContextPublication,
        reference: &crate::context::ContextReference,
        index: usize,
        text: &str,
    ) -> Result<
        (
            crate::context::ContextSnapshot,
            crate::context::ContextDraft,
        ),
        StoreError,
    > {
        let frontier = store.embedded_round_frontier(&receipt.issuer)?;
        let call = crate::model::CallId(format!("edit-{index}"));
        let item = Item(json!({"type":"custom_tool_call", "call_id":call.0,
            "name":"haskell_sync", "input":"refresh"}));
        let head = if let Some(pending) = frontier.pending_head {
            store.append_items(&pending, &[item])?;
            pending
        } else {
            let head = RequestId(format!("edit-round-{index}"));
            store.write_embedded_request(
                &receipt.issuer,
                &head,
                frontier.settled_head.as_ref(),
                &[item],
                crate::store::Usage::default(),
            )?;
            head
        };
        let operation = store.claim(&call, &head)?;
        store.initialize_context_target(
            &operation.origin,
            &crate::context::ContextProjectionTarget {
                model: "luna".into(),
                route: crate::transport::ResponsesRoute::ChatGptPlan,
                protocol: crate::transport::ResponsesProtocol::Standard,
            },
        )?;
        let snapshot = store.begin_context(&operation, &head)?;
        let mut draft = crate::context::ContextDraft {
            document: snapshot.document.clone(),
            next_model: None,
            next_effort: None,
        };
        // A caller can provide a foreign reference, but cannot turn it into authority.
        let authored = draft.document.blocks.iter_mut().find(|block| matches!(block,
            crate::context::ContextBlock::Text { reference: Some(r), .. } if Some(r) == receipt.authored_reference.as_ref())).unwrap();
        if let crate::context::ContextBlock::Text {
            reference: target,
            text: body,
            ..
        } = authored
        {
            *target = Some(reference.clone());
            *body = text.into();
        }
        Ok((snapshot, draft))
    }

    fn refresh_initial_context(
        store: &Store,
        receipt: &InitialContextPublication,
        reference: &crate::context::ContextReference,
        index: usize,
        text: &str,
    ) -> Result<crate::context::ContextCommitReceipt, StoreError> {
        let settled = store.embedded_agent_head(&receipt.issuer)?;
        let (snapshot, draft) = initial_context_edit(store, receipt, reference, index, text)?;
        let committed = store.commit_context(crate::context::ContextCommit {
            snapshot: &snapshot,
            draft: &draft,
            output: &crate::turn::JobOutput::Completed(Ok(json!({"value":"done"}))),
            pending: &[],
        })?;
        if !store.settle_embedded_round(
            &receipt.issuer,
            settled.as_ref(),
            &committed.head,
            crate::store::EmbeddedRoundOutcome::Completed,
        )? {
            return Err(StoreError::InvalidEmbeddedFrontier);
        }
        Ok(committed)
    }

    #[tokio::test]
    async fn initial_context_equal_bodies_have_distinct_editable_references() {
        let store = Arc::new(Store::memory().unwrap());
        let conversation = Conversation::attach(store.clone(), seed_host(), None)
            .await
            .unwrap();
        let receipt = conversation
            .seed_initial_context("startup", Some("same"), Some("same"))
            .await
            .unwrap();
        assert_ne!(receipt.caller_reference, receipt.authored_reference);
        let committed = refresh_initial_context(
            &store,
            &receipt,
            receipt.authored_reference.as_ref().unwrap(),
            0,
            "replacement",
        )
        .unwrap();
        let active = store.read_context(&committed.head).unwrap();
        assert!(
            matches!(&active.blocks[0], crate::context::ContextBlock::Text { reference, text, .. }
            if reference == &receipt.caller_reference && text == "same")
        );
        assert!(
            matches!(&active.blocks[1], crate::context::ContextBlock::Text { reference, text, .. }
            if reference == &receipt.authored_reference && text == "replacement")
        );
    }

    #[tokio::test]
    async fn initial_context_failed_invocation_and_stale_edit_preserve_active_memory() {
        let store = Arc::new(Store::memory().unwrap());
        let conversation = Conversation::attach(store.clone(), seed_host(), None)
            .await
            .unwrap();
        let receipt = conversation
            .seed_initial_context("startup", None, Some("memory"))
            .await
            .unwrap();
        let reference = receipt.authored_reference.as_ref().unwrap();
        let (failed_snapshot, failed_draft) =
            initial_context_edit(&store, &receipt, reference, 0, "failed edit").unwrap();
        assert!(matches!(
            store.commit_context(crate::context::ContextCommit {
                snapshot: &failed_snapshot,
                draft: &failed_draft,
                output: &crate::turn::JobOutput::Completed(Err("authored failure".into())),
                pending: &[],
            }),
            Err(StoreError::Context(
                crate::context::ContextError::Ineligible
            ))
        ));
        assert_eq!(
            store.embedded_agent_head(conversation.identity()).unwrap(),
            Some(receipt.request.clone())
        );
        let frontier = store
            .embedded_round_frontier(conversation.identity())
            .unwrap();
        let pending_document = store
            .read_context(
                frontier
                    .pending_head
                    .as_ref()
                    .or(frontier.settled_head.as_ref())
                    .unwrap(),
            )
            .unwrap();
        assert!(pending_document.blocks.iter().any(|block| matches!(block,
            crate::context::ContextBlock::Text { reference: Some(r), text, .. }
                if r == reference && text == "memory")));
        let (stale_snapshot, stale_draft) =
            initial_context_edit(&store, &receipt, reference, 1, "stale edit").unwrap();
        let committed =
            refresh_initial_context(&store, &receipt, reference, 2, "accepted edit").unwrap();
        assert!(matches!(
            store.commit_context(crate::context::ContextCommit {
                snapshot: &stale_snapshot,
                draft: &stale_draft,
                output: &crate::turn::JobOutput::Completed(Ok(json!({"value":"done"}))),
                pending: &[],
            }),
            Err(StoreError::Context(crate::context::ContextError::Conflict))
        ));
        assert_eq!(
            store.embedded_agent_head(conversation.identity()).unwrap(),
            Some(committed.head.clone())
        );
        let active = store.read_context(&committed.head).unwrap();
        assert!(active.blocks.iter().any(|block| matches!(block,
            crate::context::ContextBlock::Text { reference: Some(r), text, .. }
                if r == reference && text == "accepted edit")));
        assert_eq!(
            conversation
                .initial_context_publication(&receipt.issuer, "startup")
                .unwrap(),
            Some(receipt)
        );
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(32))]
        #[test]
        fn initial_context_history_keeps_receipt_after_edits_retries_and_reopen(
            caller in "[a-z λ]{0,24}", initial in "[a-z λ]{1,24}",
            edits in proptest::collection::vec("[a-z λ]{0,24}", 1..5),
        ) {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let directory = std::env::temp_dir().join(format!("harness-initial-history-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&directory).unwrap();
            let path = directory.join("store.sqlite");
            let host = seed_host();
            let receipt;
            {
                let store = Arc::new(Store::open(&path).unwrap());
                let conversation = Conversation::attach(store.clone(), host.clone(), None).await.unwrap();
                receipt = conversation.seed_initial_context("startup", Some(&caller), Some(&initial)).await.unwrap();
                let reference = receipt.authored_reference.as_ref().unwrap();
                proptest::prop_assert_ne!(receipt.caller_reference.as_ref().unwrap(), reference);
                for (index, replacement) in edits.iter().enumerate() {
                    let committed = refresh_initial_context(&store, &receipt, reference, index, replacement).unwrap();
                    let active = store.read_context(&committed.head).unwrap();
                    let authored = active.blocks.iter().find(|block| matches!(block,
                        crate::context::ContextBlock::Text { reference: Some(r), .. } if r == reference)).unwrap();
                    proptest::prop_assert!(matches!(authored, crate::context::ContextBlock::Text { text, .. } if text == replacement), "the referenced authored block retains the replacement");
                    proptest::prop_assert!(active.blocks.iter().any(|block| matches!(block,
                        crate::context::ContextBlock::Text { reference: Some(r), text, .. } if Some(r) == receipt.caller_reference.as_ref() && text == &caller)), "the caller block retains its original body");
                    proptest::prop_assert_eq!(conversation.seed_initial_context("startup", Some(&caller), Some(&initial)).await.unwrap(), receipt.clone());
                    proptest::prop_assert_eq!(store.embedded_agent_head(host.identity()).unwrap(), Some(committed.head));
                }
            }
            {
                let store = Arc::new(Store::open(&path).unwrap());
                let conversation = Conversation::attach(store.clone(), host.clone(), None).await.unwrap();
                proptest::prop_assert_eq!(conversation.initial_context_publication(&receipt.issuer, "startup").unwrap(), Some(receipt.clone()));
                proptest::prop_assert_eq!(conversation.seed_initial_context("startup", Some(&caller), Some(&initial)).await.unwrap(), receipt.clone());
                let head = store.embedded_agent_head(host.identity()).unwrap().unwrap();
                let active = store.read_context(&head).unwrap();
                proptest::prop_assert!(active.blocks.iter().any(|block| matches!(block,
                    crate::context::ContextBlock::Text { reference: Some(r), text, .. } if Some(r) == receipt.authored_reference.as_ref() && text == edits.last().unwrap())), "reopen retains the last accepted replacement");
            }
            std::fs::remove_dir_all(directory).unwrap();
            Ok(())
            })?;
        }
    }

    #[tokio::test]
    async fn initial_context_foreign_reference_and_receipt_are_refused() {
        let store = Arc::new(Store::memory().unwrap());
        let first = Conversation::attach(store.clone(), seed_host(), None)
            .await
            .unwrap();
        let receipt = first
            .seed_initial_context("startup", Some("caller"), Some("memory"))
            .await
            .unwrap();
        let foreign = Conversation::attach(
            store.clone(),
            Arc::new(SeedHost(HostIdentity {
                actor: AgentPath("/root/foreign".into()),
                ..first.identity().clone()
            })),
            Some(&first.identity().actor),
        )
        .await
        .unwrap();
        let other = foreign
            .seed_initial_context("startup", None, Some("foreign memory"))
            .await
            .unwrap();
        let first_head = store.embedded_agent_head(first.identity()).unwrap();
        let foreign_head = store.embedded_agent_head(foreign.identity()).unwrap();
        assert!(matches!(
            first.initial_context_publication(&other.issuer, "startup"),
            Err(EmbeddedError::Binding(_))
        ));
        assert!(matches!(
            refresh_initial_context(
                &store,
                &receipt,
                other.authored_reference.as_ref().unwrap(),
                0,
                "replacement"
            ),
            Err(StoreError::Context(
                crate::context::ContextError::InvalidReference
            ))
        ));
        assert_eq!(
            first
                .initial_context_publication(&receipt.issuer, "startup")
                .unwrap(),
            Some(receipt.clone())
        );
        assert_eq!(
            store.embedded_agent_head(first.identity()).unwrap(),
            first_head
        );
        assert_eq!(
            store.embedded_agent_head(foreign.identity()).unwrap(),
            foreign_head
        );
        let foreign_document = store.read_context(&foreign_head.unwrap()).unwrap();
        assert!(foreign_document.blocks.iter().any(|block| matches!(block,
            crate::context::ContextBlock::Text { reference: Some(r), text, .. }
                if Some(r) == other.authored_reference.as_ref() && text == "foreign memory")));
        let frontier = store.embedded_round_frontier(first.identity()).unwrap();
        let document = store
            .read_context(
                frontier
                    .pending_head
                    .as_ref()
                    .or(frontier.settled_head.as_ref())
                    .unwrap(),
            )
            .unwrap();
        assert!(document.blocks.iter().any(|block| matches!(block,
            crate::context::ContextBlock::Text { reference: Some(r), text, .. }
                if Some(r) == receipt.authored_reference.as_ref() && text == "memory")));
    }

    #[tokio::test]
    async fn context_seed_reopens_exact_prompt_without_input_or_pending_round() {
        let directory = std::env::temp_dir().join(format!("harness-seed-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("store.sqlite");
        let host = seed_host();
        let prompt = "  Initial context\nλ\n\n";
        let request = {
            let store = Arc::new(Store::open(&path).unwrap());
            let conversation = Conversation::attach(store.clone(), host.clone(), None)
                .await
                .unwrap();
            let request = conversation.seed_context("spawn-op", prompt).await.unwrap();
            assert!(store.inbox("/root").unwrap().is_empty());
            assert!(store.events(None).unwrap().is_empty());
            assert!(store.model_request_outcomes(128).unwrap().is_empty());
            assert_eq!(
                store.usage_subtree(&request).unwrap(),
                crate::store::Usage::default()
            );
            let frontier = store.embedded_round_frontier(host.identity()).unwrap();
            assert_eq!(frontier.settled_head, Some(request.clone()));
            assert_eq!(frontier.pending_head, None);
            request
        };
        {
            let store = Arc::new(Store::open(&path).unwrap());
            let conversation = Conversation::attach(store.clone(), host.clone(), None)
                .await
                .unwrap();
            assert_eq!(
                conversation.seed_context("spawn-op", prompt).await.unwrap(),
                request
            );
            assert!(store.model_request_outcomes(128).unwrap().is_empty());
            assert_eq!(
                store.usage_subtree(&request).unwrap(),
                crate::store::Usage::default()
            );
            assert_eq!(
                store.items(&request).unwrap(),
                vec![Item(json!({
                    "type":"message", "role":"user", "content":prompt
                }))]
            );
            let next = RequestId("next-model-history".into());
            store
                .create_request(&next, Some(&request), "/root")
                .unwrap();
            assert!(
                store
                    .advance_agent_head(&host.identity().actor, Some(&request), Some(&next))
                    .unwrap()
            );
            assert_eq!(
                conversation.seed_context("spawn-op", prompt).await.unwrap(),
                request
            );
            assert_eq!(
                store.embedded_agent_head(host.identity()).unwrap(),
                Some(next)
            );
            assert!(matches!(
                conversation.seed_context("spawn-op", "changed").await,
                Err(EmbeddedError::ConflictingSeed)
            ));
            assert!(matches!(
                conversation.seed_context("other-op", prompt).await,
                Err(EmbeddedError::SeedRequiresFreshConversation)
            ));
            assert!(store.inbox("/root").unwrap().is_empty());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn context_seed_refuses_input_history_and_foreign_binding_without_partial_writes() {
        for started_with_input in [false, true] {
            let store = Arc::new(Store::memory().unwrap());
            let host = seed_host();
            let conversation = Conversation::attach(store.clone(), host.clone(), None)
                .await
                .unwrap();
            if started_with_input {
                store
                    .admit_embedded_input(
                        host.identity(),
                        "input",
                        "operator",
                        &Item(json!({
                            "type":"message", "role":"user", "content":"existing input"
                        })),
                    )
                    .unwrap();
            } else {
                store
                    .create_request(&RequestId("pending-history".into()), None, "/root")
                    .unwrap();
            }
            assert!(matches!(
                conversation.seed_context("seed", "prompt").await,
                Err(EmbeddedError::SeedRequiresFreshConversation)
            ));
            assert_eq!(store.embedded_agent_head(host.identity()).unwrap(), None);
            let foreign = Conversation {
                store: store.clone(),
                host: Arc::new(SeedHost(HostIdentity {
                    incarnation: "foreign".into(),
                    ..host.identity().clone()
                })),
            };
            assert!(matches!(
                foreign.seed_context("seed", "prompt").await,
                Err(EmbeddedError::Binding(_))
            ));
            assert!(matches!(
                conversation.seed_context("", "prompt").await,
                Err(EmbeddedError::Binding(_))
            ));
            let count: i64 = store
                .lock()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM embedded_context_seeds", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
        }
    }

    #[test]
    fn installed_manifest_pins_typed_scheduling_and_derives_wire_flag() {
        let tool = serde_json::json!({"type":"custom","name":"cell","description":"Run a cell","async":false});
        let asynchronous = EmbeddedToolManifest::new(vec![tool.clone()]).unwrap();
        assert_eq!(asynchronous.tools()[0]["async"], true);
        let policies = HashMap::from([("cell".to_owned(), ToolScheduling::BeforeNextInference)]);
        let synchronous = EmbeddedToolManifest::with_scheduling(vec![tool], policies).unwrap();
        assert_eq!(synchronous.tools()[0]["async"], false);
        assert_eq!(
            synchronous.scheduling["cell"],
            ToolScheduling::BeforeNextInference
        );
        assert!(
            EmbeddedToolManifest::with_scheduling(
                vec![],
                HashMap::from([("missing".to_owned(), ToolScheduling::Async)])
            )
            .is_err()
        );
    }

    #[test]
    fn installed_manifest_refuses_false_strict_claim_and_keeps_metadata() {
        let valid = json!({"type":"function","name":"status","strict":true,"parameters":{
            "type":"object","properties":{"view":{"type":["string","null"],"description":"A view"}},
            "required":["view"],"additionalProperties":false
        }});
        let manifest = EmbeddedToolManifest::new(vec![valid.clone()]).unwrap();
        assert_eq!(manifest.tools()[0]["parameters"], valid["parameters"]);
        let mut incomplete = valid.clone();
        incomplete["parameters"]
            .as_object_mut()
            .unwrap()
            .remove("required");
        assert!(matches!(
            EmbeddedToolManifest::new(vec![incomplete]),
            Err(EmbeddedError::Surface(_))
        ));
        let mut unchecked_nested = valid;
        unchecked_nested["parameters"]["properties"]["view"] = json!({"type":"object","properties":{"value":{"type":"string"}},"additionalProperties":false});
        assert!(EmbeddedToolManifest::new(vec![unchecked_nested]).is_err());
    }
}
