//! End-to-end transfer tests over the local disk. A second "remote" source
//! is the same disk behind a different `SourceId`, which forces the
//! cross-source (stream copy) path instead of `rename`.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use vpsfiles_transfer::{ConflictPolicy, JobSnapshot, JobState, TransferManager, TransferOp, TransferRequest};
use vpsfiles_vfs::local::{from_native, LocalBackend};
use vpsfiles_vfs::{
    path, Backend, BackendResolver, BoxRead, BoxWrite, Entry, Location, Result, SourceId, WriteMode,
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
    async fn backend(&self, source: &SourceId) -> Result<Arc<dyn Backend>> {
        Ok(match source {
            SourceId::Local => Arc::new(LocalBackend::new()),
            SourceId::Ssh { .. } => Arc::new(FakeRemote(LocalBackend::new())),
        })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: String,
    mgr: TransferManager,
}

impl Fixture {
    fn new(parallel: usize) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = from_native(dir.path());
        Self { _dir: dir, root, mgr: TransferManager::new(Arc::new(Resolver), parallel) }
    }

    fn p(&self, rel: &str) -> String {
        rel.split('/').fold(self.root.clone(), |acc, part| path::join(&acc, part).unwrap())
    }

    fn write(&self, rel: &str, contents: &[u8]) {
        let p = self.p(rel);
        std::fs::create_dir_all(std::path::Path::new(&p).parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }

    fn read(&self, rel: &str) -> Vec<u8> {
        std::fs::read(self.p(rel)).unwrap()
    }

    fn exists(&self, rel: &str) -> bool {
        std::path::Path::new(&self.p(rel)).exists()
    }

    fn req(&self, op: TransferOp, src: &[(&SourceId, &str)], dest: (&SourceId, &str), conflict: ConflictPolicy) -> TransferRequest {
        TransferRequest {
            op,
            sources: src.iter().map(|(s, p)| Location::new((*s).clone(), self.p(p))).collect(),
            dest_dir: Location::new(dest.0.clone(), self.p(dest.1)),
            conflict,
        }
    }

    async fn wait(&self, id: &str) -> JobSnapshot {
        for _ in 0..500 {
            let snap = self.mgr.list().into_iter().find(|j| j.id == id).unwrap();
            if snap.state.is_finished() {
                return snap;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("job {id} did not finish");
    }
}

fn local() -> SourceId {
    SourceId::Local
}
fn remote() -> SourceId {
    SourceId::ssh("fake")
}

#[tokio::test]
async fn copies_tree_across_sources_with_totals() {
    let fx = Fixture::new(2);
    let big: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
    fx.write("src/proj/a.txt", b"alpha");
    fx.write("src/proj/sub/big.bin", &big);
    std::fs::create_dir_all(fx.p("src/proj/empty")).unwrap();
    std::fs::create_dir_all(fx.p("dst")).unwrap();

    let id = fx
        .mgr
        .start(fx.req(TransferOp::Copy, &[(&local(), "src/proj")], (&remote(), "dst"), ConflictPolicy::Skip))
        .unwrap();
    let snap = fx.wait(&id).await;

    assert_eq!(snap.state, JobState::Completed, "{:?}", snap.error);
    assert_eq!(snap.total_files, 2);
    assert_eq!(snap.done_files, 2);
    assert_eq!(snap.total_bytes, 5 + big.len() as u64);
    assert_eq!(snap.done_bytes, snap.total_bytes);
    assert_eq!(fx.read("dst/proj/a.txt"), b"alpha");
    assert_eq!(fx.read("dst/proj/sub/big.bin"), big);
    assert!(fx.exists("dst/proj/empty"));
    assert!(fx.exists("src/proj/a.txt"), "copy must keep the source");
}

#[tokio::test]
async fn conflict_policies() {
    let fx = Fixture::new(1);
    fx.write("src/report.pdf", b"new");
    fx.write("dst/report.pdf", b"old");
    fx.write("dst/report 2.pdf", b"older");

    let id = fx.mgr.start(fx.req(TransferOp::Copy, &[(&local(), "src/report.pdf")], (&remote(), "dst"), ConflictPolicy::Skip)).unwrap();
    assert_eq!(fx.wait(&id).await.state, JobState::Completed);
    assert_eq!(fx.read("dst/report.pdf"), b"old");

    let id = fx.mgr.start(fx.req(TransferOp::Copy, &[(&local(), "src/report.pdf")], (&remote(), "dst"), ConflictPolicy::KeepBoth)).unwrap();
    assert_eq!(fx.wait(&id).await.state, JobState::Completed);
    assert_eq!(fx.read("dst/report 3.pdf"), b"new");

    let id = fx.mgr.start(fx.req(TransferOp::Copy, &[(&local(), "src/report.pdf")], (&remote(), "dst"), ConflictPolicy::Overwrite)).unwrap();
    assert_eq!(fx.wait(&id).await.state, JobState::Completed);
    assert_eq!(fx.read("dst/report.pdf"), b"new");
}

#[tokio::test]
async fn same_source_move_is_a_rename_and_cross_source_move_deletes() {
    let fx = Fixture::new(2);
    fx.write("a/one/x.txt", b"x");
    fx.write("b/two.txt", b"two");
    std::fs::create_dir_all(fx.p("dst")).unwrap();

    let id = fx.mgr.start(fx.req(TransferOp::Move, &[(&local(), "a/one")], (&local(), "dst"), ConflictPolicy::Skip)).unwrap();
    let snap = fx.wait(&id).await;
    assert_eq!(snap.state, JobState::Completed);
    assert_eq!(snap.total_bytes, 0, "rename should not scan bytes");
    assert_eq!(fx.read("dst/one/x.txt"), b"x");
    assert!(!fx.exists("a/one"));

    let id = fx.mgr.start(fx.req(TransferOp::Move, &[(&local(), "b/two.txt")], (&remote(), "dst"), ConflictPolicy::Skip)).unwrap();
    assert_eq!(fx.wait(&id).await.state, JobState::Completed);
    assert_eq!(fx.read("dst/two.txt"), b"two");
    assert!(!fx.exists("b/two.txt"));
}

#[tokio::test]
async fn refuses_copying_a_folder_into_itself() {
    let fx = Fixture::new(1);
    fx.write("a/inner/f", b"f");
    let id = fx.mgr.start(fx.req(TransferOp::Copy, &[(&local(), "a")], (&local(), "a/inner"), ConflictPolicy::KeepBoth)).unwrap();
    let snap = fx.wait(&id).await;
    assert_eq!(snap.state, JobState::Failed);
    assert_eq!(snap.error.map(|e| e.code()), Some("invalid_name"));
}

#[tokio::test]
async fn pause_resume_and_cancel_queued() {
    let fx = Fixture::new(1);
    fx.write("src/f.txt", b"payload");
    std::fs::create_dir_all(fx.p("dst")).unwrap();

    let a = fx.mgr.start(fx.req(TransferOp::Copy, &[(&local(), "src/f.txt")], (&remote(), "dst"), ConflictPolicy::KeepBoth)).unwrap();
    fx.mgr.pause(&a).unwrap();
    // Only one slot: b waits behind the paused a.
    let b = fx.mgr.start(fx.req(TransferOp::Copy, &[(&local(), "src/f.txt")], (&remote(), "dst"), ConflictPolicy::KeepBoth)).unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;
    let state = |id: &str| fx.mgr.list().into_iter().find(|j| j.id == id).unwrap().state;
    assert_eq!(state(&a), JobState::Paused);
    assert_eq!(state(&b), JobState::Queued);
    assert!(!fx.exists("dst/f.txt"));

    fx.mgr.cancel(&b).unwrap();
    assert_eq!(fx.wait(&b).await.state, JobState::Cancelled);

    fx.mgr.resume(&a).unwrap();
    assert_eq!(fx.wait(&a).await.state, JobState::Completed);
    assert_eq!(fx.read("dst/f.txt"), b"payload");

    fx.mgr.clear_finished();
    assert!(fx.mgr.list().is_empty());
}

fn names(fx: &Fixture, rel: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(fx.p(rel))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn replace_swaps_whole_folder_and_merge_keeps_extras() {
    let fx = Fixture::new(1);
    fx.write("src/site/index.html", b"new");
    fx.write("dst/site/index.html", b"old");
    fx.write("dst/site/extra.css", b"keep?");

    let id = fx.mgr.start(fx.req(TransferOp::Copy, &[(&local(), "src/site")], (&remote(), "dst"), ConflictPolicy::Merge)).unwrap();
    assert_eq!(fx.wait(&id).await.state, JobState::Completed);
    assert_eq!(fx.read("dst/site/index.html"), b"new");
    assert_eq!(names(&fx, "dst/site"), ["extra.css", "index.html"]);

    let id = fx.mgr.start(fx.req(TransferOp::Copy, &[(&local(), "src/site")], (&remote(), "dst"), ConflictPolicy::Overwrite)).unwrap();
    assert_eq!(fx.wait(&id).await.state, JobState::Completed);
    assert_eq!(names(&fx, "dst/site"), ["index.html"], "Replace drops what the new folder lacks");
    assert_eq!(names(&fx, "dst"), ["site"], "no staging leftovers");
}

#[tokio::test]
async fn same_source_move_replaces_folder() {
    let fx = Fixture::new(1);
    fx.write("a/site/new.txt", b"n");
    fx.write("dst/site/old.txt", b"o");
    let id = fx.mgr.start(fx.req(TransferOp::Move, &[(&local(), "a/site")], (&local(), "dst"), ConflictPolicy::Overwrite)).unwrap();
    assert_eq!(fx.wait(&id).await.state, JobState::Completed);
    assert_eq!(names(&fx, "dst/site"), ["new.txt"]);
    assert_eq!(names(&fx, "dst"), ["site"]);
    assert!(!fx.exists("a/site"));
}

#[cfg(unix)]
#[tokio::test]
async fn failed_replace_leaves_original_untouched() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fixture::new(1);
    fx.write("src/site/a.txt", b"a");
    fx.write("src/site/locked.txt", b"secret");
    std::fs::set_permissions(fx.p("src/site/locked.txt"), std::fs::Permissions::from_mode(0o000)).unwrap();
    fx.write("dst/site/old.txt", b"old");

    let id = fx.mgr.start(fx.req(TransferOp::Copy, &[(&local(), "src/site")], (&remote(), "dst"), ConflictPolicy::Overwrite)).unwrap();
    let snap = fx.wait(&id).await;
    std::fs::set_permissions(fx.p("src/site/locked.txt"), std::fs::Permissions::from_mode(0o644)).unwrap();

    assert_eq!(snap.state, JobState::Failed);
    assert_eq!(fx.read("dst/site/old.txt"), b"old");
    assert_eq!(names(&fx, "dst"), ["site"], "staging dir must be cleaned up");
}
