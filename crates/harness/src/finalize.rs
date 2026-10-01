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
    #[error("finalize result does not match schema at {0}")]
    ResultSchemaMismatch(String),
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

/// Convert schemars' schema to the conservative subset accepted by strict
/// Responses tools. Unknown validation keywords are rejected rather than
/// discarded, since dropping them could silently broaden the reply contract.
pub(crate) fn normalize_schema(schema: Value, path: &str) -> Result<Value, FinalizeError> {
    let object = schema
        .as_object()
        .ok_or_else(|| unsupported(path, "schema must be an object"))?;
    if let Some(variants) = object.get("oneOf") {
        if object.keys().any(|key| {
            !["oneOf", "title", "description", "$schema", "$defs"].contains(&key.as_str())
        }) {
            return Err(unsupported(path, "unsupported union keyword"));
        }
        let variants = variants
            .as_array()
            .filter(|variants| !variants.is_empty())
            .ok_or_else(|| unsupported(path, "oneOf requires nonempty variants"))?;
        let mut tags = std::collections::HashSet::new();
        let mut normalized = Vec::new();
        for (index, variant) in variants.iter().enumerate() {
            let tag = variant["properties"]["tag"]["enum"]
                .as_array()
                .filter(|values| values.len() == 1)
                .and_then(|values| values[0].as_str())
                .ok_or_else(|| unsupported(path, "oneOf requires disjoint singleton tags"))?;
            if variant["type"] != "object"
                || !tags.insert(tag)
                || !variant["required"]
                    .as_array()
                    .is_some_and(|required| required.iter().any(|field| field == "tag"))
            {
                return Err(unsupported(path, "oneOf requires distinct required tags"));
            }
            normalized.push(normalize_schema(
                variant.clone(),
                &format!("{path}.oneOf[{index}]"),
            )?);
        }
        // Distinct required singleton tags make these variants disjoint, so
        // anyOf has exactly the same acceptance set as the original oneOf.
        return Ok(json!({"anyOf": normalized}));
    }
    if let Some(variants) = object.get("anyOf") {
        if object.keys().any(|key| {
            !["anyOf", "title", "description", "$schema", "$defs"].contains(&key.as_str())
        }) {
            return Err(unsupported(path, "unsupported union keyword"));
        }
        let variants = variants
            .as_array()
            .filter(|variants| !variants.is_empty())
            .ok_or_else(|| unsupported(path, "anyOf requires nonempty variants"))?;
        return Ok(
            json!({"anyOf": variants.iter().enumerate().map(|(index, variant)|
            normalize_schema(variant.clone(), &format!("{path}.anyOf[{index}]")))
            .collect::<Result<Vec<_>, _>>()?}),
        );
    }
    if let Some(types) = object.get("type").and_then(Value::as_array) {
        let mut variants = Vec::new();
        for variant in types {
            let mut child = object.clone();
            child.insert("type".into(), variant.clone());
            variants.push(normalize_schema(Value::Object(child), path)?);
        }
        if variants.is_empty() {
            return Err(unsupported(path, "empty type union"));
        }
        return Ok(json!({"anyOf": variants}));
    }
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
        "array" => &["type", "items", "minItems", "maxItems"],
        // A pattern would require local regex validation for ReplayTransport.
        // Refuse it rather than advertise a constraint the harness can bypass.
        "string" => &["type", "minLength", "maxLength", "enum", "const"],
        "boolean" => &["type", "enum", "const"],
        "null" => &["type"],
        "integer" | "number" => &[
            "type",
            "minimum",
            "maximum",
            "exclusiveMinimum",
            "exclusiveMaximum",
            "enum",
            "const",
            "format",
        ],
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

    for key in ["minLength", "maxLength", "minItems", "maxItems"] {
        if object
            .get(key)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(unsupported(
                path,
                format!("{key} requires a nonnegative integer"),
            ));
        }
    }
    if object
        .get("enum")
        .is_some_and(|value| value.as_array().is_none_or(|values| values.is_empty()))
    {
        return Err(unsupported(path, "enum requires a nonempty array"));
    }

    let mut normalized = Map::new();
    normalized.insert("type".into(), Value::String(schema_type.into()));
    match schema_type {
        "object" => {
            let properties = object
                .get("properties")
                .and_then(Value::as_object)
                .ok_or_else(|| unsupported(path, "object requires properties object"))?;
            if object
                .get("additionalProperties")
                .is_some_and(|value| value != &Value::Bool(false))
            {
                return Err(unsupported(path, "open object properties are unsupported"));
            }
            if object.get("required").is_some_and(|value| {
                value.as_array().is_none_or(|fields| {
                    let mut names = std::collections::HashSet::new();
                    fields.iter().any(|field| {
                        field.as_str().is_none_or(|name| {
                            !properties.contains_key(name) || !names.insert(name)
                        })
                    })
                })
            }) {
                return Err(unsupported(
                    path,
                    "required must name distinct declared properties",
                ));
            }
            let mut normalized_properties = Map::new();
            for (name, property) in properties {
                let property_path = format!("{path}.properties.{name}");
                let mut normalized_property = normalize_schema(property.clone(), &property_path)?;
                if object
                    .get("required")
                    .and_then(Value::as_array)
                    .is_some_and(|required| {
                        !required.iter().any(|field| field.as_str() == Some(name))
                    })
                    && validate_result(&Value::Null, &normalized_property, &property_path).is_err()
                {
                    normalized_property = json!({"anyOf":[normalized_property,{"type":"null"}]});
                }
                normalized_properties.insert(name.clone(), normalized_property);
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
            copy_constraints(object, &mut normalized, &["minItems", "maxItems"]);
        }
        "string" => copy_constraints(
            object,
            &mut normalized,
            &["minLength", "maxLength", "enum", "const"],
        ),
        "boolean" => copy_constraints(object, &mut normalized, &["enum", "const"]),
        "integer" | "number" => copy_constraints(
            object,
            &mut normalized,
            &[
                "minimum",
                "maximum",
                "exclusiveMinimum",
                "exclusiveMaximum",
                "enum",
                "const",
            ],
        ),
        "null" => {}
        _ => unreachable!("type checked above"),
    }
    for key in ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"] {
        if let Some(bound) = normalized.get(key) {
            if !bound.is_number()
                || (schema_type == "integer"
                    && bound.as_i64().is_none()
                    && bound.as_u64().is_none())
            {
                return Err(unsupported(
                    path,
                    format!("{key} requires a supported numeric bound"),
                ));
            }
        }
    }
    if let Some(format) = object.get("format") {
        let bounds = match (schema_type, format.as_str()) {
            ("integer", Some("int8")) => (json!(i8::MIN), json!(i8::MAX)),
            ("integer", Some("uint8")) => (json!(0), json!(u8::MAX)),
            ("integer", Some("int16")) => (json!(i16::MIN), json!(i16::MAX)),
            ("integer", Some("uint16")) => (json!(0), json!(u16::MAX)),
            ("integer", Some("int32")) => (json!(i32::MIN), json!(i32::MAX)),
            ("integer", Some("uint32")) => (json!(0), json!(u32::MAX)),
            ("integer", Some("int64")) => (json!(i64::MIN), json!(i64::MAX)),
            ("integer", Some("uint64")) => (json!(0), json!(u64::MAX)),
            ("number", Some("float")) => (json!(f32::MIN), json!(f32::MAX)),
            ("number", Some("double")) => (json!(f64::MIN), json!(f64::MAX)),
            _ => return Err(unsupported(path, "unsupported numeric format")),
        };
        for (key, bound, lower) in [("minimum", bounds.0, true), ("maximum", bounds.1, false)] {
            let tighter = normalized
                .get(key)
                .and_then(|existing| number_cmp(existing, &bound))
                .is_some_and(|order| if lower { order.is_gt() } else { order.is_lt() });
            if !tighter {
                normalized.insert(key.into(), bound);
            }
        }
    }
    for key in ["minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum"] {
        if normalized.get(key).is_some_and(|value| !value.is_number()) {
            return Err(unsupported(path, format!("{key} requires a number")));
        }
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

fn number_cmp(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    let integer = |value: &Value| {
        value
            .as_i64()
            .map(i128::from)
            .or_else(|| value.as_u64().map(i128::from))
    };
    let integer_float = |integer: i128, float: f64| {
        if float < i128::MIN as f64 {
            return std::cmp::Ordering::Greater;
        }
        if float >= i128::MAX as f64 {
            return std::cmp::Ordering::Less;
        }
        integer.cmp(&(float as i128)).then_with(|| {
            // Compare the fractional remainder only after the whole parts
            // match; converting the integer to f64 would lose boundary bits.
            0.0_f64.partial_cmp(&float.fract()).unwrap()
        })
    };
    match (integer(left), integer(right)) {
        (Some(left), Some(right)) => Some(left.cmp(&right)),
        (Some(left), None) => Some(integer_float(left, right.as_f64()?)),
        (None, Some(right)) => Some(integer_float(right, left.as_f64()?).reverse()),
        (None, None) => left.as_f64()?.partial_cmp(&right.as_f64()?),
    }
}

pub(crate) fn validate_result(
    value: &Value,
    schema: &Value,
    path: &str,
) -> Result<(), FinalizeError> {
    if let Some(variants) = schema["oneOf"].as_array() {
        return if variants
            .iter()
            .filter(|variant| validate_result(value, variant, path).is_ok())
            .count()
            == 1
        {
            Ok(())
        } else {
            Err(FinalizeError::ResultSchemaMismatch(path.into()))
        };
    }
    if let Some(variants) = schema["anyOf"].as_array() {
        return if variants
            .iter()
            .any(|variant| validate_result(value, variant, path).is_ok())
        {
            Ok(())
        } else {
            Err(FinalizeError::ResultSchemaMismatch(path.into()))
        };
    }
    let matches_type = match schema["type"].as_str() {
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("string") => value.is_string(),
        Some("boolean") => value.is_boolean(),
        Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
        Some("number") => value.is_number(),
        Some("null") => value.is_null(),
        _ => false,
    };
    if !matches_type
        || schema
            .get("const")
            .is_some_and(|expected| expected != value)
        || schema["enum"]
            .as_array()
            .is_some_and(|choices| !choices.contains(value))
    {
        return Err(FinalizeError::ResultSchemaMismatch(path.into()));
    }
    if value.is_number() {
        for (key, inclusive) in [
            ("minimum", true),
            ("maximum", true),
            ("exclusiveMinimum", false),
            ("exclusiveMaximum", false),
        ] {
            if let Some(bound) = schema.get(key) {
                let order = number_cmp(value, bound)
                    .ok_or_else(|| FinalizeError::ResultSchemaMismatch(path.into()))?;
                let lower = key == "minimum" || key == "exclusiveMinimum";
                let passes = if lower {
                    order.is_gt() || (inclusive && order.is_eq())
                } else {
                    order.is_lt() || (inclusive && order.is_eq())
                };
                if !passes {
                    return Err(FinalizeError::ResultSchemaMismatch(path.into()));
                }
            }
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema["properties"]
            .as_object()
            .ok_or_else(|| FinalizeError::ResultSchemaMismatch(path.into()))?;
        if object.len() != properties.len() {
            return Err(FinalizeError::ResultSchemaMismatch(path.into()));
        }
        for (key, property_schema) in properties {
            let child = object
                .get(key)
                .ok_or_else(|| FinalizeError::ResultSchemaMismatch(format!("{path}.{key}")))?;
            validate_result(child, property_schema, &format!("{path}.{key}"))?;
        }
    } else if let Some(array) = value.as_array() {
        let min = schema["minItems"].as_u64().unwrap_or(0) as usize;
        let max = schema["maxItems"].as_u64().unwrap_or(u64::MAX) as usize;
        if array.len() < min || array.len() > max {
            return Err(FinalizeError::ResultSchemaMismatch(path.into()));
        }
        for (index, child) in array.iter().enumerate() {
            validate_result(child, &schema["items"], &format!("{path}[{index}]"))?;
        }
    } else if let Some(text) = value.as_str() {
        let length = text.chars().count();
        let min = schema["minLength"].as_u64().unwrap_or(0) as usize;
        let max = schema["maxLength"].as_u64().unwrap_or(u64::MAX) as usize;
        if length < min || length > max {
            return Err(FinalizeError::ResultSchemaMismatch(path.into()));
        }
    }
    Ok(())
}

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
    fn normalizes_nested_objects_and_arrays_for_strict_mode() {
        let result = normalize_schema(
            json!({
                "type":"object",
                "title":"Reply",
                "properties":{
                    "nested":{
                        "type":"object",
                        "properties":{"count":{"type":"string"}},
                        "required":["count"],
                        "additionalProperties":false
                    },
                    "entries":{"type":"array","items":{"type":"string"}}
                },
                "required":["nested"],
                "additionalProperties":false
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
        assert_eq!(
            result["properties"]["entries"]["anyOf"][0]["items"]["type"],
            "string"
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
    fn rejects_open_objects_and_malformed_validation_constraints() {
        for schema in [
            json!({"type":"object","properties":{},"additionalProperties":true}),
            json!({"type":"object","properties":{},"additionalProperties":{"type":"string"}}),
            json!({"type":"object","properties":{},"required":["missing"]}),
            json!({"type":"object","properties":{},"required":false}),
            json!({"type":"string","minLength":"3"}),
            json!({"type":"array","items":{"type":"string"},"maxItems":-1}),
            json!({"type":"string","enum":false}),
            json!({"type":"string","enum":[]}),
        ] {
            assert!(matches!(
                normalize_schema(schema, "$"),
                Err(FinalizeError::UnsupportedSchema { .. })
            ));
        }
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
    #[test]
    fn overlapping_one_of_is_rejected_and_direct_validation_requires_one_match() {
        assert!(
            tool_schema_from_result_schema(json!({"oneOf":[{"type":"number"},{"type":"integer"}]}))
                .is_err()
        );
        assert!(
            validate_result(
                &json!(2),
                &json!({"oneOf":[{"type":"number"},{"type":"integer"}]}),
                "$"
            )
            .is_err()
        );
        assert!(
            validate_result(
                &json!(2.5),
                &json!({"oneOf":[{"type":"number"},{"type":"integer"}]}),
                "$"
            )
            .is_ok()
        );
    }
    #[test]
    fn integer_bounds_remain_exact_above_float_precision_and_across_signs() {
        let minimum = json!({"type":"integer","minimum":9007199254740993_u64});
        assert!(validate_result(&json!(9007199254740992_u64), &minimum, "$").is_err());
        assert!(validate_result(&json!(9007199254740993_u64), &minimum, "$").is_ok());
        let number_minimum = json!({"type":"number","minimum":9007199254740993_u64});
        assert!(validate_result(&json!(9007199254740992.0_f64), &number_minimum, "$").is_err());
        let float_maximum = json!({"type":"number","maximum":9007199254740992.0_f64});
        assert!(validate_result(&json!(9007199254740993_u64), &float_maximum, "$").is_err());
        assert!(number_cmp(&json!(-2), &json!(-2.5)).unwrap().is_gt());
        assert!(number_cmp(&json!(2), &json!(2.5)).unwrap().is_lt());
        let exclusive = json!({"type":"integer","exclusiveMaximum":u64::MAX});
        assert!(validate_result(&json!(u64::MAX), &exclusive, "$").is_err());
        assert!(validate_result(&json!(u64::MAX - 1), &exclusive, "$").is_ok());
        let nonnegative = json!({"type":"integer","minimum":0_u64});
        assert!(validate_result(&json!(-1), &nonnegative, "$").is_err());
        assert!(
            validate_result(
                &json!(i64::MIN),
                &json!({"type":"integer","minimum":i64::MIN}),
                "$"
            )
            .is_ok()
        );
        assert!(normalize_schema(json!({"type":"integer","minimum":2.5}), "$").is_err());
        let schema = tool_schema::<u32>().unwrap();
        let integer = &schema["parameters"]["properties"]["result"];
        assert!(validate_result(&json!(u32::MAX), integer, "$").is_ok());
        assert!(validate_result(&json!(u32::MAX as u64 + 1), integer, "$").is_err());
        assert!(tool_schema::<f64>().is_ok());
    }
}
