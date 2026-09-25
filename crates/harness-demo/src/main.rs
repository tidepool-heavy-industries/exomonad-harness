//! Reference provider for the harness. `run` intentionally invokes a local
//! shell and is suitable only for trusted, development-time demonstrations.
pub mod driver;
mod trace;
pub mod tree;

use async_trait::async_trait;
use driver::{Driver, HarnessEngineFactory};
use harness::agent_runtime::StoreAgentToolService;
use harness::engine::{Engine, EngineCompletion, EngineConfig, EngineError};
use harness::item::Item;
use harness::model::{AgentPath, Effort};
use harness::provider::{CallContext, Provider, ProviderError};
use harness::server::{self, ClientCommand, QueuedCommand, ServerConfig, SessionSecret, Snapshot};
use harness::store::Store;
use harness::transport::{ResponsesClient, TransportError, auth::CodexFileAuth};
use harness::turn::JobScheduler;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use trace::{JobEvent, TraceSink, TraceTransport};
use tree::TreeProvider;

const OUTPUT_LIMIT: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CliOptions {
    db: PathBuf,
    ask: String,
    dev_shell: bool,
    tree: bool,
    trace_jsonl: Option<PathBuf>,
    compact_at_input_tokens: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Smoke,
    Ask(CliOptions),
    Serve {
        db: PathBuf,
        addr: SocketAddr,
        dev_shell: bool,
    },
}

fn parse_args(args: &[String]) -> Result<Mode, String> {
    if args.first().map(String::as_str) == Some("--smoke") {
        return Ok(Mode::Smoke);
    }
    let (
        mut db,
        mut ask,
        mut serve,
        mut dev_shell,
        mut tree,
        mut trace_jsonl,
        mut compact_at_input_tokens,
    ) = (None, None, None, false, false, None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => {
                i += 1;
                db = Some(PathBuf::from(args.get(i).ok_or("--db requires a path")?));
            }
            "--ask" => {
                i += 1;
                ask = Some(args.get(i).ok_or("--ask requires text")?.clone());
            }
            "--serve" => {
                i += 1;
                serve = Some(parse_serve_address(
                    args.get(i).ok_or("--serve requires an address")?,
                )?);
            }
            "--dev-shell" => dev_shell = true,
            "--tree" => tree = true,
            "--trace-jsonl" => {
                i += 1;
                trace_jsonl = Some(PathBuf::from(
                    args.get(i).ok_or("--trace-jsonl requires a path")?,
                ));
            }
            "--compact-at-input-tokens" => {
                i += 1;
                let raw = args
                    .get(i)
                    .ok_or("--compact-at-input-tokens requires a number")?;
                let value: u64 = raw
                    .parse()
                    .map_err(|_| "--compact-at-input-tokens requires a positive integer")?;
                if value == 0 {
                    return Err("--compact-at-input-tokens requires a positive integer".into());
                }
                compact_at_input_tokens = Some(value);
            }
            "--allow-shell" => {
                return Err("use --dev-shell to opt into development shell execution".into());
            }
            flag => return Err(format!("unknown argument: {flag}")),
        }
        i += 1;
    }
    let db = db.ok_or("--db <sqlite-path> is required")?;
    if trace_jsonl.is_some() && (!tree || serve.is_some()) {
        return Err("--trace-jsonl requires --tree --ask".into());
    }
    if serve.is_some() && compact_at_input_tokens.is_some() {
        return Err("--compact-at-input-tokens requires --ask".into());
    }
    if let Some(addr) = serve {
        if ask.is_some() {
            return Err("--serve and --ask are mutually exclusive".into());
        }
        if tree {
            return Err("--tree is currently supported with --ask only".into());
        }
        return Ok(Mode::Serve {
            db,
            addr,
            dev_shell,
        });
    }
    let ask = ask.ok_or("--ask <text> is required")?;
    if ask.trim().is_empty() {
        return Err("--ask text must not be empty".into());
    }
    Ok(Mode::Ask(CliOptions {
        db,
        ask,
        dev_shell,
        tree,
        trace_jsonl,
        compact_at_input_tokens,
    }))
}

/// Browser login and its cookie use HTTP in this demo; do not expose them over
/// a LAN interface. A deliberate HTTPS/reverse-proxy mode can be added later.
fn parse_serve_address(value: &str) -> Result<SocketAddr, String> {
    let address = value
        .parse::<SocketAddr>()
        .map_err(|_| "--serve requires a valid socket address".to_owned())?;
    if !address.ip().is_loopback() {
        return Err(
            "--serve is loopback-only because browser-session login uses plain HTTP".into(),
        );
    }
    Ok(address)
}

fn ensure_asset_root(path: &Path) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!(
            "web assets are missing at {}; build web/dist before starting --serve",
            path.display()
        ));
    }
    Ok(())
}

pub struct CliProvider(DemoProvider);

#[async_trait]
impl Provider for CliProvider {
    async fn call(&self, name: &str, args: Value) -> Result<Value, ProviderError> {
        self.0.call(name, args).await
    }

    async fn call_with_context(
        &self,
        name: &str,
        args: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        self.0.call_with_context(name, args, context).await
    }

    fn tools(&self) -> Vec<Value> {
        self.0
            .tools()
            .into_iter()
            .filter_map(|mut tool| {
                if matches!(tool["name"].as_str(), Some("ask" | "form")) {
                    return None;
                }
                if tool["name"] != "run" {
                    return Some(tool);
                }
                if !self.0.allow_shell {
                    return None;
                }
                // The engine currently dispatches function_call items only;
                // expose the opt-in shell as a strict function at this CLI
                // adapter boundary rather than inventing another call loop.
                tool["type"] = json!("function");
                tool["strict"] = json!(true);
                tool["parameters"] = json!({
                    "type":"object",
                    "properties":{"script":{"type":"string"}},
                    "required":["script"],
                    "additionalProperties":false
                });
                Some(tool)
            })
            .collect()
    }

    // This single-agent CLI has no AgentToolService for the built-in verbs.
    fn all_tools(&self) -> Vec<Value> {
        self.tools()
    }
}

struct CliDriver {
    engine: Engine<CodexFileAuth, CliProvider>,
}

