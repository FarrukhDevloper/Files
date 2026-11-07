use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, watch, Semaphore};
use vpsfiles_vfs::{BackendResolver, Result, VfsError};

use crate::job::{self, Ctl};
use crate::{JobId, JobSnapshot, JobState, TransferRequest};

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// State shared between the manager (UI commands) and one job's worker task.
pub(crate) struct JobShared {
    pub snap: Mutex<JobSnapshot>,
    pub ctl: watch::Sender<Ctl>,
    /// The state the worker is actually in, restored on resume.
    pub active: Mutex<JobState>,
    events: broadcast::Sender<JobSnapshot>,
}

impl JobShared {
    /// Mutate the snapshot and broadcast the result.
    pub fn update(&self, f: impl FnOnce(&mut JobSnapshot)) {
        let snap = {
            let mut s = self.snap.lock().unwrap();
            f(&mut s);
            s.clone()
        };
        let _ = self.events.send(snap);
    }

    /// Worker entered a new phase. Doesn't override a user's pause.
    pub fn enter(&self, state: JobState) {
        *self.active.lock().unwrap() = state;
        if *self.ctl.borrow() != Ctl::Pause {
            self.update(|s| s.state = state);
        }
    }

    pub fn finish(&self, result: Result<()>) {
        let state = match &result {
            Ok(()) => JobState::Completed,
            Err(VfsError::Cancelled) => JobState::Cancelled,
            Err(_) => JobState::Failed,
        };
        *self.active.lock().unwrap() = state;
        self.update(|s| {
            s.state = state;
            s.error = result.err().filter(|e| *e != VfsError::Cancelled);
            s.current = None;
            s.bytes_per_sec = 0;
            s.finished_at = Some(now_ms());
        });
    }

    fn state(&self) -> JobState {
        self.snap.lock().unwrap().state
    }
}

pub struct TransferManager {
    resolver: Arc<dyn BackendResolver>,
    events: broadcast::Sender<JobSnapshot>,
    slots: Arc<Semaphore>,
    /// Insertion order = display order.
    jobs: Mutex<Vec<Arc<JobShared>>>,
}

impl TransferManager {
    /// `max_parallel` jobs run at once; the rest wait as `Queued`.
    pub fn new(resolver: Arc<dyn BackendResolver>, max_parallel: usize) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            resolver,
            events,
            slots: Arc::new(Semaphore::new(max_parallel.max(1))),
            jobs: Mutex::new(Vec::new()),
        }
    }

    /// Queue a job and return immediately. Must be called inside a tokio
    /// runtime (the worker is spawned onto it).
    pub fn start(&self, req: TransferRequest) -> Result<JobId> {
        if req.sources.is_empty() {
            return Err(VfsError::Other("nothing to transfer".into()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let snap = JobSnapshot {
            id: id.clone(),
            op: req.op,
            state: JobState::Queued,
            sources: req.sources.clone(),
            dest_dir: req.dest_dir.clone(),
            total_bytes: 0,
            done_bytes: 0,
            total_files: 0,
            done_files: 0,
            current: None,
            bytes_per_sec: 0,
            error: None,
            created_at: now_ms(),
            finished_at: None,
        };
        let (ctl, ctl_rx) = watch::channel(Ctl::Run);
        let shared = Arc::new(JobShared {
            snap: Mutex::new(snap.clone()),
            ctl,
            active: Mutex::new(JobState::Queued),
            events: self.events.clone(),
        });
        self.jobs.lock().unwrap().push(shared.clone());
        let _ = self.events.send(snap);

        let resolver = self.resolver.clone();
        let slots = self.slots.clone();
        tokio::spawn(async move {
            let result = job::run(&shared, req, resolver, slots, ctl_rx).await;
            shared.finish(result);
        });
        Ok(id)
    }

    pub fn list(&self) -> Vec<JobSnapshot> {
        self.jobs.lock().unwrap().iter().map(|j| j.snap.lock().unwrap().clone()).collect()
    }

    fn find(&self, id: &str) -> Result<Arc<JobShared>> {
        self.jobs
            .lock()
            .unwrap()
            .iter()
            .find(|j| j.snap.lock().unwrap().id == id)
            .cloned()
            .ok_or_else(|| VfsError::NotFound(format!("transfer {id}")))
    }

    pub fn pause(&self, id: &str) -> Result<()> {
        let job = self.find(id)?;
        if !job.state().is_finished() {
            job.ctl.send_replace(Ctl::Pause);
            job.update(|s| {
                s.state = JobState::Paused;
                s.bytes_per_sec = 0;
            });
        }
        Ok(())
    }

    pub fn resume(&self, id: &str) -> Result<()> {
        let job = self.find(id)?;
        if job.state() == JobState::Paused {
            job.ctl.send_replace(Ctl::Run);
            let active = *job.active.lock().unwrap();
            job.update(|s| s.state = active);
        }
        Ok(())
    }

    /// The worker notices at its next checkpoint (≤ one chunk) and reports
    /// `Cancelled`.
    pub fn cancel(&self, id: &str) -> Result<()> {
        let job = self.find(id)?;
        if !job.state().is_finished() {
            job.ctl.send_replace(Ctl::Cancel);
        }
        Ok(())
    }

    /// Drop completed/failed/cancelled jobs from the list.
    pub fn clear_finished(&self) {
        self.jobs.lock().unwrap().retain(|j| !j.state().is_finished());
    }

    pub fn subscribe(&self) -> broadcast::Receiver<JobSnapshot> {
        self.events.subscribe()
    }
}
