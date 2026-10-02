use super::*;
use crate::{item::Item, model::Effort};
use serde_json::json;

fn tool(name: &str) -> Value {
    json!({"type":"function","name":name,"description":"fixture","strict":true,"async":true,"parameters":{"type":"object","properties":{},"required":[],"additionalProperties":false}})
}
fn request() -> ResponsesRequest {
    ResponsesRequest {
        model:"opaque-provider-model".into(),instructions:"shared instructions".into(),
        input:vec![Item::configuration_update(Effort::Low),Item(json!({"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}))],
        tools:vec![tool("work"),json!({"type":"custom","name":"cell","description":"raw Haskell","format":{"type":"text"}}),json!({"type":"function","name":"yield","strict":true,"parameters":{"type":"object","properties":{"until":{"anyOf":[{"type":"number","minimum":0},{"type":"null"}]}},"required":["until"],"additionalProperties":false}})].into(),
        tools_allowed:None,pinned_effort:Effort::Low,session_id:"shared-session".into(),
    }
}
fn lite(request: &ResponsesRequest) -> Value {
    request_body_for_protocol(request, ResponsesProtocol::Lite).unwrap()
}

#[test]
fn explicit_lite_header_and_body_preserve_engine_tool_contract() {
    let request = request();
    let original = request.clone();
    let body = lite(&request);
    assert_eq!(
        protocol_headers(ResponsesProtocol::Lite)[RESPONSES_LITE_HEADER],
        "true"
    );
    assert!(!protocol_headers(ResponsesProtocol::Standard).contains_key(RESPONSES_LITE_HEADER));
    assert!(body.get("instructions").is_none());
    assert!(body.get("tools").is_none());
    assert_eq!(body["parallel_tool_calls"], false);
    assert_eq!(body["reasoning"]["context"], "all_turns");
    assert_eq!(body["reasoning"]["summary"], "auto");
    assert_eq!(body["model"], "opaque-provider-model");
    assert_eq!(body["stream"], true);
    assert_eq!(body["store"], false);
    assert_eq!(body["prompt_cache_key"], "shared-session");
    assert!(body.get("previous_response_id").is_none());
    assert_eq!(body["input"][0]["type"], "additional_tools");
    assert_eq!(body["input"][0]["role"], "developer");
    let namespace = &body["input"][0]["tools"][0];
    assert_eq!(namespace["type"], "namespace");
    assert_eq!(namespace["name"], "functions");
    assert_eq!(namespace["description"], "");
    assert_eq!(namespace["tools"].as_array().unwrap().len(), 3);
    assert_eq!(namespace["tools"][0]["name"], "work");
    assert!(namespace["tools"][0].get("async").is_none());
    assert_eq!(namespace["tools"][1], original.tools[1]);
    assert_eq!(namespace["tools"][2], original.tools[2]);
    assert_eq!(body["input"][1]["role"], "developer");
    assert_eq!(
        body["input"][1]["content"],
        json!([{"type":"input_text","text":"shared instructions"}])
    );
    assert_eq!(
        body["input"][1]["internal_chat_message_metadata_passthrough"]["content_item_kinds"],
        json!(["model.base_instructions"])
    );
    assert_eq!(body["input"][2], original.input[1].0);
    assert_eq!(request.input, original.input);
    assert_eq!(request.tools, original.tools);
}

#[test]
fn lite_prefix_ids_track_exact_payload_across_sessions_and_followups() {
    let mut request = request();
    let before = lite(&request);
    request.session_id = "new-exact-context".into();
    request.input.push(Item(
        json!({"type":"function_call_output","call_id":"original","output":"done"}),
    ));
    let continued = lite(&request);
    assert_eq!(before["input"][0], continued["input"][0]);
    assert_eq!(before["input"][1], continued["input"][1]);
    request.instructions.push_str(" changed");
    let changed = lite(&request);
    assert_eq!(changed["input"][0], continued["input"][0]);
    assert_ne!(changed["input"][1]["id"], continued["input"][1]["id"]);
    request.tools = vec![tool("changed")].into();
    let tools_changed = lite(&request);
    assert_ne!(tools_changed["input"][0]["id"], changed["input"][0]["id"]);
    assert_eq!(tools_changed["input"][1], changed["input"][1]);
}

#[test]
fn lite_projects_latest_effort_without_changing_durable_history() {
    let mut request = request();
    request.input.push(Item::configuration_update(Effort::High));
    let before = request.input.clone();
    let body = lite(&request);
    assert_eq!(body["reasoning"]["effort"], "high");
    assert!(
        body["input"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["type"] != "configuration_update")
    );
    assert_eq!(request.input, before);
    let standard = request_body(&request).unwrap();
    assert_eq!(standard["reasoning"]["effort"], "low");
    assert_eq!(standard["input"].as_array().unwrap().len(), before.len());
    assert_eq!(standard["parallel_tool_calls"], true);
    assert!(standard["reasoning"].get("context").is_none());
}

#[test]
fn lite_empty_model_prompt_and_no_tool_restriction_are_explicit() {
    let mut request = request();
    request.instructions.clear();
    request.tools = vec![].into();
    request.input.clear();
    let body = lite(&request);
    assert_eq!(body["input"].as_array().unwrap().len(), 1);
    assert_eq!(body["input"][0]["tools"], json!([]));
    assert_eq!(body["reasoning"]["effort"], "low");
    request.tools_allowed = Some(vec![]);
    assert_eq!(lite(&request)["tool_choice"], "none");
}

#[test]
fn lite_restrictions_project_only_allowed_tools_in_original_order() {
    let mut request = request();
    request.tools_allowed = Some(vec!["yield".into(), "work".into()]);
    let body = lite(&request);
    let tools = body["input"][0]["tools"][0]["tools"].as_array().unwrap();
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["work", "yield"]
    );
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(request.tools.len(), 3);
}

#[test]
fn lite_admits_strict_schema_before_any_wire_projection() {
    let mut request = request();
    request.tools =
        vec![json!({"type":"function","name":"bad","strict":false,"parameters":{}})].into();
    request.tools_allowed = Some(vec![]);
    assert!(request_body_for_protocol(&request, ResponsesProtocol::Lite).is_err());
    request.tools = vec![json!({"type":"namespace","name":"foreign","tools":[]})].into();
    request.tools_allowed = None;
    assert!(request_body_for_protocol(&request, ResponsesProtocol::Lite).is_err());
}

#[test]
fn lite_namespace_calls_retain_exact_wire_items_and_outputs() {
    use crate::turn::JobOutput;
    for (kind, payload) in [
        ("function_call", json!({"arguments":"{\"until\":null}"})),
        ("custom_tool_call", json!({"input":"raw λ\\ncell"})),
    ] {
        let mut wire = json!({"type":kind,"namespace":"functions","name":"yield","call_id":"exact-call","id":"fc_exact","encrypted_function_args":["opaque"]});
        wire.as_object_mut()
            .unwrap()
            .extend(payload.as_object().unwrap().clone());
        let item = Item(wire.clone());
        let call = item.tool_call().unwrap().unwrap();
        assert_eq!(call.call_id.0, "exact-call");
        assert_eq!(call.name, "yield");
        assert_eq!(item.0, wire);
        let output = Item::tool_output(
            &call.call_id,
            call.input.kind(),
            &JobOutput::Completed(Ok(json!({"ready":true}))),
        );
        assert_eq!(output.0["call_id"], "exact-call");
        for namespace in [json!("foreign"), json!(false), json!(4)] {
            let mut invalid = item.clone();
            invalid.0["namespace"] = namespace;
            assert!(invalid.tool_call().is_err());
        }
    }
}
