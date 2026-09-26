//! Reference provider for the harness. `run` intentionally invokes a local
//! shell and is suitable only for trusted, development-time demonstrations.
pub mod driver;
#[cfg(test)]
mod process_restart_tests;
mod trace;
pub mod tree;

use async_trait::async_trait;
use driver::{Driver, HarnessEngineFactory};
use harness::agent_runtime::StoreAgentToolService;
use harness::agents::{AgentToolService, Contract, SpawnSource};
use harness::engine::{Engine, EngineCompletion, EngineConfig, EngineError};
use harness::item::Item;
use harness::model::{AgentPath, Effort};
use harness::provider::{CallContext, Provider, ProviderError};
use harness::server::{self, ClientCommand, QueuedCommand, ServerConfig, SessionSecret, Snapshot};
use harness::store::Store;
use harness::transport::{
    Auth, ResponsesClient, ResponsesRequest, ResponsesTurn, TransportError, Usage,
    auth::CodexFileAuth,
};
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
    assets: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Smoke,
    Ask(CliOptions),
    Serve {
        db: PathBuf,
        addr: SocketAddr,
        dev_shell: bool,
        assets: Option<PathBuf>,
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
        mut assets,
    ) = (None, None, None, false, false, None, None, None);
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
            "--assets" => {
                i += 1;
                let path = PathBuf::from(args.get(i).ok_or("--assets requires an absolute path")?);
                if !path.is_absolute() {
                    return Err("--assets requires an absolute path".into());
                }
                assets = Some(path);
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
    if assets.is_some() && serve.is_none() {
        return Err("--assets is serve-only".into());
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
            assets,
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
        assets: None,
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
    async fn before_request(
        &self,
        plan: &harness::hooks::RequestPlan,
    ) -> harness::hooks::BeforeRequestResult {
        self.0.before_request(plan).await
    }

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

fn command_request_record(
    id: &str,
    state: &str,
    command_id: &str,
    command: &str,
    outcome: &str,
    detail: Option<&str>,
) -> Value {
    let mut record = request_record(id, state);
    record["commandId"] = json!(command_id);
    record["command"] = json!(command);
    record["outcome"] = json!(outcome);
    if let Some(detail) = detail {
        record["detail"] = json!(detail);
    }
    record
}

fn next_envelope_ordinal(envelopes: &[Value]) -> u64 {
    envelopes
        .iter()
        .filter_map(|envelope| envelope["ordinal"].as_u64())
        .max()
        .unwrap_or(0)
        + 1
}

fn active_wait_request(requests: &[Value], history: &[Item]) -> Option<String> {
    requests.iter().find_map(|request| {
        if request["state"] != "running" {
            return None;
        }
        let request_id = request["id"].as_str()?;
        history
            .iter()
            .any(|item| item.0["request_id"] == request_id && item.0["command"] == "wait")
            .then(|| request_id.to_owned())
    })
}

fn job_record(id: &str, state: &str) -> Value {
    json!({"id":id,"conversationId":ROOT_CONVERSATION_ID,"state":state})
}

fn conversation_rows(root: &Value, envelopes: &[Value]) -> Vec<Value> {
    let mut rows = vec![root.clone()];
    let mut paths = std::collections::BTreeSet::new();
    for envelope in envelopes {
        for key in ["sender", "recipient"] {
            if let Some(path) = envelope[key]
                .as_str()
                .filter(|path| path.starts_with("/root/"))
            {
                paths.insert(path.to_owned());
            }
        }
    }
    rows.extend(
        paths
            .into_iter()
            .map(|path| json!({"id":format!("conversation/{path}"),"path":path,"state":"idle"})),
    );
    rows
}

struct AbortTasksOnDrop(Arc<std::sync::Mutex<Vec<tokio::task::AbortHandle>>>);

impl Drop for AbortTasksOnDrop {
    fn drop(&mut self) {
        let handles = self.0.lock().unwrap_or_else(|poison| poison.into_inner());
        for handle in handles.iter() {
            handle.abort();
        }
    }
}

#[cfg(test)]
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

fn deterministic_command(
    command: &str,
    waiting: bool,
) -> (&'static str, &'static str, Option<String>) {
    if command == "wait" {
        return if waiting {
            (
                "failed",
                "already pending",
                Some("A wait is already pending.".into()),
            )
        } else {
            ("running", "pending", None)
        };
    }
    if command == "cancel" && waiting {
        return (
            "settled",
            "completed",
            Some("Pending wait cancelled.".into()),
        );
    }
    if let Some(text) = command.strip_prefix("echo ") {
        return ("settled", "completed", Some(text.into()));
    }
    if command == "test" {
        return (
            "settled",
            "completed",
            Some("Deterministic check passed.".into()),
        );
    }
    if let Some(text) = command.strip_prefix("message ") {
        return (
            "settled",
            if waiting { "queued" } else { "no pending wait" },
            Some(if waiting {
                format!("Message queued: {text}")
            } else {
                "No pending wait; message not queued.".into()
            }),
        );
    }
    if let Some(text) = command.strip_prefix("child ") {
        return (
            "settled",
            "completed",
            Some(format!(
                "Child /root/demo-child received and replied: {text}"
            )),
        );
    }
    if command == "child-reply" || command.starts_with("child-reply ") {
        let text = command.strip_prefix("child-reply ").unwrap_or_default();
        return (
            "settled",
            "completed",
            Some(format!("Child received and replied: {text}")),
        );
    }
    if command == "fail" {
        return (
            "failed",
            "failed",
            Some("Controlled deterministic failure.".into()),
        );
    }
    (
        "failed",
        "failed",
        Some("Unknown deterministic command.".into()),
    )
}

#[derive(Clone)]
struct OfflineServerAuth;

impl Auth for OfflineServerAuth {
    fn access(&self) -> Result<(String, String), TransportError> {
        Err(TransportError::Authentication)
    }
}

struct DeterministicServerTransport {
    command: String,
    waiting: bool,
    answer_override: Option<String>,
}

#[async_trait]
impl harness::engine::ResponsesTransport for DeterministicServerTransport {
    async fn create(&self, request: ResponsesRequest) -> Result<ResponsesTurn, TransportError> {
        if let Some(path) = std::env::var_os("HARNESS_DEMO_CAPTURE_REQUESTS") {
            capture_deterministic_request(&request, Path::new(&path))?;
        }
        let (state, _outcome, answer) = deterministic_command(&self.command, self.waiting);
        if self.command == "wait" && state == "running" {
            return std::future::pending().await;
        }
        if state == "failed" {
            return Err(TransportError::Stream(
                "controlled deterministic server failure".into(),
            ));
        }
        let inbox_reply = (self.command == "child-reply")
            .then(|| {
                request.input.iter().find_map(|item| {
                    let text = item.0["content"]
                        .as_array()?
                        .iter()
                        .filter_map(|part| part["text"].as_str())
                        .collect::<String>();
                    if !text.contains("Message Type: MESSAGE") {
                        return None;
                    }
                    text.split_once("Payload:\n")
                        .map(|(_, payload)| format!("Child received and replied: {payload}"))
                })
            })
            .flatten();
        if self.command == "child-reply" && inbox_reply.is_none() {
            return Err(TransportError::Stream(
                "child had no delivered parent message".into(),
            ));
        }
        let answer = inbox_reply
            .or_else(|| self.answer_override.clone())
            .or(answer)
            .unwrap_or_default();
        Ok(ResponsesTurn {
            response_id: format!("deterministic-{}", request.session_id),
            items: vec![Item(json!({
                "type":"message","role":"assistant","phase":"final_answer",
                "content":[{"type":"output_text","text":answer}]
            }))],
            usage: Usage::default(),
        })
    }
}

/// Append the production transport's serialized body as one JSONL record.
/// The caller owns the path and must provide an isolated capture destination.
fn capture_deterministic_request(
    request: &ResponsesRequest,
    path: &Path,
) -> Result<(), TransportError> {
    use std::io::Write;
    let body = harness::transport::client::request_body(request)?;
    let mut record = serde_json::to_vec(&body).map_err(|error| {
        TransportError::Stream(format!("request capture serialization failed: {error}"))
    })?;
    record.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| TransportError::Stream(format!("request capture open failed: {error}")))?;
    file.write_all(&record).map_err(|error| {
        TransportError::Stream(format!("request capture write failed: {error}"))
    })?;
    Ok(())
}

async fn run_deterministic_engine_completion(
    store: Arc<Store>,
    scheduler: Arc<JobScheduler>,
    command_id: &str,
    command: &str,
    waiting: bool,
    cancel_rx: tokio::sync::watch::Receiver<bool>,
    agent: AgentPath,
    answer_override: Option<String>,
) -> Result<EngineCompletion, String> {
    let provider = Arc::new(BrowserProvider(CliProvider(DemoProvider::development(
        ".", false,
    ))));
    let engine = Engine::<OfflineServerAuth, BrowserProvider, _>::with_transport(
        DeterministicServerTransport {
            command: command.to_owned(),
            waiting,
            answer_override,
        },
        store,
        scheduler,
        provider.clone(),
        EngineConfig {
            instructions: "Deterministic browser journey. Do not use tools.".into(),
            tools: Vec::new(),
            model: "deterministic-local".into(),
            effort: Effort::Low,
            session_id: format!("harness-demo-server-{command_id}"),
            agent,
        },
    );
    engine
        .run(
            None,
            command_input(&[], command),
            cancel_rx,
            tokio::sync::mpsc::unbounded_channel().1,
        )
        .await
        .map_err(|_| "deterministic engine request failed".to_owned())
}

/// Browser-only deterministic policy. The shared CLI provider remains Send/auto.
struct BrowserProvider(CliProvider);

#[async_trait]
impl Provider for BrowserProvider {
    async fn before_request(
        &self,
        plan: &harness::hooks::RequestPlan,
    ) -> harness::hooks::BeforeRequestResult {
        let mut result = self.0.before_request(plan).await;
        let current_command = plan.items.iter().rev().find_map(|item| {
            let value = &item.0;
            (value["type"] == "message" && value["role"] == "user")
                .then(|| value["content"].as_str())
                .flatten()
        });
        let advertised = plan
            .tools_allowed
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();
        let sleep_advertised = advertised.contains(&"sleep");
        let (selection, label) =
            if current_command.is_some_and(|command| command.starts_with("child ")) {
                (Some(Vec::new()), "child-empty")
            } else if current_command.is_some_and(|command| command.starts_with("echo ")) {
                // Always request the intended selection. Engine validation must
                // reject it as an invalid selection if the final plan omits sleep;
                // never silently widen availability to Send.
                (Some(vec!["sleep".to_owned()]), "echo-sleep")
            } else {
                (None, "send")
            };
        if let Some(tools_allowed) = selection {
            result.decision =
                harness::hooks::BeforeRequestDecision::SendRestricted { tools_allowed };
        }
        result.evidence = Some(json!({
            "consumer":"standalone-browser",
            "selection":label,
            "sleep_advertised":sleep_advertised
        }));
        result
    }
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
        self.0.tools()
    }
    fn all_tools(&self) -> Vec<Value> {
        self.0.all_tools()
    }
}

