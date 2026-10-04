//! Bounded reconnect state for live output on the existing ordered event stream.
use super::{ServerControl, ServerEvent, browser_contract};
use crate::{
    engine::{ModelOutput, ModelOutputObserver, ModelOutputUpdate},
    model::ConversationIdentity,
};
pub use browser_contract::{HistoryRevision, LiveOutput};
use browser_contract::{OutputCommit, OutputDelta, OutputItem, OutputScope, StateEvent};

pub const OUTPUT_ITEMS: usize = 128;
pub const OUTPUT_BYTES: usize = 64 * 1024;
impl ModelOutputObserver for ServerControl {
    fn observe(&self, output: ModelOutput) {
        let mut next = self.next_sequence.lock().expect("sequence lock poisoned");
        let mut snapshot = self.snapshot.write().expect("snapshot lock poisoned");
        let mut removals = Vec::new();
        let scope = OutputScope {
            origin: output.origin.clone(),
            request_id: output.request_id.0.clone(),
        };
        let event = match &output.update {
            ModelOutputUpdate::Stopped => {
                for item in &mut snapshot.live_output {
                    if item.origin == output.origin && item.request_id == output.request_id.0 {
                        item.streaming = false;
                    }
                }
                StateEvent::ModelOutputStopped(scope)
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
                StateEvent::ModelOutputStarted(scope)
            }
            ModelOutputUpdate::Delta {
                item_id,
                channel,
                index,
                text,
            } => {
                let existing = snapshot.live_output.iter().position(|item| {
                    item.origin == output.origin
                        && item.request_id == output.request_id.0
                        && item.item_id == *item_id
                        && item.channel == *channel
                        && item.index.get() == *index
                });
                let item = if let Some(index) = existing {
                    &mut snapshot.live_output[index]
                } else {
                    let capacity_overflow = snapshot.live_output.len() == OUTPUT_ITEMS;
                    if capacity_overflow {
                        let old = snapshot.live_output.remove(0);
                        removals.push(StateEvent::ModelOutputRemove(OutputItem {
                            origin: old.origin,
                            request_id: old.request_id,
                            item_id: old.item_id,
                            channel: old.channel,
                            index: old.index,
                        }));
                    }
                    snapshot.live_output.push(LiveOutput {
                        origin: output.origin.clone(),
                        request_id: output.request_id.0.clone(),
                        item_id: item_id.clone(),
                        channel: *channel,
                        index: (*index).into(),
                        text: String::new(),
                        version: (*next).into(),
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
                item.version = (*next + removals.len() as u64).into();
                item.overflow |= end != text.len();
                StateEvent::ModelOutputDelta(OutputDelta {
                    origin: output.origin.clone(),
                    request_id: output.request_id.0.clone(),
                    item_id: item_id.clone(),
                    channel: *channel,
                    index: (*index).into(),
                    text: text.clone(),
                    version: item.version,
                    overflow: item.overflow,
                })
            }
            ModelOutputUpdate::Committed { item_id, hash } => {
                for item in &mut snapshot.live_output {
                    if item.origin == output.origin
                        && item.request_id == output.request_id.0
                        && Some(&item.item_id) == item_id.as_ref()
                    {
                        item.committed_hash = Some(hash.0.clone());
                        item.streaming = false;
                    }
                }
                snapshot
                    .history_revisions
                    .retain(|r| r.origin != output.origin || r.request_id != output.request_id.0);
                if snapshot.history_revisions.len() == OUTPUT_ITEMS {
                    snapshot.history_revisions.remove(0);
                }
                let version = (*next + removals.len() as u64).into();
                snapshot.history_revisions.push(HistoryRevision {
                    origin: output.origin.clone(),
                    request_id: output.request_id.0.clone(),
                    version,
                });
                StateEvent::ModelOutputCommitted(OutputCommit {
                    origin: output.origin.clone(),
                    request_id: output.request_id.0.clone(),
                    item_id: item_id.clone(),
                    hash: hash.0.clone(),
                    version,
                })
            }
        };
        for event in removals {
            let sequence = *next;
            *next += 1;
            let _ = self.events.send(ServerEvent {
                sequence: sequence.into(),
                event,
            });
        }
        let sequence = *next;
        *next += 1;
        snapshot.seq = sequence.into();
        let _ = self.events.send(ServerEvent {
            sequence: sequence.into(),
            event,
        });
    }
}

impl ServerControl {
    /// The exact committed projection is shared with protected history reads.
    pub fn publish_actor_output(&self, output: &crate::store::actor_output::StoredActorOutput) {
        let output = browser_contract::ActorOutputProjection::from(output);
        let reference = &output.reference;
        let mut next = self.next_sequence.lock().expect("sequence lock poisoned");
        let mut snapshot = self.snapshot.write().expect("snapshot lock poisoned");
        if snapshot.actor_output_revisions.contains(reference) {
            return;
        }
        snapshot.actor_output_revisions.push(reference.clone());
        while snapshot.actor_output_revisions.len() > OUTPUT_ITEMS
            || serde_json::to_vec(&snapshot.actor_output_revisions)
                .expect("output references serialize")
                .len()
                > OUTPUT_BYTES
        {
            snapshot.actor_output_revisions.remove(0);
        }
        let sequence = *next;
        *next += 1;
        snapshot.seq = sequence.into();
        let _ = self.events.send(ServerEvent {
            sequence: sequence.into(),
            event: StateEvent::ActorOutputCommitted(output),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        item::ItemHash,
        model::AgentPath,
        model::RequestId,
        server::{
            HostActorIdentity, HostActorKind, HostActorLifecycle, HostActorProjection,
            ServerConfig, Snapshot, WireU64, server_with_config,
        },
        transport::sse::OutputChannel,
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
                output_origin: None,
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
        let mut last = WireU64::default();
        while let Ok(event) = events.try_recv() {
            saw_remove |= matches!(event.event, StateEvent::ModelOutputRemove(_));
            assert!(event.sequence > last);
            last = event.sequence;
        }
        assert!(saw_remove);
        assert_eq!(last, snapshot.seq);
    }
}
