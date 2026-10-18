//! DeepSeek API-key access from one explicitly selected reference, never fallback.
use super::super::{
    Auth, CredentialReference, InferenceLimits, NativeTransportAdmission, NativeTransportContract,
    ResponsesProtocol, ResponsesRoute, TransportError,
};
use std::{io::Read, path::PathBuf};

#[derive(Clone)]
pub struct DeepSeekAuth {
    admission: NativeTransportAdmission,
}
impl DeepSeekAuth {
    pub fn new(path: PathBuf, limits: InferenceLimits) -> Result<Self, TransportError> {
        Self::from_reference(CredentialReference::File { path }, limits)
    }
    pub fn from_reference(
        credential: CredentialReference,
        limits: InferenceLimits,
    ) -> Result<Self, TransportError> {
        Self::from_admission(NativeTransportAdmission::rehydrate(
            NativeTransportContract {
                route: ResponsesRoute::DeepSeek,
                protocol: ResponsesProtocol::Standard,
                endpoint: super::super::client::DEEPSEEK_ENDPOINT.into(),
                credential,
                limits,
            },
        )?)
    }
    pub fn from_admission(admission: NativeTransportAdmission) -> Result<Self, TransportError> {
        admission.contract().validate()?;
        if admission.contract().route != ResponsesRoute::DeepSeek {
            return Err(TransportError::InvalidAdmission);
        }
        Ok(Self { admission })
    }
}
impl Auth for DeepSeekAuth {
    fn admission(&self) -> Result<NativeTransportAdmission, TransportError> {
        Ok(self.admission.clone())
    }
    fn access(&self) -> Result<String, TransportError> {
        read_api_key(
            &self.admission.contract().credential,
            ResponsesRoute::DeepSeek,
        )
    }
}

/// Shared credential-source validation for API-key providers; no fallback.
pub(super) fn read_api_key(
    credential: &CredentialReference,
    route: ResponsesRoute,
) -> Result<String, TransportError> {
    use super::super::CredentialFailure as Failure;
    let fail = |reason| TransportError::CredentialUnavailable { route, reason };
    let secret = match credential {
        CredentialReference::Env { name } => std::env::var(name).map_err(|error| {
            fail(match error {
                std::env::VarError::NotPresent => Failure::Missing,
                _ => Failure::Invalid,
            })
        })?,
        CredentialReference::File { path } => {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(path)
                .map_err(|error| {
                    fail(match error.kind() {
                        std::io::ErrorKind::NotFound => Failure::Missing,
                        _ => Failure::Unreadable,
                    })
                })?;
            let metadata = file.metadata().map_err(|_| fail(Failure::Unreadable))?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o777 != 0o600
                || metadata.nlink() != 1
            {
                return Err(fail(Failure::Unsafe));
            }
            let mut bytes = Vec::new();
            (&mut file)
                .take(16_385)
                .read_to_end(&mut bytes)
                .map_err(|_| fail(Failure::Unreadable))?;
            String::from_utf8(bytes).map_err(|_| fail(Failure::Invalid))?
        }
        CredentialReference::RunAccount { .. } => return Err(TransportError::InvalidAdmission),
    };
    // A single trailing line ending permits conventional private key files.
    let key = secret
        .strip_suffix("\r\n")
        .or_else(|| secret.strip_suffix('\n'))
        .unwrap_or(&secret);
    if key.is_empty()
        || key.len() > 16_384
        || !key.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    {
        return Err(fail(Failure::Invalid));
    }
    Ok(key.to_owned())
}

#[cfg(test)]
#[path = "deepseek_tests.rs"]
mod tests;
