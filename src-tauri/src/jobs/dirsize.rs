use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch, Mutex, Notify};

use crate::state::SharedState;
use crate::ws::protocol::{now, ServerMessage};

const MAX_CONCURRENT: usize = 4;
const CACHE_TTL: Duration = Duration::from_secs(60);
/// Maximum wall-clock time allowed for a single directory-size walk.
/// If a walk reaches this limit, the dispatcher asks it to cancel and keeps
/// the worker slot occupied until the blocking scan actually returns. That
/// makes blocked syscalls visible instead of hiding detached workers.
const CALC_TIMEOUT: Duration = Duration::from_secs(60);
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(500);
const STALE_LOG_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirSizeTaskStatus {
    Queued,
    Running,
}

impl DirSizeTaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
        }
    }
}

#[derive(Clone, Debug)]
struct TrackedTask {
    task_id: u64,
    full_path: String,
    enqueued_at: u64,
    started_at: Option<u64>,
    last_progress_at: Option<u64>,
    status: DirSizeTaskStatus,
    cancel_tx: watch::Sender<bool>,
    progress: DirSizeProgress,
}

struct DirSizeWorkItem {
    task_id: u64,
    panel_id: String,
    entry_name: String,
    full_path: String,
    enqueued_at: u64,
    state: SharedState,
    tx: mpsc::Sender<ServerMessage>,
    cancel_tx: watch::Sender<bool>,
    cancel_rx: watch::Receiver<bool>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirSizeTaskSnapshot {
    pub task_id: u64,
    pub panel_id: String,
    pub entry_name: String,
    pub full_path: String,
    pub enqueued_at: u64,
    pub started_at: Option<u64>,
    pub last_progress_at: Option<u64>,
    pub status: String,
    pub dirs_scanned: u64,
    pub files_scanned: u64,
    pub bytes_scanned: u64,
    pub errors: u64,
    pub current_path: Option<String>,
    pub current_action: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct DirSizeProgress {
    dirs_scanned: u64,
    files_scanned: u64,
    bytes_scanned: u64,
    errors: u64,
    current_path: Option<String>,
    current_action: Option<String>,
}

impl DirSizeProgress {
    fn touched(
        &mut self,
        now_ms: u64,
        current_path: Option<String>,
        current_action: Option<&str>,
    ) -> DirSizeProgressUpdate {
        if let Some(path) = current_path {
            self.current_path = Some(path);
        }
        if let Some(action) = current_action {
            self.current_action = Some(action.to_string());
        }
        DirSizeProgressUpdate {
            timestamp: now_ms,
            progress: self.clone(),
        }
    }
}

#[derive(Clone, Debug)]
struct DirSizeProgressUpdate {
    timestamp: u64,
    progress: DirSizeProgress,
}

#[derive(Clone, Debug)]
struct DirSizeResult {
    size: u64,
    progress: DirSizeProgress,
}

enum DirSizeOutcome {
    Completed(DirSizeResult),
    Cancelled,
    Panicked,
    TimedOut,
}

// ── DirSizeCache ─────────────────────────────────────────────────────────────

struct CacheEntry {
    size: u64,
    computed_at: Instant,
}

/// Scoped cache for computed directory sizes.  Keyed by full filesystem path.
/// Entries expire after CACHE_TTL (1 minute).
pub struct DirSizeCache {
    entries: HashMap<String, CacheEntry>,
}

impl DirSizeCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Returns the cached size if the entry exists and has not expired.
    pub fn get(&self, path: &str) -> Option<u64> {
        self.entries
            .get(path)
            .filter(|e| e.computed_at.elapsed() < CACHE_TTL)
            .map(|e| e.size)
    }

    fn insert(&mut self, path: String, size: u64) {
        self.entries.insert(
            path,
            CacheEntry {
                size,
                computed_at: Instant::now(),
            },
        );
    }

    /// Remove an entry so the next calculation starts fresh.
    pub fn invalidate(&mut self, path: &str) {
        self.entries.remove(path);
    }
}

// ── DirSizeQueue ─────────────────────────────────────────────────────────────

pub struct DirSizeQueue {
    tasks: Arc<Mutex<HashMap<(String, String), TrackedTask>>>,
    pending: Arc<Mutex<VecDeque<DirSizeWorkItem>>>,
    notify: Arc<Notify>,
    cache: Arc<Mutex<DirSizeCache>>,
    next_task_id: AtomicU64,
}

impl DirSizeQueue {
    pub fn new() -> Self {
        let tasks = Arc::new(Mutex::new(HashMap::new()));
        let pending = Arc::new(Mutex::new(VecDeque::new()));
        let notify = Arc::new(Notify::new());
        let cache = Arc::new(Mutex::new(DirSizeCache::new()));

        spawn_queue_monitor(Arc::clone(&tasks), Arc::clone(&pending));
        for worker_id in 0..MAX_CONCURRENT {
            spawn_worker_loop(
                worker_id,
                Arc::clone(&tasks),
                Arc::clone(&pending),
                Arc::clone(&notify),
                Arc::clone(&cache),
            );
        }

        Self {
            tasks,
            pending,
            notify,
            cache,
            next_task_id: AtomicU64::new(1),
        }
    }

