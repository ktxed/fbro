pub mod dirsize;
pub mod extract_archive;
pub mod fedup;
pub mod file_ops;
pub mod fp_bro;
pub mod integrity_compare;
pub(crate) mod git;
pub(crate) mod progress;
pub mod scan;
pub mod search;
pub mod viewer_prefetch;

use std::collections::{HashMap, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::bro::{BroInputItem, ResolvedBro};
use crate::vfs::VfsDriver;
use crate::ws::protocol::FedupBroResultSchema;

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
    Resume,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerificationPolicy {
    Fail,
    Prompt,
}

#[derive(Clone, Debug)]
pub struct DecisionResponse {
    pub decision: String,
    pub apply_to_all: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Queued,
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
    pub dest_name: Option<String>,
    pub source_path: String,
    pub size: Option<u64>,
    pub kind: String,
    pub mtime: Option<u64>,
    pub permissions: Option<String>,
}

/// Rich progress for file operations.
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOpProgress {
    pub total_files: usize,
    pub completed_files: usize,
    pub current_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    pub current_file_bytes_total: u64,
    pub current_file_bytes_copied: u64,
    pub total_bytes_total: u64,
    pub total_bytes_copied: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_file_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overall_percent: Option<u8>,
}

/// A pending conflict decision the job is waiting on.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDecision {
    pub kind: Option<String>,
    pub source: String,
    pub destination: String,
    pub message: Option<String>,
    pub options: Vec<String>,
    pub supports_apply_to_all: bool,
}

/// Tracks a running file operation job.
#[allow(dead_code)]
pub struct AppJob {
    pub job_id: String,
    pub kind: JobKind,
    pub source_panel: String,
    pub dest_panel: Option<String>,
    pub source_path: String,
    pub dest_path: Option<String>,
    pub state: JobState,
    pub progress: FileOpProgress,
    pub decision: Option<ConflictDecision>,
    pub error: Option<String>,
    pub background: bool,
    pub paused: bool,
    pub handle: Option<JoinHandle<()>>,
    pub cancel_tx: watch::Sender<bool>,
    pub pause_tx: watch::Sender<bool>,
    pub skip_current_file: Arc<AtomicBool>,
    pub keep_partial_files_on_cancel: Arc<AtomicBool>,
}

impl AppJob {
    pub fn set_keep_partial_files_on_cancel(&self, keep: bool) {
        self.keep_partial_files_on_cancel
            .store(keep, Ordering::Relaxed);
    }

    pub fn request_skip_current_file(&self) {
        self.skip_current_file.store(true, Ordering::SeqCst);
    }
}

pub enum JobKind {
    FileOp(FileOpJobMeta),
    FpBro(FpBroJobMeta),
    Search(SearchJobMeta),
    OutgoingTransfer(OutgoingTransferJobMeta),
    SyncBro(SyncBroJobMeta),
    FedupBro(FedupBroJobMeta),
    RemoteManifest(RemoteManifestJobMeta),
}

#[allow(dead_code)]
pub struct SearchJobMeta {
    pub params: search::SearchParams,
    pub source_id: String,
}

#[allow(dead_code)]
pub struct FileOpJobMeta {
    pub op_type: FileOpType,
    pub files: Vec<FileOpEntry>,
    pub conflict_policy: ConflictPolicy,
    pub apply_to_all_decision: Option<ConflictPolicy>,
}

#[allow(dead_code)]
pub struct FpBroJobMeta {
    pub bro_id: String,
    pub bro_name: String,
    pub output_path: String,
    pub inputs: Vec<BroInputItem>,
    pub params: HashMap<String, String>,
    pub bro: ResolvedBro,
}

#[allow(dead_code)]
pub struct OutgoingTransferJobMeta {
    pub peer_fp: String,
    pub completion_generation: u64,
}

#[allow(dead_code)]
pub struct SyncBroJobMeta {
    pub local_panel: String,
    pub remote_panel: String,
    pub local_root: String,
    pub remote_root: String,
    pub report_path: String,
}

