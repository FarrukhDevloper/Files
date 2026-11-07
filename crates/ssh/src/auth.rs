//! Client authentication, in OpenSSH order: ssh-agent → identity files
//! (passphrase via the prompter) → password / keyboard-interactive.
//!
//! Passwords and passphrases saved in the keychain are tried before asking;
//! a saved one that turns out wrong is deleted and the user is asked.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::client::{AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::agent::AgentIdentity;
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use russh::{MethodKind, MethodSet};
use vpsfiles_vfs::{Result, VfsError};

use crate::manager::{ssh_error, Client};
use crate::secrets::{HostSecrets, Secrets};
use crate::{HostConfig, Prompter};

/// Wrong passphrases/passwords before we give up on that secret.
const SECRET_ATTEMPTS: usize = 3;

type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

#[cfg(unix)]
async fn connect_agent(path: &Path) -> std::result::Result<Agent, russh::keys::Error> {
    Ok(AgentClient::connect_uds(path).await?.dynamic())
}

#[cfg(windows)]
async fn connect_agent(path: &Path) -> std::result::Result<Agent, russh::keys::Error> {
    Ok(AgentClient::connect_named_pipe(path).await?.dynamic())
}

/// An `IdentityFile` that exists on disk.
struct IdentityFile {
    path: PathBuf,
    text: String,
    /// From `<path>.pub`, or the unencrypted public half of an OpenSSH key.
    public: Option<PublicKey>,
}

impl IdentityFile {
    fn read(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut pub_path = path.as_os_str().to_owned();
        pub_path.push(".pub");
        let public = russh::keys::load_public_key(&pub_path).ok().or_else(|| {
            russh::keys::ssh_key::PrivateKey::from_openssh(&text).ok().map(|k| k.public_key().clone())
        });
        Some(Self { path: path.to_path_buf(), text, public })
    }

    fn has(&self, key: &PublicKey) -> bool {
        self.public.as_ref().is_some_and(|p| p.key_data() == key.key_data())
    }

    /// Decrypt if needed. `None` = skip this key (unreadable or cancelled).
    async fn load(&self, cx: &AuthContext<'_>, saved: &mut Saved<'_>) -> Option<PrivateKey> {
        match russh::keys::decode_secret_key(&self.text, None) {
            Ok(k) => return Some(k),
            Err(russh::keys::Error::KeyIsEncrypted) => {}
            Err(e) => {
                tracing::warn!("{}: unusable key: {e}", self.path.display());
                return None;
            }
        }
        let shown = self.path.display().to_string();
        if let Some(pass) = saved.get().await.passphrases.get(&shown).cloned() {
            match russh::keys::decode_secret_key(&self.text, Some(&pass)) {
                Ok(k) => return Some(k),
                // A quiet attempt leaves it for the user's next try.
                Err(_) if cx.interactive => saved.update(|s| drop(s.passphrases.remove(&shown))).await,
                Err(_) => {}
            }
        }
        for _ in 0..SECRET_ATTEMPTS {
            let pass = cx.prompter.passphrase(cx.label, &shown).await?;
            match russh::keys::decode_secret_key(&self.text, Some(&pass.value)) {
                Ok(k) => {
                    if pass.remember {
                        saved.update(|s| drop(s.passphrases.insert(shown, pass.value))).await;
                    }
                    return Some(k);
                }
                Err(e) => tracing::debug!("{shown}: wrong passphrase: {e}"),
            }
        }
        None
    }
}

/// The host's keychain item, read on first need (it may show an unlock
/// dialog, so not for hosts that never need a secret).
struct Saved<'a> {
    secrets: &'a Secrets,
    alias: &'a str,
    cached: Option<HostSecrets>,
}

impl Saved<'_> {
    async fn get(&mut self) -> &HostSecrets {
        if self.cached.is_none() {
            self.cached = Some(self.secrets.load(self.alias).await);
        }
        self.cached.as_ref().unwrap()
    }

    async fn update(&mut self, f: impl FnOnce(&mut HostSecrets)) {
        self.cached = Some(self.secrets.update(self.alias, f).await);
    }
}

fn names(methods: &MethodSet) -> String {
    let v: Vec<&str> = methods.iter().map(<&str>::from).collect();
    if v.is_empty() { "none".into() } else { v.join(", ") }
}

/// Lazily asks the server which RSA signature hash it accepts (can take up
/// to a second, so only when an RSA key is actually tried).
struct RsaHash(Option<Option<HashAlg>>);

impl RsaHash {
    async fn for_key(&mut self, handle: &Handle<Client>, key: &PublicKey) -> Option<HashAlg> {
        if !key.algorithm().is_rsa() {
            return None;
        }
        if self.0.is_none() {
            self.0 = Some(handle.best_supported_rsa_hash().await.ok().flatten().flatten());
        }
        self.0.flatten()
    }
}

pub(crate) struct AuthContext<'a> {
    pub host: &'a HostConfig,
    /// What prompts call the host: the alias, or "tanla via bastion" for a
    /// jump host.
    pub label: &'a str,
    /// `false`: a background attempt; a saved secret that fails is kept.
    pub interactive: bool,
    pub user: &'a str,
    pub agent: Option<&'a Path>,
    pub default_identities: &'a [PathBuf],
    pub prompter: &'a dyn Prompter,
    pub secrets: &'a Secrets,
}