    pub async fn snapshot(&self) -> Vec<DirSizeTaskSnapshot> {
        let tasks = self.tasks.lock().await;
        let mut snapshot: Vec<DirSizeTaskSnapshot> = tasks
            .iter()
            .map(|((panel_id, entry_name), task)| DirSizeTaskSnapshot {
                task_id: task.task_id,
                panel_id: panel_id.clone(),
                entry_name: entry_name.clone(),
                full_path: task.full_path.clone(),
                enqueued_at: task.enqueued_at,
                started_at: task.started_at,
                last_progress_at: task.last_progress_at,
                status: task.status.as_str().to_string(),
                dirs_scanned: task.progress.dirs_scanned,
                files_scanned: task.progress.files_scanned,
                bytes_scanned: task.progress.bytes_scanned,
                errors: task.progress.errors,
                current_path: task.progress.current_path.clone(),
                current_action: task.progress.current_action.clone(),
            })
            .collect();
        snapshot.sort_by(|left, right| {
            left.enqueued_at
                .cmp(&right.enqueued_at)
                .then_with(|| left.panel_id.cmp(&right.panel_id))
                .then_with(|| left.entry_name.cmp(&right.entry_name))
        });
        snapshot
    }

    /// Return a cached size for `path` if one exists and has not expired.
    pub async fn get_cached_size(&self, path: &str) -> Option<u64> {
        self.cache.lock().await.get(path)
    }

    /// Invalidate the cache entry for `path` (called before explicit recalculation).
    pub async fn invalidate_cache(&self, path: &str) {
        self.cache.lock().await.invalidate(path);
    }

    /// Enqueue a recursive size calculation for a directory entry.
    /// If already queued for the same (panel_id, entry_name), the old task is
    /// replaced (cancel signal sent) and a fresh one is started.
    pub async fn enqueue(
        &self,
        panel_id: String,
        entry_name: String,
        full_path: String,
        state: SharedState,
        tx: mpsc::Sender<ServerMessage>,
    ) {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let task_id = self.next_task_id.fetch_add(1, Ordering::Relaxed);
        let enqueued_at = now();

        // Cancel any existing task for this entry and replace it.
        let replaced_task_id = {
            let mut tasks = self.tasks.lock().await;
            let old = tasks.insert(
                (panel_id.clone(), entry_name.clone()),
                TrackedTask {
                    task_id,
                    full_path: full_path.clone(),
                    enqueued_at,
                    started_at: None,
                    last_progress_at: None,
                    status: DirSizeTaskStatus::Queued,
                    cancel_tx: cancel_tx.clone(),
                    progress: DirSizeProgress::default(),
                },
            );
            if let Some(old) = &old {
                eprintln!(
                    "[dirsize] replace: old_task={} new_task={} panel={} entry={} path={}",
                    old.task_id, task_id, panel_id, entry_name, full_path
                );
                let _ = old.cancel_tx.send(true);
            }
            old.map(|task| task.task_id)
        };
        if let Some(old_task_id) = replaced_task_id {
            remove_pending_task(&self.pending, old_task_id).await;
        }
        emit_queue_patch(&self.tasks, &tx).await;
        eprintln!(
            "[dirsize] task tracked: task={task_id} panel={panel_id} entry={entry_name} queued_at={enqueued_at}"
        );

        // Mark as calculating in panel state immediately.
        let panel_patch = {
            let mut s = state.lock().await;
            if let Some(panel) = s.panels.get_mut(&panel_id) {
                panel.calculating_size.insert(entry_name.clone());
                panel.version += 1;
                let calc = calculating_vec(&panel.calculating_size);
                let v = panel.version;
                Some(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id.clone()),
                    version: v,
                    patch: serde_json::json!({ "calculatingSize": calc }),
                })
            } else {
                eprintln!(
                    "[dirsize] enqueue abandoned: task={task_id} panel={panel_id} entry={entry_name} missing_panel"
                );
                {
                    let mut tasks = self.tasks.lock().await;
                    tasks.remove(&(panel_id.clone(), entry_name.clone()));
                }
                emit_queue_patch(&self.tasks, &tx).await;
                return;
            }
        };
        if let Some(patch) = panel_patch {
            match tx.try_send(patch) {
                Ok(()) => eprintln!(
                    "[dirsize] panel calculating patch sent: task={task_id} panel={panel_id} entry={entry_name}"
                ),
                Err(error) => eprintln!(
                    "[dirsize] panel calculating patch dropped: task={task_id} panel={panel_id} entry={entry_name} error={error}"
                ),
            }
        }

