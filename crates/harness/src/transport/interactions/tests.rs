//! Protocol fixtures follow the official Interactions OpenAPI and thinking REST
//! example, retained under the task's interactions-contract evidence directory.
//! https://ai.google.dev/static/api/interactions.openapi.json
//! https://ai.google.dev/gemini-api/docs/thinking
//! https://ai.google.dev/gemini-api/docs/function-calling#stateless-mode
use super::*;
use super::super::{ResponseEnd, TransportObservation};

fn observation() -> TransportObservation {
    TransportObservation::new(ResponseEnd::ProviderTerminal)
}
fn send(assembly: &mut Assembly, event: Value) -> Result<Vec<StreamEvent>, TransportError> {
    assembly.accept(&event.to_string(), &mut observation())
}
fn created() -> Assembly {
    let mut assembly = Assembly::default();
    send(&mut assembly, json!({"event_type":"interaction.created","interaction":{"id":"turn-λ","status":"in_progress"}})).unwrap();
    assembly
}
fn start(assembly: &mut Assembly, index: u64, step: Value) {
    send(assembly, json!({"event_type":"step.start","index":index,"step":step})).unwrap();
}
fn stop(assembly: &mut Assembly, index: u64) -> Vec<StreamEvent> {
    send(assembly, json!({"event_type":"step.stop","index":index})).unwrap()
}
fn terminal(assembly: &mut Assembly, status: &str) {
    // Schema-derived requires_action fixture; this is not a live server capture.
    send(assembly, json!({"event_type":"interaction.status_update","interaction_id":"turn-λ","status":status})).unwrap();
}
fn request_fixture() -> ResponsesRequest {
    ResponsesRequest {
        input: vec![Item::user_text("hello λ")], instructions: "instructions".into(),
        tools: Vec::<Value>::new().into(), tools_allowed: None,
        model: "gemini-4-argon".into(), pinned_effort: Effort::Medium,
        session_id: "session".into(), prompt_cache_comparison: None,
    }
}
fn normalize(request_value: &ResponsesRequest, authored: &[bool]) -> Result<Value, TransportError> {
    request(request_value, InferenceLimits::gemini_default(), authored)
}

