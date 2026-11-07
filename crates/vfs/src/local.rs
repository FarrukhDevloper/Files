//! The local disk as a [`Backend`].
//!
//! Listings run in one `spawn_blocking` call with `std::fs` instead of a
//! tokio future per entry: a 10k-entry directory is one thread hop, not 10k.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use tokio::io::AsyncSeekExt;

use crate::backend::{Backend, BoxRead, BoxWrite, Capabilities, DiskSpace, WriteMode};
use crate::{path, Entry, EntryKind, Place, PlaceKind, Result, SourceId, VfsError};

#[derive(Debug, Default, Clone)]
pub struct LocalBackend;

impl LocalBackend {
    pub fn new() -> Self {
        Self
    }
}

/// VFS path -> OS path.
pub fn to_native(p: &str) -> PathBuf {
    if cfg!(windows) { PathBuf::from(p.replace('/', "\\")) } else { PathBuf::from(p) }
}

/// OS path -> VFS path.
pub fn from_native(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) {
        let s = s.strip_prefix(r"\\?\").unwrap_or(&s).replace('\\', "/");
        // "C:" alone is drive-relative on Windows; the root is "C:/".
        if s.len() == 2 && s.ends_with(':') { format!("{s}/") } else { s }
    } else {
        s.into_owned()
    }
}

fn join_err(e: tokio::task::JoinError) -> VfsError {
    VfsError::Other(format!("background task failed: {e}"))
}

fn millis(t: std::time::SystemTime) -> Option<i64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

fn entry_at(native: &Path, name: String, vpath: String) -> std::io::Result<Entry> {
    let lmeta = std::fs::symlink_metadata(native)?;
    let is_symlink = lmeta.file_type().is_symlink();
    // Follow links for kind/size so the UI can navigate into linked dirs.
    let meta = if is_symlink { std::fs::metadata(native).ok() } else { Some(lmeta.clone()) };
    let kind = match &meta {
        Some(m) if m.is_dir() => EntryKind::Dir,
        Some(m) if m.is_file() => EntryKind::File,
        _ => EntryKind::Other,
    };
    let shown = meta.as_ref().unwrap_or(&lmeta);
    Ok(Entry {
        is_hidden: name.starts_with('.') || os_hidden(&lmeta),
        name,
        path: vpath,
        kind,
        size: if kind == EntryKind::File { shown.len() } else { 0 },
        modified: shown.modified().ok().and_then(millis),
        mode: os_mode(shown),
        is_symlink,
    })
}

#[cfg(unix)]
fn os_mode(m: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(m.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn os_mode(_: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(windows)]
fn os_hidden(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(windows))]
fn os_hidden(_: &std::fs::Metadata) -> bool {
    false
}

fn list_blocking(dir: &str) -> Result<Vec<Entry>> {
    let rd = std::fs::read_dir(to_native(dir)).map_err(|e| VfsError::from_io(e, dir))?;
    let mut out = Vec::new();
    for item in rd.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let vpath = if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") };
        // An entry can vanish or be unreadable mid-listing; skip it rather
        // than failing the whole directory.
        if let Ok(e) = entry_at(&item.path(), name, vpath) {
            out.push(e);
        }
    }
    Ok(out)
}

fn virtual_root() -> Entry {
    Entry {
        name: String::new(),
        path: "/".into(),
        kind: EntryKind::Dir,
        size: 0,
        modified: None,
        mode: None,
        is_symlink: false,
        is_hidden: false,
    }
}

/// Windows drive letters that currently exist, as `C:/` paths.
fn drive_roots() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    // Skip A:/B: — probing a missing floppy can stall for seconds.
    (b'C'..=b'Z')
        .map(|l| format!("{}:/", l as char))
        .filter(|root| to_native(root).exists())
        .collect()
}

#[async_trait]
impl Backend for LocalBackend {
    fn source(&self) -> SourceId {
        SourceId::Local
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { trash: true, exec: false }
    }

    async fn home(&self) -> Result<String> {
        dirs::home_dir()
            .map(|p| from_native(&p))
            .ok_or_else(|| VfsError::NotFound("home directory".into()))
    }

    async fn list(&self, path: &str) -> Result<Vec<Entry>> {
        if cfg!(windows) && path == "/" {
            return Ok(drive_roots()
                .into_iter()
                .map(|root| Entry { name: root[..2].to_string(), path: root, ..virtual_root() })
                .collect());
        }
        let path = path.to_string();
        tokio::task::spawn_blocking(move || list_blocking(&path)).await.map_err(join_err)?
    }

