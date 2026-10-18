//! Argon Interactions: retain native steps and derive the existing engine view.
//! The native step is replay authority; the projection never replaces it.
use super::{
    InferenceLimits, ResponsesRequest, ResponsesTurn, TransportError, UnsupportedSemantics, Usage,
    sse::{CompletedOutputItem, OutputChannel, StreamEvent},
};
use crate::{item::Item, model::Effort};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

fn invalid() -> TransportError {
    TransportError::UnsupportedSemantics(UnsupportedSemantics::InputItem)
}
fn protocol(message: &'static str) -> TransportError {
    TransportError::Stream(message.into())
}

pub(super) fn request(
    request: &ResponsesRequest,
    limits: InferenceLimits,
    authored: &[bool],
) -> Result<Value, TransportError> {
    if request.model != "gemini-4-argon"
        || !request.session_id.valid()
        || authored.len() != request.input.len()
    {
        return Err(TransportError::InvalidAdmission);
    }
    let effort = Item::effective_effort(&request.input, request.pinned_effort);
    if !matches!(effort, Effort::Low | Effort::Medium | Effort::High) {
        return Err(TransportError::UnsupportedSemantics(
            UnsupportedSemantics::ReasoningEffort,
        ));
    }
    if request.prompt_cache_comparison.is_some() {
        return Err(TransportError::UnsupportedSemantics(
            UnsupportedSemantics::CacheComparison,
        ));
    }
    request.tools.strict_tools()?;
    if request.tools.has_duplicate_names() {
        return Err(invalid());
    }
    let mut tools = Vec::new();
    for tool in request.tools.iter() {
        if tool["type"] != "function" || !tool["name"].is_string() {
            return Err(TransportError::UnsupportedSemantics(
                UnsupportedSemantics::ToolDeclaration,
            ));
        }
        if request
            .tools_allowed
            .as_ref()
            .is_none_or(|names| names.iter().any(|name| tool["name"] == name.as_str()))
        {
            tools.push(json!({"type":"function", "name":tool["name"], "description":tool["description"], "parameters":tool["parameters"]}));
        }
    }
    let mut input = Vec::new();
    let mut calls = HashMap::<String, (String, usize)>::new();
    let mut results = Vec::<Option<Value>>::new();
    let mut result_slots = Vec::new();
    for (item, authored) in request.input.iter().zip(authored) {
        if item.is_configuration_update() {
            continue;
        }
        if let Some(step) = item.0.get("gemini_step") {
            // Context overlays cannot silently change the immutable native step.
            let id = item.0["id"].as_str().ok_or_else(invalid)?;
            let expected = project(step.clone(), id)?;
            if expected != *item {
                return Err(invalid());
            }
            if step["type"] == "function_call" {
                let id = step["id"].as_str().ok_or_else(invalid)?;
                if calls
                    .insert(
                        id.into(),
                        (
                            step["name"].as_str().ok_or_else(invalid)?.into(),
                            results.len(),
                        ),
                    )
                    .is_some()
                {
                    return Err(invalid());
                }
                results.push(None);
            }
            input.push(step.clone());
            continue;
        }
        match item.0["type"].as_str() {
            Some("message")
                if *authored
                    || matches!(
                        item.0["role"].as_str(),
                        Some("user" | "developer" | "system")
                    ) =>
            {
                if item.0.get("encrypted_content").is_some() {
                    return Err(invalid());
                }
                let content = text_content(&item.0["content"])?;
                // Context notes, system/developer additions and user input are
                // explicit local input, never fabricated model-native history.
                input.push(json!({"type":"user_input","content":content}));
            }
            Some("function_call_output") => {
                let id = item.0["call_id"].as_str().ok_or_else(invalid)?;
                let (name, ordinal) = calls.remove(id).ok_or_else(invalid)?;
                let output = item.0.get("output").ok_or_else(invalid)?;
                let result = match output {
                    Value::String(text) => json!([{"type":"text","text":text}]),
                    other => other.clone(),
                };
                results[ordinal] = Some(
                    json!({"type":"function_result","call_id":id,"name":name,"result":result}),
                );
                result_slots.push(input.len());
                input.push(Value::Null);
            }
            _ => return Err(invalid()),
        }
    }
    if !calls.is_empty() {
        return Err(invalid());
    }
    // Native siblings retain their exact positions and bytes. Settlements occupy
    // their existing input slots in call issuance order, independently of when
    // the callbacks completed. Ordinals distinguish IDs reused in later turns.
    for (slot, result) in result_slots.into_iter().zip(results) {
        input[slot] = result.ok_or_else(invalid)?;
    }
    let mut config = json!({"thinking_level":effort,"thinking_summaries":"auto"});
    if let Some(limit) = limits.max_output_tokens {
        config["max_output_tokens"] = limit.into();
    }
    if !tools.is_empty() {
        config["tool_choice"] = "auto".into();
    }
    let mut body = json!({"model":request.model,"input":input,"store":false,"stream":true,"generation_config":config});
    if !request.instructions.is_empty() {
        body["system_instruction"] = request.instructions.clone().into();
    }
    if !tools.is_empty() {
        body["tools"] = tools.into();
    }
    Ok(body)
}

