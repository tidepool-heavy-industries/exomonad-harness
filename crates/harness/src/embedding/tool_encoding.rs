//! Installed wire encodings retain their issuing host contract.
use super::*;
use crate::item::{ToolCall, ToolExecution};
use crate::transport::ToolEncodingProfile;
use serde_json::json;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EncodingMap {
    profile: ToolEncodingProfile,
    rows: BTreeMap<String, Encoding>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Encoding {
    Identity,
    Text { field: String, delivery: bool },
    Arguments,
}

impl EncodingMap {
    pub(super) fn project(
        manifest: &EmbeddedToolManifest,
        profile: ToolEncodingProfile,
    ) -> Result<(EmbeddedToolManifest, Self), EmbeddedError> {
        let mut tools = manifest.tools.to_vec();
        let mut rows = BTreeMap::new();
        for tool in &mut tools {
            let name = tool["name"]
                .as_str()
                .expect("admitted declaration")
                .to_owned();
            if profile == ToolEncodingProfile::Native {
                rows.insert(name, Encoding::Identity);
                continue;
            }
            if name.len() > 128
                || !name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            {
                return Err(EmbeddedError::Surface(format!(
                    "unsupported flat tool name {name}"
                )));
            }
            if tool.get("namespace").is_some_and(|value| !value.is_null()) {
                return Err(EmbeddedError::Surface(format!(
                    "tool namespace is not flat: {name}"
                )));
            }
            let delivery = manifest.scheduling.get(&name).copied().unwrap_or_default()
                == ToolScheduling::Async;
            let encoding = if manifest.kinds[&name] == ToolKind::Custom {
                Encoding::Text {
                    field: manifest
                        .text_fields
                        .get(&name)
                        .cloned()
                        .unwrap_or_else(|| "input".into()),
                    delivery,
                }
            } else if delivery {
                Encoding::Arguments
            } else {
                Encoding::Identity
            };
            if !matches!(encoding, Encoding::Identity) {
                let (field, schema) = match &encoding {
                    Encoding::Text { field, .. } => (field.clone(), json!({"type":"string"})),
                    Encoding::Arguments => ("arguments".into(), tool["parameters"].clone()),
                    Encoding::Identity => unreachable!(),
                };
                let mut properties = serde_json::Map::new();
                properties.insert(field.clone(), schema);
                let mut required = vec![field];
                if delivery {
                    properties.insert(
                        "delivery".into(),
                        json!({"type":"string","enum":["wait","background"]}),
                    );
                    required.push("delivery".into());
                }
                let description = tool["description"].as_str().unwrap_or("").to_owned();
                *tool = json!({"type":"function","name":name,"description":description,
                    "strict":true,"parameters":{"type":"object","properties":properties,
                    "required":required,"additionalProperties":false}});
            }
            tool.as_object_mut()
                .expect("admitted declaration")
                .remove("async");
            rows.insert(name, encoding);
        }
        let mut projected =
            EmbeddedToolManifest::with_scheduling(tools, manifest.scheduling.clone())?;
        // Async is host policy, not a supported wire declaration extension.
        if profile != ToolEncodingProfile::Native {
            let mut tools = projected.tools.to_vec();
            for tool in &mut tools {
                tool.as_object_mut().expect("admitted").remove("async");
            }
            projected.tools = tools.into();
        }
        Ok((projected, Self { profile, rows }))
    }

    pub(super) fn decode(
        &self,
        name: &str,
        input: ToolInput,
    ) -> Result<(ToolInput, Option<ToolExecution>), ProviderError> {
        let fail =
            || ProviderError::Tool(format!("invalid installed tool envelope for {name}").into());
        let row = self.rows.get(name).ok_or_else(fail)?;
        if matches!(row, Encoding::Identity) {
            return Ok((input, None));
        }
        let ToolInput::Function(Value::Object(mut arguments)) = input else {
            return Err(fail());
        };
        let delivery = match row {
            Encoding::Text { delivery, .. } => *delivery,
            Encoding::Arguments => true,
            _ => unreachable!(),
        };
        let execution = if delivery {
            match arguments
                .remove("delivery")
                .and_then(|v| v.as_str().map(str::to_owned))
                .as_deref()
            {
                Some("wait") => ToolExecution::Synchronous,
                Some("background") => ToolExecution::Asynchronous,
                _ => return Err(fail()),
            }
        } else {
            ToolExecution::Synchronous
        };
        let (field, custom) = match row {
            Encoding::Text { field, .. } => (field.as_str(), true),
            Encoding::Arguments => ("arguments", false),
            _ => unreachable!(),
        };
        let value = arguments.remove(field).ok_or_else(fail)?;
        if !arguments.is_empty() {
            return Err(fail());
        }
        let input = if custom {
            ToolInput::Custom(value.as_str().ok_or_else(fail)?.to_owned())
        } else {
            if !value.is_object() {
                return Err(fail());
            }
            ToolInput::Function(value)
        };
        Ok((input, Some(execution)))
    }

    pub(super) fn execution(&self, call: &ToolCall) -> Result<ToolExecution, ProviderError> {
        self.decode(&call.name, call.input.clone())
            .map(|(_, execution)| {
                execution.unwrap_or(if self.profile == ToolEncodingProfile::Native {
                    call.execution
                } else {
                    ToolExecution::Synchronous
                })
            })
    }
}

fn operation_policy(
    store: &Store,
    operation: &crate::model::OperationId,
) -> Result<Value, ProviderError> {
    let fail = |s: String| ProviderError::Tool(s.into());
    let mut selected = None;
    for event in store
        .events(Some(&operation.request))
        .map_err(|e| fail(e.to_string()))?
    {
        if event.kind != "tool_scheduling" {
            continue;
        }
        let value: Value = serde_json::from_str(&event.payload).map_err(|e| fail(e.to_string()))?;
        if value.get("operation")
            != Some(&serde_json::to_value(operation).expect("operation serializes"))
        {
            continue;
        }
        if selected.as_ref().is_some_and(|prior| prior != &value) {
            return Err(fail("conflicting issuing operation surfaces".into()));
        }
        selected = Some(value);
    }
    selected.ok_or_else(|| fail("missing issuing operation surface".into()))
}

pub(super) fn operation_surface(
    store: &Store,
    operation: &crate::model::OperationId,
) -> Result<Value, ProviderError> {
    operation_policy(store, operation)?
        .get("surface")
        .cloned()
        .ok_or_else(|| ProviderError::Tool("missing issuing operation surface".into()))
}

pub(super) fn recorded_execution(
    store: &Store,
    operation: &crate::model::OperationId,
    call: &ToolCall,
    profile: ToolEncodingProfile,
) -> Result<ToolExecution, ProviderError> {
    if profile == ToolEncodingProfile::Native {
        return Ok(call.execution);
    }
    let fail = |s: String| ProviderError::Tool(s.into());
    let surface = operation_surface(store, operation)?;
    let map: EncodingMap = serde_json::from_value(
        surface
            .get("encoding")
            .cloned()
            .ok_or_else(|| fail("missing issuing tool encoding".into()))?,
    )
    .map_err(|e| fail(e.to_string()))?;
    if map.profile != profile {
        return Err(fail("issuing tool encoding profile mismatch".into()));
    }
    let execution = map.execution(call)?;
    let expected = match execution {
        ToolExecution::Synchronous => "wait",
        ToolExecution::Asynchronous => "background",
    };
    if operation_policy(store, operation)?["execution"] != expected {
        return Err(fail("issuing operation delivery mismatch".into()));
    }
    Ok(execution)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::CallId;

    fn custom(schedule: ToolScheduling) -> EmbeddedToolManifest {
        EmbeddedToolManifest::with_text_fields(
            vec![json!({"type":"custom","name":"notebook","description":"same host"})],
            HashMap::from([("notebook".into(), schedule)]),
            HashMap::from([("notebook".into(), "source".into())]),
        )
        .unwrap()
    }

    #[test]
    fn explicit_delivery_preserves_exact_custom_source_and_native_encoding() {
        let manifest = custom(ToolScheduling::Async);
        let (wire, map) =
            EncodingMap::project(&manifest, ToolEncodingProfile::FunctionOnlyExplicitDelivery)
                .unwrap();
        assert_eq!(wire.tools[0]["type"], "function");
        assert!(wire.tools[0].get("async").is_none());
        let source = "  display (\"雪 \\\"quoted\\\"\")\n-- {source: not JSON}\n";
        for (delivery, execution) in [
            ("wait", ToolExecution::Synchronous),
            ("background", ToolExecution::Asynchronous),
        ] {
            let input = ToolInput::Function(json!({"source":source,"delivery":delivery}));
            let call = ToolCall {
                call_id: CallId("same".into()),
                name: "notebook".into(),
                input: input.clone(),
                execution: ToolExecution::Synchronous,
            };
            assert_eq!(map.execution(&call).unwrap(), execution);
            assert_eq!(
                map.decode("notebook", input).unwrap().0,
                ToolInput::Custom(source.into())
            );
        }
        let (native, native_map) =
            EncodingMap::project(&manifest, ToolEncodingProfile::Native).unwrap();
        assert_eq!(native.tools[0]["type"], "custom");
        assert_eq!(
            native_map
                .decode("notebook", ToolInput::Custom(source.into()))
                .unwrap(),
            (ToolInput::Custom(source.into()), None)
        );
    }

    #[test]
    fn explicit_delivery_rejects_wrong_envelopes_and_reserved_text_field() {
        let (_, map) = EncodingMap::project(
            &custom(ToolScheduling::Async),
            ToolEncodingProfile::FunctionOnlyExplicitDelivery,
        )
        .unwrap();
        for value in [
            json!({"source":"x"}),
            json!({"source":0,"delivery":"wait"}),
            json!({"source":"x","delivery":"later"}),
            json!({"source":"x","delivery":"wait","extra":true}),
        ] {
            assert!(map.decode("notebook", ToolInput::Function(value)).is_err());
        }
        assert!(
            map.decode("notebook", ToolInput::Custom("raw".into()))
                .is_err()
        );
        assert!(
            map.decode("unknown", ToolInput::Function(json!({})))
                .is_err()
        );
        assert!(
            EmbeddedToolManifest::with_text_fields(
                vec![json!({"type":"custom","name":"x"})],
                HashMap::new(),
                HashMap::from([("x".into(), "delivery".into())])
            )
            .is_err()
        );
        let (_, sync) = EncodingMap::project(
            &custom(ToolScheduling::BeforeNextInference),
            ToolEncodingProfile::FunctionOnlyExplicitDelivery,
        )
        .unwrap();
        assert!(
            sync.decode(
                "notebook",
                ToolInput::Function(json!({"source":"x","delivery":"background"}))
            )
            .is_err()
        );
    }

    #[test]
    fn structured_envelope_does_not_steal_original_delivery_fields() {
        let tools = vec![
            json!({"type":"function","name":"structured","strict":true,"parameters":{"type":"object","properties":{"delivery":{"type":"string"}},"required":["delivery"],"additionalProperties":false}}),
        ];
        let original = json!({"delivery":"a domain value"});
        let async_manifest = EmbeddedToolManifest::with_scheduling(
            tools.clone(),
            HashMap::from([("structured".into(), ToolScheduling::Async)]),
        )
        .unwrap();
        let (_, map) = EncodingMap::project(
            &async_manifest,
            ToolEncodingProfile::FunctionOnlyExplicitDelivery,
        )
        .unwrap();
        assert_eq!(
            map.decode(
                "structured",
                ToolInput::Function(json!({"arguments":original,"delivery":"background"}))
            )
            .unwrap(),
            (
                ToolInput::Function(original.clone()),
                Some(ToolExecution::Asynchronous)
            )
        );
        let sync_manifest = EmbeddedToolManifest::with_scheduling(
            tools,
            HashMap::from([("structured".into(), ToolScheduling::BeforeNextInference)]),
        )
        .unwrap();
        let (_, map) = EncodingMap::project(
            &sync_manifest,
            ToolEncodingProfile::FunctionOnlyExplicitDelivery,
        )
        .unwrap();
        assert_eq!(
            map.decode("structured", ToolInput::Function(original.clone()))
                .unwrap(),
            (ToolInput::Function(original), None)
        );
    }
}
