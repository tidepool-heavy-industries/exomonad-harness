use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtocolVersion(pub u32);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn version_round_trip() {
        let encoded = serde_json::to_string(&ProtocolVersion(VERSION)).expect("serialize");
        assert_eq!(
            serde_json::from_str::<ProtocolVersion>(&encoded).expect("deserialize"),
            ProtocolVersion(VERSION)
        );
    }
}
