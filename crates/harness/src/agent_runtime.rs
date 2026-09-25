//! Durable model-facing agent verbs.
//!
//! Agent identity and its current request head are owned by Store's `agents`
//! table. Request rows are model-call history and are never walked to infer the
//! agent tree.
use crate::{
    agents::{AgentToolService, AgentVerbError, Contract, SpawnSource},
    item::Item,
    mailbox::{DeliveryClass, Envelope as ModelEnvelope, EnvelopeType},
    model::{AgentPath, RequestId},
    store::{Agent as StoredAgent, AgentState, Store},
};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

/// Runtime for one durable agent tree. The root must already have been
/// admitted by the owner (normally with parent/head unset).
pub struct StoreAgentToolService {
    store: Arc<Store>,
    root: AgentPath,
    child_generation: watch::Sender<u64>,
    mailbox_generation: watch::Sender<u64>,
}

impl StoreAgentToolService {
    pub fn new(store: Arc<Store>, root: AgentPath) -> Self {
        let (child_generation, _) = watch::channel(0);
        let (mailbox_generation, _) = watch::channel(0);
        Self {
            store,
            root,
            child_generation,
            mailbox_generation,
        }
    }

    /// Subscribe before enumerating agents; successful admissions advance the
    /// generation after the atomic Store transaction. On wake, enumerate the
    /// durable tree (watch values are hints, not an event log).
    pub fn subscribe_new_children(&self) -> watch::Receiver<u64> {
        self.child_generation.subscribe()
    }

    /// Subscribe before scanning Store::unread. Each committed message or task
    /// envelope advances this local generation; notifications are hints to
    /// rescan the durable mailbox, not an event log or cross-process signal.
    pub fn subscribe_mailbox_changes(&self) -> watch::Receiver<u64> {
        self.mailbox_generation.subscribe()
    }

    fn notify_mailbox_changed(&self) {
        self.mailbox_generation
            .send_modify(|generation| *generation = generation.wrapping_add(1));
    }

    async fn blocking<T, F>(&self, f: F) -> Result<T, AgentVerbError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<Store>) -> Result<T, AgentVerbError> + Send + 'static,
    {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || f(store))
            .await
            .map_err(|e| AgentVerbError(format!("store worker failed: {e}")))?
    }

    fn stored(store: &Store, path: &AgentPath) -> Result<StoredAgent, AgentVerbError> {
        if !path.is_canonical() {
            return Err(AgentVerbError("agent path is not canonical".into()));
        }
        store
            .agent(path)
            .map_err(err)?
            .ok_or_else(|| AgentVerbError(format!("agent does not exist: {}", path.0)))
    }

    fn within_root(root: &AgentPath, path: &AgentPath) -> bool {
        path == root
            || path
                .0
                .strip_prefix(&(root.0.trim_end_matches('/').to_owned() + "/"))
                .is_some_and(|suffix| !suffix.is_empty())
    }

    fn require_within_root(root: &AgentPath, path: &AgentPath) -> Result<(), AgentVerbError> {
        if !root.is_canonical() || !path.is_canonical() || !Self::within_root(root, path) {
            return Err(AgentVerbError(format!(
                "agent path is outside this service root: {}",
                path.0
            )));
        }
        Ok(())
    }

    fn direct_relation(a: &AgentPath, b: &AgentPath) -> bool {
        let child_of = |parent: &AgentPath, child: &AgentPath| {
            child
                .0
                .strip_prefix(&(parent.0.trim_end_matches('/').to_owned() + "/"))
                .is_some_and(|suffix| !suffix.is_empty() && !suffix.contains('/'))
        };
        child_of(a, b) || child_of(b, a)
    }

    fn collect_tree(
        store: &Store,
        parent: &AgentPath,
        output: &mut Vec<StoredAgent>,
        root: &AgentPath,
    ) -> Result<(), AgentVerbError> {
        let children = store.children_agents(parent).map_err(err)?;
        for child in children {
            if !Self::within_root(root, &child.path) {
                continue;
            }
            let path = child.path.clone();
            output.push(child);
            Self::collect_tree(store, &path, output, root)?;
        }
        Ok(())
    }
}

