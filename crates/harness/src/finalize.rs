//! Strict function-tool schema admission, host argument projection, and typed
//! `finalize` support for adapter replies.
//!
//! The finalize envelope is always exactly `{"result": T}`; it does not provide
//! a separate answer/output channel.

use crate::item::Item;
pub use harness_schema::{FunctionToolSchema, SchemaError};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use thiserror::Error;

pub const FINALIZE_TOOL_NAME: &str = "finalize";

/// Proof that an Engine-owned completion item satisfies its advertised schema.
/// Only the existing finalize parser can produce this value.
pub(crate) struct ValidatedFinalize {
    item: Item,
    schema: Value,
}

impl ValidatedFinalize {
    pub(crate) fn parse(item: &Item, schema: &Value) -> Result<Self, FinalizeError> {
        let result_schema = &schema["parameters"]["properties"]["result"];
        if tool_schema_from_result_schema(result_schema.clone())? != *schema {
            return Err(FinalizeError::WrongTool);
        }
        FinalizeParser::new().parse_completed_with_result_schema(item, result_schema)?;
        Ok(Self {
            item: item.clone(),
            schema: schema.clone(),
        })
    }

    pub(crate) fn item(&self) -> &Item {
        &self.item
    }
    pub(crate) fn schema(&self) -> &Value {
        &self.schema
    }
}

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
    #[error("JSON schema constraints exclude every type variant at {path}")]
    UnsatisfiableSchema { path: String },
    #[error("function arguments have ambiguous optional-null semantics at {0}")]
    AmbiguousArguments(String),
    #[error("finalize result does not match schema at {0}")]
    ResultSchemaMismatch(String),
}

impl From<SchemaError> for FinalizeError {
    fn from(error: SchemaError) -> Self {
        match error {
            SchemaError::UnsupportedSchema { path, keyword } => {
                Self::UnsupportedSchema { path, keyword }
            }
            SchemaError::UnsatisfiableSchema { path } => Self::UnsatisfiableSchema { path },
            SchemaError::AmbiguousArguments(path) => Self::AmbiguousArguments(path),
            SchemaError::ValueSchemaMismatch(path) => Self::ResultSchemaMismatch(path),
        }
    }
}

pub(crate) fn normalize_schema(schema: Value, path: &str) -> Result<Value, FinalizeError> {
    harness_schema::normalize_schema(schema, path).map_err(Into::into)
}

pub(crate) fn validate_function_parameters(schema: &Value) -> Result<(), FinalizeError> {
    harness_schema::validate_function_parameters(schema).map_err(Into::into)
}

pub(crate) fn validate_result(
    value: &Value,
    schema: &Value,
    path: &str,
) -> Result<(), FinalizeError> {
    harness_schema::validate_value(value, schema, path).map_err(Into::into)
}

/// Make the strict Responses function-tool schema for a typed reply.
pub fn tool_schema<T: JsonSchema>() -> Result<Value, FinalizeError> {
    let result_schema =
        serde_json::to_value(schemars::schema_for!(T)).expect("JsonSchema serializes to JSON");
    tool_schema_from_result_schema(result_schema)
}

