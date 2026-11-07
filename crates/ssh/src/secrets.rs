//! Saved passwords and key passphrases: one keychain item per host alias
//! (a small JSON object), so renaming or forgetting a host is one operation.
//!
//! The keychain is optional. When it fails (Linux without a Secret Service,
//! a locked or broken store) secrets are kept in memory for this run only
//! and connections never fail because of it.
//!
//! `secrets-index.json` (next to hosts.json) lists the aliases that have a
//! keychain item, so listing hosts never opens the keychain. It holds no
//! secrets; whenever the keychain is actually read it wins and the index is
//! corrected.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use vpsfiles_vfs::{Result, VfsError};

/// A secret store keyed by account name. Implementations may block (D-Bus,
/// an unlock dialog); callers run them on the blocking pool.
pub trait SecretStore: Send + Sync + fmt::Debug + 'static {
    /// `Ok(None)` = nothing saved under `account`.
    fn get(&self, account: &str) -> Result<Option<String>>;
    fn set(&self, account: &str, secret: &str) -> Result<()>;
    /// Deleting a missing entry is not an error.
    fn delete(&self, account: &str) -> Result<()>;
}

/// The OS keychain: Secret Service on Linux, Credential Manager on Windows,
/// Keychain on macOS.
#[derive(Debug, Clone)]
pub struct OsKeychain {
    service: String,
}

impl OsKeychain {
    pub fn new(service: impl Into<String>) -> Self {
        Self { service: service.into() }
    }

    fn entry(&self, account: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, account).map_err(keychain_error)
    }
}

fn keychain_error(e: keyring::Error) -> VfsError {
    VfsError::Other(format!("keychain: {e}"))
}

impl SecretStore for OsKeychain {
    fn get(&self, account: &str) -> Result<Option<String>> {
        match self.entry(account)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_error(e)),
        }
    }

    fn set(&self, account: &str, secret: &str) -> Result<()> {
        self.entry(account)?.set_password(secret).map_err(keychain_error)
    }

    fn delete(&self, account: &str) -> Result<()> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_error(e)),
        }
    }
}

/// What is saved for one host.
#[derive(Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HostSecrets {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// Key file path → passphrase.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub passphrases: BTreeMap<String, String>,
}

impl HostSecrets {
    fn is_empty(&self) -> bool {
        self.password.is_none() && self.passphrases.is_empty()
    }
}

impl fmt::Debug for HostSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostSecrets")
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("passphrases", &self.passphrases.keys().collect::<Vec<_>>())
            .finish()
    }
}

fn account(alias: &str) -> String {
    format!("host:{alias}")
}

#[derive(Default, Serialize, Deserialize)]
struct IndexFile {
    version: u32,
    aliases: BTreeSet<String>,
}

/// Aliases the index says have a saved secret; empty if it can't be read.
pub(crate) fn read_index(path: &Path) -> BTreeSet<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<IndexFile>(&text).map(|f| f.aliases).unwrap_or_else(|e| {
            tracing::warn!("{}: {e}", path.display());
            BTreeSet::new()
        }),
        Err(_) => BTreeSet::new(),
    }
}

/// [`SecretStore`] plus the in-memory fallback; all calls run off the
/// async threads.
pub(crate) struct Secrets {
    store: Arc<dyn SecretStore>,
    index: PathBuf,
    index_lock: Mutex<()>,
    /// Secrets the store refused to save (keychain unavailable).
    fallback: Mutex<HashMap<String, HostSecrets>>,
    /// Serializes read-modify-write of an item.
    write: tokio::sync::Mutex<()>,
    warned: AtomicBool,
}

impl Secrets {
    pub fn new(store: Arc<dyn SecretStore>, index: PathBuf) -> Self {
        Self {
            store,
            index,
            index_lock: Mutex::new(()),
            fallback: Mutex::new(HashMap::new()),
            write: tokio::sync::Mutex::new(()),
            warned: AtomicBool::new(false),
        }
    }

    fn warn(&self, e: &VfsError) {
        if !self.warned.swap(true, Ordering::Relaxed) {
            tracing::warn!("{e}; saved passwords are kept for this session only");
        } else {
            tracing::debug!("{e}");
        }
    }

    /// Record whether `alias` has a keychain item (no-op if unchanged).
    fn mark(&self, alias: &str, saved: bool) {
        let _lock = self.index_lock.lock().unwrap();
        let mut aliases = read_index(&self.index);
        let changed = if saved { aliases.insert(alias.to_string()) } else { aliases.remove(alias) };
        if !changed {
            return;
        }
        let json = serde_json::to_vec_pretty(&IndexFile { version: 1, aliases }).expect("index serializes");
        if let Err(e) = crate::hosts::write_atomic(&self.index, &json) {
            tracing::warn!("{e}");
        }
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn SecretStore) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || f(&*store))
            .await
            .unwrap_or_else(|e| Err(VfsError::Other(format!("keychain task failed: {e}"))))
    }

    /// Everything saved for `alias`; empty when nothing is or the store fails.
    pub async fn load(&self, alias: &str) -> HostSecrets {
        let acct = account(alias);
        match self.blocking(move |s| s.get(&acct)).await {
            Ok(Some(json)) => {
                self.mark(alias, true);
                serde_json::from_str(&json).unwrap_or_else(|e| {
                    tracing::warn!("keychain item for {alias} is unreadable ({e}); ignoring it");
                    HostSecrets::default()
                })
            }
            Ok(None) => {
                self.mark(alias, false);
                self.fallback.lock().unwrap().get(alias).cloned().unwrap_or_default()
            }
            Err(e) => {
                self.warn(&e);
                self.fallback.lock().unwrap().get(alias).cloned().unwrap_or_default()
            }
        }
    }

    async fn store(&self, alias: &str, secrets: HostSecrets) {
        let acct = account(alias);
        let result = if secrets.is_empty() {
            self.fallback.lock().unwrap().remove(alias);
            // Even if deleting fails, the user asked for it to be gone.
            self.mark(alias, false);
            self.blocking(move |s| s.delete(&acct)).await
        } else {
            let json = serde_json::to_string(&secrets).expect("secrets serialize");
            match self.blocking(move |s| s.set(&acct, &json)).await {
                Ok(()) => {
                    self.fallback.lock().unwrap().remove(alias);
                    self.mark(alias, true);
                    Ok(())
                }
                Err(e) => {
                    self.fallback.lock().unwrap().insert(alias.to_string(), secrets);
                    Err(e)
                }
            }
        };
        if let Err(e) = result {
            self.warn(&e);
        }
    }

    /// Read-modify-write the item for `alias`; returns the new value.
    pub async fn update(&self, alias: &str, f: impl FnOnce(&mut HostSecrets)) -> HostSecrets {
        let _guard = self.write.lock().await;
        let mut secrets = self.load(alias).await;
        let before = secrets.clone();
        f(&mut secrets);
        if secrets != before {
            self.store(alias, secrets.clone()).await;
        }
        secrets
    }

    pub async fn forget(&self, alias: &str) {
        let _guard = self.write.lock().await;
        self.store(alias, HostSecrets::default()).await;
    }

    /// Move everything saved under `from` to `to` (a host was renamed).
    pub async fn rename(&self, from: &str, to: &str) {
        if from == to {
            return;
        }
        let _guard = self.write.lock().await;
        let moved = self.load(from).await;
        if moved.is_empty() {
            return;
        }
        self.store(to, moved).await;
        self.store(from, HostSecrets::default()).await;
    }
}
