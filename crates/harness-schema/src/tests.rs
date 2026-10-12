use super::*;

#[test]
fn strict_wire_refuses_unprojected_containers_formats_and_invalid_metadata() {
    let root = json!({"type":"object","properties":{},"required":[],"additionalProperties":false});
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
    }))
    .unwrap();
    assert!(
        matches!(schema.decode_arguments(json!({"choice":{"value":null}})), Err(SchemaError::AmbiguousArguments(path)) if path == "$.choice")
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
            Err(SchemaError::UnsupportedSchema { .. })
        ));
    }
}

#[test]
fn overlapping_one_of_is_rejected_and_direct_validation_requires_one_match() {
    assert!(
        normalize_schema(json!({"oneOf":[{"type":"number"},{"type":"integer"}]}), "$").is_err()
    );
    assert!(
        validate_value(
            &json!(2),
            &json!({"oneOf":[{"type":"number"},{"type":"integer"}]}),
            "$"
        )
        .is_err()
    );
    assert!(
        validate_value(
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
    assert!(validate_value(&json!(9007199254740992_u64), &minimum, "$").is_err());
    assert!(validate_value(&json!(9007199254740993_u64), &minimum, "$").is_ok());
    let number_minimum = json!({"type":"number","minimum":9007199254740993_u64});
    assert!(validate_value(&json!(9007199254740992.0_f64), &number_minimum, "$").is_err());
    let float_maximum = json!({"type":"number","maximum":9007199254740992.0_f64});
    assert!(validate_value(&json!(9007199254740993_u64), &float_maximum, "$").is_err());
    assert!(number_cmp(&json!(-2), &json!(-2.5)).unwrap().is_gt());
    assert!(number_cmp(&json!(2), &json!(2.5)).unwrap().is_lt());
    let exclusive = json!({"type":"integer","exclusiveMaximum":u64::MAX});
    assert!(validate_value(&json!(u64::MAX), &exclusive, "$").is_err());
    assert!(validate_value(&json!(u64::MAX - 1), &exclusive, "$").is_ok());
    let nonnegative = json!({"type":"integer","minimum":0_u64});
    assert!(validate_value(&json!(-1), &nonnegative, "$").is_err());
    assert!(
        validate_value(
            &json!(i64::MIN),
            &json!({"type":"integer","minimum":i64::MIN}),
            "$"
        )
        .is_ok()
    );
    assert!(normalize_schema(json!({"type":"integer","minimum":2.5}), "$").is_err());
    let schema = normalize_schema(
        serde_json::to_value(schemars::schema_for!(u32)).unwrap(),
        "$",
    )
    .unwrap();
    let integer = &schema;
    assert!(validate_value(&json!(u32::MAX), integer, "$").is_ok());
    assert!(validate_value(&json!(u32::MAX as u64 + 1), integer, "$").is_err());
    assert!(
        normalize_schema(
            serde_json::to_value(schemars::schema_for!(f64)).unwrap(),
            "$"
        )
        .is_ok()
    );
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
            Err(SchemaError::UnsatisfiableSchema { .. })
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
            Err(SchemaError::UnsupportedSchema { .. })
        ));
    }
}
