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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolName(pub String);

pub struct CompactContext<'a> {
    pub items: &'a [Item],
    pub usage: &'a Usage,
    pub pending_calls: &'a [Item],
    pub effort: Effort,
    /// Calls the configured server compaction endpoint without constructing a
    /// second transport request at the strategy boundary.
    pub server_compact: &'a (dyn Fn(Vec<Item>) -> ServerCompactFuture<'a> + Send + Sync),
    /// Caller-provided typed-turn capability: the caller applies the forced
    /// tool choice and returns the decoded JSON value for that tool call.
    pub typed_turn:
        &'a (dyn Fn(String, ToolName, serde_json::Value) -> TypedTurnFuture<'a> + Send + Sync),
}

pub type ServerCompactFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Vec<Item>, CompactError>> + Send + 'a>,
>;
pub type TypedTurnFuture<'a> = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<serde_json::Value, CompactError>> + Send + 'a>,
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

    pub async fn typed_turn<T>(&self, instructions: &str, tool: ToolName) -> Result<T, CompactError>
    where
        T: JsonSchema + Serialize + DeserializeOwned,
    {
        let schema = serde_json::to_value(schemars::schema_for!(T)).map_err(|error| {
            CompactError::Failed(format!("serialize typed-turn schema: {error}"))
        })?;
        let value = (self.typed_turn)(instructions.to_owned(), tool, schema).await?;
        serde_json::from_value(value)
            .map_err(|error| CompactError::Failed(format!("decode typed-turn result: {error}")))
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
            .filter(|item| !is_setting(item))
            .cloned()
            .collect();
        input.push(Item(json!({"type":"compaction_trigger"})));
        let server_items = cx.server_compact(input).await?;
        let pending_ids: Vec<&str> = cx
            .pending_calls()
            .iter()
            .filter_map(function_call_id)
            .collect();
        let mut items: Vec<Item> = server_items
            .into_iter()
            .filter(|item| {
                !is_setting(item)
                    && !is_user_message(item)
                    && function_call_id(item).is_none_or(|id| !pending_ids.contains(&id))
            })
            .collect();
        // Replace endpoint user messages with the original sequence. This
        // preserves duplicates, byte-faithful values, and source ordering even
        // if server compaction omitted or reordered them.
        items.extend(
            cx.items()
                .iter()
                .filter(|item| is_user_message(item))
                .cloned(),
        );
        // Pending function calls must remain valid for late call_id outputs,
        // independent of whether the endpoint omits or rewrites them.
        items.extend(cx.pending_calls().iter().cloned());
        let mut carried = Vec::new();
        for item in cx.pending_calls() {
            if let Some(id) = item.0.get("call_id").and_then(|v| v.as_str()) {
                let id = CallId(id.to_owned());
                if !carried.contains(&id) {
                    carried.push(id);
                }
            }
        }
        items.insert(
            0,
            Item(json!({"type":"message","role":"developer","content":"[compaction-context-v1] Context was compacted; you are the successor. Summary follows; live state (bindings, worktrees, children) is listed after it."})),
        );
        items.push(Item::configuration_update(cx.effort));
        Ok(NewWindow {
            items,
            effort: cx.effort,
            carried,
        })
    }
}

fn is_user_message(item: &Item) -> bool {
    item.0.get("type").and_then(|v| v.as_str()) == Some("message")
        && item.0.get("role").and_then(|v| v.as_str()) == Some("user")
}

fn function_call_id(item: &Item) -> Option<&str> {
    (item.0.get("type").and_then(|v| v.as_str()) == Some("function_call"))
        .then(|| item.0.get("call_id").and_then(|v| v.as_str()))
        .flatten()
}

