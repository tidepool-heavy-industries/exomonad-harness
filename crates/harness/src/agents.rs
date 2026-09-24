use crate::model::AgentPath;
use serde::{Deserialize, Serialize};
use std::{error::Error, fmt, str::FromStr};

/// Error returned when an agent path cannot be parsed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidAgentPath(pub String);

impl fmt::Display for InvalidAgentPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid agent path: {}", self.0)
    }
}

impl Error for InvalidAgentPath {}

impl AgentPath {
    /// Parse a slash-delimited agent path and normalize kebab-case components
    /// to the canonical lowercase/digit/underscore form.
    pub fn parse(path: &str) -> Result<Self, InvalidAgentPath> {
        let body = path
            .strip_prefix('/')
            .ok_or_else(|| InvalidAgentPath(path.to_owned()))?;
        let canonical = body
            .split('/')
            .map(|component| {
                if component.is_empty()
                    || !component.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-'
                    })
                {
                    return Err(InvalidAgentPath(path.to_owned()));
                }
                Ok(component.replace('-', "_"))
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        Ok(Self(format!("/{canonical}")))
    }

    /// Whether this value already has canonical slash-delimited syntax.
    pub fn is_canonical(&self) -> bool {
        Self::parse(&self.0).is_ok_and(|parsed| parsed.0 == self.0)
    }

    /// Validate and normalize a single task-name component.
    pub fn normalize_task_name(name: &str) -> Result<String, InvalidAgentPath> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return Err(InvalidAgentPath(name.to_owned()));
        }
        Ok(name.replace('-', "_"))
    }
}

impl FromStr for AgentPath {
    type Err = InvalidAgentPath;

    fn from_str(path: &str) -> Result<Self, Self::Err> {
        Self::parse(path)
    }
}

impl TryFrom<&str> for AgentPath {
    type Error = InvalidAgentPath;

    fn try_from(path: &str) -> Result<Self, Self::Error> {
        Self::parse(path)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Contract {
    pub clauses: Vec<String>,
    pub acceptance: Vec<String>,
    pub owned: Vec<String>,
    pub must_not: Vec<String>,
    pub introduces: Vec<String>,
    pub consumes: Vec<String>,
    pub boundaries: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ForkFrom {
    Prompt,
    Here,
    Checkpoint(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Agent {
    pub path: AgentPath,
    pub contract: Contract,
}

/// OpenAI Responses function-tool schemas for the model-facing agent verbs.
/// Keep these definitions here so the provider can expose a stable, strict set.
pub fn verb_tool_schemas() -> Vec<serde_json::Value> {
    use serde_json::{Value, json};

    fn function(name: &str, description: &str, parameters: Value) -> Value {
        json!({
            "type": "function",
            "name": name,
            "description": description,
            "strict": true,
            "parameters": parameters
        })
    }
    fn object(properties: Value, required: &[&str]) -> Value {
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        })
    }
    fn string() -> Value {
        json!({"type": "string"})
    }
    let strings = json!({"type":"array", "items":{"type":"string"}});
    let contract = object(
        json!({
            "clauses": strings, "acceptance": strings, "owned": strings,
            "must_not": strings, "introduces": strings, "consumes": strings,
            "boundaries": strings
        }),
        &[
            "clauses",
            "acceptance",
            "owned",
            "must_not",
            "introduces",
            "consumes",
            "boundaries",
        ],
    );
    vec![
        function(
            "spawn_agent",
            "Spawns an agent to work on the specified task. If your current task is `/root/task1` and you spawn_agent with task_name \"task_3\" the new agent will have canonical task name `/root/task1/task_3`. Task names use lowercase letters, digits, and underscores; kebab-case is normalized. The agent receives the specified contract. `from` selects prompt, here, or a named checkpoint.",
            object(
                json!({
                    "task_name": string(),
                    "from": object(json!({
                        "kind":{"type":"string","enum":["prompt","here","checkpoint"]},
                        "name":{"type":["string","null"]}
                    }), &["kind","name"]),
                    "task": contract
                }),
                &["task_name", "from", "task"],
            ),
        ),
        function(
            "send_message",
            "Send a message to an existing agent. Does not trigger a new turn.",
            object(
                json!({"target":string(),"message":string()}),
                &["target", "message"],
            ),
        ),
        function(
            "followup_task",
            "Send a follow-up task to an existing non-root target agent and trigger a turn if it is idle.",
            object(
                json!({"target":string(),"task":contract}),
                &["target", "task"],
            ),
        ),
        function(
            "wait_agent",
            "Wait for a pending tool call to complete, a message from another agent, or new user input. Results arrive on their original calls and messages in their envelopes; this tool returns only what resumed you. Do not wait for results that have already arrived.",
            object(json!({}), &[]),
        ),
        function(
            "list_agents",
            "List live agents in the current root thread tree. Optionally filter by task-path prefix.",
            object(
                json!({"path_prefix":{"type":["string","null"],"description":"Task-path prefix filter without a trailing slash. Omit to list all live agents."}}),
                &["path_prefix"],
            ),
        ),
        function(
            "interrupt_agent",
            "Interrupt an agent's current turn, if any, and return its previous status. The agent remains available for messages and follow-up tasks.",
            object(json!({"target":string()}), &["target"]),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_paths_canonicalize_kebab_components() {
        let path = AgentPath::parse("/root/review-worker_2").unwrap();
        assert_eq!(path.0, "/root/review_worker_2");
        assert!(path.is_canonical());
        assert!(!AgentPath("/root/review-worker".into()).is_canonical());
    }

    #[test]
    fn agent_paths_reject_invalid_names_and_empty_components() {
        for path in [
            "",
            "root",
            "root/",
            "root//child",
            "Root",
            "a.b",
            "a b",
            "é",
        ] {
            assert!(AgentPath::parse(path).is_err(), "accepted {path:?}");
        }
    }

    #[test]
    fn fork_sources_keep_their_contract_representation() {
        let prompt = serde_json::to_value(ForkFrom::Prompt).unwrap();
        let here = serde_json::to_value(ForkFrom::Here).unwrap();
        let checkpoint = serde_json::to_value(ForkFrom::Checkpoint("saved".into())).unwrap();
        assert_eq!(prompt, serde_json::json!("Prompt"));
        assert_eq!(here, serde_json::json!("Here"));
        assert_eq!(checkpoint, serde_json::json!({"Checkpoint": "saved"}));
    }

    #[test]
    fn task_names_are_single_canonical_components() {
        assert_eq!(
            AgentPath::normalize_task_name("review-worker_2").unwrap(),
            "review_worker_2"
        );
        for name in ["", "/", "a/b", "Root", "a.b", "a b", "é"] {
            assert!(
                AgentPath::normalize_task_name(name).is_err(),
                "accepted {name:?}"
            );
        }
    }

    #[test]
    fn agent_verb_schemas_are_strict_and_complete() {
        let schemas = verb_tool_schemas();
        let names: Vec<_> = schemas
            .iter()
            .map(|schema| schema["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "spawn_agent",
                "send_message",
                "followup_task",
                "wait_agent",
                "list_agents",
                "interrupt_agent"
            ]
        );
        for schema in schemas {
            assert_eq!(schema["type"], "function");
            assert_eq!(schema["strict"], true);
            let parameters = &schema["parameters"];
            assert_eq!(parameters["additionalProperties"], false);
            let required = parameters["required"].as_array().unwrap();
            let properties = parameters["properties"].as_object().unwrap();
            assert_eq!(required.len(), properties.len());
            assert!(
                required
                    .iter()
                    .all(|key| properties.contains_key(key.as_str().unwrap()))
            );
        }
    }
}