        eprintln!(
            "[dirsize] enqueued: task={task_id} panel={panel_id} entry={entry_name} path={full_path}"
        );

        {
            let mut pending = self.pending.lock().await;
            pending.push_back(DirSizeWorkItem {
                task_id,
                panel_id: panel_id.clone(),
                entry_name: entry_name.clone(),
                full_path: full_path.clone(),
                enqueued_at,
                state,
                tx: tx.clone(),
                cancel_tx: cancel_tx.clone(),
                cancel_rx,
            });
            eprintln!(
                "[dirsize] pending pushed: task={task_id} panel={panel_id} entry={entry_name} pending_len={}",
                pending.len()
            );
        }
        self.notify.notify_one();
    }

    /// Signal an active calculation to stop. The state update happens
    /// immediately (called from the intent handler) so the FE gets instant
    /// feedback; the background task will clean up when it detects the signal.
    pub async fn cancel(&self, panel_id: &str, entry_name: &str, tx: &mpsc::Sender<ServerMessage>) {
        let removed = {
            let mut tasks = self.tasks.lock().await;
            tasks.remove(&(panel_id.to_string(), entry_name.to_string()))
        };
        if let Some(task) = removed {
            eprintln!(
                "[dirsize] cancel requested: task={} panel={} entry={} dirs={} files={} bytes={}",
                task.task_id,
                panel_id,
                entry_name,
                task.progress.dirs_scanned,
                task.progress.files_scanned,
                task.progress.bytes_scanned
            );
            remove_pending_task(&self.pending, task.task_id).await;
            let _ = task.cancel_tx.send(true);
            emit_queue_patch(&self.tasks, tx).await;
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct MonitorTaskSnapshot {
    task_id: u64,
    panel_id: String,
    entry_name: String,
    status: DirSizeTaskStatus,
    enqueued_at: u64,
    started_at: Option<u64>,
    last_progress_at: Option<u64>,
    progress: DirSizeProgress,
}

fn spawn_worker_loop(
    worker_id: usize,
    tasks: Arc<Mutex<HashMap<(String, String), TrackedTask>>>,
    pending: Arc<Mutex<VecDeque<DirSizeWorkItem>>>,
    notify: Arc<Notify>,
    cache: Arc<Mutex<DirSizeCache>>,
) {
    tokio::spawn(async move {
        loop {
            let item = loop {
                if let Some(item) = pending.lock().await.pop_front() {
                    break item;
                }
                notify.notified().await;
            };

            let current =
                is_current_task(&tasks, &item.panel_id, &item.entry_name, item.task_id).await;
            if !current {
                eprintln!(
                    "[dirsize] worker skip stale pending: worker={worker_id} task={} panel={} entry={}",
                    item.task_id, item.panel_id, item.entry_name
                );
                continue;
            }

            eprintln!(
                "[dirsize] worker dequeued: worker={worker_id} task={} panel={} entry={} path={}",
                item.task_id, item.panel_id, item.entry_name, item.full_path
            );
            run_work_item(worker_id, item, &tasks, &cache).await;
        }
    });
}

async fn run_work_item(
    worker_id: usize,
    item: DirSizeWorkItem,
    tasks: &Arc<Mutex<HashMap<(String, String), TrackedTask>>>,
    cache: &Arc<Mutex<DirSizeCache>>,
) {
    let DirSizeWorkItem {
        task_id,
        panel_id,
        entry_name,
        full_path,
        enqueued_at,
        state,
        tx,
        cancel_tx,
        cancel_rx,
    } = item;

    let started_at = now();
    let queue_wait_ms = started_at.saturating_sub(enqueued_at);
    eprintln!(
        "[dirsize] slot acquired: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} wait_ms={queue_wait_ms}"
    );

    if *cancel_rx.borrow() {
        eprintln!(
            "[dirsize] cancelled before start: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} wait_ms={queue_wait_ms}"
        );
        let owned_task = remove_task_if_matches(tasks, &panel_id, &entry_name, task_id).await;
        emit_queue_patch(tasks, &tx).await;
        if owned_task {
            patch_remove_calculating(&state, &tx, &panel_id, &entry_name).await;
        }
        return;
    }

    {
        let mut tasks_guard = tasks.lock().await;
        if let Some(task) = tasks_guard.get_mut(&(panel_id.clone(), entry_name.clone())) {
            if task.task_id == task_id {
                task.status = DirSizeTaskStatus::Running;
                task.started_at = Some(started_at);
            }
        }
    }
    eprintln!(
        "[dirsize] started: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} wait_ms={queue_wait_ms} path={full_path}"
    );
    emit_queue_patch(tasks, &tx).await;

    let path = full_path.clone();
    let mut latest_progress = DirSizeProgress::default();
    latest_progress.current_path = Some(full_path.clone());
    latest_progress.current_action = Some("spawning_worker".to_string());
    let mut latest_progress_at = now();
    update_task_progress(
        tasks,
        &panel_id,
        &entry_name,
        task_id,
        latest_progress_at,
        latest_progress.clone(),
    )
    .await;
    emit_queue_patch(tasks, &tx).await;
    eprintln!(
        "[dirsize] spawning worker: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name}"
    );
    let cancel_clone = cancel_rx.clone();
    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<DirSizeProgressUpdate>();
    let worker_panel_id = panel_id.clone();
    let worker_entry_name = entry_name.clone();
    let mut handle = tokio::task::spawn_blocking(move || {
        eprintln!(
            "[dirsize] worker entered: worker={worker_id} task={task_id} panel={worker_panel_id} entry={worker_entry_name} path={path}"
        );
        calculate_dir_size(&path, &cancel_clone, progress_tx)
    });
    latest_progress.current_action = Some("worker_spawned".to_string());
    latest_progress_at = now();
    update_task_progress(
        tasks,
        &panel_id,
        &entry_name,
        task_id,
        latest_progress_at,
        latest_progress.clone(),
    )
    .await;
    emit_queue_patch(tasks, &tx).await;

    let timeout = tokio::time::sleep(CALC_TIMEOUT);
    tokio::pin!(timeout);
    let heartbeat = tokio::time::sleep(STALE_LOG_INTERVAL);
    tokio::pin!(heartbeat);
    let mut last_emit = Instant::now()
        .checked_sub(PROGRESS_EMIT_INTERVAL)
        .unwrap_or_else(Instant::now);
    let mut cancel_wait = cancel_rx.clone();
    let mut progress_open = true;

    let outcome = loop {
        tokio::select! {
            result = &mut handle => {
                break match result {
                    Ok(Some(result)) => DirSizeOutcome::Completed(result),
                    Ok(None) => DirSizeOutcome::Cancelled,
                    Err(error) => {
                        eprintln!(
                            "[dirsize] worker join error: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} error={error}"
                        );
                        DirSizeOutcome::Panicked
                    }
                };
            }
            maybe_progress = progress_rx.recv(), if progress_open => {
                match maybe_progress {
                    Some(update) => {
                        latest_progress = update.progress;
                        latest_progress_at = update.timestamp;
                        eprintln!(
                            "[dirsize] worker progress: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} dirs={} files={} bytes={} errors={} action={} current={}",
                            latest_progress.dirs_scanned,
                            latest_progress.files_scanned,
                            latest_progress.bytes_scanned,
                            latest_progress.errors,
                            latest_progress.current_action.as_deref().unwrap_or(""),
                            latest_progress.current_path.as_deref().unwrap_or("")
                        );
                        if last_emit.elapsed() >= PROGRESS_EMIT_INTERVAL {
                            update_task_progress(
                                tasks,
                                &panel_id,
                                &entry_name,
                                task_id,
                                latest_progress_at,
                                latest_progress.clone(),
                            ).await;
                            emit_queue_patch(tasks, &tx).await;
                            last_emit = Instant::now();
                        }
                    }
                    None => {
                        progress_open = false;
                        eprintln!(
                            "[dirsize] worker progress channel closed: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name}"
                        );
                    }
                }
            }
            _ = &mut heartbeat => {
                let idle_ms = now().saturating_sub(latest_progress_at);
                eprintln!(
                    "[dirsize] heartbeat: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} elapsed_ms={} idle_ms={idle_ms} dirs={} files={} bytes={} errors={} action={} current={}",
                    now().saturating_sub(started_at),
                    latest_progress.dirs_scanned,
                    latest_progress.files_scanned,
                    latest_progress.bytes_scanned,
                    latest_progress.errors,
                    latest_progress.current_action.as_deref().unwrap_or(""),
                    latest_progress.current_path.as_deref().unwrap_or("")
                );
                heartbeat
                    .as_mut()
                    .reset(tokio::time::Instant::now() + STALE_LOG_INTERVAL);
            }
            _ = &mut timeout => {
                eprintln!(
                    "[dirsize] timeout firing: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} timeout_ms={}",
                    CALC_TIMEOUT.as_millis()
                );
                let _ = cancel_tx.send(true);
                break DirSizeOutcome::TimedOut;
            }
            changed = cancel_wait.changed() => {
                if changed.is_ok() && *cancel_wait.borrow() {
                    eprintln!(
                        "[dirsize] cancel observed: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name}"
                    );
                    break DirSizeOutcome::Cancelled;
                }
            }
        }
    };

    if matches!(
        outcome,
        DirSizeOutcome::TimedOut | DirSizeOutcome::Cancelled
    ) {
        latest_progress.current_action = Some(
            match outcome {
                DirSizeOutcome::TimedOut => "timeout_waiting_worker_exit",
                DirSizeOutcome::Cancelled => "cancel_waiting_worker_exit",
                DirSizeOutcome::Completed(_) | DirSizeOutcome::Panicked => "waiting_worker_exit",
            }
            .to_string(),
        );
        latest_progress_at = now();
        update_task_progress(
            tasks,
            &panel_id,
            &entry_name,
            task_id,
            latest_progress_at,
            latest_progress.clone(),
        )
        .await;
        emit_queue_patch(tasks, &tx).await;
        eprintln!(
            "[dirsize] waiting for worker exit after {}: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name}",
            outcome_name(&outcome)
        );
        let final_result = handle.await;
        match final_result {
            Ok(Some(result)) => {
                latest_progress = result.progress;
                latest_progress_at = now();
                eprintln!(
                    "[dirsize] worker returned after {}: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} size={} dirs={} files={} errors={}",
                    outcome_name(&outcome),
                    result.size,
                    latest_progress.dirs_scanned,
                    latest_progress.files_scanned,
                    latest_progress.errors
                );
            }
            Ok(None) => eprintln!(
                "[dirsize] worker acknowledged cancellation: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name}"
            ),
            Err(error) => eprintln!(
                "[dirsize] worker join error after {}: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} error={error}",
                outcome_name(&outcome)
            ),
        }
    }

    update_task_progress(
        tasks,
        &panel_id,
        &entry_name,
        task_id,
        latest_progress_at,
        latest_progress.clone(),
    )
    .await;
    let owned_task = remove_task_if_matches(tasks, &panel_id, &entry_name, task_id).await;
    emit_queue_patch(tasks, &tx).await;

    match outcome {
        DirSizeOutcome::Completed(result) => {
            let size = result.size;
            latest_progress = result.progress;
            if !owned_task {
                eprintln!(
                    "[dirsize] stale finish ignored: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} size={size}"
                );
                cache.lock().await.insert(full_path, size);
                return;
            }
            let elapsed_ms = now().saturating_sub(started_at);
            eprintln!(
                "[dirsize] finished: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} elapsed_ms={elapsed_ms} size={size} dirs={} files={} errors={}",
                latest_progress.dirs_scanned,
                latest_progress.files_scanned,
                latest_progress.errors
            );
            let panel_patch = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    if let Some(e) = panel.entries.iter_mut().find(|e| e.name == entry_name) {
                        e.size = Some(size);
                    }
                    panel.calculating_size.remove(&entry_name);
                    panel.version += 1;
                    let calc = calculating_vec(&panel.calculating_size);

                    let version = panel.version;
                    Some(ServerMessage::StatePatch {
                        timestamp: now(),
                        domain: "panels".to_string(),
                        entity_id: Some(panel_id.clone()),
                        version,
                        patch: serde_json::json!({
                            "calculatingSize": calc,
                            "updateEntrySize": { "name": entry_name, "size": size }
                        }),
                    })
                } else {
                    eprintln!(
                        "[dirsize] finish state skipped: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} missing_panel"
                    );
                    None
                }
            };
            if let Some(patch) = panel_patch {
                let version = match &patch {
                    ServerMessage::StatePatch { version, .. } => *version,
                    _ => 0,
                };
                match tx.send(patch).await {
                    Ok(()) => eprintln!(
                        "[dirsize] panel size patch sent: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} version={version} size={size}"
                    ),
                    Err(error) => eprintln!(
                        "[dirsize] panel size patch failed: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} version={version} size={size} error={error}"
                    ),
                }
            }
            cache.lock().await.insert(full_path, size);
        }
        DirSizeOutcome::Cancelled => {
            let elapsed_ms = now().saturating_sub(started_at);
            eprintln!(
                "[dirsize] cancelled: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} elapsed_ms={elapsed_ms} dirs={} files={} bytes={} errors={} action={} current={}",
                latest_progress.dirs_scanned,
                latest_progress.files_scanned,
                latest_progress.bytes_scanned,
                latest_progress.errors,
                latest_progress.current_action.as_deref().unwrap_or(""),
                latest_progress.current_path.as_deref().unwrap_or("")
            );
            if owned_task {
                patch_remove_calculating(&state, &tx, &panel_id, &entry_name).await;
            }
        }
        DirSizeOutcome::Panicked => {
            eprintln!(
                "[dirsize] error: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} worker panicked"
            );
            if owned_task {
                patch_remove_calculating(&state, &tx, &panel_id, &entry_name).await;
            }
        }
        DirSizeOutcome::TimedOut => {
            eprintln!(
                "[dirsize] timeout: worker={worker_id} task={task_id} panel={panel_id} entry={entry_name} timeout_ms={} dirs={} files={} bytes={} errors={} action={} current={}",
                CALC_TIMEOUT.as_millis(),
                latest_progress.dirs_scanned,
                latest_progress.files_scanned,
                latest_progress.bytes_scanned,
                latest_progress.errors,
                latest_progress.current_action.as_deref().unwrap_or(""),
                latest_progress.current_path.as_deref().unwrap_or("")
            );
            if owned_task {
                patch_remove_calculating(&state, &tx, &panel_id, &entry_name).await;
            }
        }
    }
}