#[allow(dead_code)]
pub struct FedupBroJobMeta {
    pub inputs: Vec<BroInputItem>,
    pub report_path: String,
    pub result: Option<Arc<FedupBroResultSchema>>,
}

#[allow(dead_code)]
pub struct RemoteManifestJobMeta {
    pub root: String,
    pub peer_fp: String,
}

/// Container for all active jobs in AppState.
pub struct JobStore {
    pub jobs: HashMap<String, AppJob>,
    pub fp_queue: VecDeque<String>,
    pub fp_running: Option<String>,
    pub file_queue: VecDeque<String>,
    pub file_queue_running: Option<String>,
    pub file_queue_completed: usize,
    pub queued_file_ops: HashMap<String, QueuedFileOpLaunch>,
    next_id: u64,
}

pub struct QueuedFileOpLaunch {
    pub op_type: FileOpType,
    pub files: Vec<FileOpEntry>,
    pub dest_path: Option<String>,
    pub conflict_policy: ConflictPolicy,
    pub verification_policy: VerificationPolicy,
    pub save_digest_file: bool,
    pub src_driver: Arc<dyn VfsDriver>,
    pub dst_driver: Option<Arc<dyn VfsDriver>>,
    pub preserve_attributes: bool,
    pub auto_resume_copies: bool,
    pub cancel_rx: watch::Receiver<bool>,
    pub pause_rx: watch::Receiver<bool>,
    pub decision_rx: DecisionRx,
    pub skip_current_file: Arc<AtomicBool>,
    pub keep_partial_files_on_cancel: Arc<AtomicBool>,
}

impl JobStore {
    pub fn new() -> Self {
        Self {
            jobs: HashMap::new(),
            fp_queue: VecDeque::new(),
            fp_running: None,
            file_queue: VecDeque::new(),
            file_queue_running: None,
            file_queue_completed: 0,
            queued_file_ops: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn next_job_id(&mut self) -> String {
        let id = format!("job-{}", self.next_id);
        self.next_id += 1;
        id
    }

    pub fn remove_from_fp_queue(&mut self, job_id: &str) {
        if let Some(index) = self.fp_queue.iter().position(|queued| queued == job_id) {
            self.fp_queue.remove(index);
        }
    }

    pub fn remove_from_file_queue(&mut self, job_id: &str) {
        if let Some(index) = self.file_queue.iter().position(|queued| queued == job_id) {
            self.file_queue.remove(index);
        }
        self.queued_file_ops.remove(job_id);
        if self.file_queue_running.as_deref() == Some(job_id) {
            self.file_queue_running = None;
        }
        if self.file_queue.is_empty() {
            self.file_queue_completed = 0;
        }
    }
}

/// Channel type for the file_ops worker to receive decisions.
pub type DecisionRx = mpsc::Receiver<DecisionResponse>;

#[cfg(test)]
mod tests {
    use super::JobStore;

    #[test]
    fn removing_last_file_queue_job_resets_completed_count() {
        let mut store = JobStore::new();
        store.file_queue.push_back("job-1".to_string());
        store.file_queue_running = Some("job-1".to_string());
        store.file_queue_completed = 2;

        store.remove_from_file_queue("job-1");

        assert!(store.file_queue.is_empty());
        assert_eq!(store.file_queue_running, None);
        assert_eq!(store.file_queue_completed, 0);
    }

    #[test]
    fn removing_pending_file_queue_job_keeps_active_job() {
        let mut store = JobStore::new();
        store.file_queue.push_back("job-1".to_string());
        store.file_queue.push_back("job-2".to_string());
        store.file_queue_running = Some("job-1".to_string());
        store.file_queue_completed = 1;

        store.remove_from_file_queue("job-2");

        assert_eq!(store.file_queue.iter().cloned().collect::<Vec<_>>(), vec!["job-1"]);
        assert_eq!(store.file_queue_running.as_deref(), Some("job-1"));
        assert_eq!(store.file_queue_completed, 1);
    }
}
