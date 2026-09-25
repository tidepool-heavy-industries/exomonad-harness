use crate::model::{AgentPath, CallId, Effort, RequestId};
use async_trait::async_trait;
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

/// Serde boundary wrapper for untrusted model/API path strings. The legacy
/// `AgentPath` tuple keeps its existing serde representation for compatibility;
/// new ingress code should deserialize this wrapper instead.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ValidatedAgentPath(pub AgentPath);

impl<'de> Deserialize<'de> for ValidatedAgentPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        AgentPath::parse(&raw)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SpawnSource {
    Prompt,
    Here,
    Checkpoint(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnSourceInput {
    kind: String,
    name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct AgentVerbError(pub String);

/// Harness-owned identity of the model call invoking an agent verb.
/// A `here` fork must not substitute the last completed agent head.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentInvocation {
    pub request: RequestId,
    pub call_id: CallId,
}

/// Execution boundary for harness-owned agent operations. Implementations
/// bind these operations to conversation storage and the request scheduler.
#[async_trait]
pub trait AgentToolService: Send + Sync {
    async fn spawn_agent(
        &self,
        parent: &AgentPath,
        task_name: &str,
        from: SpawnSource,
        contract: Contract,
    ) -> Result<serde_json::Value, AgentVerbError>;
    /// Defaults to the stored-head behavior for non-model callers. The
    /// durable runtime overrides this to snapshot the active request and
    /// gate a `here` child's first turn until its call output is persisted.
    async fn spawn_agent_from_invocation(
        &self,
        parent: &AgentPath,
        task_name: &str,
        from: SpawnSource,
        contract: Contract,
        _invocation: Option<&AgentInvocation>,
    ) -> Result<serde_json::Value, AgentVerbError> {
        self.spawn_agent(parent, task_name, from, contract).await
    }
    async fn send_message(
        &self,
        sender: &AgentPath,
        target: AgentPath,
        message: String,
    ) -> Result<serde_json::Value, AgentVerbError>;
    async fn followup_task(
        &self,
        sender: &AgentPath,
        target: AgentPath,
        contract: Contract,
    ) -> Result<serde_json::Value, AgentVerbError>;
    async fn wait_agent(&self, agent: &AgentPath) -> Result<serde_json::Value, AgentVerbError>;
    async fn checkpoint(
        &self,
        agent: &AgentPath,
        name: String,
    ) -> Result<serde_json::Value, AgentVerbError>;
    async fn set_effort(
        &self,
        _agent: &AgentPath,
        _effort: Effort,
    ) -> Result<serde_json::Value, AgentVerbError> {
        Err(AgentVerbError("set_effort is not implemented".into()))
    }
    async fn list_agents(
        &self,
        agent: &AgentPath,
        prefix: Option<AgentPath>,
    ) -> Result<serde_json::Value, AgentVerbError>;
    async fn interrupt_agent(
        &self,
        agent: &AgentPath,
        target: AgentPath,
    ) -> Result<serde_json::Value, AgentVerbError>;
}

pub fn is_agent_verb(name: &str) -> bool {
    matches!(
        name,
        "spawn_agent"
            | "send_message"
            | "followup_task"
            | "wait_agent"
            | "checkpoint"
            | "set_effort"
            | "list_agents"
            | "interrupt_agent"
    )
}

/// Validate a model tool call and route it to the runtime owner. Agent paths
/// enter only through the grammar parser at this boundary.
pub async fn dispatch_agent_verb(
    service: &dyn AgentToolService,
    agent: &AgentPath,
    invocation: Option<&AgentInvocation>,
    name: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, AgentVerbError> {
    if !agent.is_canonical() {
        return Err(AgentVerbError("caller agent path is not canonical".into()));
    }
    fn parse_path(value: &serde_json::Value, key: &str) -> Result<AgentPath, AgentVerbError> {
        let raw = value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AgentVerbError(format!("missing string field `{key}`")))?;
        AgentPath::parse(raw).map_err(|e| AgentVerbError(e.to_string()))
    }
    fn exact_keys(args: &serde_json::Value, allowed: &[&str]) -> Result<(), AgentVerbError> {
        let object = args
            .as_object()
            .ok_or_else(|| AgentVerbError("tool arguments must be an object".into()))?;
        if object.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(AgentVerbError(
                "tool arguments contain unknown fields".into(),
            ));
        }
        Ok(())
    }
    match name {
        "spawn_agent" => {
            exact_keys(&args, &["task_name", "from", "task"])?;
            let task_name = args
                .get("task_name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| AgentVerbError("missing string field `task_name`".into()))?;
            let task_name = AgentPath::normalize_task_name(task_name)
                .map_err(|e| AgentVerbError(e.to_string()))?;
            let from: SpawnSourceInput = serde_json::from_value(
                args.get("from")
                    .cloned()
                    .ok_or_else(|| AgentVerbError("missing field `from`".into()))?,
            )
            .map_err(|e| AgentVerbError(format!("invalid spawn source: {e}")))?;
            if args["from"].get("name").is_none() {
                return Err(AgentVerbError(
                    "spawn source requires a `name` field (null when unused)".into(),
                ));
            }
            let from = match (from.kind.as_str(), from.name) {
                ("prompt", None) => SpawnSource::Prompt,
                ("here", None) => SpawnSource::Here,
                ("checkpoint", Some(name)) if !name.is_empty() => SpawnSource::Checkpoint(name),
                ("checkpoint", _) => {
                    return Err(AgentVerbError(
                        "checkpoint source requires a non-empty name".into(),
                    ));
                }
                _ => return Err(AgentVerbError("unknown spawn source kind".into())),
            };
            let contract: Contract = serde_json::from_value(
                args.get("task")
                    .cloned()
                    .ok_or_else(|| AgentVerbError("missing field `task`".into()))?,
            )
            .map_err(|e| AgentVerbError(format!("invalid task contract: {e}")))?;
            service
                .spawn_agent_from_invocation(agent, &task_name, from, contract, invocation)
                .await
        }
        "send_message" => {
            exact_keys(&args, &["target", "message"])?;
            let target = parse_path(&args, "target")?;
            let message = args
                .get("message")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| AgentVerbError("missing string field `message`".into()))?;
            service
                .send_message(agent, target, message.to_owned())
                .await
        }
        "followup_task" => {
            exact_keys(&args, &["target", "task"])?;
            let target = parse_path(&args, "target")?;
            let contract: Contract = serde_json::from_value(
                args.get("task")
                    .cloned()
                    .ok_or_else(|| AgentVerbError("missing field `task`".into()))?,
            )
            .map_err(|e| AgentVerbError(format!("invalid task contract: {e}")))?;
            service.followup_task(agent, target, contract).await
        }
        "wait_agent" => {
            exact_keys(&args, &[])?;
            service.wait_agent(agent).await
        }
        "checkpoint" => {
            exact_keys(&args, &["name"])?;
            let name = args
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| AgentVerbError("missing string field `name`".into()))?;
            service.checkpoint(agent, name.to_owned()).await
        }
        "set_effort" => {
            exact_keys(&args, &["effort"])?;
            let effort: Effort = serde_json::from_value(
                args.get("effort")
                    .cloned()
                    .ok_or_else(|| AgentVerbError("missing field `effort`".into()))?,
            )
            .map_err(|e| AgentVerbError(format!("invalid effort: {e}")))?;
            service.set_effort(agent, effort).await
        }
        "list_agents" => {
            exact_keys(&args, &["path_prefix"])?;
            let prefix = match args.get("path_prefix") {
                None | Some(serde_json::Value::Null) => None,
                Some(serde_json::Value::String(raw)) => {
                    Some(AgentPath::parse(raw).map_err(|e| AgentVerbError(e.to_string()))?)
                }
                _ => return Err(AgentVerbError("invalid `path_prefix`".into())),
            };
            service.list_agents(agent, prefix).await
        }
        "interrupt_agent" => {
            exact_keys(&args, &["target"])?;
            let target = parse_path(&args, "target")?;
            service.interrupt_agent(agent, target).await
        }
        _ => Err(AgentVerbError(format!("unknown agent verb `{name}`"))),
    }
}

