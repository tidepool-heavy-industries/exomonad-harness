use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The provider may omit usage entirely or explicitly return JSON null. Neither
/// establishes zero consumption. Observed JSON retains unrecognized fields.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum RawUsage {
    #[default]
    Unavailable,
    Observed(Value),
}

/// Independently reported provider counters. Reasoning is a subset of output,
/// not another billable output category.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(try_from = "UsageWire", into = "UsageWire")]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cache_miss_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub raw: RawUsage,
}

impl Usage {
    pub fn has_reported_totals(&self) -> bool {
        self.input_tokens.is_some() && self.output_tokens.is_some()
    }

    /// Lower-bound accounting for known token counters. This is not complete
    /// usage or a bill; callers must separately retain missing-counter evidence.
    pub fn known_token_subtotal(&self) -> u64 {
        self.input_tokens
            .into_iter()
            .chain(self.output_tokens)
            .fold(0u64, u64::saturating_add)
    }

    /// Extract only provider observations; absent or malformed counters stay
    /// unknown. No token count is inferred from another counter or request.
    pub fn from_response_usage(response: &Value) -> Self {
        let Some(usage) = response.get("usage") else {
            return Self::default();
        };
        let input_details = &usage["input_tokens_details"];
        let output_details = &usage["output_tokens_details"];
        Self {
            input_tokens: usage.get("input_tokens").and_then(Value::as_u64),
            output_tokens: usage.get("output_tokens").and_then(Value::as_u64),
            cached_tokens: input_details.get("cached_tokens").and_then(Value::as_u64),
            cache_miss_tokens: usage
                .get("prompt_cache_miss_tokens")
                .and_then(Value::as_u64),
            cache_write_tokens: input_details
                .get("cache_write_tokens")
                .and_then(Value::as_u64),
            reasoning_tokens: output_details
                .get("reasoning_tokens")
                .and_then(Value::as_u64),
            raw: RawUsage::Observed(usage.clone()),
        }
    }
}

#[derive(Deserialize, Serialize)]
struct UsageWire {
    #[serde(default = "legacy_version")]
    version: u8,
    #[serde(default)]
    reported: bool,
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
    #[serde(default)]
    cached_tokens: Option<u64>,
    #[serde(default)]
    cache_miss_tokens: Option<u64>,
    #[serde(default)]
    cache_write_tokens: Option<u64>,
    #[serde(default)]
    reasoning_tokens: Option<u64>,
    #[serde(default)]
    raw: RawUsage,
}

fn legacy_version() -> u8 {
    1
}

impl From<Usage> for UsageWire {
    fn from(value: Usage) -> Self {
        Self {
            version: 3,
            reported: value.has_reported_totals(),
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
            cached_tokens: value.cached_tokens,
            cache_miss_tokens: value.cache_miss_tokens,
            cache_write_tokens: value.cache_write_tokens,
            reasoning_tokens: value.reasoning_tokens,
            raw: value.raw,
        }
    }
}

