//! Edit sessions against a fake "remote" (the local disk behind an SSH
//! SourceId), with a real filesystem watcher.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use vpsfiles_edit::{EditManager, EditSession, EditState, Resolve};
use vpsfiles_vfs::local::{from_native, LocalBackend};
use vpsfiles_vfs::{
    Backend, BackendResolver, BoxRead, BoxWrite, Entry, Location, Result, SourceId, WriteMode,
};

struct FakeRemote(LocalBackend);

#[async_trait]
impl Backend for FakeRemote {
    fn source(&self) -> SourceId { SourceId::ssh("fake") }
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

struct Resolver;

#[async_trait]
impl BackendResolver for Resolver {
    async fn backend(&self, _: &SourceId) -> Result<Arc<dyn Backend>> {
        Ok(Arc::new(FakeRemote(LocalBackend::new())))
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    remote_file: PathBuf,
    loc: Location,
    cache: PathBuf,
    opened: Arc<Mutex<Vec<PathBuf>>>,
    mgr: EditManager,
}

fn fixture(contents: &str) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let remote_dir = tmp.path().join("server");
    std::fs::create_dir_all(&remote_dir).unwrap();
    let remote_file = remote_dir.join("nginx.conf");
    std::fs::write(&remote_file, contents).unwrap();
    let cache = tmp.path().join("cache");
    let opened = Arc::new(Mutex::new(Vec::new()));
    let log = opened.clone();
    let mgr = EditManager::new(
        Arc::new(Resolver),
        cache.clone(),
        Arc::new(move |p: &Path| {
            log.lock().unwrap().push(p.to_path_buf());
            Ok(())
        }),
    );
    let loc = Location::new(SourceId::ssh("fake"), from_native(&remote_file));
    Fixture { _tmp: tmp, remote_file, loc, cache, opened, mgr }
}

async fn wait_for(mgr: &EditManager, id: &str, state: EditState) -> EditSession {
    for _ in 0..100 {
        if let Some(s) = mgr.list().into_iter().find(|s| s.id == id && s.state == state) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("session never reached {state:?}: {:?}", mgr.list());
}

/// Make sure the next write gets a different mtime than the previous one.
async fn tick() {
    tokio::time::sleep(Duration::from_millis(20)).await;
}

#[tokio::test]
async fn open_edit_save_syncs_back() {
    let fx = fixture("worker_processes 1;\n");
    let s = fx.mgr.open(fx.loc.clone()).await.unwrap();

    assert_eq!(s.state, EditState::Open);
    assert_eq!(s.local_path.file_name().unwrap(), "nginx.conf", "keeps the name so the OS picks the app");
    assert!(s.local_path.starts_with(&fx.cache));
    assert_eq!(std::fs::read_to_string(&s.local_path).unwrap(), "worker_processes 1;\n");
    assert_eq!(fx.opened.lock().unwrap().as_slice(), std::slice::from_ref(&s.local_path));

    tick().await;
    std::fs::write(&s.local_path, "worker_processes 4;\n").unwrap();
    let synced = wait_for(&fx.mgr, &s.id, EditState::Synced).await;
    assert!(synced.last_sync.is_some());
    assert_eq!(std::fs::read_to_string(&fx.remote_file).unwrap(), "worker_processes 4;\n");

    // Opening again reuses the session and just re-opens the app.
    let again = fx.mgr.open(fx.loc.clone()).await.unwrap();
    assert_eq!(again.id, s.id);
    assert_eq!(fx.opened.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn server_change_becomes_conflict_and_resolves() {
    let fx = fixture("v1\n");
    let s = fx.mgr.open(fx.loc.clone()).await.unwrap();

    tick().await;
    std::fs::write(&fx.remote_file, "server edit, longer\n").unwrap();
    std::fs::write(&s.local_path, "my edit\n").unwrap();
    wait_for(&fx.mgr, &s.id, EditState::Conflict).await;
    assert_eq!(std::fs::read_to_string(&fx.remote_file).unwrap(), "server edit, longer\n", "never clobbered");

    fx.mgr.resolve(&s.id, Resolve::Discard).await.unwrap();
    assert_eq!(std::fs::read_to_string(&s.local_path).unwrap(), "server edit, longer\n");
    wait_for(&fx.mgr, &s.id, EditState::Open).await;

    tick().await;
    std::fs::write(&s.local_path, "mine wins\n").unwrap();
    wait_for(&fx.mgr, &s.id, EditState::Synced).await;
    tick().await;
    std::fs::write(&fx.remote_file, "theirs again!\n").unwrap();
    fx.mgr.resolve(&s.id, Resolve::Upload).await.unwrap();
    assert_eq!(std::fs::read_to_string(&fx.remote_file).unwrap(), "mine wins\n");
}

#[tokio::test]
async fn stop_removes_temp_copy() {
    let fx = fixture("x");
    let s = fx.mgr.open(fx.loc.clone()).await.unwrap();
    let dir = s.local_path.parent().unwrap().to_path_buf();
    assert!(dir.exists());
    fx.mgr.stop(&s.id).await.unwrap();
    assert!(!dir.exists());
    assert!(fx.mgr.list().is_empty());
    assert_eq!(fx.mgr.stop(&s.id).await.err().map(|e| e.code()), Some("not_found"));
}

#[tokio::test]
async fn opening_a_folder_fails_cleanly() {
    let fx = fixture("x");
    let dir = Location::new(SourceId::ssh("fake"), from_native(fx.remote_file.parent().unwrap()));
    assert_eq!(fx.mgr.open(dir).await.err().map(|e| e.code()), Some("is_a_directory"));
    assert!(fx.mgr.list().is_empty());
    assert!(fx.opened.lock().unwrap().is_empty());
}
