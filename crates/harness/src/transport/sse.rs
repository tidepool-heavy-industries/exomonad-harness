use super::{ResponsesTurn, TransportError, Usage};
use crate::item::Item;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Completed output items are carried by SSE `response.output_item.done`;
/// this backend may leave `response.completed.response.output` empty.
#[derive(Default)]
pub struct ResponseAssembly {
    items: Vec<Item>,
    completed: Option<(String, Usage)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputChannel {
    Assistant,
    ReasoningSummary,
    Reasoning,
    ToolArguments,
    ToolInput,
    Refusal,
}

#[derive(Debug)]
pub enum StreamEvent {
    ItemDone(Item),
    Delta {
        item_id: String,
        channel: OutputChannel,
        index: u64,
        text: String,
    },
}

impl ResponseAssembly {
    /// Returns a live event for the loop, while retaining completed items for
    /// the final turn. The caller is responsible for SSE framing.
    pub fn accept(&mut self, data: &str) -> Result<Option<StreamEvent>, TransportError> {
        let event: Value = serde_json::from_str(data)
            .map_err(|_| TransportError::Stream("invalid SSE JSON".into()))?;
        match event.get("type").and_then(Value::as_str) {
            Some("response.output_item.done") => {
                let item = event
                    .get("item")
                    .cloned()
                    .ok_or_else(|| TransportError::Stream("missing done item".into()))?;
                self.items.push(Item(item.clone()));
                Ok(Some(StreamEvent::ItemDone(Item(item))))
            }
            Some(
                kind @ ("response.output_text.delta"
                | "response.reasoning_summary_text.delta"
                | "response.function_call_arguments.delta"
                | "response.custom_tool_call_input.delta"
                | "response.refusal.delta"
                | "response.reasoning_text.delta"),
            ) => {
                let item_id = event
                    .get("item_id")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        if kind == "response.custom_tool_call_input.delta" {
                            event.get("call_id").and_then(Value::as_str)
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| TransportError::Stream("delta missing item_id".into()))?;
                Ok(event
                    .get("delta")
                    .and_then(Value::as_str)
                    .map(|text| StreamEvent::Delta {
                        item_id: item_id.to_owned(),
                        channel: match kind {
                            "response.output_text.delta" => OutputChannel::Assistant,
                            "response.reasoning_summary_text.delta" => {
                                OutputChannel::ReasoningSummary
                            }
                            "response.reasoning_text.delta" => OutputChannel::Reasoning,
                            "response.function_call_arguments.delta" => {
                                OutputChannel::ToolArguments
                            }
                            "response.custom_tool_call_input.delta" => OutputChannel::ToolInput,
                            "response.refusal.delta" => OutputChannel::Refusal,
                            _ => unreachable!("matched channel"),
                        },
                        index: event
                            .get("content_index")
                            .or_else(|| event.get("summary_index"))
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        text: text.to_owned(),
                    }))
            }
            Some("response.completed") => {
                let response = event
                    .get("response")
                    .ok_or_else(|| TransportError::Stream("missing response".into()))?;
                let id = response
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| TransportError::Stream("missing response id".into()))?;
                let usage = response.get("usage").unwrap_or(&Value::Null);
                let details = usage.get("input_tokens_details").unwrap_or(&Value::Null);
                self.completed = Some((
                    id.to_owned(),
                    Usage {
                        reported: usage.get("input_tokens").and_then(Value::as_u64).is_some()
                            && usage.get("output_tokens").and_then(Value::as_u64).is_some(),
                        input_tokens: usage
                            .get("input_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        output_tokens: usage
                            .get("output_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        cached_tokens: details
                            .get("cached_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        cache_write_tokens: details
                            .get("cache_write_tokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    },
                ));
                Ok(None)
            }
            Some("response.failed" | "error") => Err(TransportError::Stream(
                "server signaled response failure".into(),
            )),
            _ => Ok(None),
        }
    }

    pub fn finish(self) -> Result<ResponsesTurn, TransportError> {
        let (response_id, usage) = self.completed.ok_or(TransportError::IncompleteResponse(
            super::StreamInterruption::MissingCompletion,
        ))?;
        Ok(ResponsesTurn {
            response_id,
            items: self.items,
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_done_supplies_phase_when_completed_output_is_empty() {
        let mut a = ResponseAssembly::default();
        let done = r#"{"type":"response.output_item.done","item":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"ok"}]}}"#;
        assert!(matches!(a.accept(done), Ok(Some(StreamEvent::ItemDone(_)))));
        let completed = r#"{"type":"response.completed","response":{"id":"resp_1","output":[],"usage":{"input_tokens":12,"output_tokens":2,"input_tokens_details":{"cached_tokens":3,"cache_write_tokens":4}}}}"#;
        assert!(matches!(a.accept(completed), Ok(None)));
        let turn = a.finish().expect("complete");
        assert_eq!(turn.items.len(), 1);
        assert_eq!(turn.items[0].0["phase"], "final_answer");
        assert_eq!(turn.response_id, "resp_1");
        assert_eq!(turn.usage.input_tokens, 12);
        assert_eq!(turn.usage.output_tokens, 2);
        assert_eq!(turn.usage.cached_tokens, 3);
        assert_eq!(turn.usage.cache_write_tokens, 4);
    }

    #[test]
    fn live_output_deltas_keep_item_channel_and_block_identity() {
        let mut assembly = ResponseAssembly::default();
        for (channel, expected) in [
            ("response.output_text.delta", OutputChannel::Assistant),
            (
                "response.reasoning_summary_text.delta",
                OutputChannel::ReasoningSummary,
            ),
            ("response.reasoning_text.delta", OutputChannel::Reasoning),
            (
                "response.function_call_arguments.delta",
                OutputChannel::ToolArguments,
            ),
            (
                "response.custom_tool_call_input.delta",
                OutputChannel::ToolInput,
            ),
            ("response.refusal.delta", OutputChannel::Refusal),
        ] {
            let data = serde_json::json!({"type":channel,"item_id":"message-2","content_index":3,"delta":"λ"});
            match assembly.accept(&data.to_string()).unwrap().unwrap() {
                StreamEvent::Delta {
                    item_id,
                    channel: actual,
                    index,
                    text,
                } => {
                    assert_eq!(item_id, "message-2");
                    assert_eq!(actual, expected);
                    assert_eq!(index, 3);
                    assert_eq!(text, "λ");
                }
                _ => panic!("expected delta"),
            }
        }
        assert!(
            assembly
                .accept(r#"{"type":"response.output_text.delta","delta":"unscoped"}"#)
                .is_err()
        );
    }

    #[test]
    fn missing_completed_is_error() {
        assert!(ResponseAssembly::default().finish().is_err());
    }
}

#[cfg(test)]
mod lite_tests {
    use super::*;
    use crate::item::ToolInput;
    use serde_json::json;

    #[test]
    fn lite_namespaced_tool_done_uses_existing_call_and_completion_contract() {
        let mut assembly = ResponseAssembly::default();
        for (kind, name, payload) in [
            (
                "function_call",
                "yield",
                json!({"arguments":"{\"until\":null}"}),
            ),
            ("custom_tool_call", "cell", json!({"input":"raw Haskell λ"})),
        ] {
            let mut item = json!({"type":kind,"namespace":"functions","name":name,"call_id":format!("original-{name}"),"id":format!("item-{name}"),"encrypted_function_args":["retained"]});
            item.as_object_mut()
                .unwrap()
                .extend(payload.as_object().unwrap().clone());
            let event = json!({"type":"response.output_item.done","item":item});
            let Some(StreamEvent::ItemDone(retained)) =
                assembly.accept(&event.to_string()).unwrap()
            else {
                panic!("completed call expected");
            };
            assert_eq!(retained.0, item);
            let call = retained.tool_call().unwrap().unwrap();
            assert_eq!(call.call_id.0, format!("original-{name}"));
            assert_eq!(call.name, name);
            match call.input {
                ToolInput::Function(args) => assert_eq!(args, json!({"until":null})),
                ToolInput::Custom(input) => assert_eq!(input, "raw Haskell λ"),
            }
        }
        assembly.accept(&json!({"type":"response.completed","response":{"id":"lite-response","output":[],"usage":{"input_tokens":12,"output_tokens":7,"input_tokens_details":{"cached_tokens":4}}}}).to_string()).unwrap();
        let completed = assembly.finish().unwrap();
        assert_eq!(completed.items.len(), 2);
        assert_eq!(completed.response_id, "lite-response");
        assert_eq!(completed.usage.cached_tokens, 4);
    }

    #[test]
    fn custom_input_delta_uses_native_call_id_fallback_without_renaming() {
        let mut assembly = ResponseAssembly::default();
        let event = json!({"type":"response.custom_tool_call_input.delta","call_id":"exact-call","delta":"λ"});
        let Some(StreamEvent::Delta {
            item_id,
            channel,
            text,
            ..
        }) = assembly.accept(&event.to_string()).unwrap()
        else {
            panic!("tool input delta expected");
        };
        assert_eq!(item_id, "exact-call");
        assert_eq!(channel, OutputChannel::ToolInput);
        assert_eq!(text, "λ");
        let mut preferred = event.clone();
        preferred["item_id"] = Value::Null;
        let Some(StreamEvent::Delta { item_id, .. }) =
            assembly.accept(&preferred.to_string()).unwrap()
        else {
            panic!("tool input delta expected");
        };
        assert_eq!(item_id, "exact-call");
        preferred["item_id"] = json!("native-item");
        let Some(StreamEvent::Delta { item_id, .. }) =
            assembly.accept(&preferred.to_string()).unwrap()
        else {
            panic!("tool input delta expected");
        };
        assert_eq!(item_id, "native-item");
        preferred.as_object_mut().unwrap().remove("item_id");
        preferred.as_object_mut().unwrap().remove("call_id");
        assert!(assembly.accept(&preferred.to_string()).is_err());
    }
}