fn err(e: impl std::fmt::Display) -> AgentVerbError {
    AgentVerbError(e.to_string())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn envelope_item(envelope: &ModelEnvelope) -> Item {
    Item(json!({
        "type": "message",
        "role": "assistant",
        "content": [{
            "type": "output_text",
            "text": format!(
                "Message Type: {}\nTask name: {}\nSender: {}\nPayload:\n{}",
                envelope.kind, envelope.recipient.0, envelope.sender.0, envelope.payload
            )
        }]
    }))
}

fn stored_state(state: AgentState) -> &'static str {
    match state {
        AgentState::Active => "active",
        AgentState::Idle => "idle",
        AgentState::Completed => "completed",
        AgentState::Cancelled => "cancelled",
    }
}

fn followup_allowed(state: AgentState) -> bool {
    !matches!(state, AgentState::Completed | AgentState::Cancelled)
}

fn envelope(
    kind: EnvelopeType,
    sender: AgentPath,
    recipient: AgentPath,
    payload: String,
) -> ModelEnvelope {
    ModelEnvelope {
        kind,
        recipient,
        sender,
        payload,
        class: DeliveryClass::AtBoundary,
        timestamp_ms: now_ms(),
    }
}

fn persist_envelope(store: &Store, envelope: &ModelEnvelope) -> Result<(), AgentVerbError> {
    store
        .add_envelope(
            &envelope.sender.0,
            &envelope.recipient.0,
            "AtBoundary",
            &envelope_item(envelope),
            None,
        )
        .map_err(err)?;
    Ok(())
}

// TODO(correction-wave c/d): checkpoints are a JSON blob under `session_state`.
// PRD `checkpoint_split` nudge: one row, four uses (fork point, cache
// breakpoint, compaction boundary, tree label). Give it a table when settings
// items land, since `compact{keep_since}` and the breakpoint both key on it.
fn checkpoints_key(path: &AgentPath) -> String {
    format!("agent-checkpoints:{}", path.0)
}

fn decode_checkpoints(raw: Option<&str>) -> Result<Value, AgentVerbError> {
    let Some(raw) = raw else {
        return Ok(json!({}));
    };
    let state: Value = serde_json::from_str(raw)
        .map_err(|e| AgentVerbError(format!("corrupt checkpoint state: {e}")))?;
    if !state.is_object() {
        return Err(AgentVerbError(
            "corrupt checkpoint state: expected a JSON object".into(),
        ));
    }
    for (name, checkpoint) in state.as_object().expect("checked above") {
        checkpoint_head(checkpoint, name)?;
    }
    Ok(state)
}

fn checkpoint_head(saved: &Value, name: &str) -> Result<Option<RequestId>, AgentVerbError> {
    let Some(value) = saved.get("head_request") else {
        return Err(AgentVerbError(format!(
            "corrupt checkpoint `{name}`: missing head_request"
        )));
    };
    match value {
        Value::Null => Ok(None),
        Value::String(id) => Ok(Some(RequestId(id.clone()))),
        _ => Err(AgentVerbError(format!(
            "corrupt checkpoint `{name}`: head_request must be string or null"
        ))),
    }
}

