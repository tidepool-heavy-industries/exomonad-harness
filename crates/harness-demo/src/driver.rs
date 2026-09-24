//! One-process durable agent-tree runtime.
//!
//! `Driver` owns one engine task per admitted agent. Store rows are the source
//! of truth; watches only trigger rescans. The factory is deliberately
//! injectable so callers can supply a deterministic `ResponsesTransport`.
use async_trait::async_trait;
use harness::{
    agent_runtime::StoreAgentToolService,
    engine::{Engine, EngineCompletion, EngineError, ResponsesTransport},
    item::Item,
    mailbox::{DeliveryClass, Envelope, EnvelopeType},
    model::{AgentPath, Effort, RequestId},
    provider::Provider,
    store::{AgentState, Store},
    transport::Auth,
    turn::JobScheduler,
};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{Mutex, mpsc, watch};

/// Creates an engine bound to the supplied Store agent identity.
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

/// Generic bridge for production and replay engine construction.
pub struct HarnessEngineFactory<A, P, C, F> {
    pub auth: Arc<A>,
    pub store: Arc<Store>,
    pub scheduler: Arc<JobScheduler>,
    pub provider: Arc<P>,
    pub config: F,
    pub transport: std::marker::PhantomData<C>,
}

#[async_trait]
impl<A, P, C, F> EngineFactory for HarnessEngineFactory<A, P, C, F>
where
    A: Auth + Clone + Send + Sync + 'static,
    P: Provider + 'static,
    C: ResponsesTransport + 'static,
    F: Fn(&AgentPath) -> Result<C, String> + Send + Sync + 'static,
{
    type Engine = Engine<A, P, C>;

    async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
        let transport = (self.config)(&agent)?;
        Ok(Engine::with_transport(
            transport,
            self.store.clone(),
            self.scheduler.clone(),
            self.provider.clone(),
            harness::engine::EngineConfig {
                instructions: "You are a helpful assistant. Complete the assigned task, and use wait_agent when coordinating with children.".into(),
                tools: Vec::new(),
                model: "gpt-6-sol".into(),
                effort: Effort::Low,
                session_id: format!("harness-tree-{}", agent.0),
                agent,
            },
        ))
    }

    async fn run(
        &self,
        engine: &Self::Engine,
        head: Option<RequestId>,
        initial: Vec<Item>,
        cancel: watch::Receiver<bool>,
        inbox: mpsc::UnboundedReceiver<Envelope>,
    ) -> Result<EngineCompletion, String> {
        engine
            .run_from_head_with_durable_envelopes(head, initial, cancel, inbox)
            .await
            .map_err(|e: EngineError| e.to_string())
    }
}

struct Running {
    cancel: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    outcome: Arc<Mutex<Option<Result<EngineCompletion, String>>>>,
    notifier: tokio::task::JoinHandle<()>,
    inbox: mpsc::UnboundedSender<Envelope>,
    seen_inbox_ids: Arc<std::sync::Mutex<HashSet<i64>>>,
}