fn outcome_name(outcome: &DirSizeOutcome) -> &'static str {
    match outcome {
        DirSizeOutcome::Completed(_) => "completed",
        DirSizeOutcome::Cancelled => "cancelled",
        DirSizeOutcome::Panicked => "panicked",
        DirSizeOutcome::TimedOut => "timed_out",
    }
}

fn spawn_queue_monitor(
    tasks: Arc<Mutex<HashMap<(String, String), TrackedTask>>>,
    pending: Arc<Mutex<VecDeque<DirSizeWorkItem>>>,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(STALE_LOG_INTERVAL).await;

            let now_ms = now();
            let pending_len = pending.lock().await.len();
            let snapshot: Vec<MonitorTaskSnapshot> = {
                let tasks = tasks.lock().await;
                tasks
                    .iter()
                    .map(|((panel_id, entry_name), task)| MonitorTaskSnapshot {
                        task_id: task.task_id,
                        panel_id: panel_id.clone(),
                        entry_name: entry_name.clone(),
                        status: task.status,
                        enqueued_at: task.enqueued_at,
                        started_at: task.started_at,
                        last_progress_at: task.last_progress_at,
                        progress: task.progress.clone(),
                    })
                    .collect()
            };

            if snapshot.is_empty() {
                continue;
            }

            let running = snapshot
                .iter()
                .filter(|task| task.status == DirSizeTaskStatus::Running)
                .count();
            let queued = snapshot.len().saturating_sub(running);
            let stale_count = snapshot
                .iter()
                .filter(|task| {
                    let idle_from = task
                        .last_progress_at
                        .or(task.started_at)
                        .unwrap_or(task.enqueued_at);
                    now_ms.saturating_sub(idle_from) >= STALE_LOG_INTERVAL.as_millis() as u64
                })
                .count();

            eprintln!(
                "[dirsize] queue heartbeat: total={} running={running} queued={queued} pending={pending_len} stale={stale_count}",
                snapshot.len()
            );

            for task in snapshot
                .iter()
                .filter(|task| task.status == DirSizeTaskStatus::Running)
                .chain(
                    snapshot
                        .iter()
                        .filter(|task| task.status == DirSizeTaskStatus::Queued)
                        .take(4),
                )
            {
                let elapsed_from = task.started_at.unwrap_or(task.enqueued_at);
                let idle_from = task
                    .last_progress_at
                    .or(task.started_at)
                    .unwrap_or(task.enqueued_at);
                eprintln!(
                    "[dirsize] queue task: task={} status={} panel={} entry={} elapsed_ms={} idle_ms={} dirs={} files={} bytes={} errors={} action={} current={}",
                    task.task_id,
                    task.status.as_str(),
                    task.panel_id,
                    task.entry_name,
                    now_ms.saturating_sub(elapsed_from),
                    now_ms.saturating_sub(idle_from),
                    task.progress.dirs_scanned,
                    task.progress.files_scanned,
                    task.progress.bytes_scanned,
                    task.progress.errors,
                    task.progress.current_action.as_deref().unwrap_or(""),
                    task.progress.current_path.as_deref().unwrap_or("")
                );
            }
        }
    });
}