#[async_trait::async_trait]
impl AgentToolService for StoreAgentToolService {
    async fn spawn_agent(
        &self,
        parent: &AgentPath,
        task_name: &str,
        from: SpawnSource,
        contract: Contract,
    ) -> Result<Value, AgentVerbError> {
        let name = AgentPath::normalize_task_name(task_name).map_err(err)?;
        Self::require_within_root(&self.root, parent)?;
        let path = AgentPath(format!("{}/{}", parent.0, name));
        Self::require_within_root(&self.root, &path)?;
        let parent_path = parent.clone();
        let created_path = self
            .blocking(move |store| {
                let parent_agent = Self::stored(&store, &parent_path)?;
                let (head, source) = match from {
                    SpawnSource::Prompt => (None, json!({"kind":"prompt"})),
                    // TODO(correction-wave c): a `here` fork must apply the strip
                    // list (parent's configuration_updates, annotations, dropped
                    // claims) and re-pin effort with ONE fresh update, and the
                    // child must inherit the parent's claims on pending calls
                    // (PRD `agent verbs`, acceptance 11 and 13). Today it only
                    // points the child at the parent's head request. The strip
                    // list needs no provenance lookup: every configuration_update
                    // in a stored history is harness-authored by construction
                    // (see `Store::append_items`), so strip = drop by item type.
                    SpawnSource::Here => (
                        parent_agent.head_request.clone(),
                        json!({"kind":"here","head_request":parent_agent.head_request}),
                    ),
                    SpawnSource::Checkpoint(checkpoint) => {
                        let state = store
                            .session_state(&checkpoints_key(&parent_path))
                            .map_err(err)?;
                        let checkpoints =
                            decode_checkpoints(state.as_ref().map(|s| s.state.as_str()))?;
                        let saved = checkpoints.get(&checkpoint).ok_or_else(|| {
                            AgentVerbError(format!("unknown checkpoint: {checkpoint}"))
                        })?;
                        let head = checkpoint_head(saved, &checkpoint)?;
                        (
                            head.clone(),
                            json!({
                                "kind":"checkpoint",
                                "name":checkpoint,
                                "head_request":head
                            }),
                        )
                    }
                };
                let contract_value = serde_json::to_value(&contract).map_err(err)?;
                let task_payload = serde_json::to_string(&contract_value).map_err(err)?;
                let task_envelope = envelope(
                    EnvelopeType::NewTask,
                    parent_path.clone(),
                    path.clone(),
                    task_payload,
                );
                let (admitted, _envelope_id) = store
                    .admit_agent_with_envelope(
                        &path,
                        Some(&parent_path),
                        head.as_ref(),
                        &contract_value,
                        &source,
                        &parent_path.0,
                        &path.0,
                        "AtBoundary",
                        &envelope_item(&task_envelope),
                    )
                    .map_err(err)?;
                Ok(admitted.path)
            })
            .await?;
        self.child_generation.send_modify(|generation| {
            *generation = generation.wrapping_add(1);
        });
        self.notify_mailbox_changed();
        Ok(json!({"task_name":created_path.0}))
    }

    async fn send_message(
        &self,
        sender: &AgentPath,
        target: AgentPath,
        message: String,
    ) -> Result<Value, AgentVerbError> {
        Self::require_within_root(&self.root, sender)?;
        Self::require_within_root(&self.root, &target)?;
        let sender = sender.clone();
        let result = self
            .blocking(move |store| {
                Self::stored(&store, &sender)?;
                Self::stored(&store, &target)?;
                if !Self::direct_relation(&sender, &target) {
                    return Err(AgentVerbError(
                        "messages are restricted to parent/child agents".into(),
                    ));
                }
                persist_envelope(
                    &store,
                    &envelope(EnvelopeType::Message, sender, target, message),
                )?;
                Ok(json!({"accepted":true}))
            })
            .await?;
        self.notify_mailbox_changed();
        Ok(result)
    }

    async fn followup_task(
        &self,
        sender: &AgentPath,
        target: AgentPath,
        contract: Contract,
    ) -> Result<Value, AgentVerbError> {
        Self::require_within_root(&self.root, sender)?;
        Self::require_within_root(&self.root, &target)?;
        let sender = sender.clone();
        let root = self.root.clone();
        let result = self
            .blocking(move |store| {
                Self::stored(&store, &sender)?;
                let target_agent = Self::stored(&store, &target)?;
                if target == root || target == sender || !Self::direct_relation(&sender, &target) {
                    return Err(AgentVerbError(
                        "follow-up target must be a related non-root agent".into(),
                    ));
                }
                if !followup_allowed(target_agent.state) {
                    return Err(AgentVerbError(
                        "follow-up target is completed or cancelled".into(),
                    ));
                }
                let payload = serde_json::to_string(&contract).map_err(err)?;
                persist_envelope(
                    &store,
                    &envelope(EnvelopeType::NewTask, sender, target, payload),
                )?;
                Ok(json!({
                    "status":"queued",
                    "delivery":"at_boundary",
                    "host_scheduling_needed":true
                }))
            })
            .await?;
        self.notify_mailbox_changed();
        Ok(result)
    }

    async fn wait_agent(&self, agent: &AgentPath) -> Result<Value, AgentVerbError> {
        Self::require_within_root(&self.root, agent)?;
        let agent = agent.clone();
        self.blocking(move |store| {
            Self::stored(&store, &agent)?;
            Ok(())
        })
        .await?;
        // The engine owns wait_agent because it must join job settlements and
        // its own mailbox receiver. This Store-local service cannot safely wait.
        Ok(json!({"status":"refused","reason":"engine_managed"}))
    }

