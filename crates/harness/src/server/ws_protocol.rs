//! Wire-stable WebSocket frames shared with the web client.
use crate::item::ToolKind;
use serde::{Deserialize, Serialize};

pub mod contract;
mod contract_extensions;
mod decimal;
pub use decimal::{WireI64, WireU64};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostActorKind {
    Model,
    Workflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostActorLifecycle {
    Running,
    Waiting,
    Retiring,
    Retired,
    Lost,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
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
    RecoveryAuthorized {
        target: HostActorIdentity,
        block: crate::embedding::InferenceBlock,
        mode: crate::embedding::RecoveryMode,
        projection: crate::context::RecoveryProjectionReceipt,
    },
    Admitted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<HostActorIdentity>,
        envelope_id: WireI64,
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

pub use contract::{
    ClientFrame as WsClientFrame, HostActorProjection, SequencedEvent as WsEvent,
    ServerFrame as WsServerFrame, Snapshot, StateEvent as WsEventPayload,
};

impl From<&ToolJobRecord> for contract::JobProjection {
    fn from(record: &ToolJobRecord) -> Self {
        Self {
            id: record.id.clone(),
            conversation_id: record.conversation_id.clone(),
            request_id: Some(record.request_id.clone()),
            call_id: Some(record.call_id.clone()),
            tool_name: Some(record.tool_name.clone()),
            tool_kind: record.tool_kind,
            state: record.state,
            delivered: Some(record.delivered),
            started_at_ms: record.started_at_ms.map(Into::into),
            ended_at_ms: record.ended_at_ms.map(Into::into),
            output: record.output.clone().map(Into::into),
            version: None,
        }
    }
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
        snapshot.jobs.push((&pending).into());
        let encoded = serde_json::to_string(&WsServerFrame::Snapshot { snapshot }).unwrap();
        let decoded: WsServerFrame = serde_json::from_str(&encoded).unwrap();
        let WsServerFrame::Snapshot { snapshot } = decoded else {
            panic!("expected snapshot");
        };
        assert_eq!(snapshot.jobs[0], contract::JobProjection::from(&pending));
        assert_eq!(snapshot.jobs[0].call_id.as_deref(), Some("command-7/a"));
        assert_eq!(snapshot.jobs[0].tool_kind, Some(ToolKind::Function));
        assert!(snapshot.jobs[0].output.is_none());
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
            seq: 11u64.into(),
            conversations: vec![crate::server::fixture_conversation("root", "/root")],
            ..Snapshot::default()
        };
        assert_eq!(
            serde_json::to_value(WsServerFrame::Snapshot {
                snapshot: snapshot.clone()
            })
            .unwrap(),
            json!({"type":"snapshot","snapshot":{
                "seq":"11","conversations":[crate::server::fixture_conversation("root", "/root")],"requests":[],"jobs":[],"envelopes":[]
            }})
        );
        let event = WsServerFrame::Event {
            event: WsEvent {
                seq: 12u64.into(),
                event: WsEventPayload::JobUpsert(crate::server::fixture_job("job-1")),
            },
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({"type":"event","event":{
                "seq":"12","event":{"kind":"job.upsert","value":crate::server::fixture_job("job-1")}
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
            output_origin: None,
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
            inference: None,
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
            output_origin: None,
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
            inference: None,
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
                envelope_id: 9i64.into(),
                wake_error: None,
            },
        };
        assert_eq!(
            serde_json::to_value(&admitted).unwrap(),
            json!({
                "commandId":"cmd-1",
                "target":{"run":"run-1","actor":"/root/worker","incarnation":"inc-2"},
                "outcome":"admitted",
                "envelopeId":"9"
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
    fn recover_roundtrips_at_browser_boundaries(generation: u64) {
        use crate::embedding::{InferenceBlock, InferenceBlockReason, RecoveryMode};
        let operation_id = ClientOperationId(uuid::Uuid::from_u128(17));
        let target = HostActorIdentity {
            run: "run λ".into(),
            actor: crate::model::AgentPath("/root".into()),
            incarnation: "incarnation 🐈".into(),
        };
        for mode in [RecoveryMode::Exact, RecoveryMode::VisibleHistory] {
            for reason in [
                InferenceBlockReason::ContinuityUnproven,
                InferenceBlockReason::HostInterrupted,
            ] {
                // A stale generation must survive transport distinctly; admission is
                // owned by the inference gate, not by the browser decoder.
                for observed in [generation, generation.wrapping_add(1)] {
                    let command = HostCommand::Recover {
                        target: target.clone(),
                        expected_block: InferenceBlock {
                            request: crate::model::RequestId("request λ".into()),
                            generation: observed,
                            reason: reason.clone(),
                        },
                        mode,
                    };
                    let native = ClientCommand::Host {
                        operation_id,
                        command: command.clone(),
                    };
                    let durable = serde_json::to_value(&native).unwrap();
                    assert_eq!(
                        durable["command"]["expected_block"]["generation"],
                        json!(observed)
                    );
                    assert_eq!(
                        serde_json::from_value::<ClientCommand>(durable.clone()).unwrap(),
                        native
                    );
                    assert!(
                        serde_json::from_value::<contract_extensions::ClientCommand>(durable)
                            .is_err()
                    );

                    let http = contract_extensions::ClientCommand::Host {
                        operation_id,
                        command: (&command).into(),
                    };
                    let value = serde_json::to_value(&http).unwrap();
                    assert_eq!(
                        value["command"]["expected_block"]["generation"],
                        observed.to_string()
                    );
                    let decoded: ClientCommand =
                        serde_json::from_value::<contract_extensions::ClientCommand>(value)
                            .unwrap()
                            .into();
                    assert_eq!(decoded, native);

                    let ws = WsClientFrame::HostCommand {
                        operation_id,
                        command: (&command).into(),
                    };
                    let value = serde_json::to_value(&ws).unwrap();
                    assert_eq!(
                        value["command"]["expected_block"]["generation"],
                        observed.to_string()
                    );
                    let WsClientFrame::HostCommand {
                        operation_id,
                        command,
                    } = serde_json::from_value::<WsClientFrame>(value.clone()).unwrap()
                    else {
                        panic!("Recover must remain a host command");
                    };
                    assert_eq!(
                        ClientCommand::Host {
                            operation_id,
                            command: command.into()
                        },
                        native
                    );
                    let mut numeric = value;
                    numeric["command"]["expected_block"]["generation"] = json!(observed);
                    assert!(serde_json::from_value::<WsClientFrame>(numeric).is_err());
                }
            }
        }
    }

    #[test]
    fn recover_envelopes_preserve_decimal_boundaries_and_native_durable_encoding() {
        for generation in [0, 1, 9_007_199_254_740_993, u64::MAX] {
            recover_roundtrips_at_browser_boundaries(generation);
        }
    }

    proptest::proptest! {
        #[test]
        fn recover_envelopes_preserve_generated_exact_and_stale_generations(generation in proptest::prelude::any::<u64>()) {
            recover_roundtrips_at_browser_boundaries(generation);
        }
    }

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
            command: (&command).into(),
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