async fn remove_task_if_matches(
    tasks: &Arc<Mutex<HashMap<(String, String), TrackedTask>>>,
    panel_id: &str,
    entry_name: &str,
    task_id: u64,
) -> bool {
    let mut tasks = tasks.lock().await;
    let key = (panel_id.to_string(), entry_name.to_string());
    let should_remove = tasks
        .get(&key)
        .map(|task| task.task_id == task_id)
        .unwrap_or(false);
    if should_remove {
        tasks.remove(&key);
        eprintln!("[dirsize] task removed: task={task_id} panel={panel_id} entry={entry_name}");
    } else {
        eprintln!(
            "[dirsize] task remove skipped: task={task_id} panel={panel_id} entry={entry_name}"
        );
    }
    should_remove
}

async fn is_current_task(
    tasks: &Arc<Mutex<HashMap<(String, String), TrackedTask>>>,
    panel_id: &str,
    entry_name: &str,
    task_id: u64,
) -> bool {
    let tasks = tasks.lock().await;
    tasks
        .get(&(panel_id.to_string(), entry_name.to_string()))
        .map(|task| task.task_id == task_id)
        .unwrap_or(false)
}

async fn remove_pending_task(pending: &Arc<Mutex<VecDeque<DirSizeWorkItem>>>, task_id: u64) {
    let mut pending = pending.lock().await;
    let before = pending.len();
    pending.retain(|item| item.task_id != task_id);
    let removed = before.saturating_sub(pending.len());
    if removed > 0 {
        eprintln!("[dirsize] pending removed: task={task_id} removed={removed}");
    }
}

