//! Deterministic, loss-aware rendering for explicitly portable context groups.
use super::{Result, reference};
use crate::{
    context::{ContextError, ContextPortabilityRejection, Occurrence},
    item::Item,
};
use serde_json::{Map, Value, json};

/// Render supported visible response items into one assistant note. The raw
/// Store occurrence is unchanged: only this cross-model projection discards
/// known provider metadata, auxiliary reasoning and encrypted continuity.
pub(super) fn render(items: &[Occurrence]) -> Result<Item> {
    let mut rendered = Vec::with_capacity(items.len());
    let mut substantive = false;
    for occurrence in items {
        if occurrence.item.0.get("type").and_then(Value::as_str) == Some("compaction") {
            return Err(ContextError::OpaqueModel.into());
        }
        let (item, visible) = project_item(&occurrence.item)?;
        substantive |= visible;
        let origin = serde_json::to_value(&occurrence.origin)?;
        rendered.push(json!({"origin": origin, "item": item}));
    }
    if !substantive {
        return Err(
            ContextError::Portability(ContextPortabilityRejection::NoVisibleContent).into(),
        );
    }
    let source_reference = reference(items);
    let mut note = format!(
        "[Store-generated model portability note; sources: {}]",
        source_reference.as_str()
    );
    for entry in rendered {
        let origin = entry.get("origin").expect("rendered origin");
        let item = entry.get("item").expect("rendered item");
        note.push_str("\n\nSource origin/hash ");
        note.push_str(&serde_json::to_string(origin)?);
        note.push_str(":\n");
        note.push_str(&serde_json::to_string_pretty(item)?);
    }
    Ok(Item(
        json!({"type": "message", "role": "assistant", "content": note}),
    ))
}

fn invalid(item: &'static str, field: &'static str) -> super::StoreError {
    ContextError::Portability(ContextPortabilityRejection::InvalidField { item, field }).into()
}

fn project_item(item: &Item) -> Result<(Value, bool)> {
    let object = item
        .0
        .as_object()
        .ok_or_else(|| invalid("item", "object"))?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("item", "type"))?;
    match kind {
        "message" => project_message(object),
        "function_call" => project_tool_call(object, false),
        "custom_tool_call" => project_tool_call(object, true),
        "function_call_output" => project_tool_output(object),
        "custom_tool_call_output" => project_tool_output(object),
        "reasoning" => project_reasoning(object),
        _ => Err(ContextError::Portability(ContextPortabilityRejection::UnsupportedItem).into()),
    }
}

fn project_message(object: &Map<String, Value>) -> Result<(Value, bool)> {
    validate_fields(
        object,
        &["type", "role", "content", "phase", "id", "status"],
        "message",
    )?;
    if object.get("phase").is_some_and(|phase| {
        !phase.is_null() && !matches!(phase.as_str(), Some("commentary" | "final_answer"))
    }) {
        return Err(invalid("message", "phase"));
    }
    object
        .get("role")
        .and_then(Value::as_str)
        .filter(|role| matches!(*role, "user" | "assistant"))
        .ok_or_else(|| invalid("message", "role"))?;
    let content = object
        .get("content")
        .ok_or_else(|| invalid("message", "content"))?;
    let mut projected = object.clone();
    let visible = match content {
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(parts) => {
            let mut any = false;
            let mut projected_parts = Vec::with_capacity(parts.len());
            for part in parts {
                let part = part
                    .as_object()
                    .ok_or_else(|| invalid("message.content", "object"))?;
                let kind = part
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("message.content", "type"))?;
                let text_field = match kind {
                    "input_text" => {
                        validate_fields(
                            part,
                            &["type", "text", "annotations"],
                            "message.input_text",
                        )?;
                        "text"
                    }
                    "output_text" => {
                        validate_fields(
                            part,
                            &["type", "text", "annotations", "logprobs"],
                            "message.output_text",
                        )?;
                        if let Some(logprobs) =
                            nullable_array(part, "logprobs", "message.output_text")?
                        {
                            for logprob in logprobs {
                                validate_logprob(logprob, true)?;
                            }
                        }
                        "text"
                    }
                    "refusal" => {
                        validate_fields(
                            part,
                            &["type", "refusal", "annotations"],
                            "message.refusal",
                        )?;
                        "refusal"
                    }
                    _ => return Err(invalid("message.content", "type")),
                };
                let text = part
                    .get(text_field)
                    .and_then(Value::as_str)
                    .ok_or_else(|| invalid("message.content", text_field))?;
                if let Some(annotations) = part.get("annotations") {
                    reject_encrypted(annotations)?;
                }
                any |= !text.trim().is_empty();
                let mut part = part.clone();
                part.remove("logprobs");
                projected_parts.push(Value::Object(part));
            }
            projected.insert("content".into(), Value::Array(projected_parts));
            any
        }
        _ => return Err(invalid("message", "content")),
    };
    strip_metadata(&mut projected);
    if projected.get("phase").is_some_and(Value::is_null) {
        projected.remove("phase");
    }
    reject_encrypted(&Value::Object(projected.clone()))?;
    Ok((Value::Object(projected), visible))
}

