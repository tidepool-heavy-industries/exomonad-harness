//! Strict function-tool schema admission, host argument projection, and typed
//! `finalize` support for adapter replies.
//!
//! The finalize envelope is always exactly `{"result": T}`; it does not provide
//! a separate answer/output channel.

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
    #[error("function arguments have ambiguous optional-null semantics at {0}")]
    AmbiguousArguments(String),
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

/// A strict provider schema and the decoder for its host argument contract.
/// Only nulls introduced to encode omitted fields are removed on dispatch.
#[derive(Clone, Debug)]
pub struct FunctionToolSchema {
    parameters: Value,
    projection: ArgumentProjection,
}

impl FunctionToolSchema {
    pub fn new(schema: Value) -> Result<Self, FinalizeError> {
        if schema["type"] != "object" {
            return Err(unsupported("$", "function parameters must be an object"));
        }
        let parameters = normalize_schema(schema.clone(), "$")?;
        let projection = ArgumentProjection::new(&schema)?;
        Ok(Self {
            parameters,
            projection,
        })
    }

    pub fn parameters(&self) -> &Value {
        &self.parameters
    }

    pub fn decode_arguments(&self, mut arguments: Value) -> Result<Value, FinalizeError> {
        self.projection.decode(&mut arguments, "$")?;
        Ok(arguments)
    }
}

#[derive(Clone, Debug)]
enum ArgumentProjection {
    Identity,
    Object(Vec<(String, bool, ArgumentProjection)>),
    Array(Box<ArgumentProjection>),
    Alternatives(Vec<(Value, ArgumentProjection)>),
}

impl ArgumentProjection {
    fn new(schema: &Value) -> Result<Self, FinalizeError> {
        if let Some(variants) = schema["anyOf"]
            .as_array()
            .or_else(|| schema["oneOf"].as_array())
        {
            return Ok(Self::Alternatives(
                variants
                    .iter()
                    .map(|variant| {
                        Ok((normalize_schema(variant.clone(), "$")?, Self::new(variant)?))
                    })
                    .collect::<Result<_, FinalizeError>>()?,
            ));
        }
        if let Some(types) = schema["type"].as_array() {
            let mut variants = Vec::new();
            for kind in types {
                let mut variant = schema.clone();
                variant["type"] = kind.clone();
                variants.push((
                    normalize_schema(variant.clone(), "$")?,
                    Self::new(&variant)?,
                ));
            }
            return Ok(Self::Alternatives(variants));
        }
        match schema["type"].as_str() {
            Some("object") => {
                let mut fields = Vec::new();
                if let Some(properties) = schema["properties"].as_object() {
                    for (name, property) in properties {
                        let required = schema["required"].as_array().is_some_and(|names| {
                            names.iter().any(|value| value.as_str() == Some(name))
                        });
                        let nullable = validate_result(
                            &Value::Null,
                            &normalize_schema(property.clone(), "$")?,
                            "$",
                        )
                        .is_ok();
                        fields.push((name.clone(), !required && !nullable, Self::new(property)?));
                    }
                }
                Ok(Self::Object(fields))
            }
            Some("array") => Ok(Self::Array(Box::new(Self::new(&schema["items"])?))),
            _ => Ok(Self::Identity),
        }
    }

