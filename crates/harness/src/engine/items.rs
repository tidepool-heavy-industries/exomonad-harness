use crate::{item::Item, model::CallId, turn::JobOutput};
use serde_json::{Value, json};

/// Parse a Responses API function-call item.
pub(super) fn function_call(item: &Item) -> Option<(CallId, String, Value)> {
    let object = item.0.as_object()?;
    if object.get("type")?.as_str()? != "function_call" {
        return None;
    }

    let call_id = object.get("call_id")?.as_str()?.to_owned();
    let name = object.get("name")?.as_str()?.to_owned();
    let arguments = match object.get("arguments")? {
        Value::String(arguments) => serde_json::from_str(arguments).ok()?,
        Value::Object(arguments) => Value::Object(arguments.clone()),
        _ => return None,
    };

    Some((CallId(call_id), name, arguments))
}

/// Encode a settled job as a Responses API function-call output item.
pub(super) fn function_output(call_id: &CallId, output: &JobOutput) -> Item {
    let value = match output {
        JobOutput::Completed(Ok(value)) => value.clone(),
        JobOutput::Completed(Err(error)) => json!({ "error": error }),
        JobOutput::Cancelled => json!({ "error": "job cancelled" }),
        JobOutput::Interrupted => json!({ "error": "job interrupted" }),
    };
    Item(json!({
        "type": "function_call_output",
        "call_id": call_id.0,
        "output": serde_json::to_string(&value).expect("JSON value serialization cannot fail"),
    }))
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