fn project_tool_call(object: &Map<String, Value>, custom: bool) -> Result<(Value, bool)> {
    let mut fields = vec![
        "type",
        "call_id",
        "name",
        "id",
        "status",
        "namespace",
        "async",
    ];
    if custom {
        fields.push("input");
    } else {
        fields.push("arguments");
    }
    validate_fields(object, &fields, "tool_call")?;
    required_nonempty_string(object, "call_id")?;
    let name = required_nonempty_string(object, "name")?;
    if custom {
        object
            .get("input")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("tool_call", "input"))?;
    } else {
        let arguments = object
            .get("arguments")
            .ok_or_else(|| invalid("tool_call", "arguments"))?;
        if !matches!(arguments, Value::String(_) | Value::Object(_)) {
            return Err(invalid("tool_call", "arguments"));
        }
    }
    if let Some(namespace) = object.get("namespace") {
        if !namespace.is_null() && namespace.as_str() != Some("functions") {
            return Err(invalid("tool_call", "namespace"));
        }
    }
    if object.get("async").is_some_and(|value| !value.is_boolean()) {
        return Err(invalid("tool_call", "async"));
    }
    reject_encrypted(&Value::Object(object.clone()))?;
    let mut projected = object.clone();
    strip_metadata(&mut projected);
    Ok((Value::Object(projected), !name.trim().is_empty()))
}

fn project_tool_output(object: &Map<String, Value>) -> Result<(Value, bool)> {
    validate_fields(
        object,
        &["type", "call_id", "output", "id", "status"],
        "tool_output",
    )?;
    required_nonempty_string(object, "call_id")?;
    let output = object
        .get("output")
        .ok_or_else(|| invalid("tool_output", "output"))?;
    reject_encrypted(output)?;
    let visible = match output {
        Value::Null | Value::Bool(false) => false,
        Value::String(text) => !text.trim().is_empty(),
        _ => true,
    };
    let mut projected = object.clone();
    strip_metadata(&mut projected);
    Ok((Value::Object(projected), visible))
}

fn project_reasoning(object: &Map<String, Value>) -> Result<(Value, bool)> {
    validate_fields(
        object,
        &[
            "type",
            "summary",
            "content",
            "encrypted_content",
            "id",
            "status",
        ],
        "reasoning",
    )?;
    if object
        .get("encrypted_content")
        .is_some_and(|value| !value.is_null() && !value.is_string())
    {
        return Err(invalid("reasoning", "encrypted_content"));
    }
    if let Some(content) = nullable_array(object, "content", "reasoning")? {
        for block in content {
            validate_reasoning_text(block, "reasoning_text", "reasoning.content")?;
        }
    }
    let mut visible = false;
    if let Some(summary) = object.get("summary") {
        let summary = summary
            .as_array()
            .ok_or_else(|| invalid("reasoning", "summary"))?;
        for block in summary {
            let text = validate_reasoning_text(block, "summary_text", "reasoning.summary")?;
            visible |= !text.trim().is_empty();
        }
    }
    let mut projected = object.clone();
    strip_metadata(&mut projected);
    projected.remove("encrypted_content");
    // Auxiliary reasoning is not the provider's visible summary and cannot
    // establish substantive content or become instructions for another model.
    projected.remove("content");
    Ok((Value::Object(projected), visible))
}

