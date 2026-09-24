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
}