impl CliDriver {
    fn new(options: &CliOptions) -> Result<Self, String> {
        let auth_path = CodexFileAuth::default_path()
            .map_err(|_| "Codex credentials unavailable; check ~/.codex/auth.json".to_owned())?;
        let store = Arc::new(
            Store::open(&options.db).map_err(|_| "could not open SQLite store".to_owned())?,
        );
        let jobs = Arc::new(
            JobScheduler::new(4).map_err(|_| "could not initialize tool scheduler".to_owned())?,
        );
        let provider = Arc::new(CliProvider(DemoProvider::development(
            ".",
            options.dev_shell,
        )));
        let engine = Engine::new(
            CodexFileAuth::new(auth_path),
            store,
            jobs,
            provider.clone(),
            EngineConfig {
                instructions: "You are a helpful assistant. Use the available tools when useful. Shell execution, if enabled, is only for trusted development use.".into(),
                tools: Vec::new(),
                model: "gpt-6-sol".into(),
                effort: Effort::Low,
                session_id: format!("harness-demo-{}", std::process::id()),
                agent: AgentPath("/root".into()),
            },
        );
        let engine = match options.compact_at_input_tokens {
            Some(threshold) => engine.with_compaction_threshold(threshold),
            None => engine,
        };
        Ok(Self { engine })
    }

    async fn ask(
        &self,
        prompt: &str,
        history: &[Item],
        cancel_rx: tokio::sync::watch::Receiver<bool>,
    ) -> Result<(String, EngineCompletion), String> {
        let input = command_input(history, prompt);
        let completion = self
            .engine
            .run(
                None,
                input,
                cancel_rx,
                tokio::sync::mpsc::unbounded_channel().1,
            )
            .await
            .map_err(safe_engine_error)?;
        let text = final_text(&completion.turn.items)
            .ok_or_else(|| "engine returned no final assistant text".to_owned())?;
        Ok((text, completion))
    }
}

/// Opt-in, prompt-fork tree demonstration. Restart/recovery is deliberately
/// refused here until the driver can distinguish interrupted from settled
/// model calls across a process boundary.
async fn tree_ask(options: &CliOptions) -> Result<(String, EngineCompletion), String> {
    let auth_path = CodexFileAuth::default_path()
        .map_err(|_| "Codex credentials unavailable; check ~/.codex/auth.json".to_owned())?;
    let auth = CodexFileAuth::new(auth_path);
    let store =
        Arc::new(Store::open(&options.db).map_err(|_| "could not open SQLite store".to_owned())?);
    if store
        .agent(&AgentPath("/root".into()))
        .map_err(|_| "could not inspect tree root".to_owned())?
        .is_some()
    {
        return Err(
            "tree CLI restart is not yet supported; use a new --db path for each tree run".into(),
        );
    }
    let trace_sink = match &options.trace_jsonl {
        Some(path) => Some(
            TraceSink::open(path.clone())
                .await
                .map_err(|_| "could not create a new trace file".to_owned())?,
        ),
        None => None,
    };
    let outcome = async {
        let service = Arc::new(StoreAgentToolService::new(
            store.clone(),
            AgentPath("/root".into()),
        ));
        let mut demo_provider = DemoProvider::development(".", options.dev_shell);
        if let Some(sink) = trace_sink.clone() {
            demo_provider = demo_provider.with_trace(sink);
        }
        let provider = Arc::new(TreeProvider::new(
            CliProvider(demo_provider),
            service.clone(),
        ));
        let jobs = Arc::new(
            JobScheduler::new(4).map_err(|_| "could not initialize tool scheduler".to_owned())?,
        );
        let transport_trace = trace_sink.clone();
        let factory = Arc::new(HarnessEngineFactory {
            auth: Arc::new(auth.clone()),
            store: store.clone(),
            scheduler: jobs,
            provider,
            compact_at_input_tokens: options.compact_at_input_tokens,
            config: move |_agent: &AgentPath| {
                let client = ResponsesClient::new(auth.clone());
                Ok(match &transport_trace {
                    Some(sink) => TraceTransport::new(client, sink.clone()),
                    None => TraceTransport::disabled(client),
                })
            },
            transport: std::marker::PhantomData,
        });
        let driver = Driver::new(store, service, factory);
        let mut failure = driver.failure_receiver();
        if let Err(error) = driver.start(command_input(&[], &options.ask)).await {
            let _ = driver.shutdown().await;
            return Err(format!("could not start tree driver: {error}"));
        }
        let mut root_result = Box::pin(driver.wait_root_completion());
        let result = loop {
            tokio::select! {
                settled = &mut root_result => break settled,
                changed = failure.changed() => {
                    if changed.is_err() {
                        break Err("tree supervisor closed unexpectedly".into());
                    }
                    if let Some(error) = failure.borrow().clone() {
                        break Err(format!("tree agent failed: {error}"));
                    }
                }
                signal = tokio::signal::ctrl_c() => {
                    if signal.is_err() {
                        break Err("could not listen for shutdown".into());
                    }
                    break Err("tree run cancelled by operator".into());
                }
            }
        };
        drop(root_result);
        let shutdown = driver.shutdown().await;
        let completion = result?;
        shutdown.map_err(|_| "could not fully stop tree agents".to_owned())?;
        let text = final_text(&completion.turn.items)
            .ok_or_else(|| "engine returned no final assistant text".to_owned())?;
        Ok((text, completion))
    }
    .await;
    flush_trace_result(trace_sink.as_ref(), outcome).await
}

/// Every exit after opening an opt-in sink, including setup/start failures,
/// flushes it. A failed flush takes priority over the run result.
async fn flush_trace_result<T>(
    sink: Option<&TraceSink>,
    result: Result<T, String>,
) -> Result<T, String> {
    if let Some(sink) = sink {
        sink.flush()
            .await
            .map_err(|_| "could not flush trace file".to_owned())?;
    }
    result
}

const ROOT_CONVERSATION_ID: &str = "conversation/root";
const ROOT_PATH: &str = "/root";

fn conversation_record(state: &str) -> Value {
    json!({"id":ROOT_CONVERSATION_ID,"path":ROOT_PATH,"state":state})
}

fn request_record(id: &str, state: &str) -> Value {
    json!({"id":id,"conversationId":ROOT_CONVERSATION_ID,"state":state})
}

fn job_record(id: &str, state: &str) -> Value {
    json!({"id":id,"conversationId":ROOT_CONVERSATION_ID,"state":state})
}

