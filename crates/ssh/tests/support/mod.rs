//! A hermetic SSH + SFTP server for tests: russh server on 127.0.0.1:0, an
//! SFTP handler over the real filesystem (tests only touch temp dirs), and
//! `exec` via `sh -c`. Nothing here talks to a real host.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use russh::keys::ssh_key::private::Ed25519Keypair;
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, Session, VerifiedPublicKey};
use russh::{Channel, ChannelId};
use russh_sftp::extensions::Statvfs;
use russh_sftp::protocol::{
    Attrs, Data, ExtendedReply, File, FileAttributes, Handle, Name, OpenFlags, Packet, Status, StatusCode, Version,
};
use vpsfiles_ssh::{Prompter, Secret, SecretStore, SshManager, SshOptions};
use vpsfiles_vfs::VfsError;

pub const USER: &str = "tester";
pub const PASSWORD: &str = "s3cret";

pub fn key(seed: u8) -> PrivateKey {
    PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]))
}

#[derive(Default)]
pub struct Counters {
    pub connections: AtomicUsize,
    /// SSH sessions that ended (the client went away).
    pub closed: AtomicUsize,
    /// direct-tcpip channels asked for (we are someone's ProxyJump).
    pub forwards: AtomicUsize,
    pub stats: AtomicUsize,
    pub readdirs: AtomicUsize,
    pub execs: AtomicUsize,
}

pub struct ServerState {
    pub config: Mutex<Arc<russh::server::Config>>,
    pub authorized: Mutex<Vec<PublicKey>>,
    pub allow_exec: Mutex<bool>,
    pub allow_forwarding: Mutex<bool>,
    pub counters: Counters,
    sessions: Mutex<Vec<tokio::task::AbortHandle>>,
}

pub struct TestServer {
    pub port: u16,
    pub state: Arc<ServerState>,
}

fn server_config(host_key: PrivateKey) -> Arc<russh::server::Config> {
    Arc::new(russh::server::Config {
        keys: vec![host_key],
        auth_rejection_time: Duration::ZERO,
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    })
}

impl TestServer {
    pub async fn start(host_key: PrivateKey) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(ServerState {
            config: Mutex::new(server_config(host_key)),
            authorized: Mutex::new(Vec::new()),
            allow_exec: Mutex::new(true),
            allow_forwarding: Mutex::new(true),
            counters: Counters::default(),
            sessions: Mutex::new(Vec::new()),
        });
        let st = state.clone();
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                st.counters.connections.fetch_add(1, Ordering::SeqCst);
                let config = st.config.lock().unwrap().clone();
                let handler = SshHandler { state: st.clone(), channels: HashMap::new() };
                // The session talks through a pipe; aborting the pump drops
                // the TCP socket like a dead network, without a goodbye.
                let (inner, mut outer) = tokio::io::duplex(1 << 16);
                let done = st.clone();
                tokio::spawn(async move {
                    if let Ok(session) = russh::server::run_stream(config, inner, handler).await {
                        let _ = session.await;
                    }
                    done.counters.closed.fetch_add(1, Ordering::SeqCst);
                });
                let pump = tokio::spawn(async move {
                    let mut sock = sock;
                    let _ = tokio::io::copy_bidirectional(&mut sock, &mut outer).await;
                });
                st.sessions.lock().unwrap().push(pump.abort_handle());
            }
        });
        Self { port, state }
    }

    pub fn set_host_key(&self, key: PrivateKey) {
        *self.state.config.lock().unwrap() = server_config(key);
    }

    pub fn authorize(&self, key: &PublicKey) {
        self.state.authorized.lock().unwrap().push(key.clone());
    }

    /// Kill every live session, like a server reboot or network drop.
    pub fn drop_sessions(&self) {
        for s in self.state.sessions.lock().unwrap().drain(..) {
            s.abort();
        }
    }

    pub fn count(&self, f: impl Fn(&Counters) -> &AtomicUsize) -> usize {
        f(&self.state.counters).load(Ordering::SeqCst)
    }

    /// Wait (up to 5 s) until every session the server accepted has ended.
    pub async fn all_closed(&self) -> bool {
        for _ in 0..100 {
            if self.count(|c| &c.closed) == self.count(|c| &c.connections) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }
}

