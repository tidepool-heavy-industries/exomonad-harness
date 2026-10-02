//! Deterministic, loss-aware rendering for explicitly portable context groups.
use super::{Result, reference};
use crate::{
    context::{ContextError, Occurrence},
    item::Item,
};
use serde_json::{Map, Value, json};

/// Render supported visible response items into one assistant note.
///
/// This boundary is deliberately strict: portability must not turn an unknown
/// provider item or encrypted payload into ordinary text that another model
/// might mistake for supported context.
pub(super) fn render(items: &[Occurrence]) -> Result<Item> {
    let mut rendered = Vec::with_capacity(items.len());
    let mut substantive = false;

    for occurrence in items {
        let kind = occurrence.item.0.get("type").and_then(Value::as_str);
        if kind == Some("compaction") {
            return Err(ContextError::OpaqueModel.into());
        }
        let (item, visible) = project_item(&occurrence.item)?;
        substantive |= visible;
        let origin = serde_json::to_value(&occurrence.origin)?;
        rendered.push(json!({"origin": origin, "item": item}));
    }

    if !substantive {
        return Err(ContextError::UnsupportedState.into());
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

    Ok(Item(json!({
        "type": "message",
        "role": "assistant",
        "content": note,
    })))
}

fn project_item(item: &Item) -> Result<(Value, bool)> {
    let object = item.0.as_object().ok_or(ContextError::UnsupportedState)?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or(ContextError::UnsupportedState)?;

    match kind {
        "message" => project_message(object),
        "function_call" => project_tool_call(object, false),
        "custom_tool_call" => project_tool_call(object, true),
        "function_call_output" => project_tool_output(object, false),
        "custom_tool_call_output" => project_tool_output(object, true),
        "reasoning" => project_reasoning(object),
        _ => Err(ContextError::UnsupportedState.into()),
    }
}

fn project_message(object: &Map<String, Value>) -> Result<(Value, bool)> {
    validate_fields(
        object,
        &["type", "role", "content", "phase", "id", "status"],
    )?;
    if object
        .get("phase")
        .is_some_and(|phase| !matches!(phase.as_str(), Some("commentary" | "final_answer")))
    {
        return Err(ContextError::UnsupportedState.into());
    }
    object
        .get("role")
        .and_then(Value::as_str)
        .filter(|role| matches!(*role, "user" | "assistant"))
        .ok_or(ContextError::UnsupportedState)?;
    let content = object
        .get("content")
        .ok_or(ContextError::UnsupportedState)?;
    let visible = match content {
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(parts) => {
            let mut any = false;
            for part in parts {
                let object = part.as_object().ok_or(ContextError::UnsupportedState)?;
                let kind = object
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or(ContextError::UnsupportedState)?;
                match kind {
                    "input_text" | "output_text" => {
                        validate_fields(object, &["type", "text", "annotations"])?;
                        let text = object
                            .get("text")
                            .and_then(Value::as_str)
                            .ok_or(ContextError::UnsupportedState)?;
                        if let Some(annotations) = object.get("annotations") {
                            reject_encrypted(annotations)?;
                        }
                        any |= !text.trim().is_empty();
                    }
                    "refusal" => {
                        validate_fields(object, &["type", "refusal", "annotations"])?;
                        let text = object
                            .get("refusal")
                            .and_then(Value::as_str)
                            .ok_or(ContextError::UnsupportedState)?;
                        if let Some(annotations) = object.get("annotations") {
                            reject_encrypted(annotations)?;
                        }
                        any |= !text.trim().is_empty();
                    }
                    _ => return Err(ContextError::UnsupportedState.into()),
                }
            }
            any
        }
        _ => return Err(ContextError::UnsupportedState.into()),
    };

    let mut projected = object.clone();
    strip_metadata(&mut projected);
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
        fields.extend(["arguments", "namespace", "async"]);
    }
    validate_fields(object, &fields)?;
    required_nonempty_string(object, "call_id")?;
    let name = required_nonempty_string(object, "name")?;
    if custom {
        object
            .get("input")
            .and_then(Value::as_str)
            .ok_or(ContextError::UnsupportedState)?;
    } else {
        let arguments = object
            .get("arguments")
            .ok_or(ContextError::UnsupportedState)?;
        if !matches!(arguments, Value::String(_) | Value::Object(_)) {
            return Err(ContextError::UnsupportedState.into());
        }
        if let Some(namespace) = object.get("namespace") {
            if !matches!(namespace, Value::Null) && namespace.as_str() != Some("functions") {
                return Err(ContextError::UnsupportedState.into());
            }
        }
        if object.get("async").is_some_and(|value| !value.is_boolean()) {
            return Err(ContextError::UnsupportedState.into());
        }
    }
    reject_encrypted(&Value::Object(object.clone()))?;
    let mut projected = object.clone();
    strip_metadata(&mut projected);
    Ok((Value::Object(projected), !name.trim().is_empty()))
}

fn project_tool_output(object: &Map<String, Value>, _custom: bool) -> Result<(Value, bool)> {
    validate_fields(object, &["type", "call_id", "output", "id", "status"])?;
    required_nonempty_string(object, "call_id")?;
    let output = object.get("output").ok_or(ContextError::UnsupportedState)?;
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
        &["type", "summary", "encrypted_content", "id", "status"],
    )?;
    if let Some(encrypted) = object.get("encrypted_content") {
        if !encrypted.is_string() {
            return Err(ContextError::UnsupportedState.into());
        }
    }
    let mut visible = false;
    if let Some(summary) = object.get("summary") {
        let summary = summary.as_array().ok_or(ContextError::UnsupportedState)?;
        for block in summary {
            let block = block.as_object().ok_or(ContextError::UnsupportedState)?;
            validate_fields(block, &["type", "text"])?;
            if block.get("type").and_then(Value::as_str) != Some("summary_text") {
                return Err(ContextError::UnsupportedState.into());
            }
            let text = block
                .get("text")
                .and_then(Value::as_str)
                .ok_or(ContextError::UnsupportedState)?;
            visible |= !text.trim().is_empty();
        }
    }
    let mut projected = object.clone();
    strip_metadata(&mut projected);
    projected.remove("encrypted_content");
    Ok((Value::Object(projected), visible))
}

fn validate_fields(object: &Map<String, Value>, allowed: &[&str]) -> Result<()> {
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(ContextError::UnsupportedState.into());
    }
    Ok(())
}

fn required_nonempty_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    object
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ContextError::UnsupportedState.into())
}

fn strip_metadata(object: &mut Map<String, Value>) {
    object.remove("id");
    object.remove("status");
}

fn reject_encrypted(value: &Value) -> Result<()> {
    match value {
        Value::Object(object) => {
            if object.keys().any(|key| key.starts_with("encrypted")) {
                return Err(ContextError::UnsupportedState.into());
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
            origin: Origin {
                request: RequestId("request-1".into()),
                position,
                hash,
            },
            sources: vec![],
            note: false,
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
                ContextError::UnsupportedState
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
                    ContextError::UnsupportedState
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
}
