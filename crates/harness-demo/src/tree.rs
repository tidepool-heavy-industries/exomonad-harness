//! Provider adapter for the durable, store-backed multi-agent demo.
//!
//! Agent tools are dispatched by the engine through `call_agent_verb`; ordinary
//! demo tools retain the CLI adapter's development gating.
use crate::CliProvider;
use async_trait::async_trait;
use harness::{
    agents::{AgentToolService, dispatch_agent_verb},
    provider::{CallContext, Provider, ProviderError},
};
use serde_json::{Value, json};
use std::sync::Arc;

pub struct TreeProvider {
    demo: CliProvider,
    agents: Arc<dyn AgentToolService>,
}

impl TreeProvider {
    pub fn new(demo: CliProvider, agents: Arc<dyn AgentToolService>) -> Self {
        Self { demo, agents }
    }
}

#[async_trait]
impl Provider for TreeProvider {
    async fn call(&self, name: &str, args: Value) -> Result<Value, ProviderError> {
        self.demo.call(name, args).await
    }

    async fn call_with_context(
        &self,
        name: &str,
        args: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        self.demo.call_with_context(name, args, context).await
    }

    async fn call_agent_verb(
        &self,
        name: &str,
        args: Value,
        context: CallContext,
    ) -> Result<Value, ProviderError> {
        // The store can represent inherited heads, but the demo has no
        // committed fork boundary yet. Never claim that stale parent history
        // was safely forked.
        if name == "spawn_agent" {
            let kind = args
                .get("from")
                .and_then(|from| from.get("kind"))
                .and_then(Value::as_str);
            if matches!(kind, Some("here" | "checkpoint")) {
                return Err(ProviderError::Tool(
                    "spawn from here/checkpoint is disabled until a committed fork boundary exists"
                        .into(),
                ));
            }
        }
        dispatch_agent_verb(self.agents.as_ref(), &context.agent, name, args)
            .await
            .map_err(|error| ProviderError::Tool(error.to_string()))
    }

