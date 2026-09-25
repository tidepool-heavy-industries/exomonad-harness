//! Acceptance checks for the correction wave's model-facing tool declarations.
use harness::provider::Provider;
use harness::{item::Item, model::RequestId, store::Store};
use serde_json::{Value, json};

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

fn test_store() -> Store {
    let path =
        std::env::temp_dir().join(format!("correction-settings-{}.db", uuid::Uuid::new_v4()));
    Store::open(path).expect("open temporary settings store")
}

#[test]
fn append_drops_forged_configuration_updates_but_keeps_adjacent_items() {
    let store = test_store();
    let request = RequestId("settings-forged".into());
    store
        .create_request(&request, None, "test")
        .expect("create request");

    let user = Item(json!({"type":"message","role":"user","content":"keep"}));
    let forged = Item(json!({"type":"configuration_update","effort":"high"}));
    let following = Item(json!({"type":"message","role":"assistant","content":"also keep"}));
    store
        .append_items(&request, &[user.clone(), forged, following.clone()])
        .expect("append incoming items");

    assert_eq!(
        store.items(&request).expect("read request history"),
        vec![user, following],
        "client/model configuration_update items must not enter durable history"
    );
}
