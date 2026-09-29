use async_trait::async_trait;
use harness::{
    cell_job::{CellInput, CellJob, CellJobProvider, CellOutput},
    item::Item,
    model::CallId,
    provider::{CallContext, Provider, ProviderError},
    turn::{JobError, JobOutput, JobScheduler},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::sync::Notify;

struct EchoCell;

#[async_trait]
impl CellJob for EchoCell {
    async fn run(
        &self,
        input: CellInput,
        _context: CallContext,
    ) -> Result<CellOutput, ProviderError> {
        Ok(CellOutput {
            value: Value::String(input.source),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

#[tokio::test]
async fn custom_cell_preserves_exact_raw_input_through_item_scheduler_and_provider() {
    let provider: Arc<dyn Provider> = Arc::new(CellJobProvider::new(EchoCell));
    assert_eq!(provider.tools()[0]["type"], "custom");
    assert_eq!(provider.tools()[0]["format"], json!({"type":"text"}));
    let scheduler = JobScheduler::new(1).unwrap();
    let raw = "say \"hello\" \\\\path\n第二行 — λ 🪼";
    let item = Item(json!({
        "type": "custom_tool_call",
        "call_id": "raw-cell",
        "name": "cell",
        "input": raw
    }));
    let handle = scheduler
        .start_completed_item(provider, &item)
        .await
        .unwrap()
        .expect("custom cell is admitted");
    assert_eq!(handle.0, "raw-cell");
    let output = scheduler.wait(&CallId("raw-cell".into())).await.unwrap();
    assert_eq!(
        output,
        JobOutput::Completed(Ok(json!({
            "value": raw,
            "stdout": "",
            "stderr": ""
        })))
    );
    assert_eq!(
        scheduler.output(&CallId("raw-cell".into())).await.unwrap(),
        Some(output),
        "the settled custom output remains retained"
    );
}

#[tokio::test]
async fn malformed_custom_input_is_rejected_before_evaluator_admission() {
    let provider: Arc<dyn Provider> = Arc::new(CellJobProvider::new(EchoCell));
    let scheduler = JobScheduler::new(1).unwrap();
    let malformed = Item(json!({
        "type": "custom_tool_call",
        "call_id": "bad-cell",
        "name": "cell",
        "input": {"source": "not raw text"}
    }));
    assert!(matches!(
        scheduler.start_completed_item(provider, &malformed).await,
        Err(JobError::InvalidCallItem)
    ));
    assert!(matches!(
        scheduler.output(&CallId("bad-cell".into())).await,
        Err(JobError::UnknownCall)
    ));
}

struct FunctionOnly;

#[async_trait]
impl Provider for FunctionOnly {
    async fn call(&self, _name: &str, args: Value) -> Result<Value, ProviderError> {
        Ok(args)
    }

    fn tools(&self) -> Vec<Value> {
        vec![]
    }
}

#[tokio::test]
async fn unsupported_custom_provider_returns_explicit_refusal() {
    let provider: Arc<dyn Provider> = Arc::new(FunctionOnly);
    let scheduler = JobScheduler::new(1).unwrap();
    let item = Item(json!({
        "type": "custom_tool_call",
        "call_id": "unsupported",
        "name": "cell",
        "input": "raw source"
    }));
    scheduler
        .start_completed_item(provider, &item)
        .await
        .unwrap()
        .expect("recognized custom call admitted");
    let output = scheduler.wait(&CallId("unsupported".into())).await.unwrap();
    let JobOutput::Completed(Err(error)) = output else {
        panic!("unsupported custom tool must settle with an explicit error");
    };
    assert!(error.contains("custom tool `cell` is not supported"));
}

#[tokio::test]
async fn function_call_still_uses_json_arguments_and_provider_function_path() {
    let provider: Arc<dyn Provider> = Arc::new(FunctionOnly);
    let scheduler = JobScheduler::new(1).unwrap();
    let item = Item(json!({
        "type": "function_call",
        "call_id": "legacy-function",
        "name": "echo",
        "arguments": "{\"answer\":42}"
    }));
    scheduler
        .start_completed_item(provider, &item)
        .await
        .unwrap()
        .expect("function call admitted");
    assert_eq!(
        scheduler
            .wait(&CallId("legacy-function".into()))
            .await
            .unwrap(),
        JobOutput::Completed(Ok(json!({"answer":42})))
    );
}

struct BlockingCell {
    started: Arc<Notify>,
    drops: Arc<AtomicUsize>,
}

struct DropCount(Arc<AtomicUsize>);

impl Drop for DropCount {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl CellJob for BlockingCell {
    async fn run(
        &self,
        _input: CellInput,
        _context: CallContext,
    ) -> Result<CellOutput, ProviderError> {
        let _drop_count = DropCount(self.drops.clone());
        self.started.notify_one();
        std::future::pending().await
    }
}

#[tokio::test]
async fn custom_cell_cancel_releases_evaluator_and_retains_cancellation() {
    let started = Arc::new(Notify::new());
    let drops = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn Provider> = Arc::new(CellJobProvider::new(BlockingCell {
        started: started.clone(),
        drops: drops.clone(),
    }));
    let scheduler = JobScheduler::new(1).unwrap();
    let call_id = CallId("cancel-custom".into());
    let item = Item(json!({
        "type": "custom_tool_call",
        "call_id": call_id.0.clone(),
        "name": "cell",
        "input": "some raw source"
    }));
    scheduler
        .start_completed_item(provider, &item)
        .await
        .unwrap()
        .expect("custom cell admitted");
    started.notified().await;
    scheduler.cancel(&call_id).await.unwrap();
    assert_eq!(
        scheduler.wait(&call_id).await.unwrap(),
        JobOutput::Cancelled
    );
    assert_eq!(
        scheduler.output(&call_id).await.unwrap(),
        Some(JobOutput::Cancelled)
    );
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}
