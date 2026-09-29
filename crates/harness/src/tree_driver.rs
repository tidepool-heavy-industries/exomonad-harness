//! Reusable Store-backed agent-tree lifecycle supervisor.

use crate::{
    agent_runtime::StoreAgentToolService,
    engine::EngineCompletion,
    finalize::FinalizeParser,
    item::Item,
    lifecycle::{CompletionCommit, PublishedAnswer},
    mailbox::{DeliveryClass, Envelope, EnvelopeType},
    model::{AgentPath, RequestId},
    store::{AgentState, Store},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Mutex, mpsc, watch};

/// Creates and runs one engine instance for each active Store agent.
#[async_trait]
pub trait EngineFactory: Send + Sync + 'static {
    type Engine: Send + Sync + 'static;
    async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String>;
    async fn run(
        &self,
        engine: &Self::Engine,
        head: Option<RequestId>,
        initial: Vec<Item>,
        cancel: watch::Receiver<bool>,
        inbox: mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<EngineCompletion, String>;
}

struct Running {
    cancel: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    outcome: Arc<Mutex<Option<Result<EngineCompletion, String>>>>,
    notifier: tokio::task::JoinHandle<()>,
    inbox: mpsc::UnboundedSender<Envelope>,
    seen_inbox_ids: Arc<std::sync::Mutex<HashSet<i64>>>,
}

// TODO(correction-wave): 1.3k lines is not the "small driver" of PRD
// `library vs driver`. Whatever here is agent lifecycle, inbox rescan, or
// wake routing belongs in the library (one future per agent over the store);
// the driver should only spawn those futures, restart from inbox, and serve.
// Shrinks naturally once the engine has one entry point.
/// The runtime is started after constructing the StoreAgentToolService used by
/// the TreeProvider. Call `shutdown` to cancel and join every admitted task.
pub struct TreeDriver<F: EngineFactory> {
    store: Arc<Store>,
    service: Arc<StoreAgentToolService>,
    factory: Arc<F>,
    running: Arc<Mutex<HashMap<AgentPath, Running>>>,
    wake: watch::Sender<u64>,
    stop: watch::Sender<bool>,
    supervisor: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    failures: watch::Sender<Option<String>>,
    root_completion: watch::Sender<Option<Result<EngineCompletion, String>>>,
    completed: Arc<Mutex<HashSet<AgentPath>>>,
    task_generation: watch::Sender<u64>,
    started: Arc<AtomicBool>,
}