/// The runtime is started after constructing the StoreAgentToolService used by
/// the TreeProvider. Call `shutdown` to cancel and join every admitted task.
pub struct Driver<F: EngineFactory> {
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

impl<F: EngineFactory> Driver<F> {
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
            if !store
                .advance_agent_head(
                    &task_path,
                    record.head_request.as_ref(),
                    Some(&completed_head),
                )
                .map_err(|e| e.to_string())?
            {
                return Err(format!("head CAS lost for {}", task_path.0));
            }
            if let Some(parent) = record.parent {
                if let Some(text) = final_answer(&completion.turn.items) {
                    let item = Item(json!({"type":"message","role":"assistant","content":[{
                        "type":"output_text","text":format!("Message Type: FINAL_ANSWER\nTask name: {}\nSender: {}\nPayload:\n{}", parent.0, task_path.0, text)
                    }]}));
                    let parent_running = running_tasks.lock().await;
                    let parent_task = parent_running.get(&parent);
                    let mut seen = parent_task.map(|task| {
                        task.seen_inbox_ids
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner())
                    });
                    let envelope_id = store
                        .add_envelope(&task_path.0, &parent.0, "AtBoundary", &item, None)
                        .map_err(|e| e.to_string())?;
                    if let Some(parent_task) = parent_task {
                        seen.as_mut().expect("parent seen set acquired").insert(envelope_id);
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
            }
            Ok::<EngineCompletion, String>(completion)
            }.await;
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

fn initial_for_agent(path: &AgentPath, has_head: bool, root_input: &[Item]) -> Vec<Item> {
    if path.0 == "/root" && !has_head {
        root_input.to_vec()
    } else {
        Vec::new()
    }
}

fn expected_cancellation(error: &str, cancellation_requested: bool) -> bool {
    cancellation_requested && error.to_ascii_lowercase().contains("cancel")
}

fn final_answer(items: &[Item]) -> Option<String> {
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

fn scan_inbox(
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

#[cfg(test)]
mod tests {
    use super::*;
    use harness::{
        engine::ResponsesTransport,
        mailbox::EnvelopeType,
        store::Usage as StoredUsage,
        transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, Usage as TurnUsage},
    };
    use serde_json::Value;
    use std::{
        collections::VecDeque,
        sync::{
            Mutex as StdMutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[derive(Clone)]
    struct ReplayAuth;
    impl Auth for ReplayAuth {
        fn access(&self) -> Result<(String, String), TransportError> {
            Err(TransportError::Authentication)
        }
    }

    #[derive(Clone)]
    struct ReplayTransport {
        agent: String,
        responses: Arc<StdMutex<HashMap<String, VecDeque<Vec<Item>>>>>,
        request_counts: Arc<StdMutex<HashMap<String, usize>>>,
        inputs: Arc<StdMutex<HashMap<String, Vec<Vec<Item>>>>>,
    }

    #[async_trait]
    impl ResponsesTransport for ReplayTransport {
        async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
            let key = self.agent.clone();
            assert!(request.session_id.ends_with(&key));
            self.inputs
                .lock()
                .unwrap()
                .entry(key.clone())
                .or_default()
                .push(request.input.clone());
            let items = self
                .responses
                .lock()
                .unwrap()
                .get_mut(&key)
                .and_then(VecDeque::pop_front)
                .ok_or_else(|| TransportError::Stream(format!("no replay response for {key}")))?;
            *self
                .request_counts
                .lock()
                .unwrap()
                .entry(key.clone())
                .or_default() += 1;
            Ok(ResponsesTurn {
                response_id: format!("replay-{key}"),
                items,
                usage: TurnUsage::default(),
            })
        }
    }

    fn function_call(id: &str, name: &str, args: Value) -> Item {
        Item(json!({
            "type":"function_call","call_id":id,"name":name,
            "arguments":serde_json::to_string(&args).unwrap()
        }))
    }

    fn final_answer(text: &str) -> Item {
        Item(json!({
            "type":"message","role":"assistant","phase":"final_answer",
            "content":[{"type":"output_text","text":text}]
        }))
    }

    struct JoinFailureFactory {
        started: Arc<AtomicUsize>,
        exited: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl EngineFactory for JoinFailureFactory {
        type Engine = AgentPath;

        async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
            Ok(agent)
        }

        async fn run(
            &self,
            engine: &Self::Engine,
            _head: Option<RequestId>,
            _initial: Vec<Item>,
            mut cancel: watch::Receiver<bool>,
            _inbox: mpsc::UnboundedReceiver<Envelope>,
        ) -> Result<EngineCompletion, String> {
            self.started.fetch_add(1, Ordering::SeqCst);
            while !*cancel.borrow() {
                cancel.changed().await.map_err(|error| error.to_string())?;
            }
            self.exited.fetch_add(1, Ordering::SeqCst);
            if engine.0 == "/root" {
                Err("intentional engine failure".into())
            } else {
                Err("cancelled".into())
            }
        }
    }

    struct NoAnswerFactory {
        store: Arc<Store>,
        calls: Arc<StdMutex<HashMap<String, usize>>>,
    }

    #[async_trait]
    impl EngineFactory for NoAnswerFactory {
        type Engine = AgentPath;

        async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
            Ok(agent)
        }

        async fn run(
            &self,
            engine: &Self::Engine,
            head: Option<RequestId>,
            _initial: Vec<Item>,
            _cancel: watch::Receiver<bool>,
            _inbox: mpsc::UnboundedReceiver<Envelope>,
        ) -> Result<EngineCompletion, String> {
            *self
                .calls
                .lock()
                .unwrap()
                .entry(engine.0.clone())
                .or_default() += 1;
            let id = RequestId(format!("no-answer-{}", engine.0.replace('/', "_")));
            self.store
                .write_request(&id, head.as_ref(), &engine.0, &[], StoredUsage::default())
                .map_err(|error| error.to_string())?;
            Ok(EngineCompletion {
                turn: ResponsesTurn {
                    response_id: id.0.clone(),
                    items: vec![],
                    usage: TurnUsage::default(),
                },
                transcript: vec![],
                head_request: id,
            })
        }
    }

    struct FastFailureFactory(Arc<AtomicUsize>);

    #[async_trait]
    impl EngineFactory for FastFailureFactory {
        type Engine = AgentPath;

        async fn create(&self, agent: AgentPath) -> Result<Self::Engine, String> {
            Ok(agent)
        }

        async fn run(
            &self,
            engine: &Self::Engine,
            _head: Option<RequestId>,
            _initial: Vec<Item>,
            _cancel: watch::Receiver<bool>,
            _inbox: mpsc::UnboundedReceiver<Envelope>,
        ) -> Result<EngineCompletion, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(format!("failure at {}", engine.0))
        }
    }

    #[tokio::test]
    async fn replay_root_child_final_answer_is_durable_before_root_wake() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
        let responses = Arc::new(StdMutex::new(HashMap::from([
            (
                "/root".to_owned(),
                VecDeque::from([
                    vec![function_call(
                        "spawn-1",
                        "spawn_agent",
                        json!({
                            "task_name":"child",
                            "from":{"kind":"prompt","name":null},
                            "task":{
                                "clauses":["answer child task"],"acceptance":["child answer"],
                                "owned":[],"must_not":[],"introduces":[],"consumes":[],"boundaries":[]
                            }
                        }),
                    )],
                    vec![function_call("wait-1", "wait_agent", json!({}))],
                    vec![final_answer("root-result")],
                ]),
            ),
            (
                "/root/child".to_owned(),
                VecDeque::from([vec![final_answer("child-result")]]),
            ),
        ])));
        let request_counts = Arc::new(StdMutex::new(HashMap::new()));
        let inputs = Arc::new(StdMutex::new(HashMap::new()));
        let provider = Arc::new(crate::tree::TreeProvider::new(
            crate::CliProvider(crate::DemoProvider::development(".", false)),
            service.clone(),
        ));
        let factory = Arc::new(HarnessEngineFactory {
            auth: Arc::new(ReplayAuth),
            store: store.clone(),
            scheduler: Arc::new(JobScheduler::new(2).unwrap()),
            provider,
            config: {
                let responses = responses.clone();
                let request_counts = request_counts.clone();
                let inputs = inputs.clone();
                move |agent: &AgentPath| {
                    Ok(ReplayTransport {
                        agent: agent.0.clone(),
                        responses: responses.clone(),
                        request_counts: request_counts.clone(),
                        inputs: inputs.clone(),
                    })
                }
            },
            transport: std::marker::PhantomData,
        });
        let driver = Driver::new(store.clone(), service, factory);
        let root_prompt = Item(json!({
            "type":"message","role":"user","content":"initial root prompt"
        }));
        driver.start(vec![root_prompt.clone()]).await.unwrap();
        assert!(
            driver.start(vec![root_prompt.clone()]).await.is_err(),
            "a second start must not replace the live supervisor"
        );
        let settled = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let root_agent = store.agent(&root).unwrap().unwrap();
                let child = store.agent(&AgentPath("/root/child".into())).unwrap();
                let Some(child) = child else {
                    tokio::task::yield_now().await;
                    continue;
                };
                if root_agent.head_request.is_some() && child.head_request.is_some() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        if settled.is_err() {
            let running = driver.running.lock().await;
            eprintln!(
                "root={:?} child={:?}",
                store.agent(&root).unwrap(),
                store.agent(&AgentPath("/root/child".into())).unwrap()
            );
            for (path, task) in running.iter() {
                eprintln!(
                    "{} finished={} inbox={:?}",
                    path.0,
                    task.task.is_finished(),
                    store.unread(&path.0).unwrap()
                );
            }
        }
        settled.expect("root and child settle");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if driver.running.lock().await.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("successful tasks are reaped from the running map");
        let completion = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            driver.wait_root_completion(),
        )
        .await
        .expect("root completion should be signaled")
        .expect("root Engine should complete successfully");
        assert_eq!(
            super::final_answer(&completion.turn.items).as_deref(),
            Some("root-result")
        );
        assert_eq!(
            driver.wait_root_completion().await.unwrap().head_request,
            completion.head_request,
            "completion remains available after task reaping"
        );
        let inbox = store.inbox("/root").unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].class, "AtBoundary");
        assert!(inbox[0].delivered_request.is_some());
        let root_head = store.agent(&root).unwrap().unwrap().head_request.unwrap();
        let child_head = store
            .agent(&AgentPath("/root/child".into()))
            .unwrap()
            .unwrap()
            .head_request
            .unwrap();
        let wait_request = inbox[0]
            .delivered_request
            .as_ref()
            .expect("final envelope was attached");
        assert_eq!(
            store.request(&root_head).unwrap().unwrap().parent.as_ref(),
            Some(wait_request),
            "final root response branches from the wait/inbox request"
        );
        assert_eq!(
            store.request(wait_request).unwrap().unwrap().branch,
            "/root",
            "wait/inbox request belongs to root"
        );
        assert_eq!(
            store.request(&child_head).unwrap().unwrap().branch,
            "/root/child",
            "child head belongs to child"
        );
        assert_eq!(
            store.inbox("/root/child").unwrap()[0]
                .delivered_request
                .as_ref(),
            Some(&child_head),
            "child task is attached to its durable head once"
        );
        let root_items = store.items(wait_request).unwrap();
        let wait_output = root_items
            .iter()
            .position(|item| {
                item.0["type"] == "function_call_output" && item.0["call_id"] == "wait-1"
            })
            .expect("wait call has a persisted output");
        let durable_final = root_items
            .iter()
            .position(|item| {
                item.0["role"] == "assistant"
                    && item.0["content"][0]["text"]
                        .as_str()
                        .is_some_and(|text| text.starts_with("Message Type: FINAL_ANSWER\n"))
            })
            .expect("child final envelope attached to root request");
        assert!(
            wait_output < durable_final,
            "wait status precedes inbox attachment"
        );
        let resumed: Value =
            serde_json::from_str(root_items[wait_output].0["output"].as_str().unwrap()).unwrap();
        assert_eq!(resumed["resumed_by"]["agent"], "/root/child");
        let item = store.get_item(&inbox[0].item_hash).unwrap().unwrap();
        let text = item.0["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("Message Type: FINAL_ANSWER\n"));
        assert!(text.ends_with("child-result"));
        assert!(store.unread("/root").unwrap().is_empty());
        let child_inbox = store.inbox("/root/child").unwrap();
        assert_eq!(child_inbox.len(), 1, "one durable NEW_TASK");
        assert!(child_inbox[0].delivered_request.is_some());
        assert_eq!(
            request_counts.lock().unwrap()["/root"],
            3,
            "no spurious wait wake"
        );
        assert_eq!(request_counts.lock().unwrap()["/root/child"], 1);
        assert_eq!(*driver.failure_receiver().borrow(), None);
        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_cancels_a_root_wait_without_reporting_success_as_failure() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
        let responses = Arc::new(StdMutex::new(HashMap::from([(
            "/root".to_owned(),
            VecDeque::from([vec![function_call(
                "wait-shutdown",
                "wait_agent",
                json!({}),
            )]]),
        )])));
        let request_counts = Arc::new(StdMutex::new(HashMap::new()));
        let inputs = Arc::new(StdMutex::new(HashMap::new()));
        let provider = Arc::new(crate::tree::TreeProvider::new(
            crate::CliProvider(crate::DemoProvider::development(".", false)),
            service.clone(),
        ));
        let factory = Arc::new(HarnessEngineFactory {
            auth: Arc::new(ReplayAuth),
            store: store.clone(),
            scheduler: Arc::new(JobScheduler::new(1).unwrap()),
            provider,
            config: {
                let responses = responses.clone();
                let request_counts = request_counts.clone();
                let inputs = inputs.clone();
                move |agent: &AgentPath| {
                    Ok(ReplayTransport {
                        agent: agent.0.clone(),
                        responses: responses.clone(),
                        request_counts: request_counts.clone(),
                        inputs: inputs.clone(),
                    })
                }
            },
            transport: std::marker::PhantomData,
        });
        let driver = Driver::new(store, service, factory);
        driver.start(Vec::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if request_counts.lock().unwrap().get("/root") == Some(&1) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("root reached wait_agent");
        driver
            .shutdown()
            .await
            .expect("expected Engine cancellation is a successful shutdown");
        assert!(driver.running.lock().await.is_empty());
        assert_eq!(*driver.failure_receiver().borrow(), None);
    }

    #[tokio::test]
    async fn shutdown_joins_remaining_tasks_after_first_task_failure() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({}))
            .unwrap();
        let child = AgentPath("/root/child".into());
        store
            .admit_agent(&child, Some(&root), None, &json!({}), &json!({}))
            .unwrap();
        let started = Arc::new(AtomicUsize::new(0));
        let exited = Arc::new(AtomicUsize::new(0));
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root));
        let driver = Driver::new(
            store.clone(),
            service,
            Arc::new(JoinFailureFactory {
                started: started.clone(),
                exited: exited.clone(),
            }),
        );
        driver.start(Vec::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while started.load(Ordering::SeqCst) != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both tasks started");
        let result = driver.shutdown().await;
        assert!(result.unwrap_err().contains("intentional engine failure"));
        assert_eq!(exited.load(Ordering::SeqCst), 2, "all engine tasks joined");
        assert!(driver.running.lock().await.is_empty());
    }

    #[tokio::test]
    async fn completion_signal_before_monitor_exit_cannot_be_missed_by_reaper() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
        let driver = Arc::new(Driver::new(
            store.clone(),
            service,
            Arc::new(NoAnswerFactory {
                store,
                calls: Arc::new(StdMutex::new(HashMap::new())),
            }),
        ));
        let completion = EngineCompletion {
            turn: ResponsesTurn {
                response_id: "early-result".into(),
                items: vec![],
                usage: TurnUsage::default(),
            },
            transcript: vec![],
            head_request: RequestId("early-head".into()),
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
        // Model the completion watcher publishing the outcome and wake while
        // its own JoinHandle is still running.
        driver
            .task_generation
            .send_modify(|generation| *generation = generation.wrapping_add(1));
        let reaper_driver = driver.clone();
        let reaper = tokio::spawn(async move { reaper_driver.reap_finished().await });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !driver.running.lock().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("outcome-ready task is reaped despite monitor still running");
        assert_eq!(monitor_exited.load(Ordering::SeqCst), 0);
        release_tx.send(()).unwrap();
        reaper.await.unwrap().unwrap();
        assert_eq!(monitor_exited.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reaper_joins_all_finished_handles_after_first_failure() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({}))
            .unwrap();
        store
            .admit_agent(
                &AgentPath("/root/child".into()),
                Some(&root),
                None,
                &json!({}),
                &json!({}),
            )
            .unwrap();
        let failures = Arc::new(AtomicUsize::new(0));
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root));
        let driver = Driver::new(
            store,
            service,
            Arc::new(FastFailureFactory(failures.clone())),
        );
        let mut failure_rx = driver.failure_receiver();
        driver.start(Vec::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), failure_rx.changed())
            .await
            .expect("supervisor reports task failure")
            .expect("failure receiver remains open");
        assert!(
            failure_rx.borrow().as_deref().unwrap().contains("failed"),
            "error is surfaced, not swallowed"
        );
        assert_eq!(failures.load(Ordering::SeqCst), 2);
        assert!(driver.running.lock().await.is_empty());
        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn successful_no_answer_child_is_not_restarted_without_new_inbox() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({}))
            .unwrap();
        store
            .admit_agent(
                &AgentPath("/root/child".into()),
                Some(&root),
                None,
                &json!({}),
                &json!({}),
            )
            .unwrap();
        let calls = Arc::new(StdMutex::new(HashMap::new()));
        let service = Arc::new(StoreAgentToolService::new(store.clone(), root));
        let driver = Driver::new(
            store.clone(),
            service,
            Arc::new(NoAnswerFactory {
                store: store.clone(),
                calls: calls.clone(),
            }),
        );
        driver.start(Vec::new()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if driver.running.lock().await.is_empty() && calls.lock().unwrap().len() == 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("both no-answer runs complete and reap");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(calls.lock().unwrap()["/root/child"], 1);
        driver.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn restart_with_existing_root_head_does_not_duplicate_prompt() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({"kind":"root"}))
            .unwrap();
        let original = Item(json!({
            "type":"message","role":"user","content":"persisted start prompt"
        }));
        let old_head = RequestId("prior-root-head".into());
        store
            .write_request(
                &old_head,
                None,
                &root.0,
                std::slice::from_ref(&original),
                StoredUsage::default(),
            )
            .unwrap();
        assert!(
            store
                .advance_agent_head(&root, None, Some(&old_head))
                .unwrap()
        );

        let service = Arc::new(StoreAgentToolService::new(store.clone(), root.clone()));
        let calls = Arc::new(StdMutex::new(HashMap::new()));
        let driver = Driver::new(
            store.clone(),
            service,
            Arc::new(NoAnswerFactory {
                store: store.clone(),
                calls: calls.clone(),
            }),
        );
        driver.start(vec![original.clone()]).await.unwrap();
        assert!(driver.running.lock().await.is_empty());
        assert!(
            calls.lock().unwrap().is_empty(),
            "head/no-inbox agent not run"
        );
        assert_eq!(
            store.agent(&root).unwrap().unwrap().head_request,
            Some(old_head.clone()),
            "startup does not create a duplicate model turn"
        );
        let history = store.items(&old_head).unwrap();
        assert_eq!(
            history
                .iter()
                .filter(|item| {
                    item.0["role"] == "user" && item.0["content"] == "persisted start prompt"
                })
                .count(),
            1,
            "persisted original prompt remains exactly once"
        );
        assert!(
            driver.start(vec![original]).await.is_err(),
            "second start is rejected instead of replacing the supervisor"
        );
        driver.shutdown().await.unwrap();
    }

    #[test]
    fn unread_wake_hints_are_minimal_and_deduplicated() {
        let store = Store::memory().unwrap();
        store
            .admit_agent(
                &AgentPath("/root".into()),
                None,
                None,
                &json!({}),
                &json!({}),
            )
            .unwrap();
        store
            .admit_agent(
                &AgentPath("/root/child".into()),
                Some(&AgentPath("/root".into())),
                None,
                &json!({}),
                &json!({}),
            )
            .unwrap();
        store
            .add_envelope(
                "/root/child",
                "/root",
                "AtBoundary",
                &Item(json!({"type":"message","content":"payload is durable"})),
                None,
            )
            .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut seen = HashSet::new();
        let root = AgentPath("/root".into());
        scan_inbox(&store, &root, &tx, &mut seen, true).unwrap();
        scan_inbox(&store, &root, &tx, &mut seen, true).unwrap();
        let hint = rx.try_recv().unwrap();
        assert_eq!(hint.kind, EnvelopeType::Message);
        assert_eq!(hint.sender.0, "/root/child");
        assert_eq!(hint.recipient.0, "/root");
        assert!(hint.payload.is_empty());
        assert!(rx.try_recv().is_err());
        assert_eq!(
            store.unread("/root").unwrap().len(),
            1,
            "hint did not deliver row"
        );
    }

    #[test]
    fn existing_root_head_does_not_reappend_original_start_input() {
        let prompt = Item(json!({"type":"message","role":"user","content":"start"}));
        assert_eq!(
            initial_for_agent(
                &AgentPath("/root".into()),
                false,
                std::slice::from_ref(&prompt)
            ),
            vec![prompt.clone()]
        );
        assert!(
            initial_for_agent(
                &AgentPath("/root".into()),
                true,
                std::slice::from_ref(&prompt)
            )
            .is_empty()
        );
        assert!(
            initial_for_agent(
                &AgentPath("/root/child".into()),
                false,
                std::slice::from_ref(&prompt)
            )
            .is_empty()
        );
    }
}
