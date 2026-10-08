use crate::{
    context::Occurrence,
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
    pub(crate) retained: Vec<(usize, Occurrence)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolName(pub String);

pub struct CompactContext<'a> {
    pub items: &'a [Item],
    pub usage: &'a Usage,
    pub pending_calls: &'a [Item],
    pub(crate) pending_positions: &'a [usize],
    pub(crate) pending_occurrences: &'a [Occurrence],
    pub(crate) occurrences: &'a [Option<Occurrence>],
    pub effort: Effort,
    /// Calls the configured server compaction endpoint without constructing a
    /// second transport request at the strategy boundary.
    pub server_compact: &'a (dyn Fn(Vec<Item>) -> ServerCompactFuture<'a> + Send + Sync),
    /// Caller-provided typed-turn capability: the caller applies the forced
    /// tool choice and returns the decoded JSON value for that tool call.
    pub typed_turn:
        &'a (dyn Fn(String, ToolName, serde_json::Value) -> TypedTurnFuture<'a> + Send + Sync),
    /// A normal Responses request with all tools disallowed. The strategy
    /// validates its returned items before any of them enter history.
    pub text_turn: &'a (dyn Fn(Vec<Item>) -> ServerCompactFuture<'a> + Send + Sync),
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

/// Model-authored text handoff. The source transcript is never replayed as
/// calls, and outstanding invocations remain exact structured items.
pub struct PlainText;

const MAX_RECENT_USER_MESSAGES: usize = 8;
const MAX_RECENT_USER_BYTES: usize = 16 * 1024;
const HANDOFF_MARKER: &str = "[plain-text-compaction-v1]";

#[async_trait]
impl Compactor for PlainText {
    type Summary = String;

    async fn compact(&self, cx: CompactContext<'_>) -> Result<NewWindow, CompactError> {
        let mut prompt: Vec<Item> = cx
            .items()
            .iter()
            .enumerate()
            .filter(|(index, _)| !cx.pending_positions.contains(index))
            .map(|(_, item)| item.clone())
            .collect();
        prompt.push(Item(json!({
            "type":"message", "role":"user",
            "content":"Summarize the conversation so the next assistant can continue the work. Include goals, decisions, important evidence, and unresolved tasks. Write plain text only. Do not call tools."
        })));
        let response = (cx.text_turn)(prompt).await?;
        if response.iter().any(|item| {
            matches!(
                item.0["type"].as_str(),
                Some(
                    "function_call"
                        | "custom_tool_call"
                        | "function_call_output"
                        | "custom_tool_call_output"
                )
            )
        }) {
            return Err(CompactError::Failed(
                "summary response contained a tool item".into(),
            ));
        }
        let summary = response
            .iter()
            .filter_map(assistant_text)
            .collect::<Vec<_>>()
            .join("\n");
        if summary.trim().is_empty() {
            return Err(CompactError::Failed(
                "summary response had no assistant text".into(),
            ));
        }
        let mut retained = Vec::new();
        let mut items = Vec::new();
        for (index, item) in cx.items().iter().enumerate().filter(|(_, item)| {
            standing_instruction(item)
                && !item.0["content"]
                    .as_str()
                    .is_some_and(|text| text.starts_with(HANDOFF_MARKER))
        }) {
            if let Some(source) = &cx.occurrences[index] {
                retained.push((items.len(), source.clone()));
            }
            items.push(item.clone());
        }
        items.push(Item(json!({
            "type":"message", "role":"developer",
            "content":format!("{HANDOFF_MARKER} Earlier conversation summary:\n{summary}")
        })));
        let mut recent = Vec::new();
        let mut bytes = 0;
        for (index, item) in cx
            .items()
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, item)| is_user_message(item))
        {
            let size = serde_json::to_vec(item)
                .expect("Item serialization is infallible")
                .len();
            if recent.len() == MAX_RECENT_USER_MESSAGES || bytes + size > MAX_RECENT_USER_BYTES {
                break;
            }
            bytes += size;
            recent.push((index, item.clone()));
        }
        for (index, item) in recent.into_iter().rev() {
            if let Some(source) = &cx.occurrences[index] {
                retained.push((items.len(), source.clone()));
            }
            items.push(item);
        }
        for occurrence in cx.pending_occurrences {
            retained.push((items.len(), occurrence.clone()));
            items.push(occurrence.item.clone());
        }
        let mut carried = Vec::new();
        for item in cx.pending_calls() {
            if let Some(id) = item.0["call_id"].as_str() {
                let id = CallId(id.to_owned());
                if !carried.contains(&id) {
                    carried.push(id);
                }
            }
        }
        items.push(Item::configuration_update(cx.effort));
        Ok(NewWindow {
            items,
            effort: cx.effort,
            carried,
            retained,
        })
    }
}

fn standing_instruction(item: &Item) -> bool {
    item.0["type"] == "message" && matches!(item.0["role"].as_str(), Some("system" | "developer"))
}

