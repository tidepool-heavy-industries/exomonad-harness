//! Conversations attached to an external actor owner. The host admits actors;
//! this module binds their model history without creating a second supervisor.
use crate::{
    item::{Item, ToolInput, ToolKind},
    model::{AgentPath, ConversationIdentity, RequestId},
    provider::{CallContext, CancellationOwner, Provider, ProviderError},
    store::{Store, StoreError},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostIdentity {
    pub run: String,
    pub actor: AgentPath,
    pub incarnation: String,
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddedError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("host refused: {0}")]
    Host(String),
    #[error("invalid embedded binding: {0}")]
    Binding(String),
    #[error("invalid tool surface: {0}")]
    Surface(String),
    #[error("input operation was already admitted with different content")]
    ConflictingInput,
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
    Interrupt,
    Retire,
}

/// Capability supplied by the embedding, never selected by model arguments.
#[async_trait]
pub trait HostActor: Send + Sync {
    fn identity(&self) -> &HostIdentity;
    fn admit(&self) -> Result<Box<dyn AdmissionGuard>, EmbeddedError>;
    fn tool_surface(&self) -> Arc<ToolSurface>;
    /// Wake after the input transaction commits. Failure leaves the envelope
    /// admitted and retryable by its original operation ID.
    async fn wake(&self, envelope_id: i64) -> Result<(), String>;
    /// A request to the host owner, not proof that retirement has completed.
    async fn control(&self, control: HostControl) -> Result<Value, String>;
}