    async fn checkpoint(&self, agent: &AgentPath, name: String) -> Result<Value, AgentVerbError> {
        Self::require_within_root(&self.root, agent)?;
        if name.is_empty() {
            return Err(AgentVerbError("checkpoint name must not be empty".into()));
        }
        let agent = agent.clone();
        self.blocking(move |store| {
            let stored = Self::stored(&store, &agent)?;
            let key = checkpoints_key(&agent);
            let saved = store.session_state(&key).map_err(err)?;
            let mut state = decode_checkpoints(saved.as_ref().map(|s| s.state.as_str()))?;
            let checkpoint = json!({
                "head_request":stored.head_request,
                "tokens":null,
                "warm_until":now_ms()+30*60*1000
            });
            state[&name] = checkpoint.clone();
            store.save_session_state(&key, &state).map_err(err)?;
            Ok(json!({
                "name":name,
                "request":stored.head_request,
                "tokens":null,
                "warm_until":checkpoint["warm_until"]
            }))
        })
        .await
    }

    async fn list_agents(
        &self,
        agent: &AgentPath,
        prefix: Option<AgentPath>,
    ) -> Result<Value, AgentVerbError> {
        Self::require_within_root(&self.root, agent)?;
        if let Some(path) = &prefix {
            Self::require_within_root(&self.root, path)?;
        }
        let caller = agent.clone();
        let root = self.root.clone();
        self.blocking(move |store| {
            Self::stored(&store, &caller)?;
            let prefix = prefix.map(|p| p.0);
            let mut all = vec![Self::stored(&store, &root)?];
            Self::collect_tree(&store, &root, &mut all, &root)?;
            Ok(Value::Array(
                all.into_iter()
                    .filter(|a| {
                        prefix.as_ref().is_none_or(|p| {
                            a.path.0 == *p || a.path.0.starts_with(&(p.clone() + "/"))
                        })
                    })
                    .map(|a| {
                        json!({
                            "path":a.path.0,
                            "state":stored_state(a.state)
                        })
                    })
                    .collect(),
            ))
        })
        .await
    }

