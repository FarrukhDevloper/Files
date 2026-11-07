use std::fmt;

use async_trait::async_trait;

/// A password or passphrase the user typed.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret {
    pub value: String,
    /// "Remember in keychain" was ticked: saved once it is proven right.
    pub remember: bool,
}

impl Secret {
    /// Typed once, not remembered.
    pub fn once(value: impl Into<String>) -> Self {
        Self { value: value.into(), remember: false }
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secret").field("value", &"<redacted>").field("remember", &self.remember).finish()
    }
}

/// Interactive questions the SSH layer needs answered by the user.
/// The app implements this with UI dialogs; tests use a canned impl.
///
/// Returning `None` / `false` means the user cancelled. Secrets saved in the
/// keychain are tried before these are asked.
#[async_trait]
pub trait Prompter: Send + Sync + 'static {
    /// The host's key is not in `known_hosts`. `true` = trust and remember.
    /// A key that *changed* is never prompted for — it fails with
    /// `VfsError::HostKey` like OpenSSH does.
    async fn confirm_host_key(&self, host: &str, algorithm: &str, fingerprint: &str) -> bool;

    async fn password(&self, host: &str, user: &str) -> Option<Secret>;

    async fn passphrase(&self, host: &str, key_path: &str) -> Option<Secret>;
}
