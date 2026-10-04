//! Exact browser integers. JSON numbers cannot represent every native counter.
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use std::borrow::Cow;

/// Unsigned native integer represented by its canonical base-ten string.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct WireU64(u64);

/// Signed native integer represented by its canonical base-ten string.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct WireI64(i64);

// Each alternative chooses the first digit below the bound, then permits all
// remaining digits. This expresses the complete native range in JSON Schema,
// so standalone validators need no custom numeric format or ambient callback.
fn positive_range(bound: &str) -> String {
    let digits = bound.as_bytes();
    let mut alternatives = vec![format!("[1-9][0-9]{{0,{}}}", digits.len() - 2)];
    for (index, digit) in digits.iter().enumerate() {
        let low = if index == 0 { b'1' } else { b'0' };
        if *digit > low {
            let choice = if *digit == low + 1 {
                char::from(low).to_string()
            } else {
                format!("[{}-{}]", char::from(low), char::from(*digit - 1))
            };
            alternatives.push(format!(
                "{}{}[0-9]{{{}}}",
                &bound[..index],
                choice,
                digits.len() - index - 1
            ));
        }
    }
    alternatives.push(bound.to_owned());
    alternatives.join("|")
}

macro_rules! decimal {
    ($name:ident, $native:ty, $pattern:expr) => {
        impl $name {
            pub const fn get(self) -> $native { self.0 }
        }
        impl From<$native> for $name {
            fn from(value: $native) -> Self { Self(value) }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(&self.0)
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let encoded = String::deserialize(deserializer)?;
                let value = encoded.parse::<$native>().map_err(D::Error::custom)?;
                if value.to_string() != encoded {
                    return Err(D::Error::custom("expected a canonical decimal integer"));
                }
                Ok(Self(value))
            }
        }
        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> { stringify!($name).into() }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                schemars::json_schema!({"type":"string", "pattern": $pattern})
            }
        }
    };
}
decimal!(
    WireU64,
    u64,
    format!(
        r"^(?:0|{})$(?![\s\S])",
        positive_range("18446744073709551615")
    )
);
decimal!(
    WireI64,
    i64,
    format!(
        r"^(?:0|{}|-(?:{}))$(?![\s\S])",
        positive_range("9223372036854775807"),
        positive_range("9223372036854775808")
    )
);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn browser_integers_preserve_native_extremes_and_refuse_lossy_spellings() {
        for value in [0, 1, 9_007_199_254_740_993, u64::MAX] {
            let wire = WireU64::from(value);
            let encoded = serde_json::to_value(wire).unwrap();
            assert_eq!(encoded, json!(value.to_string()));
            assert_eq!(serde_json::from_value::<WireU64>(encoded).unwrap(), wire);
        }
        for value in [i64::MIN, -9_007_199_254_740_993, -1, 0, i64::MAX] {
            let wire = WireI64::from(value);
            let encoded = serde_json::to_value(wire).unwrap();
            assert_eq!(encoded, json!(value.to_string()));
            assert_eq!(serde_json::from_value::<WireI64>(encoded).unwrap(), wire);
        }
        for spelling in [
            "1\n",
            "1\r",
            "1 ",
            " 1",
            "",
            "01",
            "+1",
            "-0",
            " 1",
            "1 ",
            "1.0",
            "1e3",
            "18446744073709551616",
        ] {
            assert!(
                serde_json::from_value::<WireU64>(json!(spelling)).is_err(),
                "{spelling}"
            );
        }
        for spelling in [
            "-1\n",
            "1\r",
            "1 ",
            " 1",
            "01",
            "+1",
            "-0",
            "-01",
            "9223372036854775808",
            "-9223372036854775809",
        ] {
            assert!(
                serde_json::from_value::<WireI64>(json!(spelling)).is_err(),
                "{spelling}"
            );
        }
        assert!(serde_json::from_value::<WireU64>(json!(1)).is_err());
        assert!(serde_json::from_value::<WireI64>(json!(-1)).is_err());
    }
}