    async fn interrupt_agent(
        &self,
        agent: &AgentPath,
        target: AgentPath,
    ) -> Result<Value, AgentVerbError> {
        Self::require_within_root(&self.root, agent)?;
        Self::require_within_root(&self.root, &target)?;
        let agent = agent.clone();
        self.blocking(move |store| {
            Self::stored(&store, &agent)?;
            Self::stored(&store, &target)?;
            Ok(())
        })
        .await?;
        Ok(json!({"status":"refused","reason":"not_available"}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{agents::AgentToolService, model::RequestId};

    fn contract() -> Contract {
        Contract {
            clauses: vec!["implement".into()],
            acceptance: vec!["test".into()],
            owned: vec!["src/**".into()],
            must_not: vec!["Cargo.toml".into()],
            introduces: vec![],
            consumes: vec![],
            boundaries: vec![],
        }
    }

    async fn service() -> (StoreAgentToolService, Arc<Store>, RequestId) {
        let store = Arc::new(Store::memory().unwrap());
        let root_head = RequestId("model-turn-root".into());
        store.create_request(&root_head, None, "turn").unwrap();
        store
            .admit_agent(
                &AgentPath("/root".into()),
                None,
                Some(&root_head),
                &json!({}),
                &json!({"kind":"root"}),
            )
            .unwrap();
        (
            StoreAgentToolService::new(store.clone(), AgentPath("/root".into())),
            store,
            root_head,
        )
    }

    fn add_agent(store: &Store, path: &str, parent: Option<&str>, head: Option<&RequestId>) {
        store
            .admit_agent(
                &AgentPath(path.into()),
                parent.map(|p| AgentPath(p.into())).as_ref(),
                head,
                &serde_json::to_value(contract()).unwrap(),
                &json!({"kind":"test"}),
            )
            .unwrap();
    }

    #[tokio::test]
    async fn atomic_spawn_persists_new_task_with_prompt_here_and_checkpoint_heads() {
        let (service, store, root_head) = service().await;
        let mut child_events = service.subscribe_new_children();
        let prompt = service
            .spawn_agent(
                &AgentPath("/root".into()),
                "prompt-worker",
                SpawnSource::Prompt,
                contract(),
            )
            .await
            .unwrap();
        child_events.changed().await.unwrap();
        assert_eq!(*child_events.borrow_and_update(), 1);
        assert_eq!(prompt["task_name"], "/root/prompt_worker");
        let prompt_path = AgentPath("/root/prompt_worker".into());
        let prompt_agent = store.agent(&prompt_path).unwrap().unwrap();
        assert_eq!(prompt_agent.head_request, None);
        assert_eq!(prompt_agent.fork_source["kind"], "prompt");
        let prompt_inbox = store.unread(&prompt_path.0).unwrap();
        assert_eq!(prompt_inbox.len(), 1);
        assert_eq!(prompt_inbox[0].class, "AtBoundary");
        let task_item = store.get_item(&prompt_inbox[0].item_hash).unwrap().unwrap();
        assert_eq!(task_item.0["role"], "assistant");
        assert!(
            task_item.0["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("NEW_TASK")
        );

        service
            .spawn_agent(
                &AgentPath("/root".into()),
                "here-worker",
                SpawnSource::Here,
                contract(),
            )
            .await
            .unwrap();
        let here = store
            .agent(&AgentPath("/root/here_worker".into()))
            .unwrap()
            .unwrap();
        assert_eq!(here.head_request, Some(root_head));
        assert_eq!(here.fork_source["kind"], "here");

        service
            .checkpoint(&AgentPath("/root".into()), "stable".into())
            .await
            .unwrap();
        service
            .spawn_agent(
                &AgentPath("/root".into()),
                "checkpoint-worker",
                SpawnSource::Checkpoint("stable".into()),
                contract(),
            )
            .await
            .unwrap();
        let checkpoint = store
            .agent(&AgentPath("/root/checkpoint_worker".into()))
            .unwrap()
            .unwrap();
        assert_eq!(checkpoint.head_request, here.head_request);
        assert_eq!(checkpoint.fork_source["kind"], "checkpoint");
        assert_eq!(store.unread("/root/checkpoint_worker").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_is_root_scoped_and_does_not_expose_contract_or_head() {
        let (root_service, store, root_head) = service().await;
        add_agent(&store, "/root/lead", Some("/root"), Some(&root_head));
        add_agent(
            &store,
            "/root/lead/worker",
            Some("/root/lead"),
            Some(&root_head),
        );
        add_agent(&store, "/root/sibling", Some("/root"), Some(&root_head));
        let lead_service =
            StoreAgentToolService::new(store.clone(), AgentPath("/root/lead".into()));
        let saved = root_service
            .checkpoint(&AgentPath("/root/lead".into()), "stable".into())
            .await
            .unwrap();
        assert_eq!(saved["request"], root_head.0);
        let listed = lead_service
            .list_agents(
                &AgentPath("/root/lead".into()),
                Some(AgentPath("/root/lead".into())),
            )
            .await
            .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 2);
        for agent in listed.as_array().unwrap() {
            assert!(agent.get("contract").is_none());
            assert!(agent.get("head_request").is_none());
            assert!(agent.get("parent").is_none());
            assert!(!agent["path"].as_str().unwrap().contains("sibling"));
        }
        assert!(
            root_service
                .list_agents(
                    &AgentPath("/root".into()),
                    Some(AgentPath("/root/sibling".into()))
                )
                .await
                .is_ok()
        );
        assert!(
            lead_service
                .list_agents(
                    &AgentPath("/root/lead".into()),
                    Some(AgentPath("/root/sibling".into()))
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn every_agent_path_argument_is_contained_by_service_root() {
        let (service, store, root_head) = service().await;
        add_agent(&store, "/root/lead", Some("/root"), Some(&root_head));
        add_agent(
            &store,
            "/root/lead/worker",
            Some("/root/lead"),
            Some(&root_head),
        );
        add_agent(&store, "/root/sibling", Some("/root"), Some(&root_head));
        let scoped = StoreAgentToolService::new(store, AgentPath("/root/lead".into()));
        let inside = AgentPath("/root/lead".into());
        let outside = AgentPath("/root/sibling".into());
        assert!(
            scoped
                .send_message(&outside, inside.clone(), "x".into())
                .await
                .is_err()
        );
        assert!(
            scoped
                .send_message(&inside, outside.clone(), "x".into())
                .await
                .is_err()
        );
        assert!(
            scoped
                .followup_task(&inside, outside.clone(), contract())
                .await
                .is_err()
        );
        assert!(scoped.list_agents(&outside, None).await.is_err());
        assert!(scoped.checkpoint(&outside, "bad".into()).await.is_err());
        assert!(scoped.wait_agent(&outside).await.is_err());
        assert!(
            scoped
                .spawn_agent(&outside, "child", SpawnSource::Prompt, contract())
                .await
                .is_err()
        );
        assert!(scoped.interrupt_agent(&inside, outside).await.is_err());
        assert_eq!(
            service
                .interrupt_agent(&AgentPath("/root".into()), AgentPath("/root".into()))
                .await
                .unwrap(),
            json!({"status":"refused","reason":"not_available"})
        );
    }

    #[tokio::test]
    async fn wait_is_explicitly_engine_managed() {
        let (service, _, _) = service().await;
        assert_eq!(
            service
                .wait_agent(&AgentPath("/root".into()))
                .await
                .unwrap(),
            json!({"status":"refused","reason":"engine_managed"})
        );
    }

    #[tokio::test]
    async fn dispatched_message_and_followup_are_persisted_in_arrival_order() {
        let (service, store, _) = service().await;
        let mut mailbox_changes = service.subscribe_mailbox_changes();
        let root = AgentPath("/root".into());
        let child = AgentPath("/root/worker".into());
        add_agent(&store, &child.0, Some(&root.0), None);
        add_agent(&store, "/root/worker/grandchild", Some(&child.0), None);

        service
            .spawn_agent(&root, "spawned", SpawnSource::Prompt, contract())
            .await
            .unwrap();
        mailbox_changes.changed().await.unwrap();
        assert_eq!(*mailbox_changes.borrow_and_update(), 1);

        assert!(
            service
                .send_message(
                    &root,
                    AgentPath("/root/worker/grandchild".into()),
                    "not a direct relation".into(),
                )
                .await
                .is_err()
        );
        assert!(!mailbox_changes.has_changed().unwrap());

        crate::agents::dispatch_agent_verb(
            &service,
            &root,
            "send_message",
            json!({"target":child.0,"message":"status please"}),
        )
        .await
        .unwrap();
        mailbox_changes.changed().await.unwrap();
        assert_eq!(*mailbox_changes.borrow_and_update(), 2);
        let followup = crate::agents::dispatch_agent_verb(
            &service,
            &root,
            "followup_task",
            json!({"target":child.0,"task":contract()}),
        )
        .await
        .unwrap();
        assert_eq!(followup["status"], "queued");
        assert_eq!(followup["delivery"], "at_boundary");
        assert_eq!(followup["host_scheduling_needed"], true);
        mailbox_changes.changed().await.unwrap();
        assert_eq!(*mailbox_changes.borrow_and_update(), 3);

        let mailbox = store.unread(&child.0).unwrap();
        assert_eq!(mailbox.len(), 2, "MESSAGE then follow-up");
        let texts: Vec<String> = mailbox
            .iter()
            .map(|record| {
                store.get_item(&record.item_hash).unwrap().unwrap().0["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert!(texts[0].contains("MESSAGE"));
        assert!(texts[0].contains("status please"));
        assert!(texts[1].contains("NEW_TASK"));
        assert!(texts[1].contains("acceptance"));
        for record in mailbox {
            assert_eq!(
                store.get_item(&record.item_hash).unwrap().unwrap().0["role"],
                "assistant"
            );
        }
    }

    #[test]
    fn corrupt_checkpoint_state_and_terminal_followups_are_rejected() {
        assert!(decode_checkpoints(Some("{")).is_err());
        assert!(decode_checkpoints(Some("[]")).is_err());
        assert!(decode_checkpoints(Some("{}")).is_ok());
        assert!(decode_checkpoints(Some(r#"{"saved":{}}"#)).is_err());
        assert!(followup_allowed(AgentState::Active));
        assert!(followup_allowed(AgentState::Idle));
        assert!(!followup_allowed(AgentState::Completed));
        assert!(!followup_allowed(AgentState::Cancelled));
        assert!(
            checkpoint_head(&json!({"head_request":null}), "fresh")
                .unwrap()
                .is_none()
        );
        assert!(checkpoint_head(&json!({}), "missing").is_err());
    }
}