struct SshHandler {
    state: Arc<ServerState>,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl russh::server::Handler for SshHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if user == USER && password == PASSWORD {
            return Ok(Auth::Accept);
        }
        // Like OpenSSH: a wrong password may be retried (russh would drop
        // "password" from the methods left).
        let methods = [russh::MethodKind::PublicKey, russh::MethodKind::Password];
        Ok(Auth::Reject { proceed_with_methods: Some((&methods[..]).into()), partial_success: false })
    }

    async fn auth_publickey(&mut self, user: &str, key: VerifiedPublicKey<'_>) -> Result<Auth, Self::Error> {
        let ok = user == USER
            && self.state.authorized.lock().unwrap().iter().any(|k| k.key_data() == key.public_key.key_data());
        Ok(if ok { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    /// Act as a jump host: connect to the target and pump bytes.
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.state.counters.forwards.fetch_add(1, Ordering::SeqCst);
        if !*self.state.allow_forwarding.lock().unwrap() {
            reply.reject(russh::ChannelOpenFailure::AdministrativelyProhibited).await;
            return Ok(());
        }
        let Ok(port) = u16::try_from(port_to_connect) else {
            reply.reject(russh::ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        };
        match tokio::net::TcpStream::connect((host_to_connect, port)).await {
            Ok(mut tcp) => {
                reply.accept().await;
                tokio::spawn(async move {
                    let mut stream = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
                });
            }
            Err(_) => reply.reject(russh::ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    async fn subsystem_request(
        &mut self,
        id: ChannelId,
        name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match self.channels.remove(&id) {
            Some(channel) if name == "sftp" => {
                session.channel_success(id)?;
                russh_sftp::server::run(channel.into_stream(), FsSftp::new(self.state.clone())).await;
            }
            _ => session.channel_failure(id)?,
        }
        Ok(())
    }

    async fn exec_request(&mut self, id: ChannelId, data: &[u8], session: &mut Session) -> Result<(), Self::Error> {
        self.state.counters.execs.fetch_add(1, Ordering::SeqCst);
        let allowed = *self.state.allow_exec.lock().unwrap();
        let Some(channel) = self.channels.remove(&id).filter(|_| allowed) else {
            session.channel_failure(id)?;
            return Ok(());
        };
        session.channel_success(id)?;
        let cmd = String::from_utf8_lossy(data).into_owned();
        tokio::spawn(async move {
            let out = tokio::task::spawn_blocking(move || {
                std::process::Command::new("sh").arg("-c").arg(cmd).stdin(std::process::Stdio::null()).output()
            })
            .await
            .unwrap()
            .unwrap();
            let _ = channel.data_bytes(out.stdout).await;
            let _ = channel.extended_data_bytes(1, out.stderr).await;
            let _ = channel.exit_status(out.status.code().unwrap_or(255) as u32).await;
            let _ = channel.eof().await;
            let _ = channel.close().await;
        });
        Ok(())
    }
}

// ---- SFTP over the local filesystem ---------------------------------------

enum Open {
    File { file: std::fs::File, append: bool },
    Dir(Vec<File>),
}

struct FsSftp {
    state: Arc<ServerState>,
    open: HashMap<String, Open>,
    next: u64,
}

impl FsSftp {
    fn new(state: Arc<ServerState>) -> Self {
        Self { state, open: HashMap::new(), next: 0 }
    }

    fn handle(&mut self, o: Open) -> String {
        self.next += 1;
        let h = self.next.to_string();
        self.open.insert(h.clone(), o);
        h
    }
}

fn io_status(e: std::io::Error) -> StatusCode {
    // SFTP v3 has no EEXIST/ENOTEMPTY codes; OpenSSH sends Failure.
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn attrs(m: &std::fs::Metadata) -> FileAttributes {
    FileAttributes {
        size: Some(m.len()),
        uid: Some(m.uid()),
        gid: Some(m.gid()),
        permissions: Some(m.mode()),
        atime: Some(m.atime() as u32),
        mtime: Some(m.mtime() as u32),
        ..Default::default()
    }
}

fn ok(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

const READDIR_BATCH: usize = 100;

impl russh_sftp::server::Handler for FsSftp {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _v: u32, _ext: HashMap<String, String>) -> Result<Version, Self::Error> {
        let mut v = Version::new();
        v.extensions.insert("statvfs@openssh.com".into(), "2".into());
        Ok(v)
    }

    /// Fixed numbers: 4 KiB fragments, 1000 total, 250 available.
    async fn extended(&mut self, id: u32, request: String, _data: Vec<u8>) -> Result<Packet, Self::Error> {
        if request != "statvfs@openssh.com" {
            return Err(StatusCode::OpUnsupported);
        }
        let s = Statvfs {
            block_size: 8192,
            fragment_size: 4096,
            blocks: 1000,
            blocks_free: 300,
            blocks_avail: 250,
            inodes: 0,
            inodes_free: 0,
            inodes_avail: 0,
            fs_id: 0,
            flags: 0,
            name_max: 255,
        };
        let data = russh_sftp::ser::to_bytes(&s).map_err(|_| StatusCode::Failure)?.to_vec();
        Ok(Packet::ExtendedReply(ExtendedReply { id, data }))
    }

    async fn open(&mut self, id: u32, filename: String, pflags: OpenFlags, _a: FileAttributes) -> Result<Handle, Self::Error> {
        let mut o = std::fs::OpenOptions::new();
        let append = pflags.contains(OpenFlags::APPEND);
        o.read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE))
            .append(append);
        if pflags.contains(OpenFlags::CREATE) {
            if pflags.contains(OpenFlags::EXCLUDE) {
                o.create_new(true);
            } else {
                o.create(true);
            }
        }
        o.truncate(pflags.contains(OpenFlags::TRUNCATE));
        let file = o.open(&filename).map_err(io_status)?;
        Ok(Handle { id, handle: self.handle(Open::File { file, append }) })
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.open.remove(&handle).ok_or(StatusCode::Failure)?;
        Ok(ok(id))
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, Self::Error> {
        let Some(Open::File { file, .. }) = self.open.get_mut(&handle) else { return Err(StatusCode::Failure) };
        file.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        let mut buf = vec![0; len as usize];
        let n = file.read(&mut buf).map_err(io_status)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
        let Some(Open::File { file, append }) = self.open.get_mut(&handle) else { return Err(StatusCode::Failure) };
        if !*append {
            file.seek(SeekFrom::Start(offset)).map_err(io_status)?;
        }
        file.write_all(&data).map_err(io_status)?;
        Ok(ok(id))
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.state.counters.stats.fetch_add(1, Ordering::SeqCst);
        let m = std::fs::symlink_metadata(&path).map_err(io_status)?;
        Ok(Attrs { id, attrs: attrs(&m) })
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.state.counters.stats.fetch_add(1, Ordering::SeqCst);
        let m = std::fs::metadata(&path).map_err(io_status)?;
        Ok(Attrs { id, attrs: attrs(&m) })
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let Some(Open::File { file, .. }) = self.open.get(&handle) else { return Err(StatusCode::Failure) };
        let m = file.metadata().map_err(io_status)?;
        Ok(Attrs { id, attrs: attrs(&m) })
    }

    async fn setstat(&mut self, id: u32, _p: String, _a: FileAttributes) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn fsetstat(&mut self, id: u32, _h: String, _a: FileAttributes) -> Result<Status, Self::Error> {
        Ok(ok(id))
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let mut files = vec![File::new(".", FileAttributes::default()), File::new("..", FileAttributes::default())];
        for e in std::fs::read_dir(&path).map_err(io_status)? {
            let e = e.map_err(io_status)?;
            let m = std::fs::symlink_metadata(e.path()).map_err(io_status)?;
            files.push(File::new(e.file_name().to_string_lossy(), attrs(&m)));
        }
        files.reverse(); // drained from the back
        Ok(Handle { id, handle: self.handle(Open::Dir(files)) })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        self.state.counters.readdirs.fetch_add(1, Ordering::SeqCst);
        let Some(Open::Dir(files)) = self.open.get_mut(&handle) else { return Err(StatusCode::Failure) };
        if files.is_empty() {
            return Err(StatusCode::Eof);
        }
        let n = files.len().min(READDIR_BATCH);
        let batch = files.split_off(files.len() - n);
        Ok(Name { id, files: batch })
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(&filename).map_err(io_status)?;
        Ok(ok(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _a: FileAttributes) -> Result<Status, Self::Error> {
        std::fs::create_dir(&path).map_err(io_status)?;
        Ok(ok(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        std::fs::remove_dir(&path).map_err(io_status)?;
        Ok(ok(id))
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        let p = if path == "." { std::env::temp_dir() } else { PathBuf::from(&path) };
        let p = std::fs::canonicalize(p).map_err(io_status)?;
        Ok(Name { id, files: vec![File::dummy(p.to_string_lossy())] })
    }

    async fn rename(&mut self, id: u32, old: String, new: String) -> Result<Status, Self::Error> {
        // Like OpenSSH's link()+unlink(): never replaces an existing file.
        if Path::new(&new).symlink_metadata().is_ok() {
            return Err(StatusCode::Failure);
        }
        std::fs::rename(&old, &new).map_err(io_status)?;
        Ok(ok(id))
    }
}

// ---- client side helpers --------------------------------------------------

/// Canned answers; counts how often each question was asked.
#[derive(Default)]
pub struct TestPrompter {
    pub accept_host_key: bool,
    pub password: Option<String>,
    pub passphrase: Option<String>,
    /// Ticks "Remember in keychain" on every answer.
    pub remember: bool,
    pub host_key_asked: AtomicUsize,
    pub password_asked: AtomicUsize,
    pub passphrase_asked: AtomicUsize,
}

impl TestPrompter {
    pub fn trusting() -> Self {
        Self { accept_host_key: true, ..Default::default() }
    }

    pub fn asked(&self) -> (usize, usize, usize) {
        (
            self.host_key_asked.load(Ordering::SeqCst),
            self.password_asked.load(Ordering::SeqCst),
            self.passphrase_asked.load(Ordering::SeqCst),
        )
    }
}

#[async_trait]
impl Prompter for TestPrompter {
    async fn confirm_host_key(&self, _host: &str, algorithm: &str, fingerprint: &str) -> bool {
        assert_eq!(algorithm, "ssh-ed25519");
        assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
        self.host_key_asked.fetch_add(1, Ordering::SeqCst);
        self.accept_host_key
    }

    async fn password(&self, _host: &str, user: &str) -> Option<Secret> {
        assert_eq!(user, USER);
        self.password_asked.fetch_add(1, Ordering::SeqCst);
        self.password.clone().map(|value| Secret { value, remember: self.remember })
    }

    async fn passphrase(&self, _host: &str, _key_path: &str) -> Option<Secret> {
        self.passphrase_asked.fetch_add(1, Ordering::SeqCst);
        self.passphrase.clone().map(|value| Secret { value, remember: self.remember })
    }
}

/// An in-memory keychain. `broken` makes every call fail like Linux
/// without a Secret Service.
#[derive(Debug, Default)]
pub struct MemorySecrets {
    pub items: Mutex<HashMap<String, String>>,
    pub broken: AtomicBool,
    pub calls: AtomicUsize,
}

impl MemorySecrets {
    fn check(&self) -> Result<(), VfsError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.broken.load(Ordering::SeqCst) {
            return Err(VfsError::Other("no secret service".into()));
        }
        Ok(())
    }

    /// The saved JSON item for `alias`, parsed.
    pub fn item(&self, alias: &str) -> Option<serde_json::Value> {
        let items = self.items.lock().unwrap();
        items.get(&format!("host:{alias}")).map(|s| serde_json::from_str(s).unwrap())
    }

    pub fn put(&self, alias: &str, json: serde_json::Value) {
        self.items.lock().unwrap().insert(format!("host:{alias}"), json.to_string());
    }
}

impl SecretStore for MemorySecrets {
    fn get(&self, account: &str) -> Result<Option<String>, VfsError> {
        self.check()?;
        Ok(self.items.lock().unwrap().get(account).cloned())
    }

    fn set(&self, account: &str, secret: &str) -> Result<(), VfsError> {
        self.check()?;
        self.items.lock().unwrap().insert(account.into(), secret.into());
        Ok(())
    }

    fn delete(&self, account: &str) -> Result<(), VfsError> {
        self.check()?;
        self.items.lock().unwrap().remove(account);
        Ok(())
    }
}

/// A fake `~/.ssh` in a temp dir: config pointing at the test server,
/// known_hosts, keys, hosts.json, an in-memory keychain. The agent is off
/// unless a test turns it on.
pub struct Home {
    pub dir: tempfile::TempDir,
    pub secrets: Arc<MemorySecrets>,
}

impl Home {
    pub fn new() -> Self {
        Self { dir: tempfile::tempdir().unwrap(), secrets: Arc::default() }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub fn write(&self, name: &str, text: &str) -> PathBuf {
        let p = self.path(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    /// `Host test` → the server, plus `extra` lines in that block.
    pub fn config(&self, port: u16, extra: &str) {
        self.write("config", &format!("Host test\n  HostName 127.0.0.1\n  Port {port}\n  User {USER}\n{extra}"));
    }

    pub fn options(&self) -> SshOptions {
        SshOptions {
            config_path: self.path("config"),
            known_hosts: self.path("known_hosts"),
            hosts_file: self.path("app/hosts.json"),
            secrets: self.secrets.clone(),
            agent: None,
            default_identities: vec![self.path("id_ed25519")],
            keepalive: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
        }
    }

    pub fn manager(&self, prompter: Arc<TestPrompter>) -> SshManager {
        SshManager::with_options(prompter, self.options())
    }

    pub fn known_hosts(&self) -> String {
        std::fs::read_to_string(self.path("known_hosts")).unwrap_or_default()
    }
}

/// Server already trusted + password auth: the common setup.
pub async fn connected() -> (TestServer, Home, Arc<TestPrompter>, SshManager) {
    let server = TestServer::start(key(1)).await;
    let home = Home::new();
    home.config(server.port, "");
    let prompter =
        Arc::new(TestPrompter { accept_host_key: true, password: Some(PASSWORD.into()), ..Default::default() });
    let mgr = home.manager(prompter.clone());
    (server, home, prompter, mgr)
}