async fn run_deterministic_engine_turn(
    store: Arc<Store>,
    scheduler: Arc<JobScheduler>,
    command_id: &str,
    command: &str,
    waiting: bool,
    cancel_rx: tokio::sync::watch::Receiver<bool>,
    agent: AgentPath,
    answer_override: Option<String>,
) -> Result<(String, Item), String> {
    let completion = run_deterministic_engine_completion(
        store,
        scheduler,
        command_id,
        command,
        waiting,
        cancel_rx,
        agent,
        answer_override,
    )
    .await?;
    let answer = final_text(&completion.turn.items)
        .ok_or_else(|| "deterministic engine returned no final text".to_owned())?;
    let final_item = completion
        .turn
        .items
        .iter()
        .rev()
        .find(|item| {
            item.0["type"] == "message"
                && item.0["role"] == "assistant"
                && item.0["phase"] == "final_answer"
        })
        .cloned()
        .ok_or_else(|| "deterministic Engine final item is missing".to_owned())?;
    Ok((answer, final_item))
}

async fn run_deterministic_child_message(
    store: Arc<Store>,
    scheduler: Arc<JobScheduler>,
    command_id: &str,
    message: &str,
) -> Result<(AgentPath, String, Value), String> {
    let root = AgentPath(ROOT_PATH.into());
    let root_exists = store
        .agent(&root)
        .map_err(|_| "could not inspect root agent")?
        .is_some();
    if !root_exists {
        store
            .admit_agent(&root, None, None, &json!({}), &json!({"kind":"root"}))
            .map_err(|_| "could not register root agent")?;
    }
    let service = StoreAgentToolService::new(store.clone(), root.clone());
    let mut suffix = command_id
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        .take(32)
        .collect::<String>();
    if suffix.is_empty() {
        suffix = "request".into();
    }
    let child_name = format!("demo_child_{suffix}");
    let contract = Contract {
        clauses: vec!["Receive the parent's message and reply deterministically.".into()],
        acceptance: vec!["A final answer is durably published to the parent.".into()],
        owned: vec![],
        must_not: vec!["Use remote inference or shell tools.".into()],
        introduces: vec![],
        consumes: vec![],
        boundaries: vec![],
        reply: None,
    };
    let created = service
        .spawn_agent(&root, &child_name, SpawnSource::Prompt, contract)
        .await
        .map_err(|_| "could not create deterministic child agent")?;
    let child = AgentPath(
        created["task_name"]
            .as_str()
            .ok_or("child service returned no identity")?
            .to_owned(),
    );
    service
        .send_message(&root, child.clone(), message.to_owned())
        .await
        .map_err(|_| "could not deliver parent message to child")?;
    let child_command = "child-reply".to_owned();
    let completion = run_deterministic_engine_completion(
        store.clone(),
        scheduler,
        &format!("{command_id}-child"),
        &child_command,
        false,
        tokio::sync::watch::channel(false).1,
        child.clone(),
        None,
    )
    .await?;
    let reply = final_text(&completion.turn.items)
        .ok_or_else(|| "child Engine returned no final answer".to_owned())?;
    service
        .send_message(&child, root.clone(), reply.clone())
        .await
        .map_err(|_| "could not persist child reply message")?;
    let stored = store
        .inbox(&root.0)
        .map_err(|_| "could not read root inbox")?
        .into_iter()
        .rev()
        .find(|entry| entry.sender == child.0 && entry.recipient == root.0)
        .ok_or_else(|| "committed child response is missing from parent inbox".to_owned())?;
    let item = store
        .get_item(&stored.item_hash)
        .map_err(|_| "could not read committed child response")?
        .ok_or_else(|| "committed child response item is missing".to_owned())?;
    let stored_text = item.0["content"][0]["text"]
        .as_str()
        .ok_or_else(|| "committed child message is malformed".to_owned())?;
    let reply = stored_text
        .split_once("Payload:\n")
        .map(|(_, payload)| payload)
        .ok_or_else(|| "committed child message has no payload".to_owned())?
        .to_owned();
    let envelope = json!({
        "id":format!("envelope/{}", stored.id),
        "conversationId":ROOT_CONVERSATION_ID,
        "recipient":stored.recipient,
        "sender":stored.sender,
        "type":"MESSAGE",
        "payload":reply
    });
    Ok((child, reply, envelope))
}

