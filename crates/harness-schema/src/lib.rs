//! Strict function-tool schema admission, host argument projection and value validation.

use serde_json::{Map, Value, json};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SchemaError {
    #[error("unsupported JSON schema at {path}: {keyword}")]
    UnsupportedSchema { path: String, keyword: String },
    #[error("JSON schema constraints exclude every type variant at {path}")]
    UnsatisfiableSchema { path: String },
    #[error("function arguments have ambiguous optional-null semantics at {0}")]
    AmbiguousArguments(String),
    #[error("JSON value does not match schema at {0}")]
    ValueSchemaMismatch(String),
}

/// A strict provider schema and the decoder for its host argument contract.
/// Only nulls introduced to encode omitted fields are removed on dispatch.
#[derive(Clone, Debug)]
pub struct FunctionToolSchema {
    parameters: Value,
    projection: ArgumentProjection,
}

impl FunctionToolSchema {
    pub fn new(schema: Value) -> Result<Self, SchemaError> {
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

    pub fn decode_arguments(&self, mut arguments: Value) -> Result<Value, SchemaError> {
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
    fn new(schema: &Value) -> Result<Self, SchemaError> {
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
                    .collect::<Result<_, SchemaError>>()?,
            ));
        }
        if let Some(types) = schema["type"].as_array() {
            let mut variants = Vec::new();
            for variant in type_union_variants(schema.as_object().unwrap(), types, "$")? {
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
                        let nullable = validate_value(
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

    fn decode(&self, value: &mut Value, path: &str) -> Result<(), SchemaError> {
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
                    .filter(|(schema, _)| validate_value(value, schema, path).is_ok())
                {
                    let mut candidate = value.clone();
                    projection.decode(&mut candidate, path)?;
                    if decoded
                        .as_ref()
                        .is_some_and(|previous| previous != &candidate)
                    {
                        return Err(SchemaError::AmbiguousArguments(path.into()));
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

// Document metadata does not constrain the admitted value set.
const SCHEMA_METADATA: &[&str] = &[
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

fn schema_keywords(kind: &str, path: &str) -> Result<&'static [&'static str], SchemaError> {
    Ok(match kind {
        "object" => &["type", "properties", "required", "additionalProperties"],
        "array" => &["type", "items", "minItems", "maxItems"],
        // Admit only constraints that the local value validator enforces.
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
    })
}

/// JSON Schema's numeric, string and container keywords apply only to values
/// of that type. Enum and const constrain every branch, including null.
fn type_union_variants(
    object: &Map<String, Value>,
    types: &[Value],
    path: &str,
) -> Result<Vec<Value>, SchemaError> {
    if types.is_empty() {
        return Err(unsupported(path, "empty type union"));
    }
    let mut kinds = std::collections::HashSet::new();
    let mut supported = Vec::new();
    for kind in types {
        let kind = kind
            .as_str()
            .filter(|kind| kinds.insert(*kind))
            .ok_or_else(|| unsupported(path, "type union requires distinct string kinds"))?;
        supported.push((kind, schema_keywords(kind, path)?));
    }
    for key in object.keys() {
        if !SCHEMA_METADATA.contains(&key.as_str())
            && !supported
                .iter()
                .any(|(_, allowed)| allowed.contains(&key.as_str()))
        {
            return Err(unsupported(path, key));
        }
    }
    if object
        .get("enum")
        .is_some_and(|value| value.as_array().is_none_or(|values| values.is_empty()))
    {
        return Err(unsupported(path, "enum requires a nonempty array"));
    }
    let mut variants = Vec::new();
    for (kind, allowed) in supported {
        let mut child = object.clone();
        child.retain(|key, _| {
            SCHEMA_METADATA.contains(&key.as_str())
                || allowed.contains(&key.as_str())
                || matches!(key.as_str(), "enum" | "const")
        });
        child.insert("type".into(), json!(kind));
        if kind == "null" {
            if child.get("const").is_some_and(|value| !value.is_null())
                || child
                    .get("enum")
                    .and_then(Value::as_array)
                    .is_some_and(|values| !values.contains(&Value::Null))
            {
                continue;
            }
            // Once null is admitted these global constraints are redundant;
            // null enum/const are outside the provider keyword subset.
            child.remove("enum");
            child.remove("const");
        }
        variants.push(Value::Object(child));
    }
    if variants.is_empty() {
        return Err(SchemaError::UnsatisfiableSchema { path: path.into() });
    }
    Ok(variants)
}

/// Convert schemars' schema to the conservative subset accepted by strict
/// Responses tools. Unknown validation keywords are rejected rather than
/// discarded, since dropping them could silently broaden the reply contract.
pub fn normalize_schema(schema: Value, path: &str) -> Result<Value, SchemaError> {
    normalize_schema_mode(schema, path, SchemaMode::HostProjection)
}

/// Admit provider parameters without changing their advertised bytes. Both
/// normalization and admission share the same supported-schema traversal.
pub fn validate_function_parameters(schema: &Value) -> Result<(), SchemaError> {
    if schema["type"] != "object" {
        return Err(unsupported("$", "function parameters must be an object"));
    }
    normalize_schema_mode(schema.clone(), "$", SchemaMode::StrictWire).map(|_| ())
}

fn normalize_schema_mode(
    schema: Value,
    path: &str,
    mode: SchemaMode,
) -> Result<Value, SchemaError> {
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
        let mut variants = Vec::new();
        for child in type_union_variants(object, types, path)? {
            // Validate each branch before pruning, so impossible constants
            // cannot conceal malformed or unsupported constraints.
            let mut normalized = normalize_schema_mode(child, path, mode)?;
            if normalized
                .get("const")
                .is_some_and(|value| validate_value(value, &normalized, path).is_err())
            {
                continue;
            }
            if let Some(choices) = normalized["enum"].as_array() {
                let viable = choices
                    .iter()
                    .filter(|value| validate_value(value, &normalized, path).is_ok())
                    .cloned()
                    .collect::<Vec<_>>();
                if viable.is_empty() {
                    continue;
                }
                normalized["enum"] = json!(viable);
            }
            variants.push(normalized);
        }
        if variants.is_empty() {
            return Err(SchemaError::UnsatisfiableSchema { path: path.into() });
        }
        return Ok(json!({"anyOf":variants}));
    }
    let schema_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| unsupported(path, "missing or non-string type"))?;

    let allowed = schema_keywords(schema_type, path)?;
    for key in object.keys() {
        if SCHEMA_METADATA.contains(&key.as_str()) {
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
                    && validate_value(&Value::Null, &normalized_property, &property_path).is_err()
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

fn unsupported(path: &str, keyword: impl std::fmt::Display) -> SchemaError {
    SchemaError::UnsupportedSchema {
        path: path.into(),
        keyword: keyword.to_string(),
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

pub fn validate_value(value: &Value, schema: &Value, path: &str) -> Result<(), SchemaError> {
    if let Some(variants) = schema["oneOf"].as_array() {
        return if variants
            .iter()
            .filter(|variant| validate_value(value, variant, path).is_ok())
            .count()
            == 1
        {
            Ok(())
        } else {
            Err(SchemaError::ValueSchemaMismatch(path.into()))
        };
    }
    if let Some(variants) = schema["anyOf"].as_array() {
        return if variants
            .iter()
            .any(|variant| validate_value(value, variant, path).is_ok())
        {
            Ok(())
        } else {
            Err(SchemaError::ValueSchemaMismatch(path.into()))
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
        return Err(SchemaError::ValueSchemaMismatch(path.into()));
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
                    .ok_or_else(|| SchemaError::ValueSchemaMismatch(path.into()))?;
                let lower = key == "minimum" || key == "exclusiveMinimum";
                let passes = if lower {
                    order.is_gt() || (inclusive && order.is_eq())
                } else {
                    order.is_lt() || (inclusive && order.is_eq())
                };
                if !passes {
                    return Err(SchemaError::ValueSchemaMismatch(path.into()));
                }
            }
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema["properties"]
            .as_object()
            .ok_or_else(|| SchemaError::ValueSchemaMismatch(path.into()))?;
        if object.len() != properties.len() {
            return Err(SchemaError::ValueSchemaMismatch(path.into()));
        }
        for (key, property_schema) in properties {
            let child = object
                .get(key)
                .ok_or_else(|| SchemaError::ValueSchemaMismatch(format!("{path}.{key}")))?;
            validate_value(child, property_schema, &format!("{path}.{key}"))?;
        }
    } else if let Some(array) = value.as_array() {
        let min = schema["minItems"].as_u64().unwrap_or(0) as usize;
        let max = schema["maxItems"].as_u64().unwrap_or(u64::MAX) as usize;
        if array.len() < min || array.len() > max {
            return Err(SchemaError::ValueSchemaMismatch(path.into()));
        }
        for (index, child) in array.iter().enumerate() {
            validate_value(child, &schema["items"], &format!("{path}[{index}]"))?;
        }
    } else if let Some(text) = value.as_str() {
        let length = text.chars().count();
        let min = schema["minLength"].as_u64().unwrap_or(0) as usize;
        let max = schema["maxLength"].as_u64().unwrap_or(u64::MAX) as usize;
        if length < min || length > max {
            return Err(SchemaError::ValueSchemaMismatch(path.into()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
