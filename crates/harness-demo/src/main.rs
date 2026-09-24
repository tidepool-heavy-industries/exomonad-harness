//! Reference provider for the harness. `run` intentionally invokes a local
//! shell and is suitable only for trusted, development-time demonstrations.
use async_trait::async_trait;
use harness::provider::{CallContext, Provider, ProviderError};
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct DemoProvider {
    root: PathBuf,
    allow_shell: bool,
}

impl DemoProvider {
    /// Shell execution is opt-in; callers should use it only in a disposable
    /// development checkout with trusted tool input.
    pub fn development(root: impl Into<PathBuf>, allow_shell: bool) -> Self {
        Self {
            root: root.into(),
            allow_shell,
        }
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
        Ok(json!({
            "status": output.status.code(),
            "success": output.status.success(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr)
        }))
    }

    fn edit(&self, args: &Value) -> Result<Value, ProviderError> {
        let rel = Path::new(string_arg(args, "path")?);
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(ProviderError::Tool(
                "path must be a relative, normalized path".into(),
            ));
        }
        let owned = args
            .get("owned")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::Tool("owned must declare the permitted paths".into()))?;
        let declared = owned
            .iter()
            .filter_map(Value::as_str)
            .any(|p| Path::new(p) == rel);
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
            json!({"type":"function","name":"sleep","description":"Wait asynchronously","parameters":{"type":"object","properties":{"duration_ms":{"type":"integer","minimum":0}},"required":["duration_ms"]}}),
            json!({"type":"function","name":"edit","description":"Replace one exact string in an existing owned file","parameters":{"type":"object","properties":{"path":{"type":"string"},"owned":{"type":"array","items":{"type":"string"}},"before":{"type":"string"},"after":{"type":"string"}},"required":["path","owned","before","after"]}}),
            json!({"type":"function","name":"ask","description":"Request operator input (host integration required)","parameters":{"type":"object","properties":{"question":{"type":"string"}},"required":["question"]}}),
            json!({"type":"function","name":"form","description":"Request a schema-backed operator form (host integration required)","parameters":{"type":"object","properties":{"title":{"type":"string"},"schema":{"type":"object"}},"required":["title","schema"]}}),
        ]
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--smoke") => {
            let provider = DemoProvider::development(".", false);
            let result = provider.call("sleep", json!({"duration_ms": 0})).await;
            println!("demo provider smoke: {result:?}");
        }
        Some("--allow-shell") => {
            eprintln!("unsafe development-only shell mode; pass script as remaining arguments");
            let provider = DemoProvider::development(".", true);
            let script = args[1..].join(" ");
            println!("{:?}", provider.call("run", json!({"script":script})).await);
        }
        _ => println!(
            "harness-demo protocol {} — use --smoke; --allow-shell is dev-only",
            harness::protocol::VERSION
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness::model::CallId;
    use tokio::sync::mpsc;

    fn temp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("harness-demo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[tokio::test]
    async fn sleep_is_async_and_returns_output() {
        let provider = DemoProvider::development(".", false);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let ctx = CallContext {
            handle: harness::provider::JobHandle("sleep-1".into()),
            call_id: CallId("c1".into()),
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
        assert!(
            provider
                .call(
                    "edit",
                    json!({"path":"ok.txt","owned":[],"before":"old","after":"new"})
                )
                .await
                .is_err()
        );
        assert!(provider.call("edit", json!({"path":"../outside-demo.txt","owned":["../outside-demo.txt"],"before":"safe","after":"bad"})).await.is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "safe");
        assert_eq!(
            provider
                .call(
                    "edit",
                    json!({"path":"ok.txt","owned":["ok.txt"],"before":"old","after":"new"})
                )
                .await
                .unwrap()["edited"],
            true
        );
        assert_eq!(std::fs::read_to_string(root.join("ok.txt")).unwrap(), "new");
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(outside);
    }
}