fn is_setting(item: &Item) -> bool {
    matches!(
        item.0.get("type").and_then(|v| v.as_str()),
        Some("configuration_update" | "additional_tools" | "compaction_trigger")
    )
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
            Item(json!({"type":"additional_tools","tools":[]})),
            Item(json!({"type":"compaction_trigger"})),
        ];
        let usage = Usage::default();
        let compact = |input: Vec<Item>| -> ServerCompactFuture<'_> {
            assert_eq!(input.last().unwrap().0["type"], "compaction_trigger");
            assert!(!input[..input.len() - 1].iter().any(is_setting));
            Box::pin(async {
                Ok(vec![
                    Item(json!({"type":"message","role":"user","content":"later"})),
                    Item(json!({"type":"function_call","call_id":"call-7","name":"changed"})),
                    Item(json!({"type":"message","content":"summary"})),
                    Item(json!({"type":"message","role":"user","content":"keep me"})),
                    Item(json!({"type":"configuration_update","reasoning":{"effort":"low"}})),
                    Item(json!({"type":"additional_tools","tools":[]})),
                    Item(json!({"type":"compaction_trigger"})),
                ])
            })
        };
        let typed_turn = |_instructions: String, _tool: ToolName, _schema: serde_json::Value| {
            Box::pin(async { Ok(json!({"note":"typed"})) }) as TypedTurnFuture<'_>
        };
        let user = Item(json!({"type":"message","role":"user","content":"keep me"}));
        let user_later = Item(json!({"type":"message","role":"user","content":"later"}));
        let source = [source, vec![user.clone(), user.clone(), user_later.clone()]].concat();
        let cx = CompactContext {
            items: &source,
            usage: &usage,
            pending_calls: std::slice::from_ref(&call),
            effort: Effort::Medium,
            server_compact: &compact,
            typed_turn: &typed_turn,
        };
        let window = Server.compact(cx).await.unwrap();
        let calls: Vec<_> = window
            .items
            .iter()
            .filter(|item| item.0.get("type").and_then(|v| v.as_str()) == Some("function_call"))
            .collect();
        assert_eq!(calls, vec![&call]);
        let users: Vec<_> = window
            .items
            .iter()
            .filter(|item| is_user_message(item))
            .collect();
        assert_eq!(users, vec![&user, &user, &user_later]);
        assert_eq!(window.carried, vec![CallId("call-7".into())]);
        assert_eq!(window.items[0].0["role"], "developer");
        assert!(
            window.items[0].0["content"]
                .as_str()
                .unwrap()
                .contains("[compaction-context-v1]")
        );
        assert_eq!(
            window.items.iter().filter(|item| is_setting(item)).count(),
            1
        );
        assert_eq!(
            window.items.last().unwrap().0["type"],
            "configuration_update"
        );
        assert_eq!(
            window.items.last().unwrap().0["reasoning"]["effort"],
            "medium"
        );
        assert_eq!(
            window.items.last().unwrap(),
            &Item::configuration_update(Effort::Medium)
        );
    }

    #[derive(Debug, PartialEq, JsonSchema, serde::Deserialize, serde::Serialize)]
    struct Handoff {
        note: String,
    }

    #[tokio::test]
    async fn typed_turn_sends_schema_and_decodes_json() {
        let items = [];
        let usage = Usage::default();
        let compact =
            |_input: Vec<Item>| -> ServerCompactFuture<'_> { Box::pin(async { Ok(vec![]) }) };
        let typed_turn = |instructions: String, tool: ToolName, schema: serde_json::Value| {
            Box::pin(async move {
                assert_eq!(instructions, "handoff");
                assert_eq!(tool, ToolName("finish".into()));
                assert!(schema["properties"]["note"].is_object());
                Ok(json!({"note":"done"}))
            }) as TypedTurnFuture<'_>
        };
        let cx = CompactContext {
            items: &items,
            usage: &usage,
            pending_calls: &[],
            effort: Effort::Low,
            server_compact: &compact,
            typed_turn: &typed_turn,
        };
        assert_eq!(
            cx.typed_turn::<Handoff>("handoff", ToolName("finish".into()))
                .await
                .unwrap(),
            Handoff {
                note: "done".into()
            }
        );
    }
}