fn validate_reasoning_text<'a>(
    value: &'a Value,
    kind: &str,
    location: &'static str,
) -> Result<&'a str> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid(location, "object"))?;
    validate_fields(object, &["type", "text"], location)?;
    if object.get("type").and_then(Value::as_str) != Some(kind) {
        return Err(invalid(location, "type"));
    }
    object
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(location, "text"))
}

fn nullable_array<'a>(
    object: &'a Map<String, Value>,
    key: &'static str,
    location: &'static str,
) -> Result<Option<&'a Vec<Value>>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(values)) => Ok(Some(values)),
        _ => Err(invalid(location, key)),
    }
}

fn validate_logprob(value: &Value, top: bool) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("message.logprobs", "object"))?;
    let fields: &[&str] = if top {
        &["token", "bytes", "logprob", "top_logprobs"]
    } else {
        &["token", "bytes", "logprob"]
    };
    validate_fields(object, fields, "message.logprobs")?;
    object
        .get("token")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("message.logprobs", "token"))?;
    object
        .get("logprob")
        .and_then(Value::as_f64)
        .ok_or_else(|| invalid("message.logprobs", "logprob"))?;
    let bytes = object
        .get("bytes")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("message.logprobs", "bytes"))?;
    if bytes
        .iter()
        .any(|byte| byte.as_u64().is_none_or(|byte| byte > 255))
    {
        return Err(invalid("message.logprobs", "bytes"));
    }
    if top {
        let values = object
            .get("top_logprobs")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("message.logprobs", "top_logprobs"))?;
        for value in values {
            validate_logprob(value, false)?;
        }
    }
    Ok(())
}

