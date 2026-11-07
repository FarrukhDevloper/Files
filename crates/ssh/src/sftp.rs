//! A connected host's filesystem over SFTP (protocol v3, OpenSSH flavour),
//! on the raw session so requests can be pipelined:
//!
//! * `list`: OPENDIR, then READDIRs in flight together, CLOSE unawaited,
//!   symlink targets stat'ed in one parallel batch → 2 round trips (3 with
//!   symlinks).
//! * `stat`: LSTAT and STAT together → 1 round trip.
//! * reads/writes: see `sftp_io`; read handles are reused for a few seconds.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::stream::{FuturesOrdered, StreamExt};
use russh::client::Handle;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use vpsfiles_vfs::{
    path, Backend, BoxRead, BoxWrite, Capabilities, DiskSpace, Entry, EntryKind, Result, SourceId, VfsError, WriteMode,
};

use crate::manager::{exec, ssh_error, Client, SshTuning};
use crate::shell;
use crate::sftp_io::{close_later, IdleHandles, Reader, Stamp, Tuning, Writer, MAX_IN_FLIGHT};

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

/// READDIRs in flight: starts at 2 (≤ ~100 entries done in one round
/// trip), grows for big directories.
const READDIR_FIRST: usize = 2;
const READDIR_MAX: usize = 32;
/// A stat this recent may vouch for an idle read handle.
const STAT_FRESH: Duration = Duration::from_secs(3);
const MAX_RECENT_STATS: usize = 512;

/// A connected host's filesystem. Implements [`vpsfiles_vfs::Backend`].
pub struct SftpBackend {
    host: String,
    raw: Arc<RawSftpSession>,
    /// Same connection, for `exec` (fast `rm -rf`) and liveness checks.
    handle: Arc<Handle<Client>>,
    tuning: Arc<Tuning>,
    statvfs: bool,
    idle: Arc<IdleHandles>,
    /// path → what the last `stat` saw, for reusing idle handles.
    recent: Mutex<HashMap<String, (Stamp, Instant)>>,
}

fn file_type(a: &FileAttributes) -> u32 {
    a.permissions.unwrap_or(0) & S_IFMT
}

fn is_link(a: &FileAttributes) -> bool {
    file_type(a) == S_IFLNK
}

fn is_dir(a: &FileAttributes) -> bool {
    file_type(a) == S_IFDIR
}

/// `lattrs` are lstat-style; `target` is the followed stat of a symlink
/// (`None` when broken), mirroring `LocalBackend`.
fn to_entry(name: String, path: String, lattrs: &FileAttributes, target: Option<&FileAttributes>) -> Entry {
    let is_symlink = is_link(lattrs);
    let shown = if is_symlink { target } else { Some(lattrs) };
    let kind = match shown.map(file_type) {
        Some(S_IFDIR) => EntryKind::Dir,
        Some(S_IFREG) => EntryKind::File,
        _ => EntryKind::Other,
    };
    let shown = shown.unwrap_or(lattrs);
    Entry {
        is_hidden: name.starts_with('.'),
        name,
        path,
        kind,
        size: if kind == EntryKind::File { shown.size.unwrap_or(0) } else { 0 },
        modified: shown.mtime.map(|s| i64::from(s) * 1000),
        mode: shown.permissions.map(|p| p & 0o7777),
        is_symlink,
    }
}

fn status_code(e: &SftpError) -> Option<StatusCode> {
    match e {
        SftpError::Status(s) => Some(s.status_code),
        _ => None,
    }
}

/// What was being attempted, to turn SFTP v3's catch-all `Failure` (it has
/// no EEXIST/ENOTEMPTY/ENOTDIR codes) into the precise `VfsError`.
#[derive(Clone, Copy)]
enum Op {
    List,
    Mkdir,
    CreateNew,
    OpenWrite,
    RemoveFile,
    RemoveDir,
    Other,
}