fn final_envelope(id: &str, payload: &str) -> Value {
    json!({"id":id,"conversationId":ROOT_CONVERSATION_ID,"recipient":ROOT_PATH,
        "sender":ROOT_PATH,"type":"FINAL_ANSWER","payload":payload})
}

fn command_input(history: &[Item], prompt: &str) -> Vec<Item> {
    let mut input = history.to_vec();
    input.push(Item(
        json!({"type":"message","role":"user","content":prompt}),
    ));
    input
}

#[derive(Debug)]
struct PersistedServerState {
    history: Vec<Item>,
    jobs: Vec<Value>,
    requests: Vec<Value>,
    envelopes: Vec<Value>,
    conversation: Value,
}

fn restore_server_state(raw: &str) -> Result<PersistedServerState, String> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| "persisted demo server state is malformed JSON".to_owned())?;
    let required = |field: &str| {
        value
            .get(field)
            .ok_or_else(|| format!("persisted demo server state is missing {field}"))
    };
    let history = serde_json::from_value::<Vec<Item>>(required("history")?.clone())
        .map_err(|_| "persisted demo server history is malformed".to_owned())?;
    let records = |field: &str| -> Result<Vec<Value>, String> {
        required(field)?
            .as_array()
            .cloned()
            .ok_or_else(|| format!("persisted demo server {field} is malformed"))
    };
    let mut conversation = required("conversation")?
        .as_object()
        .cloned()
        .map(Value::Object)
        .ok_or_else(|| "persisted demo server conversation is malformed".to_owned())?;
    if conversation["id"] != ROOT_CONVERSATION_ID || conversation["path"] != ROOT_PATH {
        return Err("persisted demo server conversation does not identify /root".into());
    }
    if !matches!(
        conversation["state"].as_str(),
        Some("idle" | "requesting" | "paused" | "cancelled")
    ) {
        return Err("persisted demo server conversation state is malformed".into());
    }
    let mut jobs = records("jobs")?;
    for job in &mut jobs {
        if job.get("id").and_then(Value::as_str).is_none()
            || job.get("conversationId").and_then(Value::as_str) != Some(ROOT_CONVERSATION_ID)
        {
            return Err("persisted demo server job is malformed".into());
        }
        if !matches!(
            job.get("state").and_then(Value::as_str),
            Some("running" | "settled" | "cancelled")
        ) {
            return Err("persisted demo server job state is malformed".into());
        }
        if job.get("state").and_then(Value::as_str) == Some("running") {
            job["state"] = json!("cancelled");
            job["status"] = json!("Interrupted");
            job["interrupted"] = json!(true);
        }
    }
    let mut requests = records("requests")?;
    for request in &mut requests {
        if request.get("id").and_then(Value::as_str).is_none()
            || request.get("conversationId").and_then(Value::as_str) != Some(ROOT_CONVERSATION_ID)
            || !matches!(
                request.get("state").and_then(Value::as_str),
                Some("running" | "completed" | "failed")
            )
        {
            return Err("persisted demo server request is malformed".into());
        }
        if request.get("state").and_then(Value::as_str) == Some("running") {
            request["state"] = json!("failed");
        }
    }
    let envelopes = records("envelopes")?;
    for envelope in &envelopes {
        if envelope.get("id").and_then(Value::as_str).is_none()
            || envelope.get("conversationId").and_then(Value::as_str) != Some(ROOT_CONVERSATION_ID)
            || envelope.get("sender").and_then(Value::as_str).is_none()
            || envelope.get("recipient").and_then(Value::as_str).is_none()
            || envelope.get("payload").and_then(Value::as_str).is_none()
            || envelope.get("type").and_then(Value::as_str) != Some("FINAL_ANSWER")
        {
            return Err("persisted demo server envelope is malformed".into());
        }
    }
    if conversation["state"] == "requesting" {
        conversation["state"] = json!("idle");
    }
    Ok(PersistedServerState {
        history,
        jobs,
        requests,
        envelopes,
        conversation,
    })
}

