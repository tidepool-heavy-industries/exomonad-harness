use super::{Auth, TransportError};
use serde::Deserialize;
use std::path::PathBuf;

/// Reads Codex-owned credentials for each request without writing or
/// refreshing them. The returned strings live only for the request lifetime.
#[derive(Clone)]
pub struct CodexFileAuth {
    path: PathBuf,
}

impl CodexFileAuth {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> Result<PathBuf, TransportError> {
        let home = std::env::var_os("HOME").ok_or(TransportError::Authentication)?;
        Ok(PathBuf::from(home).join(".codex/auth.json"))
    }
}

#[derive(Deserialize)]
struct AuthFile {
    auth_mode: String,
    tokens: AuthTokens,
}

#[derive(Deserialize)]
struct AuthTokens {
    access_token: String,
    account_id: String,
}

impl Auth for CodexFileAuth {
    fn access(&self) -> Result<(String, String), TransportError> {
        let bytes = std::fs::read(&self.path).map_err(|_| TransportError::Authentication)?;
        let file: AuthFile =
            serde_json::from_slice(&bytes).map_err(|_| TransportError::Authentication)?;
        if file.auth_mode != "chatgpt"
            || file.tokens.access_token.is_empty()
            || file.tokens.account_id.is_empty()
        {
            return Err(TransportError::Authentication);
        }
        Ok((file.tokens.access_token, file.tokens.account_id))
    }
}