async fn update_task_progress(
    tasks: &Arc<Mutex<HashMap<(String, String), TrackedTask>>>,
    panel_id: &str,
    entry_name: &str,
    task_id: u64,
    last_progress_at: u64,
    progress: DirSizeProgress,
) {
    let mut tasks = tasks.lock().await;
    if let Some(task) = tasks.get_mut(&(panel_id.to_string(), entry_name.to_string())) {
        if task.task_id == task_id {
            task.last_progress_at = Some(last_progress_at);
            task.progress = progress;
        }
    }
}

async fn emit_queue_patch(
    tasks: &Arc<Mutex<HashMap<(String, String), TrackedTask>>>,
    tx: &mpsc::Sender<ServerMessage>,
) {
    let snapshot = {
        let tasks = tasks.lock().await;
        let mut snapshot: Vec<DirSizeTaskSnapshot> = tasks
            .iter()
            .map(|((panel_id, entry_name), task)| DirSizeTaskSnapshot {
                task_id: task.task_id,
                panel_id: panel_id.clone(),
                entry_name: entry_name.clone(),
                full_path: task.full_path.clone(),
                enqueued_at: task.enqueued_at,
                started_at: task.started_at,
                last_progress_at: task.last_progress_at,
                status: task.status.as_str().to_string(),
                dirs_scanned: task.progress.dirs_scanned,
                files_scanned: task.progress.files_scanned,
                bytes_scanned: task.progress.bytes_scanned,
                errors: task.progress.errors,
                current_path: task.progress.current_path.clone(),
                current_action: task.progress.current_action.clone(),
            })
            .collect();
        snapshot.sort_by(|left, right| {
            left.enqueued_at
                .cmp(&right.enqueued_at)
                .then_with(|| left.panel_id.cmp(&right.panel_id))
                .then_with(|| left.entry_name.cmp(&right.entry_name))
        });
        snapshot
    };
    match tx.try_send(ServerMessage::StatePatch {
        timestamp: now(),
        domain: "dirSizeQueue".to_string(),
        entity_id: None,
        version: 0,
        patch: serde_json::json!({ "tasks": snapshot }),
    }) {
        Ok(()) => eprintln!("[dirsize] queue patch sent: tasks={}", snapshot.len()),
        Err(error) => {
            eprintln!(
                "[dirsize] queue patch dropped: tasks={} error={error}",
                snapshot.len()
            );
        }
    }
}

