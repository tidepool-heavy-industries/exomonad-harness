use super::{ResponsesTurn, TransportError, Usage};
use crate::item::Item;
use serde_json::Value;

/// Completed output items are carried by SSE `response.output_item.done`;
/// this backend may leave `response.completed.response.output` empty.
#[derive(Default)]
pub struct ResponseAssembly {
    items: Vec<Item>,
    completed: Option<(String, Usage)>,
}

#[derive(Debug)]
pub enum StreamEvent {
    ItemDone(Item),
    Delta(String),
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
            Some("response.output_text.delta") => Ok(event
                .get("delta")
                .and_then(Value::as_str)
                .map(|s| StreamEvent::Delta(s.to_owned()))),
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
        let (response_id, usage) = self
            .completed
            .ok_or_else(|| TransportError::Stream("missing response.completed".into()))?;
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
        let completed = r#"{"type":"response.completed","response":{"id":"resp_1","output":[],"usage":{"input_tokens":12,"output_tokens":2,"input_tokens_details":{"cached_tokens":0,"cache_write_tokens":0}}}}"#;
        assert!(matches!(a.accept(completed), Ok(None)));
        let turn = a.finish().expect("complete");
        assert_eq!(turn.items.len(), 1);
        assert_eq!(turn.items[0].0["phase"], "final_answer");
        assert_eq!(turn.usage.input_tokens, 12);
    }

    #[test]
    fn missing_completed_is_error() {
        assert!(ResponseAssembly::default().finish().is_err());
    }
}
