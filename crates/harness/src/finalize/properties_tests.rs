//! The generated AST models host schemas independently of the schema owner.
//! A separate JSON Schema evaluator checks the actual serialized request, so
//! construction and transport admission cannot validate each other's mistake.

use super::{FunctionToolSchema, normalize_schema};
use crate::model::Effort;
use crate::transport::{ResponsesRequest, client::request_body};
use proptest::prelude::*;
use proptest::test_runner::{
    Config, FileFailurePersistence, TestCaseResult, TestRunner, contextualize_config,
};
use serde_json::{Map, Value, json};
use std::cell::{Cell, RefCell};

#[derive(Clone, Debug)]
enum Shape {
    Integer(i64),
    Text(u8),
    Boolean,
    Array(Box<Shape>),
    Object(Box<Shape>, Box<Shape>),
    Nullable(Box<Shape>, bool),
    Tagged(Box<Shape>, Box<Shape>, bool),
}

impl Shape {
    fn schema(&self) -> Value {
        match self {
            Self::Integer(minimum) => {
                json!({"type":"integer","minimum":minimum,"maximum":minimum + 3})
            }
            Self::Text(minimum) => {
                json!({"type":"string","minLength":minimum,"maxLength":minimum + 3})
            }
            Self::Boolean => json!({"type":"boolean"}),
            Self::Array(item) => {
                json!({"type":"array","items":item.schema(),"minItems":0,"maxItems":2})
            }
            Self::Object(required, optional) => json!({"type":"object","properties":{
                "required":required.schema(),"optional":optional.schema()
            },"required":["required"],"additionalProperties":false}),
            Self::Nullable(inner, type_array) => {
                let mut schema = inner.schema();
                if *type_array && schema["type"].is_string() {
                    schema["type"] = json!([schema["type"], "null"]);
                    schema
                } else {
                    json!({"anyOf":[schema,{"type":"null"}]})
                }
            }
            Self::Tagged(left, right, one_of) => {
                let branches = [left, right]
                    .into_iter()
                    .enumerate()
                    .map(|(index, child)| {
                        json!({"type":"object","properties":{
                        "tag":{"type":"string","enum":[index.to_string()]},"contents":child.schema()
                    },"required":["tag","contents"],"additionalProperties":false})
                    })
                    .collect::<Vec<_>>();
                json!({if *one_of {"oneOf"} else {"anyOf"}:branches})
            }
        }
    }

    fn nullable(&self) -> bool {
        matches!(self, Self::Nullable(..))
    }

    /// Construct a valid strict value directly from the host model. Optional
    /// omissions have null carriers; nullable host values keep explicit nulls.
    fn wire_value(&self, seed: u8) -> Value {
        match self {
            Self::Integer(minimum) => json!(minimum + i64::from(seed % 4)),
            Self::Text(minimum) => json!("x".repeat(usize::from(minimum + seed % 4))),
            Self::Boolean => json!(seed % 2 == 0),
            Self::Array(item) => Value::Array(
                (0..seed % 3)
                    .map(|index| item.wire_value(seed.wrapping_add(index + 1)))
                    .collect(),
            ),
            Self::Object(required, optional) => json!({
                "required":required.wire_value(seed.wrapping_add(1)),
                "optional":if seed % 2 == 0 {Value::Null} else {optional.wire_value(seed.wrapping_add(2))}
            }),
            Self::Nullable(inner, _) => {
                if seed % 2 == 0 {
                    Value::Null
                } else {
                    inner.wire_value(seed.wrapping_add(1))
                }
            }
            Self::Tagged(left, right, _) => {
                let index = seed % 2;
                json!({"tag":index.to_string(),"contents":if index == 0 {left} else {right}.wire_value(seed.wrapping_add(1))})
            }
        }
    }

    /// Host projection derives from AST optionality, never from production's
    /// normalized schema or ArgumentProjection traversal.
    fn host_value(&self, wire: &Value) -> Value {
        match self {
            Self::Object(required, optional) => {
                let mut result = Map::new();
                result.insert("required".into(), required.host_value(&wire["required"]));
                if !wire["optional"].is_null() || optional.nullable() {
                    result.insert("optional".into(), optional.host_value(&wire["optional"]));
                }
                Value::Object(result)
            }
            Self::Array(item) => Value::Array(
                wire.as_array()
                    .unwrap()
                    .iter()
                    .map(|value| item.host_value(value))
                    .collect(),
            ),
            Self::Nullable(_, _) if wire.is_null() => Value::Null,
            Self::Nullable(inner, _) => inner.host_value(wire),
            Self::Tagged(left, right, _) => json!({"tag":wire["tag"],"contents":
                if wire["tag"] == "0" {left} else {right}.host_value(&wire["contents"])}),
            _ => wire.clone(),
        }
    }
}