    fn decode(&self, value: &mut Value, path: &str) -> Result<(), FinalizeError> {
        match self {
            Self::Identity => {}
            Self::Object(fields) => {
                if let Some(object) = value.as_object_mut() {
                    for (name, omit_null, projection) in fields {
                        if *omit_null && object.get(name).is_some_and(Value::is_null) {
                            object.remove(name);
                        } else if let Some(value) = object.get_mut(name) {
                            projection.decode(value, &format!("{path}.{name}"))?;
                        }
                    }
                }
            }
            Self::Array(projection) => {
                if let Some(values) = value.as_array_mut() {
                    for (index, value) in values.iter_mut().enumerate() {
                        projection.decode(value, &format!("{path}[{index}]"))?;
                    }
                }
            }
            Self::Alternatives(variants) => {
                let mut decoded = None;
                for (_, projection) in variants
                    .iter()
                    .filter(|(schema, _)| validate_result(value, schema, path).is_ok())
                {
                    let mut candidate = value.clone();
                    projection.decode(&mut candidate, path)?;
                    if decoded
                        .as_ref()
                        .is_some_and(|previous| previous != &candidate)
                    {
                        return Err(FinalizeError::AmbiguousArguments(path.into()));
                    }
                    decoded = Some(candidate);
                }
                if let Some(decoded) = decoded {
                    *value = decoded;
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SchemaMode {
    HostProjection,
    StrictWire,
}

/// Convert schemars' schema to the conservative subset accepted by strict
/// Responses tools. Unknown validation keywords are rejected rather than
/// discarded, since dropping them could silently broaden the reply contract.
pub(crate) fn normalize_schema(schema: Value, path: &str) -> Result<Value, FinalizeError> {
    normalize_schema_mode(schema, path, SchemaMode::HostProjection)
}

/// Admit provider parameters without changing their advertised bytes. Both
/// normalization and admission share the same supported-schema traversal.
pub(crate) fn validate_function_parameters(schema: &Value) -> Result<(), FinalizeError> {
    if schema["type"] != "object" {
        return Err(unsupported("$", "function parameters must be an object"));
    }
    normalize_schema_mode(schema.clone(), "$", SchemaMode::StrictWire).map(|_| ())
}

fn normalize_schema_mode(
    schema: Value,
    path: &str,
    mode: SchemaMode,
) -> Result<Value, FinalizeError> {
    let object = schema
        .as_object()
        .ok_or_else(|| unsupported(path, "schema must be an object"))?;
    if mode == SchemaMode::StrictWire {
        for key in ["$defs", "definitions"] {
            if object.contains_key(key) {
                return Err(unsupported(path, format!("{key} requires host projection")));
            }
        }
        for key in ["title", "description", "$schema"] {
            if object.get(key).is_some_and(|value| !value.is_string()) {
                return Err(unsupported(path, format!("{key} requires a string")));
            }
        }
        for key in ["deprecated", "readOnly", "writeOnly"] {
            if object.get(key).is_some_and(|value| !value.is_boolean()) {
                return Err(unsupported(path, format!("{key} requires a boolean")));
            }
        }
        if object
            .get("examples")
            .is_some_and(|value| !value.is_array())
        {
            return Err(unsupported(path, "examples requires an array"));
        }
        if object.contains_key("format")
            && matches!(
                object.get("type").and_then(Value::as_str),
                Some("integer" | "number")
            )
        {
            return Err(unsupported(path, "numeric format requires host projection"));
        }
    }
    if let Some(variants) = object.get("oneOf") {
        if mode == SchemaMode::StrictWire {
            return Err(unsupported(
                path,
                "strict tools require anyOf rather than oneOf",
            ));
        }
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
            normalized.push(normalize_schema_mode(
                variant.clone(),
                &format!("{path}.oneOf[{index}]"),
                mode,
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
            normalize_schema_mode(variant.clone(), &format!("{path}.anyOf[{index}]"), mode))
            .collect::<Result<Vec<_>, _>>()?}),
        );
    }
    if let Some(types) = object.get("type").and_then(Value::as_array) {
        let mut kinds = std::collections::HashSet::new();
        if types
            .iter()
            .any(|kind| kind.as_str().is_none_or(|kind| !kinds.insert(kind)))
        {
            return Err(unsupported(
                path,
                "type union requires distinct string kinds",
            ));
        }
        let mut variants = Vec::new();
        for variant in types {
            let mut child = object.clone();
            child.insert("type".into(), variant.clone());
            variants.push(normalize_schema_mode(Value::Object(child), path, mode)?);
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
            if mode == SchemaMode::StrictWire {
                if object.get("additionalProperties") != Some(&Value::Bool(false)) {
                    return Err(unsupported(
                        path,
                        "strict objects require additionalProperties:false",
                    ));
                }
                if object
                    .get("required")
                    .and_then(Value::as_array)
                    .is_none_or(|required| required.len() != properties.len())
                {
                    return Err(unsupported(
                        path,
                        "strict objects must require every property",
                    ));
                }
            }
            let mut normalized_properties = Map::new();
            for (name, property) in properties {
                let property_path = format!("{path}.properties.{name}");
                let mut normalized_property =
                    normalize_schema_mode(property.clone(), &property_path, mode)?;
                if !object
                    .get("required")
                    .and_then(Value::as_array)
                    .is_some_and(|required| {
                        required.iter().any(|field| field.as_str() == Some(name))
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
                normalize_schema_mode(items.clone(), &format!("{path}.items"), mode)?,
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
    fn strict_wire_refuses_unprojected_containers_formats_and_invalid_metadata() {
        let root =
            json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
        for (key, value) in [
            (
                "$defs",
                json!({"bad":{"type":"object","properties":{"value":{"type":"string"}}}}),
            ),
            ("$defs", json!(false)),
            ("definitions", json!({})),
            ("description", json!(77)),
            ("title", json!(false)),
            ("$schema", json!(false)),
            ("examples", json!(false)),
            ("readOnly", json!("yes")),
            ("writeOnly", json!(0)),
            ("deprecated", json!([])),
        ] {
            let mut invalid = root.clone();
            invalid[key] = value;
            assert!(
                validate_function_parameters(&invalid).is_err(),
                "invalid metadata {key} admitted"
            );
        }
        for value in [
            json!({"type":"integer","format":"int32"}),
            json!({"type":["string","string"]}),
            json!({"anyOf":[{"type":"string"},{"type":"null"}],"$defs":false}),
            json!({"anyOf":[{"type":"string"},{"type":"null"}],"description":false}),
        ] {
            let mut invalid = root.clone();
            invalid["properties"] = json!({"value":value});
            invalid["required"] = json!(["value"]);
            assert!(validate_function_parameters(&invalid).is_err());
        }
        let normalized = FunctionToolSchema::new(json!({"type":"object","properties":{"value":{"type":"integer","format":"int32"}},"required":["value"]})).unwrap();
        validate_function_parameters(normalized.parameters()).unwrap();
        assert_eq!(
            normalized.parameters()["properties"]["value"]["minimum"],
            i32::MIN
        );
    }

    #[test]
    fn function_schema_encodes_omission_and_preserves_nullable_required_fields() {
        let schema = FunctionToolSchema::new(json!({
            "type":"object", "properties":{
                "view":{"type":"string","enum":["changed","summary"]},
                "also_check":{"type":"array","items":{"type":"string"}},
                "nullable":{"type":["string","null"]},
                "required_nullable":{"type":["string","null"]},
                "required_string":{"type":"string"}
            }, "required":["required_nullable","required_string"], "additionalProperties":false
        }))
        .unwrap();
        validate_function_parameters(schema.parameters()).unwrap();
        assert_eq!(schema.parameters()["required"].as_array().unwrap().len(), 5);
        assert_eq!(
            schema
                .decode_arguments(json!({
                    "view":null,"also_check":null,"nullable":null,
                    "required_nullable":null,"required_string":"present"
                }))
                .unwrap(),
            json!({"nullable":null,"required_nullable":null,"required_string":"present"})
        );
        // The original host remains responsible for refusing an invalid required
        // value; the decoder must never silently omit it.
        assert_eq!(
            schema
                .decode_arguments(json!({"required_string":null}))
                .unwrap(),
            json!({"required_string":null})
        );
        let absent_required = FunctionToolSchema::new(json!({
            "type":"object","properties":{"view":{"type":"string"}}
        }))
        .unwrap();
        assert_eq!(
            absent_required.parameters()["properties"]["view"],
            json!({"anyOf":[{"type":"string"},{"type":"null"}]})
        );
        assert_eq!(
            absent_required
                .decode_arguments(json!({"view":null}))
                .unwrap(),
            json!({})
        );
    }

    #[test]
    fn function_schema_projects_nested_arrays_and_disjoint_tagged_unions() {
        let schema = FunctionToolSchema::new(json!({
            "type":"object","properties":{
                "entries":{"type":"array","items":{"type":"object","properties":{
                    "optional":{"type":"string"},"required_nullable":{"type":["integer","null"]}
                },"required":["required_nullable"]}},
                "choice":{"oneOf":[
                    {"type":"object","properties":{"tag":{"type":"string","enum":["omit"]},"value":{"type":"string"}},"required":["tag"]},
                    {"type":"object","properties":{"tag":{"type":"string","enum":["keep"]},"value":{"type":["string","null"]}},"required":["tag","value"]}
                ]}
            },"required":["entries","choice"]
        })).unwrap();
        validate_function_parameters(schema.parameters()).unwrap();
        let entries = json!([{"optional":null,"required_nullable":null},{"optional":"yes","required_nullable":1}]);
        let expected = json!([{"required_nullable":null},{"optional":"yes","required_nullable":1}]);
        assert_eq!(
            schema
                .decode_arguments(json!({"entries":entries,"choice":{"tag":"omit","value":null}}))
                .unwrap(),
            json!({"entries":expected,"choice":{"tag":"omit"}})
        );
        assert_eq!(
            schema
                .decode_arguments(json!({"entries":[],"choice":{"tag":"keep","value":null}}))
                .unwrap(),
            json!({"entries":[],"choice":{"tag":"keep","value":null}})
        );
    }

    #[test]
    fn function_schema_refuses_ambiguous_optional_null_projection() {
        let schema = FunctionToolSchema::new(json!({
            "type":"object","properties":{"choice":{"anyOf":[
                {"type":"object","properties":{"value":{"type":"string"}},"required":[]},
                {"type":"object","properties":{"value":{"type":["string","null"]}},"required":["value"]}
            ]}},"required":["choice"]
        })).unwrap();
        assert!(
            matches!(schema.decode_arguments(json!({"choice":{"value":null}})), Err(FinalizeError::AmbiguousArguments(path)) if path == "$.choice")
        );
        assert_eq!(
            schema
                .decode_arguments(json!({"choice":{"value":"present"}}))
                .unwrap(),
            json!({"choice":{"value":"present"}})
        );
        for invalid in [
            json!({"type":"string"}),
            json!({"type":"object","properties":{"value":{"type":"string","pattern":".*"}}}),
        ] {
            assert!(FunctionToolSchema::new(invalid).is_err());
        }
    }

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
