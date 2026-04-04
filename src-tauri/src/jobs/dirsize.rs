use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch, Mutex, Semaphore};

use crate::state::SharedState;
use crate::ws::protocol::{now, ServerMessage};

const MAX_CONCURRENT: usize = 4;
const CACHE_TTL: Duration = Duration::from_secs(60);

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
    status: DirSizeTaskStatus,
    cancel_tx: watch::Sender<bool>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirSizeTaskSnapshot {
    pub panel_id: String,
    pub entry_name: String,
    pub full_path: String,
    pub enqueued_at: u64,
    pub status: String,
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
    semaphore: Arc<Semaphore>,
    tasks: Arc<Mutex<HashMap<(String, String), TrackedTask>>>,
    cache: Arc<Mutex<DirSizeCache>>,
    next_task_id: AtomicU64,
}

impl DirSizeQueue {
    pub fn new() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT)),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            cache: Arc::new(Mutex::new(DirSizeCache::new())),
            next_task_id: AtomicU64::new(1),
        }
    }

    pub async fn snapshot(&self) -> Vec<DirSizeTaskSnapshot> {
        let tasks = self.tasks.lock().await;
        let mut snapshot: Vec<DirSizeTaskSnapshot> = tasks
            .iter()
            .map(|((panel_id, entry_name), task)| DirSizeTaskSnapshot {
                panel_id: panel_id.clone(),
                entry_name: entry_name.clone(),
                full_path: task.full_path.clone(),
                enqueued_at: task.enqueued_at,
                status: task.status.as_str().to_string(),
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

        // Cancel any existing task for this entry and replace it
        {
            let mut tasks = self.tasks.lock().await;
            if let Some(old) = tasks.insert(
                (panel_id.clone(), entry_name.clone()),
                TrackedTask {
                    task_id,
                    full_path: full_path.clone(),
                    enqueued_at: now(),
                    status: DirSizeTaskStatus::Queued,
                    cancel_tx: cancel_tx.clone(),
                },
            ) {
                let _ = old.cancel_tx.send(true);
            }
        }
        emit_queue_patch(&self.tasks, &tx).await;

        // Mark as calculating in panel state immediately
        {
            let mut s = state.lock().await;
            if let Some(panel) = s.panels.get_mut(&panel_id) {
                panel.calculating_size.insert(entry_name.clone());
                panel.version += 1;
                let calc = calculating_vec(&panel.calculating_size);
                let v = panel.version;
                let _ = tx.try_send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id.clone()),
                    version: v,
                    patch: serde_json::json!({ "calculatingSize": calc }),
                });
            } else {
                {
                    let mut tasks = self.tasks.lock().await;
                    tasks.remove(&(panel_id.clone(), entry_name.clone()));
                }
                emit_queue_patch(&self.tasks, &tx).await;
                return;
            }
        }

        eprintln!("[dirsize] enqueued: panel={panel_id} entry={entry_name} path={full_path}");

        let semaphore = Arc::clone(&self.semaphore);
        let tasks = Arc::clone(&self.tasks);
        let cache = Arc::clone(&self.cache);

        tokio::spawn(async move {
            // Wait for an available computation slot
            let _permit = semaphore.acquire().await.unwrap();

            // Check if cancelled while waiting in queue
            if *cancel_rx.borrow() {
                remove_task_if_matches(&tasks, &panel_id, &entry_name, task_id).await;
                emit_queue_patch(&tasks, &tx).await;
                patch_remove_calculating(&state, &tx, &panel_id, &entry_name).await;
                return;
            }

            {
                let mut tasks_guard = tasks.lock().await;
                if let Some(task) = tasks_guard.get_mut(&(panel_id.clone(), entry_name.clone())) {
                    if task.task_id == task_id {
                        task.status = DirSizeTaskStatus::Running;
                    }
                }
            }
            emit_queue_patch(&tasks, &tx).await;

            let path = full_path.clone();
            let cancel_clone = cancel_rx.clone();
            let result =
                tokio::task::spawn_blocking(move || calculate_dir_size(&path, &cancel_clone)).await;

            remove_task_if_matches(&tasks, &panel_id, &entry_name, task_id).await;
            emit_queue_patch(&tasks, &tx).await;

            match result {
                Ok(Some(size)) => {
                    eprintln!(
                        "[dirsize] finished: panel={panel_id} entry={entry_name} size={size}"
                    );
                    let mut s = state.lock().await;
                    if let Some(panel) = s.panels.get_mut(&panel_id) {
                        if let Some(e) = panel.entries.iter_mut().find(|e| e.name == entry_name) {
                            e.size = Some(size);
                        }
                        panel.calculating_size.remove(&entry_name);
                        panel.version += 1;
                        let calc = calculating_vec(&panel.calculating_size);
                        let _ = tx.try_send(ServerMessage::StatePatch {
                            timestamp: now(),
                            domain: "panels".to_string(),
                            entity_id: Some(panel_id),
                            version: panel.version,
                            patch: serde_json::json!({
                                "calculatingSize": calc,
                                "updateEntrySize": { "name": entry_name, "size": size }
                            }),
                        });
                    }
                    // Store in cache after releasing the state lock
                    drop(s);
                    cache.lock().await.insert(full_path, size);
                }
                Ok(None) => {
                    eprintln!("[dirsize] cancelled: panel={panel_id} entry={entry_name}");
                    patch_remove_calculating(&state, &tx, &panel_id, &entry_name).await;
                }
                Err(_) => {
                    eprintln!("[dirsize] error: panel={panel_id} entry={entry_name}");
                    patch_remove_calculating(&state, &tx, &panel_id, &entry_name).await;
                }
            }
        });
    }

    /// Signal an active calculation to stop. The state update happens
    /// immediately (called from the intent handler) so the FE gets instant
    /// feedback; the background task will clean up when it detects the signal.
    pub async fn cancel(
        &self,
        panel_id: &str,
        entry_name: &str,
        tx: &mpsc::Sender<ServerMessage>,
    ) {
        let removed = {
            let mut tasks = self.tasks.lock().await;
            tasks.remove(&(panel_id.to_string(), entry_name.to_string()))
        };
        if let Some(task) = removed {
            let _ = task.cancel_tx.send(true);
            emit_queue_patch(&self.tasks, tx).await;
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn remove_task_if_matches(
    tasks: &Arc<Mutex<HashMap<(String, String), TrackedTask>>>,
    panel_id: &str,
    entry_name: &str,
    task_id: u64,
) {
    let mut tasks = tasks.lock().await;
    let key = (panel_id.to_string(), entry_name.to_string());
    let should_remove = tasks
        .get(&key)
        .map(|task| task.task_id == task_id)
        .unwrap_or(false);
    if should_remove {
        tasks.remove(&key);
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
                panel_id: panel_id.clone(),
                entry_name: entry_name.clone(),
                full_path: task.full_path.clone(),
                enqueued_at: task.enqueued_at,
                status: task.status.as_str().to_string(),
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
    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "dirSizeQueue".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "tasks": snapshot }),
        })
        .await;
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
    let mut s = state.lock().await;
    if let Some(panel) = s.panels.get_mut(panel_id) {
        panel.calculating_size.remove(entry_name);
        panel.version += 1;
        let calc = calculating_vec(&panel.calculating_size);
        let _ = tx.try_send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id.to_string()),
            version: panel.version,
            patch: serde_json::json!({ "calculatingSize": calc }),
        });
    }
}

// ── Blocking recursive traversal ─────────────────────────────────────────────

/// Returns the total byte size of all files under `path`, or `None` if
/// cancelled. Symlinks are not followed to avoid cycles.
fn calculate_dir_size(path: &str, cancel: &watch::Receiver<bool>) -> Option<u64> {
    let mut total: u64 = 0;
    let mut stack = vec![PathBuf::from(path)];
    let mut iters: u32 = 0;

    while let Some(dir) = stack.pop() {
        iters += 1;
        if iters % 256 == 0 && *cancel.borrow() {
            return None;
        }

        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };

        for entry in rd.flatten() {
            let meta = match std::fs::symlink_metadata(entry.path()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if !meta.is_symlink() {
                total += meta.len();
            }
        }
    }

    Some(total)
}
