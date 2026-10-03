//! Bounded reconnect state for live output on the existing ordered event stream.
use super::{ServerControl, ServerEvent};
use crate::transport::sse::OutputChannel;
use crate::{
    engine::{ModelOutput, ModelOutputObserver, ModelOutputUpdate},
    model::{ConversationIdentity, RequestId},
};
use serde::{Deserialize, Serialize};

pub const OUTPUT_ITEMS: usize = 128;
pub const OUTPUT_BYTES: usize = 64 * 1024;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveOutput {
    pub origin: ConversationIdentity,
    pub request_id: RequestId,
    pub item_id: String,
    pub channel: OutputChannel,
    pub index: u64,
    pub text: String,
    pub version: u64,
    pub overflow: bool,
    pub streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_hash: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRevision {
    pub origin: ConversationIdentity,
    pub request_id: RequestId,
    pub version: u64,
}
impl ModelOutputObserver for ServerControl {
    fn observe(&self, output: ModelOutput) {
        let mut next = self.next_sequence.lock().expect("sequence lock poisoned");
        let mut snapshot = self.snapshot.write().expect("snapshot lock poisoned");
        let mut removals = Vec::new();
        let mut payload = serde_json::to_value(&output).expect("model output serializes");
        let kind = match &output.update {
            ModelOutputUpdate::Stopped => {
                for item in &mut snapshot.live_output {
                    if item.origin == output.origin && item.request_id == output.request_id {
                        item.streaming = false;
                    }
                }
                "model.output.stopped"
            }
            ModelOutputUpdate::Started => {
                if let ConversationIdentity::Embedded {
                    run,
                    actor,
                    incarnation,
                } = &output.origin
                {
                    if let Some(projected) = snapshot.actors.iter_mut().find(|a| {
                        a.identity.run == *run
                            && a.identity.actor == *actor
                            && a.identity.incarnation == *incarnation
                    }) {
                        projected.model_head_request = Some(output.request_id.0.clone());
                    }
                }
                "model.output.started"
            }
            ModelOutputUpdate::Delta {
                item_id,
                channel,
                index,
                text,
            } => {
                let existing = snapshot.live_output.iter().position(|item| {
                    item.origin == output.origin
                        && item.request_id == output.request_id
                        && item.item_id == *item_id
                        && item.channel == *channel
                        && item.index == *index
                });
                let item = if let Some(index) = existing {
                    &mut snapshot.live_output[index]
                } else {
                    let capacity_overflow = snapshot.live_output.len() == OUTPUT_ITEMS;
                    if capacity_overflow {
                        let old = snapshot.live_output.remove(0);
                        removals.push(
                            serde_json::json!({"origin":old.origin,"requestId":old.request_id,
                            "itemId":old.item_id,"channel":old.channel,"index":old.index}),
                        );
                    }
                    snapshot.live_output.push(LiveOutput {
                        origin: output.origin.clone(),
                        request_id: output.request_id.clone(),
                        item_id: item_id.clone(),
                        channel: channel.clone(),
                        index: *index,
                        text: String::new(),
                        version: *next,
                        overflow: capacity_overflow,
                        streaming: true,
                        committed_hash: None,
                    });
                    snapshot.live_output.last_mut().unwrap()
                };
                let mut end = text.len().min(OUTPUT_BYTES.saturating_sub(item.text.len()));
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                item.text.push_str(&text[..end]);
                item.version = *next + removals.len() as u64;
                item.overflow |= end != text.len();
                payload["overflow"] = serde_json::json!(item.overflow);
                payload["version"] = serde_json::json!(item.version);
                "model.output.delta"
            }
            ModelOutputUpdate::Committed { item_id, hash } => {
                for item in &mut snapshot.live_output {
                    if item.origin == output.origin
                        && item.request_id == output.request_id
                        && Some(&item.item_id) == item_id.as_ref()
                    {
                        item.committed_hash = Some(hash.0.clone());
                        item.streaming = false;
                    }
                }
                snapshot
                    .history_revisions
                    .retain(|r| r.origin != output.origin || r.request_id != output.request_id);
                if snapshot.history_revisions.len() == OUTPUT_ITEMS {
                    snapshot.history_revisions.remove(0);
                }
                // The revision is the event's sequence, including preceding removals.
                let version = *next + removals.len() as u64;
                snapshot.history_revisions.push(HistoryRevision {
                    origin: output.origin.clone(),
                    request_id: output.request_id.clone(),
                    version,
                });
                payload["version"] = serde_json::json!(version);
                "model.output.committed"
            }
        };
        for removal in removals {
            let sequence = *next;
            *next += 1;
            let _ = self.events.send(ServerEvent {
                sequence,
                event: "model.output.remove".into(),
                payload: removal,
            });
        }
        let sequence = *next;
        *next += 1;
        snapshot.seq = sequence;
        let _ = self.events.send(ServerEvent {
            sequence,
            event: kind.into(),
            payload,
        });
    }
}

impl ServerControl {
    /// Project a committed journal row through the existing ordered browser stream.
    /// The bounded snapshot retains references; full bodies remain in Store history.
    pub fn publish_actor_output(&self, output: &crate::store::actor_output::StoredActorOutput) {
        let reference = output.reference();
        let mut next = self.next_sequence.lock().expect("sequence lock poisoned");
        let mut snapshot = self.snapshot.write().expect("snapshot lock poisoned");
        if snapshot.actor_output_revisions.contains(reference) { return; }
        snapshot.actor_output_revisions.push(reference.clone());
        while snapshot.actor_output_revisions.len() > OUTPUT_ITEMS
            || serde_json::to_vec(&snapshot.actor_output_revisions).expect("output references serialize").len() > OUTPUT_BYTES
        {
            snapshot.actor_output_revisions.remove(0);
        }
        let sequence = *next;
        *next += 1;
        snapshot.seq = sequence;
        let _ = self.events.send(ServerEvent {
            sequence, event: "actor.output.committed".into(),
            payload: serde_json::to_value(output).expect("committed output serializes"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        item::ItemHash,
        model::AgentPath,
        server::{
            HostActorIdentity, HostActorKind, HostActorLifecycle, HostActorProjection,
            ServerConfig, Snapshot, server_with_config,
        },
    };
    use std::path::PathBuf;
    fn origin(incarnation: &str) -> ConversationIdentity {
        ConversationIdentity::Embedded {
            run: "run".into(),
            actor: AgentPath("/root".into()),
            incarnation: incarnation.into(),
        }
    }
    fn delta(incarnation: &str, item_id: &str, text: &str) -> ModelOutput {
        ModelOutput {
            origin: origin(incarnation),
            request_id: RequestId("pending".into()),
            update: ModelOutputUpdate::Delta {
                item_id: item_id.into(),
                channel: OutputChannel::Assistant,
                index: 0,
                text: text.into(),
            },
        }
    }
    #[test]
    fn live_output_snapshot_preserves_identity_commit_and_reconnect_watermark() {
        let (_, control, _) = server_with_config(ServerConfig::new(PathBuf::new()));
        control.update_host_projection(
            "run".into(),
            vec![HostActorProjection {
                identity: HostActorIdentity {
                    run: "run".into(),
                    actor: AgentPath("/root".into()),
                    incarnation: "first".into(),
                },
                parent: None,
                kind: HostActorKind::Model,
                lifecycle: HostActorLifecycle::Running,
                model_conversation: Some("/root".into()),
                model_head_request: Some("settled".into()),
                active_round: None,
            }],
            vec![],
        );
        let mut events = control.events.subscribe();
        control.observe(ModelOutput {
            origin: origin("wrong"),
            request_id: RequestId("wrong".into()),
            update: ModelOutputUpdate::Started,
        });
        assert_eq!(
            control.snapshot.read().unwrap().actors[0]
                .model_head_request
                .as_deref(),
            Some("settled")
        );
        control.observe(ModelOutput {
            origin: origin("first"),
            request_id: RequestId("pending".into()),
            update: ModelOutputUpdate::Started,
        });
        control.observe(delta("first", "assistant", "Hello "));
        control.observe(delta("first", "assistant", "world"));
        control.observe(delta("second", "assistant", "other incarnation"));
        control.observe(ModelOutput {
            origin: origin("first"),
            request_id: RequestId("pending".into()),
            update: ModelOutputUpdate::Committed {
                item_id: Some("assistant".into()),
                hash: ItemHash("durable-hash".into()),
            },
        });
        let snapshot = control.snapshot.read().unwrap().clone();
        assert_eq!(
            snapshot.actors[0].model_head_request.as_deref(),
            Some("pending")
        );
        assert_eq!(snapshot.live_output.len(), 2);
        assert_eq!(snapshot.live_output[0].text, "Hello world");
        assert_eq!(
            snapshot.live_output[0].committed_hash.as_deref(),
            Some("durable-hash")
        );
        assert_eq!(snapshot.live_output[1].committed_hash, None);
        assert_eq!(snapshot.history_revisions[0].version, snapshot.seq);
        let mut last = None;
        while let Ok(event) = events.try_recv() {
            last = Some(event);
        }
        assert_eq!(last.unwrap().sequence, snapshot.seq);
        // Host projection replacement cannot erase current provider output.
        control.set_snapshot(Snapshot {
            actors: snapshot.actors.clone(),
            ..Snapshot::default()
        });
        assert_eq!(
            control.snapshot.read().unwrap().live_output,
            snapshot.live_output
        );
    }
    #[test]
    fn live_output_interleaved_eviction_marks_missing_prefix_and_stop() {
        let (_, control, _) = server_with_config(ServerConfig::new(PathBuf::new()));
        control.observe(delta("first", "A", "prefix "));
        for item in 0..OUTPUT_ITEMS {
            control.observe(delta("first", &format!("item-{item}"), "x"));
        }
        control.observe(delta("first", "A", "suffix"));
        let snapshot = control.snapshot.read().unwrap().clone();
        let restored = snapshot
            .live_output
            .iter()
            .find(|item| item.item_id == "A")
            .unwrap();
        assert_eq!(restored.text, "suffix");
        assert!(restored.overflow);
        control.observe(ModelOutput {
            origin: origin("first"),
            request_id: RequestId("pending".into()),
            update: ModelOutputUpdate::Stopped,
        });
        assert!(
            control
                .snapshot
                .read()
                .unwrap()
                .live_output
                .iter()
                .all(|item| !item.streaming)
        );
    }
    #[test]
    fn live_output_snapshot_bounds_each_item_and_reports_evictions() {
        let (_, control, _) = server_with_config(ServerConfig::new(PathBuf::new()));
        let mut events = control.events.subscribe();
        control.observe(delta("first", "large", &"λ".repeat(OUTPUT_BYTES)));
        let snapshot = control.snapshot.read().unwrap().clone();
        assert_eq!(snapshot.live_output[0].text.len(), OUTPUT_BYTES);
        assert!(snapshot.live_output[0].overflow);
        for item in 0..OUTPUT_ITEMS {
            control.observe(delta("first", &format!("item-{item}"), "x"));
        }
        let snapshot = control.snapshot.read().unwrap().clone();
        assert_eq!(snapshot.live_output.len(), OUTPUT_ITEMS);
        assert!(
            snapshot
                .live_output
                .iter()
                .all(|item| item.item_id != "large")
        );
        let mut saw_remove = false;
        let mut last = 0;
        while let Ok(event) = events.try_recv() {
            saw_remove |= event.event == "model.output.remove";
            assert!(event.sequence > last);
            last = event.sequence;
        }
        assert!(saw_remove);
        assert_eq!(last, snapshot.seq);
    }
}
