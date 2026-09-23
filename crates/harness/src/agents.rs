use crate::model::AgentPath;
use serde::{Deserialize, Serialize};

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