    async fn stat(&self, path: &str) -> Result<Entry> {
        if cfg!(windows) && path == "/" {
            return Ok(virtual_root());
        }
        let vpath = path.to_string();
        tokio::task::spawn_blocking(move || {
            entry_at(&to_native(&vpath), path::file_name(&vpath).to_string(), vpath.clone())
                .map_err(|e| VfsError::from_io(e, &vpath))
        })
        .await
        .map_err(join_err)?
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        tokio::fs::create_dir(to_native(path)).await.map_err(|e| VfsError::from_io(e, path))
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        tokio::fs::remove_file(to_native(path)).await.map_err(|e| VfsError::from_io(e, path))
    }

    async fn remove_dir(&self, path: &str) -> Result<()> {
        tokio::fs::remove_dir(to_native(path)).await.map_err(|e| VfsError::from_io(e, path))
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        // std::fs::rename silently replaces on Unix; the contract says fail.
        if tokio::fs::symlink_metadata(to_native(to)).await.is_ok() {
            return Err(VfsError::AlreadyExists(to.to_string()));
        }
        tokio::fs::rename(to_native(from), to_native(to))
            .await
            .map_err(|e| VfsError::from_io(e, from))
    }

    async fn open_read(&self, path: &str, offset: u64) -> Result<BoxRead> {
        let mut f =
            tokio::fs::File::open(to_native(path)).await.map_err(|e| VfsError::from_io(e, path))?;
        if offset > 0 {
            f.seek(SeekFrom::Start(offset)).await.map_err(|e| VfsError::from_io(e, path))?;
        }
        Ok(Box::pin(f))
    }

    async fn open_write(&self, path: &str, mode: WriteMode) -> Result<BoxWrite> {
        let mut opts = tokio::fs::OpenOptions::new();
        match mode {
            WriteMode::CreateNew => opts.write(true).create_new(true),
            WriteMode::Truncate => opts.write(true).create(true).truncate(true),
            WriteMode::Append => opts.append(true).create(true),
        };
        let f = opts.open(to_native(path)).await.map_err(|e| VfsError::from_io(e, path))?;
        Ok(Box::pin(f))
    }

    async fn remove_all(&self, path: &str) -> Result<()> {
        let native = to_native(path);
        let meta =
            tokio::fs::symlink_metadata(&native).await.map_err(|e| VfsError::from_io(e, path))?;
        let res = if meta.is_dir() {
            tokio::fs::remove_dir_all(&native).await
        } else {
            tokio::fs::remove_file(&native).await
        };
        res.map_err(|e| VfsError::from_io(e, path))
    }

    async fn disk_space(&self, path: &str) -> Result<Option<DiskSpace>> {
        if cfg!(windows) && path == "/" {
            return Ok(None); // "This PC" spans drives
        }
        let native = to_native(path);
        let p = path.to_string();
        tokio::task::spawn_blocking(move || {
            let free = fs4::available_space(&native)?;
            let total = fs4::total_space(&native)?;
            Ok(Some(DiskSpace { free, total }))
        })
        .await
        .map_err(join_err)?
        .map_err(|e: std::io::Error| VfsError::from_io(e, &p))
    }

    async fn trash(&self, path: &str) -> Result<()> {
        let native = to_native(path);
        let p = path.to_string();
        tokio::task::spawn_blocking(move || trash::delete(&native))
            .await
            .map_err(join_err)?
            .map_err(|e| VfsError::Io(format!("{p}: {e}")))
    }
}

