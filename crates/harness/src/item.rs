use crate::{
    model::{CallId, Effort},
    turn::JobOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Wire-faithful item; unknown fields survive replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Item(pub Value);

/// One hash identifies identical canonical item bytes across request branches.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ItemHash(pub String);

/// The wire input keeps freeform text distinct from JSON function arguments.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolInput {
    Function(Value),
    Custom(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub call_id: CallId,
    pub name: String,
    pub input: ToolInput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Function,
    Custom,
}

impl ToolInput {
    pub fn kind(&self) -> ToolKind {
        match self {
            Self::Function(_) => ToolKind::Function,
            Self::Custom(_) => ToolKind::Custom,
        }
    }
}

impl Item {
    /// `None` is a non-call item; a recognized but malformed call is an error.
    pub fn tool_call(&self) -> Result<Option<ToolCall>, &'static str> {
        let Some(kind) = self.0.get("type").and_then(Value::as_str) else {
            return Ok(None);
        };
        if kind != "function_call" && kind != "custom_tool_call" {
            return Ok(None);
        }
        let object = self.0.as_object().ok_or("malformed tool call")?;
        let call_id = object
            .get("call_id")
            .and_then(Value::as_str)
            .ok_or("missing call_id")?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .ok_or("missing tool name")?;
        let input = if kind == "custom_tool_call" {
            ToolInput::Custom(
                object
                    .get("input")
                    .and_then(Value::as_str)
                    .ok_or("invalid custom input")?
                    .to_owned(),
            )
        } else {
            let arguments = object
                .get("arguments")
                .ok_or("missing function arguments")?;
            let args = match arguments {
                Value::String(text) => {
                    serde_json::from_str(text).map_err(|_| "invalid function arguments")?
                }
                Value::Object(map) => Value::Object(map.clone()),
                _ => return Err("invalid function arguments"),
            };
            ToolInput::Function(args)
        };
        Ok(Some(ToolCall {
            call_id: CallId(call_id.to_owned()),
            name: name.to_owned(),
            input,
        }))
    }

    /// The call kind must come from the persisted invocation, including recovery.
    pub fn tool_output(call_id: &CallId, kind: ToolKind, output: &JobOutput) -> Self {
        let value = match output {
            JobOutput::Completed(Ok(value)) => value.clone(),
            JobOutput::Completed(Err(error)) => json!({ "error": error }),
            JobOutput::Cancelled => json!({ "error": "job cancelled" }),
            JobOutput::Interrupted => json!({ "error": "job interrupted" }),
            JobOutput::CancellationUnconfirmed(detail) => {
                json!({ "error": "cancellation unconfirmed", "detail": detail })
            }
        };
        match kind {
            ToolKind::Function => Self(json!({
                "type": "function_call_output", "call_id": call_id.0,
                "output": serde_json::to_string(&value).expect("JSON value serialization cannot fail")
            })),
            ToolKind::Custom => Self(json!({
                "type": "custom_tool_call_output", "call_id": call_id.0,
                "output": match output {
                    JobOutput::Completed(Ok(Value::String(text))) => text.clone(),
                    _ => serde_json::to_string(&value).expect("JSON value serialization cannot fail"),
                }
            })),
        }
    }
}

impl Item {
    /// Whether this is a positional reasoning-effort update.
    pub fn is_configuration_update(&self) -> bool {
        self.0.get("type").and_then(Value::as_str) == Some("configuration_update")
    }

    /// Read the effort carried by a well-formed settings item.
    pub fn configuration_effort(&self) -> Option<Effort> {
        if !self.is_configuration_update() {
            return None;
        }
        serde_json::from_value(self.0.get("reasoning")?.get("effort")?.clone()).ok()
    }

    /// Construct the sole settings item supported by the harness.
    pub fn configuration_update(effort: Effort) -> Self {
        Self(json!({
            "type": "configuration_update",
            "reasoning": { "effort": effort }
        }))
    }
}

#[cfg(test)]
mod tool_contract_tests {
    use super::*;

    #[test]
    fn raw_custom_input_and_output_survive_item_serialization() {
        let raw = "line 1\n\"quoted\" \\\\ λ";
        let item = Item(json!({"type":"custom_tool_call","call_id":"c","name":"cell","input":raw}));
        let roundtrip: Item = serde_json::from_str(&serde_json::to_string(&item).unwrap()).unwrap();
        let call = roundtrip.tool_call().unwrap().unwrap();
        assert_eq!(call.input, ToolInput::Custom(raw.into()));
        let output = Item::tool_output(
            &call.call_id,
            call.input.kind(),
            &JobOutput::Completed(Ok(Value::String(raw.into()))),
        );
        assert_eq!(output.0["type"], "custom_tool_call_output");
        assert_eq!(output.0["output"], raw);
        assert!(
            Item(json!({"type":"custom_tool_call","call_id":"bad","name":"cell","input":{}}))
                .tool_call()
                .is_err()
        );
    }

    #[test]
    fn tool_kind_uses_wire_stable_names() {
        assert_eq!(
            serde_json::to_string(&ToolKind::Function).unwrap(),
            "\"function\""
        );
        assert_eq!(
            serde_json::to_string(&ToolKind::Custom).unwrap(),
            "\"custom\""
        );
        assert_eq!(
            serde_json::from_str::<ToolKind>("\"custom\"").unwrap(),
            ToolKind::Custom
        );
    }
}