async fn serve(db: PathBuf, addr: SocketAddr, dev_shell: bool) -> Result<(), String> {
    if !addr.ip().is_loopback() {
        return Err(
            "--serve is loopback-only because browser-session login uses plain HTTP".into(),
        );
    }
    let asset_root = PathBuf::from("web/dist");
    ensure_asset_root(&asset_root)?;
    let secret = std::env::var("HARNESS_DEMO_SESSION_SECRET")
        .map_err(|_| "HARNESS_DEMO_SESSION_SECRET is required".to_owned())?;
    let secret = SessionSecret::new(secret)?;
    let config = ServerConfig::new(asset_root)
        .with_browser_session(secret, Duration::from_secs(8 * 60 * 60))?;
    let (app, control, mut commands) = server::server_with_config(config);
    let status_store =
        Store::open(&db).map_err(|_| "could not open server status store".to_owned())?;
    let mut history = Vec::<Item>::new();
    let mut jobs = Vec::new();
    let mut requests = Vec::new();
    let mut envelopes = Vec::new();
    let mut conversation = conversation_record("idle");
    if let Some(saved) = status_store
        .session_state("harness-demo-server:/root")
        .map_err(|_| "could not read server status".to_owned())?
    {
        let restored = restore_server_state(&saved.state)?;
        history = restored.history;
        jobs = restored.jobs;
        requests = restored.requests;
        envelopes = restored.envelopes;
        conversation = restored.conversation;
        status_store
            .save_session_state(
                "harness-demo-server:/root",
                &json!({"history":history,"jobs":jobs,"requests":requests,"envelopes":envelopes,"conversation":conversation}),
            )
            .map_err(|_| "could not persist recovered server status".to_owned())?;
    }
    control.set_snapshot(Snapshot {
        conversations: vec![conversation.clone()],
        requests: requests.clone(),
        jobs: jobs.clone(),
        envelopes: envelopes.clone(),
        ..Snapshot::default()
    });
    let driver = CliDriver::new(&CliOptions {
        db,
        ask: String::new(),
        dev_shell,
        tree: false,
        trace_jsonl: None,
        compact_at_input_tokens: None,
    })?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|_| "could not bind demo server address".to_owned())?;
    eprintln!(
        "harness-demo listening on {}",
        listener
            .local_addr()
            .map_err(|_| "could not read bound address")?
    );
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let mut server_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    let shutdown_signal = tokio::signal::ctrl_c();
    tokio::pin!(shutdown_signal);
    loop {
        tokio::select! {
            _ = &mut shutdown_signal => break,
            result = &mut server_task => {
                return result.map_err(|_| "server task failed".to_owned())?
                    .map_err(|_| "HTTP server failed".to_owned());
            }
            command = commands.recv() => {
                let Some(QueuedCommand { command_id, command }) = command else { break };
                match command {
                    ClientCommand::Submit { command } => {
                        let request_id = format!("request/{command_id}");
                        let job_id = command_id.clone();
                        let queued_job = job_record(&job_id, "running");
                        let queued_request = request_record(&request_id, "running");
                        conversation = conversation_record("requesting");
                        control.publish("conversation.upsert", conversation.clone());
                        control.publish("request.upsert", queued_request.clone());
                        control.publish("job.upsert", queued_job.clone());
                        jobs.push(queued_job);
                        requests.push(queued_request);
                        control.set_snapshot(Snapshot { conversations: vec![conversation.clone()], requests: requests.clone(), jobs: jobs.clone(), envelopes: envelopes.clone(), ..Snapshot::default() });

                        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
                        let turn_history = history.clone();
                        let mut turn = Box::pin(driver.ask(&command, &turn_history, cancel_rx));
                        let (status, request_state, answer, result_items, shutting_down) = tokio::select! {
                            result = &mut turn => match result {
                                Ok((answer, completion)) => ("settled", "completed", answer, Some(completion.transcript), false),
                                Err(error) => ("settled", "failed", error, None, false),
                            },
                            _ = &mut shutdown_signal => {
                                let _ = cancel_tx.send(true);
                                let result = turn.await;
                                let (state, request_state, answer, items) = match result {
                                    Ok((answer, completion)) => ("settled", "completed", answer, Some(completion.transcript)),
                                    Err(error) => ("cancelled", "failed", error, None),
                                };
                                (state, request_state, answer, items, true)
                            }
                        };
                        if let Some(items) = result_items {
                            history = items;
                        }
                        let envelope = (request_state == "completed")
                            .then(|| final_envelope(&format!("envelope/{command_id}"), &answer));
                        if let Some(envelope) = &envelope {
                            envelopes.push(envelope.clone());
                        }
                        conversation = conversation_record("idle");
                        let done_request = request_record(
                            &request_id,
                            request_state,
                        );
                        let done_job = job_record(&job_id, status);
                        replace_by_id(&mut requests, done_request.clone());
                        replace_by_id(&mut jobs, done_job.clone());
                        control.set_snapshot(Snapshot { conversations: vec![conversation.clone()], requests: requests.clone(), jobs: jobs.clone(), envelopes: envelopes.clone(), ..Snapshot::default() });
                        status_store
                            .save_session_state(
                                "harness-demo-server:/root",
                                &json!({"history":history,"jobs":jobs,"requests":requests,"envelopes":envelopes,"conversation":conversation}),
                            )
                            .map_err(|_| "could not persist server job status".to_owned())?;
                        control.publish("request.upsert", done_request);
                        control.publish("job.upsert", done_job);
                        if let Some(envelope) = envelope {
                            control.publish("envelope.upsert", envelope);
                        }
                        control.publish("conversation.upsert", conversation);
                        if shutting_down {
                            break;
                        }
                    }
                }
            }
        }
    }
    let _ = shutdown_tx.send(());
    let _ = server_task.await;
    Ok(())
}

fn replace_by_id(records: &mut Vec<Value>, replacement: Value) {
    let id = replacement.get("id").cloned();
    records.retain(|record| record.get("id") != id.as_ref());
    records.push(replacement);
}

fn final_text(items: &[Item]) -> Option<String> {
    items.iter().rev().find_map(|item| {
        if item.0["type"] != "message"
            || item.0["role"] != "assistant"
            || item.0["phase"] != "final_answer"
        {
            return None;
        }
        match &item.0["content"] {
            Value::String(text) => Some(text.clone()),
            Value::Array(parts) => {
                let text = parts
                    .iter()
                    .filter(|part| part["type"] == "output_text")
                    .filter_map(|part| part["text"].as_str())
                    .collect::<String>();
                (!text.is_empty()).then_some(text)
            }
            _ => None,
        }
    })
}

fn safe_transport_error(error: &TransportError) -> String {
    match error {
        harness::transport::TransportError::Authentication => {
            "authentication failed; check ~/.codex/auth.json (credentials were not displayed)"
                .into()
        }
        harness::transport::TransportError::Http(status) => {
            format!("API returned HTTP status {status}")
        }
        harness::transport::TransportError::Stream(_) => "API request or response failed".into(),
    }
}

fn safe_engine_error(error: EngineError) -> String {
    match &error {
        EngineError::Cancelled => "conversation cancelled during shutdown".into(),
        EngineError::Transport(error) => safe_transport_error(error),
        EngineError::Store(_) => "conversation store operation failed".into(),
        EngineError::Job(_) => "provider job failed".into(),
        _ => "conversation engine failed (details withheld)".into(),
    }
}

#[derive(Clone)]
pub struct DemoProvider {
    root: PathBuf,
    allow_shell: bool,
    owned: Vec<PathBuf>,
    trace_sink: Option<TraceSink>,
}

impl DemoProvider {
    /// Shell execution is opt-in; callers should use it only in a disposable
    /// development checkout with trusted tool input.
    pub fn development(root: impl Into<PathBuf>, allow_shell: bool) -> Self {
        Self {
            root: root.into(),
            allow_shell,
            owned: Vec::new(),
            trace_sink: None,
        }
    }

    /// Enable the opt-in, redacted job lifecycle trace for a manual tree run.
    pub fn with_trace(mut self, sink: TraceSink) -> Self {
        self.trace_sink = Some(sink);
        self
    }

    /// Authority comes from the host's admitted task contract, never tool
    /// arguments. An empty list (the default) denies all edits.
    pub fn with_owned_paths(mut self, owned: impl IntoIterator<Item = PathBuf>) -> Self {
        self.owned = owned.into_iter().collect();
        self
    }

