//! Observational provider output; it never grants execution or settlement authority.
use crate::transport::sse::OutputChannel;
use crate::{
    item::ItemHash,
    model::{ConversationIdentity, RequestId},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOutput {
    pub origin: ConversationIdentity,
    pub request_id: RequestId,
    #[serde(flatten)]
    pub update: ModelOutputUpdate,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ModelOutputUpdate {
    Started,
    Stopped,
    Delta {
        item_id: String,
        channel: OutputChannel,
        index: u64,
        text: String,
    },
    Committed {
        item_id: Option<String>,
        hash: ItemHash,
    },
}
/// Implementations must do bounded in-memory work, without I/O or awaiting consumers.
pub trait ModelOutputObserver: Send + Sync {
    fn observe(&self, output: ModelOutput);
}

/// Dropping an in-flight provider future stops its observational partial output.
pub(super) struct OutputLifetime {
    pub observer: Option<std::sync::Arc<dyn ModelOutputObserver>>,
    pub origin: ConversationIdentity,
    pub request_id: RequestId,
}
impl OutputLifetime {
    pub fn stop(&mut self) {
        if let Some(observer) = self.observer.take() {
            observer.observe(ModelOutput {
                origin: self.origin.clone(),
                request_id: self.request_id.clone(),
                update: ModelOutputUpdate::Stopped,
            });
        }
    }
}

impl Drop for OutputLifetime {
    fn drop(&mut self) {
        self.stop();
    }
}
