use super::{
    ProviderStreamFailureEvent, ResponsesTurn, TransportError, Usage,
    assembly::{AssemblyAdvance, DeclaredStop, ProtocolFault},
};
use crate::item::Item;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A finalized item carrying its provider-issued output position. Only the
/// transport assembly constructs this evidence; deltas cannot become items.
#[derive(Clone, Debug)]
pub struct CompletedOutputItem {
    item: Item,
    output_index: u64,
    sequence_number: Option<u64>,
}
impl CompletedOutputItem {
    pub fn item(&self) -> &Item {
        &self.item
    }
    pub fn output_index(&self) -> u64 {
        self.output_index
    }
    pub fn sequence_number(&self) -> Option<u64> {
        self.sequence_number
    }
    pub(crate) fn from_complete_response(item: Item, output_index: u64) -> Self {
        Self {
            item,
            output_index,
            sequence_number: None,
        }
    }
}

/// Completed output items are carried by SSE `response.output_item.done`;
/// this backend may leave `response.completed.response.output` empty.
#[derive(Default)]
pub struct ResponseAssembly {
    items: BTreeMap<u64, Item>,
    // Display metadata only; partial input never becomes a completed item.
    tool_names: BTreeMap<String, String>,
    completed: Option<(String, Usage)>,
    response_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OutputChannel {
    Assistant,
    ReasoningSummary,
    Reasoning,
    ToolArguments,
    ToolInput,
    Refusal,
}

#[derive(Debug)]
pub enum StreamEvent {
    CompletedItem(CompletedOutputItem),
    Delta {
        item_id: String,
        channel: OutputChannel,
        index: u64,
        text: String,
        tool_name: Option<String>,
    },
}

impl ResponseAssembly {
    /// Returns a live event for the loop, while retaining completed items for
    /// the final turn. The caller is responsible for SSE framing.
    pub fn accept(&mut self, data: &str) -> Result<Option<StreamEvent>, TransportError> {
        self.accept_observed(data, "", "", None)
    }

    pub(super) fn accept_observed(
        &mut self,
        data: &str,
        token: &str,
        account: &str,
        mut observation: Option<&mut super::TransportObservation>,
    ) -> Result<Option<StreamEvent>, TransportError> {
        self.advance(data, token, account, observation.as_deref_mut())
            .map_err(ProtocolFault::into_error)?
            .into_events(observation)
            .map(|events| events.into_iter().next())
    }

