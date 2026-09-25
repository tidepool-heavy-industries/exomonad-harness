use crate::{
    item::Item,
    model::{CallId, Effort},
    transport::Usage,
};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompactError {
    #[error("compaction failed: {0}")]
    Failed(String),
}

pub struct NewWindow {
    pub items: Vec<Item>,
    pub effort: Effort,
    pub carried: Vec<CallId>,
}

pub struct CompactContext<'a> {
    pub items: &'a [Item],
    pub usage: &'a Usage,
    pub pending_calls: &'a [Item],
    pub effort: Effort,
    /// Calls the configured server compaction endpoint without constructing a
    /// second transport request at the strategy boundary.
    pub server_compact: &'a (dyn Fn(Vec<Item>) -> ServerCompactFuture<'a> + Send + Sync),
}

pub type ServerCompactFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Vec<Item>, CompactError>> + Send + 'a>,
>;

impl CompactContext<'_> {
    pub fn items(&self) -> &[Item] {
        self.items
    }
    pub fn usage(&self) -> &Usage {
        self.usage
    }
    pub fn pending_calls(&self) -> &[Item] {
        self.pending_calls
    }

    pub async fn server_compact(&self, items: Vec<Item>) -> Result<Vec<Item>, CompactError> {
        (self.server_compact)(items).await
    }

    pub async fn typed_turn<T>(&self, _instructions: &str, _tool: &str) -> Result<T, CompactError>
    where
        T: JsonSchema + Serialize + DeserializeOwned,
    {
        Err(CompactError::Failed(
            "typed_turn is not provided by the server strategy context".into(),
        ))
    }
}

#[async_trait]
pub trait Compactor: Send + Sync {
    type Summary: JsonSchema + Serialize + DeserializeOwned + Send + Sync;
    async fn compact(&self, cx: CompactContext<'_>) -> Result<NewWindow, CompactError>;
}

pub struct Server;

#[async_trait]
impl Compactor for Server {
    type Summary = serde_json::Value;

    async fn compact(&self, cx: CompactContext<'_>) -> Result<NewWindow, CompactError> {
        let mut input: Vec<Item> = cx
            .items()
            .iter()
            .filter(|item| {
                item.0.get("type").and_then(|v| v.as_str()) != Some("configuration_update")
            })
            .cloned()
            .collect();
        input.push(Item(json!({"type":"compaction_trigger"})));
        let mut items = cx.server_compact(input).await?;
        // Pending function calls must remain valid for late call_id outputs,
        // independent of whether the endpoint happens to preserve them.
        for call in cx.pending_calls() {
            if !items.contains(call) {
                items.push(call.clone());
            }
        }
        let mut carried = Vec::new();
        for item in cx.pending_calls() {
            if let Some(id) = item.0.get("call_id").and_then(|v| v.as_str()) {
                let id = CallId(id.to_owned());
                if !carried.contains(&id) {
                    carried.push(id);
                }
            }
        }
        items.push(Item(json!({"type":"configuration_update","reasoning_effort":format!("{:?}", cx.effort).to_lowercase()})));
        Ok(NewWindow {
            items,
            effort: cx.effort,
            carried,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pending_function_call_is_carried_verbatim() {
        let call = Item(json!({"type":"function_call","call_id":"call-7","name":"work"}));
        let source = vec![
            call.clone(),
            Item(json!({"type":"configuration_update","reasoning_effort":"low"})),
        ];
        let usage = Usage::default();
        let compact = |_input: Vec<Item>| -> ServerCompactFuture<'_> {
            Box::pin(async { Ok(vec![Item(json!({"type":"message","content":"summary"}))]) })
        };
        let cx = CompactContext {
            items: &source,
            usage: &usage,
            pending_calls: std::slice::from_ref(&call),
            effort: Effort::Medium,
            server_compact: &compact,
        };
        let window = Server.compact(cx).await.unwrap();
        assert!(window.items.contains(&call));
        assert_eq!(window.carried, vec![CallId("call-7".into())]);
        assert!(!window.items.iter().any(|item| {
            item.0.get("type").and_then(|v| v.as_str()) == Some("compaction_trigger")
        }));
        assert_eq!(
            window.items.last().unwrap().0["type"],
            "configuration_update"
        );
    }
}
