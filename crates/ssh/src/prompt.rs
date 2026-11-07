use async_trait::async_trait;

/// Interactive questions the SSH layer needs answered by the user.
/// The app implements this with UI dialogs; tests use a canned impl.
///
/// Returning `None` / `false` means the user cancelled.
#[async_trait]
pub trait Prompter: Send + Sync + 'static {
    /// The host's key is not in `known_hosts`. `true` = trust and remember.
    /// A key that *changed* is never prompted for — it fails with
    /// `VfsError::HostKey` like OpenSSH does.
    async fn confirm_host_key(&self, host: &str, algorithm: &str, fingerprint: &str) -> bool;

    async fn password(&self, host: &str, user: &str) -> Option<String>;

    async fn passphrase(&self, host: &str, key_path: &str) -> Option<String>;
}