    async fn run_command(&self, args: &Value) -> Result<Value, ProviderError> {
        if !self.allow_shell {
            return Err(ProviderError::Tool(
                "run is disabled; enable only for trusted development use".into(),
            ));
        }
        let script = string_arg(args, "script")?;
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .current_dir(&self.root)
            .output()
            .await
            .map_err(|e| ProviderError::Tool(format!("could not start shell: {e}")))?;
        let (stdout, stdout_truncated) = capped(&output.stdout);
        let (stderr, stderr_truncated) = capped(&output.stderr);
        Ok(json!({
            "status": output.status.code(),
            "success": output.status.success(),
            "stdout": stdout,
            "stderr": stderr,
            "output_truncated": stdout_truncated || stderr_truncated,
            "output_limit_bytes_per_stream": OUTPUT_LIMIT
        }))
    }

    fn edit(&self, args: &Value) -> Result<Value, ProviderError> {
        let rel = Path::new(string_arg(args, "path")?);
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(ProviderError::Tool(
                "path must be a relative, normalized path".into(),
            ));
        }
        let declared = self.owned.iter().any(|p| rel == p || rel.starts_with(p));
        if !declared {
            return Err(ProviderError::Tool("path is not declared owned".into()));
        }
        let root = self
            .root
            .canonicalize()
            .map_err(|e| ProviderError::Tool(format!("invalid provider root: {e}")))?;
        let target = root.join(rel);
        let parent = target
            .parent()
            .ok_or_else(|| ProviderError::Tool("invalid target".into()))?;
        let canonical_parent = parent
            .canonicalize()
            .map_err(|e| ProviderError::Tool(format!("invalid parent: {e}")))?;
        if !canonical_parent.starts_with(&root) {
            return Err(ProviderError::Tool("path escapes provider root".into()));
        }
        let canonical_target = target
            .canonicalize()
            .map_err(|e| ProviderError::Tool(format!("target must already exist: {e}")))?;
        if !canonical_target.starts_with(&root) || !canonical_target.is_file() {
            return Err(ProviderError::Tool(
                "target escapes root or is not a file".into(),
            ));
        }
        let old = std::fs::read_to_string(&canonical_target)
            .map_err(|e| ProviderError::Tool(format!("read failed: {e}")))?;
        let before = string_arg(args, "before")?;
        let after = string_arg(args, "after")?;
        if before.is_empty() || old.matches(before).count() != 1 {
            return Err(ProviderError::Tool("before must match exactly once".into()));
        }
        std::fs::write(&canonical_target, old.replacen(before, after, 1))
            .map_err(|e| ProviderError::Tool(format!("write failed: {e}")))?;
        Ok(json!({"path": rel, "edited": true}))
    }
}

fn string_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, ProviderError> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| ProviderError::Tool(format!("{key} must be a string")))
}

fn capped(bytes: &[u8]) -> (String, bool) {
    let end = bytes.len().min(OUTPUT_LIMIT);
    (
        String::from_utf8_lossy(&bytes[..end]).into_owned(),
        bytes.len() > OUTPUT_LIMIT,
    )
}