pub(crate) async fn authenticate(handle: &mut Handle<Client>, cx: AuthContext<'_>) -> Result<()> {
    let alias = cx.host.alias.as_str();
    let user = cx.user;
    let err = |e| ssh_error(alias, e);

    let mut methods = match handle.authenticate_none(user).await.map_err(err)? {
        AuthResult::Success => return Ok(()),
        AuthResult::Failure { remaining_methods, .. } => remaining_methods,
    };
    let configured =
        if cx.host.identity_files.is_empty() { cx.default_identities } else { &cx.host.identity_files };
    let files: Vec<IdentityFile> = configured.iter().filter_map(|p| IdentityFile::read(p)).collect();
    let mut rsa = RsaHash(None);
    let mut tried: Vec<PublicKey> = Vec::new();
    let mut saved = Saved { secrets: cx.secrets, alias, cached: None };

    // 1. ssh-agent.
    if let Some(path) = cx.agent.filter(|_| methods.contains(&MethodKind::PublicKey)) {
        match connect_agent(path).await {
            Ok(mut agent) => {
                if let Some(binding) = handle.agent_session_binding() {
                    // Needed for destination-constrained keys; harmless otherwise.
                    let _ = agent.bind_session(binding, false).await;
                }
                let ids = agent.request_identities().await.unwrap_or_default();
                for id in ids {
                    let AgentIdentity::PublicKey { key, .. } = id else { continue };
                    if cx.host.identities_only && !files.iter().any(|f| f.has(&key)) {
                        continue;
                    }
                    if !methods.contains(&MethodKind::PublicKey) {
                        break;
                    }
                    let hash = rsa.for_key(handle, &key).await;
                    tried.push(key.clone());
                    match handle.authenticate_publickey_with(user, key, hash, &mut agent).await {
                        Ok(AuthResult::Success) => return Ok(()),
                        Ok(AuthResult::Failure { remaining_methods, .. }) => methods = remaining_methods,
                        Err(e) => tracing::debug!("{alias}: agent could not sign: {e}"),
                    }
                }
            }
            Err(e) => tracing::debug!("{alias}: no ssh-agent at {}: {e}", path.display()),
        }
    }

    // 2. Identity files (skipping keys the agent already offered).
    for file in &files {
        if !methods.contains(&MethodKind::PublicKey) {
            break;
        }
        if tried.iter().any(|k| file.has(k)) {
            continue;
        }
        let Some(key) = file.load(&cx, &mut saved).await else { continue };
        let hash = rsa.for_key(handle, key.public_key()).await;
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        match handle.authenticate_publickey(user, key).await.map_err(err)? {
            AuthResult::Success => return Ok(()),
            AuthResult::Failure { remaining_methods, .. } => methods = remaining_methods,
        }
    }

    // 3. Password, or keyboard-interactive answered with the password (PAM):
    //    the saved one first, then up to SECRET_ATTEMPTS typed ones.
    if takes_password(&methods)
        && let Some(password) = saved.get().await.password.clone()
    {
        match password_auth(handle, user, &methods, &password).await.map_err(err)? {
            AuthResult::Success => return Ok(()),
            AuthResult::Failure { remaining_methods, .. } => {
                methods = remaining_methods;
                if cx.interactive {
                    tracing::info!("{alias}: saved password was rejected; forgetting it");
                    saved.update(|s| s.password = None).await;
                }
            }
        }
    }
    for _ in 0..SECRET_ATTEMPTS {
        if !takes_password(&methods) {
            break;
        }
        let Some(password) = cx.prompter.password(cx.label, user).await else {
            return Err(VfsError::Cancelled);
        };
        match password_auth(handle, user, &methods, &password.value).await.map_err(err)? {
            AuthResult::Success => {
                if password.remember {
                    saved.update(|s| s.password = Some(password.value)).await;
                }
                return Ok(());
            }
            AuthResult::Failure { remaining_methods, .. } => methods = remaining_methods,
        }
    }

    Err(VfsError::Auth(format!(
        "{user}@{alias}: no authentication method succeeded (server accepts: {})",
        names(&methods)
    )))
}

fn takes_password(methods: &MethodSet) -> bool {
    methods.contains(&MethodKind::Password) || methods.contains(&MethodKind::KeyboardInteractive)
}

async fn password_auth(
    handle: &mut Handle<Client>,
    user: &str,
    methods: &MethodSet,
    password: &str,
) -> std::result::Result<AuthResult, russh::Error> {
    if methods.contains(&MethodKind::Password) {
        handle.authenticate_password(user, password).await
    } else {
        keyboard_interactive(handle, user, password).await
    }
}

async fn keyboard_interactive(
    handle: &mut Handle<Client>,
    user: &str,
    password: &str,
) -> std::result::Result<AuthResult, russh::Error> {
    let mut reply = handle.authenticate_keyboard_interactive_start(user, None).await?;
    // A server may send an empty info request before the real one.
    for _ in 0..4 {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(AuthResult::Success),
            KeyboardInteractiveAuthResponse::Failure { remaining_methods, partial_success } => {
                return Ok(AuthResult::Failure { remaining_methods, partial_success });
            }
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                reply = handle.authenticate_keyboard_interactive_respond(answers).await?;
            }
        }
    }
    Ok(AuthResult::Failure { remaining_methods: MethodSet::empty(), partial_success: false })
}
