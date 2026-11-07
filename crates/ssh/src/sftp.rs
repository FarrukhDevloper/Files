//! A connected host's filesystem over SFTP (protocol v3, OpenSSH flavour).

use std::io::SeekFrom;
use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use russh::client::Handle;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::{FileAttributes, OpenFlags, StatusCode};
use tokio::io::AsyncSeekExt;
use vpsfiles_vfs::{
    path, Backend, BoxRead, BoxWrite, Capabilities, DiskSpace, Entry, EntryKind, Result, SourceId, VfsError, WriteMode,
};

use crate::manager::{exec, ssh_error, Client};

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

/// Symlink targets stat'ed in parallel while listing.
const LINK_STATS_IN_FLIGHT: usize = 16;

/// A connected host's filesystem. Implements [`vpsfiles_vfs::Backend`].
pub struct SftpBackend {
    host: String,
    sftp: SftpSession,
    /// Same connection, for `exec` (fast `rm -rf`) and liveness checks.
    handle: Arc<Handle<Client>>,
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

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
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
    pub(crate) async fn open(alias: &str, handle: Arc<Handle<Client>>) -> Result<Self> {
        let channel = handle.channel_open_session().await.map_err(|e| ssh_error(alias, e))?;
        channel.request_subsystem(true, "sftp").await.map_err(|e| ssh_error(alias, e))?;
        // 16 x 256 KiB reads in flight can take a while on a slow uplink.
        let cfg = russh_sftp::client::Config { request_timeout_secs: 60, ..Default::default() };
        let sftp = SftpSession::new_with_config(channel.into_stream(), cfg)
            .await
            .map_err(|e| VfsError::NotConnected(format!("{alias}: SFTP unavailable: {e}")))?;
        Ok(Self { host: alias.to_string(), sftp, handle })
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

    async fn explain(&self, e: SftpError, path: &str, op: Op) -> VfsError {
        if status_code(&e) != Some(StatusCode::Failure) {
            return self.err(e, path);
        }
        let p = path.to_string();
        let lstat = self.sftp.symlink_metadata(path).await.ok();
        let stat = match &lstat {
            Some(l) if is_link(l) => self.sftp.metadata(path).await.ok(),
            other => other.clone(),
        };
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
        self.sftp.canonicalize(".").await.map_err(|e| self.err(e, "."))
    }

    async fn list(&self, dir: &str) -> Result<Vec<Entry>> {
        // readdir returns attrs in batches (~100 per round trip); only
        // symlinks need an extra stat to learn what they point at.
        let rd = match self.sftp.read_dir(dir).await {
            Ok(rd) => rd,
            Err(e) => return Err(self.explain(e, dir, Op::List).await),
        };
        let raw: Vec<(String, String, FileAttributes)> = rd
            .map(|item| {
                let name = item.file_name();
                let p = if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") };
                (name, p, item.metadata())
            })
            .collect();
        let links: Vec<(usize, String)> =
            raw.iter().enumerate().filter(|(_, (_, _, a))| is_link(a)).map(|(i, (_, p, _))| (i, p.clone())).collect();
        let sftp = &self.sftp;
        let mut targets: Vec<Option<FileAttributes>> = vec![None; raw.len()];
        let mut stats = futures::stream::iter(links)
            .map(|(i, p)| async move { (i, sftp.metadata(p).await.ok()) })
            .buffer_unordered(LINK_STATS_IN_FLIGHT);
        while let Some((i, t)) = stats.next().await {
            targets[i] = t;
        }
        drop(stats);
        Ok(raw
            .into_iter()
            .enumerate()
            .map(|(i, (name, p, attrs))| to_entry(name, p, &attrs, targets[i].take().as_ref()))
            .collect())
    }

    async fn stat(&self, p: &str) -> Result<Entry> {
        let lattrs = self.sftp.symlink_metadata(p).await.map_err(|e| self.err(e, p))?;
        let target = if is_link(&lattrs) { self.sftp.metadata(p).await.ok() } else { None };
        Ok(to_entry(path::file_name(p).to_string(), p.to_string(), &lattrs, target.as_ref()))
    }

    async fn mkdir(&self, p: &str) -> Result<()> {
        match self.sftp.create_dir(p).await {
            Ok(()) => Ok(()),
            Err(e) => Err(self.explain(e, p, Op::Mkdir).await),
        }
    }

    async fn remove_file(&self, p: &str) -> Result<()> {
        match self.sftp.remove_file(p).await {
            Ok(()) => Ok(()),
            Err(e) => Err(self.explain(e, p, Op::RemoveFile).await),
        }
    }

    async fn remove_dir(&self, p: &str) -> Result<()> {
        match self.sftp.remove_dir(p).await {
            Ok(()) => Ok(()),
            Err(e) => Err(self.explain(e, p, Op::RemoveDir).await),
        }
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        // OpenSSH's plain rename already refuses to replace files, but it
        // replaces empty dirs and other servers differ: check first, like
        // LocalBackend.
        if self.sftp.symlink_metadata(to).await.is_ok() {
            return Err(VfsError::AlreadyExists(to.to_string()));
        }
        match self.sftp.rename(from, to).await {
            Ok(()) => Ok(()),
            Err(e) if status_code(&e) == Some(StatusCode::NoSuchFile) => {
                // Missing source, or missing target directory.
                if self.sftp.symlink_metadata(from).await.is_err() {
                    Err(VfsError::NotFound(from.to_string()))
                } else {
                    Err(VfsError::NotFound(to.to_string()))
                }
            }
            Err(e) => Err(self.explain(e, from, Op::Other).await),
        }
    }

    async fn open_read(&self, p: &str, offset: u64) -> Result<BoxRead> {
        let mut f = self.sftp.open(p).await.map_err(|e| self.err(e, p))?;
        if offset > 0 {
            f.seek(SeekFrom::Start(offset)).await.map_err(|e| VfsError::from_io(e, p))?;
        }
        Ok(Box::pin(f))
    }

    async fn open_write(&self, p: &str, mode: WriteMode) -> Result<BoxWrite> {
        let (flags, op) = match mode {
            WriteMode::CreateNew => (OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE, Op::CreateNew),
            WriteMode::Truncate => (OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE, Op::OpenWrite),
            WriteMode::Append => (OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::APPEND, Op::OpenWrite),
        };
        let mut f = match self.sftp.open_with_flags(p, flags).await {
            Ok(f) => f,
            Err(e) => return Err(self.explain(e, p, op).await),
        };
        if mode == WriteMode::Append {
            // Servers that ignore APPEND still write at our offset: start at
            // the current end either way.
            let size = f.metadata().await.map_err(|e| self.err(e, p))?.size.unwrap_or(0);
            f.seek(SeekFrom::Start(size)).await.map_err(|e| VfsError::from_io(e, p))?;
        }
        Ok(Box::pin(f))
    }

    async fn disk_space(&self, p: &str) -> Result<Option<DiskSpace>> {
        // statvfs@openssh.com; servers without it just show no space info.
        match self.sftp.fs_info(p).await {
            Ok(Some(s)) => {
                let unit = if s.fragment_size > 0 { s.fragment_size } else { s.block_size };
                Ok(Some(DiskSpace {
                    free: s.blocks_avail.saturating_mul(unit),
                    total: s.blocks.saturating_mul(unit),
                }))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(self.err(e, p)),
        }
    }

    async fn remove_all(&self, p: &str) -> Result<()> {
        if !p.starts_with('/') || path::is_root(p) {
            return Err(VfsError::PermissionDenied(format!("refusing to delete {p}")));
        }
        // Missing path -> NotFound, like LocalBackend.
        self.sftp.symlink_metadata(p).await.map_err(|e| self.err(e, p))?;

        // One command instead of a round trip per file. Trust it only if
        // the path is really gone (an sftp-only account runs no shell).
        if let Ok(out) = exec(&self.handle, &self.host, &format!("rm -rf -- {}", shell_quote(p))).await {
            if out.exit_status == 0 {
                match self.sftp.symlink_metadata(p).await {
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
    fn quoting() {
        assert_eq!(shell_quote("/a b"), "'/a b'");
        assert_eq!(shell_quote("/it's"), r"'/it'\''s'");
        assert_eq!(shell_quote("/$(x)`y`"), "'/$(x)`y`'");
    }

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
