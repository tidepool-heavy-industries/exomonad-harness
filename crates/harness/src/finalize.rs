//! Typed `finalize` tool support for adapter replies.
//!
//! The wire arguments are always exactly `{"result": T}`. This module only
//! describes and decodes the existing finalize function call; it does not
//! provide a separate answer/output channel.

use crate::item::Item;
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use thiserror::Error;

pub const FINALIZE_TOOL_NAME: &str = "finalize";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FinalizeError {
    #[error("expected a finalize function_call")]
    WrongTool,
    #[error("finalize was already completed")]
    AlreadyFinalized,
    #[error("malformed finalize arguments: {0}")]
    MalformedArguments(String),
    #[error("finalize arguments must contain exactly the result field")]
    InvalidEnvelope,
    #[error("could not serialize finalize result: {0}")]
    SerializeResult(String),
    #[error("unsupported JSON schema at {path}: {keyword}")]
    UnsupportedSchema { path: String, keyword: String },
}

/// Make the strict Responses function-tool schema for a typed reply.
pub fn tool_schema<T: JsonSchema>() -> Result<Value, FinalizeError> {
    let result_schema =
        serde_json::to_value(schemars::schema_for!(T)).expect("JsonSchema serializes to JSON");
    let result_schema = normalize_schema(result_schema, "$")?;
    Ok(json!({
        "type": "function",
        "name": FINALIZE_TOOL_NAME,
        "description": "Return the typed final reply.",
        "strict": true,
        "parameters": {
            "type": "object",
            "properties": { "result": result_schema },
            "required": ["result"],
            "additionalProperties": false
        }
    }))
}

/// Convert schemars' schema to the conservative subset accepted by strict
/// Responses tools. Unknown validation keywords are rejected rather than
/// discarded, since dropping them could silently broaden the reply contract.
fn normalize_schema(schema: Value, path: &str) -> Result<Value, FinalizeError> {
    let object = schema
        .as_object()
        .ok_or_else(|| unsupported(path, "schema must be an object"))?;
    let schema_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| unsupported(path, "missing or non-string type"))?;

    // These are descriptive or document-container metadata, not constraints.
    const METADATA: &[&str] = &[
        "$schema",
        "$defs",
        "definitions",
        "title",
        "description",
        "default",
        "examples",
        "deprecated",
        "readOnly",
        "writeOnly",
    ];
    let allowed: &[&str] = match schema_type {
        "object" => &["type", "properties", "required", "additionalProperties"],
        "array" => &["type", "items", "minItems", "maxItems", "uniqueItems"],
        "string" => &[
            "type",
            "format",
            "minLength",
            "maxLength",
            "pattern",
            "enum",
            "const",
        ],
        "integer" | "number" => &[
            "type",
            "format",
            "minimum",
            "maximum",
            "exclusiveMinimum",
            "exclusiveMaximum",
            "multipleOf",
            "enum",
            "const",
        ],
        "boolean" | "null" => &["type", "enum", "const"],
        other => return Err(unsupported(path, format!("unsupported type {other}"))),
    };
    for key in object.keys() {
        if METADATA.contains(&key.as_str()) {
            continue;
        }
        if !allowed.contains(&key.as_str()) {
            return Err(unsupported(path, key));
        }
    }

    let mut normalized = Map::new();
    normalized.insert("type".into(), Value::String(schema_type.into()));
    match schema_type {
        "object" => {
            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| unsupported(path, "object requires properties object"))?;
            let mut normalized_properties = Map::new();
            for (name, property) in properties {
                let property_path = format!("{path}.properties.{name}");
                normalized_properties.insert(
                    name.clone(),
                    normalize_schema(property.clone(), &property_path)?,
                );
            }
            normalized.insert(
                "required".into(),
                Value::Array(properties.keys().cloned().map(Value::String).collect()),
            );
            normalized.insert("properties".into(), Value::Object(normalized_properties));
            normalized.insert("additionalProperties".into(), Value::Bool(false));
        }
        "array" => {
            let items = object
                .get("items")
                .ok_or_else(|| unsupported(path, "array requires items schema"))?;
            normalized.insert(
                "items".into(),
                normalize_schema(items.clone(), &format!("{path}.items"))?,
            );
            copy_constraints(
                object,
                &mut normalized,
                &["minItems", "maxItems", "uniqueItems"],
            );
        }
        "string" => copy_constraints(
            object,
            &mut normalized,
            &[
                "format",
                "minLength",
                "maxLength",
                "pattern",
                "enum",
                "const",
            ],
        ),
        "integer" | "number" => copy_constraints(
            object,
            &mut normalized,
            &[
                "format",
                "minimum",
                "maximum",
                "exclusiveMinimum",
                "exclusiveMaximum",
                "multipleOf",
                "enum",
                "const",
            ],
        ),
        "boolean" | "null" => copy_constraints(object, &mut normalized, &["enum", "const"]),
        _ => unreachable!("type checked above"),
    }
    Ok(Value::Object(normalized))
}

