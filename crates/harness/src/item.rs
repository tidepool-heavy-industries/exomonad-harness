use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Wire-faithful item; unknown fields survive replay.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Item(pub Value);

/// One hash identifies identical canonical item bytes across request branches.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ItemHash(pub String);