impl TryFrom<UsageWire> for Usage {
    type Error = &'static str;
    fn try_from(value: UsageWire) -> Result<Self, Self::Error> {
        match value.version {
            1 | 2 => {
                // Legacy zero-filled totals cannot establish independent
                // presence. Positive totals survived provider validation even
                // when the other counter was absent and reported was false.
                if value.reported && (value.input_tokens.is_none() || value.output_tokens.is_none())
                {
                    return Err("reported legacy usage lacks totals");
                }
                Ok(Self {
                    input_tokens: value
                        .input_tokens
                        .filter(|count| value.reported || *count != 0),
                    output_tokens: value
                        .output_tokens
                        .filter(|count| value.reported || *count != 0),
                    cached_tokens: value
                        .cached_tokens
                        .filter(|count| value.version == 2 || *count != 0),
                    cache_write_tokens: value
                        .cache_write_tokens
                        .filter(|count| value.version == 2 || *count != 0),
                    // Old wire formats did not retain these provider facts.
                    ..Self::default()
                })
            }
            3 => Ok(Self {
                input_tokens: value.input_tokens,
                output_tokens: value.output_tokens,
                cached_tokens: value.cached_tokens,
                cache_miss_tokens: value.cache_miss_tokens,
                cache_write_tokens: value.cache_write_tokens,
                reasoning_tokens: value.reasoning_tokens,
                raw: value.raw,
            }),
            _ => Err("unsupported usage observation version"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn legacy_usage_migration_never_recreates_unreported_totals() {
        for version in [1, 2] {
            for reported in [false, true] {
                let usage: Usage = serde_json::from_value(json!({"version":version,
                    "reported":reported,"input_tokens":12,"output_tokens":0,
                    "cached_tokens":0,"cache_write_tokens":5}))
                .unwrap();
                assert_eq!(usage.input_tokens, Some(12));
                assert_eq!(usage.output_tokens, reported.then_some(0));
                assert_eq!(usage.cached_tokens, (version == 2).then_some(0));
                assert_eq!(usage.cache_write_tokens, Some(5));
                assert_eq!(usage.raw, RawUsage::Unavailable);
                assert_eq!(usage.reasoning_tokens, None);
            }
        }
        let absent: Usage =
            serde_json::from_value(json!({"input_tokens":0,"output_tokens":0})).unwrap();
        assert_eq!(absent, Usage::default());
    }

    #[test]
    fn provider_usage_preserves_presence_unknowns_and_independent_counters() {
        assert_eq!(
            Usage::from_response_usage(&json!({})).raw,
            RawUsage::Unavailable
        );
        let null = Usage::from_response_usage(&json!({"usage":null}));
        assert_eq!(null.raw, RawUsage::Observed(Value::Null));
        for input in [None, Some(0), Some(12)] {
            for output in [None, Some(0), Some(7)] {
                let mut raw = json!({"future":{"count":null}, "input_tokens_details":{"cached_tokens":0},
                    "output_tokens_details":{"reasoning_tokens":2}});
                if let Some(input) = input {
                    raw["input_tokens"] = json!(input);
                }
                if let Some(output) = output {
                    raw["output_tokens"] = json!(output);
                }
                let usage = Usage::from_response_usage(&json!({"usage":raw}));
                assert_eq!(usage.input_tokens, input);
                assert_eq!(usage.output_tokens, output);
                assert_eq!(usage.cached_tokens, Some(0));
                assert_eq!(usage.cache_write_tokens, None);
                assert_eq!(usage.reasoning_tokens, Some(2));
                assert_eq!(usage.raw, RawUsage::Observed(raw));
                let wire = serde_json::to_value(&usage).unwrap();
                assert_eq!(wire["version"], 3);
                assert_eq!(serde_json::from_value::<Usage>(wire).unwrap(), usage);
            }
        }
        assert_eq!(
            serde_json::from_value::<Usage>(serde_json::to_value(&null).unwrap()).unwrap(),
            null
        );
        let malformed =
            Usage::from_response_usage(&json!({"usage":{"input_tokens":"0","output_tokens":-1}}));
        assert_eq!(malformed.input_tokens, None);
        assert_eq!(malformed.output_tokens, None);
        assert!(matches!(malformed.raw, RawUsage::Observed(_)));
    }

    #[test]
    fn raw_usage_survives_store_event_reopen_without_inventing_totals() {
        let path =
            std::env::temp_dir().join(format!("harness-usage-{}.sqlite", uuid::Uuid::new_v4()));
        let examples = [
            serde_json::json!({}),
            serde_json::json!({"usage":null}),
            serde_json::json!({"usage":{"input_tokens":0,"output_tokens_details":{"reasoning_tokens":0},"future":{"value":null}}}),
        ];
        let expected = examples
            .iter()
            .map(Usage::from_response_usage)
            .collect::<Vec<_>>();
        {
            let store = crate::store::Store::open(&path).unwrap();
            for usage in &expected {
                store
                    .record_event(
                        None,
                        "responses_usage",
                        &serde_json::to_value(usage).unwrap(),
                    )
                    .unwrap();
            }
        }
        let reopened = crate::store::Store::open(&path).unwrap();
        let actual = reopened
            .events(None)
            .unwrap()
            .into_iter()
            .map(|event| serde_json::from_str::<Usage>(&event.payload).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(actual, expected);
        assert!(actual.iter().all(|usage| !usage.has_reported_totals()));
        assert_eq!(actual[2].input_tokens, Some(0));
        assert_eq!(actual[2].output_tokens, None);
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn unsupported_usage_versions_refuse_decoding() {
        assert!(serde_json::from_value::<Usage>(json!({"version":4})).is_err());
    }
}