fn shapes() -> impl Strategy<Value = Shape> {
    prop_oneof![
        (0_i64..4).prop_map(Shape::Integer),
        (0_u8..3).prop_map(Shape::Text),
        Just(Shape::Boolean)
    ]
    .prop_recursive(4, 48, 3, |inner| {
        prop_oneof![
            inner.clone().prop_map(|item| Shape::Array(Box::new(item))),
            (inner.clone(), inner.clone()).prop_map(|(required, optional)| Shape::Object(
                Box::new(required),
                Box::new(optional)
            )),
            (inner.clone(), any::<bool>())
                .prop_map(|(item, array)| Shape::Nullable(Box::new(item), array)),
            (inner.clone(), inner, any::<bool>()).prop_map(|(left, right, one_of)| Shape::Tagged(
                Box::new(left),
                Box::new(right),
                one_of
            )),
        ]
    })
}

fn values() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-2_i64..8).prop_map(|value| json!(value)),
        "[x-z]{0,6}".prop_map(Value::String)
    ]
    .prop_recursive(3, 24, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            (inner.clone(), inner.clone())
                .prop_map(|(required, optional)| json!({"required":required,"optional":optional})),
            (inner, any::<bool>()).prop_map(
                |(contents, first)| json!({"tag":if first {"0"} else {"1"},"contents":contents})
            ),
        ]
    })
}

/// Interpret JSON Schema semantics directly, including keywords that apply
/// only to a value's type. This code does not invoke production admission or
/// validation, and object requiredness comes from the advertised required set.
fn accepts(schema: &Value, value: &Value) -> bool {
    let kind_matches = |kind: &str| match kind {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        _ => false,
    };
    if let Some(kinds) = schema.get("type") {
        let matches = if let Some(kind) = kinds.as_str() {
            kind_matches(kind)
        } else {
            kinds
                .as_array()
                .unwrap()
                .iter()
                .any(|kind| kind_matches(kind.as_str().unwrap()))
        };
        if !matches {
            return false;
        }
    }
    if schema
        .get("const")
        .is_some_and(|expected| expected != value)
        || schema["enum"]
            .as_array()
            .is_some_and(|options| !options.contains(value))
    {
        return false;
    }
    for (keyword, exclusive) in [("anyOf", false), ("oneOf", true)] {
        if let Some(branches) = schema[keyword].as_array() {
            let count = branches
                .iter()
                .filter(|branch| accepts(branch, value))
                .count();
            if count == 0 || exclusive && count != 1 {
                return false;
            }
        }
    }
    if let Some(number) = value.as_f64() {
        for (keyword, lower, inclusive) in [
            ("minimum", true, true),
            ("maximum", false, true),
            ("exclusiveMinimum", true, false),
            ("exclusiveMaximum", false, false),
        ] {
            if let Some(bound) = schema[keyword].as_f64() {
                let passes = if lower {
                    number > bound || inclusive && number == bound
                } else {
                    number < bound || inclusive && number == bound
                };
                if !passes {
                    return false;
                }
            }
        }
    }
    if let Some(text) = value.as_str() {
        let length = text.chars().count() as u64;
        if schema["minLength"].as_u64().is_some_and(|min| length < min)
            || schema["maxLength"].as_u64().is_some_and(|max| length > max)
        {
            return false;
        }
    }
    if let Some(items) = value.as_array() {
        if schema["minItems"]
            .as_u64()
            .is_some_and(|min| (items.len() as u64) < min)
            || schema["maxItems"]
                .as_u64()
                .is_some_and(|max| (items.len() as u64) > max)
            || schema
                .get("items")
                .is_some_and(|child| items.iter().any(|value| !accepts(child, value)))
        {
            return false;
        }
    }
    if let Some(object) = value.as_object() {
        if schema["required"].as_array().is_some_and(|required| {
            required
                .iter()
                .any(|name| !object.contains_key(name.as_str().unwrap()))
        }) {
            return false;
        }
        if let Some(properties) = schema["properties"].as_object() {
            for (name, value) in object {
                match properties.get(name) {
                    Some(child) if !accepts(child, value) => return false,
                    None if schema["additionalProperties"] == false => return false,
                    _ => {}
                }
            }
        }
    }
    true
}

