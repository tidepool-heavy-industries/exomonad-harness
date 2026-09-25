use crate::model::Effort;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Wire-faithful item; unknown fields survive replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Item(pub Value);

/// One hash identifies identical canonical item bytes across request branches.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ItemHash(pub String);

impl Item {
    /// Whether this is a positional reasoning-effort update.
    pub fn is_configuration_update(&self) -> bool {
        self.0.get("type").and_then(Value::as_str) == Some("configuration_update")
    }

    /// Read the effort carried by a well-formed settings item.
    pub fn configuration_effort(&self) -> Option<Effort> {
        if !self.is_configuration_update() {
            return None;
        }
        serde_json::from_value(self.0.get("reasoning")?.get("effort")?.clone()).ok()
    }

    /// Construct the sole settings item supported by the harness.
    pub fn configuration_update(effort: Effort) -> Self {
        Self(json!({
            "type": "configuration_update",
            "reasoning": { "effort": effort }
        }))
    }
}