impl<F: EngineFactory> TreeDriver<F> {
    pub fn new(store: Arc<Store>, service: Arc<StoreAgentToolService>, factory: Arc<F>) -> Self {
        let (wake, _) = watch::channel(0);
        let (stop, _) = watch::channel(false);
        let (failures, _) = watch::channel(None);
        let (root_completion, _) = watch::channel(None);
        let (task_generation, _) = watch::channel(0);
        Self {
            store,
            service,
            factory,
            running: Arc::new(Mutex::new(HashMap::new())),
            wake,
            stop,
            supervisor: Arc::new(Mutex::new(None)),
            failures,
            root_completion,
            completed: Arc::new(Mutex::new(HashSet::new())),
            task_generation,
            started: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Admit `/root` if absent, subscribe before scanning durable state, then
    /// start every active agent and supervise later prompt admissions.
    pub async fn start(&self, root_input: Vec<Item>) -> Result<(), String> {
        self.started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "driver.start may only be called once".to_owned())?;
        let root = AgentPath("/root".into());
        if self
            .store
            .agent(&root)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            self.store
                .admit_agent(&root, None, None, &json!({}), &json!({"kind":"root"}))
                .map_err(|e| e.to_string())?;
        }
        let mut children = self.service.subscribe_new_children();
        let mut mailbox = self.service.subscribe_mailbox_changes();
        let mut task_changes = self.task_generation.subscribe();
        // Subscribe-before-scan closes the race with a concurrent admission.
        self.scan(root_input).await?;
        let this = self.clone_runtime();
        let mut stop = self.stop.subscribe();
        let supervisor = tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = stop.changed() => if changed.is_err() || *stop.borrow() { break },
                    changed = children.changed() => if changed.is_err() { break },
                    changed = mailbox.changed() => if changed.is_err() { break },
                    changed = task_changes.changed() => if changed.is_err() { break },
                }
                if let Err(error) = this.scan(Vec::new()).await {
                    this.failures.send_replace(Some(error));
                    break;
                }
            }
        });
        *self.supervisor.lock().await = Some(supervisor);
        Ok(())
    }

    fn clone_runtime(&self) -> Self {
        Self {
            store: self.store.clone(),
            service: self.service.clone(),
            factory: self.factory.clone(),
            running: self.running.clone(),
            wake: self.wake.clone(),
            stop: self.stop.clone(),
            supervisor: self.supervisor.clone(),
            failures: self.failures.clone(),
            root_completion: self.root_completion.clone(),
            completed: self.completed.clone(),
            task_generation: self.task_generation.clone(),
            started: self.started.clone(),
        }
    }

    async fn scan(&self, root_input: Vec<Item>) -> Result<(), String> {
        self.reap_finished().await?;
        let agents = self.store.list_agents().map_err(|e| e.to_string())?;
        for agent in agents {
            if agent.state == AgentState::Active {
                let path = agent.path.clone();
                let unread = self.store.unread(&path.0).map_err(|e| e.to_string())?;
                if agent.head_request.is_some() && unread.is_empty() {
                    // A stored active agent with a head but no pending inbox is
                    // not safely resumable here; fail closed rather than replay.
                    continue;
                }
                let is_completed = self.completed.lock().await.contains(&path);
                if is_completed && unread.is_empty() {
                    continue;
                }
                if is_completed {
                    self.completed.lock().await.remove(&path);
                }
                self.start_agent(
                    path.clone(),
                    initial_for_agent(&path, agent.head_request.is_some(), &root_input),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn reap_finished(&self) -> Result<(), String> {
        let finished = {
            let mut running = self.running.lock().await;
            let paths = running
                .iter()
                .filter_map(|(path, item)| {
                    item.outcome
                        .try_lock()
                        .is_ok_and(|outcome| outcome.is_some())
                        .then_some(path.clone())
                })
                .collect::<Vec<_>>();
            paths
                .into_iter()
                .filter_map(|path| running.remove(&path).map(|item| (path, item)))
                .collect::<Vec<_>>()
        };
        let mut failures = Vec::new();
        for (path, running) in finished {
            let was_cancelled = *running.cancel.borrow();
            let outcome = running.outcome.lock().await.take();
            if let Err(error) = running.task.await {
                failures.push(format!(
                    "agent {} completion watcher failed: {error}",
                    path.0
                ));
            }
            if let Err(error) = running.notifier.await {
                failures.push(format!("agent {} notifier join failed: {error}", path.0));
            }
            match outcome {
                Some(Ok(_)) => {
                    self.completed.lock().await.insert(path);
                }
                Some(Err(error)) if expected_cancellation(&error, was_cancelled) => {}
                Some(Err(error)) => {
                    let message = format!("agent {} failed: {error}", path.0);
                    self.failures.send_replace(Some(message.clone()));
                    failures.push(message);
                }
                None => {
                    let message = format!("agent {} completed without an outcome", path.0);
                    self.failures.send_replace(Some(message.clone()));
                    failures.push(message);
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    pub fn failure_receiver(&self) -> watch::Receiver<Option<String>> {
        self.failures.subscribe()
    }

    /// Number of agent tasks currently owned by this supervisor.
    pub async fn active_agent_count(&self) -> usize {
        self.running.lock().await.len()
    }

    async fn start_agent(&self, path: AgentPath, initial: Vec<Item>) -> Result<(), String> {
        let mut running = self.running.lock().await;
        if running.contains_key(&path) {
            return Ok(());
        }
        let record = self
            .store
            .agent(&path)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("agent disappeared: {}", path.0))?;
        let engine = self.factory.create(path.clone()).await?;
        let (cancel, cancel_rx) = watch::channel(false);
        let (inbox_tx, inbox_rx) = mpsc::unbounded_channel();
        let (done_tx, mut done_rx) = watch::channel(false);
        // Subscribe before scanning; a concurrent delivery is either found
        // here or represented by a pending generation change.
        let mut changes = self.service.subscribe_mailbox_changes();
        let mut observed = HashSet::new();
        scan_inbox(&self.store, &path, &inbox_tx, &mut observed, false)?;
        let seen_inbox_ids = Arc::new(std::sync::Mutex::new(observed.clone()));
        let store = self.store.clone();
        let wake = self.wake.clone();
        let task_path = path.clone();
        let task_factory = self.factory.clone();
        let running_tasks = self.running.clone();
        let engine_task = tokio::spawn(async move {
            // A durable scan supplies wake hints; Engine claims inbox entries
            // transactionally when it creates the next request.
            let result = async {
                let completion = task_factory
                    .run(
                        &engine,
                        record.head_request.clone(),
                        initial,
                        cancel_rx,
                        inbox_rx,
                    )
                    .await?;
                let completed_head = completion.head_request.clone();
                // Prepare the typed answer and its provenance before changing
                // the durable head. A failed preparation must not consume a
                // completion that the parent can never receive.
                let publication = match record.parent.as_ref() {
                    Some(parent) => {
                        published_final_answer(&store, &task_path, parent, &completion)?
                    }
                    None => None,
                };
                let publication_target = record.parent.as_ref().zip(publication.as_ref());
                let committed = store
                    .complete_agent_with_publication(
                        &task_path,
                        record.head_request.as_ref(),
                        &completed_head,
                        publication_target,
                    )
                    .map_err(|error| error.to_string())?;
                let envelope_id = match committed {
                    CompletionCommit::HeadMismatch => {
                        return Err(format!("head CAS lost for {}", task_path.0));
                    }
                    CompletionCommit::Committed { envelope_id } => envelope_id,
                };
                if let (Some(parent), Some(envelope_id)) = (record.parent, envelope_id) {
                    let parent_running = running_tasks.lock().await;
                    let parent_task = parent_running.get(&parent);
                    let mut seen = parent_task.map(|task| {
                        task.seen_inbox_ids
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner())
                    });
                    if let Some(parent_task) = parent_task {
                        seen.as_mut()
                            .expect("parent seen set acquired")
                            .insert(envelope_id);
                        // One local post-commit wake; durable payload remains in Store.
                        wake.send_modify(|n| *n = n.wrapping_add(1));
                        let _ = parent_task.inbox.send(Envelope {
                            kind: EnvelopeType::Message,
                            recipient: parent,
                            sender: task_path,
                            payload: String::new(),
                            class: DeliveryClass::AtBoundary,
                            timestamp_ms: 0,
                        });
                    }
                }
                Ok::<EngineCompletion, String>(completion)
            }
            .await;
            let _ = done_tx.send(true);
            result
        });
        let outcome = Arc::new(Mutex::new(None));
        let monitor_outcome = outcome.clone();
        let task_generation = self.task_generation.clone();
        let root_completion = self.root_completion.clone();
        let is_root = path.0 == "/root";
        let task = tokio::spawn(async move {
            let result = match engine_task.await {
                Ok(result) => result,
                Err(error) => Err(format!("engine task panicked: {error}")),
            };
            if is_root {
                root_completion.send_replace(Some(result.clone()));
            }
            *monitor_outcome.lock().await = Some(result);
            // Publish only after the Engine task has resolved and its outcome
            // is visible to reaping; a wake cannot race task termination.
            task_generation.send_modify(|n| *n = n.wrapping_add(1));
        });
        // Follow-up scans only emit a hint for newly observed unread IDs.
        let inbox_store = self.store.clone();
        let inbox_path = path.clone();
        let inbox_notifications = inbox_tx.clone();
        let notifier_seen = seen_inbox_ids.clone();
        let notifier_cancel_tx = cancel.clone();
        let notifier_cancel_rx = cancel.subscribe();
        let failure_sender = self.failures.clone();
        let notifier = tokio::spawn(async move {
            let mut notifier_cancel = notifier_cancel_rx;
            loop {
                let result = scan_inbox(
                    &inbox_store,
                    &inbox_path,
                    &inbox_notifications,
                    &mut notifier_seen
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner()),
                    true,
                );
                if result.is_err() {
                    failure_sender.send_replace(result.err());
                    let _ = notifier_cancel_tx.send(true);
                    break;
                }
                tokio::select! {
                    _ = done_rx.changed() => break,
                    changed = notifier_cancel.changed() => {
                        if changed.is_err() || *notifier_cancel.borrow() { break }
                    },
                    _ = changes.changed() => {},
                }
            }
        });
        running.insert(
            path,
            Running {
                cancel,
                task,
                outcome,
                notifier,
                inbox: inbox_tx,
                seen_inbox_ids,
            },
        );
        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        let _ = self.stop.send(true);
        let mut failures = Vec::new();
        if let Some(supervisor) = self.supervisor.lock().await.take() {
            if let Err(error) = supervisor.await {
                failures.push(format!("supervisor join failed: {error}"));
            }
        }
        let mut tasks = self.running.lock().await;
        for running in tasks.values() {
            let _ = running.cancel.send(true);
        }
        let drained = std::mem::take(&mut *tasks);
        drop(tasks);
        for (path, running) in drained {
            let cancelled = *running.cancel.borrow();
            let monitor_result = running.task.await;
            let outcome = running.outcome.lock().await.take();
            if let Err(error) = monitor_result {
                failures.push(format!(
                    "agent {} completion watcher failed: {error}",
                    path.0
                ));
            }
            if let Err(error) = running.notifier.await {
                failures.push(format!("agent {} notifier join failed: {error}", path.0));
            }
            match outcome {
                Some(Ok(_)) => {}
                Some(Err(error)) if expected_cancellation(&error, cancelled) => {}
                Some(Err(error)) => failures.push(format!("agent {} failed: {error}", path.0)),
                None => failures.push(format!("agent {} ended without an outcome", path.0)),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }

    pub fn wake_receiver(&self) -> watch::Receiver<u64> {
        self.wake.subscribe()
    }

    /// Wait for the root Engine's terminal result. The result is retained
    /// independently of task reaping, so callers may subscribe before or after
    /// completion and receive usage/transcript on success or the terminal error.
    pub async fn wait_root_completion(&self) -> Result<EngineCompletion, String> {
        let mut completion = self.root_completion.subscribe();
        loop {
            if let Some(result) = completion.borrow().clone() {
                return result;
            }
            completion
                .changed()
                .await
                .map_err(|_| "root completion channel closed without a result".to_owned())?;
        }
    }

    /// Subscribe to retained root completion without awaiting it immediately.
    pub fn root_completion_receiver(
        &self,
    ) -> watch::Receiver<Option<Result<EngineCompletion, String>>> {
        self.root_completion.subscribe()
    }
}

pub fn initial_for_agent(path: &AgentPath, has_head: bool, root_input: &[Item]) -> Vec<Item> {
    if path.0 == "/root" && !has_head {
        root_input.to_vec()
    } else {
        Vec::new()
    }
}

fn expected_cancellation(error: &str, cancellation_requested: bool) -> bool {
    cancellation_requested && error.to_ascii_lowercase().contains("cancel")
}

pub fn final_answer(items: &[Item]) -> Option<String> {
    items.iter().rev().find_map(|item| {
        if item.0["type"] != "message"
            || item.0["role"] != "assistant"
            || item.0["phase"] != "final_answer"
        {
            return None;
        }
        item.0["content"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|part| {
                        (part["type"] == "output_text")
                            .then(|| part["text"].as_str())
                            .flatten()
                    })
                    .collect::<String>()
            })
            .filter(|s| !s.is_empty())
    })
}

/// Publish only the completion that the Engine actually accepted. For a
/// strict finalize, preserve the result and Store input-provenance through
/// the shared wire-safe JSON message codec.
pub fn published_final_answer(
    store: &Store,
    sender: &AgentPath,
    parent: &AgentPath,
    completion: &EngineCompletion,
) -> Result<Option<Item>, String> {
    let finalize_calls = completion
        .turn
        .items
        .iter()
        .filter(|item| item.0["type"] == "function_call" && item.0["name"] == "finalize")
        .collect::<Vec<_>>();
    if let [finalize] = finalize_calls.as_slice() {
        // The Engine has already enforced strict schema and exactly one call.
        // Decode here only to carry that same typed JSON into publication.
        let result: Value = FinalizeParser::new()
            .parse_completed(finalize)
            .map_err(|error| error.to_string())?;
        let provenance = store
            .completion_provenance(sender, &completion.head_request)
            .map_err(|error| error.to_string())?;
        let answer = PublishedAnswer {
            sender: sender.0.clone(),
            result,
            provenance,
        };
        return answer
            .to_message_item()
            .map(Some)
            .map_err(|error| error.to_string());
    }
    if !finalize_calls.is_empty() {
        return Err("multiple finalize calls in accepted completion".into());
    }
    let Some(text) = final_answer(&completion.turn.items) else {
        return Ok(None);
    };
    Ok(Some(Item(json!({
        "type": "message",
        "role": "assistant",
        "content": [{
            "type": "output_text",
            "text": format!(
                "Message Type: FINAL_ANSWER\nTask name: {}\nSender: {}\nPayload:\n{}",
                parent.0, sender.0, text
            )
        }]
    }))))
}

pub fn scan_inbox(
    store: &Store,
    path: &AgentPath,
    sender: &mpsc::UnboundedSender<Envelope>,
    sent: &mut HashSet<i64>,
    send_hints: bool,
) -> Result<(), String> {
    let inbox = store.unread(&path.0).map_err(|e| e.to_string())?;
    for entry in inbox {
        if !sent.insert(entry.id) {
            continue;
        }
        let sender_path = AgentPath::parse(&entry.sender)
            .map_err(|e| format!("malformed sender on inbox envelope {}: {e}", entry.id))?;
        let recipient_path = AgentPath::parse(&entry.recipient)
            .map_err(|e| format!("malformed recipient on inbox envelope {}: {e}", entry.id))?;
        let class = match entry.class.as_str() {
            "Steer" => DeliveryClass::Steer,
            "AtBoundary" => DeliveryClass::AtBoundary,
            "Hold" => DeliveryClass::Hold,
            other => {
                return Err(format!(
                    "malformed delivery class on inbox envelope {}: {other}",
                    entry.id
                ));
            }
        };
        if !send_hints {
            continue;
        }
        sender
            .send(Envelope {
                kind: EnvelopeType::Message,
                recipient: recipient_path,
                sender: sender_path,
                payload: String::new(),
                class,
                timestamp_ms: entry.created_at,
            })
            .map_err(|_| format!("engine inbox closed while signaling envelope {}", entry.id))?;
    }
    Ok(())
}

/// Compatibility name for the reusable tree lifecycle.
pub type Driver<F> = TreeDriver<F>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{ResponsesTurn, Usage};
    use std::sync::atomic::AtomicUsize;

    struct UnusedFactory;

    #[async_trait]
    impl EngineFactory for UnusedFactory {
        type Engine = ();

        async fn create(&self, _agent: AgentPath) -> Result<Self::Engine, String> {
            Ok(())
        }

        async fn run(
            &self,
            _engine: &Self::Engine,
            _head: Option<RequestId>,
            _initial: Vec<Item>,
            _cancel: watch::Receiver<bool>,
            _inbox: mpsc::UnboundedReceiver<Envelope>,
        ) -> Result<EngineCompletion, String> {
            unreachable!("the barrier test inserts a synthetic completed worker")
        }
    }

    #[tokio::test]
    async fn reaper_waits_for_completion_monitor_join_after_removing_ready_outcome() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
        let driver = Arc::new(TreeDriver::new(store, service, Arc::new(UnusedFactory)));
        let completion = EngineCompletion {
            turn: ResponsesTurn {
                response_id: "test-completion".into(),
                items: vec![],
                usage: Usage::default(),
            },
            transcript: vec![],
            head_request: RequestId("test-head".into()),
        };
        let outcome = Arc::new(Mutex::new(Some(Ok(completion))));
        let (cancel, _) = watch::channel(false);
        let (inbox, _receiver) = mpsc::unbounded_channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let monitor_exited = Arc::new(AtomicUsize::new(0));
        let monitor_exited_task = monitor_exited.clone();
        let monitor = tokio::spawn(async move {
            let _ = release_rx.await;
            monitor_exited_task.store(1, Ordering::SeqCst);
        });
        driver.running.lock().await.insert(
            root.clone(),
            Running {
                cancel,
                task: monitor,
                outcome,
                notifier: tokio::spawn(async {}),
                inbox,
                seen_inbox_ids: Arc::new(std::sync::Mutex::new(HashSet::new())),
            },
        );
        let reaper_driver = driver.clone();
        let reaper = tokio::spawn(async move { reaper_driver.reap_finished().await });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while driver.active_agent_count().await != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("ready outcome is removed before the monitor join");
        assert_eq!(monitor_exited.load(Ordering::SeqCst), 0);
        release_tx.send(()).unwrap();
        reaper.await.unwrap().unwrap();
        assert_eq!(monitor_exited.load(Ordering::SeqCst), 1);
    }
}