fn calculating_vec(set: &std::collections::HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = set.iter().cloned().collect();
    v.sort();
    v
}

async fn patch_remove_calculating(
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    panel_id: &str,
    entry_name: &str,
) {
    let panel_patch = {
        let mut s = state.lock().await;
        if let Some(panel) = s.panels.get_mut(panel_id) {
            panel.calculating_size.remove(entry_name);
            panel.version += 1;
            let calc = calculating_vec(&panel.calculating_size);
            let version = panel.version;
            Some(ServerMessage::StatePatch {
                timestamp: now(),
                domain: "panels".to_string(),
                entity_id: Some(panel_id.to_string()),
                version,
                patch: serde_json::json!({ "calculatingSize": calc }),
            })
        } else {
            eprintln!(
                "[dirsize] panel calculating remove skipped: panel={panel_id} entry={entry_name} missing_panel"
            );
            None
        }
    };
    if let Some(patch) = panel_patch {
        let version = match &patch {
            ServerMessage::StatePatch { version, .. } => *version,
            _ => 0,
        };
        match tx.send(patch).await {
            Ok(()) => eprintln!(
                "[dirsize] panel calculating remove patch sent: panel={panel_id} entry={entry_name} version={version}"
            ),
            Err(error) => eprintln!(
                "[dirsize] panel calculating remove patch failed: panel={panel_id} entry={entry_name} version={version} error={error}"
            ),
        }
    }
}