#[async_trait]
impl Provider for DemoProvider {
    async fn call(&self, name: &str, args: Value) -> Result<Value, ProviderError> {
        match name {
            "run" => self.run_command(&args).await,
            "edit" => self.edit(&args),
            "sleep" => {
                let ms = args
                    .get("duration_ms")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        ProviderError::Tool("duration_ms must be a nonnegative integer".into())
                    })?;
                tokio::time::sleep(Duration::from_millis(ms)).await;
                Ok(json!({"slept_ms": ms}))
            }
            "ask" => Ok(
                json!({"kind":"operator_form", "question":string_arg(&args,"question")?,
                "status":"not_connected", "note":"wire this request to the host operator"}),
            ),
            "form" => Ok(
                json!({"kind":"operator_form", "title":string_arg(&args,"title")?,
                "schema":args.get("schema").cloned().unwrap_or(Value::Null),
                "status":"not_connected", "note":"wire this request to the host operator"}),
            ),
            _ => Err(ProviderError::Tool(format!("unknown tool: {name}"))),
        }
    }

    async fn call_with_context(
        &self,
        name: &str,
        args: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        if name == "sleep" {
            let ms = args
                .get("duration_ms")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    ProviderError::Tool("duration_ms must be a nonnegative integer".into())
                })?;
            if let Some(sink) = &self.trace_sink {
                sink.record_job(JobEvent::SleepStarted {
                    call_id: context.call_id.0.clone(),
                    handle: context.handle.0.clone(),
                })
                .await
                .map_err(|_| ProviderError::Tool("trace recording failed".into()))?;
            }
            let _ = context.progress.send(
                json!({"event":"sleep_started","handle":context.handle.0.clone(),"duration_ms":ms}),
            );
            tokio::time::sleep(Duration::from_millis(ms)).await;
            if let Some(sink) = &self.trace_sink {
                sink.record_job(JobEvent::SleepSettled {
                    call_id: context.call_id.0,
                    handle: context.handle.0,
                    duration_ms: ms,
                })
                .await
                .map_err(|_| ProviderError::Tool("trace recording failed".into()))?;
            }
            return Ok(json!({"slept_ms":ms}));
        }
        self.call(name, args).await
    }

    // NOTE(correction-wave b): no per-tool `async` flags needed here; the
    // crate's `Provider::all_tools` stamps them. `sleep` is the item-2 tool.
    fn tools(&self) -> Vec<Value> {
        vec![
            json!({"type":"custom","name":"run","description":"DEV ONLY: execute trusted shell script locally"}),
            json!({"type":"function","name":"sleep","description":"Wait asynchronously","parameters":{"type":"object","properties":{"duration_ms":{"type":"integer","minimum":0}},"required":["duration_ms"],"additionalProperties":false},"strict":true}),
            json!({"type":"function","name":"edit","description":"Replace one exact string in an existing host-authorized owned file","parameters":{"type":"object","properties":{"path":{"type":"string"},"before":{"type":"string"},"after":{"type":"string"}},"required":["path","before","after"],"additionalProperties":false},"strict":true}),
            json!({"type":"function","name":"ask","description":"Request operator input (host integration required)","parameters":{"type":"object","properties":{"question":{"type":"string"}},"required":["question"],"additionalProperties":false},"strict":true}),
            json!({"type":"function","name":"form","description":"Request a schema-backed operator form (host integration required)","parameters":{"type":"object","properties":{"title":{"type":"string"},"schema":{"type":"object"}},"required":["title","schema"],"additionalProperties":false},"strict":true}),
        ]
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(Mode::Smoke) => {
            let provider = DemoProvider::development(".", false);
            let result = provider.call("sleep", json!({"duration_ms": 0})).await;
            println!("demo provider smoke: {result:?}");
        }
        Ok(Mode::Ask(options)) => {
            if options.tree {
                match tree_ask(&options).await {
                    Ok((text, completion)) => {
                        println!("{text}");
                        eprintln!(
                            "usage (final response): {} input / {} output tokens",
                            completion.turn.usage.input_tokens, completion.turn.usage.output_tokens
                        );
                    }
                    Err(error) => {
                        eprintln!("harness-demo: {error}");
                        std::process::exit(1);
                    }
                }
                return;
            }
            let driver = match CliDriver::new(&options) {
                Ok(driver) => driver,
                Err(error) => {
                    eprintln!("harness-demo: {error}");
                    std::process::exit(2);
                }
            };
            let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
            match driver.ask(&options.ask, &[], cancel_rx).await {
                Ok((text, completion)) => {
                    println!("{text}");
                    eprintln!(
                        "usage (final response): {} input / {} output tokens",
                        completion.turn.usage.input_tokens, completion.turn.usage.output_tokens
                    );
                }
                Err(error) => {
                    eprintln!("harness-demo: {error}");
                    std::process::exit(1);
                }
            }
        }
        Ok(Mode::Serve {
            db,
            addr,
            dev_shell,
        }) => {
            if let Err(error) = serve(db, addr, dev_shell).await {
                eprintln!("harness-demo: {error}");
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!(
                "harness-demo: {error}\nusage: harness-demo --db <sqlite-path> --ask <text> [--tree] [--dev-shell] | --db <sqlite-path> --serve <addr> | --smoke"
            );
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness::engine::ResponsesTransport;
    use harness::model::{AgentPath, CallId, Effort};
    use harness::server::{WsEvent, WsEventPayload, WsServerFrame};
    use harness::store::Store;
    use harness::transport::{Auth, ResponsesRequest, ResponsesTurn, TransportError, Usage};
    use harness::turn::JobScheduler;
    use tokio::sync::mpsc;

    #[derive(Clone)]
    struct FakeAuth;

    impl Auth for FakeAuth {
        fn access(&self) -> Result<(String, String), TransportError> {
            Err(TransportError::Authentication)
        }
    }

    struct FinalResponse;

    #[async_trait]
    impl ResponsesTransport for FinalResponse {
        async fn create(
            &self,
            _request: ResponsesRequest,
        ) -> Result<ResponsesTurn, TransportError> {
            Ok(ResponsesTurn {
                response_id: "offline-final".into(),
                items: vec![Item(json!({
                    "type":"message",
                    "role":"assistant",
                    "phase":"final_answer",
                    "content":[{"type":"output_text","text":"offline engine answer"}]
                }))],
                usage: Usage {
                    input_tokens: 12,
                    output_tokens: 4,
                    cached_tokens: 0,
                    cache_write_tokens: 0,
                },
            })
        }
    }

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!(
            "harness-demo-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn cli_arguments_require_db_and_nonempty_ask_and_gate_shell() {
        let parse =
            |args: &[&str]| parse_args(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert!(parse(&["--ask", "hello"]).unwrap_err().contains("--db"));
        assert!(
            parse(&["--db", "state.sqlite"])
                .unwrap_err()
                .contains("--ask")
        );
        assert!(parse(&["--db", "state.sqlite", "--ask", " "]).is_err());
        assert!(parse(&["--db", "state.sqlite", "--ask", "hi", "--allow-shell"]).is_err());
        assert_eq!(
            parse(&["--db", "state.sqlite", "--ask", "hi", "--dev-shell"]).unwrap(),
            Mode::Ask(CliOptions {
                db: PathBuf::from("state.sqlite"),
                ask: "hi".into(),
                dev_shell: true,
                tree: false,
                trace_jsonl: None,
                compact_at_input_tokens: None,
            })
        );
        assert!(matches!(
            parse(&["--db", "state.sqlite", "--ask", "hi", "--tree"]).unwrap(),
            Mode::Ask(CliOptions { tree: true, .. })
        ));
        assert!(matches!(
            parse(&[
                "--db",
                "state.sqlite",
                "--ask",
                "hi",
                "--tree",
                "--compact-at-input-tokens",
                "200000"
            ])
            .unwrap(),
            Mode::Ask(CliOptions {
                compact_at_input_tokens: Some(200000),
                ..
            })
        ));
        assert!(
            parse(&[
                "--db",
                "state.sqlite",
                "--ask",
                "hi",
                "--compact-at-input-tokens",
                "0"
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "--db",
                "state.sqlite",
                "--ask",
                "hi",
                "--trace-jsonl",
                "trace.jsonl"
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "--db",
                "state.sqlite",
                "--serve",
                "127.0.0.1:8000",
                "--tree",
                "--trace-jsonl",
                "trace.jsonl"
            ])
            .is_err()
        );
        assert!(matches!(
            parse(&[
                "--db",
                "state.sqlite",
                "--ask",
                "hi",
                "--tree",
                "--trace-jsonl",
                "trace.jsonl"
            ])
            .unwrap(),
            Mode::Ask(CliOptions {
                trace_jsonl: Some(_),
                ..
            })
        ));
        assert!(
            parse(&[
                "--db",
                "state.sqlite",
                "--serve",
                "127.0.0.1:8080",
                "--tree"
            ])
            .is_err()
        );
        assert!(matches!(parse(&["--smoke"]).unwrap(), Mode::Smoke));
        assert_eq!(
            parse(&["--db", "state.sqlite", "--serve", "127.0.0.1:8080"]).unwrap(),
            Mode::Serve {
                db: PathBuf::from("state.sqlite"),
                addr: "127.0.0.1:8080".parse().unwrap(),
                dev_shell: false,
            }
        );
        assert!(parse(&["--db", "state.sqlite", "--serve", "bad"]).is_err());
        assert!(
            parse(&["--db", "state.sqlite", "--serve", "0.0.0.0:8080"])
                .unwrap_err()
                .contains("loopback-only")
        );
        assert!(parse(&["--db", "state.sqlite", "--serve", "[::]:8080"]).is_err());
        assert!(parse(&["--db", "state.sqlite", "--serve", "[::1]:8080"]).is_ok());
        assert!(
            parse(&[
                "--db",
                "state.sqlite",
                "--serve",
                "127.0.0.1:8080",
                "--ask",
                "x"
            ])
            .is_err()
        );
    }

    #[test]
    fn serving_requires_built_web_assets() {
        let root = temp();
        assert!(ensure_asset_root(&root).is_ok());
        let missing = root.join("not-built");
        assert!(
            ensure_asset_root(&missing)
                .unwrap_err()
                .contains("build web/dist")
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn browser_secret_validation_fails_closed() {
        assert!(SessionSecret::new("short").is_err());
        assert!(SessionSecret::new("this-is-a-long-enough-test-secret-value").is_ok());
    }

    #[test]
    fn server_state_matches_web_contract_and_event_kinds() {
        let conversation = conversation_record("idle");
        let request = request_record("r1", "completed");
        let job = job_record("j1", "settled");
        let envelope = final_envelope("e1", "final answer");
        assert_eq!(
            conversation,
            json!({"id":"conversation/root","path":"/root","state":"idle"})
        );
        assert_eq!(
            request,
            json!({"id":"r1","conversationId":"conversation/root","state":"completed"})
        );
        assert_eq!(
            job,
            json!({"id":"j1","conversationId":"conversation/root","state":"settled"})
        );
        assert_eq!(
            envelope,
            json!({"id":"e1","conversationId":"conversation/root","recipient":"/root","sender":"/root","type":"FINAL_ANSWER","payload":"final answer"})
        );
        let frame = WsServerFrame::Event {
            event: WsEvent {
                seq: 1,
                event: WsEventPayload {
                    kind: "job.upsert".into(),
                    value: job,
                },
            },
        };
        assert_eq!(
            serde_json::to_value(frame).unwrap(),
            json!({"type":"event","event":{"seq":1,"event":{"kind":"job.upsert","value":{
                "id":"j1","conversationId":"conversation/root","state":"settled"
            }}}})
        );
        assert_eq!(envelope["type"], "FINAL_ANSWER");
        assert_eq!(
            serde_json::to_value(Snapshot {
                seq: 5,
                conversations: vec![conversation],
                requests: vec![request],
                jobs: vec![job_record("j1", "settled")],
                envelopes: vec![envelope],
            })
            .unwrap(),
            json!({
                "seq":5,
                "conversations":[{"id":"conversation/root","path":"/root","state":"idle"}],
                "requests":[{"id":"r1","conversationId":"conversation/root","state":"completed"}],
                "jobs":[{"id":"j1","conversationId":"conversation/root","state":"settled"}],
                "envelopes":[{"id":"e1","conversationId":"conversation/root","recipient":"/root","sender":"/root","type":"FINAL_ANSWER","payload":"final answer"}]
            })
        );
    }

    #[test]
    fn next_server_command_uses_complete_recorded_tool_transcript_once() {
        let recorded_completion = vec![
            Item(json!({"type":"message","role":"user","content":"first task"})),
            Item(json!({"type":"function_call","call_id":"call-1","name":"echo","arguments":"{}"})),
            Item(json!({"type":"function_call_output","call_id":"call-1","output":"tool result"})),
            Item(
                json!({"type":"function_call","call_id":"call-2","name":"echo","arguments":"{\"next\":true}"}),
            ),
            Item(
                json!({"type":"function_call_output","call_id":"call-2","output":"second tool result"}),
            ),
            Item(
                json!({"type":"message","role":"assistant","phase":"final_answer","content":"first answer"}),
            ),
        ];
        let next_input = command_input(&recorded_completion, "follow-up");
        assert_eq!(next_input.len(), recorded_completion.len() + 1);
        assert_eq!(
            &next_input[..recorded_completion.len()],
            recorded_completion
        );
        assert_eq!(
            next_input
                .iter()
                .filter(|item| item.0["type"] == "function_call")
                .count(),
            2
        );
        assert_eq!(
            next_input
                .iter()
                .filter(|item| item.0["type"] == "function_call_output")
                .count(),
            2
        );
        assert_eq!(next_input.last().unwrap().0["content"], "follow-up");
    }

    #[test]
    fn malformed_persisted_state_fails_and_running_records_are_interrupted() {
        assert!(restore_server_state("{").is_err());
        assert!(
            restore_server_state(r#"{"jobs":[]}"#)
                .unwrap_err()
                .contains("history")
        );
        let restored = restore_server_state(
            r#"{"history":[],"jobs":[{"id":"j1","conversationId":"conversation/root","state":"running"}],"requests":[{"id":"r1","conversationId":"conversation/root","state":"running"}],"envelopes":[],"conversation":{"id":"conversation/root","path":"/root","state":"requesting"}}"#,
        )
        .unwrap();
        assert_eq!(restored.jobs[0]["state"], "cancelled");
        assert_eq!(restored.jobs[0]["status"], "Interrupted");
        assert_eq!(restored.jobs[0]["interrupted"], true);
        assert_eq!(restored.requests[0]["state"], "failed");
        assert_eq!(restored.conversation["state"], "idle");
    }

    #[test]
    fn driver_constructs_offline_without_reading_credentials_or_enabling_edit() {
        let root = temp();
        let options = CliOptions {
            db: root.join("state.sqlite"),
            ask: "hello".into(),
            dev_shell: false,
            tree: false,
            trace_jsonl: None,
            compact_at_input_tokens: None,
        };
        let _driver = CliDriver::new(&options).unwrap();
        let provider = CliProvider(DemoProvider::development(".", false));
        assert!(provider.all_tools().iter().any(|t| t["name"] == "edit"));
        assert!(!provider.all_tools().iter().any(|t| t["name"] == "ask"));
        assert!(!provider.all_tools().iter().any(|t| t["name"] == "form"));
        assert!(
            !provider
                .all_tools()
                .iter()
                .any(|t| t["name"] == "spawn_agent")
        );
        assert!(!provider.all_tools().iter().any(|t| t["name"] == "run"));
        let dev_provider = CliProvider(DemoProvider::development(".", true));
        let run = dev_provider
            .all_tools()
            .into_iter()
            .find(|t| t["name"] == "run")
            .unwrap();
        assert_eq!(run["type"], "function");
        assert_eq!(run["strict"], true);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn extracts_only_final_assistant_text() {
        let final_item = Item(
            json!({"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"answer"}]}),
        );
        assert_eq!(
            final_text(std::slice::from_ref(&final_item)).as_deref(),
            Some("answer")
        );
        let multipart = Item(
            json!({"type":"message","role":"assistant","phase":"final_answer","content":[
                {"type":"output_text","text":"one"},
                {"type":"output_text","text":" two"}
            ]}),
        );
        assert_eq!(final_text(&[multipart]).as_deref(), Some("one two"));
        assert_eq!(
            final_text(&[Item(
                json!({"type":"message","role":"assistant","phase":"commentary","content":"not final"})
            )]),
            None
        );
    }

    #[tokio::test]
    async fn engine_runs_and_cli_extracts_final_text_offline() {
        let root = temp();
        let provider = Arc::new(CliProvider(DemoProvider::development(&root, false)));
        let engine = Engine::<FakeAuth, CliProvider, _>::with_transport(
            FinalResponse,
            Arc::new(Store::memory().unwrap()),
            Arc::new(JobScheduler::new(2).unwrap()),
            provider,
            EngineConfig {
                instructions: "offline test".into(),
                tools: Vec::new(),
                model: "test-model".into(),
                effort: Effort::Low,
                session_id: "offline-test".into(),
                agent: AgentPath("/root".into()),
            },
        );
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let turn = engine
            .run(
                None,
                vec![Item(json!({"role":"user","content":"hello"}))],
                cancel_rx,
                tokio::sync::mpsc::unbounded_channel().1,
            )
            .await
            .unwrap();
        assert_eq!(
            final_text(&turn.turn.items).as_deref(),
            Some("offline engine answer")
        );
        assert_eq!(turn.turn.usage.input_tokens, 12);
        assert_eq!(turn.turn.usage.output_tokens, 4);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn sleep_is_async_and_returns_output() {
        let provider = DemoProvider::development(".", false);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ctx = CallContext {
            handle: harness::provider::JobHandle("sleep-1".into()),
            call_id: CallId("c1".into()),
            agent: harness::model::AgentPath("/root".into()),
            request: None,
            progress: tx,
        };
        let value = provider
            .call_with_context("sleep", json!({"duration_ms": 2}), ctx)
            .await
            .unwrap();
        assert_eq!(value, json!({"slept_ms":2}));
        assert_eq!(rx.recv().await.unwrap()["event"], "sleep_started");
    }

    #[tokio::test]
    async fn traced_sleep_records_start_and_settlement_without_raw_ids() {
        let root = temp();
        let path = root.join("sleep.jsonl");
        let sink = TraceSink::open(path.clone()).await.unwrap();
        let provider = DemoProvider::development(".", false).with_trace(sink.clone());
        let (progress, _) = mpsc::unbounded_channel();
        let context = CallContext {
            handle: harness::provider::JobHandle("private-handle".into()),
            call_id: CallId("private-call-id".into()),
            agent: harness::model::AgentPath("/root".into()),
            request: None,
            progress,
        };
        assert_eq!(
            provider
                .call_with_context("sleep", json!({"duration_ms": 1}), context)
                .await
                .unwrap(),
            json!({"slept_ms": 1})
        );
        sink.flush().await.unwrap();
        let raw = std::fs::read_to_string(path).unwrap();
        assert!(!raw.contains("private-handle"));
        assert!(!raw.contains("private-call-id"));
        let events = raw
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["event"], "sleep_started");
        assert_eq!(events[1]["event"], "sleep_settled");
        assert_eq!(events[0]["handle_hash"], events[1]["handle_hash"]);
        assert_eq!(events[0]["call_id_hash"], events[1]["call_id_hash"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn trace_flush_runs_on_forced_start_failure() {
        let root = temp();
        let path = root.join("failed-start.jsonl");
        let sink = TraceSink::open(path.clone()).await.unwrap();
        sink.record_job(JobEvent::SleepStarted {
            call_id: "private-call".into(),
            handle: "private-handle".into(),
        })
        .await
        .unwrap();
        let outcome: Result<(), String> =
            flush_trace_result(Some(&sink), Err("forced driver.start failure".into())).await;
        assert_eq!(outcome.unwrap_err(), "forced driver.start failure");
        let raw = std::fs::read_to_string(path).unwrap();
        assert_eq!(raw.lines().count(), 1);
        assert!(!raw.contains("private-call"));
        assert!(!raw.contains("private-handle"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn edit_requires_owned_path_and_stays_inside_root() {
        let root = temp();
        std::fs::write(root.join("ok.txt"), "old").unwrap();
        let provider = DemoProvider::development(&root, false);
        let outside = root.parent().unwrap().join("outside-demo.txt");
        std::fs::write(&outside, "safe").unwrap();
        // Forged `owned` is ignored; only constructor-supplied authority counts.
        assert!(
            provider
                .call(
                    "edit",
                    json!({"path":"ok.txt","owned":["ok.txt"],"before":"old","after":"new"})
                )
                .await
                .is_err()
        );
        let authorized =
            DemoProvider::development(&root, false).with_owned_paths([PathBuf::from("ok.txt")]);
        assert!(
            authorized
                .call(
                    "edit",
                    json!({"path":"../outside-demo.txt","before":"safe","after":"bad"})
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "safe");
        assert_eq!(
            authorized
                .call(
                    "edit",
                    json!({"path":"ok.txt","before":"old","after":"new"})
                )
                .await
                .unwrap()["edited"],
            true
        );
        assert_eq!(std::fs::read_to_string(root.join("ok.txt")).unwrap(), "new");
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(outside);
    }

    #[tokio::test]
    async fn function_tools_are_strict_and_shell_output_is_capped() {
        let provider = DemoProvider::development(".", true);
        let functions: Vec<_> = provider
            .tools()
            .into_iter()
            .filter(|t| t["type"] == "function")
            .collect();
        assert_eq!(functions.len(), 4);
        for tool in functions {
            assert_eq!(tool["strict"], true);
            assert_eq!(tool["parameters"]["additionalProperties"], false);
        }
        let result = provider
            .call("run", json!({"script":"yes x | head -c 40000"}))
            .await
            .unwrap();
        assert_eq!(result["stdout"].as_str().unwrap().len(), OUTPUT_LIMIT);
        assert_eq!(result["output_truncated"], true);
    }
}