fn assert_closed(schema: &Value) {
    if schema["type"] == "object" {
        let properties = schema["properties"].as_object().unwrap();
        let required = schema["required"].as_array().unwrap();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(properties.len(), required.len());
        for (name, child) in properties {
            assert!(required.contains(&json!(name)));
            assert_closed(child);
        }
    }
    if let Some(items) = schema.get("items") {
        assert_closed(items);
    }
    for keyword in ["anyOf", "oneOf"] {
        if let Some(branches) = schema[keyword].as_array() {
            for branch in branches {
                assert_closed(branch);
            }
        }
    }
}

fn require_all_fields(schema: &mut Value) {
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        for child in properties.values_mut() {
            require_all_fields(child);
        }
        let names = properties.keys().cloned().collect::<Vec<_>>();
        schema["required"] = json!(names);
    }
    if let Some(items) = schema.get_mut("items") {
        require_all_fields(items);
    }
    for keyword in ["anyOf", "oneOf"] {
        if let Some(branches) = schema.get_mut(keyword).and_then(Value::as_array_mut) {
            for branch in branches {
                require_all_fields(branch);
            }
        }
    }
}

fn config() -> Config {
    let mut config = Config::default();
    if std::env::var_os("PROPTEST_CASES").is_none() {
        config.cases = 512;
    }
    if let Some(path) = option_env!("HARNESS_PROPTEST_REGRESSIONS") {
        config.failure_persistence = Some(Box::new(FileFailurePersistence::Direct(path)));
    }
    config
}

#[derive(Debug, Default)]
struct Coverage {
    arrays: usize,
    objects: usize,
    nullable: usize,
    type_arrays: usize,
    tagged_any_of: usize,
    tagged_one_of: usize,
    maximum_depth: usize,
}

impl Coverage {
    fn observe(&mut self, shape: &Shape, depth: usize) {
        self.maximum_depth = self.maximum_depth.max(depth);
        match shape {
            Shape::Array(child) => {
                self.arrays += 1;
                self.observe(child, depth + 1);
            }
            Shape::Object(left, right) => {
                self.objects += 1;
                self.observe(left, depth + 1);
                self.observe(right, depth + 1);
            }
            Shape::Nullable(child, type_array) => {
                self.nullable += 1;
                self.type_arrays += usize::from(*type_array && child.schema()["type"].is_string());
                self.observe(child, depth + 1);
            }
            Shape::Tagged(left, right, one_of) => {
                if *one_of {
                    self.tagged_one_of += 1;
                } else {
                    self.tagged_any_of += 1;
                }
                self.observe(left, depth + 1);
                self.observe(right, depth + 1);
            }
            _ => {}
        }
    }
}

fn run_property<S: Strategy>(
    name: &'static str,
    strategy: S,
    check: impl Fn(S::Value) -> TestCaseResult,
    coverage: &RefCell<Coverage>,
) {
    let mut config = contextualize_config(config());
    config.source_file = Some(file!());
    config.test_name = Some(name);
    let fresh_cases = config.cases;
    let callbacks = Cell::new(0);
    let result = TestRunner::new(config).run(&strategy, |value| {
        callbacks.set(callbacks.get() + 1);
        check(value)
    });
    eprintln!(
        "{name}: configured_fresh_cases={fresh_cases} actual_callbacks={} coverage={:?}",
        callbacks.get(),
        coverage.borrow()
    );
    assert!(result.is_ok(), "{result:?}");
}

fn request(parameters: Value) -> ResponsesRequest {
    ResponsesRequest {
        input: vec![],
        instructions: String::new(),
        tools: vec![
            json!({"type":"function","name":"generated","strict":true,"parameters":parameters}),
        ]
        .into(),
        tools_allowed: None,
        model: "fixture-model".into(),
        pinned_effort: Effort::Low,
        session_id: "generated-schema".into(),
    }
}

#[test]
fn recursive_host_schemas_preserve_strict_wire_and_omission_contract() {
    let coverage = RefCell::new(Coverage::default());
    run_property(
        "recursive_host_schemas_preserve_strict_wire_and_omission_contract",
        (shapes(), any::<u8>()),
        |(shape, seed)| {
            let host = Shape::Object(Box::new(shape.clone()), Box::new(shape));
            coverage.borrow_mut().observe(&host, 0);
            let schema = FunctionToolSchema::new(host.schema())?;
            let body = request_body(&request(schema.parameters().clone()))?;
            let advertised = &body["tools"][0]["parameters"];
            assert_closed(advertised);
            let wire = host.wire_value(seed);
            prop_assert!(
                accepts(advertised, &wire),
                "schema={advertised} wire={wire}"
            );
            let expected = host.host_value(&wire);
            prop_assert!(accepts(&host.schema(), &expected));
            prop_assert_eq!(schema.decode_arguments(wire)?, expected);
            Ok(())
        },
        &coverage,
    );
}