/// Immutable manifest and dispatcher published together by the host. Reload
/// replaces this value; requests retain the old value until their calls end.
pub struct ToolSurface {
    version: String,
    tools: Vec<Value>,
    kinds: HashMap<String, ToolKind>,
    dispatcher: Arc<dyn Provider>,
}
impl ToolSurface {
    pub fn new(
        version: String,
        mut tools: Vec<Value>,
        dispatcher: Arc<dyn Provider>,
    ) -> Result<Self, EmbeddedError> {
        if version.is_empty() {
            return Err(EmbeddedError::Surface("empty version".into()));
        }
        let mut kinds = HashMap::new();
        for tool in &mut tools {
            if let Some(object) = tool.as_object_mut() {
                object.insert("async".into(), Value::Bool(true));
            }
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
        Ok(Self {
            version,
            tools,
            kinds,
            dispatcher,
        })
    }
    pub fn version(&self) -> &str {
        &self.version
    }
    pub fn tools(&self) -> &[Value] {
        &self.tools
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputReceipt {
    pub envelope_id: i64,
    /// Delivery failure does not revoke the durable admission.
    pub wake_error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputObservation {
    Admitted,
    Included(RequestId),
}

pub struct Conversation {
    store: Arc<Store>,
    host: Arc<dyn HostActor>,
}
impl Conversation {
    /// Register model history for an already admitted host actor. Existing
    /// durable metadata is checked, never used to resurrect a live host.
    pub fn attach(
        store: Arc<Store>,
        host: Arc<dyn HostActor>,
        parent: Option<&AgentPath>,
    ) -> Result<Self, EmbeddedError> {
        let _admission = host.admit()?;
        store.bind_embedded_actor(host.identity(), parent)?;
        Ok(Self { store, host })
    }
    /// Attach checkpoint history and host identity in one Store transaction.
    /// Host admission precedes every write; this does not start or supervise an actor.
    pub fn from_checkpoint<T: ?Sized + Send + Sync + 'static>(
        store: Arc<Store>,
        host: Arc<dyn HostActor>,
        parent: &AgentPath,
        checkpoint: &crate::checkpoint::Checkpoint<T>,
        contract: &Value,
        checkout: &Value,
    ) -> Result<Self, EmbeddedError> {
        let _admission = host.admit()?;
        store.attach_bound_checkpoint_child(
            checkpoint,
            crate::checkpoint::CheckpointChild {
                path: &host.identity().actor,
                parent,
                contract,
                checkout,
                task: None,
            },
            host.identity(),
        )?;
        Ok(Self { store, host })
    }

    /// Construct the model loop for this binding. The host manifest is the sole
    /// tool surface; the caller supplies shared Store/scheduler infrastructure.
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
        Ok(crate::engine::Engine::with_transport(
            transport,
            self.store.clone(),
            scheduler,
            self.provider(),
            config,
        )
        .with_origin(ConversationIdentity::Embedded {
            run: self.identity().run.clone(),
            actor: self.identity().actor.clone(),
            incarnation: self.identity().incarnation.clone(),
        })
        .with_plain_text_compaction(context_capacity))
    }
    pub fn identity(&self) -> &HostIdentity {
        self.host.identity()
    }
    pub fn provider(&self) -> Arc<BoundProvider> {
        Arc::new(BoundProvider {
            store: self.store.clone(),
            host: self.host.clone(),
        })
    }
    pub async fn input(
        &self,
        operation_id: &str,
        sender: &str,
        text: &str,
    ) -> Result<InputReceipt, EmbeddedError> {
        let _admission = self.host.admit()?;
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
    pub async fn control(&self, control: HostControl) -> Result<Value, EmbeddedError> {
        self.host
            .control(control)
            .await
            .map_err(EmbeddedError::Host)
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
    fn validate(
        &self,
        name: &str,
        input: &ToolInput,
        context: &CallContext,
    ) -> Result<(), ProviderError> {
        let fail = |message: &str| ProviderError::Tool(message.to_owned());
        let _admission = self.host.admit().map_err(|e| fail(&e.to_string()))?;
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
        if self.surface.kinds.get(name) != Some(&input.kind()) {
            return Err(fail("call does not match issuing tool surface"));
        }
        let items = self
            .store
            .items(request)
            .map_err(|e| fail(&e.to_string()))?;
        let recorded = items
            .iter()
            .filter_map(|item| item.tool_call().ok().flatten())
            .filter(|call| call.call_id == context.call_id)
            .collect::<Vec<_>>();
        if recorded.len() != 1 || recorded[0].name != name || &recorded[0].input != input {
            return Err(fail("call does not match durable invocation"));
        }
        let events = self
            .store
            .events(Some(request))
            .map_err(|e| fail(&e.to_string()))?;
        let version = events
            .iter()
            .rev()
            .find(|event| event.kind == "tool_surface")
            .and_then(|event| serde_json::from_str::<Value>(&event.payload).ok());
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
    fn request_snapshot(&self) -> Option<Arc<dyn Provider>> {
        Some(Arc::new(PinnedProvider {
            store: self.store.clone(),
            host: self.host.clone(),
            surface: self.host.tool_surface(),
        }))
    }
    fn tools(&self) -> Vec<Value> {
        self.host.tool_surface().tools.clone()
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
    fn tool_surface_version(&self) -> Option<&str> {
        Some(self.surface.version())
    }
    fn validate_call(&self, name: &str, kind: ToolKind) -> Result<(), ProviderError> {
        if self.surface.kinds.get(name) == Some(&kind) {
            Ok(())
        } else {
            Err(ProviderError::Tool(
                "call does not match issuing tool surface".into(),
            ))
        }
    }
    fn tools(&self) -> Vec<Value> {
        self.surface.tools.clone()
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
        plan: &crate::hooks::RequestPlan,
    ) -> crate::hooks::BeforeRequestResult {
        self.surface.dispatcher.before_request(plan).await
    }
    async fn call_with_context(
        &self,
        name: &str,
        args: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        self.validate(name, &ToolInput::Function(args.clone()), &context)?;
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
        self.validate(name, &ToolInput::Custom(input.clone()), &context)?;
        self.surface
            .dispatcher
            .call_custom_with_context(name, input, context)
            .await
    }
}
