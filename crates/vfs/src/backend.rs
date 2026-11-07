use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{Entry, EntryKind, Result, SourceId, VfsError};

pub type BoxRead = Pin<Box<dyn AsyncRead + Send>>;
pub type BoxWrite = Pin<Box<dyn AsyncWrite + Send>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WriteMode {
    /// Fail with `AlreadyExists` if the file exists.
    CreateNew,
    /// Create or truncate.
    Truncate,
    /// Create or append — used to resume interrupted transfers.
    Append,
}

/// Size of the filesystem (volume) that holds a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskSpace {
    /// Bytes available to this user.
    pub free: u64,
    pub total: u64,
}

/// What a backend can do beyond the required trait methods.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// `trash()` moves to the OS trash instead of failing.
    pub trash: bool,
    /// The source can run shell commands (SSH). Enables direct VPS→VPS copy.
    pub exec: bool,
}

/// A browsable filesystem: the local disk or one SSH host.
///
/// Paths follow [`crate::path`]: absolute, `/`-separated. Implementations
/// must be cheap to share (`Arc<dyn Backend>`) and safe to call concurrently.
#[async_trait]
pub trait Backend: Send + Sync + 'static {
    fn source(&self) -> SourceId;

    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    /// The user's home directory on this source.
    async fn home(&self) -> Result<String>;

    /// Directory contents, unsorted, without `.` and `..`.
    async fn list(&self, path: &str) -> Result<Vec<Entry>>;

    /// Follows symlinks for `kind`/`size`, but reports `is_symlink`.
    async fn stat(&self, path: &str) -> Result<Entry>;

    /// Create one directory. Parent must exist.
    async fn mkdir(&self, path: &str) -> Result<()>;

    async fn remove_file(&self, path: &str) -> Result<()>;

    /// Remove an empty directory.
    async fn remove_dir(&self, path: &str) -> Result<()>;

    /// Rename or move within the same source. Fails if `to` exists.
    async fn rename(&self, from: &str, to: &str) -> Result<()>;

    /// Stream the file starting at `offset` bytes.
    async fn open_read(&self, path: &str, offset: u64) -> Result<BoxRead>;

    async fn open_write(&self, path: &str, mode: WriteMode) -> Result<BoxWrite>;

    /// Recursively delete. Backends with a faster native path (local
    /// `remove_dir_all`, remote `rm -rf`) should override this.
    async fn remove_all(&self, path: &str) -> Result<()> {
        let entry = self.stat(path).await?;
        if entry.kind != EntryKind::Dir || entry.is_symlink {
            return self.remove_file(path).await;
        }
        // Iterative post-order walk: avoids async recursion.
        let mut stack = vec![(path.to_string(), false)];
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

    /// Free/total space of the volume holding `path`; `None` when unknown.
    async fn disk_space(&self, _path: &str) -> Result<Option<DiskSpace>> {
        Ok(None)
    }

    /// Move to the OS trash. Only meaningful when `capabilities().trash`.
    async fn trash(&self, path: &str) -> Result<()> {
        Err(VfsError::Unsupported(format!("trash is not available for {path}")))
    }
}

/// Turns a [`SourceId`] into a live backend, connecting on demand.
/// Implemented by the app; consumed by the transfer engine and media server.
#[async_trait]
pub trait BackendResolver: Send + Sync + 'static {
    async fn backend(&self, source: &SourceId) -> Result<Arc<dyn Backend>>;
}
