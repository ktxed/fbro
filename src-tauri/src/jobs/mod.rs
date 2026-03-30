pub mod dirsize;
pub mod file_ops;
pub mod scan;

use std::collections::HashMap;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

// ─── Scan handle (existing) ─────────────────────────────────────────────────

pub struct ScanHandle {
    pub handle: JoinHandle<()>,
}

impl ScanHandle {
    pub fn abort(&self) {
        self.handle.abort();
    }
}

// ─── File operation types ───────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FileOpType {
    Copy,
    Move,
    Delete,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConflictPolicy {
    Ask,
    Skip,
    Overwrite,
}

#[derive(Clone, Debug)]
pub struct DecisionResponse {
    pub decision: String,
    pub apply_to_all: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Running,
    AwaitingDecision,
    Completed,
    Cancelled,
    Failed,
}

/// A single file to operate on.
#[derive(Clone, Debug, serde::Serialize)]
pub struct FileOpEntry {
    pub name: String,
    pub source_path: String,
    pub size: Option<u64>,
    pub kind: String,
}

/// Rich progress for file operations.
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOpProgress {
    pub total_files: usize,
    pub completed_files: usize,
    pub current_file: Option<String>,
    pub current_file_bytes_total: u64,
    pub current_file_bytes_copied: u64,
}

/// A pending conflict decision the job is waiting on.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDecision {
    pub source: String,
    pub destination: String,
    pub options: Vec<String>,
    pub supports_apply_to_all: bool,
}

/// Tracks a running file operation job.
pub struct FileOpJob {
    pub job_id: String,
    pub op_type: FileOpType,
    pub source_panel: String,
    pub dest_panel: Option<String>,
    pub dest_path: Option<String>,
    pub files: Vec<FileOpEntry>,
    pub conflict_policy: ConflictPolicy,
    pub state: JobState,
    pub progress: FileOpProgress,
    pub decision: Option<ConflictDecision>,
    pub apply_to_all_decision: Option<ConflictPolicy>,
    pub error: Option<String>,
    pub background: bool,
    pub paused: bool,
    pub handle: JoinHandle<()>,
    pub cancel_tx: watch::Sender<bool>,
    pub pause_tx: watch::Sender<bool>,
}

/// Container for all active jobs in AppState.
pub struct JobStore {
    pub jobs: HashMap<String, FileOpJob>,
    next_id: u64,
}

impl JobStore {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn next_job_id(&mut self) -> String {
        let id = format!("job-{}", self.next_id);
        self.next_id += 1;
        id
    }
}

/// Channel type for the file_ops worker to receive decisions.
pub type DecisionRx = mpsc::Receiver<DecisionResponse>;
