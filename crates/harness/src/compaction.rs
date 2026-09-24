use crate::{item::Item, model::Effort};
use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompactError {
    #[error("compaction failed: {0}")]
    Failed(String),
}

// TODO(correction-wave d): this is a placeholder, not the PRD trait. Target:
//   trait Compactor { type Summary: JsonSchema+Serialize+DeserializeOwned;
//     async fn compact(&self, cx: CompactContext<'_>) -> Result<NewWindow, CompactError>; }
//   CompactContext: items() | usage() | pending_calls() | typed_turn::<T>(..) | server_compact()
//   NewWindow { items, effort, carried: Vec<CallId> }
// Ship `Server` (strip updates -> `compaction_trigger` LAST -> returned window
// -> fresh pin) and run the experiment: does the compacted window keep an
// unanswered `function_call`? -> docs/findings.md. Invariants the harness
// enforces regardless of strategy are in PRD `compaction`. No impl exists yet.
pub struct NewWindow {
    pub items: Vec<Item>,
    pub effort: Effort,
}

/// Wave 0 supplies `Server`; later strategies share this boundary.
#[async_trait]
pub trait Compactor: Send + Sync {
    async fn compact(&self, items: &[Item]) -> Result<NewWindow, CompactError>;
}
