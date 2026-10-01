//! Wire-stable WebSocket frames shared with the web client.
use crate::item::ToolKind;
use serde::{Deserialize, Serialize};

/// Browser projection of an Engine-owned call, not a second job registry.
/// `delivered` means the output was persisted in the conversation's input
/// history, not merely that the provider future settled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolJobRecord {
    pub id: String,
    pub conversation_id: String,
    pub request_id: String,
    pub call_id: String,
    pub tool_name: String,
    /// Missing on historical projections; live Engine jobs derive this from
    /// the scoped invocation persisted by Store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_kind: Option<ToolKind>,
    pub state: ToolJobState,
    pub delivered: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolJobState {
    Running,
    Settled,
    Cancelled,
    Interrupted,
}

pub use crate::embedding::HostIdentity as HostActorIdentity;

impl HostActorIdentity {
    /// Stable key used by browser actor.upsert/entity.remove projections.
    pub fn wire_key(&self) -> String {
        serde_json::to_string(&(&self.run, &self.actor, &self.incarnation))
            .expect("actor identity strings serialize")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostActorKind {
    Model,
    Workflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostActorLifecycle {
    Running,
    Waiting,
    Retiring,
    Retired,
    Lost,
}

/// A host-supplied browser row, including Haskell-only workflow actors.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostActorProjection {
    pub identity: HostActorIdentity,
    pub parent: Option<HostActorIdentity>,
    pub kind: HostActorKind,
    pub lifecycle: HostActorLifecycle,
    pub model_conversation: Option<String>,
    /// Exact actor-owned history entrypoint, independent of the activity rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_head_request: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_round: Option<crate::embedding::EmbeddedRoundId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandControl {
    Interrupt,
    Retire,
}

/// Bounded observation of a command handoff. It records admission or routing
/// receipts only; durable input and completion remain owned elsewhere.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandReceipt {
    pub command_id: String,
    #[serde(flatten)]
    pub outcome: CommandReceiptOutcome,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum CommandReceiptOutcome {
    Admitted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<HostActorIdentity>,
        envelope_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wake_error: Option<String>,
    },
    ControlRequested {
        target: HostActorIdentity,
        control: CommandControl,
    },
    Unconfirmed {
        target: HostActorIdentity,
        reason: String,
    },
    Refused {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<HostActorIdentity>,
        reason: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub seq: u64,
    /// Present only when this snapshot is projected from an embedded host run.
    /// An empty actor list does not imply standalone mode.
    #[serde(default, rename = "hostRun", skip_serializing_if = "Option::is_none")]
    pub host_run: Option<String>,
    #[serde(
        default,
        rename = "commandReceipts",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub command_receipts: Vec<CommandReceipt>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<HostActorProjection>,
    pub conversations: Vec<serde_json::Value>,
    pub requests: Vec<serde_json::Value>,
    pub jobs: Vec<serde_json::Value>,
    pub envelopes: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WsEvent {
    pub seq: u64,
    pub event: WsEventPayload,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WsEventPayload {
    pub kind: String,
    pub value: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsServerFrame {
    #[serde(rename = "command.refused")]
    CommandRefused {
        operation_id: Option<crate::embedding::ClientOperationId>,
        code: super::CommandRefusal,
        reason: String,
    },
    Snapshot {
        snapshot: Snapshot,
    },
    Event {
        event: WsEvent,
    },
    #[serde(rename = "command.accepted")]
    CommandAccepted {
        command_id: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsClientFrame {
    Command {
        command: String,
    },
    HostCommand {
        operation_id: crate::embedding::ClientOperationId,
        command: super::HostCommand,
    },
    #[serde(rename = "snapshot.request")]
    SnapshotRequest,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn async_tool_job_snapshot_preserves_identity_and_delivery() {
        let pending = ToolJobRecord {
            id: "tool/command-7/a".into(),
            conversation_id: "conversation/root".into(),
            request_id: "request-7".into(),
            call_id: "command-7/a".into(),
            tool_name: "deterministic_gate".into(),
            tool_kind: Some(ToolKind::Function),
            state: ToolJobState::Running,
            delivered: false,
            started_at_ms: None,
            ended_at_ms: None,
            output: None,
        };
        let mut snapshot = Snapshot::default();
        snapshot.jobs.push(serde_json::to_value(&pending).unwrap());
        let encoded = serde_json::to_string(&WsServerFrame::Snapshot { snapshot }).unwrap();
        let decoded: WsServerFrame = serde_json::from_str(&encoded).unwrap();
        let WsServerFrame::Snapshot { snapshot } = decoded else {
            panic!("expected snapshot");
        };
        let actual: ToolJobRecord = serde_json::from_value(snapshot.jobs[0].clone()).unwrap();
        assert_eq!(actual, pending);
        assert_eq!(snapshot.jobs[0]["callId"], "command-7/a");
        assert_eq!(snapshot.jobs[0]["toolKind"], "function");
        assert!(snapshot.jobs[0].get("output").is_none());
        let cancelled = ToolJobRecord {
            state: ToolJobState::Cancelled,
            delivered: true,
            output: Some(json!({"status":"Cancelled"})),
            ..pending
        };
        let wire = serde_json::to_value(&cancelled).unwrap();
        assert_eq!(wire["state"], "cancelled");
        assert_eq!(wire["callId"], "command-7/a");
        assert_eq!(wire["delivered"], true);
    }

    #[test]
    fn custom_tool_kind_is_typed_and_legacy_record_without_kind_reopens() {
        let mut legacy = json!({
            "id":"tool/root/call", "conversationId":"root", "requestId":"request-1",
            "callId":"call", "toolName":"run", "state":"running", "delivered":false
        });
        let restored: ToolJobRecord = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(restored.tool_kind, None);
        assert!(
            serde_json::to_value(&restored)
                .unwrap()
                .get("toolKind")
                .is_none()
        );
        legacy["toolKind"] = json!("custom");
        let custom: ToolJobRecord = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(custom.tool_kind, Some(ToolKind::Custom));
        assert_eq!(serde_json::to_value(custom).unwrap()["toolKind"], "custom");
        legacy["toolKind"] = json!("unknown");
        assert!(serde_json::from_value::<ToolJobRecord>(legacy).is_err());
    }

    #[test]
    fn serializes_snapshot_and_event_frames_to_web_wire_shapes() {
        let snapshot = Snapshot {
            seq: 11,
            conversations: vec![json!({"id":"root"})],
            ..Snapshot::default()
        };
        assert_eq!(
            serde_json::to_value(WsServerFrame::Snapshot {
                snapshot: snapshot.clone()
            })
            .unwrap(),
            json!({"type":"snapshot","snapshot":{
                "seq":11,"conversations":[{"id":"root"}],"requests":[],"jobs":[],"envelopes":[]
            }})
        );
        let event = WsServerFrame::Event {
            event: WsEvent {
                seq: 12,
                event: WsEventPayload {
                    kind: "job.started".into(),
                    value: json!({"id":"job-1"}),
                },
            },
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({"type":"event","event":{
                "seq":12,"event":{"kind":"job.started","value":{"id":"job-1"}}
            }})
        );
    }

    #[test]
    fn parses_commands_and_snapshot_resync_requests() {
        assert_eq!(
            serde_json::from_value::<WsClientFrame>(json!({"type":"command","command":"start"}))
                .unwrap(),
            WsClientFrame::Command {
                command: "start".into()
            }
        );
        assert_eq!(
            serde_json::from_value::<WsClientFrame>(json!({"type":"snapshot.request"})).unwrap(),
            WsClientFrame::SnapshotRequest
        );
    }

    #[test]
    fn model_actor_projection_roundtrips_optional_exact_head() {
        let mut actor = HostActorProjection {
            identity: HostActorIdentity {
                run: "run".into(),
                actor: crate::model::AgentPath("/root/old".into()),
                incarnation: "1".into(),
            },
            parent: None,
            kind: HostActorKind::Model,
            lifecycle: HostActorLifecycle::Retired,
            model_conversation: Some("/root/old".into()),
            model_head_request: None,
            active_round: None,
        };
        let old_wire = serde_json::to_value(&actor).unwrap();
        assert!(old_wire.get("modelHeadRequest").is_none());
        assert_eq!(
            serde_json::from_value::<HostActorProjection>(old_wire).unwrap(),
            actor
        );
        actor.model_head_request = Some("retained-head".into());
        let wire = serde_json::to_value(&actor).unwrap();
        assert_eq!(wire["modelHeadRequest"], "retained-head");
        assert_eq!(
            serde_json::from_value::<HostActorProjection>(wire).unwrap(),
            actor
        );
    }

    #[test]
    fn host_actor_projection_keeps_exact_incarnation_and_workflow_kind() {
        let parent = HostActorIdentity {
            run: "run-1".into(),
            actor: crate::model::AgentPath("root".into()),
            incarnation: "first".into(),
        };
        let child = HostActorProjection {
            identity: HostActorIdentity {
                run: "run-1".into(),
                actor: crate::model::AgentPath("reviewer".into()),
                incarnation: "second".into(),
            },
            parent: Some(parent.clone()),
            kind: HostActorKind::Workflow,
            lifecycle: HostActorLifecycle::Waiting,
            model_conversation: None,
            model_head_request: None,
            active_round: None,
        };
        let frame = WsServerFrame::Snapshot {
            snapshot: Snapshot {
                actors: vec![child.clone()],
                ..Snapshot::default()
            },
        };
        let wire = serde_json::to_value(&frame).unwrap();
        assert_eq!(
            wire["snapshot"]["actors"][0]["identity"]["incarnation"],
            "second"
        );
        assert_eq!(
            wire["snapshot"]["actors"][0]["parent"]["incarnation"],
            "first"
        );
        assert_eq!(wire["snapshot"]["actors"][0]["kind"], "workflow");
        assert_eq!(
            child.identity.wire_key(),
            r#"["run-1","reviewer","second"]"#
        );
        assert_eq!(
            wire["snapshot"]["actors"][0]["modelConversation"],
            serde_json::Value::Null
        );
        assert_eq!(
            serde_json::from_value::<WsServerFrame>(wire).unwrap(),
            frame
        );
    }

    #[test]
    fn command_receipt_wire_shapes_distinguish_handoff_from_completion() {
        let identity = HostActorIdentity {
            run: "run-1".into(),
            actor: crate::model::AgentPath("/root/worker".into()),
            incarnation: "inc-2".into(),
        };
        let admitted = CommandReceipt {
            command_id: "cmd-1".into(),
            outcome: CommandReceiptOutcome::Admitted {
                target: Some(identity.clone()),
                envelope_id: "envelope-9".into(),
                wake_error: None,
            },
        };
        assert_eq!(
            serde_json::to_value(&admitted).unwrap(),
            json!({
                "commandId":"cmd-1",
                "target":{"run":"run-1","actor":"/root/worker","incarnation":"inc-2"},
                "outcome":"admitted",
                "envelopeId":"envelope-9"
            })
        );
        assert_eq!(
            serde_json::from_value::<CommandReceipt>(serde_json::to_value(&admitted).unwrap())
                .unwrap(),
            admitted
        );

        let requested = CommandReceipt {
            command_id: "cmd-2".into(),
            outcome: CommandReceiptOutcome::ControlRequested {
                target: identity,
                control: CommandControl::Interrupt,
            },
        };
        assert_eq!(
            serde_json::to_value(requested).unwrap()["outcome"],
            "control_requested"
        );
    }
}

#[cfg(test)]
mod host_wire_tests {
    use super::*;
    use crate::{
        embedding::{ClientOperationId, EmbeddedRoundId, HostControl},
        server::{ClientCommand, HostCommand},
    };
    use serde_json::json;
    #[test]
    fn host_wire_requires_operation_and_interrupt_round_and_preserves_both() {
        let operation = ClientOperationId(uuid::Uuid::new_v4());
        let round = EmbeddedRoundId(uuid::Uuid::new_v4());
        let target = HostActorIdentity {
            run: "run".into(),
            actor: crate::model::AgentPath("/root".into()),
            incarnation: "one".into(),
        };
        let command = HostCommand::Interrupt {
            target,
            expected_round: round,
        };
        let frame = WsClientFrame::HostCommand {
            operation_id: operation,
            command: command.clone(),
        };
        let mut value = serde_json::to_value(&frame).unwrap();
        assert_eq!(value["operation_id"], operation.to_string());
        assert_eq!(value["command"]["expected_round"], round.to_string());
        assert_eq!(
            serde_json::from_value::<WsClientFrame>(value.clone()).unwrap(),
            frame
        );
        value.as_object_mut().unwrap().remove("operation_id");
        assert!(serde_json::from_value::<WsClientFrame>(value).is_err());
        let mut value = serde_json::to_value(ClientCommand::Host {
            operation_id: operation,
            command,
        })
        .unwrap();
        value["command"]
            .as_object_mut()
            .unwrap()
            .remove("expected_round");
        assert!(serde_json::from_value::<ClientCommand>(value).is_err());
        assert!(serde_json::from_value::<ClientOperationId>(json!("not-a-UUID")).is_err());
        assert!(serde_json::from_value::<EmbeddedRoundId>(json!("not-a-UUID")).is_err());
        assert_eq!(
            serde_json::to_value(HostControl::Interrupt {
                expected_round: round
            })
            .unwrap()["expected_round"],
            round.to_string()
        );
    }
}
