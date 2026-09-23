use async_trait::async_trait;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("tool failed: {0}")]
    Tool(String),
}

/// A provider owns tool meaning; the harness owns scheduling and history.
#[async_trait]
pub trait Provider: Send + Sync {
    async fn call(&self, name: &str, args: Value) -> Result<Value, ProviderError>;
    fn tools(&self) -> Vec<Value>;
}