fn copy_constraints(source: &Map<String, Value>, target: &mut Map<String, Value>, keys: &[&str]) {
    for key in keys {
        if let Some(value) = source.get(*key) {
            target.insert((*key).into(), value.clone());
        }
    }
}

fn unsupported(path: &str, keyword: impl std::fmt::Display) -> FinalizeError {
    FinalizeError::UnsupportedSchema {
        path: path.into(),
        keyword: keyword.to_string(),
    }
}

/// Convert a typed reply to the exact arguments object expected by finalize.
pub fn respond_to_finalize<T: Serialize>(reply: T) -> Result<Value, FinalizeError> {
    serde_json::to_value(reply)
        .map(|result| json!({"result": result}))
        .map_err(|error| FinalizeError::SerializeResult(error.to_string()))
}

/// Stateful decoder: at most one valid finalize call may be accepted.
#[derive(Clone, Debug, Default)]
pub struct FinalizeParser {
    finalized: bool,
}

impl FinalizeParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a completed Responses `function_call` item.
    ///
    /// Arguments may be the decoded object form or the JSON string form used
    /// by Responses. A successful decode consumes this parser's one finalize.
    pub fn parse_completed<T: DeserializeOwned>(
        &mut self,
        item: &Item,
    ) -> Result<T, FinalizeError> {
        if self.finalized {
            return Err(FinalizeError::AlreadyFinalized);
        }

        let object = item.0.as_object().ok_or(FinalizeError::WrongTool)?;
        if object.get("type").and_then(Value::as_str) != Some("function_call")
            || object.get("name").and_then(Value::as_str) != Some(FINALIZE_TOOL_NAME)
        {
            return Err(FinalizeError::WrongTool);
        }

        let arguments = match object.get("arguments") {
            Some(Value::String(text)) => serde_json::from_str::<Value>(text)
                .map_err(|error| FinalizeError::MalformedArguments(error.to_string()))?,
            Some(Value::Object(arguments)) => Value::Object(arguments.clone()),
            _ => {
                return Err(FinalizeError::MalformedArguments(
                    "expected a JSON object or object-encoded JSON string".into(),
                ));
            }
        };

        let mut args = match arguments {
            Value::Object(args) => args,
            _ => return Err(FinalizeError::InvalidEnvelope),
        };
        let result = args
            .remove("result")
            .ok_or(FinalizeError::InvalidEnvelope)?;
        if !args.is_empty() {
            return Err(FinalizeError::InvalidEnvelope);
        }
        let decoded = serde_json::from_value(result)
            .map_err(|error| FinalizeError::MalformedArguments(error.to_string()))?;
        self.finalized = true;
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
    struct Reply {
        answer: u32,
    }

    #[derive(Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
    struct Nested {
        count: u32,
    }

    #[derive(Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
    struct OptionalReply {
        maybe: Option<String>,
    }

    #[derive(Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
    #[serde(untagged)]
    enum UnionReply {
        Text(String),
        Number(u32),
    }

    #[derive(Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
    struct ReferencedReply {
        nested: Nested,
    }

    fn call(name: &str, arguments: Value) -> Item {
        Item(json!({
            "type": "function_call",
            "call_id": "final-1",
            "name": name,
            "arguments": arguments
        }))
    }

    #[test]
    fn strict_schema_wraps_typed_result_and_respond_maps_into_it() {
        let schema = tool_schema::<Reply>().unwrap();
        assert_eq!(schema["name"], FINALIZE_TOOL_NAME);
        assert_eq!(schema["strict"], true);
        assert_eq!(schema["parameters"]["required"], json!(["result"]));
        assert_eq!(schema["parameters"]["additionalProperties"], false);
        assert!(schema["parameters"]["properties"]["result"].is_object());
        assert_eq!(
            respond_to_finalize(Reply { answer: 42 }).unwrap(),
            json!({"result":{"answer":42}})
        );
    }

    #[test]
    fn normalizes_nested_objects_and_arrays_for_strict_mode() {
        let result = normalize_schema(
            json!({
                "type":"object",
                "title":"Reply",
                "properties":{
                    "nested":{
                        "type":"object",
                        "properties":{"count":{"type":"integer"}},
                        "required":["count"],
                        "additionalProperties":true
                    },
                    "entries":{"type":"array","items":{"type":"string"}}
                },
                "required":["nested"],
                "additionalProperties":true
            }),
            "$",
        )
        .unwrap();
        assert_eq!(result["additionalProperties"], false);
        assert_eq!(result["required"], json!(["entries", "nested"]));
        assert_eq!(
            result["properties"]["nested"]["additionalProperties"],
            false
        );
        assert_eq!(result["properties"]["nested"]["required"], json!(["count"]));
        assert_eq!(result["properties"]["entries"]["items"]["type"], "string");
    }

    #[test]
    fn explicitly_rejects_optional_union_and_reference_schemas() {
        assert!(matches!(
            tool_schema::<OptionalReply>(),
            Err(FinalizeError::UnsupportedSchema { .. })
        ));
        assert!(matches!(
            tool_schema::<UnionReply>(),
            Err(FinalizeError::UnsupportedSchema { .. })
        ));
        assert!(matches!(
            tool_schema::<ReferencedReply>(),
            Err(FinalizeError::UnsupportedSchema { .. })
        ));
    }

    #[test]
    fn parses_completed_finalize_from_json_arguments() {
        let mut parser = FinalizeParser::new();
        let parsed: Reply = parser
            .parse_completed(&call("finalize", json!("{\"result\":{\"answer\":42}}")))
            .unwrap();
        assert_eq!(parsed, Reply { answer: 42 });
    }

    #[test]
    fn rejects_wrong_name_malformed_unknown_missing_and_second_finalize() {
        let mut parser = FinalizeParser::new();
        assert_eq!(
            parser.parse_completed::<Reply>(&call("other", json!({"result":{"answer":42}}))),
            Err(FinalizeError::WrongTool)
        );
        assert!(matches!(
            parser.parse_completed::<Reply>(&call("finalize", json!("{"))),
            Err(FinalizeError::MalformedArguments(_))
        ));
        assert_eq!(
            parser.parse_completed::<Reply>(&call(
                "finalize",
                json!({"result":{"answer":42},"extra":true})
            )),
            Err(FinalizeError::InvalidEnvelope)
        );
        assert_eq!(
            parser.parse_completed::<Reply>(&call("finalize", json!({}))),
            Err(FinalizeError::InvalidEnvelope)
        );

        let valid = call("finalize", json!({"result":{"answer":42}}));
        assert_eq!(
            parser.parse_completed::<Reply>(&valid),
            Ok(Reply { answer: 42 })
        );
        assert_eq!(
            parser.parse_completed::<Reply>(&valid),
            Err(FinalizeError::AlreadyFinalized)
        );
    }

    #[test]
    fn rejects_invalid_t_payload_without_consuming_finalize() {
        let mut parser = FinalizeParser::new();
        assert!(matches!(
            parser.parse_completed::<Reply>(&call("finalize", json!({"result":{"answer":"no"}}))),
            Err(FinalizeError::MalformedArguments(_))
        ));
        assert_eq!(
            parser.parse_completed::<Reply>(&call("finalize", json!({"result":{"answer":7}}))),
            Ok(Reply { answer: 7 })
        );
    }
}
