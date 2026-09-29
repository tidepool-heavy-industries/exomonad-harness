#[cfg(test)]
use crate::{item::ToolKind, turn::JobOutput};
use crate::{
    item::{Item, ToolInput},
    model::CallId,
};
use serde_json::Value;
#[cfg(test)]
use serde_json::json;

/// Parse a Responses API function-call item.
pub(super) fn function_call(item: &Item) -> Option<(CallId, String, Value)> {
    let call = item.tool_call().ok()??;
    let ToolInput::Function(arguments) = call.input else {
        return None;
    };
    Some((call.call_id, call.name, arguments))
}

/// Encode a settled job as a Responses API function-call output item.
#[cfg(test)]
pub(super) fn function_output(call_id: &CallId, output: &JobOutput) -> Item {
    Item::tool_output(call_id, ToolKind::Function, output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_function_call_with_object_arguments() {
        let item = Item(json!({
            "type": "function_call",
            "call_id": "call-1",
            "name": "lookup",
            "arguments": { "query": "rust" },
            "extra": true
        }));
        assert_eq!(
            function_call(&item),
            Some((
                CallId("call-1".into()),
                "lookup".into(),
                json!({ "query": "rust" })
            ))
        );
    }

    #[test]
    fn parses_json_string_arguments_and_rejects_malformed_calls() {
        let item = Item(json!({
            "type": "function_call",
            "call_id": "call-2",
            "name": "lookup",
            "arguments": "{\"query\":\"rust\"}"
        }));
        assert_eq!(function_call(&item).unwrap().2, json!({ "query": "rust" }));

        let malformed = Item(json!({
            "type": "function_call",
            "call_id": "call-2",
            "name": "lookup",
            "arguments": "{"
        }));
        assert_eq!(function_call(&malformed), None);
        assert_eq!(
            function_call(&Item(json!({
                "type": "message", "call_id": "c", "name": "n", "arguments": {}
            }))),
            None
        );
    }

    #[test]
    fn encodes_every_job_output_as_json_text() {
        let call_id = CallId("call-3".into());
        let cases = [
            (
                JobOutput::Completed(Ok(json!({ "answer": 42 }))),
                json!({ "answer": 42 }),
            ),
            (
                JobOutput::Completed(Err("failed".into())),
                json!({ "error": "failed" }),
            ),
            (JobOutput::Cancelled, json!({ "error": "job cancelled" })),
            (
                JobOutput::Interrupted,
                json!({ "error": "job interrupted" }),
            ),
        ];

        for (job, expected) in cases {
            let item = function_output(&call_id, &job);
            assert_eq!(item.0["type"], "function_call_output");
            assert_eq!(item.0["call_id"], "call-3");
            assert_eq!(
                serde_json::from_str::<Value>(item.0["output"].as_str().unwrap()).unwrap(),
                expected
            );
        }
    }
}