impl SftpBackend {
    pub(crate) async fn open(
        alias: &str,
        handle: Arc<Handle<Client>>,
        keep_handles: Duration,
        tuning: SshTuning,
    ) -> Result<Self> {
        let channel = handle.channel_open_session().await.map_err(|e| ssh_error(alias, e))?;
        channel.request_subsystem(true, "sftp").await.map_err(|e| ssh_error(alias, e))?;
        // Many big reads in flight can take a while on a slow uplink.
        let cfg = russh_sftp::client::Config { request_timeout_secs: 60, ..Default::default() };
        let raw = Arc::new(RawSftpSession::new_with_config(channel.into_stream(), cfg));
        let version =
            raw.init().await.map_err(|e| VfsError::NotConnected(format!("{alias}: SFTP unavailable: {e}")))?;
        let has = |name: &str, ver: &str| version.extensions.get(name).is_some_and(|v| v == ver);
        let tuning = Arc::new(Tuning::new(tuning.requests_in_flight, tuning.max_chunk));
        if has(russh_sftp::extensions::LIMITS, "1") {
            // Not waited for: until it's back, requests use the safe size.
            let (raw, tuning, alias) = (raw.clone(), tuning.clone(), alias.to_string());
            tokio::spawn(async move {
                match raw.limits().await {
                    Ok(l) => {
                        tracing::debug!(target: "vpsfiles_ssh::io", "{alias}: server limits {l:?}");
                        tuning.set_limits(l.max_read_len, l.max_write_len);
                    }
                    Err(e) => tracing::debug!("{alias}: limits@openssh.com: {e}"),
                }
            });
        }
        Ok(Self {
            host: alias.to_string(),
            idle: IdleHandles::new(raw.clone(), keep_handles),
            raw,
            handle,
            tuning,
            statvfs: has(russh_sftp::extensions::STATVFS, "2"),
            recent: Mutex::new(HashMap::new()),
        })
    }

    fn err(&self, e: SftpError, path: &str) -> VfsError {
        if self.handle.is_closed() {
            return VfsError::NotConnected(format!("{}: connection lost", self.host));
        }
        let p = path.to_string();
        match e {
            SftpError::Status(s) => {
                let msg = if s.error_message.is_empty() { s.status_code.to_string() } else { s.error_message };
                match s.status_code {
                    StatusCode::NoSuchFile => VfsError::NotFound(p),
                    StatusCode::PermissionDenied => VfsError::PermissionDenied(p),
                    StatusCode::OpUnsupported => VfsError::Unsupported(format!("{path}: {msg}")),
                    StatusCode::NoConnection | StatusCode::ConnectionLost => {
                        VfsError::NotConnected(format!("{}: {msg}", self.host))
                    }
                    _ => VfsError::Io(format!("{path}: {msg}")),
                }
            }
            SftpError::Timeout => VfsError::Io(format!("{path}: {} did not answer in time", self.host)),
            other => VfsError::Io(format!("{path}: {other}")),
        }
    }

    async fn lstat(&self, p: &str) -> std::result::Result<FileAttributes, SftpError> {
        self.raw.lstat(p).await.map(|a| a.attrs)
    }

    async fn explain(&self, e: SftpError, path: &str, op: Op) -> VfsError {
        if status_code(&e) != Some(StatusCode::Failure) {
            return self.err(e, path);
        }
        let p = path.to_string();
        let (lstat, stat) = futures::join!(self.raw.lstat(path), self.raw.stat(path));
        let lstat = lstat.ok().map(|a| a.attrs);
        let stat = stat.ok().map(|a| a.attrs);
        match op {
            Op::Mkdir | Op::CreateNew if lstat.is_some() => VfsError::AlreadyExists(p),
            Op::OpenWrite if stat.as_ref().is_some_and(is_dir) => VfsError::IsADirectory(p),
            Op::RemoveFile if lstat.as_ref().is_some_and(is_dir) => VfsError::IsADirectory(p),
            Op::RemoveDir if lstat.as_ref().is_some_and(|a| !is_dir(a)) => VfsError::NotADirectory(p),
            Op::RemoveDir if lstat.is_some() => VfsError::NotEmpty(p),
            Op::List if stat.as_ref().is_some_and(|a| !is_dir(a)) => VfsError::NotADirectory(p),
            _ => self.err(e, path),
        }
    }

    /// Remember what a stat saw, for `open_read` to trust an idle handle.
    fn saw(&self, p: &str, a: &FileAttributes) {
        let mut recent = self.recent.lock().unwrap();
        if recent.len() >= MAX_RECENT_STATS {
            recent.retain(|_, (_, at)| at.elapsed() < STAT_FRESH);
            if recent.len() >= MAX_RECENT_STATS {
                recent.clear();
            }
        }
        recent.insert(p.to_string(), (Stamp::of(a), Instant::now()));
    }

    fn fresh_stamp(&self, p: &str) -> Option<Stamp> {
        let recent = self.recent.lock().unwrap();
        recent.get(p).filter(|(_, at)| at.elapsed() < STAT_FRESH).map(|(s, _)| *s)
    }

