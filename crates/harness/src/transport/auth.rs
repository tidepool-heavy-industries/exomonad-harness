use super::{Auth, TransportError};
use serde::Deserialize;
use std::path::PathBuf;

pub mod chatgpt;
pub use chatgpt::ChatGptPlanAuth;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct AuthFilePath(PathBuf);

    impl AuthFilePath {
        fn new() -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "harness-codex-auth-{}-{unique}.json",
                std::process::id()
            )))
        }

        fn auth(&self) -> CodexFileAuth {
            CodexFileAuth::new(self.0.clone())
        }
    }

    impl Drop for AuthFilePath {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    fn auth_json(mode: &str, token: &str, account: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "auth_mode": mode,
            "tokens": {
                "access_token": token,
                "account_id": account
            }
        }))
        .unwrap()
    }

    fn assert_authentication_error(
        result: Result<(String, String), TransportError>,
        secrets: &[&str],
    ) {
        let Err(error) = result else {
            panic!("invalid credential file was accepted");
        };
        assert!(matches!(&error, TransportError::Authentication));
        let rendered = format!("{error} {error:?}");
        for secret in secrets {
            assert!(
                !rendered.contains(secret),
                "credential leaked in {rendered:?}"
            );
        }
    }

    #[test]
    fn missing_or_invalid_credentials_fail_without_rewriting_the_file() {
        let missing = AuthFilePath::new();
        assert_authentication_error(missing.auth().access(), &["secret-token", "account-secret"]);
        assert!(
            !missing.0.exists(),
            "read-only auth must not create the file"
        );

        let path = AuthFilePath::new();
        let invalid_files = [
            (
                b"not json with secret-token and account-secret".to_vec(),
                vec!["secret-token", "account-secret"],
            ),
            (
                auth_json("api_key", "secret-token", "account-secret"),
                vec!["secret-token", "account-secret"],
            ),
            (
                auth_json("chatgpt", "", "account-secret"),
                vec!["account-secret"],
            ),
            (
                auth_json("chatgpt", "secret-token", ""),
                vec!["secret-token"],
            ),
        ];

        for (contents, secrets) in invalid_files {
            fs::write(&path.0, &contents).unwrap();
            let before = fs::read(&path.0).unwrap();
            assert_authentication_error(path.auth().access(), &secrets);
            assert_eq!(fs::read(&path.0).unwrap(), before);
        }
    }

    #[test]
    fn each_access_reads_the_current_credentials_and_never_mutates_them() {
        let path = AuthFilePath::new();
        let auth = path.auth();
        let first = auth_json("chatgpt", "old-token", "old-account");
        fs::write(&path.0, &first).unwrap();
        assert_eq!(
            auth.access().unwrap(),
            ("old-token".into(), "old-account".into())
        );
        assert_eq!(fs::read(&path.0).unwrap(), first);

        // Codex owns credential refresh; a later request sees its replacement.
        let refreshed = auth_json("chatgpt", "new-token", "new-account");
        fs::write(&path.0, &refreshed).unwrap();
        assert_eq!(
            auth.access().unwrap(),
            ("new-token".into(), "new-account".into())
        );
        assert_eq!(fs::read(&path.0).unwrap(), refreshed);
    }
}