#[test]
fn late_signature_retains_initial_summary_and_exact_native_step() {
    let mut assembly = created();
    start(&mut assembly, 0, json!({"type":"thought","signature":"","summary":[{"type":"text","text":"initial λ"}]}));
    send(&mut assembly, json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_summary","content":{"type":"text","text":"second"}}})).unwrap();
    assert!(send(&mut assembly, json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_signature","signature":"opaque+/="}})).unwrap().is_empty());
    let done = stop(&mut assembly, 0);
    let StreamEvent::CompletedItem(done) = &done[0] else { panic!("expected complete thought") };
    assert_eq!(done.output_index(), 0);
    assert_eq!(done.item().0["summary"], json!([{"type":"summary_text","text":"initial λ"},{"type":"summary_text","text":"second"}]));
    assert_eq!(done.item().0["gemini_step"], json!({"type":"thought","signature":"opaque+/=","summary":[{"type":"text","text":"initial λ"},{"type":"text","text":"second"}]}));
    terminal(&mut assembly, "completed");
    assert_eq!(assembly.finish().unwrap().items[0], *done.item());
}

#[test]
fn signature_without_summary_survives_replay_without_inventing_summary() {
    let mut assembly = created();
    start(&mut assembly, 0, json!({"type":"thought"}));
    send(&mut assembly, json!({"event_type":"step.delta","index":0,"delta":{"type":"thought_signature","signature":"opaque"}})).unwrap();
    stop(&mut assembly, 0);
    terminal(&mut assembly, "completed");
    let item = assembly.finish().unwrap().items.remove(0);
    assert_eq!(item.0["summary"], json!([]));
    let mut request = request_fixture(); request.input.push(item);
    let body = normalize(&request, &[true,false]).unwrap();
    assert_eq!(body["input"][1], json!({"type":"thought","signature":"opaque"}));
}

#[test]
fn arguments_fragments_and_out_of_order_stops_release_calls_in_provider_order() {
    let mut assembly = created();
    start(&mut assembly, 0, json!({"type":"function_call","id":"a","name":"echo","arguments":{}}));
    start(&mut assembly, 1, json!({"type":"function_call","id":"b","name":"echo","arguments":{"n":2}}));
    for arguments in ["{\"nested\":", "{\"λ\":[1,true]}}"] {
        send(&mut assembly, json!({"event_type":"step.delta","index":0,"delta":{"type":"arguments_delta","arguments":arguments}})).unwrap();
    }
    assert!(stop(&mut assembly, 1).is_empty());
    let done = stop(&mut assembly, 0);
    let indices = done.iter().map(|event| match event { StreamEvent::CompletedItem(item) => item.output_index(), _ => panic!("unexpected delta") }).collect::<Vec<_>>();
    assert_eq!(indices, vec![0,1]);
    terminal(&mut assembly, "requires_action");
    let turn = assembly.finish().unwrap();
    assert_eq!(turn.items[0].tool_call().unwrap().unwrap().input, crate::item::ToolInput::Function(json!({"nested":{"λ":[1,true]}})));
    let mut request = request_fixture();
    request.input.extend(turn.items.clone());
    request.input.extend([Item(json!({"type":"function_call_output","call_id":"b","output":"second"})),Item(json!({"type":"function_call_output","call_id":"a","output":{"ok":true}}))]);
    let body = normalize(&request, &[true,false,false,false,false]).unwrap();
    assert_eq!(body["input"][1], turn.items[0].0["gemini_step"]);
    assert_eq!(body["input"][2], turn.items[1].0["gemini_step"]);
    assert_eq!(body["input"][3], json!({"type":"function_result","call_id":"b","name":"echo","result":[{"type":"text","text":"second"}]}));
    assert_eq!(body["input"][4]["result"], json!({"ok":true}));
}

#[test]
fn event_id_replay_is_idempotent_but_conflicting_bytes_are_refused() {
    let mut assembly = created();
    let event = json!({"event_id":"one","event_type":"step.start","index":0,"step":{"type":"model_output","content":[{"type":"text","text":"once"}]}});
    send(&mut assembly, event.clone()).unwrap();
    assert!(send(&mut assembly, event.clone()).unwrap().is_empty());
    let mut conflict = event; conflict["step"]["content"][0]["text"] = "changed".into();
    assert!(send(&mut assembly, conflict).is_err());
}

#[test]
fn incomplete_and_gapped_steps_never_produce_a_successful_turn() {
    let mut unfinished = created();
    start(&mut unfinished, 0, json!({"type":"model_output"}));
    assert!(send(&mut unfinished, json!({"event_type":"interaction.completed","interaction":{"id":"turn-λ","status":"completed"}})).is_err());
    assert!(unfinished.finish().is_err());
    let mut gap = created();
    start(&mut gap, 1, json!({"type":"model_output","content":[]}));
    assert!(stop(&mut gap, 1).is_empty());
    terminal(&mut gap, "completed");
    assert!(gap.finish().is_err());
    let mut no_terminal = created();
    start(&mut no_terminal, 0, json!({"type":"model_output","content":[]}));
    stop(&mut no_terminal, 0);
    assert!(no_terminal.finish().is_err());
}

#[test]
fn duplicate_indices_late_deltas_and_invalid_arguments_are_refused() {
    let mut duplicate = created();
    start(&mut duplicate, 0, json!({"type":"model_output"}));
    assert!(send(&mut duplicate, json!({"event_type":"step.start","index":0,"step":{"type":"model_output"}})).is_err());
    let mut late = created();
    start(&mut late, 0, json!({"type":"model_output","content":[]})); stop(&mut late, 0);
    assert!(send(&mut late, json!({"event_type":"step.delta","index":0,"delta":{"type":"text","text":"late"}})).is_err());
    let mut invalid = created();
    start(&mut invalid, 0, json!({"type":"function_call","id":"a","name":"echo","arguments":{}}));
    send(&mut invalid, json!({"event_type":"step.delta","index":0,"delta":{"type":"arguments_delta","arguments":"{broken"}})).unwrap();
    assert!(send(&mut invalid, json!({"event_type":"step.stop","index":0})).is_err());
}

#[test]
fn native_projection_mutations_and_foreign_provider_history_are_refused() {
    let native = json!({"type":"function_call","id":"call","name":"echo","arguments":{"n":1}});
    let call = project(native, "turn:0").unwrap();
    for (field,value) in [("name",json!("other")),("call_id",json!("different")),("arguments",json!({"n":2}))] {
        let mut changed = call.clone(); changed.0[field] = value;
        let mut request = request_fixture(); request.input.push(changed);
        request.input.push(Item(json!({"type":"function_call_output","call_id":"call","output":"ok"})));
        assert!(normalize(&request, &[true,false,false]).is_err(), "accepted changed {field}");
    }
    for foreign in [json!({"type":"reasoning","encrypted_content":"opaque","summary":[]}),json!({"type":"message","role":"assistant","content":"foreign response"}),json!({"type":"function_call","call_id":"foreign","name":"echo","arguments":"{}"})] {
        let mut request = request_fixture(); request.input.push(Item(foreign));
        assert!(normalize(&request, &[true,false]).is_err());
    }
}

#[test]
fn only_argon_and_supported_reasoning_levels_are_admitted() {
    let mut request = request_fixture();
    for model in ["gemini-3.8-flash","gemini-4-argon-preview","deepseek-flash",""] {
        request.model = model.into(); assert!(normalize(&request, &[true]).is_err());
    }
    request.model = "gemini-4-argon".into();
    for effort in [Effort::Low,Effort::Medium,Effort::High] {
        request.pinned_effort = effort;
        let body = normalize(&request, &[true]).unwrap();
        assert_eq!(body["generation_config"]["thinking_level"], serde_json::to_value(effort).unwrap());
        assert_eq!(body["store"], false);
        assert!(body.get("previous_interaction_id").is_none());
    }
    assert!(normalize(&request, &[]).is_err());
}

#[test]
fn requires_action_needs_a_call_and_failed_status_is_not_success() {
    let mut empty = created();
    assert!(send(&mut empty, json!({"event_type":"interaction.status_update","interaction_id":"turn-λ","status":"requires_action"})).is_err());
    for status in ["failed","cancelled","incomplete"] {
        let mut assembly = created();
        assert!(send(&mut assembly,json!({"event_type":"interaction.completed","interaction":{"id":"turn-λ","status":status}})).is_err());
        assert!(assembly.finish().is_err());
    }
}

#[test]
fn usage_missing_components_are_unknown_and_explicit_zero_remains_zero() {
    let absent = Usage::from_interaction_usage(&json!({}));
    assert_eq!(absent.input_tokens, None); assert_eq!(absent.output_tokens, None);
    let zero = Usage::from_interaction_usage(&json!({"usage":{"total_input_tokens":0,"total_output_tokens":0,"total_thought_tokens":0,"total_cached_tokens":0}}));
    assert_eq!(zero.input_tokens, Some(0)); assert_eq!(zero.output_tokens, Some(0)); assert_eq!(zero.cached_tokens, Some(0));
    let partial = Usage::from_interaction_usage(&json!({"usage":{"total_output_tokens":5}}));
    assert_eq!(partial.output_tokens, None); assert_eq!(partial.reasoning_tokens, None);
    let full = Usage::from_interaction_usage(&json!({"usage":{"total_input_tokens":2,"total_output_tokens":5,"total_thought_tokens":7}}));
    assert_eq!(full.output_tokens, Some(12)); assert_eq!(full.reasoning_tokens, Some(7));
}