    /// Post-order walk over SFTP (the trait's default, which an override
    /// can't call). Used when `rm -rf` isn't available.
    async fn walk_remove(&self, root: &str) -> Result<()> {
        let mut stack = vec![(root.to_string(), false)];
        while let Some((dir, visited)) = stack.pop() {
            if visited {
                self.remove_dir(&dir).await?;
                continue;
            }
            stack.push((dir.clone(), true));
            for child in self.list(&dir).await? {
                if child.kind == EntryKind::Dir && !child.is_symlink {
                    stack.push((child.path, false));
                } else {
                    self.remove_file(&child.path).await?;
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Backend for SftpBackend {
    fn source(&self) -> SourceId {
        SourceId::ssh(&self.host)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { trash: false, exec: true }
    }

    async fn home(&self) -> Result<String> {
        let name = self.raw.realpath(".").await.map_err(|e| self.err(e, "."))?;
        name.files.into_iter().next().map(|f| f.filename).ok_or_else(|| VfsError::Io(format!("{}: no home", self.host)))
    }

    async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        let handle = match self.raw.opendir(dir).await {
            Ok(h) => h.handle,
            Err(e) => return Err(self.explain(e, dir, Op::List).await),
        };
        // READDIRs on one handle are answered in order; asking for several
        // at once costs nothing extra (extra ones just say EOF).
        let mut files = Vec::new();
        let mut pending = FuturesOrdered::new();
        let mut window = READDIR_FIRST;
        let read = |raw: Arc<RawSftpSession>, h: String| async move { raw.readdir(h).await };
        let mut failed = None;
        loop {
            while pending.len() < window {
                pending.push_back(read(self.raw.clone(), handle.clone()));
            }
            match pending.next().await {
                Some(Ok(name)) => {
                    files.extend(name.files);
                    window = (window + 2).min(READDIR_MAX);
                }
                Some(Err(e)) if status_code(&e) == Some(StatusCode::Eof) => break,
                Some(Err(e)) => {
                    failed = Some(e);
                    break;
                }
                None => break,
            }
        }
        drop(pending);
        close_later(&self.raw, handle);
        if let Some(e) = failed {
            return Err(self.explain(e, dir, Op::List).await);
        }

        // Only symlinks need another look, all at once.
        let path_of = |name: &str| if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") };
        let links: Vec<(usize, String)> =
            files.iter().enumerate().filter(|(_, f)| is_link(&f.attrs)).map(|(i, f)| (i, path_of(&f.filename))).collect();
        let mut targets: HashMap<usize, FileAttributes> = HashMap::new();
        let mut stats = futures::stream::iter(links)
            .map(|(i, p)| {
                let raw = self.raw.clone();
                async move { (i, raw.stat(p).await.ok()) }
            })
            .buffer_unordered(MAX_IN_FLIGHT);
        while let Some((i, t)) = stats.next().await {
            if let Some(t) = t {
                targets.insert(i, t.attrs);
            }
        }
        drop(stats);
        Ok(files
            .into_iter()
            .enumerate()
            .filter(|(_, f)| f.filename != "." && f.filename != "..")
            .map(|(i, f)| {
                let p = path_of(&f.filename);
                to_entry(f.filename, p, &f.attrs, targets.get(&i))
            })
            .collect())
    }

    async fn stat(&self, p: &str) -> Result<Entry> {
        // Both at once: a symlink needs the second answer, others ignore it.
        let (lstat, stat) = futures::join!(self.raw.lstat(p), self.raw.stat(p));
        let lattrs = lstat.map_err(|e| self.err(e, p))?.attrs;
        let target = if is_link(&lattrs) { stat.ok().map(|a| a.attrs) } else { None };
        let shown = if is_link(&lattrs) { target.as_ref() } else { Some(&lattrs) };
        if let Some(a) = shown.filter(|a| file_type(a) == S_IFREG) {
            self.saw(p, a);
        }
        Ok(to_entry(path::file_name(p).to_string(), p.to_string(), &lattrs, target.as_ref()))
    }

    async fn mkdir(&self, p: &str) -> Result<()> {
        match self.raw.mkdir(p, FileAttributes::empty()).await {
            Ok(_) => Ok(()),
            Err(e) => Err(self.explain(e, p, Op::Mkdir).await),
        }
    }

    async fn remove_file(&self, p: &str) -> Result<()> {
        match self.raw.remove(p).await {
            Ok(_) => Ok(()),
            Err(e) => Err(self.explain(e, p, Op::RemoveFile).await),
        }
    }

    async fn remove_dir(&self, p: &str) -> Result<()> {
        match self.raw.rmdir(p).await {
            Ok(_) => Ok(()),
            Err(e) => Err(self.explain(e, p, Op::RemoveDir).await),
        }
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        // OpenSSH's plain rename already refuses to replace files, but it
        // replaces empty dirs and other servers differ: check first, like
        // LocalBackend.
        if self.lstat(to).await.is_ok() {
            return Err(VfsError::AlreadyExists(to.to_string()));
        }
        match self.raw.rename(from, to).await {
            Ok(_) => Ok(()),
            Err(e) if status_code(&e) == Some(StatusCode::NoSuchFile) => {
                // Missing source, or missing target directory.
                if self.lstat(from).await.is_err() {
                    Err(VfsError::NotFound(from.to_string()))
                } else {
                    Err(VfsError::NotFound(to.to_string()))
                }
            }
            Err(e) => Err(self.explain(e, from, Op::Other).await),
        }
    }

    async fn open_read(&self, p: &str, offset: u64) -> Result<BoxRead> {
        let idle = Some((self.idle.clone(), p.to_string()));
        // An idle handle is trusted only if a stat just now still shows the
        // file it was opened on (not replaced or changed since).
        if let Some(stamp) = self.fresh_stamp(p)
            && let Some(h) = self.idle.take(p, stamp)
        {
            return Ok(Box::pin(Reader::new(self.raw.clone(), &self.tuning, h, offset, Some(stamp), idle)));
        }
        let h = self.raw.open(p, OpenFlags::READ, FileAttributes::empty()).await.map_err(|e| self.err(e, p))?.handle;
        Ok(Box::pin(Reader::new(self.raw.clone(), &self.tuning, h, offset, None, idle)))
    }

    async fn open_write(&self, p: &str, mode: WriteMode) -> Result<BoxWrite> {
        let (flags, op) = match mode {
            WriteMode::CreateNew => (OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE, Op::CreateNew),
            WriteMode::Truncate => (OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE, Op::OpenWrite),
            WriteMode::Append => (OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::APPEND, Op::OpenWrite),
        };
        let h = match self.raw.open(p, flags, FileAttributes::empty()).await {
            Ok(h) => h.handle,
            Err(e) => return Err(self.explain(e, p, op).await),
        };
        let offset = if mode == WriteMode::Append {
            // Servers that ignore APPEND still write at our offset: start at
            // the current end either way.
            match self.raw.fstat(h.clone()).await {
                Ok(a) => a.attrs.size.unwrap_or(0),
                Err(e) => {
                    close_later(&self.raw, h);
                    return Err(self.err(e, p));
                }
            }
        } else {
            0
        };
        Ok(Box::pin(Writer::new(self.raw.clone(), &self.tuning, h, offset)))
    }

    async fn disk_space(&self, p: &str) -> Result<Option<DiskSpace>> {
        // statvfs@openssh.com; servers without it just show no space info.
        if !self.statvfs {
            return Ok(None);
        }
        let s = self.raw.statvfs(p).await.map_err(|e| self.err(e, p))?;
        let unit = if s.fragment_size > 0 { s.fragment_size } else { s.block_size };
        Ok(Some(DiskSpace { free: s.blocks_avail.saturating_mul(unit), total: s.blocks.saturating_mul(unit) }))
    }

    async fn remove_all(&self, p: &str) -> Result<()> {
        if !p.starts_with('/') || path::is_root(p) {
            return Err(VfsError::PermissionDenied(format!("refusing to delete {p}")));
        }
        // Missing path -> NotFound, like LocalBackend.
        self.lstat(p).await.map_err(|e| self.err(e, p))?;

        // One command instead of a round trip per file. Trust it only if
        // the path is really gone (an sftp-only account runs no shell).
        if let Ok(out) = exec(&self.handle, &self.host, &format!("rm -rf -- {}", shell::quote(p))).await {
            if out.exit_status == 0 {
                match self.lstat(p).await {
                    Err(e) if status_code(&e) == Some(StatusCode::NoSuchFile) => return Ok(()),
                    _ => {}
                }
            } else {
                tracing::debug!("{}: rm -rf failed: {}", self.host, String::from_utf8_lossy(&out.stderr));
            }
        }

        // The walk removes what it can and reports the precise failure.
        let entry = self.stat(p).await?;
        if entry.kind != EntryKind::Dir || entry.is_symlink {
            return self.remove_file(p).await;
        }
        self.walk_remove(p).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_follow_links() {
        let mut file = FileAttributes::empty();
        file.permissions = Some(S_IFREG | 0o644);
        file.size = Some(42);
        file.mtime = Some(10);
        let mut link = FileAttributes::empty();
        link.permissions = Some(S_IFLNK | 0o777);
        link.size = Some(7);

        let e = to_entry(".f".into(), "/d/.f".into(), &file, None);
        assert_eq!((e.kind, e.size, e.modified, e.mode, e.is_hidden), (EntryKind::File, 42, Some(10_000), Some(0o644), true));

        let e = to_entry("l".into(), "/d/l".into(), &link, Some(&file));
        assert_eq!((e.kind, e.size, e.is_symlink), (EntryKind::File, 42, true));

        let e = to_entry("broken".into(), "/d/broken".into(), &link, None);
        assert_eq!((e.kind, e.size, e.is_symlink), (EntryKind::Other, 0, true));
    }
}
