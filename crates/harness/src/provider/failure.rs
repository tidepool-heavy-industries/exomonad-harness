use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

pub const MAX_TOOL_FAILURE_METADATA_BYTES: usize = 8192;
pub const MAX_TOOL_FAILURE_METADATA_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataOmission {
    SizeLimit,
    DepthLimit,
}

/// A failed tool's human diagnostic and optional provider-owned classification.
/// Metadata is descriptive: scheduling and cancellation never parse it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolFailure {
    message: String,
    provider_context: bool,
    metadata: Option<Value>,
    metadata_omitted: Option<MetadataOmission>,
}

impl ToolFailure {
    pub(super) fn with_tool_prefix(mut self) -> Self {
        if !self.provider_context {
            self.message = format!("tool failed: {}", self.message);
            self.provider_context = true;
        }
        self
    }

    pub(super) fn fmt_provider(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.provider_context {
            write!(output, "{}", self.message)
        } else {
            write!(output, "tool failed: {}", self.message)
        }
    }

    pub fn with_metadata(message: impl Into<String>, metadata: Value) -> Self {
        let mut failure = Self::from(message.into());
        match admit_metadata(&metadata) {
            Ok(()) => failure.metadata = Some(metadata),
            Err(reason) => failure.metadata_omitted = Some(reason),
        }
        failure
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn metadata(&self) -> Option<&Value> {
        self.metadata.as_ref()
    }

    pub fn metadata_omitted(&self) -> Option<MetadataOmission> {
        self.metadata_omitted
    }

    /// Preserve the existing error text in model output and durable Items.
    pub fn output_value(&self) -> Value {
        let mut output = serde_json::json!({ "error": self.message });
        if let Some(metadata) = &self.metadata {
            output["failure"] = metadata.clone();
        } else if let Some(reason) = self.metadata_omitted {
            output["failure"] = serde_json::json!({ "metadata_omitted": reason });
        }
        output
    }
}

impl From<String> for ToolFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            provider_context: false,
            metadata: None,
            metadata_omitted: None,
        }
    }
}

impl From<&str> for ToolFailure {
    fn from(message: &str) -> Self {
        Self::from(message.to_owned())
    }
}

impl std::fmt::Display for ToolFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(output)
    }
}

// Bare text-only wire values retain their existing encoding. Admission also applies
// when a structured terminal result crosses a serde boundary.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum FailureWire {
    Text(String),
    Structured {
        message: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        provider_context: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata_omitted: Option<MetadataOmission>,
    },
}

impl Serialize for ToolFailure {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !self.provider_context && self.metadata.is_none() && self.metadata_omitted.is_none() {
            self.message.serialize(serializer)
        } else {
            FailureWire::Structured {
                message: self.message.clone(),
                provider_context: self.provider_context,
                metadata: self.metadata.clone(),
                metadata_omitted: self.metadata_omitted,
            }
            .serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for ToolFailure {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match FailureWire::deserialize(deserializer)? {
            FailureWire::Text(message) => message.into(),
            FailureWire::Structured {
                message,
                provider_context,
                metadata: Some(metadata),
                ..
            } => {
                let mut failure = Self::with_metadata(message, metadata);
                failure.provider_context = provider_context;
                failure
            }
            FailureWire::Structured {
                message,
                provider_context,
                metadata: None,
                metadata_omitted,
            } => Self {
                message,
                provider_context,
                metadata: None,
                metadata_omitted,
            },
        })
    }
}

fn admit_metadata(metadata: &Value) -> Result<(), MetadataOmission> {
    let mut pending = vec![(metadata, 1usize)];
    let mut visited = 0;
    while let Some((value, depth)) = pending.pop() {
        if depth > MAX_TOOL_FAILURE_METADATA_DEPTH {
            return Err(MetadataOmission::DepthLimit);
        }
        visited += 1;
        let children = match value {
            Value::Array(values) => values.len(),
            Value::Object(values) => values.len(),
            _ => 0,
        };
        if visited + pending.len() + children > MAX_TOOL_FAILURE_METADATA_BYTES {
            return Err(MetadataOmission::SizeLimit);
        }
        match value {
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)))
            }
            _ => {}
        }
    }
    struct Budget(usize);
    impl std::io::Write for Budget {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(std::io::Error::other("tool failure metadata size limit"));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Budget(MAX_TOOL_FAILURE_METADATA_BYTES), metadata)
        .map_err(|_| MetadataOmission::SizeLimit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_failure_metadata_limits_apply_to_construction_and_serde() {
        let oversized = serde_json::json!({ "cause": "x".repeat(MAX_TOOL_FAILURE_METADATA_BYTES) });
        let failure = ToolFailure::with_metadata("original failure", oversized.clone());
        assert_eq!(failure.message(), "original failure");
        assert_eq!(
            failure.metadata_omitted(),
            Some(MetadataOmission::SizeLimit)
        );
        assert_eq!(
            failure.output_value()["failure"]["metadata_omitted"],
            "size_limit"
        );
        let from_wire: ToolFailure = serde_json::from_value(serde_json::json!({
            "message": "original failure", "metadata": oversized
        }))
        .unwrap();
        assert_eq!(from_wire, failure);

        let mut nested = Value::Null;
        for _ in 0..MAX_TOOL_FAILURE_METADATA_DEPTH {
            nested = serde_json::json!({ "cause": nested });
        }
        let failure = ToolFailure::with_metadata("deep failure", nested);
        assert_eq!(
            failure.metadata_omitted(),
            Some(MetadataOmission::DepthLimit)
        );
        assert_eq!(
            serde_json::from_str::<ToolFailure>(&serde_json::to_string(&failure).unwrap()).unwrap(),
            failure
        );
    }

    #[test]
    fn tool_failure_terminal_roundtrip_preserves_metadata_and_text_encoding() {
        use crate::provider::{CancellationAcknowledgment, ProviderError};
        let text = ToolFailure::from("plain failure");
        assert_eq!(serde_json::to_value(&text).unwrap(), "plain failure");
        assert_eq!(
            serde_json::from_value::<ToolFailure>(Value::String("plain failure".into())).unwrap(),
            text
        );
        let metadata = serde_json::json!({ "class": "input-rejected", "phase": "compile", "cause": { "kind": "input_rejected" } });
        let failure = ProviderError::Tool(ToolFailure::with_metadata(
            "unchanged cause",
            metadata.clone(),
        ))
        .into_tool_failure();
        assert_eq!(failure.message(), "tool failed: unchanged cause");
        assert_eq!(
            failure.output_value(),
            serde_json::json!({ "error": "tool failed: unchanged cause", "failure": metadata })
        );
        assert_eq!(
            ProviderError::Tool(failure.clone()).to_string(),
            "tool failed: unchanged cause"
        );
        assert_eq!(
            ProviderError::Tool(failure.clone()).into_tool_failure(),
            failure
        );
        let terminal = CancellationAcknowledgment::Completed(Err(failure));
        assert_eq!(
            serde_json::from_value::<CancellationAcknowledgment>(
                serde_json::to_value(&terminal).unwrap()
            )
            .unwrap(),
            terminal
        );
    }
}
