//! Browser projections own their wire shapes; Store records remain internal.
use super::{
    CommandReceipt, HostActorIdentity, HostActorKind, HostActorLifecycle, ToolJobState, WireI64,
    WireU64,
};
use crate::{
    embedding::{ClientOperationId, EmbeddedRoundId},
    item::ToolKind,
    model::ConversationIdentity,
    server::{CommandRefusal, HostCommand},
    store::actor_output as stored,
    transport::{RequestFailure, sse::OutputChannel},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Provider and tool JSON passes through this carrier without interpretation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonValue(pub serde_json::Value);
impl From<serde_json::Value> for JsonValue {
    fn from(value: serde_json::Value) -> Self {
        Self(value)
    }
}
impl JsonSchema for JsonValue {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "JsonValue".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // An explicit true branch accepts every JSON form even when a field's
        // description is attached; schema consumers must not infer an object.
        schemars::json_schema!({"anyOf": [true]})
    }
}
impl JsonValue {
    fn deserialize_present<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Self>, D::Error> {
        Self::deserialize(deserializer).map(Some)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorOutputOrigin {
    pub run: String,
    pub native_actor: WireU64,
    pub incarnation: WireU64,
}
impl From<&stored::ActorOutputOrigin> for ActorOutputOrigin {
    fn from(origin: &stored::ActorOutputOrigin) -> Self {
        Self {
            run: origin.run.clone(),
            native_actor: origin.native_actor.into(),
            incarnation: origin.incarnation.into(),
        }
    }
}
impl From<ActorOutputOrigin> for stored::ActorOutputOrigin {
    fn from(origin: ActorOutputOrigin) -> Self {
        Self {
            run: origin.run,
            native_actor: origin.native_actor.get(),
            incarnation: origin.incarnation.get(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorOutputReference {
    pub origin: ActorOutputOrigin,
    pub sequence: WireI64,
}
impl From<&stored::ActorOutputReference> for ActorOutputReference {
    fn from(reference: &stored::ActorOutputReference) -> Self {
        Self {
            origin: (&reference.origin).into(),
            sequence: reference.sequence.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorOutputId {
    pub display_slot: WireU64,
    pub page_ordinal: WireU64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorDisplayPage {
    pub text: String,
    pub expansions: Vec<(WireU64, String)>,
    pub unavailable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorOutputEmission {
    pub origin: ActorOutputOrigin,
    pub id: ActorOutputId,
    pub page: ActorDisplayPage,
}

/// The same projection is carried by history responses and committed events.
/// It conveys observations only, never an output admission capability.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorOutputProjection {
    pub reference: ActorOutputReference,
    pub emission: ActorOutputEmission,
}
impl From<&stored::StoredActorOutput> for ActorOutputProjection {
    fn from(output: &stored::StoredActorOutput) -> Self {
        let emission = output.emission();
        Self {
            reference: output.reference().into(),
            emission: ActorOutputEmission {
                origin: (&emission.origin).into(),
                id: ActorOutputId {
                    display_slot: emission.id.display_slot.into(),
                    page_ordinal: emission.id.page_ordinal.into(),
                },
                page: ActorDisplayPage {
                    text: emission.page.text.clone(),
                    expansions: emission
                        .page
                        .expansions
                        .iter()
                        .map(|(key, label)| ((*key).into(), label.clone()))
                        .collect(),
                    unavailable: emission.page.unavailable,
                },
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorOutputHistoryPage {
    pub origin: ActorOutputOrigin,
    pub outputs: Vec<ActorOutputProjection>,
    pub next_after: Option<WireI64>,
}
impl From<&stored::ActorOutputHistoryPage> for ActorOutputHistoryPage {
    fn from(page: &stored::ActorOutputHistoryPage) -> Self {
        Self {
            origin: (&page.origin).into(),
            outputs: page
                .outputs
                .iter()
                .map(ActorOutputProjection::from)
                .collect(),
            next_after: page.next_after.map(Into::into),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActorDisplayExpansion {
    pub origin: ActorOutputOrigin,
    pub display_slot: WireU64,
    pub key: WireU64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryItem {
    pub position: WireU64,
    pub hash: String,
    pub byte_len: WireU64,
    /// Opaque provider item body inside a typed retained-history observation.
    pub item: JsonValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OversizedHistoryItem {
    pub position: WireU64,
    pub hash: String,
    pub byte_len: WireU64,
    pub skip_offset: WireU64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryPage {
    pub request_id: String,
    pub parent_id: Option<String>,
    pub branch: String,
    pub items: Vec<HistoryItem>,
    pub next_offset: Option<WireU64>,
    pub oversized_item: Option<OversizedHistoryItem>,
}
impl From<&crate::store::history::HistoryPage> for HistoryPage {
    fn from(page: &crate::store::history::HistoryPage) -> Self {
        Self {
            request_id: page.request_id.clone(),
            parent_id: page.parent_id.clone(),
            branch: page.branch.clone(),
            items: page
                .items
                .iter()
                .map(|item| HistoryItem {
                    position: item.position.into(),
                    hash: item.hash.clone(),
                    byte_len: (item.byte_len as u64).into(),
                    item: item.item.0.clone().into(),
                })
                .collect(),
            next_offset: page.next_offset.map(Into::into),
            oversized_item: page
                .oversized_item
                .as_ref()
                .map(|item| OversizedHistoryItem {
                    position: item.position.into(),
                    hash: item.hash.clone(),
                    byte_len: (item.byte_len as u64).into(),
                    skip_offset: item.skip_offset.into(),
                }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostActorProjection {
    pub identity: HostActorIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_origin: Option<ActorOutputOrigin>,
    pub parent: Option<HostActorIdentity>,
    pub kind: HostActorKind,
    pub lifecycle: HostActorLifecycle,
    pub model_conversation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_head_request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_round: Option<EmbeddedRoundId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConversationState {
    Idle,
    Requesting,
    Paused,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationProjection {
    pub id: String,
    pub path: String,
    pub parent_id: Option<String>,
    pub fork_source_request_id: Option<String>,
    pub state: ConversationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<WireU64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    Running,
    Completed,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    Accepted,
    Pending,
    Queued,
    Presented,
    Acted,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RequestProjection {
    pub id: String,
    pub conversation_id: String,
    pub parent_id: Option<String>,
    pub created_at_ms: Option<WireI64>,
    pub ended_at_ms: Option<WireI64>,
    pub state: RequestState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RequestOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub failure: Option<RequestFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<WireU64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobProjection {
    pub id: String,
    pub conversation_id: String,
    pub state: ToolJobState,
    pub started_at_ms: Option<WireI64>,
    pub ended_at_ms: Option<WireI64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<ToolKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered: Option<bool>,
    /// The tool's own result is opaque inside this typed observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(deserialize_with = "JsonValue::deserialize_present")]
    pub output: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<WireU64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum EnvelopeKind {
    #[serde(rename = "NEW_TASK")]
    NewTask,
    #[serde(rename = "MESSAGE")]
    Message,
    #[serde(rename = "FINAL_ANSWER")]
    FinalAnswer,
    #[serde(rename = "PROGRESS")]
    Progress,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvelopeProjection {
    pub id: String,
    pub recipient: String,
    pub sender: String,
    #[serde(rename = "type")]
    pub kind: EnvelopeKind,
    pub payload: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<WireU64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<WireU64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputScope {
    pub origin: ConversationIdentity,
    pub request_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputItem {
    pub origin: ConversationIdentity,
    pub request_id: String,
    pub item_id: String,
    pub channel: OutputChannel,
    pub index: WireU64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputDelta {
    pub origin: ConversationIdentity,
    pub request_id: String,
    pub item_id: String,
    pub channel: OutputChannel,
    pub index: WireU64,
    pub text: String,
    pub version: WireU64,
    pub overflow: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputCommit {
    pub origin: ConversationIdentity,
    pub request_id: String,
    pub item_id: Option<String>,
    pub hash: String,
    pub version: WireU64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveOutput {
    pub origin: ConversationIdentity,
    pub request_id: String,
    pub item_id: String,
    pub channel: OutputChannel,
    pub index: WireU64,
    pub text: String,
    pub version: WireU64,
    pub overflow: bool,
    pub streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryRevision {
    pub origin: ConversationIdentity,
    pub request_id: String,
    pub version: WireU64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Actor,
    Conversation,
    Request,
    Job,
    Envelope,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EntityRemoval {
    pub entity: EntityKind,
    pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostRun {
    pub run: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "value", deny_unknown_fields)]
pub enum StateEvent {
    #[serde(rename = "actor.output.committed")]
    ActorOutputCommitted(ActorOutputProjection),
    #[serde(rename = "model.output.started")]
    ModelOutputStarted(OutputScope),
    #[serde(rename = "model.output.stopped")]
    ModelOutputStopped(OutputScope),
    #[serde(rename = "model.output.delta")]
    ModelOutputDelta(OutputDelta),
    #[serde(rename = "model.output.committed")]
    ModelOutputCommitted(OutputCommit),
    #[serde(rename = "model.output.remove")]
    ModelOutputRemove(OutputItem),
    #[serde(rename = "host_run.upsert")]
    HostRunUpsert(HostRun),
    #[serde(rename = "command.receipt")]
    CommandReceipt(CommandReceipt),
    #[serde(rename = "actor.upsert")]
    ActorUpsert(HostActorProjection),
    #[serde(rename = "conversation.upsert")]
    ConversationUpsert(ConversationProjection),
    #[serde(rename = "request.upsert")]
    RequestUpsert(RequestProjection),
    #[serde(rename = "job.upsert")]
    JobUpsert(JobProjection),
    #[serde(rename = "envelope.upsert")]
    EnvelopeUpsert(EnvelopeProjection),
    #[serde(rename = "entity.remove")]
    EntityRemove(EntityRemoval),
}

impl StateEvent {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::ActorOutputCommitted(_) => "actor.output.committed",
            Self::ModelOutputStarted(_) => "model.output.started",
            Self::ModelOutputStopped(_) => "model.output.stopped",
            Self::ModelOutputDelta(_) => "model.output.delta",
            Self::ModelOutputCommitted(_) => "model.output.committed",
            Self::ModelOutputRemove(_) => "model.output.remove",
            Self::HostRunUpsert(_) => "host_run.upsert",
            Self::CommandReceipt(_) => "command.receipt",
            Self::ActorUpsert(_) => "actor.upsert",
            Self::ConversationUpsert(_) => "conversation.upsert",
            Self::RequestUpsert(_) => "request.upsert",
            Self::JobUpsert(_) => "job.upsert",
            Self::EnvelopeUpsert(_) => "envelope.upsert",
            Self::EntityRemove(_) => "entity.remove",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SequencedEvent {
    pub seq: WireU64,
    pub event: StateEvent,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Snapshot {
    pub seq: WireU64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub live_output: Vec<LiveOutput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history_revisions: Vec<HistoryRevision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actor_output_revisions: Vec<ActorOutputReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_run: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<HostActorProjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command_receipts: Vec<CommandReceipt>,
    pub conversations: Vec<ConversationProjection>,
    pub requests: Vec<RequestProjection>,
    pub jobs: Vec<JobProjection>,
    pub envelopes: Vec<EnvelopeProjection>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ServerFrame {
    #[serde(rename = "snapshot")]
    Snapshot { snapshot: Snapshot },
    #[serde(rename = "event")]
    Event { event: SequencedEvent },
    #[serde(rename = "command.accepted")]
    CommandAccepted { command_id: String },
    #[serde(rename = "command.refused")]
    CommandRefused {
        operation_id: Option<ClientOperationId>,
        code: CommandRefusal,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ClientFrame {
    #[serde(rename = "command")]
    Command { command: String },
    #[serde(rename = "host_command")]
    HostCommand {
        operation_id: ClientOperationId,
        command: HostCommand,
    },
    #[serde(rename = "snapshot.request")]
    SnapshotRequest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmbeddedCommandRecord {
    pub operation_id: ClientOperationId,
    pub command: HostCommand,
    pub state: crate::store::EmbeddedCommandState,
    pub envelope_id: Option<WireI64>,
    pub receipt: Option<CommandReceipt>,
}
impl From<&crate::store::EmbeddedCommandRecord> for EmbeddedCommandRecord {
    fn from(record: &crate::store::EmbeddedCommandRecord) -> Self {
        Self {
            operation_id: record.operation_id,
            command: record.command.clone(),
            state: record.state,
            envelope_id: record.envelope_id.map(Into::into),
            receipt: record.receipt.clone(),
        }
    }
}

/// Schemas are generated separately for outbound serialization and inbound
/// deserialization: omitted optional fields and nullability follow Serde.
pub fn schemas() -> serde_json::Value {
    use schemars::generate::SchemaSettings;
    serde_json::json!({
        "serverEvent": SchemaSettings::draft07().for_serialize().into_generator().into_root_schema_for::<crate::server::ServerEvent>(),
        "server": SchemaSettings::draft07().for_serialize().into_generator().into_root_schema_for::<ServerFrame>(),
        "client": SchemaSettings::draft07().for_deserialize().into_generator().into_root_schema_for::<ClientFrame>(),
        "actorOutputHistory": SchemaSettings::draft07().for_serialize().into_generator().into_root_schema_for::<ActorOutputHistoryPage>(),
        "actorDisplayExpansion": SchemaSettings::draft07().for_deserialize().into_generator().into_root_schema_for::<ActorDisplayExpansion>(),
        "embeddedCommand": SchemaSettings::draft07().for_serialize().into_generator().into_root_schema_for::<EmbeddedCommandRecord>(),
        "history": SchemaSettings::draft07().for_serialize().into_generator().into_root_schema_for::<HistoryPage>(),
    })
}

/// Positive wire samples are encoded from the same Rust types as production.
/// Browser checks consume these alongside the generated schemas.
pub fn wire_samples() -> serde_json::Value {
    let conversation = ConversationProjection {
        id: "conversation/root".into(),
        path: "/root".into(),
        parent_id: None,
        fork_source_request_id: None,
        state: ConversationState::Idle,
        version: Some(u64::MAX.into()),
    };
    let job = JobProjection {
        id: "tool/root/call".into(),
        conversation_id: conversation.id.clone(),
        state: ToolJobState::Settled,
        started_at_ms: Some(i64::MIN.into()),
        ended_at_ms: Some(i64::MAX.into()),
        request_id: Some("request/root".into()),
        call_id: Some("call".into()),
        tool_name: Some("inspect".into()),
        tool_kind: Some(ToolKind::Custom),
        delivered: Some(true),
        output: Some(
            serde_json::json!({"providerOwned": [null, "opaque", {"status": "ready"}]}).into(),
        ),
        version: Some(9_007_199_254_740_993u64.into()),
    };
    let origin = ActorOutputOrigin {
        run: "run".into(),
        native_actor: (i64::MAX as u64).into(),
        incarnation: 9_007_199_254_740_993u64.into(),
    };
    let output = ActorOutputProjection {
        reference: ActorOutputReference {
            origin: origin.clone(),
            sequence: i64::MAX.into(),
        },
        emission: ActorOutputEmission {
            origin: origin.clone(),
            id: ActorOutputId {
                display_slot: 9_007_199_254_740_993u64.into(),
                page_ordinal: 1u64.into(),
            },
            page: ActorDisplayPage {
                text: "A retained value".into(),
                expansions: vec![(9_007_199_254_740_993u64.into(), "field".into())],
                unavailable: false,
            },
        },
    };
    let identity = HostActorIdentity {
        run: "run".into(),
        actor: crate::model::AgentPath("/root".into()),
        incarnation: "incarnation".into(),
    };
    let actor = HostActorProjection {
        identity: identity.clone(),
        output_origin: Some(origin.clone()),
        parent: None,
        kind: HostActorKind::Model,
        lifecycle: HostActorLifecycle::Waiting,
        model_conversation: Some(conversation.id.clone()),
        model_head_request: Some("request/root".into()),
        active_round: Some(EmbeddedRoundId(uuid::Uuid::nil())),
    };
    let request = RequestProjection {
        id: "request/root".into(),
        conversation_id: conversation.id.clone(),
        parent_id: None,
        created_at_ms: Some(i64::MIN.into()),
        ended_at_ms: Some(i64::MAX.into()),
        state: RequestState::Failed,
        command_id: None,
        command: None,
        outcome: Some(RequestOutcome::Failed),
        detail: Some("Provider refused the request".into()),
        failure: Some(RequestFailure::Http {
            status: 400,
            diagnostic: Some(crate::transport::HttpDiagnostic {
                code: Some("invalid_request".into()),
                ..Default::default()
            }),
        }),
        version: Some(u64::MAX.into()),
    };
    let receipt = CommandReceipt {
        command_id: uuid::Uuid::nil().to_string(),
        outcome: super::CommandReceiptOutcome::Admitted {
            target: Some(identity.clone()),
            envelope_id: i64::MAX.into(),
            wake_error: None,
        },
    };
    let scope = OutputScope {
        origin: ConversationIdentity::Embedded {
            run: identity.run.clone(),
            actor: identity.actor.clone(),
            incarnation: identity.incarnation.clone(),
        },
        request_id: request.id.clone(),
    };
    let item = OutputItem {
        origin: scope.origin.clone(),
        request_id: scope.request_id.clone(),
        item_id: "assistant".into(),
        channel: OutputChannel::Assistant,
        index: u64::MAX.into(),
    };
    let live_output = LiveOutput {
        origin: item.origin.clone(),
        request_id: item.request_id.clone(),
        item_id: item.item_id.clone(),
        channel: item.channel,
        index: item.index,
        text: "Hello".into(),
        version: u64::MAX.into(),
        overflow: false,
        streaming: false,
        committed_hash: Some("a".repeat(64)),
    };
    let envelope = EnvelopeProjection {
        id: format!("envelope/{}", i64::MAX),
        recipient: "/root".into(),
        sender: "operator".into(),
        kind: EnvelopeKind::Message,
        payload: "Continue".into(),
        ordinal: Some(u64::MAX.into()),
        version: None,
    };
    let events = [
        StateEvent::ConversationUpsert(conversation.clone()),
        StateEvent::RequestUpsert(request.clone()),
        StateEvent::JobUpsert(job.clone()),
        StateEvent::EnvelopeUpsert(envelope.clone()),
        StateEvent::ActorUpsert(actor.clone()),
        StateEvent::ActorOutputCommitted(output.clone()),
        StateEvent::ModelOutputStarted(scope.clone()),
        StateEvent::ModelOutputStopped(scope.clone()),
        StateEvent::ModelOutputDelta(OutputDelta {
            origin: item.origin.clone(),
            request_id: item.request_id.clone(),
            item_id: item.item_id.clone(),
            channel: item.channel,
            index: item.index,
            text: "Hello".into(),
            version: u64::MAX.into(),
            overflow: false,
        }),
        StateEvent::ModelOutputCommitted(OutputCommit {
            origin: scope.origin.clone(),
            request_id: scope.request_id.clone(),
            item_id: None,
            hash: "a".repeat(64),
            version: u64::MAX.into(),
        }),
        StateEvent::ModelOutputRemove(item),
        StateEvent::HostRunUpsert(HostRun {
            run: identity.run.clone(),
        }),
        StateEvent::CommandReceipt(receipt.clone()),
        StateEvent::EntityRemove(EntityRemoval {
            entity: EntityKind::Job,
            id: job.id.clone(),
        }),
    ];
    let opaque_values = [
        serde_json::json!({"provider": [null, "nested"]}),
        serde_json::json!([null, true, "retained"]),
        serde_json::json!("text"),
        serde_json::json!(23.5),
        serde_json::json!(false),
        serde_json::Value::Null,
    ];
    let opaque_history = opaque_values
        .iter()
        .map(|value| HistoryPage {
            request_id: "request/root".into(),
            parent_id: None,
            branch: "/root".into(),
            items: vec![HistoryItem {
                position: 9_007_199_254_740_993u64.into(),
                hash: "a".repeat(64),
                byte_len: (serde_json::to_vec(value).unwrap().len() as u64).into(),
                item: value.clone().into(),
            }],
            next_offset: Some(9_007_199_254_740_994u64.into()),
            oversized_item: None,
        })
        .collect::<Vec<_>>();
    let opaque_job_events = opaque_values
        .into_iter()
        .map(|value| {
            StateEvent::JobUpsert(JobProjection {
                output: Some(value.into()),
                ..job.clone()
            })
        })
        .collect::<Vec<_>>();
    let mut frames = vec![ServerFrame::Snapshot {
        snapshot: Snapshot {
            seq: 9_007_199_254_740_993u64.into(),
            conversations: vec![conversation],
            jobs: vec![job],
            requests: vec![request],
            actors: vec![actor],
            envelopes: vec![envelope],
            command_receipts: vec![receipt.clone()],
            live_output: vec![live_output],
            history_revisions: vec![HistoryRevision {
                origin: scope.origin,
                request_id: scope.request_id,
                version: u64::MAX.into(),
            }],
            host_run: Some(identity.run.clone()),
            actor_output_revisions: vec![output.reference.clone()],
            ..Snapshot::default()
        },
    }];
    frames.extend(
        events
            .into_iter()
            .chain(opaque_job_events)
            .enumerate()
            .map(|(index, event)| ServerFrame::Event {
                event: SequencedEvent {
                    seq: (9_007_199_254_740_994u64 + index as u64).into(),
                    event,
                },
            }),
    );
    frames.push(ServerFrame::CommandAccepted {
        command_id: "operation".into(),
    });
    frames.push(ServerFrame::CommandRefused {
        operation_id: None,
        code: CommandRefusal::Conflict,
        reason: "Owner has changed".into(),
    });
    serde_json::json!({
        "serverEvent": frames.iter().filter_map(|frame| match frame { ServerFrame::Event { event } => Some(crate::server::ServerEvent { sequence: event.seq, event: event.event.clone() }), _ => None }).collect::<Vec<_>>(),
        "server": frames,
        "client": [ClientFrame::Command { command: "start".into() }, ClientFrame::SnapshotRequest, ClientFrame::HostCommand { operation_id: ClientOperationId(uuid::Uuid::nil()), command: HostCommand::Interrupt { target: identity, expected_round: EmbeddedRoundId(uuid::Uuid::nil()) } }],
        "actorOutputHistory": [ActorOutputHistoryPage { origin: origin.clone(), outputs: vec![output.clone()], next_after: Some(i64::MAX.into()) }],
        "actorDisplayExpansion": [ActorDisplayExpansion { origin, display_slot: output.emission.id.display_slot, key: output.emission.page.expansions[0].0 }],
        "embeddedCommand": [EmbeddedCommandRecord { operation_id: ClientOperationId(uuid::Uuid::nil()), command: HostCommand::Input { target: HostActorIdentity { run: "run".into(), actor: crate::model::AgentPath("/root".into()), incarnation: "incarnation".into() }, text: "start".into() }, state: crate::store::EmbeddedCommandState::InputAdmitted, envelope_id: Some(i64::MAX.into()), receipt: Some(receipt) }, EmbeddedCommandRecord { operation_id: ClientOperationId(uuid::Uuid::nil()), command: HostCommand::Input { target: HostActorIdentity { run: "run".into(), actor: crate::model::AgentPath("/root".into()), incarnation: "incarnation".into() }, text: "start".into() }, state: crate::store::EmbeddedCommandState::Queued, envelope_id: None, receipt: None }],
        "history": opaque_history,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn typed_events_refuse_unknown_kinds_and_invalid_projection_fields() {
        let event = StateEvent::ConversationUpsert(ConversationProjection {
            id: "root".into(),
            path: "/root".into(),
            parent_id: None,
            fork_source_request_id: None,
            state: ConversationState::Idle,
            version: Some(u64::MAX.into()),
        });
        let wire = serde_json::to_value(&event).unwrap();
        assert_eq!(wire["value"]["version"], u64::MAX.to_string());
        assert_eq!(
            serde_json::from_value::<StateEvent>(wire.clone()).unwrap(),
            event
        );
        let mut unknown = wire.clone();
        unknown["kind"] = json!("conversation.started");
        assert!(serde_json::from_value::<StateEvent>(unknown).is_err());
        let mut invalid = wire.clone();
        invalid["value"]["state"] = json!("invented");
        assert!(serde_json::from_value::<StateEvent>(invalid).is_err());
        let mut lossy = wire;
        lossy["value"]["version"] = json!(9007199254740992u64);
        assert!(serde_json::from_value::<StateEvent>(lossy).is_err());
    }

    #[test]
    fn exported_positive_wire_samples_roundtrip_through_the_contract_owner() {
        let samples = wire_samples();
        for wire in samples["server"].as_array().unwrap() {
            let frame: ServerFrame = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(frame).unwrap(), *wire);
        }
        for wire in samples["client"].as_array().unwrap() {
            let frame: ClientFrame = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(frame).unwrap(), *wire);
        }
        for wire in samples["serverEvent"].as_array().unwrap() {
            let event: crate::server::ServerEvent = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(event).unwrap(), *wire);
        }
        for wire in samples["history"].as_array().unwrap() {
            let page: HistoryPage = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(page).unwrap(), *wire);
        }
        for wire in samples["embeddedCommand"].as_array().unwrap() {
            let record: EmbeddedCommandRecord = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(record).unwrap(), *wire);
        }
        for wire in samples["actorDisplayExpansion"].as_array().unwrap() {
            let input: ActorDisplayExpansion = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(input).unwrap(), *wire);
        }
        for wire in samples["actorOutputHistory"].as_array().unwrap() {
            let page: ActorOutputHistoryPage = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(page).unwrap(), *wire);
        }
    }
}
