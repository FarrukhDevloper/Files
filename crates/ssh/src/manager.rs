//! One SSH connection + SFTP session per host alias, opened on first use and
//! shared by browsing, transfers, streaming and exec.
//!
//! `ProxyJump a,b` is done in-process: connect to `a`, open a direct-tcpip
//! channel to `b` and run SSH over it, and so on to the target. Every hop
//! gets the full host-key / auth / keychain treatment under its own name.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
use crate::hosts::{self, AppHost, HostDraft};
use crate::known_hosts::{self, Check};
use crate::secrets::{self, OsKeychain, SecretStore, Secrets};
use crate::sftp::SftpBackend;
use crate::{config, HostConfig, Prompter};

/// Hops in one ProxyJump route (also stops `ProxyJump` loops).
const MAX_HOPS: usize = 8;

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
/// `~/.ssh`, the app's hosts.json and the OS keychain; tests point
/// everything at temp dirs and an in-memory store.
#[derive(Debug, Clone)]
pub struct SshOptions {
    pub config_path: PathBuf,
    pub known_hosts: PathBuf,
    /// App-managed hosts (`<config dir>/vps-files/hosts.json`).
    pub hosts_file: PathBuf,
    /// Saved passwords and passphrases.
    pub secrets: Arc<dyn SecretStore>,
    /// ssh-agent socket (Unix) or named pipe (Windows); `None` = no agent.
    pub agent: Option<PathBuf>,
    /// Tried when a host has no `IdentityFile`.
    pub default_identities: Vec<PathBuf>,
    /// Keepalive after this much silence; 3 unanswered = connection dead.
    pub keepalive: Duration,
    /// TCP connect + SSH handshake (prompts happen outside of it).
    pub connect_timeout: Duration,
}

impl SshOptions {
    /// Which aliases have a saved secret; lives next to hosts.json.
    fn secrets_index(&self) -> PathBuf {
        self.hosts_file.with_file_name("secrets-index.json")
    }
}