/// Sidebar shortcuts for this machine. Only places that exist are returned.
pub fn places() -> Vec<Place> {
    let mut out = Vec::new();
    let mut push = |dir: Option<PathBuf>, label: &str, kind: PlaceKind| {
        if let Some(d) = dir.filter(|d| d.is_dir()) {
            out.push(Place { label: label.to_string(), path: from_native(&d), kind });
        }
    };
    push(dirs::home_dir(), "Home", PlaceKind::Home);
    push(dirs::desktop_dir(), "Desktop", PlaceKind::Desktop);
    push(dirs::document_dir(), "Documents", PlaceKind::Documents);
    push(dirs::download_dir(), "Downloads", PlaceKind::Downloads);
    push(dirs::picture_dir(), "Pictures", PlaceKind::Pictures);
    push(dirs::audio_dir(), "Music", PlaceKind::Music);
    push(dirs::video_dir(), "Videos", PlaceKind::Videos);

    if cfg!(windows) {
        for root in drive_roots() {
            out.push(Place { label: format!("Local Disk ({})", &root[..2]), path: root, kind: PlaceKind::Drive });
        }
    } else {
        out.push(Place { label: "Computer".into(), path: "/".into(), kind: PlaceKind::Root });
        // Removable media mounted by udisks.
        if let Some(user) = std::env::var_os("USER") {
            for base in [Path::new("/media").join(&user), Path::new("/run/media").join(&user)] {
                let Ok(rd) = std::fs::read_dir(&base) else { continue };
                for m in rd.flatten().filter(|m| m.path().is_dir()) {
                    out.push(Place {
                        label: m.file_name().to_string_lossy().into_owned(),
                        path: from_native(&m.path()),
                        kind: PlaceKind::Drive,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn tmp() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let p = from_native(dir.path());
        (dir, p)
    }

    #[tokio::test]
    async fn write_read_list_rename_remove() {
        let (_guard, root) = tmp();
        let fs = LocalBackend::new();

        let sub = path::join(&root, "sub").unwrap();
        fs.mkdir(&sub).await.unwrap();
        let file = path::join(&sub, "a.txt").unwrap();

        let mut w = fs.open_write(&file, WriteMode::CreateNew).await.unwrap();
        w.write_all(b"hello world").await.unwrap();
        w.shutdown().await.unwrap();
        drop(w);
        assert_eq!(
            fs.open_write(&file, WriteMode::CreateNew).await.err().map(|e| e.code()),
            Some("already_exists")
        );

        let mut r = fs.open_read(&file, 6).await.unwrap();
        let mut s = String::new();
        r.read_to_string(&mut s).await.unwrap();
        assert_eq!(s, "world");

        let listing = fs.list(&sub).await.unwrap();
        assert_eq!(listing.len(), 1);
        assert_eq!(listing[0].name, "a.txt");
        assert_eq!(listing[0].kind, EntryKind::File);
        assert_eq!(listing[0].size, 11);
        assert_eq!(listing[0].path, file);

        let moved = path::join(&root, "b.txt").unwrap();
        fs.rename(&file, &moved).await.unwrap();
        assert_eq!(fs.stat(&moved).await.unwrap().size, 11);
        fs.mkdir(&file).await.unwrap(); // reuse old name as a dir
        assert_eq!(fs.rename(&moved, &file).await.err().map(|e| e.code()), Some("already_exists"));

        fs.remove_all(&sub).await.unwrap();
        assert_eq!(fs.stat(&sub).await.err().map(|e| e.code()), Some("not_found"));
    }

    #[tokio::test]
    async fn default_remove_all_walks_tree() {
        // Exercise the trait's default impl (used by SFTP) via a thin wrapper.
        struct Walker(LocalBackend);
        #[async_trait]
        impl Backend for Walker {
            fn source(&self) -> SourceId { SourceId::Local }
            async fn home(&self) -> Result<String> { self.0.home().await }
            async fn list(&self, p: &str) -> Result<Vec<Entry>> { self.0.list(p).await }
            async fn stat(&self, p: &str) -> Result<Entry> { self.0.stat(p).await }
            async fn mkdir(&self, p: &str) -> Result<()> { self.0.mkdir(p).await }
            async fn remove_file(&self, p: &str) -> Result<()> { self.0.remove_file(p).await }
            async fn remove_dir(&self, p: &str) -> Result<()> { self.0.remove_dir(p).await }
            async fn rename(&self, a: &str, b: &str) -> Result<()> { self.0.rename(a, b).await }
            async fn open_read(&self, p: &str, o: u64) -> Result<BoxRead> { self.0.open_read(p, o).await }
            async fn open_write(&self, p: &str, m: WriteMode) -> Result<BoxWrite> { self.0.open_write(p, m).await }
        }

        let (_guard, root) = tmp();
        let fs = Walker(LocalBackend::new());
        let a = path::join(&root, "a").unwrap();
        let b = path::join(&a, "b").unwrap();
        fs.mkdir(&a).await.unwrap();
        fs.mkdir(&b).await.unwrap();
        for f in [path::join(&a, "1").unwrap(), path::join(&b, "2").unwrap()] {
            fs.open_write(&f, WriteMode::Truncate).await.unwrap();
        }
        fs.remove_all(&a).await.unwrap();
        assert!(fs.list(&root).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn disk_space_of_home() {
        let fs = LocalBackend::new();
        let space = fs.disk_space(&fs.home().await.unwrap()).await.unwrap().unwrap();
        assert!(space.total > 0 && space.free <= space.total);
    }

    #[test]
    fn places_include_home() {
        assert!(places().iter().any(|p| p.kind == PlaceKind::Home));
    }
}