fn assistant_text(item: &Item) -> Option<String> {
    if item.0["type"] != "message" || item.0["role"] != "assistant" {
        return None;
    }
    match &item.0["content"] {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(|part| {
                    (part["type"] == "output_text")
                        .then(|| part["text"].as_str())
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

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
        // External Items do not select pending operations by their wire ID.
        // Store reconciles these raw results before installing the successor.
        let mut items: Vec<Item> = server_items
            .into_iter()
            .filter(|item| !is_setting(item) && !is_user_message(item))
            .collect();
        let mut retained = Vec::new();
        for (index, item) in cx
            .items()
            .iter()
            .enumerate()
            .filter(|(_, item)| is_user_message(item))
        {
            if let Some(source) = &cx.occurrences[index] {
                retained.push((items.len(), source.clone()));
            }
            items.push(item.clone());
        }
        for occurrence in cx.pending_occurrences {
            retained.push((items.len(), occurrence.clone()));
            items.push(occurrence.item.clone());
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
        for (position, _) in &mut retained {
            *position += 1;
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
            retained,
        })
    }
}

fn is_user_message(item: &Item) -> bool {
    item.0.get("type").and_then(|v| v.as_str()) == Some("message")
        && item.0.get("role").and_then(|v| v.as_str()) == Some("user")
}

fn tool_call_id(item: &Item) -> Option<&str> {
    matches!(
        item.0.get("type").and_then(|v| v.as_str()),
        Some("function_call" | "custom_tool_call")
    )
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
    fn source_occurrences(items: &[Item]) -> Vec<Option<Occurrence>> {
        items
            .iter()
            .enumerate()
            .map(|(position, item)| {
                let hash = crate::item::ItemHash(
                    blake3::hash(&serde_json::to_vec(item).unwrap())
                        .to_hex()
                        .to_string(),
                );
                let request = crate::model::RequestId("strategy-source".into());
                Some(Occurrence {
                    request: request.clone(),
                    position: position as i64,
                    hash: hash.clone(),
                    item: item.clone(),
                    output_operation: None,
                    origin: crate::context::Origin {
                        request,
                        position: position as i64,
                        hash,
                    },
                    sources: vec![],
                    note: false,
                    overlays: vec![],
                })
            })
            .collect()
    }

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
        let occurrences = source_occurrences(&source);
        let selected = [occurrences[0].clone().unwrap()];
        let cx = CompactContext {
            items: &source,
            occurrences: &occurrences,
            pending_positions: &[0],
            pending_occurrences: &selected,
            usage: &usage,
            pending_calls: std::slice::from_ref(&call),
            effort: Effort::Medium,
            server_compact: &compact,
            typed_turn: &typed_turn,
            text_turn: &compact,
        };
        let window = Server.compact(cx).await.unwrap();
        let calls: Vec<_> = window
            .items
            .iter()
            .filter(|item| item.0.get("type").and_then(|v| v.as_str()) == Some("function_call"))
            .collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0["name"], "changed");
        assert_eq!(calls[1], &call);
        assert!(
            window
                .retained
                .iter()
                .any(|(_, source)| source.origin == selected[0].origin && source.item == call)
        );
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
            occurrences: &[],
            pending_positions: &[],
            pending_occurrences: &[],
            usage: &usage,
            pending_calls: &[],
            effort: Effort::Low,
            server_compact: &compact,
            typed_turn: &typed_turn,
            text_turn: &compact,
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

    #[tokio::test]
    async fn plain_text_keeps_both_pending_call_kinds_as_exact_items() {
        let function = Item(
            json!({"type":"function_call","call_id":"f","name":"run","arguments":"{}","opaque":"function"}),
        );
        let custom = Item(
            json!({"type":"custom_tool_call","call_id":"c","name":"shell","input":"echo hi","opaque":"custom"}),
        );
        let history = vec![
            Item(json!({"type":"message","role":"developer","content":"standing"})),
            function.clone(),
            custom.clone(),
            Item(json!({"type":"message","role":"user","content":"recent"})),
        ];
        let usage = Usage::default();
        let server =
            |_input: Vec<Item>| -> ServerCompactFuture<'_> { Box::pin(async { unreachable!() }) };
        let typed = |_instructions: String,
                     _tool: ToolName,
                     _schema: serde_json::Value|
         -> TypedTurnFuture<'_> { Box::pin(async { unreachable!() }) };
        let text = |input: Vec<Item>| -> ServerCompactFuture<'_> {
            assert!(!input.contains(&function));
            assert!(!input.contains(&custom));
            Box::pin(async {
                Ok(vec![Item(
                    json!({"type":"message","role":"assistant","content":"summary"}),
                )])
            })
        };
        let pending = [function.clone(), custom.clone()];
        let occurrences = source_occurrences(&history);
        let selected = [
            occurrences[1].clone().unwrap(),
            occurrences[2].clone().unwrap(),
        ];
        let window = PlainText
            .compact(CompactContext {
                items: &history,
                occurrences: &occurrences,
                pending_positions: &[1, 2],
                pending_occurrences: &selected,
                usage: &usage,
                pending_calls: &pending,
                effort: Effort::High,
                server_compact: &server,
                typed_turn: &typed,
                text_turn: &text,
            })
            .await
            .unwrap();
        assert!(window.items.contains(&function));
        assert!(window.items.contains(&custom));
        assert_eq!(window.carried, vec![CallId("f".into()), CallId("c".into())]);
        assert_eq!(
            window.items.last(),
            Some(&Item::configuration_update(Effort::High))
        );
        assert_eq!(
            window
                .items
                .iter()
                .filter(|item| tool_call_id(item).is_some())
                .count(),
            2
        );
    }
}