// ── Blocking recursive traversal ─────────────────────────────────────────────

/// Returns the total byte size of all files under `path`, or `None` if
/// cancelled. Symlinks are not followed to avoid cycles.
fn calculate_dir_size(
    path: &str,
    cancel: &watch::Receiver<bool>,
    progress_tx: tokio::sync::mpsc::UnboundedSender<DirSizeProgressUpdate>,
) -> Option<DirSizeResult> {
    let mut total: u64 = 0;
    let mut stack = vec![PathBuf::from(path)];
    let mut progress = DirSizeProgress::default();
    let mut ops: u64 = 0;

    while let Some(dir) = stack.pop() {
        if *cancel.borrow() {
            eprintln!(
                "[dirsize] scan cancelled before read_dir: dirs={} files={} bytes={} errors={} path={}",
                progress.dirs_scanned,
                progress.files_scanned,
                progress.bytes_scanned,
                progress.errors,
                dir.display()
            );
            return None;
        }
        progress.dirs_scanned += 1;
        let current_path = dir.to_string_lossy().to_string();
        eprintln!(
            "[dirsize] read_dir begin: dirs={} files={} bytes={} errors={} path={}",
            progress.dirs_scanned,
            progress.files_scanned,
            progress.bytes_scanned,
            progress.errors,
            dir.display()
        );
        let _ = progress_tx.send(progress.touched(now(), Some(current_path), Some("read_dir")));

        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => {
                progress.errors += 1;
                eprintln!(
                    "[dirsize] read_dir error: dirs={} files={} bytes={} errors={} path={}",
                    progress.dirs_scanned,
                    progress.files_scanned,
                    progress.bytes_scanned,
                    progress.errors,
                    dir.display()
                );
                let _ = progress_tx.send(progress.touched(now(), None, Some("read_dir_error")));
                continue;
            }
        };
        eprintln!(
            "[dirsize] read_dir ok: dirs={} files={} bytes={} errors={} path={}",
            progress.dirs_scanned,
            progress.files_scanned,
            progress.bytes_scanned,
            progress.errors,
            dir.display()
        );
        let _ = progress_tx.send(progress.touched(now(), None, Some("scan_entries")));

        for entry in rd {
            ops += 1;
            if ops % 64 == 0 && *cancel.borrow() {
                eprintln!(
                    "[dirsize] scan cancelled during entries: ops={ops} dirs={} files={} bytes={} errors={} dir={}",
                    progress.dirs_scanned,
                    progress.files_scanned,
                    progress.bytes_scanned,
                    progress.errors,
                    dir.display()
                );
                return None;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    progress.errors += 1;
                    eprintln!(
                        "[dirsize] entry error: dirs={} files={} bytes={} errors={} dir={}",
                        progress.dirs_scanned,
                        progress.files_scanned,
                        progress.bytes_scanned,
                        progress.errors,
                        dir.display()
                    );
                    continue;
                }
            };
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(m) => m,
                Err(_) => {
                    progress.errors += 1;
                    eprintln!(
                        "[dirsize] metadata error: dirs={} files={} bytes={} errors={} path={}",
                        progress.dirs_scanned,
                        progress.files_scanned,
                        progress.bytes_scanned,
                        progress.errors,
                        path.display()
                    );
                    continue;
                }
            };
            if meta.is_dir() {
                stack.push(path);
            } else if !meta.is_symlink() {
                progress.files_scanned += 1;
                total += meta.len();
                progress.bytes_scanned = total;
            }
            if ops % 256 == 0 {
                eprintln!(
                    "[dirsize] scan_entries progress: ops={ops} dirs={} files={} bytes={} errors={} dir={}",
                    progress.dirs_scanned,
                    progress.files_scanned,
                    progress.bytes_scanned,
                    progress.errors,
                    dir.display()
                );
                let _ = progress_tx.send(progress.touched(now(), None, Some("scan_entries")));
            }
        }
    }

    progress.bytes_scanned = total;
    eprintln!(
        "[dirsize] scan complete: dirs={} files={} bytes={} errors={} path={path}",
        progress.dirs_scanned, progress.files_scanned, progress.bytes_scanned, progress.errors
    );
    let _ = progress_tx.send(progress.touched(now(), None, Some("complete")));
    Some(DirSizeResult {
        size: total,
        progress,
    })
}
