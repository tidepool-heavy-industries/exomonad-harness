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
    pub output: Option<serde_json::Value>,
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

/// Schemas are generated separately for outbound serialization and inbound
/// deserialization: omitted optional fields and nullability follow Serde.
pub fn schemas() -> serde_json::Value {
    use schemars::generate::SchemaSettings;
    serde_json::json!({
        "server": SchemaSettings::draft07().for_serialize().into_generator().into_root_schema_for::<ServerFrame>(),
        "client": SchemaSettings::draft07().for_deserialize().into_generator().into_root_schema_for::<ClientFrame>(),
        "actorOutputHistory": SchemaSettings::draft07().for_serialize().into_generator().into_root_schema_for::<ActorOutputHistoryPage>(),
        "actorDisplayExpansion": SchemaSettings::draft07().for_deserialize().into_generator().into_root_schema_for::<ActorDisplayExpansion>(),
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
        output: Some(serde_json::json!({"providerOwned": [null, "opaque", {"status": "ready"}]})),
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
    let events = [
        StateEvent::ConversationUpsert(conversation.clone()),
        StateEvent::JobUpsert(job.clone()),
        StateEvent::ActorOutputCommitted(output.clone()),
        StateEvent::EntityRemove(EntityRemoval {
            entity: EntityKind::Job,
            id: job.id.clone(),
        }),
    ];
    let mut frames = vec![ServerFrame::Snapshot {
        snapshot: Snapshot {
            seq: 9_007_199_254_740_993u64.into(),
            conversations: vec![conversation],
            jobs: vec![job],
            actor_output_revisions: vec![output.reference.clone()],
            ..Snapshot::default()
        },
    }];
    frames.extend(
        events
            .into_iter()
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
        "server": frames,
        "client": [ClientFrame::Command { command: "start".into() }, ClientFrame::SnapshotRequest],
        "actorOutputHistory": [ActorOutputHistoryPage { origin: origin.clone(), outputs: vec![output.clone()], next_after: Some(i64::MAX.into()) }],
        "actorDisplayExpansion": [ActorDisplayExpansion { origin, display_slot: output.emission.id.display_slot, key: output.emission.page.expansions[0].0 }],
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
        for wire in samples["actorOutputHistory"].as_array().unwrap() {
            let page: ActorOutputHistoryPage = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(page).unwrap(), *wire);
        }
    }
}
