//! One job's worker: wait for a slot, plan, copy, finish.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch, Semaphore};
use vpsfiles_vfs::{path, Backend, BackendResolver, EntryKind, Result, VfsError, WriteMode};

use crate::manager::JobShared;
use crate::{ConflictPolicy, JobState, TransferOp, TransferRequest};

const CHUNK: usize = 256 * 1024;
/// Chunks buffered between the reader and writer: lets a remote download
/// and a remote upload overlap instead of taking turns (matters for relay).
const IN_FLIGHT: usize = 4;
const EMIT_EVERY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ctl {
    Run,
    Pause,
    Cancel,
}

/// Blocks while paused; errors once cancelled.
async fn checkpoint(rx: &mut watch::Receiver<Ctl>) -> Result<()> {
    loop {
        let ctl = *rx.borrow_and_update();
        match ctl {
            Ctl::Run => return Ok(()),
            Ctl::Cancel => return Err(VfsError::Cancelled),
            Ctl::Pause => {}
        }
        if rx.changed().await.is_err() {
            return Err(VfsError::Cancelled);
        }
    }
}

async fn exists(fs: &dyn Backend, p: &str) -> Result<bool> {
    match fs.stat(p).await {
        Ok(_) => Ok(true),
        Err(VfsError::NotFound(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// One file or directory to create at the destination.
struct Item {
    src: String,
    dst: String,
    is_dir: bool,
    size: u64,
}

/// What happens to a destination name that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Existing {
    /// The name is free.
    No,
    /// Finder "Replace": the old item is swapped out only after the new one
    /// is complete, so a failed or cancelled copy never destroys it.
    Replace,
    /// Copy into the existing folder, overwriting clashing files.
    Merge,
}

/// A top-level source after conflict resolution.
struct Plan {
    src_fs: Arc<dyn Backend>,
    src: String,
    dst: String,
    /// Same-source move that can be a single `rename`.
    rename: bool,
    existing: Existing,
    /// Hidden sibling the copy is written to before being swapped in
    /// (`Replace` only). Items' `dst` paths live under it.
    stage: Option<String>,
    items: Vec<Item>,
}

/// Hidden sibling name used while replacing `dst`.
fn staging_path(dst: &str) -> Result<String> {
    let parent = path::parent(dst).unwrap_or_else(|| "/".into());
    let tag = &uuid::Uuid::new_v4().simple().to_string()[..8];
    path::join(&parent, &format!(".{}.vpsfiles-{tag}.part", path::file_name(dst)))
}

pub(crate) async fn run(
    job: &JobShared,
    req: TransferRequest,
    resolver: Arc<dyn BackendResolver>,
    slots: Arc<Semaphore>,
    mut ctl: watch::Receiver<Ctl>,
) -> Result<()> {
    let _permit = loop {
        tokio::select! {
            p = slots.clone().acquire_owned() => break p.map_err(|_| VfsError::Cancelled)?,
            changed = ctl.changed() => {
                if changed.is_err() || *ctl.borrow() == Ctl::Cancel {
                    return Err(VfsError::Cancelled);
                }
            }
        }
    };
    checkpoint(&mut ctl).await?;

    job.enter(JobState::Scanning);
    let dst_fs = resolver.backend(&req.dest_dir.source).await?;
    let dest = dst_fs.stat(&req.dest_dir.path).await?;
    if dest.kind != EntryKind::Dir {
        return Err(VfsError::NotADirectory(req.dest_dir.path.clone()));
    }

    let mut plans = Vec::new();
    for loc in &req.sources {
        checkpoint(&mut ctl).await?;
        let src_fs = resolver.backend(&loc.source).await?;
        let same_source = loc.source == req.dest_dir.source;
        if let Some(plan) =
            plan_one(src_fs, &loc.path, dst_fs.as_ref(), &req, same_source, &mut ctl).await?
        {
            plans.push(plan);
        }
    }
    let (files, bytes) = plans.iter().fold((0, 0), |(f, b), p| {
        let (pf, pb) = if p.rename { (1, 0) } else { totals(&p.items) };
        (f + pf, b + pb)
    });
    job.update(|s| {
        s.total_files = files;
        s.total_bytes = bytes;
    });

    job.enter(JobState::Running);
    let mut progress = Progress::new();
    for mut plan in plans {
        checkpoint(&mut ctl).await?;
        if plan.rename {
            job.update(|s| s.current = Some(plan.src.clone()));
            if move_by_rename(dst_fs.as_ref(), &plan).await? {
                job.update(|s| s.done_files += 1);
                continue;
            }
            // e.g. EXDEV between two local mounts: copy + delete instead.
            if plan.existing == Existing::Replace {
                plan.stage = Some(staging_path(&plan.dst)?);
            }
            let entry = plan.src_fs.stat(&plan.src).await?;
            let root = plan.stage.clone().unwrap_or_else(|| plan.dst.clone());
            plan.items = walk(plan.src_fs.as_ref(), entry.kind, &plan.src, &root, &mut ctl).await?;
            let (files, bytes) = totals(&plan.items);
            job.update(|s| {
                s.total_files += files.saturating_sub(1);
                s.total_bytes += bytes;
            });
        }

        let copied = copy_plan(&plan, dst_fs.as_ref(), job, &mut progress, &mut ctl).await;
        if let Err(e) = copied {
            if let Some(stage) = &plan.stage {
                let _ = dst_fs.remove_all(stage).await;
            }
            return Err(e);
        }
        if req.op == TransferOp::Move {
            plan.src_fs.remove_all(&plan.src).await?;
        }
    }
    Ok(())
}

/// Same-source move via `rename`. `Ok(false)` means the backend can't rename
/// across this boundary and the caller should copy instead.
async fn move_by_rename(fs: &dyn Backend, plan: &Plan) -> Result<bool> {
    let backup = match plan.existing {
        Existing::Replace => {
            let b = staging_path(&plan.dst)?;
            fs.rename(&plan.dst, &b).await?;
            Some(b)
        }
        _ => None,
    };
    match fs.rename(&plan.src, &plan.dst).await {
        Ok(()) => {
            if let Some(b) = backup {
                fs.remove_all(&b).await?;
            }
            Ok(true)
        }
        Err(e) => {
            if let Some(b) = backup {
                let _ = fs.rename(&b, &plan.dst).await;
            }
            match e {
                VfsError::Io(_) | VfsError::Other(_) | VfsError::Unsupported(_) => Ok(false),
                e => Err(e),
            }
        }
    }
}

/// Write every item, then swap a staged replacement into place.
async fn copy_plan(
    plan: &Plan,
    dst_fs: &dyn Backend,
    job: &JobShared,
    progress: &mut Progress,
    ctl: &mut watch::Receiver<Ctl>,
) -> Result<()> {
    let mode = if plan.existing == Existing::Merge { WriteMode::Truncate } else { WriteMode::CreateNew };
    for item in &plan.items {
        checkpoint(ctl).await?;
        if item.is_dir {
            match dst_fs.mkdir(&item.dst).await {
                Ok(()) | Err(VfsError::AlreadyExists(_)) => {}
                Err(e) => return Err(e),
            }
            continue;
        }
        job.update(|s| s.current = Some(item.src.clone()));
        let copied =
            copy_file(plan.src_fs.as_ref(), &item.src, dst_fs, &item.dst, mode, job, progress, ctl).await;
        if let Err(e) = copied {
            if e == VfsError::Cancelled && plan.stage.is_none() {
                // Don't leave a half-written file behind.
                let _ = dst_fs.remove_file(&item.dst).await;
            }
            return Err(e);
        }
        job.update(|s| s.done_files += 1);
    }
    if let Some(stage) = &plan.stage {
        dst_fs.remove_all(&plan.dst).await?;
        dst_fs.rename(stage, &plan.dst).await?;
    }
    Ok(())
}

fn totals(items: &[Item]) -> (u64, u64) {
    let files = items.iter().filter(|i| !i.is_dir).count() as u64;
    (files, items.iter().map(|i| i.size).sum())
}

async fn stat_opt(fs: &dyn Backend, p: &str) -> Result<Option<vpsfiles_vfs::Entry>> {
    match fs.stat(p).await {
        Ok(e) => Ok(Some(e)),
        Err(VfsError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Resolve the destination name for one source and, unless it is a plain
/// rename, walk it into a flat list of items.
async fn plan_one(
    src_fs: Arc<dyn Backend>,
    src: &str,
    dst_fs: &dyn Backend,
    req: &TransferRequest,
    same_source: bool,
    ctl: &mut watch::Receiver<Ctl>,
) -> Result<Option<Plan>> {
    let entry = src_fs.stat(src).await?;
    let name = path::file_name(src).to_string();
    let mut dst = path::join(&req.dest_dir.path, &name)?;

    if same_source
        && entry.kind == EntryKind::Dir
        && (req.dest_dir.path == src || req.dest_dir.path.starts_with(&format!("{src}/")))
    {
        return Err(VfsError::InvalidName(format!("cannot copy {src} into itself")));
    }

    let mut existing = Existing::No;
    if let Some(found) = stat_opt(dst_fs, &dst).await? {
        match req.conflict {
            ConflictPolicy::Skip => return Ok(None),
            // Replacing or merging an item with itself is a no-op.
            ConflictPolicy::Overwrite | ConflictPolicy::Merge if same_source && dst == src => return Ok(None),
            ConflictPolicy::Overwrite => existing = Existing::Replace,
            ConflictPolicy::Merge => {
                let both_dirs = entry.kind == EntryKind::Dir && found.kind == EntryKind::Dir && !found.is_symlink;
                existing = if both_dirs { Existing::Merge } else { Existing::Replace };
            }
            ConflictPolicy::KeepBoth => {
                let mut n = 2;
                loop {
                    dst = path::join(&req.dest_dir.path, &path::numbered_name(&name, n))?;
                    if !exists(dst_fs, &dst).await? {
                        break;
                    }
                    n += 1;
                }
            }
        }
    }

    // Same-source move: one rename, unless a folder has to be merged.
    if req.op == TransferOp::Move && same_source && existing != Existing::Merge {
        return Ok(Some(Plan { src_fs, src: src.into(), dst, rename: true, existing, stage: None, items: Vec::new() }));
    }

    let stage = if existing == Existing::Replace { Some(staging_path(&dst)?) } else { None };
    let root = stage.clone().unwrap_or_else(|| dst.clone());
    let items = walk(src_fs.as_ref(), entry.kind, src, &root, ctl).await?;
    Ok(Some(Plan { src_fs, src: src.into(), dst, rename: false, existing, stage, items }))
}

/// Flatten a source into items, parents before children.
async fn walk(
    fs: &dyn Backend,
    kind: EntryKind,
    src: &str,
    dst: &str,
    ctl: &mut watch::Receiver<Ctl>,
) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    match kind {
        EntryKind::File => {
            let size = fs.stat(src).await?.size;
            items.push(Item { src: src.into(), dst: dst.into(), is_dir: false, size });
        }
        EntryKind::Dir => {
            items.push(Item { src: src.into(), dst: dst.into(), is_dir: true, size: 0 });
            let mut stack = vec![(src.to_string(), dst.to_string())];
            while let Some((sdir, ddir)) = stack.pop() {
                checkpoint(ctl).await?;
                for child in fs.list(&sdir).await? {
                    let cdst = path::join(&ddir, &child.name)?;
                    match child.kind {
                        // Linked dirs are skipped: following them risks cycles.
                        EntryKind::Dir if child.is_symlink => {}
                        EntryKind::Dir => {
                            items.push(Item { src: child.path.clone(), dst: cdst.clone(), is_dir: true, size: 0 });
                            stack.push((child.path, cdst));
                        }
                        EntryKind::File => {
                            items.push(Item { src: child.path, dst: cdst, is_dir: false, size: child.size })
                        }
                        EntryKind::Other => {}
                    }
                }
            }
        }
        EntryKind::Other => return Err(VfsError::Unsupported(format!("{src} is not a regular file"))),
    }
    Ok(items)
}

struct Progress {
    last_emit: Instant,
    pending: u64,
    speed: f64,
}

impl Progress {
    fn new() -> Self {
        Self { last_emit: Instant::now(), pending: 0, speed: 0.0 }
    }

    fn add(&mut self, job: &JobShared, n: u64) {
        self.pending += n;
        let dt = self.last_emit.elapsed();
        if dt < EMIT_EVERY {
            return;
        }
        let inst = self.pending as f64 / dt.as_secs_f64();
        // Light smoothing so the "time left" label doesn't jitter.
        self.speed = if self.speed == 0.0 { inst } else { self.speed * 0.6 + inst * 0.4 };
        let (done, speed) = (self.pending, self.speed as u64);
        job.update(|s| {
            s.done_bytes += done;
            s.bytes_per_sec = speed;
        });
        self.pending = 0;
        self.last_emit = Instant::now();
    }

    fn flush(&mut self, job: &JobShared) {
        if self.pending > 0 {
            let done = self.pending;
            job.update(|s| s.done_bytes += done);
            self.pending = 0;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn copy_file(
    src_fs: &dyn Backend,
    src: &str,
    dst_fs: &dyn Backend,
    dst: &str,
    mode: WriteMode,
    job: &JobShared,
    progress: &mut Progress,
    ctl: &mut watch::Receiver<Ctl>,
) -> Result<()> {
    let mut reader = src_fs.open_read(src, 0).await?;
    let mut writer = dst_fs.open_write(dst, mode).await?;

    // Reader runs ahead by up to IN_FLIGHT chunks.
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(IN_FLIGHT);
    let src_path = src.to_string();
    let read_task = tokio::spawn(async move {
        loop {
            let mut buf = vec![0u8; CHUNK];
            let n = reader.read(&mut buf).await.map_err(|e| VfsError::from_io(e, &src_path))?;
            if n == 0 {
                return Ok::<(), VfsError>(());
            }
            buf.truncate(n);
            if tx.send(buf).await.is_err() {
                return Ok(()); // writer stopped; its error wins
            }
        }
    });

    let written: Result<()> = async {
        while let Some(chunk) = rx.recv().await {
            checkpoint(ctl).await?;
            writer.write_all(&chunk).await.map_err(|e| VfsError::from_io(e, dst))?;
            progress.add(job, chunk.len() as u64);
        }
        writer.shutdown().await.map_err(|e| VfsError::from_io(e, dst))
    }
    .await;
    drop(rx);
    progress.flush(job);

    let read = read_task.await.map_err(|e| VfsError::Other(format!("reader task failed: {e}")))?;
    written.and(read)
}