fn persist_browser_envelope(
    store: &Store,
    sender: &str,
    recipient: &str,
    kind: &str,
    payload: &str,
    ordinal: u64,
) -> Result<Value, String> {
    let role = if sender == "/operator" {
        "user"
    } else {
        "assistant"
    };
    let item = Item(json!({
        "type":"message",
        "role":role,
        "content":[{"type":"output_text","text":payload}]
    }));
    let id = store
        .add_envelope(sender, recipient, "AtBoundary", &item, None)
        .map_err(|_| "could not persist browser message envelope")?;
    Ok(json!({
        "id":format!("envelope/{id}"),
        "conversationId":if recipient.starts_with("/root/") { format!("conversation/{recipient}") } else { ROOT_CONVERSATION_ID.to_owned() },
        "recipient":recipient,
        "sender":sender,
        "type":kind,
        "payload":payload,
        "ordinal":ordinal
    }))
}

fn persist_engine_final_envelope(
    store: &Store,
    item: &Item,
    ordinal: u64,
) -> Result<Value, String> {
    let answer = final_text(std::slice::from_ref(item))
        .ok_or_else(|| "Engine final item is not a final answer".to_owned())?;
    let id = store
        .add_envelope(ROOT_PATH, "/operator", "AtBoundary", item, None)
        .map_err(|_| "could not persist Engine final answer envelope")?;
    Ok(json!({
        "id":format!("envelope/{id}"),
        "conversationId":ROOT_CONVERSATION_ID,
        "recipient":"/operator",
        "sender":ROOT_PATH,
        "type":"FINAL_ANSWER",
        "payload":answer,
        "ordinal":ordinal
    }))
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
            if request.get("commandId").is_some() {
                request["outcome"] = json!("failed");
                request["detail"] = json!("Process restarted before this request settled.");
            }
        }
    }
    let envelopes = records("envelopes")?;
    let mut last_ordinal = 0;
    for envelope in &envelopes {
        if envelope.get("id").and_then(Value::as_str).is_none()
            || !envelope
                .get("conversationId")
                .and_then(Value::as_str)
                .is_some_and(|id| {
                    id == ROOT_CONVERSATION_ID || id.starts_with("conversation//root/")
                })
            || envelope.get("sender").and_then(Value::as_str).is_none()
            || envelope.get("recipient").and_then(Value::as_str).is_none()
            || envelope.get("payload").and_then(Value::as_str).is_none()
            || !matches!(
                envelope.get("type").and_then(Value::as_str),
                Some("PROGRESS" | "MESSAGE" | "FINAL_ANSWER")
            )
        {
            return Err("persisted demo server envelope is malformed".into());
        }
        if let Some(ordinal) = envelope["ordinal"].as_u64() {
            if ordinal <= last_ordinal {
                return Err("persisted demo server envelope ordinals are not monotone".into());
            }
            last_ordinal = ordinal;
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

async fn serve(
    db: PathBuf,
    addr: SocketAddr,
    dev_shell: bool,
    assets: Option<PathBuf>,
) -> Result<(), String> {
    if !addr.ip().is_loopback() {
        return Err(
            "--serve is loopback-only because browser-session login uses plain HTTP".into(),
        );
    }
    let cwd = std::env::current_dir().map_err(|_| "could not determine current directory")?;
    let db = if db.is_absolute() { db } else { cwd.join(db) };
    let asset_root = assets.unwrap_or_else(|| cwd.join("web/dist"));
    ensure_asset_root(&asset_root)?;
    let secret = std::env::var("HARNESS_DEMO_SESSION_SECRET")
        .map_err(|_| "HARNESS_DEMO_SESSION_SECRET is required".to_owned())?;
    let secret = SessionSecret::new(secret)?;
    let config = ServerConfig::new(asset_root)
        .with_browser_session(secret, Duration::from_secs(8 * 60 * 60))?;
    let (app, control, mut commands) = server::server_with_config(config);
    let status_store =
        Arc::new(Store::open(&db).map_err(|_| "could not open server status store".to_owned())?);
    let root_agent = AgentPath(ROOT_PATH.into());
    if status_store
        .agent(&root_agent)
        .map_err(|_| "could not inspect server root agent")?
        .is_none()
    {
        status_store
            .admit_agent(&root_agent, None, None, &json!({}), &json!({"kind":"root"}))
            .map_err(|_| "could not register server root agent")?;
    }
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
        conversations: conversation_rows(&conversation, &envelopes),
        requests: requests.clone(),
        jobs: jobs.clone(),
        envelopes: envelopes.clone(),
        ..Snapshot::default()
    });
    let scheduler = Arc::new(
        JobScheduler::new(4)
            .map_err(|_| "could not initialize server tool scheduler".to_owned())?,
    );
    // --serve is the credential-free deterministic browser journey. The
    // interactive --ask path remains backed by CliDriver/Engine; browser
    // commands must never silently turn into model requests.
    let _ = dev_shell;
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
    let task_aborts = Arc::new(std::sync::Mutex::new(Vec::new()));
    let _task_cleanup = AbortTasksOnDrop(task_aborts.clone());
    let mut server_task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    task_aborts
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push(server_task.abort_handle());
    let shutdown_signal = tokio::signal::ctrl_c();
    tokio::pin!(shutdown_signal);
    let mut pending_engine: Option<(
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<Result<(String, Item), String>>,
    )> = None;
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
                        let conversation_was_requesting = conversation["state"] == "requesting";
                        let request_id = format!("request/{command_id}");
                        let job_id = command_id.clone();
                        let queued_job = job_record(&job_id, "running");
                        let queued_request = command_request_record(
                            &request_id, "running", &command_id, &command, "accepted", None,
                        );
                        jobs.push(queued_job);
                        requests.push(queued_request);
                        conversation = conversation_record("requesting");
                        status_store
                            .save_session_state(
                                "harness-demo-server:/root",
                                &json!({"history":history,"jobs":jobs,"requests":requests,"envelopes":envelopes,"conversation":conversation}),
                            )
                            .map_err(|_| "could not persist accepted server command".to_owned())?;
                        control.set_snapshot(Snapshot { conversations: conversation_rows(&conversation, &envelopes), requests: requests.clone(), jobs: jobs.clone(), envelopes: envelopes.clone(), ..Snapshot::default() });
                        if !conversation_was_requesting {
                            control.publish("conversation.upsert", conversation.clone());
                        }
                        control.publish("request.upsert", requests.last().unwrap().clone());
                        control.publish("job.upsert", jobs.last().unwrap().clone());
                        let waiting = active_wait_request(&requests, &history).is_some();
                        let (initial_state, initial_outcome, fallback_answer) =
                            deterministic_command(&command, waiting);
                        if command == "wait" && initial_state == "running" {
                            let pending_record = command_request_record(
                                &request_id,
                                "running",
                                &command_id,
                                &command,
                                "pending",
                                Some("Waiting for a cancel command."),
                            );
                            replace_by_id(&mut requests, pending_record.clone());
                            status_store
                                .save_session_state(
                                    "harness-demo-server:/root",
                                    &json!({"history":history,"jobs":jobs,"requests":requests,"envelopes":envelopes,"conversation":conversation}),
                                )
                                .map_err(|_| "could not persist pending wait")?;
                            control.set_snapshot(Snapshot { conversations: conversation_rows(&conversation, &envelopes), requests: requests.clone(), jobs: jobs.clone(), envelopes: envelopes.clone(), ..Snapshot::default() });
                            control.publish("request.upsert", pending_record);
                        }
                        if command.starts_with("message ") && waiting {
                            let text = command.strip_prefix("message ").unwrap_or_default();
                            let message = persist_browser_envelope(
                                &status_store,
                                "/operator",
                                ROOT_PATH,
                                "MESSAGE",
                                text,
                                next_envelope_ordinal(&envelopes),
                            )?;
                            envelopes.push(message.clone());
                            status_store
                                .save_session_state(
                                    "harness-demo-server:/root",
                                    &json!({"history":history,"jobs":jobs,"requests":requests,"envelopes":envelopes,"conversation":conversation}),
                                )
                                .map_err(|_| "could not persist queued browser message")?;
                            control.set_snapshot(Snapshot { conversations: conversation_rows(&conversation, &envelopes), requests: requests.clone(), jobs: jobs.clone(), envelopes: envelopes.clone(), ..Snapshot::default() });
                            control.publish("envelope.upsert", message);
                        }
                        let progress = persist_browser_envelope(
                            &status_store,
                            "/harness",
                            ROOT_PATH,
                            "PROGRESS",
                            &format!("Request {command_id} accepted; deterministic Engine is processing it."),
                            next_envelope_ordinal(&envelopes),
                        )?;
                        envelopes.push(progress.clone());
                        status_store
                            .save_session_state(
                                "harness-demo-server:/root",
                                &json!({"history":history,"jobs":jobs,"requests":requests,"envelopes":envelopes,"conversation":conversation}),
                            )
                            .map_err(|_| "could not persist server progress")?;
                        control.set_snapshot(Snapshot { conversations: conversation_rows(&conversation, &envelopes), requests: requests.clone(), jobs: jobs.clone(), envelopes: envelopes.clone(), ..Snapshot::default() });
                        control.publish("envelope.upsert", progress);
                        let mut child_envelopes = Vec::new();
                        let child_result = if let Some(text) = command.strip_prefix("child ") {
                            Some(
                                run_deterministic_child_message(
                                    status_store.clone(),
                                    scheduler.clone(),
                                    &command_id,
                                    text,
                                )
                                .await,
                            )
                        } else {
                            None
                        };
                        if let Some(Ok((child, _, child_envelope))) = &child_result {
                            let parent_message = persist_browser_envelope(
                                &status_store,
                                ROOT_PATH,
                                &child.0,
                                "MESSAGE",
                                command.strip_prefix("child ").unwrap_or_default(),
                                next_envelope_ordinal(&envelopes),
                            )?;
                            envelopes.push(parent_message.clone());
                            child_envelopes.push(parent_message);
                            let mut reply = child_envelope.clone();
                            reply["ordinal"] = json!(next_envelope_ordinal(&envelopes));
                            envelopes.push(reply.clone());
                            child_envelopes.push(reply);
                        }
                        if command == "cancel" && waiting {
                            if let Some((cancel_tx, task)) = pending_engine.take() {
                                let _ = cancel_tx.send(true);
                                let _ = task.await;
                            }
                        }
                        let skip_engine_for_queued_message =
                            command.starts_with("message ") && waiting;
                        let child_failure = child_result
                            .as_ref()
                            .and_then(|result| result.as_ref().err())
                            .cloned();
                        let child_answer = child_result.as_ref().and_then(|result| {
                            result.as_ref().ok().map(|(path, answer, _)| {
                                format!("{} replied: {answer}", path.0)
                            })
                        });
                        let engine_answer = if initial_state == "running" {
                            let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
                            let engine_command_id = command_id.clone();
                            let engine_command = command.clone();
                            let engine_store = status_store.clone();
                            let engine_scheduler = scheduler.clone();
                            let task = tokio::spawn(async move {
                                run_deterministic_engine_turn(
                                    engine_store,
                                    engine_scheduler,
                                    &engine_command_id,
                                    &engine_command,
                                    waiting,
                                    cancel_rx,
                                    AgentPath(ROOT_PATH.into()),
                                    None,
                                )
                                .await
                            });
                            task_aborts
                                .lock()
                                .unwrap_or_else(|poison| poison.into_inner())
                                .push(task.abort_handle());
                            pending_engine = Some((cancel_tx, task));
                            None
                        } else if skip_engine_for_queued_message {
                            None
                        } else if let Some(error) = child_failure {
                            Some(Err(error))
                        } else {
                            let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
                            Some(
                                run_deterministic_engine_turn(
                                    status_store.clone(),
                                    scheduler.clone(),
                                    &command_id,
                                    &command,
                                    waiting,
                                    cancel_rx,
                                    AgentPath(ROOT_PATH.into()),
                                    child_answer,
                                )
                                .await,
                            )
                        };
                        let (state, outcome, answer, final_item) = match engine_answer {
                            None => (initial_state, initial_outcome, fallback_answer, None),
                            Some(Ok((answer, item))) => {
                                (initial_state, initial_outcome, Some(answer), Some(item))
                            }
                            Some(Err(_)) => ("failed", "failed", fallback_answer, None),
                        };
                        history.push(Item(json!({"type":"demo_command","command_id":command_id,
                            "request_id":request_id,
                            "command":command,"outcome":outcome,"answer":answer})));
                        if command == "cancel" && waiting {
                            if let Some(pending_id) = active_wait_request(&requests, &history) {
                                if let Some(pending) = history.iter_mut().find(|item| {
                                    item.0["request_id"] == pending_id && item.0["command"] == "wait"
                                }) {
                                    pending.0["outcome"] = json!("cancelled");
                                    pending.0["answer"] = json!("Request cancelled.");
                                }
                            }
                        }
                        let mut changed_requests = Vec::new();
                        let mut changed_jobs = Vec::new();
                        let mut changed_envelopes = child_envelopes;
                        if state != "running" {
                            let own_request_state = if state == "failed" { "failed" } else { "completed" };
                            let own_job_state = if state == "failed" { "settled" } else if outcome == "cancelled" { "cancelled" } else { "settled" };
                            let done_request = command_request_record(
                                &request_id,
                                own_request_state,
                                &command_id,
                                &command,
                                outcome,
                                answer.as_deref(),
                            );
                            let done_job = job_record(&job_id, own_job_state);
                            replace_by_id(&mut requests, done_request.clone());
                            replace_by_id(&mut jobs, done_job.clone());
                            changed_requests.push(done_request);
                            changed_jobs.push(done_job);
                            if command == "cancel" && waiting {
                                if let Some(pending_id) = active_wait_request(&requests, &history) {
                                    let wait_item = history.iter().find(|item| item.0["request_id"] == pending_id && item.0["command"] == "wait");
                                    let wait_command_id = wait_item.and_then(|item| item.0["command_id"].as_str()).unwrap_or_default();
                                    let cancelled_request = command_request_record(
                                        &pending_id, "failed", wait_command_id, "wait",
                                        "cancelled", Some("Request cancelled."),
                                    );
                                    let cancelled_job = job_record(pending_id.strip_prefix("request/").unwrap_or(&pending_id), "cancelled");
                                    replace_by_id(&mut requests, cancelled_request.clone());
                                    replace_by_id(&mut jobs, cancelled_job.clone());
                                    changed_requests.push(cancelled_request);
                                    changed_jobs.push(cancelled_job);
                                }
                            }
                            if own_request_state == "completed" {
                                if let Some(Ok((_, _, child_envelope))) = &child_result {
                                    let _ = child_envelope;
                                } else if let Some(item) = final_item.as_ref() {
                                    let envelope = persist_engine_final_envelope(&status_store, item, next_envelope_ordinal(&envelopes))?;
                                    envelopes.push(envelope.clone());
                                    changed_envelopes.push(envelope);
                                } else if skip_engine_for_queued_message {
                                    let queued = persist_browser_envelope(
                                        &status_store,
                                        ROOT_PATH,
                                        "/operator",
                                        "MESSAGE",
                                        answer.as_deref().unwrap_or("Message queued."),
                                        next_envelope_ordinal(&envelopes),
                                    )?;
                                    envelopes.push(queued.clone());
                                    changed_envelopes.push(queued);
                                }
                            } else if let Some(answer) = answer.as_deref() {
                                let failure = persist_browser_envelope(
                                    &status_store,
                                    ROOT_PATH,
                                    "/operator",
                                    "MESSAGE",
                                    answer,
                                    next_envelope_ordinal(&envelopes),
                                )?;
                                envelopes.push(failure.clone());
                                changed_envelopes.push(failure);
                            }
                        } else {
                            // Keep the wait outstanding while the receiver is
                            // free to accept message/cancel frames.
                        }
                        let next_conversation = if active_wait_request(&requests, &history).is_some() {
                            conversation_record("requesting")
                        } else {
                            conversation_record("idle")
                        };
                        let conversation_changed = conversation != next_conversation;
                        conversation = next_conversation;
                        status_store
                            .save_session_state(
                                "harness-demo-server:/root",
                                &json!({"history":history,"jobs":jobs,"requests":requests,"envelopes":envelopes,"conversation":conversation}),
                            )
                            .map_err(|_| "could not persist server job status".to_owned())?;
                        control.set_snapshot(Snapshot { conversations: conversation_rows(&conversation, &envelopes), requests: requests.clone(), jobs: jobs.clone(), envelopes: envelopes.clone(), ..Snapshot::default() });
                        for request in changed_requests {
                            control.publish("request.upsert", request);
                        }
                        for job in changed_jobs {
                            control.publish("job.upsert", job);
                        }
                        for envelope in changed_envelopes {
                            control.publish("envelope.upsert", envelope);
                        }
                        if conversation_changed {
                            control.publish("conversation.upsert", conversation.clone());
                        }
                    }
                }
            }
        }
    }
    if let Some((cancel_tx, task)) = pending_engine.take() {
        let _ = cancel_tx.send(true);
        let _ = task.await;
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
    async fn before_request(
        &self,
        _plan: &harness::hooks::RequestPlan,
    ) -> harness::hooks::BeforeRequestResult {
        harness::hooks::BeforeRequestResult {
            decision: harness::hooks::BeforeRequestDecision::Send,
            evidence: Some(json!({"consumer":"standalone-browser"})),
        }
    }

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
            assets,
        }) => {
            if let Err(error) = serve(db, addr, dev_shell, assets).await {
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

    #[tokio::test]
    async fn browser_provider_restricts_echo_and_child_by_final_plan() {
        let provider = BrowserProvider(CliProvider(DemoProvider::development(".", false)));
        fn assert_send<T: Send>(_: &T) {}
        assert_send(&provider);
        let plan = harness::hooks::RequestPlan {
            items: vec![
                harness::item::Item(json!({
                    "type":"message","role":"user","content":"earlier child should not affect routing"
                })),
                harness::item::Item(json!({
                    "type":"message","role":"user","content":"echo hello"
                })),
            ],
            tools_allowed: provider.all_tools(),
            effort: Effort::Low,
        };
        assert!(
            plan.tools_allowed
                .iter()
                .any(|tool| tool["name"] == "sleep")
        );
        assert!(!plan.tools_allowed.iter().any(|tool| tool["name"] == "ask"));
        let result = provider.before_request(&plan).await;
        assert_eq!(
            result.decision,
            harness::hooks::BeforeRequestDecision::SendRestricted {
                tools_allowed: vec!["sleep".into()]
            }
        );
        assert_eq!(
            result.evidence,
            Some(
                json!({"consumer":"standalone-browser","selection":"echo-sleep","sleep_advertised":true})
            )
        );
        let missing_sleep_plan = harness::hooks::RequestPlan {
            items: plan.items.clone(),
            tools_allowed: vec![],
            effort: Effort::Low,
        };
        let missing_sleep = provider.before_request(&missing_sleep_plan).await;
        assert_eq!(
            missing_sleep.decision,
            harness::hooks::BeforeRequestDecision::SendRestricted {
                tools_allowed: vec!["sleep".into()]
            }
        );
        assert_eq!(
            missing_sleep.evidence,
            Some(
                json!({"consumer":"standalone-browser","selection":"echo-sleep","sleep_advertised":false})
            )
        );
        let incidental_words_plan = harness::hooks::RequestPlan {
            items: vec![harness::item::Item(json!({
                "type":"message","role":"user","content":"please echo hello, and child appears only in this payload"
            }))],
            tools_allowed: provider.all_tools(),
            effort: Effort::Low,
        };
        let incidental_words = provider.before_request(&incidental_words_plan).await;
        assert_eq!(
            incidental_words.decision,
            harness::hooks::BeforeRequestDecision::Send
        );
        assert_eq!(
            incidental_words.evidence,
            Some(
                json!({"consumer":"standalone-browser","selection":"send","sleep_advertised":true})
            )
        );
        let child_plan = harness::hooks::RequestPlan {
            items: vec![
                harness::item::Item(json!({
                    "type":"message","role":"user","content":"earlier echo should not affect routing"
                })),
                harness::item::Item(json!({
                    "type":"message","role":"user","content":"child hello"
                })),
            ],
            tools_allowed: provider.all_tools(),
            effort: Effort::Low,
        };
        let child = provider.before_request(&child_plan).await;
        assert_eq!(
            child.decision,
            harness::hooks::BeforeRequestDecision::SendRestricted {
                tools_allowed: vec![]
            }
        );
        assert_eq!(
            child.evidence,
            Some(
                json!({"consumer":"standalone-browser","selection":"child-empty","sleep_advertised":true})
            )
        );
        let cli = CliProvider(DemoProvider::development(".", false));
        let cli_result = cli.before_request(&plan).await;
        assert_eq!(
            cli_result.decision,
            harness::hooks::BeforeRequestDecision::Send
        );
    }

    #[test]
    fn deterministic_capture_appends_production_request_body_jsonl() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("demo-request-{nonce}.jsonl"));
        let request = ResponsesRequest {
            input: vec![],
            instructions: "capture-test".into(),
            tools: vec![json!({"type":"function","name":"sleep","strict":true,
                "parameters":{"type":"object","properties":{"duration_ms":{"type":"integer"}},
                    "required":["duration_ms"],"additionalProperties":false}})],
            tools_allowed: Some(vec!["sleep".into()]),
            model: "deterministic-local".into(),
            pinned_effort: Effort::Low,
            session_id: "capture-test".into(),
        };
        capture_deterministic_request(&request, &path).unwrap();
        let record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(record["tools"], json!(request.tools));
        let _ = std::fs::remove_file(path);
        let directory = std::env::temp_dir();
        assert!(matches!(
            capture_deterministic_request(&request, &directory),
            Err(TransportError::Stream(_))
        ));
    }

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
                assets: None,
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
                assets: None,
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
    fn assets_cli_is_absolute_and_serve_only() {
        let parse =
            |args: &[&str]| parse_args(&args.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert!(
            parse(&[
                "--db",
                "state.sqlite",
                "--ask",
                "hi",
                "--assets",
                "/tmp/assets"
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "--db",
                "state.sqlite",
                "--serve",
                "127.0.0.1:0",
                "--assets",
                "web/dist"
            ])
            .is_err()
        );
        assert!(matches!(
            parse(&["--db", "state.sqlite", "--serve", "127.0.0.1:0", "--assets", "/tmp/assets"]).unwrap(),
            Mode::Serve { assets: Some(path), .. } if path == PathBuf::from("/tmp/assets")
        ));
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
    fn deterministic_server_commands_keep_wait_pending_without_blocking_followups() {
        assert_eq!(
            deterministic_command("wait", false),
            ("running", "pending", None)
        );
        assert_eq!(
            deterministic_command("message queued text", true),
            (
                "settled",
                "queued",
                Some("Message queued: queued text".into())
            )
        );
        assert_eq!(
            deterministic_command("cancel", true),
            (
                "settled",
                "completed",
                Some("Pending wait cancelled.".into())
            )
        );
        let request_id = "request/client-command-1";
        assert_eq!(
            active_wait_request(
                &[request_record(request_id, "running")],
                &[Item(json!({
                    "type":"demo_command",
                    "command_id":"client-command-1",
                    "request_id":request_id,
                    "command":"wait"
                }))]
            )
            .as_deref(),
            Some(request_id),
            "the wait matches its explicit request id, not client command id"
        );
    }

    #[test]
    fn deterministic_server_commands_separate_failure_and_recovery_success() {
        assert_eq!(
            deterministic_command("fail", false),
            (
                "failed",
                "failed",
                Some("Controlled deterministic failure.".into())
            )
        );
        assert_eq!(
            deterministic_command("echo recovered", false),
            ("settled", "completed", Some("recovered".into()))
        );
    }

    #[tokio::test]
    async fn deterministic_server_child_message_and_answer_are_store_backed() {
        let store = Arc::new(Store::memory().unwrap());
        let scheduler = Arc::new(JobScheduler::new(2).unwrap());
        let (child, reply, snapshot) =
            run_deterministic_child_message(store.clone(), scheduler, "child-id", "hello")
                .await
                .unwrap();
        assert!(child.0.starts_with("/root/demo_child_"));
        assert_eq!(reply, "Child received and replied: hello");
        let stored = store.inbox(ROOT_PATH).unwrap();
        let committed = stored
            .iter()
            .rev()
            .find(|entry| entry.sender == child.0)
            .unwrap();
        assert_eq!(snapshot["id"], format!("envelope/{}", committed.id));
        let item = store.get_item(&committed.item_hash).unwrap().unwrap();
        let stored_text = item.0["content"][0]["text"].as_str().unwrap();
        assert!(stored_text.ends_with(&reply));
        let delivered = store
            .inbox(&child.0)
            .unwrap()
            .into_iter()
            .find(|entry| entry.sender == ROOT_PATH)
            .expect("child Engine did not claim a real parent message");
        assert!(
            delivered.delivered_request.is_some(),
            "child inbound envelope was not claimed by its Engine request"
        );
        let parent = run_deterministic_engine_completion(
            store.clone(),
            Arc::new(JobScheduler::new(2).unwrap()),
            "parent-id",
            "child hello",
            false,
            tokio::sync::watch::channel(false).1,
            AgentPath(ROOT_PATH.into()),
            Some(format!("{} replied: {reply}", child.0)),
        )
        .await
        .unwrap();
        let parent_reply = format!("{} replied: {reply}", child.0);
        assert_eq!(
            final_text(&parent.turn.items).as_deref(),
            Some(parent_reply.as_str())
        );
        assert!(store.unread(ROOT_PATH).unwrap().is_empty());
    }

    #[tokio::test]
    async fn deterministic_server_turn_uses_store_backed_offline_engine() {
        let store = Arc::new(Store::memory().unwrap());
        let scheduler = Arc::new(JobScheduler::new(2).unwrap());
        assert!(
            run_deterministic_engine_turn(
                store.clone(),
                scheduler.clone(),
                "fail-id",
                "fail",
                false,
                tokio::sync::watch::channel(false).1,
                AgentPath(ROOT_PATH.into()),
                None,
            )
            .await
            .is_err()
        );
        assert_eq!(
            run_deterministic_engine_turn(
                store,
                scheduler,
                "echo-id",
                "echo offline engine",
                false,
                tokio::sync::watch::channel(false).1,
                AgentPath(ROOT_PATH.into()),
                None,
            )
            .await
            .unwrap()
            .0,
            "offline engine"
        );
    }

    #[tokio::test]
    async fn deterministic_server_engine_wait_observes_cancellation() {
        let store = Arc::new(Store::memory().unwrap());
        let scheduler = Arc::new(JobScheduler::new(2).unwrap());
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run_deterministic_engine_turn(
            store,
            scheduler,
            "wait-id",
            "wait",
            false,
            cancel_rx,
            AgentPath(ROOT_PATH.into()),
            None,
        ));
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_tx.send(true).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap()
                .is_err()
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
            assets: None,
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