impl Default for SshOptions {
    fn default() -> Self {
        let ssh_dir = dirs::home_dir().unwrap_or_default().join(".ssh");
        let agent = std::env::var_os("SSH_AUTH_SOCK").filter(|s| !s.is_empty()).map(PathBuf::from);
        #[cfg(windows)]
        let agent = agent.or_else(|| Some(PathBuf::from(r"\\.\pipe\openssh-ssh-agent")));
        let app_dir = dirs::config_dir().unwrap_or_else(|| ssh_dir.with_file_name(".config")).join("vps-files");
        Self {
            config_path: ssh_dir.join("config"),
            known_hosts: ssh_dir.join("known_hosts"),
            hosts_file: app_dir.join("hosts.json"),
            secrets: Arc::new(OsKeychain::new("vps-files")),
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
                secrets: Secrets::new(options.secrets.clone(), options.secrets_index()),
                opts: options,
                events,
                slots: Mutex::new(HashMap::new()),
                next_gen: AtomicU64::new(1),
                hosts_file: Mutex::new(()),
            }),
        }
    }

    /// Hosts from `~/.ssh/config`, then app-managed ones; re-read on every
    /// call. An app host whose alias the config now also defines is hidden.
    /// Never touches the keychain.
    pub fn hosts(&self) -> Result<Vec<HostConfig>> {
        let opts = &self.inner.opts;
        let mut list = config::load_hosts_from(&opts.config_path)?;
        let in_config = config::names_from(&opts.config_path)?;
        let app = hosts::load(&opts.hosts_file).unwrap_or_else(|e| {
            tracing::warn!("{e}");
            Vec::new()
        });
        for host in app {
            if in_config.contains(&host.alias) {
                tracing::warn!("app host {} is shadowed by ~/.ssh/config", host.alias);
            } else {
                list.push(host.config());
            }
        }
        let saved = secrets::read_index(&opts.secrets_index());
        for host in &mut list {
            host.has_saved_secret = saved.contains(&host.alias);
        }
        Ok(list)
    }

    /// The `~/.ssh/config` this manager reads (to open it in an editor).
    pub fn config_path(&self) -> &Path {
        &self.inner.opts.config_path
    }

    /// Add an app host (`previous_alias = None`) or replace the app host
    /// `previous_alias` (edit, maybe renamed). A `draft.password` is saved to
    /// the keychain; without one the saved password is kept, unless the host
    /// name or user changed (it must not be sent to a different server).
    /// The edited host is disconnected.
    pub async fn save_host(&self, draft: HostDraft, previous_alias: Option<&str>) -> Result<HostConfig> {
        let home = dirs::home_dir().unwrap_or_default();
        let host = AppHost::from_draft(&draft, &home)?;
        let previous = {
            let _lock = self.inner.hosts_file.lock().unwrap();
            let in_config = config::names_from(&self.inner.opts.config_path)?;
            let defined_in_config = |alias: &str| in_config.iter().any(|a| a == alias);
            let mut list = hosts::load(&self.inner.opts.hosts_file)?;
            let index = match previous_alias {
                Some(prev) => match list.iter().position(|h| h.alias == prev) {
                    Some(i) => Some(i),
                    None if defined_in_config(prev) => {
                        return Err(VfsError::Unsupported(format!(
                            "{prev} is defined in {}; edit it there",
                            self.inner.opts.config_path.display()
                        )));
                    }
                    None => return Err(VfsError::NotFound(format!("host {prev}"))),
                },
                None => None,
            };
            if defined_in_config(&host.alias) {
                return Err(VfsError::AlreadyExists(format!(
                    "{} is already defined in {}",
                    host.alias,
                    self.inner.opts.config_path.display()
                )));
            }
            if list.iter().enumerate().any(|(i, h)| h.alias == host.alias && Some(i) != index) {
                return Err(VfsError::AlreadyExists(format!("a server named {}", host.alias)));
            }
            let previous = match index {
                Some(i) => Some(std::mem::replace(&mut list[i], host.clone())),
                None => {
                    list.push(host.clone());
                    None
                }
            };
            hosts::save(&self.inner.opts.hosts_file, &list)?;
            previous
        };

        let secrets = &self.inner.secrets;
        if let Some(prev) = &previous {
            self.disconnect(&prev.alias).await;
            secrets.rename(&prev.alias, &host.alias).await;
        }
        self.disconnect(&host.alias).await;
        match draft.password {
            Some(password) => {
                secrets.update(&host.alias, |s| s.password = Some(password)).await;
            }
            None if previous.as_ref().is_some_and(|p| p.hostname != host.hostname || p.user != host.user) => {
                secrets.update(&host.alias, |s| s.password = None).await;
            }
            None => {}
        }
        Ok(host.config())
    }

    /// Delete an app host and everything saved for it.
    pub async fn remove_host(&self, alias: &str) -> Result<()> {
        {
            let _lock = self.inner.hosts_file.lock().unwrap();
            let mut list = hosts::load(&self.inner.opts.hosts_file)?;
            let Some(i) = list.iter().position(|h| h.alias == alias) else {
                let in_config = config::names_from(&self.inner.opts.config_path)?;
                return Err(if in_config.iter().any(|a| a == alias) {
                    VfsError::Unsupported(format!(
                        "{alias} is defined in {}; remove it there",
                        self.inner.opts.config_path.display()
                    ))
                } else {
                    VfsError::NotFound(format!("host {alias}"))
                });
            };
            list.remove(i);
            hosts::save(&self.inner.opts.hosts_file, &list)?;
        }
        self.disconnect(alias).await;
        self.inner.secrets.forget(alias).await;
        Ok(())
    }

    /// Delete the password and key passphrases saved for `alias` (any host).
    pub async fn forget_password(&self, alias: &str) -> Result<()> {
        self.inner.secrets.forget(alias).await;
        Ok(())
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
            c.close().await;
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
    /// ProxyJump hosts, first hop first; the target's transport runs
    /// through the last one.
    jumps: Vec<Arc<Handle<Client>>>,
    sftp: Arc<SftpBackend>,
}

impl Connection {
    async fn close(&self) {
        let _ = self.handle.disconnect(Disconnect::ByApplication, "", "en").await;
        close_all(&self.jumps).await;
    }
}

/// Disconnect hops, last first.
async fn close_all(handles: &[Arc<Handle<Client>>]) {
    for h in handles.iter().rev() {
        let _ = h.disconnect(Disconnect::ByApplication, "", "en").await;
    }
}

/// How a hop is reached: directly, or through an open jump connection.
#[derive(Clone, Copy)]
struct Via<'a> {
    handle: &'a Handle<Client>,
    name: &'a str,
}

/// `[ssh://][user@]host[:port]` from a ProxyJump list (`[v6]:port` too).
fn parse_jump(spec: &str) -> Option<(Option<&str>, &str, Option<u16>)> {
    let s = spec.strip_prefix("ssh://").unwrap_or(spec);
    let s = s.strip_suffix('/').unwrap_or(s);
    let (user, hostport) = match s.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, s),
    };
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        let port = if after.is_empty() { None } else { Some(after.strip_prefix(':')?.parse().ok()?) };
        (h, port)
    } else {
        match hostport.split_once(':') {
            Some((h, p)) if !p.contains(':') => (h, Some(p.parse().ok()?)),
            // A bare IPv6 address has no port.
            _ => (hostport, None),
        }
    };
    let bad = |t: &str| t.is_empty() || t.chars().any(|c| c.is_whitespace() || c.is_control());
    if bad(host) || user.is_some_and(bad) || port == Some(0) {
        return None;
    }
    Some((user, host, port))
}

