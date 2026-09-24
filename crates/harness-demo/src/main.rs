//! Reference provider for the harness. `run` intentionally invokes a local
//! shell and is suitable only for trusted, development-time demonstrations.
use async_trait::async_trait;
use harness::engine::{Engine, EngineConfig, EngineError};
use harness::item::Item;
use harness::model::{AgentPath, Effort};
use harness::provider::{CallContext, Provider, ProviderError};
use harness::store::Store;
use harness::transport::{TransportError, auth::CodexFileAuth};
use harness::turn::JobScheduler;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const OUTPUT_LIMIT: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct CliOptions {
    db: PathBuf,
    ask: String,
    dev_shell: bool,
}

fn parse_args(args: &[String]) -> Result<Option<CliOptions>, String> {
    if args.first().map(String::as_str) == Some("--smoke") {
        return Ok(None);
    }
    let (mut db, mut ask, mut dev_shell) = (None, None, false);
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
            "--dev-shell" => dev_shell = true,
            "--allow-shell" => {
                return Err("use --dev-shell to opt into development shell execution".into());
            }
            flag => return Err(format!("unknown argument: {flag}")),
        }
        i += 1;
    }
    let db = db.ok_or("--db <sqlite-path> is required")?;
    let ask = ask.ok_or("--ask <text> is required")?;
    if ask.trim().is_empty() {
        return Err("--ask text must not be empty".into());
    }
    Ok(Some(CliOptions { db, ask, dev_shell }))
}

struct CliProvider(DemoProvider);

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
        Ok(Self { engine })
    }

    async fn ask(&self, prompt: &str) -> Result<(String, u64, u64), String> {
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let turn = self
            .engine
            .run(
                vec![Item(
                    json!({"type":"message","role":"user","content":prompt}),
                )],
                cancel_rx,
            )
            .await
            .map_err(safe_engine_error)?;
        let text = final_text(&turn.items)
            .ok_or_else(|| "engine returned no final assistant text".to_owned())?;
        Ok((text, turn.usage.input_tokens, turn.usage.output_tokens))
    }
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
        EngineError::Transport(error) => safe_transport_error(error),
        EngineError::Store(_) => "conversation store operation failed".into(),
        EngineError::Job(_) => "provider job failed".into(),
        _ => "conversation engine failed (details withheld)".into(),
    }
}

#[derive(Clone, Debug)]
pub struct DemoProvider {
    root: PathBuf,
    allow_shell: bool,
    owned: Vec<PathBuf>,
}

impl DemoProvider {
    /// Shell execution is opt-in; callers should use it only in a disposable
    /// development checkout with trusted tool input.
    pub fn development(root: impl Into<PathBuf>, allow_shell: bool) -> Self {
        Self {
            root: root.into(),
            allow_shell,
            owned: Vec::new(),
        }
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
            let _ = context
                .progress
                .send(json!({"event":"sleep_started","handle":context.handle.0,"duration_ms":ms}));
            tokio::time::sleep(Duration::from_millis(ms)).await;
            return Ok(json!({"slept_ms":ms}));
        }
        self.call(name, args).await
    }

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
        Ok(None) => {
            let provider = DemoProvider::development(".", false);
            let result = provider.call("sleep", json!({"duration_ms": 0})).await;
            println!("demo provider smoke: {result:?}");
        }
        Ok(Some(options)) => {
            let driver = match CliDriver::new(&options) {
                Ok(driver) => driver,
                Err(error) => {
                    eprintln!("harness-demo: {error}");
                    std::process::exit(2);
                }
            };
            match driver.ask(&options.ask).await {
                Ok((text, input_tokens, output_tokens)) => {
                    println!("{text}");
                    eprintln!(
                        "usage (final response): {} input / {} output tokens",
                        input_tokens, output_tokens
                    );
                }
                Err(error) => {
                    eprintln!("harness-demo: {error}");
                    std::process::exit(1);
                }
            }
        }
        Err(error) => {
            eprintln!(
                "harness-demo: {error}\nusage: harness-demo --db <sqlite-path> --ask <text> [--dev-shell] | --smoke"
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
            Some(CliOptions {
                db: PathBuf::from("state.sqlite"),
                ask: "hi".into(),
                dev_shell: true
            })
        );
        assert_eq!(parse(&["--smoke"]).unwrap(), None);
    }

    #[test]
    fn driver_constructs_offline_without_reading_credentials_or_enabling_edit() {
        let root = temp();
        let options = CliOptions {
            db: root.join("state.sqlite"),
            ask: "hello".into(),
            dev_shell: false,
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
                vec![Item(json!({"role":"user","content":"hello"}))],
                cancel_rx,
            )
            .await
            .unwrap();
        assert_eq!(
            final_text(&turn.items).as_deref(),
            Some("offline engine answer")
        );
        assert_eq!(turn.usage.input_tokens, 12);
        assert_eq!(turn.usage.output_tokens, 4);
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