    pub(super) fn advance(
        &mut self,
        data: &str,
        token: &str,
        account: &str,
        mut observation: Option<&mut super::TransportObservation>,
    ) -> Result<AssemblyAdvance, ProtocolFault> {
        let event: Value =
            serde_json::from_str(data).map_err(|_| ProtocolFault::Invalid("invalid SSE JSON"))?;
        if let Some(observation) = observation.as_deref_mut() {
            observation.event(super::EventKind::from_provider(
                event.get("type").and_then(Value::as_str),
            ));
        }
        if matches!(
            event.get("type").and_then(Value::as_str),
            Some("response.completed" | "response.incomplete" | "response.failed")
        ) {
            if let (Some(observation), Some(response)) =
                (observation.as_deref_mut(), event.get("response"))
            {
                observation.observe_response_metadata(response, token, account);
            }
        }
        match event.get("type").and_then(Value::as_str) {
            Some("response.created") => {
                let id = event
                    .get("response")
                    .and_then(|response| response.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| ProtocolFault::Invalid("created response missing id"))?;
                if self.response_id.as_deref().is_some_and(|prior| prior != id) {
                    return Err(ProtocolFault::Invalid("response identity changed"));
                }
                self.response_id = Some(id.to_owned());
                if let Some(observation) = observation.as_deref_mut() {
                    let (id, truncated) = super::observation::metadata(id, token, account);
                    observation.response_created_id = Some(id);
                    observation.metadata_truncated |= truncated;
                }
                Ok(AssemblyAdvance::Progress(Vec::new()))
            }
            Some("response.output_item.added") => {
                let item = &event["item"];
                if matches!(
                    item["type"].as_str(),
                    Some("function_call" | "custom_tool_call")
                ) {
                    if let Some(name) = item["name"].as_str() {
                        for id in [item["id"].as_str(), item["call_id"].as_str()]
                            .into_iter()
                            .flatten()
                        {
                            self.tool_names.insert(id.to_owned(), name.to_owned());
                        }
                    }
                }
                Ok(AssemblyAdvance::Progress(Vec::new()))
            }
            Some("response.output_item.done") => {
                let item = event
                    .get("item")
                    .cloned()
                    .ok_or_else(|| ProtocolFault::Invalid("missing done item"))?;
                let output_index = event
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .ok_or(ProtocolFault::MissingOutputIndex)?;
                let done = CompletedOutputItem {
                    item: Item(item),
                    output_index,
                    sequence_number: event.get("sequence_number").and_then(Value::as_u64),
                };
                if let Some(previous) = self.items.get(&output_index) {
                    if previous != done.item() {
                        return Err(ProtocolFault::ConflictingOutputIndex {
                            index: output_index,
                        });
                    }
                } else {
                    self.items.insert(output_index, done.item.clone());
                }
                Ok(AssemblyAdvance::Progress(vec![StreamEvent::CompletedItem(
                    done,
                )]))
            }
            Some(
                kind @ ("response.output_text.delta"
                | "response.reasoning_summary_text.delta"
                | "response.function_call_arguments.delta"
                | "response.custom_tool_call_input.delta"
                | "response.refusal.delta"
                | "response.reasoning_text.delta"),
            ) => {
                let item_id = event
                    .get("item_id")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        if kind == "response.custom_tool_call_input.delta" {
                            event.get("call_id").and_then(Value::as_str)
                        } else {
                            None
                        }
                    })
                    .ok_or_else(|| ProtocolFault::Invalid("delta missing item_id"))?;
                Ok(AssemblyAdvance::Progress(
                    event
                        .get("delta")
                        .and_then(Value::as_str)
                        .map(|text| StreamEvent::Delta {
                            item_id: item_id.to_owned(),
                            channel: match kind {
                                "response.output_text.delta" => OutputChannel::Assistant,
                                "response.reasoning_summary_text.delta" => {
                                    OutputChannel::ReasoningSummary
                                }
                                "response.reasoning_text.delta" => OutputChannel::Reasoning,
                                "response.function_call_arguments.delta" => {
                                    OutputChannel::ToolArguments
                                }
                                "response.custom_tool_call_input.delta" => OutputChannel::ToolInput,
                                "response.refusal.delta" => OutputChannel::Refusal,
                                _ => unreachable!("matched channel"),
                            },
                            index: event
                                .get("content_index")
                                .or_else(|| event.get("summary_index"))
                                .and_then(Value::as_u64)
                                .unwrap_or(0),
                            text: text.to_owned(),
                            tool_name: matches!(
                                kind,
                                "response.function_call_arguments.delta"
                                    | "response.custom_tool_call_input.delta"
                            )
                            .then(|| self.tool_names.get(item_id).cloned())
                            .flatten(),
                        })
                        .into_iter()
                        .collect(),
                ))
            }
            Some("response.completed") => {
                let response = event
                    .get("response")
                    .ok_or_else(|| ProtocolFault::Invalid("missing response"))?;
                let id = response
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ProtocolFault::Invalid("missing response id"))?;
                if self.response_id.as_deref().is_some_and(|prior| prior != id) {
                    return Err(ProtocolFault::Invalid("response identity changed"));
                }
                // A nonempty terminal output array carries authoritative output
                // positions, including items whose streamed events were absent.
                // Completed output items remain authoritative when this array is empty.
                if let Some(output) = response
                    .get("output")
                    .and_then(Value::as_array)
                    .filter(|output| !output.is_empty())
                {
                    for (index, item) in output.iter().enumerate() {
                        let item = Item(item.clone());
                        if self
                            .items
                            .get(&(index as u64))
                            .is_some_and(|prior| prior != &item)
                        {
                            return Err(ProtocolFault::ConflictingOutputIndex {
                                index: index as u64,
                            });
                        }
                        self.items.insert(index as u64, item);
                    }
                    if self.items.len() != output.len() {
                        return Err(ProtocolFault::IncompleteOutputIndices {
                            index: output.len() as u64,
                        });
                    }
                }
                let usage = response.get("usage").unwrap_or(&Value::Null);
                if let Some(observation) = observation.as_deref_mut() {
                    observation.terminal = Some(super::ResponseTerminal::Completed);
                }
                let details = usage.get("input_tokens_details").unwrap_or(&Value::Null);
                let output_details = usage.get("output_tokens_details").unwrap_or(&Value::Null);
                // Opt-in diagnostics expose only provider-returned mode and counters.
                // Missing fields remain unavailable; requested reasoning settings
                // do not establish which context mode the provider actually used.
                tracing::debug!(
                    target: "harness::transport::completion",
                    reasoning_context = match response["reasoning"]["context"].as_str() {
                        Some("all_turns") => "all_turns",
                        Some("current_turn") => "current_turn",
                        Some(_) => "unrecognized",
                        None => "unavailable",
                    },
                    model = ?response.get("model"),
                    service_tier = ?response.get("service_tier"),
                    prompt_cache_diagnostics = ?response.get("prompt_cache_diagnostics"),
                    reasoning_tokens = ?output_details["reasoning_tokens"].as_u64(),
                    accepted_prediction_tokens = ?output_details["accepted_prediction_tokens"].as_u64(),
                    rejected_prediction_tokens = ?output_details["rejected_prediction_tokens"].as_u64(),
                    cached_tokens = ?details["cached_tokens"].as_u64(),
                    cache_write_tokens = ?details["cache_write_tokens"].as_u64(),
                    "provider completion diagnostics"
                );
                self.completed = Some((id.to_owned(), Usage::from_response_usage(response)));
                Ok(AssemblyAdvance::Complete(Vec::new()))
            }
            Some("response.incomplete") => {
                let response = event
                    .get("response")
                    .ok_or_else(|| ProtocolFault::Invalid("missing response"))?;
                let id = response
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ProtocolFault::Invalid("missing response id"))?;
                if self.response_id.as_deref().is_some_and(|prior| prior != id) {
                    return Err(ProtocolFault::Invalid("response identity changed"));
                }
                let reason = response
                    .get("incomplete_details")
                    .and_then(|details| details.get("reason"))
                    .and_then(Value::as_str)
                    .map(super::IncompleteReason::from_provider);
                let (response_id, truncated) = super::observation::metadata(id, token, account);
                if let Some(observation) = observation.as_deref_mut() {
                    observation.metadata_truncated |= truncated;
                }
                Ok(AssemblyAdvance::Stopped(DeclaredStop::Incomplete {
                    response_id,
                    reason,
                }))
            }
            Some(kind @ ("response.failed" | "error")) => {
                let response_failed = kind == "response.failed";
                let failure = if response_failed {
                    ProviderStreamFailureEvent::ResponseFailed
                } else {
                    ProviderStreamFailureEvent::Error
                };
                let nested_error = if response_failed {
                    event
                        .get("response")
                        .and_then(|response| response.get("error"))
                } else {
                    event.get("error").filter(|error| error.is_object())
                };
                let field = |name: &str| {
                    nested_error
                        .and_then(|error| error.get(name)?.as_str())
                        .or_else(|| {
                            if response_failed {
                                None
                            } else {
                                event.get(name)?.as_str()
                            }
                        })
                };
                Ok(AssemblyAdvance::Stopped(DeclaredStop::Failed {
                    event: failure,
                    diagnostic: super::http_error::diagnostic(
                        field("code"),
                        nested_error
                            .and_then(|error| error.get("type")?.as_str())
                            .or_else(|| field("error_type")),
                        field("param"),
                        field("message"),
                        token,
                        account,
                    ),
                }))
            }
            _ => Ok(AssemblyAdvance::Progress(Vec::new())),
        }
    }

    pub fn finish(mut self) -> Result<ResponsesTurn, TransportError> {
        let (response_id, usage) =
            self.completed
                .take()
                .ok_or(TransportError::IncompleteResponse(
                    super::StreamInterruption::MissingCompletion,
                ))?;
        self.finish_items(response_id, usage)
            .map_err(ProtocolFault::into_error)
    }

    pub(super) fn finish_complete(mut self) -> Result<ResponsesTurn, ProtocolFault> {
        let (response_id, usage) = self
            .completed
            .take()
            .ok_or(ProtocolFault::Invalid("missing native completion"))?;
        self.finish_items(response_id, usage)
    }

    fn finish_items(
        self,
        response_id: String,
        usage: Usage,
    ) -> Result<ResponsesTurn, ProtocolFault> {
        let mut items = Vec::with_capacity(self.items.len());
        for (expected, (index, item)) in self.items.into_iter().enumerate() {
            if index != expected as u64 {
                return Err(ProtocolFault::IncompleteOutputIndices {
                    index: expected as u64,
                });
            }
            items.push(item);
        }
        Ok(ResponsesTurn {
            response_id,
            items,
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn incomplete_terminal_retains_typed_reason_and_refuses_changed_identity() {
        for (reason, expected) in [
            (
                json!("max_output_tokens"),
                Some(super::super::IncompleteReason::MaxOutputTokens),
            ),
            (
                json!("content_filter"),
                Some(super::super::IncompleteReason::ContentFilter),
            ),
            (
                json!("steered"),
                Some(super::super::IncompleteReason::Steered),
            ),
            (
                json!("private unknown reason"),
                Some(super::super::IncompleteReason::Unknown),
            ),
            (Value::Null, None),
        ] {
            let mut assembly = ResponseAssembly::default();
            assembly
                .accept(r#"{"type":"response.created","response":{"id":"original"}}"#)
                .unwrap();
            let error = assembly.accept(&json!({"type":"response.incomplete","response":{"id":"original","incomplete_details":{"reason":reason}}}).to_string()).unwrap_err();
            assert!(
                matches!(error, TransportError::ResponseIncomplete { response_id, reason } if response_id == "original" && reason == expected)
            );
            assert!(assembly.completed.is_none());
        }
        let mut assembly = ResponseAssembly::default();
        assembly
            .accept(r#"{"type":"response.created","response":{"id":"original"}}"#)
            .unwrap();
        assert!(matches!(
            assembly.accept(r#"{"type":"response.incomplete","response":{"id":"changed"}}"#),
            Err(TransportError::Stream(_))
        ));
    }

    #[test]
    fn incomplete_diagnostics_bound_utf8_bytes_and_exclude_provider_content() {
        let token = "active-request-secret";
        let account = "active-account-secret";
        let id = format!("{token} {account} {}", "λ".repeat(200));
        let packet = json!({"type":"response.incomplete", "response":{
            "id":id, "incomplete_details":{"reason":"private unknown reason"},
            "encrypted_content":"private reasoning", "output":"private body",
        }});
        let mut summary =
            super::super::TransportObservation::new(super::super::ResponseEnd::ProviderTerminal);
        let error = ResponseAssembly::default()
            .accept_observed(&packet.to_string(), token, account, Some(&mut summary))
            .unwrap_err();
        let TransportError::ResponseIncomplete {
            response_id,
            reason,
        } = error
        else {
            panic!("wrong terminal");
        };
        assert_eq!(reason, Some(super::super::IncompleteReason::Unknown));
        assert!(response_id.len() <= 128);
        assert!(summary.metadata_truncated);
        let encoded = serde_json::to_string(&summary).unwrap();
        for excluded in [
            token,
            account,
            "private unknown reason",
            "private reasoning",
            "private body",
            "encrypted_content",
        ] {
            assert!(!encoded.contains(excluded));
        }
    }

    #[test]
    fn indexed_items_require_positions_and_conflicting_duplicates_refuse() {
        let mut assembly = ResponseAssembly::default();
        assert!(matches!(
            assembly.accept(
                r#"{"type":"response.output_item.done","item":{"type":"reasoning","id":"r"}}"#
            ),
            Err(TransportError::MissingOutputIndex)
        ));
        let done = r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"r"}}"#;
        assert!(matches!(
            assembly.accept(done),
            Ok(Some(StreamEvent::CompletedItem(_)))
        ));
        assert!(matches!(
            assembly.accept(done),
            Ok(Some(StreamEvent::CompletedItem(_)))
        ));
        assert!(matches!(
            assembly.accept(r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"other"}}"#),
            Err(TransportError::ConflictingOutputIndex { index: 0 })
        ));
    }

    #[test]
    fn terminal_output_reconciles_observed_gaps_without_inventing_items() {
        let mut assembly = ResponseAssembly::default();
        assembly.accept(r#"{"type":"response.output_item.done","output_index":1,"item":{"type":"reasoning","id":"second"}}"#).unwrap();
        assembly.accept(r#"{"type":"response.completed","response":{"id":"complete","output":[{"type":"reasoning","id":"first"},{"type":"reasoning","id":"second"}]}}"#).unwrap();
        let turn = assembly.finish().unwrap();
        assert_eq!(
            turn.items
                .iter()
                .map(|item| item.0["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );

        let mut gap = ResponseAssembly::default();
        gap.accept(r#"{"type":"response.output_item.done","output_index":1,"item":{"type":"reasoning","id":"second"}}"#).unwrap();
        gap.accept(r#"{"type":"response.completed","response":{"id":"complete","output":[]}}"#)
            .unwrap();
        assert!(matches!(
            gap.finish(),
            Err(TransportError::IncompleteOutputIndices { index: 0 })
        ));
    }

    #[test]
    fn terminal_output_conflicts_and_changed_response_identity_refuse() {
        let mut assembly = ResponseAssembly::default();
        assembly.accept(r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"observed"}}"#).unwrap();
        assert!(matches!(
            assembly.accept(r#"{"type":"response.completed","response":{"id":"complete","output":[{"type":"reasoning","id":"changed"}]}}"#),
            Err(TransportError::ConflictingOutputIndex { index: 0 })
        ));
        let mut identity = ResponseAssembly::default();
        identity
            .accept(r#"{"type":"response.created","response":{"id":"original"}}"#)
            .unwrap();
        assert!(
            identity
                .accept(r#"{"type":"response.completed","response":{"id":"changed","output":[]}}"#)
                .is_err()
        );
    }

    #[test]
    fn item_done_supplies_phase_when_completed_output_is_empty() {
        let mut a = ResponseAssembly::default();
        let done = r#"{"type":"response.output_item.done","output_index":0,"item":{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"ok"}]}}"#;
        assert!(matches!(
            a.accept(done),
            Ok(Some(StreamEvent::CompletedItem(_)))
        ));
        let completed = r#"{"type":"response.completed","response":{"id":"resp_1","output":[],"usage":{"input_tokens":12,"output_tokens":2,"input_tokens_details":{"cached_tokens":3,"cache_write_tokens":4}}}}"#;
        assert!(matches!(a.accept(completed), Ok(None)));
        let turn = a.finish().expect("complete");
        assert_eq!(turn.items.len(), 1);
        assert_eq!(turn.items[0].0["phase"], "final_answer");
        assert_eq!(turn.response_id, "resp_1");
        assert_eq!(turn.usage.input_tokens, Some(12));
        assert_eq!(turn.usage.output_tokens, Some(2));
        assert_eq!(turn.usage.cached_tokens, Some(3));
        assert_eq!(turn.usage.cache_write_tokens, Some(4));
    }

    #[test]
    fn usage_presence_distinguishes_reported_zero_from_missing_or_partial_counters() {
        for (usage, reported) in [
            (json!({"input_tokens":0,"output_tokens":0}), true),
            (json!({"input_tokens":0}), false),
            (json!({"input_tokens":"0","output_tokens":0}), false),
            (Value::Null, false),
        ] {
            let mut assembly = ResponseAssembly::default();
            assembly.accept(&json!({"type":"response.completed","response":{"id":"usage-proof","output":[],"usage":usage}}).to_string()).unwrap();
            let turn = assembly.finish().unwrap();
            assert_eq!(turn.usage.has_reported_totals(), reported);
            assert_eq!(
                turn.usage.input_tokens,
                usage.get("input_tokens").and_then(Value::as_u64)
            );
            assert_eq!(
                turn.usage.output_tokens,
                usage.get("output_tokens").and_then(Value::as_u64)
            );
        }
    }

    #[test]
    fn terminal_usage_and_model_observations_survive_refusal_without_completion() {
        for kind in [
            "response.completed",
            "response.incomplete",
            "response.failed",
        ] {
            for raw in [
                None,
                Some(Value::Null),
                Some(json!({"input_tokens":0,"future":null})),
            ] {
                let mut response = json!({"id":"observed-terminal", "model":"deepseek-flash",
                    "reasoning":{"effort":"high"},"max_output_tokens":2048,
                    "output":[], "incomplete_details":{"reason":"max_output_tokens"},
                    "error":{"code":"provider_failure","message":"failed"}});
                if let Some(raw) = &raw {
                    response["usage"] = raw.clone();
                }
                let mut observation = super::super::TransportObservation::new(
                    super::super::ResponseEnd::ProviderTerminal,
                );
                let mut assembly = ResponseAssembly::default();
                let result = assembly.accept_observed(
                    &json!({"type":kind,"response":response}).to_string(),
                    "",
                    "",
                    Some(&mut observation),
                );
                assert_eq!(result.is_ok(), kind == "response.completed");
                let usage = observation.usage.as_ref().unwrap();
                assert_eq!(
                    usage.raw,
                    raw.map_or(
                        super::super::usage::RawUsage::Unavailable,
                        super::super::usage::RawUsage::Observed
                    )
                );
                assert_eq!(
                    observation.returned_model.as_deref(),
                    Some("deepseek-flash")
                );
                assert_eq!(
                    observation.returned_reasoning_effort.as_deref(),
                    Some("high")
                );
                assert_eq!(observation.reported_max_output_tokens, Some(2048));
                let restored: super::super::TransportObservation =
                    serde_json::from_value(serde_json::to_value(&observation).unwrap()).unwrap();
                assert_eq!(restored, observation);
                if kind != "response.completed" {
                    assert!(assembly.finish().is_err());
                }
            }
        }
    }

    #[test]
    fn cache_counter_presence_survives_provider_assembly_independently() {
        for (details, cached, written) in [
            (Value::Null, None, None),
            (json!({}), None, None),
            (json!({"cached_tokens":0}), Some(0), None),
            (json!({"cache_write_tokens":0}), None, Some(0)),
            (
                json!({"cached_tokens":17,"cache_write_tokens":3}),
                Some(17),
                Some(3),
            ),
            (
                json!({"cached_tokens":"0","cache_write_tokens":-1}),
                None,
                None,
            ),
        ] {
            let mut assembly = ResponseAssembly::default();
            assembly
                .accept(
                    &json!({"type":"response.completed","response":{
                        "id":"cache-observation","output":[],"usage":{
                            "input_tokens":20,"output_tokens":2,"input_tokens_details":details
                        }
                    }})
                    .to_string(),
                )
                .unwrap();
            let usage = assembly.finish().unwrap().usage;
            assert!(usage.has_reported_totals());
            assert_eq!(usage.cached_tokens, cached);
            assert_eq!(usage.cache_write_tokens, written);
            assert_eq!(
                serde_json::from_value::<Usage>(serde_json::to_value(&usage).unwrap()).unwrap(),
                usage
            );
        }
    }

    #[test]
    fn quota_failure_type_aliases_preserve_the_provider_failure_boundary() {
        let code = "subscription_sharing_usage_limit_exceeded";
        for (packet, expected_type) in [
            (
                json!({"type":"error","code":code,"error_type":"invalid_request_error"}),
                Some("invalid_request_error"),
            ),
            (
                json!({"type":"error","error":{"code":code,"error_type":"invalid_request_error"}}),
                Some("invalid_request_error"),
            ),
            (
                json!({"type":"error","error":{"code":code,"type":"invalid_request_error","error_type":"alias"}}),
                Some("invalid_request_error"),
            ),
            (
                json!({"type":"response.failed","response":{"error":{"code":code,"error_type":"invalid_request_error"}}}),
                Some("invalid_request_error"),
            ),
            (json!({"type":"error","code":code}), None),
        ] {
            let error = ResponseAssembly::default()
                .accept(&packet.to_string())
                .unwrap_err();
            let TransportError::ProviderStreamFailure {
                diagnostic: Some(diagnostic),
                ..
            } = &error
            else {
                panic!("quota event lost its provider failure boundary: {error:?}");
            };
            assert_eq!(diagnostic.code.as_deref(), Some(code));
            assert_eq!(diagnostic.error_type.as_deref(), expected_type);
            assert!(
                error.request_failure().is_none(),
                "quota does not prove pre-response rejection"
            );
        }
    }

    #[test]
    fn live_tool_names_come_from_added_items_without_completing_partial_input() {
        let mut assembly = ResponseAssembly::default();
        for (kind, channel) in [
            ("custom_tool_call", "response.custom_tool_call_input.delta"),
            ("function_call", "response.function_call_arguments.delta"),
        ] {
            assembly.accept(&serde_json::json!({"type":"response.output_item.added","item":{"type":kind,"id":"item","call_id":"call","name":"haskell"}}).to_string()).unwrap();
            for id in ["item", "call"] {
                let event = serde_json::json!({"type":channel,"item_id":id,"delta":"let x = \"unfinished λ"});
                let Some(StreamEvent::Delta {
                    text, tool_name, ..
                }) = assembly.accept(&event.to_string()).unwrap()
                else {
                    panic!("expected preview")
                };
                assert_eq!(tool_name.as_deref(), Some("haskell"));
                assert_eq!(text, "let x = \"unfinished λ");
            }
            let Some(StreamEvent::Delta { tool_name, .. }) = assembly
                .accept(
                    &serde_json::json!({"type":channel,"item_id":"unknown","delta":"x"})
                        .to_string(),
                )
                .unwrap()
            else {
                panic!("expected preview")
            };
            assert_eq!(tool_name, None);
        }
        assert!(
            assembly.items.is_empty(),
            "display metadata cannot manufacture completed items"
        );
        let Some(StreamEvent::Delta { tool_name, .. }) = assembly
            .accept(r#"{"type":"response.output_text.delta","item_id":"item","delta":"prose"}"#)
            .unwrap()
        else {
            panic!("expected preview")
        };
        assert_eq!(tool_name, None);
    }

    #[test]
    fn live_output_deltas_keep_item_channel_and_block_identity() {
        let mut assembly = ResponseAssembly::default();
        for (channel, expected) in [
            ("response.output_text.delta", OutputChannel::Assistant),
            (
                "response.reasoning_summary_text.delta",
                OutputChannel::ReasoningSummary,
            ),
            ("response.reasoning_text.delta", OutputChannel::Reasoning),
            (
                "response.function_call_arguments.delta",
                OutputChannel::ToolArguments,
            ),
            (
                "response.custom_tool_call_input.delta",
                OutputChannel::ToolInput,
            ),
            ("response.refusal.delta", OutputChannel::Refusal),
        ] {
            let data = serde_json::json!({"type":channel,"item_id":"message-2","content_index":3,"delta":"λ"});
            match assembly.accept(&data.to_string()).unwrap().unwrap() {
                StreamEvent::Delta {
                    item_id,
                    channel: actual,
                    index,
                    text,
                    tool_name,
                } => {
                    assert_eq!(tool_name, None);
                    assert_eq!(item_id, "message-2");
                    assert_eq!(actual, expected);
                    assert_eq!(index, 3);
                    assert_eq!(text, "λ");
                }
                _ => panic!("expected delta"),
            }
        }
        assert!(
            assembly
                .accept(r#"{"type":"response.output_text.delta","delta":"unscoped"}"#)
                .is_err()
        );
    }

    #[test]
    fn provider_failures_preserve_allowlisted_fields_and_stay_non_rejections() {
        for (packet, expected) in [
            (
                serde_json::json!({"type":"response.failed", "response":{
                    "id":"response-unretained", "error":{"code":"invalid_tools", "message":"unsupported tool schema"},
                    "input":[{"content":"private prompt must not be retained"}],
                    "output":[{"content":"private output must not be retained"}],
                }}),
                ProviderStreamFailureEvent::ResponseFailed,
            ),
            (
                serde_json::json!({"type":"error", "code":"invalid_tools", "message":"unsupported tool schema", "param":"tools[0].parameters",
                    "request":{"input":"private prompt must not be retained"}, "unknown":"unretained metadata",
                }),
                ProviderStreamFailureEvent::Error,
            ),
        ] {
            let mut assembly = ResponseAssembly::default();
            let failure = assembly.accept(&packet.to_string()).unwrap_err();
            let TransportError::ProviderStreamFailure { event, diagnostic } = &failure else {
                panic!("provider failure was confused with a local stream error");
            };
            assert_eq!(*event, expected);
            let diagnostic = diagnostic.as_ref().unwrap();
            assert_eq!(diagnostic.code.as_deref(), Some("invalid_tools"));
            assert_eq!(
                diagnostic.message.as_deref(),
                Some("unsupported tool schema")
            );
            assert_eq!(diagnostic.error_type, None);
            if expected == ProviderStreamFailureEvent::Error {
                assert_eq!(diagnostic.param.as_deref(), Some("tools[0].parameters"));
            }
            let encoded = serde_json::to_string(diagnostic).unwrap();
            assert_eq!(
                serde_json::from_str::<super::super::HttpDiagnostic>(&encoded).unwrap(),
                *diagnostic
            );
            let rendered = failure.to_string();
            assert!(rendered.contains("invalid_tools"));
            assert!(rendered.contains("unsupported tool schema"));
            for private in [
                "private prompt",
                "private output",
                "response-unretained",
                "unretained metadata",
            ] {
                assert!(!rendered.contains(private));
            }
            assert!(
                failure.request_failure().is_none(),
                "provider failure is not pre-response rejection proof"
            );
            assert!(matches!(
                assembly.finish(),
                Err(TransportError::IncompleteResponse(_))
            ));
        }
    }

    #[test]
    fn nested_provider_errors_preserve_the_selected_diagnostic_envelope() {
        // The public error event can carry the same error object as HTTP
        // failures. Do not mistake the outer event type for an error class.
        for error in [
            serde_json::json!({"type":"subscription_sharing_error", "code":"subscription_sharing_unsupported_capability", "param":"tools[0]", "message":"Unsupported capability"}),
            serde_json::json!({"type":"invalid_request_error", "code":"model_not_found", "param":"model", "message":"Model unavailable"}),
        ] {
            let packet = serde_json::json!({"type":"error", "error":error,
                "code":"outer-unselected", "message":"outer-unselected",
                "request":{"input":"private prompt"}, "unknown":"private metadata"});
            let failure = ResponseAssembly::default()
                .accept(&packet.to_string())
                .unwrap_err();
            let TransportError::ProviderStreamFailure {
                event: ProviderStreamFailureEvent::Error,
                diagnostic: Some(ref diagnostic),
            } = failure
            else {
                panic!("nested provider diagnostic was discarded");
            };
            assert_eq!(diagnostic.code.as_deref(), error["code"].as_str());
            assert_eq!(diagnostic.error_type.as_deref(), error["type"].as_str());
            assert_eq!(diagnostic.param.as_deref(), error["param"].as_str());
            assert_eq!(diagnostic.message.as_deref(), error["message"].as_str());
            assert!(failure.request_failure().is_none());
            let rendered = failure.to_string();
            for excluded in ["outer-unselected", "private prompt", "private metadata"] {
                assert!(!rendered.contains(excluded));
            }
        }
        // Missing or malformed nested fields do not hide documented flat fields.
        for nested in [
            serde_json::Value::Null,
            serde_json::json!(42),
            serde_json::json!(["private"]),
            serde_json::json!({}),
            serde_json::json!({"message":42,"code":null}),
        ] {
            let packet =
                serde_json::json!({"type":"error", "error":nested, "message":"Flat diagnostic"});
            let TransportError::ProviderStreamFailure {
                diagnostic: Some(diagnostic),
                ..
            } = ResponseAssembly::default()
                .accept(&packet.to_string())
                .unwrap_err()
            else {
                panic!("flat diagnostic was discarded");
            };
            assert_eq!(diagnostic.message.as_deref(), Some("Flat diagnostic"));
            assert_eq!(diagnostic.error_type, None);
        }
    }
    #[test]
    fn partial_nested_error_fields_fall_back_to_valid_flat_fields() {
        let packet = serde_json::json!({"type":"error", "code":"unselected-outer-code", "message":"Useful message", "param":"model",
            "error":{"code":"specific-inner-code", "message":null, "param":42}});
        let TransportError::ProviderStreamFailure {
            diagnostic: Some(diagnostic),
            ..
        } = ResponseAssembly::default()
            .accept(&packet.to_string())
            .unwrap_err()
        else {
            panic!("partial nested provider diagnostic was discarded");
        };
        assert_eq!(diagnostic.code.as_deref(), Some("specific-inner-code"));
        assert_eq!(diagnostic.message.as_deref(), Some("Useful message"));
        assert_eq!(diagnostic.param.as_deref(), Some("model"));
        assert_eq!(diagnostic.error_type, None);
    }

    #[test]
    fn provider_failure_diagnostics_share_bounds_and_credential_redaction() {
        let token = "active-request-secret";
        let account = "active-account-secret";
        let message = format!(
            "{token} {account} Bearer other-secret sk-other acct_other {}{token}",
            "λ".repeat(2048)
        );
        for packet in [
            serde_json::json!({"type":"response.failed", "response":{"error":{
                "code":format!("{}{}", "λ".repeat(255), token), "param":account, "message":message,
            }, "echoed_request":{"credentials":token}}}),
            serde_json::json!({"type":"error", "code":format!("{}{}", "λ".repeat(255), token), "param":account, "message":message, "authorization":token}),
            serde_json::json!({"type":"error", "error":{"type":token,"code":format!("{}{}", "λ".repeat(255), token), "param":account, "message":message}, "authorization":token}),
        ] {
            let failure = ResponseAssembly::default()
                .accept_observed(&packet.to_string(), token, account, None)
                .unwrap_err();
            let TransportError::ProviderStreamFailure {
                diagnostic: Some(ref diagnostic),
                ..
            } = failure
            else {
                panic!("missing provider diagnostic");
            };
            assert_eq!(diagnostic.code.as_ref().unwrap().chars().count(), 256);
            assert_eq!(diagnostic.message.as_ref().unwrap().chars().count(), 2048);
            assert_eq!(diagnostic.param.as_deref(), Some("[redacted]"));
            let rendered = failure.to_string();
            for private in [
                token,
                account,
                "other-secret",
                "sk-other",
                "acct_other",
                "echoed_request",
                "authorization",
            ] {
                assert!(!rendered.contains(private), "private field was retained");
            }
            // Redaction precedes the code limit, so no active-secret prefix survives.
            assert!(diagnostic.code.as_ref().unwrap().ends_with('['));
        }
    }

    #[test]
    fn absent_provider_fields_remain_unknown_and_local_schema_errors_stay_distinct() {
        for packet in [
            r#"{"type":"response.failed","response":{"error":null}}"#,
            r#"{"type":"error","code":42,"message":{},"param":[]}"#,
        ] {
            assert!(matches!(
                ResponseAssembly::default().accept(packet),
                Err(TransportError::ProviderStreamFailure {
                    diagnostic: None,
                    ..
                })
            ));
        }
        for packet in [
            "not JSON",
            r#"{"type":"response.output_item.done"}"#,
            r#"{"type":"response.completed","response":{}}"#,
        ] {
            assert!(matches!(
                ResponseAssembly::default().accept(packet),
                Err(TransportError::Stream(_))
            ));
        }
    }

    #[test]
    fn missing_completed_is_error() {
        assert!(ResponseAssembly::default().finish().is_err());
    }
}

#[cfg(test)]
mod native_replay_tests {
    use super::*;
    use crate::item::ToolInput;
    use serde_json::json;

    #[test]
    fn namespaced_tool_done_preserves_raw_call_and_completion_contract() {
        let mut assembly = ResponseAssembly::default();
        for (index, (kind, name, payload)) in [
            (
                "function_call",
                "yield",
                json!({"arguments":"{\"until\":null}"}),
            ),
            ("custom_tool_call", "cell", json!({"input":"raw Haskell λ"})),
        ]
        .into_iter()
        .enumerate()
        {
            let mut item = json!({"type":kind,"namespace":"functions","name":name,"call_id":format!("original-{name}"),"id":format!("item-{name}"),"encrypted_function_args":["retained"]});
            item.as_object_mut()
                .unwrap()
                .extend(payload.as_object().unwrap().clone());
            let event =
                json!({"type":"response.output_item.done","output_index":index,"item":item});
            let Some(StreamEvent::CompletedItem(done)) =
                assembly.accept(&event.to_string()).unwrap()
            else {
                panic!("completed call expected");
            };
            let retained = done.item();
            assert_eq!(retained.0, item);
            let call = retained.tool_call().unwrap().unwrap();
            assert_eq!(call.call_id.0, format!("original-{name}"));
            assert_eq!(call.name, name);
            match call.input {
                ToolInput::Function(args) => assert_eq!(args, json!({"until":null})),
                ToolInput::Custom(input) => assert_eq!(input, "raw Haskell λ"),
            }
        }
        assembly.accept(&json!({"type":"response.completed","response":{"id":"native-response","output":[],"usage":{"input_tokens":12,"output_tokens":7,"input_tokens_details":{"cached_tokens":4}}}}).to_string()).unwrap();
        let completed = assembly.finish().unwrap();
        assert_eq!(completed.items.len(), 2);
        assert_eq!(completed.response_id, "native-response");
        assert_eq!(completed.usage.cached_tokens, Some(4));
    }

    #[test]
    fn custom_input_delta_uses_native_call_id_fallback_without_renaming() {
        let mut assembly = ResponseAssembly::default();
        let event = json!({"type":"response.custom_tool_call_input.delta","call_id":"exact-call","delta":"λ"});
        let Some(StreamEvent::Delta {
            item_id,
            channel,
            text,
            ..
        }) = assembly.accept(&event.to_string()).unwrap()
        else {
            panic!("tool input delta expected");
        };
        assert_eq!(item_id, "exact-call");
        assert_eq!(channel, OutputChannel::ToolInput);
        assert_eq!(text, "λ");
        let mut preferred = event.clone();
        preferred["item_id"] = Value::Null;
        let Some(StreamEvent::Delta { item_id, .. }) =
            assembly.accept(&preferred.to_string()).unwrap()
        else {
            panic!("tool input delta expected");
        };
        assert_eq!(item_id, "exact-call");
        preferred["item_id"] = json!("native-item");
        let Some(StreamEvent::Delta { item_id, .. }) =
            assembly.accept(&preferred.to_string()).unwrap()
        else {
            panic!("tool input delta expected");
        };
        assert_eq!(item_id, "native-item");
        preferred.as_object_mut().unwrap().remove("item_id");
        preferred.as_object_mut().unwrap().remove("call_id");
        assert!(assembly.accept(&preferred.to_string()).is_err());
    }
}