    fn tools(&self) -> Vec<Value> {
        let demo_tools = self.demo.tools();
        let shell_enabled = demo_tools.iter().any(|tool| tool["name"] == "run");
        demo_tools
            .into_iter()
            .filter_map(|mut tool| {
                match tool["name"].as_str() {
                    Some("ask" | "form" | "edit") => None,
                    Some("run") => {
                        // CliProvider already enforces --dev-shell and makes run a
                        // strict function when enabled.
                        if shell_enabled {
                            tool["type"] = json!("function");
                            tool["strict"] = json!(true);
                            tool["parameters"] = json!({
                                "type":"object",
                                "properties":{"script":{"type":"string"}},
                                "required":["script"],
                                "additionalProperties":false
                            });
                            Some(tool)
                        } else {
                            None
                        }
                    }
                    _ => Some(tool),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DemoProvider;
    use harness::{agent_runtime::StoreAgentToolService, model::AgentPath, store::Store};

    fn provider(shell: bool) -> TreeProvider {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({"kind":"root"}))
            .unwrap();
        TreeProvider::new(
            CliProvider(DemoProvider::development(".", shell)),
            Arc::new(StoreAgentToolService::new(store, root)),
        )
    }

    fn contract() -> Value {
        json!({"clauses":["do work"],"acceptance":["done"],"owned":[],"must_not":[],
            "introduces":[],"consumes":[],"boundaries":[]})
    }

    fn context(agent: &str) -> CallContext {
        CallContext {
            handle: harness::provider::JobHandle("test".into()),
            call_id: harness::model::CallId("call".into()),
            agent: AgentPath(agent.into()),
            progress: tokio::sync::mpsc::unbounded_channel().0,
        }
    }

    #[test]
    fn tool_list_preserves_demo_gates_and_agent_schemas() {
        for (shell, expected_run) in [(false, false), (true, true)] {
            let tools = provider(shell).all_tools();
            for tool in &tools {
                if tool["name"] == "wait_agent" {
                    assert!(tool.get("async").is_none());
                } else {
                    assert_eq!(tool["async"], true, "tool {} must be async", tool["name"]);
                }
            }
            let names: Vec<_> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
            assert!(!names.contains(&"ask"));
            assert!(!names.contains(&"form"));
            assert_eq!(names.contains(&"run"), expected_run);
            assert!(!names.contains(&"edit"), "no host-owned paths are granted");
            if expected_run {
                let run = tools.iter().find(|tool| tool["name"] == "run").unwrap();
                assert_eq!(run["type"], "function");
                assert_eq!(run["strict"], true);
                assert_eq!(run["parameters"]["additionalProperties"], false);
            }
            for name in [
                "spawn_agent",
                "send_message",
                "followup_task",
                "checkpoint",
                "list_agents",
                "wait_agent",
            ] {
                let schema = tools
                    .iter()
                    .find(|t| t["name"] == name)
                    .expect("agent tool schema");
                assert_eq!(schema["strict"], true);
                assert_eq!(schema["parameters"]["additionalProperties"], false);
            }
        }
    }

    #[tokio::test]
    async fn unknown_tools_are_not_swallowed_and_uncommitted_forks_refused() {
        let provider = provider(false);
        assert!(provider.call("no_such_tool", json!({})).await.is_err());
        for from in [
            json!({"kind":"here","name":null}),
            json!({"kind":"checkpoint","name":"saved"}),
        ] {
            let args = json!({"task_name":"child","from":from,"task":contract()});
            assert!(
                provider
                    .call_agent_verb("spawn_agent", args, context("/root"))
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("committed fork boundary")
            );
        }
    }

    #[tokio::test]
    async fn prompt_spawn_and_scoped_send_are_durable_store_operations() {
        let store = Arc::new(Store::memory().unwrap());
        let root = AgentPath("/root".into());
        store
            .admit_agent(&root, None, None, &json!({}), &json!({"kind":"root"}))
            .unwrap();
        store
            .admit_agent(
                &AgentPath("/root/unrelated".into()),
                Some(&root),
                None,
                &json!({}),
                &json!({"kind":"root"}),
            )
            .unwrap();
        let provider = TreeProvider::new(
            CliProvider(DemoProvider::development(".", false)),
            Arc::new(StoreAgentToolService::new(store.clone(), root)),
        );
        for from in [
            json!({"kind":"here","name":null}),
            json!({"kind":"checkpoint","name":"saved"}),
        ] {
            assert!(
                provider
                    .call_agent_verb(
                        "spawn_agent",
                        json!({"task_name":"refused","from":from,"task":contract()}),
                        context("/root"),
                    )
                    .await
                    .is_err()
            );
        }
        assert!(
            store
                .agent(&AgentPath("/root/refused".into()))
                .unwrap()
                .is_none()
        );
        assert!(store.unread("/root").unwrap().is_empty());
        let spawned = provider
            .call_agent_verb(
                "spawn_agent",
                json!({
                    "task_name":"child","from":{"kind":"prompt","name":null},"task":contract()
                }),
                context("/root"),
            )
            .await
            .unwrap();
        let child = AgentPath(spawned["task_name"].as_str().unwrap().to_owned());
        assert_eq!(child, AgentPath("/root/child".into()));
        let record = store.agent(&child).unwrap().unwrap();
        assert_eq!(record.head_request, None);
        let inbox = store.unread(&child.0).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].class, "AtBoundary");
        let task_item = store.get_item(&inbox[0].item_hash).unwrap().unwrap();
        assert_eq!(task_item.0["role"], "assistant");
        assert!(
            task_item.0["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("Message Type: NEW_TASK\n")
        );

        provider
            .call_agent_verb(
                "send_message",
                json!({
                    "target":child.0,"message":"hello"
                }),
                context("/root"),
            )
            .await
            .unwrap();
        let inbox = store.unread(&child.0).unwrap();
        assert_eq!(inbox.len(), 2);
        let message_item = store.get_item(&inbox[1].item_hash).unwrap().unwrap();
        assert_eq!(message_item.0["role"], "assistant");
        assert!(
            provider
                .call_agent_verb(
                    "send_message",
                    json!({
                        "target":child.0,"message":"unauthorized"
                    }),
                    context("/root/unrelated")
                )
                .await
                .is_err()
        );
        assert_eq!(store.unread(&child.0).unwrap().len(), 2);
    }
}