#[test]
fn nullable_type_arrays_preserve_independent_schema_semantics() {
    let coverage = RefCell::new(Coverage::default());
    run_property(
        "nullable_type_arrays_preserve_independent_schema_semantics",
        (shapes(), values()),
        |(shape, value)| {
            coverage.borrow_mut().observe(&shape, 0);
            let mut original = Shape::Nullable(Box::new(shape), true).schema();
            // Semantic equivalence applies to unchanged requiredness. The separate
            // host/wire property checks intentional optional-field null encoding.
            require_all_fields(&mut original);
            let normalized = normalize_schema(original.clone(), "$")?;
            prop_assert_eq!(
                accepts(&original, &value),
                accepts(&normalized, &value),
                "original={} normalized={} value={}",
                original,
                normalized,
                value
            );
            Ok(())
        },
        &coverage,
    );
}

#[test]
fn constrained_nullable_type_array_regression() {
    for schema in [
        json!({"type":["integer","null"],"minimum":0}),
        json!({"type":["string","null"],"minLength":1}),
        json!({"type":["array","null"],"items":{"type":"string"}}),
        json!({"type":["object","null"],"properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false}),
    ] {
        let normalized = normalize_schema(schema.clone(), "$").unwrap();
        for value in [
            Value::Null,
            json!(-1),
            json!(0),
            json!(""),
            json!("x"),
            json!([]),
            json!(["x"]),
            json!({"value":"x"}),
            json!({}),
        ] {
            assert_eq!(
                accepts(&schema, &value),
                accepts(&normalized, &value),
                "schema={schema} normalized={normalized} value={value}"
            );
        }
    }
}

#[test]
fn rust_optional_numeric_schema_retains_bounds_and_null() {
    let schema = super::tool_schema::<Option<u32>>().unwrap();
    let result = &schema["parameters"]["properties"]["result"];
    for value in [Value::Null, json!(0), json!(u32::MAX)] {
        assert!(accepts(result, &value));
    }
    for value in [json!(-1), json!(u64::from(u32::MAX) + 1)] {
        assert!(!accepts(result, &value));
    }
}

#[test]
fn strict_request_preserves_constrained_nullable_type_array_bytes() {
    let parameters = json!({"type":"object","properties":{"nested":{
        "type":["object","null"],"properties":{"entries":{
            "type":["array","null"],"items":{"type":["integer","null"],"minimum":0}
        }},"required":["entries"],"additionalProperties":false
    }},"required":["nested"],"additionalProperties":false});
    let body = request_body(&request(parameters.clone())).unwrap();
    assert_eq!(body["tools"][0]["parameters"], parameters);
    for key in ["required", "additionalProperties"] {
        let mut invalid = parameters.clone();
        invalid["properties"]["nested"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(request_body(&request(invalid)).is_err());
    }
}

#[test]
fn recursive_reference_model_supports_required_schema_partitions() {
    let shape = Shape::Object(
        Box::new(Shape::Array(Box::new(Shape::Nullable(
            Box::new(Shape::Integer(0)),
            true,
        )))),
        Box::new(Shape::Tagged(
            Box::new(Shape::Nullable(Box::new(Shape::Text(1)), false)),
            Box::new(Shape::Tagged(
                Box::new(Shape::Boolean),
                Box::new(Shape::Text(0)),
                false,
            )),
            true,
        )),
    );
    let mut coverage = Coverage::default();
    coverage.observe(&shape, 0);
    assert!(coverage.arrays > 0 && coverage.objects > 0 && coverage.type_arrays > 0);
    assert!(coverage.tagged_one_of > 0 && coverage.tagged_any_of > 0 && coverage.maximum_depth > 2);
    let schema = FunctionToolSchema::new(shape.schema()).unwrap();
    for seed in 0..8 {
        let wire = shape.wire_value(seed);
        assert!(accepts(schema.parameters(), &wire));
        assert_eq!(
            schema.decode_arguments(wire.clone()).unwrap(),
            shape.host_value(&wire)
        );
    }
}

#[test]
fn generated_invalid_nested_schemas_are_refused_before_request_serialization() {
    let coverage = RefCell::new(Coverage::default());
    run_property(
        "generated_invalid_nested_schemas_are_refused_before_request_serialization",
        (shapes(), 0_u8..8),
        |(shape, invalid)| {
            coverage.borrow_mut().observe(&shape, 0);
            let valid = normalize_schema(shape.schema(), "$")?;
            let bad = match invalid {
                0 => json!({"type":["string","null"],"pattern":".*"}),
                1 => json!({"type":["string","null"],"minLength":false}),
                2 => json!({"type":["integer","null"],"minimum":"zero"}),
                3 => json!({"type":["array","null"],"items":false}),
                4 => json!({"type":["object","null"],"properties":{},"additionalProperties":true}),
                5 => json!({"type":["string","string"]}),
                6 => json!({"type":["boolean","null"],"items":{"type":"string"}}),
                _ => json!({"type":["integer","null"],"unknownKeyword":true}),
            };
            let parameters = |bad| json!({"type":"object","properties":{"valid":valid,"nested":{"type":"array","items":bad}},"required":["valid","nested"],"additionalProperties":false});
            // Establish valid sibling and transport setup before introducing the
            // malformed leaf, so unrelated refusal cannot satisfy this property.
            let control = request(parameters(json!({"type":"string"})));
            prop_assert!(
                request_body(&control).is_ok(),
                "valid sibling and transport setup refused"
            );
            let malformed = parameters(bad);
            prop_assert!(normalize_schema(malformed.clone(), "$").is_err());
            prop_assert!(request_body(&request(malformed)).is_err());
            Ok(())
        },
        &coverage,
    );
}

#[test]
fn type_array_constraints_never_discard_global_or_unknown_keywords() {
    for schema in [
        json!({"type":["string","null"],"enum":["yes"]}),
        json!({"type":["string","null"],"enum":["yes",null]}),
        json!({"type":["integer","null"],"const":1,"minimum":0}),
        json!({"type":["integer","null"],"const":null,"minimum":0}),
    ] {
        let normalized = normalize_schema(schema.clone(), "$").unwrap();
        for value in [
            Value::Null,
            json!("yes"),
            json!("no"),
            json!(-1),
            json!(0),
            json!(1),
        ] {
            assert_eq!(
                accepts(&schema, &value),
                accepts(&normalized, &value),
                "schema={schema} value={value}"
            );
        }
    }
    for schema in [
        json!({"type":["string","null"],"pattern":".*"}),
        json!({"type":["string","null"],"minLength":false}),
        json!({"type":["integer","null"],"minimum":"zero"}),
        json!({"type":["array","null"],"items":false}),
        json!({"type":["object","null"],"properties":{},"additionalProperties":true}),
        json!({"type":["boolean","null"],"items":{"type":"string"}}),
        json!({"type":["string","string"],"minLength":1}),
    ] {
        assert!(
            normalize_schema(schema.clone(), "$").is_err(),
            "unsupported schema admitted: {schema}"
        );
    }
}

#[test]
fn type_array_pruning_preserves_provider_subset_and_typed_refusals() {
    for (schema, expected) in [
        (
            json!({"type":["string","null"],"enum":["yes"]}),
            json!({"anyOf":[{"type":"string","enum":["yes"]}]}),
        ),
        (
            json!({"type":["string","null"],"enum":["yes",null]}),
            json!({"anyOf":[{"type":"string","enum":["yes"]},{"type":"null"}]}),
        ),
        (
            json!({"type":["integer","null"],"const":null,"minimum":0}),
            json!({"anyOf":[{"type":"null"}]}),
        ),
        (
            json!({"type":["integer","null"],"enum":[-1,1,null],"minimum":0}),
            json!({"anyOf":[{"type":"integer","enum":[1],"minimum":0},{"type":"null"}]}),
        ),
    ] {
        assert_eq!(normalize_schema(schema, "$"), Ok(expected));
    }
    for schema in [
        json!({"type":["string","null"],"enum":[1]}),
        json!({"type":["integer","null"],"const":-1,"minimum":0}),
        json!({"type":["integer","null"],"enum":[-1],"minimum":0}),
        json!({"type":["integer","null"],"const":null,"enum":[1]}),
    ] {
        assert!(matches!(
            normalize_schema(schema, "$"),
            Err(super::FinalizeError::UnsatisfiableSchema { .. })
        ));
    }
    for schema in [
        json!({"type":["integer","null"],"const":"excluded","minimum":"malformed"}),
        json!({"type":["integer","null"],"const":"excluded","pattern":".*"}),
        json!({"type":["string","null"],"enum":[]}),
        json!({"type":"null","enum":[null]}),
        json!({"type":"null","const":null}),
    ] {
        assert!(matches!(
            normalize_schema(schema, "$"),
            Err(super::FinalizeError::UnsupportedSchema { .. })
        ));
    }
}
