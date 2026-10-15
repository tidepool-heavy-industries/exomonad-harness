use serde::{Deserialize, Serialize};

/// Provider observations. Cache-counter absence is independent of whether both
/// ordinary token counters were reported.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "UsageWire", into = "UsageWire")]
pub struct Usage {
    pub reported: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}

#[derive(Deserialize, Serialize)]
struct UsageWire {
    #[serde(default = "legacy_version")]
    version: u8,
    #[serde(default)]
    reported: bool,
    input_tokens: u64,
    output_tokens: u64,
    #[serde(default)]
    cached_tokens: Option<u64>,
    #[serde(default)]
    cache_write_tokens: Option<u64>,
}

fn legacy_version() -> u8 {
    1
}

impl From<Usage> for UsageWire {
    fn from(value: Usage) -> Self {
        Self {
            version: 2,
            reported: value.reported,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cached_tokens: value.cached_tokens,
            cache_write_tokens: value.cache_write_tokens,
        }
    }
}

impl TryFrom<UsageWire> for Usage {
    type Error = &'static str;

    fn try_from(value: UsageWire) -> Result<Self, Self::Error> {
        let counter = |count: Option<u64>| match value.version {
            // The previous reader defaulted missing provider counters to zero.
            1 => count.filter(|count| *count != 0),
            2 => count,
            _ => unreachable!("version checked before counters"),
        };
        if !matches!(value.version, 1 | 2) {
            return Err("unsupported usage observation version");
        }
        Ok(Self {
            reported: value.reported,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cached_tokens: counter(value.cached_tokens),
            cache_write_tokens: counter(value.cache_write_tokens),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_zero_is_unknown_but_positive_counts_survive() {
        for version in [None, Some(1)] {
            let mut wire = json!({"reported":true,"input_tokens":12,"output_tokens":3,
                "cached_tokens":5,"cache_write_tokens":0});
            if let Some(version) = version {
                wire["version"] = json!(version);
            }
            let usage: Usage = serde_json::from_value(wire).unwrap();
            assert_eq!(usage.cached_tokens, Some(5));
            assert_eq!(usage.cache_write_tokens, None);
            assert!(usage.reported);
        }
    }

    #[test]
    fn current_usage_preserves_independent_absence_zero_and_positive_counts() {
        for cached_tokens in [None, Some(0), Some(19)] {
            for cache_write_tokens in [None, Some(0), Some(7)] {
                let usage = Usage {
                    reported: true,
                    input_tokens: 20,
                    output_tokens: 3,
                    cached_tokens,
                    cache_write_tokens,
                };
                let wire = serde_json::to_value(&usage).unwrap();
                assert_eq!(wire["version"], 2);
                assert_eq!(serde_json::from_value::<Usage>(wire).unwrap(), usage);
            }
        }
        let absent: Usage = serde_json::from_value(json!({
            "version":2,"input_tokens":0,"output_tokens":0
        }))
        .unwrap();
        assert_eq!(absent, Usage::default());
    }

    #[test]
    fn unsupported_usage_versions_refuse_decoding() {
        assert!(
            serde_json::from_value::<Usage>(json!({
                "version":3,"input_tokens":0,"output_tokens":0
            }))
            .is_err()
        );
    }
}