fn validate_fields(
    object: &Map<String, Value>,
    allowed: &[&str],
    item: &'static str,
) -> Result<()> {
    if let Some(field) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(
            ContextError::Portability(ContextPortabilityRejection::UnsupportedField {
                item,
                field: field.clone(),
            })
            .into(),
        );
    }
    Ok(())
}
fn required_nonempty_string<'a>(
    object: &'a Map<String, Value>,
    key: &'static str,
) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| invalid("tool", key))
}
fn strip_metadata(object: &mut Map<String, Value>) {
    object.remove("id");
    object.remove("status");
}
fn reject_encrypted(value: &Value) -> Result<()> {
    match value {
        Value::Object(object) => {
            if object.keys().any(|key| key.starts_with("encrypted")) {
                return Err(ContextError::Portability(
                    ContextPortabilityRejection::EncryptedPayload,
                )
                .into());
            }
            for value in object.values() {
                reject_encrypted(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_encrypted(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{context::Origin, model::RequestId};
    use serde_json::json;

    fn occurrence(item: Value, position: i64) -> Occurrence {
        let item = Item(item);
        let hash = crate::item::ItemHash(format!("hash-{position}"));
        Occurrence {
            request: RequestId("request-1".into()),
            position,
            hash: hash.clone(),
            item,
            output_operation: None,
            origin: Origin {
                request: RequestId("request-1".into()),
                position,
                hash,
            },
            sources: vec![],
            note: false,
            overlays: Vec::new(),
        }
    }

    #[test]
    fn preserves_visible_parts_calls_outputs_origins_and_full_reference() {
        let items = vec![
            occurrence(
                json!({"type":"message","id":"provider-id","status":"completed","role":"assistant","content":[
                    {"type":"input_text","text":"A full user part.","annotations":[]},
                    {"type":"output_text","text":"A complete answer.","annotations":[{"type":"citation","url":"https://example.invalid"}]},
                    {"type":"refusal","refusal":"A full refusal.","annotations":[]}
                ]}),
                4,
            ),
            occurrence(
                json!({"type":"function_call","id":"call-item-id","status":"completed","call_id":"call-1","name":"lookup","arguments":"{\"query\": \"full input\"}"}),
                5,
            ),
            occurrence(
                json!({"type":"function_call_output","id":"output-item-id","status":"completed","call_id":"call-1","output":{"complete":"result"}}),
                6,
            ),
            occurrence(
                json!({"type":"custom_tool_call","id":"custom-item-id","status":"completed","call_id":"custom-1","name":"run_cell","input":"line 1\n\"quoted\""}),
                7,
            ),
            occurrence(
                json!({"type":"custom_tool_call_output","id":"custom-output-id","status":"completed","call_id":"custom-1","output":"full custom result"}),
                8,
            ),
            occurrence(
                json!({"type":"reasoning","id":"reasoning-id","status":"completed","encrypted_content":"secret","summary":[{"type":"summary_text","text":"First full summary."},{"type":"summary_text","text":"Second full summary."}]}),
                9,
            ),
        ];
        let rendered = render(&items).unwrap().0["content"]
            .as_str()
            .unwrap()
            .to_owned();

        assert!(rendered.starts_with(&format!(
            "[Store-generated model portability note; sources: {}]",
            reference(&items).as_str()
        )));
        for expected in [
            "A full user part.",
            "A complete answer.",
            "A full refusal.",
            "citation",
        ] {
            assert!(rendered.contains(expected));
        }
        assert!(!rendered.contains("call-item-id"));
        assert!(rendered.contains("\"arguments\": \"{\\\"query\\\": \\\"full input\\\"}\""));
        assert!(rendered.contains("\"output\": {\n    \"complete\": \"result\"\n  }"));
        assert!(rendered.contains("\"input\": \"line 1\\n\\\"quoted\\\"\""));
        assert!(rendered.contains("full custom result"));
        assert!(rendered.contains("First full summary."));
        assert!(rendered.contains("Second full summary."));
        assert!(rendered.contains("\"position\":5"));
        assert!(!rendered.contains("secret"));
    }

    #[test]
    fn accepts_empty_reasoning_only_when_the_group_has_visible_content() {
        let reasoning = occurrence(json!({"type":"reasoning","encrypted_content":"opaque"}), 0);
        assert!(matches!(
            render(std::slice::from_ref(&reasoning)),
            Err(crate::store::StoreError::Context(
                ContextError::Portability(_)
            ))
        ));
        let message = occurrence(
            json!({"type":"message","role":"user","content":"visible"}),
            1,
        );
        assert!(render(&[reasoning, message]).is_ok());
    }

    #[test]
    fn rejects_unknown_shapes_missing_tool_data_and_non_reasoning_ciphertext() {
        for item in [
            json!({"type":"image","url":"opaque"}),
            json!({"type":"message","role":"user","content":"hello","provider_secret":"x"}),
            json!({"type":"function_call","call_id":"c","name":"f"}),
            json!({"type":"function_call_output","call_id":"c"}),
            json!({"type":"custom_tool_call","call_id":"c","name":"f","input":{}}),
            json!({"type":"message","role":"user","content":[{"type":"image","url":"x"}]}),
            json!({"type":"message","role":"user","content":"hello","encrypted_payload":"opaque"}),
        ] {
            assert!(matches!(
                render(&[occurrence(item, 0)]),
                Err(crate::store::StoreError::Context(
                    ContextError::Portability(_)
                ))
            ));
        }
    }

    #[test]
    fn compaction_remains_opaque_across_models() {
        assert!(matches!(
            render(&[occurrence(
                json!({"type":"compaction","encrypted_content":"opaque"}),
                0
            )]),
            Err(crate::store::StoreError::Context(ContextError::OpaqueModel))
        ));
    }

    #[test]
    fn known_optional_provider_fields_project_without_metadata_or_auxiliary_reasoning() {
        for phase in [
            None,
            Some(Value::Null),
            Some(json!("commentary")),
            Some(json!("final_answer")),
        ] {
            for content in [
                json!("visible message"),
                json!([{"type":"output_text","text":"visible message","annotations":[],"logprobs":null}]),
                json!([{"type":"output_text","text":"visible message","annotations":[],"logprobs":[]}]),
                json!([{"type":"output_text","text":"visible message","annotations":[],"logprobs":[{"token":"metadata-token","bytes":[32,255],"logprob":-0.5,"top_logprobs":[{"token":"alternative","bytes":[],"logprob":-1.0}]}]}]),
            ] {
                let mut message = json!({"type":"message","id":"metadata-id","status":"completed","role":"assistant","content":content});
                if let Some(phase) = &phase {
                    message["phase"] = phase.clone();
                }
                let original = message.clone();
                let (projected, visible) = project_item(&Item(message)).unwrap();
                assert!(visible);
                assert!(projected.get("id").is_none() && projected.get("status").is_none());
                assert!(
                    !serde_json::to_string(&projected)
                        .unwrap()
                        .contains("logprobs")
                );
                assert_eq!(
                    projected.get("phase"),
                    phase.as_ref().filter(|phase| !phase.is_null())
                );
                assert_eq!(original["content"], content);
            }
        }
        for content in [
            None,
            Some(Value::Null),
            Some(json!([])),
            Some(json!([{"type":"reasoning_text","text":"auxiliary-private-text"}])),
        ] {
            for encrypted in [None, Some(Value::Null), Some(json!("synthetic-ciphertext"))] {
                let mut reasoning = json!({"type":"reasoning","id":"r","summary":[{"type":"summary_text","text":"visible summary"}]});
                if let Some(content) = &content {
                    reasoning["content"] = content.clone();
                }
                if let Some(encrypted) = &encrypted {
                    reasoning["encrypted_content"] = encrypted.clone();
                }
                let original = Item(reasoning.clone());
                let (projected, visible) = project_item(&original).unwrap();
                assert!(visible);
                assert_eq!(
                    projected,
                    json!({"type":"reasoning","summary":[{"type":"summary_text","text":"visible summary"}]})
                );
                assert_eq!(original.0, reasoning);
            }
        }
        let auxiliary = occurrence(
            json!({"type":"reasoning","content":[{"type":"reasoning_text","text":"auxiliary"}],"summary":[],"encrypted_content":null}),
            0,
        );
        assert!(matches!(
            render(&[auxiliary]),
            Err(crate::store::StoreError::Context(
                ContextError::Portability(ContextPortabilityRejection::NoVisibleContent)
            ))
        ));
    }

    #[test]
    fn malformed_optional_fields_refuse_with_payload_free_typed_causes() {
        for item in [
            json!({"type":"message","role":"assistant","phase":17,"content":"visible"}),
            json!({"type":"message","role":"assistant","phase":"private-invalid-value","content":"visible"}),
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"visible","logprobs":{}}]}),
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"visible","logprobs":[null]}]}),
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"visible","logprobs":[{"token":"x","bytes":[256],"logprob":-1,"top_logprobs":[]}]}]}),
            json!({"type":"reasoning","summary":[],"content":false}),
            json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":null}]}),
            json!({"type":"reasoning","summary":[],"content":[{"type":"output_text","text":"auxiliary"}]}),
            json!({"type":"reasoning","summary":[],"encrypted_content":{}}),
        ] {
            let error = project_item(&Item(item)).unwrap_err();
            assert!(matches!(
                error,
                crate::store::StoreError::Context(ContextError::Portability(
                    ContextPortabilityRejection::InvalidField { .. }
                ))
            ));
            assert!(!error.to_string().contains("private-invalid-value"));
        }
        let error = project_item(&Item(
            json!({"type":"reasoning","summary":[],"provider_extension":"private-payload"}),
        ))
        .unwrap_err();
        assert!(
            matches!(&error, crate::store::StoreError::Context(ContextError::Portability(ContextPortabilityRejection::UnsupportedField { item: "reasoning", field })) if field == "provider_extension")
        );
        assert!(!error.to_string().contains("private-payload"));
        for item in [
            json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"visible","annotations":[{"encrypted_content":"opaque"}]}]}),
            json!({"type":"function_call_output","call_id":"c","output":{"encrypted_payload":"opaque"}}),
        ] {
            assert!(matches!(
                project_item(&Item(item)),
                Err(crate::store::StoreError::Context(
                    ContextError::Portability(ContextPortabilityRejection::EncryptedPayload)
                ))
            ));
        }
    }

    #[test]
    fn both_tool_constructors_validate_the_shared_namespace_and_async_contract() {
        for custom in [false, true] {
            let base = if custom {
                json!({"type":"custom_tool_call","call_id":"original","name":"cell","input":"original input"})
            } else {
                json!({"type":"function_call","call_id":"original","name":"lookup","arguments":"{}"})
            };
            for namespace in [None, Some(Value::Null), Some(json!("functions"))] {
                for asynchronous in [None, Some(json!(false)), Some(json!(true))] {
                    let mut item = base.clone();
                    if let Some(namespace) = &namespace {
                        item["namespace"] = namespace.clone();
                    }
                    if let Some(asynchronous) = &asynchronous {
                        item["async"] = asynchronous.clone();
                    }
                    let (projected, _) = project_item(&Item(item.clone())).unwrap();
                    assert_eq!(projected, item);
                }
            }
            for (field, value) in [
                ("namespace", json!(17)),
                ("namespace", json!("other")),
                ("async", Value::Null),
                ("async", json!("true")),
            ] {
                let mut item = base.clone();
                item[field] = value;
                assert!(matches!(
                    project_item(&Item(item)),
                    Err(crate::store::StoreError::Context(
                        ContextError::Portability(ContextPortabilityRejection::InvalidField { .. })
                    ))
                ));
            }
        }
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(128))]
        #[test]
        fn provider_metadata_variations_preserve_exact_visible_projection(
            text in ".{0,80}", array in proptest::bool::ANY, phase in 0u8..4,
            logprobs in 0u8..3, auxiliary in 0u8..4, encrypted in 0u8..3,
        ) {
            let text = format!("visible {text}");
            let content = if array { json!([{"type":"output_text","text":text,"annotations":[]}]) } else { json!(text) };
            let mut expected = json!({"type":"message","role":"assistant","content":content});
            if phase >= 2 { expected["phase"] = json!(if phase == 2 { "commentary" } else { "final_answer" }); }
            let mut original = expected.clone();
            if phase == 1 { original["phase"] = Value::Null; }
            original["id"] = json!("provider-id"); original["status"] = json!("completed");
            if array && logprobs > 0 { original["content"][0]["logprobs"] = if logprobs == 1 { Value::Null } else { json!([]) }; }
            let before = Item(original);
            let (actual, visible) = project_item(&before).unwrap();
            proptest::prop_assert_eq!(actual, expected);
            proptest::prop_assert!(visible);
            let mut reasoning = json!({"type":"reasoning","summary":[{"type":"summary_text","text":text}]});
            if auxiliary > 0 { reasoning["content"] = match auxiliary { 1 => Value::Null, 2 => json!([]), _ => json!([{"type":"reasoning_text","text":"auxiliary-private"}]) }; }
            if encrypted > 0 { reasoning["encrypted_content"] = if encrypted == 1 { Value::Null } else { json!("synthetic-ciphertext") }; }
            let (actual, visible) = project_item(&Item(reasoning)).unwrap();
            proptest::prop_assert_eq!(actual, json!({"type":"reasoning","summary":[{"type":"summary_text","text":text}]}));
            proptest::prop_assert!(visible);
        }

        #[test]
        fn unknown_fields_never_become_visible_portable_payload(
            field in "provider_[a-z]{1,12}", payload in ".{0,80}", constructor in 0u8..4,
        ) {
            let mut value = match constructor {
                0 => json!({"type":"message","role":"assistant","content":"visible"}),
                1 => json!({"type":"reasoning","summary":[{"type":"summary_text","text":"visible"}]}),
                2 => json!({"type":"custom_tool_call","call_id":"c","name":"cell","input":"original"}),
                _ => json!({"type":"custom_tool_call_output","call_id":"c","output":"result"}),
            };
            value[&field] = json!(payload);
            let error = project_item(&Item(value)).unwrap_err();
            proptest::prop_assert!(matches!(error, crate::store::StoreError::Context(ContextError::Portability(ContextPortabilityRejection::UnsupportedField { .. }))), "unknown provider field must be refused");
        }
    }
}
