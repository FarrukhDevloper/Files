//! Open remote files in the user's own apps.
//!
//! `open()` downloads a temp copy (keeping the file name so the OS picks the
//! right app), opens it, and watches its folder. Every save is uploaded back
//! after a short debounce — unless the server copy changed since we last
//! synced, in which case the session goes to `conflict` and waits for
//! `resolve()`.
//!
//! Uploads write the server file in place (truncate + write) rather than
//! via temp-and-rename, so its permissions and owner survive — this is what
//! editing a config file on a server needs.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notify::{EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::{broadcast, mpsc};
use vpsfiles_vfs::{path, BackendResolver, EntryKind, Location, Result, VfsError, WriteMode};

/// Editors often save in bursts (write, then chmod, then rename).
const DEBOUNCE: Duration = Duration::from_millis(700);
/// Temp copies untouched for this long are pruned at startup.
const STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditState {
    Downloading,
    Open,
    Uploading,
    Synced,
    Conflict,
    Failed,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditSession {
    pub id: String,
    pub location: Location,
    pub local_path: PathBuf,
    pub state: EditState,
    pub error: Option<VfsError>,
    /// Unix epoch ms of the last successful upload.
    pub last_sync: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Resolve {
    /// Overwrite the server copy with the local one.
    Upload,
    /// Drop local changes and re-download the server copy.
    Discard,
}

/// Opens a local file with the OS default app.
pub type Opener = Arc<dyn Fn(&Path) -> Result<()> + Send + Sync>;

/// (modified, size) — what we compare to notice changes on either side.
type Stamp = (Option<i64>, u64);

struct Session {
    snap: Mutex<EditSession>,
    dir: PathBuf,
    /// Server stamp as of our last download/upload.
    remote: Mutex<Stamp>,
    /// Local stamp as of our last download/upload; a different one = a save.
    local: Mutex<Option<Stamp>>,
    /// Bumped per watcher event; only the last one in a burst syncs.
    generation: AtomicU64,
    /// Serializes download/upload for this file.
    io: tokio::sync::Mutex<()>,
}

impl Session {
    fn location(&self) -> Location {
        self.snap.lock().unwrap().location.clone()
    }

    fn local_path(&self) -> PathBuf {
        self.snap.lock().unwrap().local_path.clone()
    }
}

struct Inner {
    resolver: Arc<dyn BackendResolver>,
    root: PathBuf,
    opener: Opener,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    events: broadcast::Sender<EditSession>,
    /// Created on first `open()`, inside the tokio runtime.
    watcher: OnceLock<Mutex<notify::RecommendedWatcher>>,
}

#[derive(Clone)]
pub struct EditManager {
    inner: Arc<Inner>,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn io_err(e: std::io::Error, p: &Path) -> VfsError {
    VfsError::from_io(e, &p.to_string_lossy())
}

fn local_stamp(p: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(p).ok()?;
    let modified = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos() as i64;
    Some((Some(modified), m.len()))
}

impl EditManager {
    /// `root` holds the temp copies (e.g. `<cache>/vps-files/edit`).
    pub fn new(resolver: Arc<dyn BackendResolver>, root: PathBuf, opener: Opener) -> Self {
        prune_stale(&root);
        let (events, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(Inner {
                resolver,
                root,
                opener,
                sessions: Mutex::new(HashMap::new()),
                events,
                watcher: OnceLock::new(),
            }),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EditSession> {
        self.inner.events.subscribe()
    }

    pub fn list(&self) -> Vec<EditSession> {
        self.inner.sessions.lock().unwrap().values().map(|s| s.snap.lock().unwrap().clone()).collect()
    }

    /// Download (or reuse) a temp copy of `loc`, open it, start syncing.
    pub async fn open(&self, loc: Location) -> Result<EditSession> {
        let existing = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .values()
            .find(|s| s.location() == loc)
            .cloned();
        if let Some(s) = existing {
            (self.inner.opener)(&s.local_path())?;
            return Ok(s.snap.lock().unwrap().clone());
        }

        let name = path::file_name(&loc.path).to_string();
        if name.is_empty() {
            return Err(VfsError::IsADirectory(loc.path.clone()));
        }
        let mut h = DefaultHasher::new();
        (loc.source.to_string(), &loc.path).hash(&mut h);
        let dir = self.inner.root.join(format!("{:016x}", h.finish()));
        tokio::fs::create_dir_all(&dir).await.map_err(|e| io_err(e, &dir))?;

        let id = uuid::Uuid::new_v4().to_string();
        let session = Arc::new(Session {
            snap: Mutex::new(EditSession {
                id: id.clone(),
                location: loc,
                local_path: dir.join(&name),
                state: EditState::Downloading,
                error: None,
                last_sync: None,
            }),
            dir: dir.clone(),
            remote: Mutex::new((None, 0)),
            local: Mutex::new(None),
            generation: AtomicU64::new(0),
            io: tokio::sync::Mutex::new(()),
        });
        self.inner.sessions.lock().unwrap().insert(id.clone(), session.clone());
        self.emit(&session);

        let opened = async {
            self.download(&session).await?;
            self.watch(&dir)?;
            (self.inner.opener)(&session.local_path())
        }
        .await;
        match opened {
            Ok(()) => {
                self.set(&session, EditState::Open, None);
                Ok(session.snap.lock().unwrap().clone())
            }
            Err(e) => {
                self.inner.sessions.lock().unwrap().remove(&id);
                let _ = tokio::fs::remove_dir_all(&dir).await;
                self.set(&session, EditState::Closed, Some(e.clone()));
                Err(e)
            }
        }
    }

    /// Stop syncing and delete the temp copy.
    pub async fn stop(&self, id: &str) -> Result<()> {
        let session = self.inner.sessions.lock().unwrap().remove(id).ok_or_else(|| not_found(id))?;
        let _io = session.io.lock().await;
        if let Some(w) = self.inner.watcher.get() {
            let _ = w.lock().unwrap().unwatch(&session.dir);
        }
        let _ = tokio::fs::remove_dir_all(&session.dir).await;
        self.set(&session, EditState::Closed, None);
        Ok(())
    }

    pub async fn resolve(&self, id: &str, action: Resolve) -> Result<()> {
        let session = self.session(id)?;
        match action {
            Resolve::Upload => self.upload(&session, true).await,
            Resolve::Discard => {
                self.download(&session).await?;
                self.set(&session, EditState::Open, None);
                Ok(())
            }
        }
    }

    fn session(&self, id: &str) -> Result<Arc<Session>> {
        self.inner.sessions.lock().unwrap().get(id).cloned().ok_or_else(|| not_found(id))
    }

    fn emit(&self, s: &Session) {
        let _ = self.inner.events.send(s.snap.lock().unwrap().clone());
    }

    fn set(&self, s: &Session, state: EditState, error: Option<VfsError>) {
        {
            let mut snap = s.snap.lock().unwrap();
            snap.state = state;
            snap.error = error;
            if state == EditState::Synced {
                snap.last_sync = Some(now_ms());
            }
        }
        self.emit(s);
    }

    async fn download(&self, s: &Session) -> Result<()> {
        let _io = s.io.lock().await;
        let loc = s.location();
        let fs = self.inner.resolver.backend(&loc.source).await?;
        let entry = fs.stat(&loc.path).await?;
        if entry.kind != EntryKind::File {
            return Err(VfsError::IsADirectory(loc.path));
        }
        let local = s.local_path();
        // Write beside the target and rename, so an editor that has the file
        // open never sees it half-written.
        let part = s.dir.join(format!(".{}.download", path::file_name(&loc.path)));
        let mut reader = fs.open_read(&loc.path, 0).await?;
        let mut file = tokio::fs::File::create(&part).await.map_err(|e| io_err(e, &part))?;
        tokio::io::copy(&mut reader, &mut file).await.map_err(|e| io_err(e, &part))?;
        file.flush().await.map_err(|e| io_err(e, &part))?;
        drop(file);
        tokio::fs::rename(&part, &local).await.map_err(|e| io_err(e, &local))?;

        *s.remote.lock().unwrap() = (entry.modified, entry.size);
        *s.local.lock().unwrap() = local_stamp(&local);
        Ok(())
    }

    /// Push the local copy to the server. Without `force`, a server copy
    /// that changed since our last sync turns the session into `conflict`.
    async fn upload(&self, s: &Session, force: bool) -> Result<()> {
        let _io = s.io.lock().await;
        let local = s.local_path();
        let Some(stamp) = local_stamp(&local) else {
            return Ok(()); // mid atomic-save; the follow-up event syncs
        };
        if !force && Some(stamp) == *s.local.lock().unwrap() {
            return Ok(());
        }
        let loc = s.location();
        let result: Result<bool> = async {
            let fs = self.inner.resolver.backend(&loc.source).await?;
            let current = fs.stat(&loc.path).await.ok().map(|e| (e.modified, e.size));
            if !force && current.is_some_and(|c| c != *s.remote.lock().unwrap()) {
                return Ok(false);
            }
            self.set(s, EditState::Uploading, None);
            let mut src = tokio::fs::File::open(&local).await.map_err(|e| io_err(e, &local))?;
            let mut dst = fs.open_write(&loc.path, WriteMode::Truncate).await?;
            tokio::io::copy(&mut src, &mut dst).await.map_err(|e| VfsError::from_io(e, &loc.path))?;
            dst.shutdown().await.map_err(|e| VfsError::from_io(e, &loc.path))?;
            let after = fs.stat(&loc.path).await?;
            *s.remote.lock().unwrap() = (after.modified, after.size);
            *s.local.lock().unwrap() = Some(stamp);
            Ok(true)
        }
        .await;
        match result {
            Ok(true) => self.set(s, EditState::Synced, None),
            Ok(false) => self.set(s, EditState::Conflict, None),
            Err(e) => self.set(s, EditState::Failed, Some(e.clone())),
        }
        Ok(())
    }

    fn watch(&self, dir: &Path) -> Result<()> {
        let watcher = match self.inner.watcher.get() {
            Some(w) => w,
            None => {
                let (tx, rx) = mpsc::unbounded_channel::<PathBuf>();
                let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                    let Ok(ev) = res else { return };
                    if matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                        for p in ev.paths {
                            let _ = tx.send(p);
                        }
                    }
                })
                .map_err(|e| VfsError::Other(format!("file watcher: {e}")))?;
                tokio::spawn(self.clone().pump(rx));
                // A concurrent first open may have won the race; either works.
                let _ = self.inner.watcher.set(Mutex::new(w));
                self.inner.watcher.get().expect("just set")
            }
        };
        watcher
            .lock()
            .unwrap()
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| VfsError::Other(format!("watch {}: {e}", dir.display())))
    }

    /// Debounce watcher events per session, then upload.
    async fn pump(self, mut rx: mpsc::UnboundedReceiver<PathBuf>) {
        while let Some(changed) = rx.recv().await {
            let session = self
                .inner
                .sessions
                .lock()
                .unwrap()
                .values()
                .find(|s| s.local_path() == changed)
                .cloned();
            let Some(session) = session else { continue };
            let generation = session.generation.fetch_add(1, Ordering::SeqCst) + 1;
            let this = self.clone();
            tokio::spawn(async move {
                tokio::time::sleep(DEBOUNCE).await;
                if session.generation.load(Ordering::SeqCst) == generation {
                    let _ = this.upload(&session, false).await;
                }
            });
        }
    }
}

fn not_found(id: &str) -> VfsError {
    VfsError::NotFound(format!("edit session {id}"))
}

fn prune_stale(root: &Path) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for dir in rd.flatten() {
        let stale = dir
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE_AFTER);
        if stale {
            let _ = std::fs::remove_dir_all(dir.path());
        }
    }
}
