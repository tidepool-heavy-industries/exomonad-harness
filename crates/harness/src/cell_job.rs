//! Resident evaluator calls use the existing asynchronous JobScheduler lane.
//!
//! A `cell` tool call is admitted as a Job, not evaluated inline with a model
//! request. Implementations must stop their work when the `run` future is
//! dropped: JobScheduler cancellation aborts that future. The complete
//! `CellOutput` is the retained Job output. For a native custom call, Engine
//! must persist it as a custom tool-call output; progress is only out-of-band.
use crate::provider::{CallContext, Provider, ProviderError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const CELL_TOOL: &str = "cell";

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellInput {
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellOutput {
    pub value: Value,
    pub stdout: String,
    pub stderr: String,
}

// TODO(adoption H1; docs/daily-driver-plan.md): connect this seam to the
// embedding resident workbench. Prove underlying execution/resource cancellation;
// dropping this future is insufficient for work retained by another owner.
/// A resident interpreter; a future is one cancellable, asynchronous cell Job.
#[async_trait]
pub trait CellJob: Send + Sync {
    async fn run(
        &self,
        input: CellInput,
        context: CallContext,
    ) -> Result<CellOutput, ProviderError>;
}

/// Connect a resident evaluator to the sole Provider/JobScheduler path.
pub struct CellJobProvider<E> {
    evaluator: E,
}

impl<E> CellJobProvider<E> {
    pub fn new(evaluator: E) -> Self {
        Self { evaluator }
    }
}

#[async_trait]
impl<E: CellJob> Provider for CellJobProvider<E> {
    async fn call(&self, name: &str, _args: Value) -> Result<Value, ProviderError> {
        Err(ProviderError::Tool(format!(
            "`{name}` requires an asynchronous call context"
        )))
    }

    async fn call_with_context(
        &self,
        name: &str,
        args: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        if name != CELL_TOOL {
            return Err(ProviderError::Tool(format!("unknown cell tool `{name}`")));
        }
        let input: CellInput = serde_json::from_value(args)
            .map_err(|error| ProviderError::Tool(format!("invalid cell input: {error}")))?;
        let output = self.evaluator.run(input, context).await?;
        serde_json::to_value(output)
            .map_err(|error| ProviderError::Tool(format!("invalid cell output: {error}")))
    }

    async fn call_custom_with_context(
        &self,
        name: &str,
        input: String,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        if name != CELL_TOOL {
            return Err(ProviderError::Tool(format!("unknown cell tool `{name}`")));
        }
        let output = self
            .evaluator
            .run(CellInput { source: input }, context)
            .await?;
        serde_json::to_value(output)
            .map_err(|error| ProviderError::Tool(format!("invalid cell output: {error}")))
    }

    fn tools(&self) -> Vec<Value> {
        vec![json!({
            "type": "custom",
            "name": CELL_TOOL,
            "description": "Run one cell in the resident evaluator as a cancellable async Job.",
            "format": {"type": "text"}
        })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{AgentPath, CallId},
        provider::JobHandle,
        turn::{JobOutput, JobScheduler},
    };
    use std::sync::Arc;

    struct EchoCell;

    #[async_trait]
    impl CellJob for EchoCell {
        async fn run(
            &self,
            input: CellInput,
            context: CallContext,
        ) -> Result<CellOutput, ProviderError> {
            let _ = context.progress.try_send(json!({"phase":"running"}));
            if input.source == "never" {
                std::future::pending::<()>().await;
            }
            Ok(CellOutput {
                value: json!({"source":input.source}),
                stdout: "complete, retained stdout".into(),
                stderr: String::new(),
            })
        }
    }

    #[tokio::test]
    async fn cell_rejects_malformed_function_arguments_and_unknown_name() {
        let provider = CellJobProvider::new(EchoCell);
        for args in [
            json!({"source": 3}),
            json!({}),
            json!({"source": "valid", "unexpected": true}),
        ] {
            let (context, _) = CallContext::detached_for_test(
                JobHandle("cell-invalid".into()),
                CallId("cell-invalid".into()),
                AgentPath("/root".into()),
            );
            let error = provider
                .call_with_context(CELL_TOOL, args, context)
                .await
                .unwrap_err();
            assert!(
                matches!(error, ProviderError::Tool(message) if message.starts_with("invalid cell input:")),
                "malformed function arguments must be explicitly refused"
            );
        }
        let (context, _) = CallContext::detached_for_test(
            JobHandle("cell-unknown".into()),
            CallId("cell-unknown".into()),
            AgentPath("/root".into()),
        );
        let error = provider
            .call_with_context("not_cell", json!({"source": "valid"}), context)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ProviderError::Tool(message) if message.contains("unknown cell tool"))
        );
    }

    #[tokio::test]
    async fn cell_custom_input_reaches_evaluator_exactly() {
        let provider: Arc<dyn Provider> = Arc::new(CellJobProvider::new(EchoCell));
        let raw = "line1\n\"quoted\" \\\\path 第二行 — λ 🪼";
        let (context, _) = CallContext::detached_for_test(
            JobHandle("cell-raw".into()),
            CallId("cell-raw".into()),
            AgentPath("/root".into()),
        );
        let result = provider
            .call_custom_with_context(CELL_TOOL, raw.to_owned(), context)
            .await
            .unwrap();
        assert_eq!(result["value"], json!({"source": raw}));

        let (context, _) = CallContext::detached_for_test(
            JobHandle("cell-unknown-custom".into()),
            CallId("cell-unknown-custom".into()),
            AgentPath("/root".into()),
        );
        let error = provider
            .call_custom_with_context("not_cell", raw.to_owned(), context)
            .await
            .unwrap_err();
        assert!(
            matches!(error, ProviderError::Tool(message) if message.contains("unknown cell tool"))
        );
    }

    #[tokio::test]
    async fn cell_legacy_function_job_retains_output_and_cancels() {
        let jobs = JobScheduler::new(2).unwrap();
        let provider: Arc<dyn Provider> = Arc::new(CellJobProvider::new(EchoCell));
        assert_eq!(
            provider.tools(),
            vec![json!({
                "type": "custom",
                "name": CELL_TOOL,
                "description": "Run one cell in the resident evaluator as a cancellable async Job.",
                "format": {"type": "text"}
            })]
        );
        let done = CallId("cell-done".into());
        jobs.start(
            provider.clone(),
            done.clone(),
            CELL_TOOL.into(),
            json!({"source":"1+1"}),
        )
        .await
        .unwrap();
        let output = jobs.wait(&done).await.unwrap();
        let JobOutput::Completed(Ok(value)) = output else {
            panic!("cell should complete through JobScheduler");
        };
        assert_eq!(value["value"], json!({"source":"1+1"}));
        assert_eq!(value["stdout"], "complete, retained stdout");
        assert_eq!(
            jobs.output(&done).await.unwrap(),
            Some(JobOutput::Completed(Ok(value)))
        );
        assert_eq!(
            jobs.progress(&done).await.unwrap(),
            vec![json!({"phase":"running"})]
        );

        let cancelled = CallId("cell-cancel".into());
        jobs.start(
            provider,
            cancelled.clone(),
            CELL_TOOL.into(),
            json!({"source":"never"}),
        )
        .await
        .unwrap();
        jobs.cancel(&cancelled).await.unwrap();
        assert_eq!(jobs.wait(&cancelled).await.unwrap(), JobOutput::Cancelled);
        assert_eq!(
            jobs.output(&cancelled).await.unwrap(),
            Some(JobOutput::Cancelled)
        );
    }
}
