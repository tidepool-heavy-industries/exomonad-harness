//! Argon Interactions: retain native steps and derive the existing engine view.
//! The native step is replay authority; the projection never replaces it.
use super::{
    InferenceLimits, ResponsesRequest, ResponsesTurn, TransportError, UnsupportedSemantics, Usage,
    assembly::{AssemblyAdvance, DeclaredStop, ProtocolFault},
    sse::{CompletedOutputItem, OutputChannel, StreamEvent},
};
use crate::{item::Item, model::Effort};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

fn invalid() -> TransportError {
    TransportError::UnsupportedSemantics(UnsupportedSemantics::InputItem)
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
            let expected =
                project(step.clone(), id).map_err(TransportError::UnsupportedSemantics)?;
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
fn project(step: Value, display_id: &str) -> Result<Item, UnsupportedSemantics> {
    let kind = step["type"]
        .as_str()
        .ok_or(UnsupportedSemantics::InputItem)?;
    let mut value = match kind {
        "function_call" => json!({"type":"function_call","id":display_id,
            "call_id":step["id"].as_str().filter(|s| !s.is_empty()).ok_or(UnsupportedSemantics::InputItem)?,
            "name":step["name"].as_str().filter(|s| !s.is_empty()).ok_or(UnsupportedSemantics::InputItem)?,
            "arguments":step["arguments"].as_object().ok_or(UnsupportedSemantics::InputItem)?}),
        "thought" => {
            if step
                .get("signature")
                .is_some_and(|signature| !signature.is_string())
            {
                return Err(UnsupportedSemantics::InputItem);
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
                    .ok_or(UnsupportedSemantics::InputItem)?
                    .iter()
                    .map(|part| {
                        if part["type"] != "text" {
                            return Err(UnsupportedSemantics::InputItem);
                        }
                        let text = part["text"]
                            .as_str()
                            .ok_or(UnsupportedSemantics::InputItem)?;
                        Ok(json!({"type":"output_text","text":text}))
                    })
                    .collect::<Result<Vec<_>, UnsupportedSemantics>>()?,
            };
            json!({"type":"message","id":display_id,"role":"assistant","content":parts})
        }
        _ => json!({"type":"gemini_native","id":display_id}),
    };
    value["gemini_step"] = step;
    Ok(Item(value))
}

#[derive(Clone, Copy)]
enum InteractionStatus {
    InProgress,
    Queued,
    Completed,
    RequiresAction,
    Incomplete,
    BudgetExceeded,
    Failed,
    Cancelled,
}
impl InteractionStatus {
    fn parse(value: Option<&str>) -> Result<Self, ProtocolFault> {
        match value {
            Some("in_progress") => Ok(Self::InProgress),
            Some("queued") => Ok(Self::Queued),
            Some("completed") => Ok(Self::Completed),
            Some("requires_action") => Ok(Self::RequiresAction),
            Some("incomplete") => Ok(Self::Incomplete),
            Some("budget_exceeded") => Ok(Self::BudgetExceeded),
            Some("failed") => Ok(Self::Failed),
            Some("cancelled") => Ok(Self::Cancelled),
            _ => Err(ProtocolFault::Invalid("unsupported interaction status")),
        }
    }
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
    fn identity(&mut self, id: &str) -> Result<(), ProtocolFault> {
        if id.is_empty() || self.id.as_deref().is_some_and(|old| old != id) {
            return Err(ProtocolFault::Invalid("interaction identity changed"));
        }
        self.id = Some(id.into());
        Ok(())
    }
    pub(super) fn advance(
        &mut self,
        data: &str,
        token: &str,
        account: &str,
        observation: &mut super::TransportObservation,
    ) -> Result<AssemblyAdvance<Vec<StreamEvent>>, ProtocolFault> {
        let event: Value = serde_json::from_str(data)
            .map_err(|_| ProtocolFault::Invalid("invalid Interactions SSE JSON"))?;
        if let Some(id) = event["event_id"].as_str() {
            if let Some(previous) = self.seen_events.get(id) {
                return if previous == &event {
                    Ok(AssemblyAdvance::Progress(Vec::new()))
                } else {
                    Err(ProtocolFault::Invalid("conflicting Interactions event id"))
                };
            }
            self.seen_events.insert(id.into(), event.clone());
        }
        let kind = event["event_type"]
            .as_str()
            .ok_or_else(|| ProtocolFault::Invalid("missing Interactions event type"))?;
        observation.event(super::EventKind::from_provider(Some(kind)));
        if self.terminal.is_some() {
            return Err(ProtocolFault::Invalid("event after terminal interaction"));
        }
        let mut events = Vec::new();
        match kind {
            "interaction.created" => self.identity(
                event["interaction"]["id"]
                    .as_str()
                    .ok_or_else(ProtocolFault::invalid_input)?,
            )?,
            "step.start" => {
                let index = event["index"]
                    .as_u64()
                    .ok_or(ProtocolFault::MissingOutputIndex)?;
                let native = event
                    .get("step")
                    .filter(|s| s.is_object())
                    .ok_or_else(ProtocolFault::invalid_input)?
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
                    return Err(ProtocolFault::ConflictingOutputIndex { index });
                }
            }
            "step.delta" => {
                let index = event["index"]
                    .as_u64()
                    .ok_or(ProtocolFault::MissingOutputIndex)?;
                let step = self
                    .steps
                    .get_mut(&index)
                    .ok_or_else(|| ProtocolFault::Invalid("delta before step start"))?;
                if step.stopped {
                    return Err(ProtocolFault::Invalid("delta after step stop"));
                }
                let delta = &event["delta"];
                let mut visible = None;
                match delta["type"].as_str() {
                    Some("thought_signature") if step.native["type"] == "thought" => {
                        let signature = delta["signature"]
                            .as_str()
                            .ok_or_else(ProtocolFault::invalid_input)?;
                        // A signature delta is the complete opaque signature, not text.
                        if step.native["signature"]
                            .as_str()
                            .is_some_and(|old| !old.is_empty() && old != signature)
                        {
                            return Err(ProtocolFault::Invalid("conflicting thought signature"));
                        }
                        step.native["signature"] = signature.into();
                    }
                    Some("thought_summary") if step.native["type"] == "thought" => {
                        let content = delta
                            .get("content")
                            .ok_or_else(ProtocolFault::invalid_input)?
                            .clone();
                        if let Some(text) = content["text"].as_str() {
                            visible = Some((OutputChannel::ReasoningSummary, text.to_owned()));
                        }
                        if step.native.get("summary").is_none() {
                            step.native["summary"] = json!([]);
                        }
                        step.native["summary"]
                            .as_array_mut()
                            .ok_or_else(ProtocolFault::invalid_input)?
                            .push(content);
                    }
                    Some("arguments_delta") if step.native["type"] == "function_call" => {
                        let text = delta["arguments"]
                            .as_str()
                            .ok_or_else(ProtocolFault::invalid_input)?;
                        step.arguments
                            .get_or_insert_with(String::new)
                            .push_str(text);
                        visible = Some((OutputChannel::ToolArguments, text.to_owned()));
                    }
                    Some("text") if step.native["type"] == "model_output" => {
                        let text = delta["text"]
                            .as_str()
                            .ok_or_else(ProtocolFault::invalid_input)?;
                        if step.native.get("content").is_none() {
                            step.native["content"] = json!([]);
                        }
                        let parts = step.native["content"]
                            .as_array_mut()
                            .ok_or_else(ProtocolFault::invalid_input)?;
                        if let Some(last) = parts.last_mut().filter(|part| part["type"] == "text") {
                            let Value::String(accumulated) = &mut last["text"] else {
                                return Err(ProtocolFault::invalid_input());
                            };
                            accumulated.push_str(text);
                        } else {
                            parts.push(json!({"type":"text","text":text}));
                        }
                        visible = Some((OutputChannel::Assistant, text.to_owned()));
                    }
                    _ => {
                        return Err(ProtocolFault::Invalid(
                            "unsupported Interactions step delta",
                        ));
                    }
                }
                if let Some((channel, text)) = visible {
                    events.push(StreamEvent::Delta {
                        item_id: format!(
                            "{}:{index}",
                            self.id
                                .as_deref()
                                .ok_or_else(ProtocolFault::invalid_input)?
                        ),
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
                    .ok_or(ProtocolFault::MissingOutputIndex)?;
                let step = self
                    .steps
                    .get_mut(&index)
                    .ok_or_else(|| ProtocolFault::Invalid("stop before step start"))?;
                if step.stopped {
                    return Err(ProtocolFault::Invalid("duplicate step stop"));
                }
                if let Some(arguments) = step.arguments.take() {
                    step.native["arguments"] = serde_json::from_str(&arguments).map_err(|_| {
                        ProtocolFault::Invalid("invalid completed function arguments")
                    })?;
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
                        .ok_or_else(ProtocolFault::invalid_input)?,
                )?;
                let status = InteractionStatus::parse(interaction["status"].as_str())?;
                match status {
                    InteractionStatus::InProgress | InteractionStatus::Queued => {
                        if kind != "interaction.status_update" {
                            return Err(ProtocolFault::Invalid("unsupported interaction status"));
                        }
                    }
                    InteractionStatus::Completed | InteractionStatus::RequiresAction => {
                        if let Some(steps) = interaction
                            .get("steps")
                            .and_then(Value::as_array)
                            .filter(|v| !v.is_empty())
                        {
                            for (index, native) in steps.iter().enumerate() {
                                if let Some(prior) = self.steps.get(&(index as u64)) {
                                    if !prior.stopped || &prior.native != native {
                                        return Err(ProtocolFault::ConflictingOutputIndex {
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
                                return Err(ProtocolFault::invalid_input());
                            }
                        }
                        if self.steps.values().any(|step| !step.stopped) {
                            return Err(ProtocolFault::Invalid(
                                "interaction ended with unfinished step",
                            ));
                        }
                        if matches!(status, InteractionStatus::RequiresAction)
                            && !self
                                .steps
                                .values()
                                .any(|step| step.native["type"] == "function_call")
                        {
                            return Err(ProtocolFault::Invalid(
                                "requires_action without function call",
                            ));
                        }
                        let usage = Usage::from_interaction_usage(interaction);
                        observation.usage = Some(usage.clone());
                        observation.terminal = Some(super::ResponseTerminal::Completed);
                        self.terminal = Some(usage);
                    }
                    InteractionStatus::Incomplete | InteractionStatus::BudgetExceeded => {
                        let reason = matches!(status, InteractionStatus::BudgetExceeded)
                            .then_some(super::IncompleteReason::BudgetExceeded);
                        let (response_id, truncated) = super::observation::metadata(
                            self.id.as_deref().unwrap(),
                            token,
                            account,
                        );
                        observation.metadata_truncated |= truncated;
                        return Ok(AssemblyAdvance::Stopped(DeclaredStop::Incomplete {
                            response_id,
                            reason,
                        }));
                    }
                    InteractionStatus::Failed | InteractionStatus::Cancelled => {
                        let event = match status {
                            InteractionStatus::Failed => {
                                super::ProviderStreamFailureEvent::InteractionFailed
                            }
                            InteractionStatus::Cancelled => {
                                super::ProviderStreamFailureEvent::InteractionCancelled
                            }
                            _ => unreachable!("failed or cancelled status"),
                        };
                        return Ok(AssemblyAdvance::Stopped(DeclaredStop::Failed {
                            event,
                            diagnostic: None,
                        }));
                    }
                }
            }
            "error" => {
                return Ok(AssemblyAdvance::Stopped(DeclaredStop::Failed {
                    event: super::ProviderStreamFailureEvent::Error,
                    diagnostic: None,
                }));
            }
            _ => return Err(ProtocolFault::Invalid("unsupported Interactions event")),
        }
        // Only release complete contiguous siblings. Network completion order is
        // never tool-call order, and incomplete steps never become executable.
        while let Some(step) = self
            .steps
            .get(&self.next_emitted)
            .filter(|step| step.stopped)
        {
            let index = self.next_emitted;
            let id = format!(
                "{}:{index}",
                self.id
                    .as_deref()
                    .ok_or_else(ProtocolFault::invalid_input)?
            );
            let item = project(step.native.clone(), &id).map_err(ProtocolFault::Unsupported)?;
            events.push(StreamEvent::CompletedItem(
                CompletedOutputItem::from_complete_response(item, index),
            ));
            self.next_emitted += 1;
        }
        Ok(if self.terminal.is_some() {
            AssemblyAdvance::Complete(events)
        } else {
            AssemblyAdvance::Progress(events)
        })
    }
    pub(super) fn finish_complete(self) -> Result<ResponsesTurn, ProtocolFault> {
        let usage = self
            .terminal
            .ok_or(ProtocolFault::Invalid("missing native completion"))?;
        let response_id = self.id.ok_or_else(ProtocolFault::invalid_input)?;
        let mut items = Vec::new();
        for (expected, (index, step)) in self.steps.into_iter().enumerate() {
            if index != expected as u64 {
                return Err(ProtocolFault::IncompleteOutputIndices {
                    index: expected as u64,
                });
            }
            if !step.stopped {
                return Err(ProtocolFault::Invalid("unfinished Gemini step"));
            }
            items.push(
                project(step.native, &format!("{response_id}:{index}"))
                    .map_err(ProtocolFault::Unsupported)?,
            );
        }
        Ok(ResponsesTurn {
            response_id,
            items,
            usage,
        })
    }
}

#[cfg(test)]
mod terminal_tests;
#[cfg(test)]
mod tests;
