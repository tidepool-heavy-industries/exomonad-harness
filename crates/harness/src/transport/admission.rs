//! One immutable, nonsecret run contract; clones share inference admission.
use super::{ResponsesProtocol, ResponsesRoute, TransportError};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceLimits {
    pub max_output_tokens: Option<u32>,
    pub connect_timeout_secs: u64,
    pub total_timeout_secs: u64,
    pub max_inflight: usize,
}
impl InferenceLimits {
    pub const fn deepseek_default() -> Self {
        Self {
            max_output_tokens: Some(65_536),
            connect_timeout_secs: 15,
            total_timeout_secs: 300,
            max_inflight: 4,
        }
    }
    pub const fn chatgpt_default() -> Self {
        Self {
            max_output_tokens: None,
            connect_timeout_secs: 15,
            total_timeout_secs: 300,
            max_inflight: 4,
        }
    }
    fn validate(self, route: ResponsesRoute) -> Result<(), TransportError> {
        if self.connect_timeout_secs == 0
            || self.total_timeout_secs < self.connect_timeout_secs
            || self.max_inflight == 0
            || self.max_inflight > Semaphore::MAX_PERMITS
            || self.max_output_tokens == Some(0)
            || (route == ResponsesRoute::DeepSeek && self.max_output_tokens.is_none())
        {
            return Err(TransportError::InvalidAdmission);
        }
        Ok(())
    }
}

/// A selected source location, never the credential bytes at that location.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialReference {
    File { path: PathBuf },
    Env { name: String },
    RunAccount { launch_path: PathBuf, run: String },
}
impl std::fmt::Debug for CredentialReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialReference([redacted])")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTransportContract {
    pub route: ResponsesRoute,
    pub protocol: ResponsesProtocol,
    pub endpoint: String,
    pub credential: CredentialReference,
    pub limits: InferenceLimits,
}
impl NativeTransportContract {
    pub fn validate(&self) -> Result<(), TransportError> {
        self.limits.validate(self.route)?;
        let expected = match self.route {
            ResponsesRoute::ChatGptPlan => super::client::CHATGPT_PLAN_ENDPOINT,
            ResponsesRoute::DeepSeek => super::client::DEEPSEEK_ENDPOINT,
        };
        if self.endpoint != expected {
            return Err(TransportError::InvalidAdmission);
        }
        let path = match &self.credential {
            CredentialReference::Env { name } => {
                if self.route != ResponsesRoute::DeepSeek
                    || name.is_empty()
                    || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return Err(TransportError::InvalidAdmission);
                }
                return Ok(());
            }
            CredentialReference::File { path } => path,
            CredentialReference::RunAccount { launch_path, run } => {
                if self.route != ResponsesRoute::ChatGptPlan || run.is_empty() {
                    return Err(TransportError::InvalidAdmission);
                }
                launch_path
            }
        };
        if !path.is_absolute() {
            return Err(TransportError::InvalidAdmission);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct NativeTransportAdmission(Arc<Admitted>);
struct Admitted {
    contract: NativeTransportContract,
    permits: Arc<Semaphore>,
}
impl NativeTransportAdmission {
    /// Revalidates a retained nonsecret contract. Reuse this handle for all run
    /// clients so children and one-shot calls share its concurrency budget.
    pub fn rehydrate(contract: NativeTransportContract) -> Result<Self, TransportError> {
        contract.validate()?;
        Ok(Self(Arc::new(Admitted {
            permits: Arc::new(Semaphore::new(contract.limits.max_inflight)),
            contract,
        })))
    }
    pub fn chatgpt_plan(path: PathBuf) -> Result<Self, TransportError> {
        Self::rehydrate(NativeTransportContract {
            route: ResponsesRoute::ChatGptPlan,
            protocol: ResponsesProtocol::Standard,
            endpoint: super::client::CHATGPT_PLAN_ENDPOINT.into(),
            credential: CredentialReference::File { path },
            limits: InferenceLimits::chatgpt_default(),
        })
    }
    pub fn contract(&self) -> &NativeTransportContract {
        &self.0.contract
    }
    pub(crate) async fn acquire(&self) -> Result<OwnedSemaphorePermit, TransportError> {
        self.0
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| TransportError::InvalidAdmission)
    }
}