/// Build the strict tool from a persisted contract's result JSON Schema.
/// This uses the same conservative validation as a Rust `JsonSchema` type.
pub fn tool_schema_from_result_schema(result_schema: Value) -> Result<Value, FinalizeError> {
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

    /// Decode a dynamic contract reply and verify the conservative structural
    /// schema subset locally. A replay transport does not itself enforce the
    /// provider's strict tool schema.
    pub fn parse_completed_with_result_schema(
        &mut self,
        item: &Item,
        result_schema: &Value,
    ) -> Result<Value, FinalizeError> {
        let result: Value = self.parse_completed(item)?;
        if let Err(error) = validate_result(&result, result_schema, "$") {
            // Parsing alone does not complete finalize: a schema-invalid call
            // must leave the parser usable for a later valid call.
            self.finalized = false;
            return Err(error);
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "finalize/properties_tests.rs"]
mod properties;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_result_rejects_wrong_shape_and_preserves_json() {
        let schema = tool_schema_from_result_schema(json!({
            "type":"object", "properties":{"answer":{"type":"string"}},
            "required":["answer"],"additionalProperties":false
        }))
        .unwrap();
        let result_schema = &schema["parameters"]["properties"]["result"];
        let good = call("finalize", json!({"result":{"answer":"ready"}}));
        assert_eq!(
            FinalizeParser::new()
                .parse_completed_with_result_schema(&good, result_schema)
                .unwrap(),
            json!({"answer":"ready"})
        );
        for bad in [
            json!({"result":{"answer":7}}),
            json!({"result":{"answer":"ready","extra":true}}),
            json!({"result":{}}),
        ] {
            assert!(matches!(
                FinalizeParser::new()
                    .parse_completed_with_result_schema(&call("finalize", bad), result_schema),
                Err(FinalizeError::ResultSchemaMismatch(_))
            ));
        }
    }

    #[test]
    fn dynamic_schema_invalid_call_does_not_consume_finalize_parser() {
        let schema = tool_schema_from_result_schema(json!({
            "type":"object", "properties":{"answer":{"type":"string"}}
        }))
        .unwrap();
        let result_schema = &schema["parameters"]["properties"]["result"];
        let mut parser = FinalizeParser::new();
        assert!(matches!(
            parser.parse_completed_with_result_schema(
                &call("finalize", json!({"result":{"answer":7}})),
                result_schema
            ),
            Err(FinalizeError::ResultSchemaMismatch(_))
        ));
        assert_eq!(
            parser
                .parse_completed_with_result_schema(
                    &call("finalize", json!({"result":{"answer":"valid"}})),
                    result_schema
                )
                .unwrap(),
            json!({"answer":"valid"})
        );
        assert!(matches!(
            parser.parse_completed_with_result_schema(
                &call("finalize", json!({"result":{"answer":"again"}})),
                result_schema
            ),
            Err(FinalizeError::AlreadyFinalized)
        ));
    }

    #[test]
    fn rejects_pattern_schema_instead_of_advertising_unchecked_constraint() {
        assert!(matches!(
            tool_schema_from_result_schema(json!({
                "type":"object", "properties":{
                    "answer":{"type":"string","pattern":"^ready$"}
                }
            })),
            Err(FinalizeError::UnsupportedSchema { keyword, .. }) if keyword == "pattern"
        ));
    }
    use serde::Deserialize;
    use std::collections::HashSet;
    type StringSet = HashSet<String>;

    #[derive(Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
    struct Reply {
        answer: String,
    }

    #[derive(Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
    struct Nested {
        count: String,
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

    fn email_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": "string", "format": "email"})
    }

    #[derive(Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
    struct FormattedReply {
        #[schemars(schema_with = "email_schema")]
        email: String,
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
            respond_to_finalize(Reply {
                answer: "ok".into()
            })
            .unwrap(),
            json!({"result":{"answer":"ok"}})
        );
    }

    #[test]
    fn accepts_finite_values_and_rejects_unchecked_schemas() {
        assert!(tool_schema::<OptionalReply>().is_ok());
        assert!(tool_schema::<UnionReply>().is_ok());
        assert!(matches!(
            tool_schema::<ReferencedReply>(),
            Err(FinalizeError::UnsupportedSchema { .. })
        ));
        assert!(tool_schema::<u32>().is_ok());
        assert!(matches!(
            tool_schema::<FormattedReply>(),
            Err(FinalizeError::UnsupportedSchema { .. })
        ));
        let hash_set_schema = serde_json::to_value(schemars::schema_for!(StringSet)).unwrap();
        assert_eq!(hash_set_schema["uniqueItems"], true);
        assert!(matches!(
            tool_schema::<HashSet<String>>(),
            Err(FinalizeError::UnsupportedSchema { .. })
        ));
        assert!(normalize_schema(json!({"type":"null"}), "$").is_ok());
    }

    #[test]
    fn parses_completed_finalize_from_json_arguments() {
        let mut parser = FinalizeParser::new();
        let parsed: Reply = parser
            .parse_completed(&call("finalize", json!("{\"result\":{\"answer\":\"42\"}}")))
            .unwrap();
        assert_eq!(
            parsed,
            Reply {
                answer: "42".into()
            }
        );
    }

    #[test]
    fn rejects_wrong_name_malformed_unknown_missing_and_second_finalize() {
        let mut parser = FinalizeParser::new();
        assert_eq!(
            parser.parse_completed::<Reply>(&call("other", json!({"result":{"answer":"42"}}))),
            Err(FinalizeError::WrongTool)
        );
        assert!(matches!(
            parser.parse_completed::<Reply>(&call("finalize", json!("{"))),
            Err(FinalizeError::MalformedArguments(_))
        ));
        assert_eq!(
            parser.parse_completed::<Reply>(&call(
                "finalize",
                json!({"result":{"answer":"42"},"extra":true})
            )),
            Err(FinalizeError::InvalidEnvelope)
        );
        assert_eq!(
            parser.parse_completed::<Reply>(&call("finalize", json!({}))),
            Err(FinalizeError::InvalidEnvelope)
        );

        let valid = call("finalize", json!({"result":{"answer":"42"}}));
        assert_eq!(
            parser.parse_completed::<Reply>(&valid),
            Ok(Reply {
                answer: "42".into()
            })
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
            parser.parse_completed::<Reply>(&call("finalize", json!({"result":{"answer":7}}))),
            Err(FinalizeError::MalformedArguments(_))
        ));
        assert_eq!(
            parser.parse_completed::<Reply>(&call("finalize", json!({"result":{"answer":"7"}}))),
            Ok(Reply { answer: "7".into() })
        );
    }
    #[test]
    fn tagged_sums_optional_fields_and_numbers_preserve_haskell_codec() {
        let schema=tool_schema_from_result_schema(json!({"type":"object","properties":{
            "maybe":{"type":"integer"},
            "choice":{"oneOf":[
                {"type":"object","properties":{"tag":{"type":"string","enum":["Count"]},"contents":{"type":"number"}},"required":["tag","contents"],"additionalProperties":false},
                {"type":"object","properties":{"tag":{"type":"string","enum":["Empty"]}},"required":["tag"],"additionalProperties":false}
            ]}
        },"required":["choice"],"additionalProperties":false})).unwrap();
        let result = &schema["parameters"]["properties"]["result"];
        assert!(result["properties"]["choice"]["anyOf"].is_array());
        for value in [
            json!({"maybe":null,"choice":{"tag":"Count","contents":2.5}}),
            json!({"maybe":7,"choice":{"tag":"Empty"}}),
        ] {
            validate_result(&value, result, "$").unwrap();
        }
        for value in [
            json!({"maybe":"bad","choice":{"tag":"Empty"}}),
            json!({"maybe":null,"choice":{"tag":"Count","contents":"bad"}}),
            json!({"maybe":null,"choice":{"tag":"unknown"}}),
        ] {
            assert!(validate_result(&value, result, "$").is_err());
        }
    }
}
