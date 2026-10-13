//! Provider cache affinity and current thread are issued together before dispatch.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize)]
pub struct SessionIdentity {
    thread: String,
    affinity: String,
}

impl SessionIdentity {
    pub fn standalone(thread: impl Into<String>) -> Self {
        let thread = thread.into();
        Self {
            affinity: thread.clone(),
            thread,
        }
    }

    /// Select the immediate parent's thread, otherwise the exact fork source.
    /// Callers supply owner-issued relationships; this never parses actor paths.
    pub fn from_relationship(
        thread: String,
        parent_thread: Option<String>,
        fork_source_thread: Option<String>,
    ) -> Self {
        let affinity = parent_thread
            .or(fork_source_thread)
            .unwrap_or_else(|| thread.clone());
        Self { thread, affinity }
    }

    pub fn thread(&self) -> &str {
        &self.thread
    }
    pub fn affinity(&self) -> &str {
        &self.affinity
    }
    pub(crate) fn valid(&self) -> bool {
        !self.thread.is_empty() && !self.affinity.is_empty()
    }

    pub(crate) fn cache_key(&self) -> String {
        let key = self
            .affinity
            .strip_prefix("ses_")
            .filter(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .unwrap_or(&self.affinity);
        key.chars().take(64).collect()
    }
}

impl From<String> for SessionIdentity {
    fn from(thread: String) -> Self {
        Self::standalone(thread)
    }
}
impl From<&str> for SessionIdentity {
    fn from(thread: &str) -> Self {
        Self::standalone(thread)
    }
}
impl std::fmt::Display for SessionIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.thread)
    }
}

impl<'de> Deserialize<'de> for SessionIdentity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RetainedIdentity {
            LegacyThread(String),
            Issued { thread: String, affinity: String },
        }
        Ok(match RetainedIdentity::deserialize(deserializer)? {
            RetainedIdentity::LegacyThread(thread) => Self::standalone(thread),
            RetainedIdentity::Issued { thread, affinity } => Self { thread, affinity },
        })
    }
}

impl PartialEq<&str> for SessionIdentity {
    fn eq(&self, other: &&str) -> bool {
        self.thread == *other && self.affinity == *other
    }
}