/// OpenAI Responses function-tool schemas for the model-facing agent verbs.
/// Keep these definitions here so the provider can expose a stable, strict set.
pub fn verb_tool_schemas() -> Vec<serde_json::Value> {
    use serde_json::{Value, json};

    // NOTE(correction-wave b): do not add `"async": true` per schema here;
    // `Provider::all_tools` stamps it once for every tool but `wait_agent`.
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
            "checkpoint",
            "Create a named checkpoint at the current completed request boundary.",
            object(json!({"name":string()}), &["name"]),
        ),
        function(
            "set_effort",
            "Set reasoning effort for the next request by appending a positional configuration update. A second change before a request replaces the first.",
            object(
                json!({"effort":{"type":"string","enum":["low","medium","high"]}}),
                &["effort"],
            ),
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
        assert_eq!(AgentPath::parse("/root").unwrap().0, "/root");
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
    fn validated_serde_path_rejects_non_absolute_and_invalid_paths() {
        assert_eq!(
            serde_json::from_str::<ValidatedAgentPath>(r#""/root/worker-1""#)
                .unwrap()
                .0
                .0,
            "/root/worker_1"
        );
        assert!(serde_json::from_str::<ValidatedAgentPath>(r#""root/worker""#).is_err());
        assert!(serde_json::from_str::<ValidatedAgentPath>(r#""/root//worker""#).is_err());
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
                "checkpoint",
                "set_effort",
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
