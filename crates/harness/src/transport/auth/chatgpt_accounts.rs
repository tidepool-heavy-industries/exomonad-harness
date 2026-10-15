//! Saved native registrations. Browser identities never resolve arbitrary paths.
use super::{ChatGptPlanAuth, LoginError, load};
use crate::{
    store::Store,
    transport::{Auth, TransportError},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Stable credential-owner-issued identity; its representation is not a path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct SavedAccountId(String);
impl SavedAccountId {
    fn for_path(path: &Path) -> Self {
        use std::os::unix::ffi::OsStrExt;
        Self(format!("{:x}", Sha256::digest(path.as_os_str().as_bytes())))
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedAccount {
    pub id: SavedAccountId,
    pub label: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountSelection {
    pub selected_id: SavedAccountId,
    pub accounts: Vec<SavedAccount>,
}
#[derive(Clone)]
pub struct SavedAccounts {
    entries: Vec<(SavedAccount, PathBuf)>,
    launch: SavedAccountId,
}
impl SavedAccounts {
    /// The launch path is trusted configuration. Other entries must be safe,
    /// valid native records in that same directory. No OAuth flow is performed.
    pub fn beside(launch: PathBuf) -> Result<Self, LoginError> {
        if !launch.is_absolute() {
            return Err(LoginError::Storage);
        }
        let parent = launch.parent().ok_or(LoginError::Storage)?;
        let mut paths = Vec::new();
        match std::fs::read_dir(parent) {
            Ok(entries) => {
                for entry in entries {
                    // Bound filesystem discovery before loading records. This is a
                    // startup work budget, not an account format constraint.
                    if paths.len() >= 128 {
                        return Err(LoginError::Storage);
                    }
                    let entry = entry.map_err(|_| LoginError::Storage)?;
                    paths.push(entry.path());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(LoginError::Storage),
        }
        paths.sort();
        let launch_id = SavedAccountId::for_path(&launch);
        let mut accounts = Self {
            entries: vec![(
                SavedAccount {
                    id: launch_id.clone(),
                    label: "Launch account".into(),
                },
                launch.clone(),
            )],
            launch: launch_id,
        };
        for path in paths {
            if path == launch || path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let record = match load(&path) {
                Ok(Some(record)) => record,
                Ok(None) | Err(LoginError::InvalidCredentials) => continue,
                Err(error) => return Err(error),
            };
            let label = record
                .email
                .filter(|email| {
                    email.len() <= 120
                        && email.contains('@')
                        && email.chars().all(|c| !c.is_control())
                })
                .unwrap_or_else(|| format!("Saved account {}", accounts.entries.len()));
            accounts.entries.push((
                SavedAccount {
                    id: SavedAccountId::for_path(&path),
                    label,
                },
                path,
            ));
        }
        Ok(accounts)
    }
    pub fn launch_id(&self) -> &SavedAccountId {
        &self.launch
    }
    pub fn contains(&self, id: &SavedAccountId) -> bool {
        self.entries.iter().any(|(entry, _)| &entry.id == id)
    }
    pub fn projection(&self, selected_id: SavedAccountId) -> AccountSelection {
        AccountSelection {
            selected_id,
            accounts: self
                .entries
                .iter()
                .map(|(entry, _)| entry.clone())
                .collect(),
        }
    }
    fn auth(&self, id: &SavedAccountId) -> Result<ChatGptPlanAuth, TransportError> {
        self.entries
            .iter()
            .find(|(entry, _)| &entry.id == id)
            .map(|(_, path)| ChatGptPlanAuth::new(path.clone()))
            .ok_or(TransportError::Authentication)
    }
}

/// All model consumers in one run share this Store-backed selection. The Store
/// guard is gone before the credential owner locks or refreshes the account.
#[derive(Clone)]
pub struct RunAccountAuth {
    store: Arc<Store>,
    run: String,
    accounts: SavedAccounts,
}
impl RunAccountAuth {
    pub fn new(
        store: Arc<Store>,
        run: String,
        accounts: SavedAccounts,
    ) -> Result<Self, TransportError> {
        let selected = store
            .initialize_run_account(&run, accounts.launch_id())
            .map_err(|_| TransportError::Authentication)?;
        if !accounts.contains(&selected) {
            return Err(TransportError::Authentication);
        }
        Ok(Self {
            store,
            run,
            accounts,
        })
    }
    pub fn accounts(&self) -> &SavedAccounts {
        &self.accounts
    }
    pub fn selection(&self) -> Result<AccountSelection, TransportError> {
        let id = self
            .store
            .run_account(&self.run)
            .map_err(|_| TransportError::Authentication)?
            .ok_or(TransportError::Authentication)?;
        Ok(self.accounts.projection(id))
    }
}
impl Auth for RunAccountAuth {
    fn access(&self) -> Result<String, TransportError> {
        let id = self
            .store
            .run_account(&self.run)
            .map_err(|_| TransportError::Authentication)?
            .ok_or(TransportError::Authentication)?;
        self.accounts.auth(&id)?.access()
    }
}
