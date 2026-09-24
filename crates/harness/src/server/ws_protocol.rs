//! Wire-stable WebSocket frames shared with the web client.
use serde::{Deserialize, Serialize};

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