struct Inner {
    prompter: Arc<dyn Prompter>,
    secrets: Secrets,
    opts: SshOptions,
    events: broadcast::Sender<ConnEvent>,
    slots: Mutex<HashMap<String, Slot>>,
    next_gen: AtomicU64,
    /// Serializes hosts.json read-modify-write.
    hosts_file: Mutex<()>,
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
            conn.close().await;
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

    /// `alias`'s settings: `~/.ssh/config` if it lists the alias, else the
    /// app host, else config defaults (like `ssh alias`).
    fn resolve(&self, alias: &str) -> Result<HostConfig> {
        let (host, listed) = config::host_entry(&self.opts.config_path, alias)?;
        if !listed {
            let app = hosts::load(&self.opts.hosts_file).unwrap_or_else(|e| {
                tracing::warn!("{e}");
                Vec::new()
            });
            if let Some(h) = app.iter().find(|h| h.alias == alias) {
                return Ok(h.config());
            }
        }
        Ok(host)
    }

    /// The jump hosts in front of `host`, in connection order. Like OpenSSH,
    /// the first jump's own ProxyJump is followed too; later ones' aren't
    /// (`-J a,b` reaches `b` through `a`, whatever `b` says).
    fn route(&self, host: &HostConfig, depth: usize, hops: &mut Vec<HostConfig>) -> Result<()> {
        let Some(list) = &host.proxy_jump else {
            if host.proxy_command.is_some() {
                return Err(VfsError::Unsupported(format!(
                    "{} uses ProxyCommand, which VPS Files doesn't support (ProxyJump works)",
                    host.alias
                )));
            }
            return Ok(());
        };
        for (i, spec) in list.split(',').map(str::trim).enumerate() {
            let Some((user, name, port)) = parse_jump(spec) else {
                return Err(VfsError::InvalidName(format!("{}: bad ProxyJump entry {spec:?}", host.alias)));
            };
            let mut hop = self.resolve(name)?;
            if let Some(u) = user {
                hop.user = Some(u.to_string());
            }
            if let Some(p) = port {
                hop.port = p;
            }
            if i == 0 {
                if depth >= MAX_HOPS {
                    return Err(VfsError::Unsupported(format!(
                        "{}: more than {MAX_HOPS} ProxyJump hops (a loop?)",
                        host.alias
                    )));
                }
                self.route(&hop, depth + 1, hops)?;
            }
            hops.push(hop);
        }
        if hops.len() > MAX_HOPS {
            return Err(VfsError::Unsupported(format!("{}: more than {MAX_HOPS} ProxyJump hops", host.alias)));
        }
        Ok(())
    }

    async fn connect(self: &Arc<Self>, alias: &str, generation: u64) -> Result<Arc<Connection>> {
        let host = self.resolve(alias)?;
        let mut route = Vec::new();
        self.route(&host, 0, &mut route)?;
        let mut jumps = Vec::new();
        match self.connect_via(&host, &route, &mut jumps, generation).await {
            Ok((handle, sftp)) => Ok(Arc::new(Connection { generation, handle, jumps, sftp: Arc::new(sftp) })),
            Err(e) => {
                close_all(&jumps).await;
                Err(e)
            }
        }
    }

    /// Open each hop of `route` (pushed onto `jumps` as it comes up), then
    /// the target and its SFTP session.
    async fn connect_via(
        self: &Arc<Self>,
        host: &HostConfig,
        route: &[HostConfig],
        jumps: &mut Vec<Arc<Handle<Client>>>,
        generation: u64,
    ) -> Result<(Arc<Handle<Client>>, SftpBackend)> {
        for (i, hop) in route.iter().enumerate() {
            let via = i.checked_sub(1).map(|prev| Via { handle: &jumps[prev], name: &route[prev].alias });
            let handle = self.open(hop, &host.alias, generation, via).await?;
            jumps.push(Arc::new(handle));
        }
        let via = route.last().map(|last| Via { handle: jumps.last().unwrap(), name: &last.alias });
        let handle = Arc::new(self.open(host, &host.alias, generation, via).await?);
        let sftp = SftpBackend::open(&host.alias, handle.clone()).await?;
        Ok((handle, sftp))
    }

