//! Acceptance checks for the correction wave's model-facing tool declarations.
use harness::provider::Provider;
use serde_json::{json, Value};

struct SchemaProvider;

#[async_trait::async_trait]
impl Provider for SchemaProvider {
    async fn call(&self, _: &str, _: Value) -> Result<Value, harness::provider::ProviderError> {
        Ok(Value::Null)
    }

    fn tools(&self) -> Vec<Value> {
        vec![json!({
            "type": "function",
            "name": "slow_tool",
            "description": "A provider-owned asynchronous tool",
            "parameters": {"type": "object", "properties": {}, "required": []},
            "strict": true
        })]
    }
}

#[test]
fn all_model_facing_tools_are_async_except_wait_agent() {
    let tools = SchemaProvider.all_tools();
    assert!(!tools.is_empty());
    for tool in tools {
        let name = tool["name"].as_str().expect("tool schema has a name");
        if name == "wait_agent" {
            assert_ne!(tool["async"], json!(true), "wait_agent is synchronous");
        } else {
            assert_eq!(
                tool["async"],
                json!(true),
                "{name} must be declared async so a pending call can be continued"
            );
        }
    }
}
