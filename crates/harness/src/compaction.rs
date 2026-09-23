use crate::{item::Item, model::Effort};
use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompactError {
    #[error("compaction failed: {0}")]
    Failed(String),
}

pub struct NewWindow {
    pub items: Vec<Item>,
    pub effort: Effort,
}

/// Wave 0 supplies `Server`; later strategies share this boundary.
#[async_trait]
pub trait Compactor: Send + Sync {
    async fn compact(&self, items: &[Item]) -> Result<NewWindow, CompactError>;
}