    /// One authenticated SSH connection to `host`. `slot` is the alias whose
    /// connection this is part of (a dropped hop drops it).
    async fn open(
        self: &Arc<Self>,
        host: &HostConfig,
        slot: &str,
        generation: u64,
        via: Option<Via<'_>>,
    ) -> Result<Handle<Client>> {
        let user = host.user.clone().unwrap_or_else(config::local_user);

        // An unknown host key ends the first handshake; we ask the user
        // with no connection held open, then connect again.
        let mut trusted_now = false;
        let mut handle = loop {
            match self.handshake(host, slot, generation, via).await {
                Ok(handle) => break handle,
                Err(ClientError::UnknownHostKey(key)) if !trusted_now => {
                    self.trust(host, &key).await?;
                    trusted_now = true;
                }
                Err(e) => return Err(e.into_vfs(&host.alias)),
            }
        };

        auth::authenticate(
            &mut handle,
            AuthContext {
                host,
                user: &user,
                agent: self.opts.agent.as_deref(),
                default_identities: &self.opts.default_identities,
                prompter: &*self.prompter,
                secrets: &self.secrets,
            },
        )
        .await?;
        Ok(handle)
    }

    async fn handshake(
        self: &Arc<Self>,
        host: &HostConfig,
        slot: &str,
        generation: u64,
        via: Option<Via<'_>>,
    ) -> Result<Handle<Client>, ClientError> {
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
            slot: slot.to_string(),
            name: host.alias.clone(),
            hostname: host.hostname.clone(),
            port: host.port,
            known_hosts: self.opts.known_hosts.clone(),
            manager: Arc::downgrade(self),
            generation,
        };
        let (hostname, port) = (host.hostname.clone(), host.port);
        let fut = async move {
            match via {
                None => {
                    let stream = tokio::net::TcpStream::connect((hostname, port)).await.map_err(russh::Error::from)?;
                    client::connect_stream(config, stream, handler).await
                }
                Some(via) => {
                    let channel = via
                        .handle
                        .channel_open_direct_tcpip(hostname.as_str(), u32::from(port), "127.0.0.1", 0)
                        .await
                        .map_err(|e| ClientError::Vfs(forward_error(via.name, &hostname, port, e)))?;
                    client::connect_stream(config, channel.into_stream(), handler).await
                }
            }
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

/// A jump host couldn't open the forwarded connection.
fn forward_error(jump: &str, hostname: &str, port: u16, e: russh::Error) -> VfsError {
    let to = format!("{hostname}:{port}");
    match e {
        russh::Error::ChannelOpenFailure(russh::ChannelOpenFailure::AdministrativelyProhibited) => {
            VfsError::Unsupported(format!("{jump} doesn't allow forwarding to {to} (AllowTcpForwarding?)"))
        }
        russh::Error::ChannelOpenFailure(why) => {
            VfsError::NotConnected(format!("{jump} can't reach {to} ({why:?})"))
        }
        e => ssh_error(jump, e),
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
    /// The manager slot this connection belongs to (the target's alias,
    /// also for its jump hosts).
    slot: String,
    /// This hop's name, for messages.
    name: String,
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
                self.name
            ))));
        };
        match known_hosts::check(&self.known_hosts, &self.hostname, self.port, key) {
            Check::Known => Ok(true),
            Check::Unknown => Err(ClientError::UnknownHostKey(key.clone())),
            Check::Changed => Err(ClientError::Vfs(VfsError::HostKey(format!(
                "{}: the {} host key of {} changed (now {}). This can mean someone is \
                 intercepting the connection. If the server was reinstalled, remove its old \
                 entry from {}.",
                self.name,
                key.algorithm(),
                self.hostname,
                key.fingerprint(HashAlg::Sha256),
                self.known_hosts.display()
            )))),
            Check::Revoked => Err(ClientError::Vfs(VfsError::HostKey(format!(
                "{}: the host key of {} is marked @revoked in {}",
                self.name,
                self.hostname,
                self.known_hosts.display()
            )))),
        }
    }

    async fn disconnected(&mut self, reason: DisconnectReason<ClientError>) -> Result<(), ClientError> {
        if let Some(m) = self.manager.upgrade() {
            m.on_dropped(&self.slot, self.generation);
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

#[cfg(test)]
mod tests {
    use super::parse_jump;

    #[test]
    fn jump_specs() {
        assert_eq!(parse_jump("bastion"), Some((None, "bastion", None)));
        assert_eq!(parse_jump("me@bastion:2222"), Some((Some("me"), "bastion", Some(2222))));
        assert_eq!(parse_jump("ssh://me@10.0.0.1:22/"), Some((Some("me"), "10.0.0.1", Some(22))));
        assert_eq!(parse_jump("[::1]:2200"), Some((None, "::1", Some(2200))));
        assert_eq!(parse_jump("[::1]"), Some((None, "::1", None)));
        assert_eq!(parse_jump("::1"), Some((None, "::1", None)));
        assert_eq!(parse_jump("a@b@host"), Some((Some("a@b"), "host", None)));
        for bad in ["", "me@", "@host", "host:", "host:x", "host:0", "[::1", "[::1]x", "a b"] {
            assert_eq!(parse_jump(bad), None, "{bad:?}");
        }
    }
}
