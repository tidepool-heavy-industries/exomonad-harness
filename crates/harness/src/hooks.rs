//! Typed before-request hook contract. This slice supports Send only; tool
//! restriction and item injection are not represented until implemented.
use crate::{item::Item, model::Effort};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestPlan {
    pub items: Vec<Item>,
    pub tools_allowed: Vec<Value>,
    pub effort: Effort,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum BeforeRequestDecision {
    #[default]
    Send,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BeforeRequestResult {
    pub decision: BeforeRequestDecision,
    /// Provider-opaque; the harness persists but never interprets this value.
    pub evidence: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn send_plan_and_opaque_evidence_cross_serde_boundary() {
        let plan = RequestPlan {
            items: vec![Item(json!({"type":"message","content":"hello"}))],
            tools_allowed: vec![json!({"type":"function","name":"echo"})],
            effort: Effort::Medium,
        };
        let encoded = serde_json::to_value(&plan).unwrap();
        assert_eq!(serde_json::from_value::<RequestPlan>(encoded).unwrap(), plan);
        let result = BeforeRequestResult {
            decision: BeforeRequestDecision::Send,
            evidence: Some(json!({"source":"deterministic-hook"})),
        };
        assert_eq!(
            serde_json::from_value::<BeforeRequestResult>(serde_json::to_value(&result).unwrap())
                .unwrap(),
            result
        );
    }
}
