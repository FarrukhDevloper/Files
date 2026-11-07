//! Transfer queue: copy/move between any two sources (local↔VPS, VPS↔VPS)
//! with progress, pause, resume and cancel.
//!
//! OWNER: lead. The API below is the contract the app and UI rely on.

use serde::{Deserialize, Serialize};
use vpsfiles_vfs::{Location, VfsError};

mod job;
mod manager;

pub use manager::TransferManager;

pub type JobId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferOp {
    Copy,
    Move,
}

/// What to do when the destination name already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// Finder "Replace": the existing item (file or whole folder) is
    /// replaced. The new copy is staged under a hidden name and swapped in at
    /// the end, so a failed copy leaves the original untouched.
    Overwrite,
    /// Folder onto folder: copy into it, replacing clashing files and keeping
    /// the rest. Anything else behaves like `Overwrite`.
    Merge,
    Skip,
    /// Finder's "Keep Both": `report.pdf` -> `report 2.pdf`.
    KeepBoth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferRequest {
    pub op: TransferOp,
    /// Files and/or directories; may come from different sources.
    pub sources: Vec<Location>,
    pub dest_dir: Location,
    pub conflict: ConflictPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
    /// Walking source trees to compute totals.
    Scanning,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        matches!(self, JobState::Completed | JobState::Failed | JobState::Cancelled)
    }
}

/// Everything the UI needs to draw one row of the transfers panel.
/// Broadcast on every state change and at most ~5×/s while running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: JobId,
    pub op: TransferOp,
    pub state: JobState,
    pub sources: Vec<Location>,
    pub dest_dir: Location,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub total_files: u64,
    pub done_files: u64,
    /// Path of the file being copied right now.
    pub current: Option<String>,
    pub bytes_per_sec: u64,
    pub error: Option<VfsError>,
    /// Unix epoch milliseconds.
    pub created_at: i64,
    pub finished_at: Option<i64>,
}