fn text_content(content: &Value) -> Result<Vec<Value>, TransportError> {
    if let Some(text) = content.as_str() {
        return Ok(vec![json!({"type":"text","text":text})]);
    }
    content
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(|part| {
            if matches!(
                part["type"].as_str(),
                Some("text" | "input_text" | "output_text")
            ) {
                Ok(json!({"type":"text","text":part["text"].as_str().ok_or_else(invalid)?}))
            } else {
                Err(invalid())
            }
        })
        .collect()
}

/// The envelope makes the projection and its exact provider source indivisible.
fn project(step: Value, display_id: &str) -> Result<Item, TransportError> {
    let kind = step["type"].as_str().ok_or_else(invalid)?;
    let mut value = match kind {
        "function_call" => json!({"type":"function_call","id":display_id,
            "call_id":step["id"].as_str().filter(|s| !s.is_empty()).ok_or_else(invalid)?,
            "name":step["name"].as_str().filter(|s| !s.is_empty()).ok_or_else(invalid)?,
            "arguments":step["arguments"].as_object().ok_or_else(invalid)?}),
        "thought" => {
            if step
                .get("signature")
                .is_some_and(|signature| !signature.is_string())
            {
                return Err(invalid());
            }
            let summaries = step
                .get("summary")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|part| {
                    part["text"]
                        .as_str()
                        .map(|text| json!({"type":"summary_text","text":text}))
                })
                .collect::<Vec<_>>();
            json!({"type":"reasoning","id":display_id,"summary":summaries})
        }
        "model_output" => {
            let parts = match step.get("content") {
                None => Vec::new(),
                Some(content) => content
                    .as_array()
                    .ok_or_else(invalid)?
                    .iter()
                    .map(|part| {
                        if part["type"] != "text" {
                            return Err(invalid());
                        }
                        let text = part["text"].as_str().ok_or_else(invalid)?;
                        Ok(json!({"type":"output_text","text":text}))
                    })
                    .collect::<Result<Vec<_>, TransportError>>()?,
            };
            json!({"type":"message","id":display_id,"role":"assistant","content":parts})
        }
        _ => json!({"type":"gemini_native","id":display_id}),
    };
    value["gemini_step"] = step;
    Ok(Item(value))
}

#[derive(Default)]
struct Step {
    native: Value,
    arguments: Option<String>,
    stopped: bool,
}

