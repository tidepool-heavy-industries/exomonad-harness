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
    sync::{Arc, Mutex},
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
    launch_path: PathBuf,
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
            launch_path: launch.clone(),
        };
        for path in paths {
            if path == launch || path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let record = match load(&path) {
                Ok(Some(record)) => record,
                Ok(None) | Err(LoginError::InvalidCredentials) => continue,
                Err(_) => continue,
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
    pub fn is_available(&self, id: &SavedAccountId) -> bool {
        self.entries
            .iter()
            .find(|(entry, _)| &entry.id == id)
            .is_some_and(|(_, path)| matches!(load(path), Ok(Some(_))))
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
    fn selected_auth(
        launch: &Path,
        id: &SavedAccountId,
    ) -> Result<ChatGptPlanAuth, TransportError> {
        if SavedAccountId::for_path(launch) == *id {
            return Ok(ChatGptPlanAuth::new(launch.to_owned()));
        }
        let entries = std::fs::read_dir(launch.parent().ok_or(TransportError::Authentication)?)
            .map_err(|_| TransportError::Authentication)?;
        for entry in entries {
            let path = entry.map_err(|_| TransportError::Authentication)?.path();
            if path.extension().is_some_and(|ext| ext == "json")
                && SavedAccountId::for_path(&path) == *id
            {
                return Ok(ChatGptPlanAuth::new(path));
            }
        }
        Err(TransportError::Authentication)
    }
}

/// All model consumers in one run share this Store-backed selection. The Store
/// guard is gone before the credential owner locks or refreshes the account.
#[derive(Clone)]
pub struct RunAccountAuth {
    store: Arc<Store>,
    run: String,
    launch_path: PathBuf,
    selection_writer: Arc<Mutex<()>>,
    admission: super::super::NativeTransportAdmission,
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
        let admission = super::super::NativeTransportAdmission::rehydrate(
            super::super::NativeTransportContract {
                route: super::super::ResponsesRoute::ChatGptPlan,
                protocol: super::super::ResponsesProtocol::Standard,
                endpoint: super::super::client::CHATGPT_PLAN_ENDPOINT.into(),
                credential: super::super::CredentialReference::RunAccount {
                    launch_path: accounts.launch_path.clone(),
                    run: run.clone(),
                },
                limits: super::super::InferenceLimits::chatgpt_default(),
            },
        )?;
        Ok(Self {
            admission,
            store,
            run,
            launch_path: accounts.launch_path.clone(),
            selection_writer: Arc::new(Mutex::new(())),
        })
    }
    /// The sole host constructs this owner once and shares its clones. Keep
    /// credential validation, durable selection and browser publication in the
    /// same order even when several browser clients submit concurrently.
    pub fn select_account(
        &self,
        operation: crate::embedding::ClientOperationId,
        publish: impl FnOnce(AccountSelection),
    ) -> Result<crate::server::CommandReceipt, crate::store::StoreError> {
        let _writer = self
            .selection_writer
            .lock()
            .map_err(|_| crate::store::StoreError::InvalidCommandState)?;
        let accounts = match self.accounts() {
            Ok(accounts) => accounts,
            Err(_) => {
                let target = self
                    .store
                    .embedded_command(&self.run, operation)?
                    .ok_or(crate::store::StoreError::InvalidCommandState)?
                    .command
                    .target()
                    .clone();
                return self.store.settle_embedded_command(
                    &self.run,
                    operation,
                    crate::server::CommandReceiptOutcome::Refused {
                        target: Some(target),
                        reason: "Saved accounts are unavailable.".into(),
                    },
                );
            }
        };
        let receipt = self
            .store
            .select_run_account(&self.run, operation, &accounts)?;
        if matches!(
            receipt.outcome,
            crate::server::CommandReceiptOutcome::AccountSelected { .. }
        ) {
            // A retained receipt may describe an earlier selection. Project
            // current authority, never reconstruct it from that operation.
            let selected = self
                .store
                .run_account(&self.run)?
                .ok_or(crate::store::StoreError::InvalidCommandState)?;
            publish(accounts.projection(selected));
        }
        Ok(receipt)
    }

    pub fn accounts(&self) -> Result<SavedAccounts, TransportError> {
        SavedAccounts::beside(self.launch_path.clone()).map_err(|_| TransportError::Authentication)
    }
    pub fn selection(&self) -> Result<AccountSelection, TransportError> {
        let id = self
            .store
            .run_account(&self.run)
            .map_err(|_| TransportError::Authentication)?
            .ok_or(TransportError::Authentication)?;
        Ok(self.accounts()?.projection(id))
    }
}
impl Auth for RunAccountAuth {
    fn admission(&self) -> Result<super::super::NativeTransportAdmission, TransportError> {
        Ok(self.admission.clone())
    }
    fn access(&self) -> Result<String, TransportError> {
        let id = self
            .store
            .run_account(&self.run)
            .map_err(|_| TransportError::Authentication)?
            .ok_or(TransportError::Authentication)?;
        SavedAccounts::selected_auth(&self.launch_path, &id)?.access()
    }
}
