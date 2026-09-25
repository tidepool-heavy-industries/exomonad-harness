//! Resident evaluator calls use the existing asynchronous JobScheduler lane.
//!
//! A `cell` tool call is admitted as a Job, not evaluated inline with a model
//! request. Implementations must stop their work when the `run` future is
//! dropped: JobScheduler cancellation aborts that future. The complete
//! `CellOutput` is the retained Job output and is persisted as the ordinary
//! function-call output by Engine; progress is only out-of-band.
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

    fn tools(&self) -> Vec<Value> {
        vec![json!({
            "type": "function",
            "name": CELL_TOOL,
            "description": "Run one cell in the resident evaluator as a cancellable async Job.",
            "parameters": {
                "type": "object",
                "properties": {"source": {"type": "string"}},
                "required": ["source"],
                "additionalProperties": false
            },
            "strict": true
        })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::CallId,
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
            context.progress.send(json!({"phase":"running"})).unwrap();
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
    async fn cell_uses_retained_async_job_output_and_typed_cancellation() {
        let jobs = JobScheduler::new(2).unwrap();
        let provider: Arc<dyn Provider> = Arc::new(CellJobProvider::new(EchoCell));
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