#[derive(Default)]
pub(super) struct Assembly {
    id: Option<String>,
    steps: BTreeMap<u64, Step>,
    next_emitted: u64,
    terminal: Option<Usage>,
    seen_events: HashMap<String, Value>,
}
impl Assembly {
    fn identity(&mut self, id: &str) -> Result<(), TransportError> {
        if id.is_empty() || self.id.as_deref().is_some_and(|old| old != id) {
            return Err(protocol("interaction identity changed"));
        }
        self.id = Some(id.into());
        Ok(())
    }
    pub(super) fn accept(
        &mut self,
        data: &str,
        observation: &mut super::TransportObservation,
    ) -> Result<Vec<StreamEvent>, TransportError> {
        let event: Value =
            serde_json::from_str(data).map_err(|_| protocol("invalid Interactions SSE JSON"))?;
        if let Some(id) = event["event_id"].as_str() {
            if let Some(previous) = self.seen_events.get(id) {
                return if previous == &event {
                    Ok(Vec::new())
                } else {
                    Err(protocol("conflicting Interactions event id"))
                };
            }
            self.seen_events.insert(id.into(), event.clone());
        }
        let kind = event["event_type"]
            .as_str()
            .ok_or_else(|| protocol("missing Interactions event type"))?;
        observation.event(super::EventKind::from_provider(Some(kind)));
        if self.terminal.is_some() {
            return Err(protocol("event after terminal interaction"));
        }
        let mut events = Vec::new();
        match kind {
            "interaction.created" => {
                self.identity(event["interaction"]["id"].as_str().ok_or_else(invalid)?)?
            }
            "step.start" => {
                let index = event["index"]
                    .as_u64()
                    .ok_or(TransportError::MissingOutputIndex)?;
                let native = event
                    .get("step")
                    .filter(|s| s.is_object())
                    .ok_or_else(invalid)?
                    .clone();
                if self
                    .steps
                    .insert(
                        index,
                        Step {
                            native,
                            ..Step::default()
                        },
                    )
                    .is_some()
                {
                    return Err(TransportError::ConflictingOutputIndex { index });
                }
            }
            "step.delta" => {
                let index = event["index"]
                    .as_u64()
                    .ok_or(TransportError::MissingOutputIndex)?;
                let step = self
                    .steps
                    .get_mut(&index)
                    .ok_or_else(|| protocol("delta before step start"))?;
                if step.stopped {
                    return Err(protocol("delta after step stop"));
                }
                let delta = &event["delta"];
                let mut visible = None;
                match delta["type"].as_str() {
                    Some("thought_signature") if step.native["type"] == "thought" => {
                        let signature = delta["signature"].as_str().ok_or_else(invalid)?;
                        // A signature delta is the complete opaque signature, not text.
                        if step.native["signature"]
                            .as_str()
                            .is_some_and(|old| !old.is_empty() && old != signature)
                        {
                            return Err(protocol("conflicting thought signature"));
                        }
                        step.native["signature"] = signature.into();
                    }
                    Some("thought_summary") if step.native["type"] == "thought" => {
                        let content = delta.get("content").ok_or_else(invalid)?.clone();
                        if let Some(text) = content["text"].as_str() {
                            visible = Some((OutputChannel::ReasoningSummary, text.to_owned()));
                        }
                        if step.native.get("summary").is_none() {
                            step.native["summary"] = json!([]);
                        }
                        step.native["summary"]
                            .as_array_mut()
                            .ok_or_else(invalid)?
                            .push(content);
                    }
                    Some("arguments_delta") if step.native["type"] == "function_call" => {
                        let text = delta["arguments"].as_str().ok_or_else(invalid)?;
                        step.arguments
                            .get_or_insert_with(String::new)
                            .push_str(text);
                        visible = Some((OutputChannel::ToolArguments, text.to_owned()));
                    }
                    Some("text") if step.native["type"] == "model_output" => {
                        let text = delta["text"].as_str().ok_or_else(invalid)?;
                        if step.native.get("content").is_none() {
                            step.native["content"] = json!([]);
                        }
                        let parts = step.native["content"].as_array_mut().ok_or_else(invalid)?;
                        if let Some(last) = parts.last_mut().filter(|part| part["type"] == "text") {
                            let joined =
                                format!("{}{}", last["text"].as_str().ok_or_else(invalid)?, text);
                            last["text"] = joined.into();
                        } else {
                            parts.push(json!({"type":"text","text":text}));
                        }
                        visible = Some((OutputChannel::Assistant, text.to_owned()));
                    }
                    _ => return Err(protocol("unsupported Interactions step delta")),
                }
                if let Some((channel, text)) = visible {
                    events.push(StreamEvent::Delta {
                        item_id: format!("{}:{index}", self.id.as_deref().ok_or_else(invalid)?),
                        channel,
                        index: 0,
                        text,
                        tool_name: step.native["name"].as_str().map(str::to_owned),
                    });
                }
            }
            "step.stop" => {
                let index = event["index"]
                    .as_u64()
                    .ok_or(TransportError::MissingOutputIndex)?;
                let step = self
                    .steps
                    .get_mut(&index)
                    .ok_or_else(|| protocol("stop before step start"))?;
                if step.stopped {
                    return Err(protocol("duplicate step stop"));
                }
                if let Some(arguments) = step.arguments.take() {
                    step.native["arguments"] = serde_json::from_str(&arguments)
                        .map_err(|_| protocol("invalid completed function arguments"))?;
                }
                step.stopped = true;
            }
            "interaction.completed" | "interaction.status_update" => {
                let interaction = if kind == "interaction.completed" {
                    &event["interaction"]
                } else {
                    &event
                };
                self.identity(
                    interaction
                        .get("id")
                        .or_else(|| interaction.get("interaction_id"))
                        .and_then(Value::as_str)
                        .ok_or_else(invalid)?,
                )?;
                match interaction["status"].as_str() {
                    Some("in_progress" | "queued") if kind == "interaction.status_update" => {}
                    Some("completed" | "requires_action") => {
                        if let Some(steps) = interaction
                            .get("steps")
                            .and_then(Value::as_array)
                            .filter(|v| !v.is_empty())
                        {
                            for (index, native) in steps.iter().enumerate() {
                                if let Some(prior) = self.steps.get(&(index as u64)) {
                                    if !prior.stopped || &prior.native != native {
                                        return Err(TransportError::ConflictingOutputIndex {
                                            index: index as u64,
                                        });
                                    }
                                } else {
                                    self.steps.insert(
                                        index as u64,
                                        Step {
                                            native: native.clone(),
                                            stopped: true,
                                            arguments: None,
                                        },
                                    );
                                }
                            }
                            if self.steps.len() != steps.len() {
                                return Err(invalid());
                            }
                        }
                        if self.steps.values().any(|step| !step.stopped) {
                            return Err(protocol("interaction ended with unfinished step"));
                        }
                        if interaction["status"] == "requires_action"
                            && !self
                                .steps
                                .values()
                                .any(|step| step.native["type"] == "function_call")
                        {
                            return Err(protocol("requires_action without function call"));
                        }
                        let usage = Usage::from_interaction_usage(interaction);
                        observation.usage = Some(usage.clone());
                        observation.terminal = Some(super::ResponseTerminal::Completed);
                        self.terminal = Some(usage);
                    }
                    Some("incomplete" | "budget_exceeded") => {
                        observation.terminal = Some(super::ResponseTerminal::Incomplete {
                            response_id: self.id.clone().unwrap(),
                            reason: None,
                        });
                        return Err(protocol("incomplete Gemini interaction"));
                    }
                    Some("failed" | "cancelled") => {
                        observation.terminal = Some(super::ResponseTerminal::ResponseFailed);
                        return Err(protocol("Gemini interaction failed"));
                    }
                    _ => return Err(protocol("unsupported interaction status")),
                }
            }
            "error" => {
                observation.terminal = Some(super::ResponseTerminal::Error);
                return Err(protocol("Gemini stream error"));
            }
            _ => return Err(protocol("unsupported Interactions event")),
        }
        // Only release complete contiguous siblings. Network completion order is
        // never tool-call order, and incomplete steps never become executable.
        while let Some(step) = self
            .steps
            .get(&self.next_emitted)
            .filter(|step| step.stopped)
        {
            let index = self.next_emitted;
            let id = format!("{}:{index}", self.id.as_deref().ok_or_else(invalid)?);
            let item = project(step.native.clone(), &id)?;
            events.push(StreamEvent::CompletedItem(
                CompletedOutputItem::from_complete_response(item, index),
            ));
            self.next_emitted += 1;
        }
        Ok(events)
    }
    pub(super) fn is_complete(&self) -> bool {
        self.terminal.is_some()
    }
    pub(super) fn finish(self) -> Result<ResponsesTurn, TransportError> {
        let usage = self
            .terminal
            .ok_or_else(|| protocol("missing terminal Gemini interaction"))?;
        let response_id = self.id.ok_or_else(invalid)?;
        let mut items = Vec::new();
        for (expected, (index, step)) in self.steps.into_iter().enumerate() {
            if index != expected as u64 {
                return Err(TransportError::IncompleteOutputIndices {
                    index: expected as u64,
                });
            }
            if !step.stopped {
                return Err(protocol("unfinished Gemini step"));
            }
            items.push(project(step.native, &format!("{response_id}:{index}"))?);
        }
        Ok(ResponsesTurn {
            response_id,
            items,
            usage,
        })
    }
}

#[cfg(test)]
mod tests;
