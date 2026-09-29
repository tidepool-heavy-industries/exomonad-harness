//! Wire-stable WebSocket frames shared with the web client.
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub seq: u64,
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
}
