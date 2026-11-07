//! One SSH connection + SFTP session per host alias, opened on first use and
//! shared by browsing, transfers, streaming and exec.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures::future::{BoxFuture, FutureExt, Shared};
use russh::client::{self, DisconnectReason, Handle};
use russh::keys::{HashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use serde::Serialize;
use tokio::sync::broadcast;
use vpsfiles_vfs::{Backend, Result, VfsError};

use crate::auth::{self, AuthContext};
use crate::known_hosts::{self, Check};
use crate::sftp::SftpBackend;
use crate::{config, HostConfig, Prompter};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum ConnStatus {
    Disconnected,
    Connecting,
    Connected,
    Failed { error: VfsError },
}

/// Broadcast whenever a host's [`ConnStatus`] changes.
#[derive(Debug, Clone, Serialize)]
pub struct ConnEvent {
    pub host: String,
    pub status: ConnStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub exit_status: u32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Where the manager finds the user's SSH files. `Default` is the real
/// `~/.ssh`; tests point everything at a temp dir.
#[derive(Debug, Clone)]
pub struct SshOptions {
    pub config_path: PathBuf,
    pub known_hosts: PathBuf,
    /// ssh-agent socket (Unix) or named pipe (Windows); `None` = no agent.
    pub agent: Option<PathBuf>,
    /// Tried when a host has no `IdentityFile`.
    pub default_identities: Vec<PathBuf>,
    /// Keepalive after this much silence; 3 unanswered = connection dead.
    pub keepalive: Duration,
    /// TCP connect + SSH handshake (prompts happen outside of it).
    pub connect_timeout: Duration,
}

impl Default for SshOptions {
    fn default() -> Self {
        let ssh_dir = dirs::home_dir().unwrap_or_default().join(".ssh");
        let agent = std::env::var_os("SSH_AUTH_SOCK").filter(|s| !s.is_empty()).map(PathBuf::from);
        #[cfg(windows)]
        let agent = agent.or_else(|| Some(PathBuf::from(r"\\.\pipe\openssh-ssh-agent")));
        Self {
            config_path: ssh_dir.join("config"),
            known_hosts: ssh_dir.join("known_hosts"),
            agent,
            default_identities: ["id_ed25519", "id_ecdsa", "id_rsa"].iter().map(|n| ssh_dir.join(n)).collect(),
            keepalive: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(20),
        }
    }
}

/// Owns at most one SSH connection (+ SFTP session) per host alias.
pub struct SshManager {
    inner: Arc<Inner>,
}

impl SshManager {
    pub fn new(prompter: Arc<dyn Prompter>) -> Self {
        Self::with_options(prompter, SshOptions::default())
    }

    pub fn with_options(prompter: Arc<dyn Prompter>, options: SshOptions) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(Inner {
                prompter,
                opts: options,
                events,
                slots: Mutex::new(HashMap::new()),
                next_gen: AtomicU64::new(1),
            }),
        }
    }

    /// Hosts from `~/.ssh/config`, re-read on every call.
    pub fn hosts(&self) -> Result<Vec<HostConfig>> {
        config::load_hosts_from(&self.inner.opts.config_path)
    }

    pub fn status(&self, alias: &str) -> ConnStatus {
        match self.inner.slots.lock().unwrap().get(alias) {
            None => ConnStatus::Disconnected,
            Some(Slot::Connecting { .. }) => ConnStatus::Connecting,
            Some(Slot::Connected(c)) if c.handle.is_closed() => ConnStatus::Disconnected,
            Some(Slot::Connected(_)) => ConnStatus::Connected,
            Some(Slot::Failed(e)) => ConnStatus::Failed { error: e.clone() },
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ConnEvent> {
        self.inner.events.subscribe()
    }

    /// Connect if needed and return the host's SFTP backend. Concurrent
    /// callers for the same alias share one connection attempt.
    pub async fn backend(&self, alias: &str) -> Result<Arc<dyn Backend>> {
        Ok(self.connection(alias).await?.sftp.clone())
    }

    pub async fn disconnect(&self, alias: &str) {
        let prev = {
            let mut slots = self.inner.slots.lock().unwrap();
            let prev = slots.remove(alias);
            if prev.is_some() {
                self.inner.emit(alias, ConnStatus::Disconnected);
            }
            prev
        };
        // A pending attempt notices the missing slot when it finishes and
        // closes what it opened.
        if let Some(Slot::Connected(c)) = prev {
            let _ = c.handle.disconnect(Disconnect::ByApplication, "", "en").await;
        }
    }

    /// Run a shell command on the host (used for direct VPS→VPS copies).
    pub async fn exec(&self, alias: &str, command: &str) -> Result<ExecOutput> {
        let conn = self.connection(alias).await?;
        exec(&conn.handle, alias, command).await
    }

    async fn connection(&self, alias: &str) -> Result<Arc<Connection>> {
        let attempt = {
            let mut slots = self.inner.slots.lock().unwrap();
            match slots.get(alias) {
                Some(Slot::Connected(c)) if !c.handle.is_closed() => return Ok(c.clone()),
                Some(Slot::Connecting { attempt, .. }) => attempt.clone(),
                _ => {
                    let generation = self.inner.next_gen.fetch_add(1, Ordering::Relaxed);
                    // Spawned so a caller that goes away can't abort a
                    // half-done handshake that others are waiting on.
                    let task = tokio::spawn(self.inner.clone().establish(alias.to_string(), generation));
                    let attempt = async move {
                        task.await.unwrap_or_else(|e| Err(VfsError::Other(format!("connect task failed: {e}"))))
                    }
                    .boxed()
                    .shared();
                    slots.insert(alias.to_string(), Slot::Connecting { generation, attempt: attempt.clone() });
                    self.inner.emit(alias, ConnStatus::Connecting);
                    attempt
                }
            }
        };
        attempt.await
    }
}

type Attempt = Shared<BoxFuture<'static, Result<Arc<Connection>>>>;

enum Slot {
    Connecting { generation: u64, attempt: Attempt },
    Connected(Arc<Connection>),
    /// Last attempt failed; kept so `status()` can show why.
    Failed(VfsError),
}

pub(crate) struct Connection {
    generation: u64,
    handle: Arc<Handle<Client>>,
    sftp: Arc<SftpBackend>,
}

struct Inner {
    prompter: Arc<dyn Prompter>,
    opts: SshOptions,
    events: broadcast::Sender<ConnEvent>,
    slots: Mutex<HashMap<String, Slot>>,
    next_gen: AtomicU64,
}

impl Inner {
    /// Call with `slots` locked so events go out in state order.
    fn emit(&self, alias: &str, status: ConnStatus) {
        let _ = self.events.send(ConnEvent { host: alias.to_string(), status });
    }

    async fn establish(self: Arc<Self>, alias: String, generation: u64) -> Result<Arc<Connection>> {
        let result = self.connect(&alias, generation).await;
        let orphan = {
            let mut slots = self.slots.lock().unwrap();
            let current = matches!(slots.get(&alias), Some(Slot::Connecting { generation: g, .. }) if *g == generation);
            match &result {
                _ if !current => result.as_ref().ok().cloned(),
                Ok(conn) => {
                    slots.insert(alias.clone(), Slot::Connected(conn.clone()));
                    self.emit(&alias, ConnStatus::Connected);
                    None
                }
                Err(VfsError::Cancelled) => {
                    slots.remove(&alias);
                    self.emit(&alias, ConnStatus::Disconnected);
                    None
                }
                Err(e) => {
                    slots.insert(alias.clone(), Slot::Failed(e.clone()));
                    self.emit(&alias, ConnStatus::Failed { error: e.clone() });
                    None
                }
            }
        };
        if let Some(conn) = orphan {
            // `disconnect()` ran while we were connecting.
            let _ = conn.handle.disconnect(Disconnect::ByApplication, "", "en").await;
            return Err(VfsError::NotConnected(format!("{alias}: disconnected")));
        }
        result
    }

    /// The SSH session ended on its own (network drop, server restart,
    /// keepalive timeout): the next call reconnects.
    fn on_dropped(&self, alias: &str, generation: u64) {
        let mut slots = self.slots.lock().unwrap();
        if matches!(slots.get(alias), Some(Slot::Connected(c)) if c.generation == generation) {
            slots.remove(alias);
            self.emit(alias, ConnStatus::Disconnected);
        }
    }

    async fn connect(self: &Arc<Self>, alias: &str, generation: u64) -> Result<Arc<Connection>> {
        let host = config::host_from(&self.opts.config_path, alias)?;
        if let Some(jump) = &host.proxy_jump {
            return Err(VfsError::Unsupported(format!(
                "{alias} uses ProxyJump ({jump}), which VPS Files doesn't support yet"
            )));
        }
        if host.proxy_command.is_some() {
            return Err(VfsError::Unsupported(format!(
                "{alias} uses ProxyCommand, which VPS Files doesn't support yet"
            )));
        }
        let user = host.user.clone().unwrap_or_else(config::local_user);

        // An unknown host key ends the first handshake; we ask the user
        // with no connection held open, then connect again.
        let mut trusted_now = false;
        let mut handle = loop {
            match self.handshake(&host, generation).await {
                Ok(handle) => break handle,
                Err(ClientError::UnknownHostKey(key)) if !trusted_now => {
                    self.trust(&host, &key).await?;
                    trusted_now = true;
                }
                Err(e) => return Err(e.into_vfs(alias)),
            }
        };

        auth::authenticate(
            &mut handle,
            AuthContext {
                host: &host,
                user: &user,
                agent: self.opts.agent.as_deref(),
                default_identities: &self.opts.default_identities,
                prompter: &*self.prompter,
            },
        )
        .await?;

        let handle = Arc::new(handle);
        let sftp = SftpBackend::open(alias, handle.clone()).await?;
        Ok(Arc::new(Connection { generation, handle, sftp: Arc::new(sftp) }))
    }

    async fn handshake(self: &Arc<Self>, host: &HostConfig, generation: u64) -> Result<Handle<Client>, ClientError> {
        let known = known_hosts::known_algorithms(&self.opts.known_hosts, &host.hostname, host.port);
        let mut preferred = russh::Preferred::default();
        if !known.is_empty() {
            // Like OpenSSH: ask for the key types we can already verify.
            let mut keys = preferred.key.to_vec();
            keys.sort_by_key(|a| !known.iter().any(|k| known_hosts::same_type(k, a)));
            preferred.key = keys.into();
        }
        let config = Arc::new(client::Config {
            keepalive_interval: Some(self.opts.keepalive),
            keepalive_max: 3,
            nodelay: true,
            preferred,
            ..Default::default()
        });
        let handler = Client {
            alias: host.alias.clone(),
            hostname: host.hostname.clone(),
            port: host.port,
            known_hosts: self.opts.known_hosts.clone(),
            manager: Arc::downgrade(self),
            generation,
        };
        let addr = (host.hostname.clone(), host.port);
        let fut = async move {
            let stream = tokio::net::TcpStream::connect(addr).await.map_err(russh::Error::from)?;
            client::connect_stream(config, stream, handler).await
        };
        match tokio::time::timeout(self.opts.connect_timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(ClientError::Vfs(VfsError::NotConnected(format!(
                "{}: no answer from {}:{} within {}s",
                host.alias,
                host.hostname,
                host.port,
                self.opts.connect_timeout.as_secs()
            )))),
        }
    }

    /// TOFU: ask, then remember. Declining is a cancel, not an error.
    async fn trust(&self, host: &HostConfig, key: &PublicKey) -> Result<()> {
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let algorithm = key.algorithm().to_string();
        if !self.prompter.confirm_host_key(&host.alias, &algorithm, &fingerprint).await {
            return Err(VfsError::Cancelled);
        }
        known_hosts::append(&self.opts.known_hosts, &host.hostname, host.port, key).map_err(|e| {
            VfsError::Io(format!("{}: can't save host key: {e}", self.opts.known_hosts.display()))
        })
    }
}

#[derive(Debug)]
pub(crate) enum ClientError {
    Ssh(russh::Error),
    Vfs(VfsError),
    /// Not in known_hosts yet; the handshake stops so we can ask the user.
    UnknownHostKey(PublicKey),
}

impl From<russh::Error> for ClientError {
    fn from(e: russh::Error) -> Self {
        ClientError::Ssh(e)
    }
}

impl ClientError {
    fn into_vfs(self, alias: &str) -> VfsError {
        match self {
            ClientError::Ssh(e) => ssh_error(alias, e),
            ClientError::Vfs(e) => e,
            ClientError::UnknownHostKey(_) => VfsError::HostKey(format!("{alias}: host key still unknown")),
        }
    }
}

pub(crate) fn ssh_error(alias: &str, e: russh::Error) -> VfsError {
    match e {
        russh::Error::NotAuthenticated | russh::Error::UnsupportedAuthMethod => {
            VfsError::Auth(format!("{alias}: {e}"))
        }
        russh::Error::NoCommonAlgo { .. } | russh::Error::UnknownAlgo => {
            VfsError::Unsupported(format!("{alias}: {e}"))
        }
        _ => VfsError::NotConnected(format!("{alias}: {e}")),
    }
}

/// russh connection callbacks for one connection attempt.
pub(crate) struct Client {
    alias: String,
    hostname: String,
    port: u16,
    known_hosts: PathBuf,
    manager: Weak<Inner>,
    generation: u64,
}

impl client::Handler for Client {
    type Error = ClientError;

    async fn check_server_key(&mut self, server_key: &PublicKeyOrCertificate) -> Result<bool, ClientError> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = server_key else {
            return Err(ClientError::Vfs(VfsError::HostKey(format!(
                "{}: server sent a host certificate; only plain host keys are supported",
                self.alias
            ))));
        };
        match known_hosts::check(&self.known_hosts, &self.hostname, self.port, key) {
            Check::Known => Ok(true),
            Check::Unknown => Err(ClientError::UnknownHostKey(key.clone())),
            Check::Changed => Err(ClientError::Vfs(VfsError::HostKey(format!(
                "{}: the {} host key of {} changed (now {}). This can mean someone is \
                 intercepting the connection. If the server was reinstalled, remove its old \
                 entry from {}.",
                self.alias,
                key.algorithm(),
                self.hostname,
                key.fingerprint(HashAlg::Sha256),
                self.known_hosts.display()
            )))),
            Check::Revoked => Err(ClientError::Vfs(VfsError::HostKey(format!(
                "{}: the host key of {} is marked @revoked in {}",
                self.alias,
                self.hostname,
                self.known_hosts.display()
            )))),
        }
    }

    async fn disconnected(&mut self, reason: DisconnectReason<ClientError>) -> Result<(), ClientError> {
        if let Some(m) = self.manager.upgrade() {
            m.on_dropped(&self.alias, self.generation);
        }
        match reason {
            DisconnectReason::ReceivedDisconnect(_) => Ok(()),
            DisconnectReason::Error(e) => Err(e),
        }
    }
}

/// Run `command` in a new session channel and collect its output.
pub(crate) async fn exec(handle: &Handle<Client>, alias: &str, command: &str) -> Result<ExecOutput> {
    let mut channel = handle.channel_open_session().await.map_err(|e| ssh_error(alias, e))?;
    channel.exec(true, command).await.map_err(|e| ssh_error(alias, e))?;
    // No stdin: a command that reads it (or an sftp-only ForceCommand)
    // sees EOF instead of hanging.
    channel.eof().await.map_err(|e| ssh_error(alias, e))?;
    let (mut stdout, mut stderr, mut status) = (Vec::new(), Vec::new(), None);
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
            ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
            ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
            // Killed by a signal: no exit status; 255 like `ssh` itself.
            ChannelMsg::ExitSignal { .. } => status = status.or(Some(255)),
            ChannelMsg::Failure => {
                return Err(VfsError::Unsupported(format!("{alias}: the server refused to run commands")));
            }
            _ => {}
        }
    }
    Ok(ExecOutput { exit_status: status.unwrap_or(255), stdout, stderr })
}
