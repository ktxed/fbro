use std::io::Read;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as SyncMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;
use tokio::sync::{mpsc, watch, Mutex, Semaphore};

use crate::vfs::format_mtime;
use crate::ws::protocol::{now, SearchProgressSchema, SearchResultSchema, ServerMessage};

const MAX_CONCURRENT: usize = 8;
const FLUSH_BATCH: usize = 50;
const FLUSH_INTERVAL: Duration = Duration::from_millis(200);
const CONTENT_READ_BUF: usize = 65536; // 64 KiB

// ─── Search parameters ────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub enum SearchDepth {
    Unlimited,
    Current,
    Depth(u8),
}

#[derive(Clone, Debug)]
pub enum MaskMode {
    Substring,
    Wildcard,
    Regex,
}

#[derive(Clone, Copy, Debug)]
pub enum SizeOp {
    Interval,
    Gt,
    Lt,
    Eq,
}

#[derive(Clone, Copy, Debug)]
pub enum TimeOp {
    Interval,
    Before,
    After,
    Relative,
}

#[derive(Clone, Debug)]
pub struct SearchParams {
    pub paths: Vec<String>,
    pub depth: SearchDepth,
    pub file_mask: Option<String>,
    pub mask_mode: MaskMode,
    /// Content query; always matched case-insensitively.
    pub content_query: Option<String>,
    /// File size filter (value and max are already in bytes).
    pub size_op: Option<SizeOp>,
    pub size_value: Option<u64>,
    pub size_max: Option<u64>,
    /// File datetime filter.
    pub time_op: Option<TimeOp>,
    /// For absolute ops: epoch seconds as string. For relative: compound string like "1h30m".
    pub time_value: Option<String>,
    pub time_to: Option<String>,
    /// Which timestamp to filter against: "modified" (default, most widely
    /// available) or "created" (birth-time; may be unavailable on some
    /// filesystems, in which case entries are skipped).
    pub time_date_field: Option<String>,
}

// ─── Search results ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SearchResultEntry {
    pub name: String,
    pub kind: String, // "file" | "directory"
    pub size: Option<u64>,
    pub mtime: Option<String>,
    pub parent_path: String,
    pub full_path: String,
    /// The real (non-shadow) source that owns this entry's filesystem path.
    pub source_id: String,
}

pub struct SearchResultStore {
    pub results: Vec<SearchResultEntry>,
    pub done: bool,
}

impl SearchResultStore {
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
            done: false,
        }
    }
}

// ─── Mask compilation ─────────────────────────────────────────────────────────

/// Returns a closure that tests an entry name against the compiled mask.
/// None mask = always match.
fn compile_mask(mask: Option<&str>, mode: &MaskMode) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
    let mask = match mask {
        None | Some("") => return Arc::new(|_| true),
        Some(m) => m.to_string(),
    };

    match mode {
        MaskMode::Substring => {
            let lower = mask.to_lowercase();
            Arc::new(move |name: &str| name.to_lowercase().contains(&lower))
        }
        MaskMode::Wildcard => {
            // Translate glob pattern to regex: * -> .*, ? -> .
            let escaped = regex::escape(&mask);
            let pattern = escaped.replace("\\*", ".*").replace("\\?", ".");
            match Regex::new(&format!("(?i)^{}$", pattern)) {
                Ok(re) => Arc::new(move |name: &str| re.is_match(name)),
                Err(_) => Arc::new(|_| false),
            }
        }
        MaskMode::Regex => match Regex::new(&format!("(?i){}", mask)) {
            Ok(re) => Arc::new(move |name: &str| re.is_match(name)),
            Err(_) => Arc::new(|_| false),
        },
    }
}

// ─── Extra filter helpers ──────────────────────────────────────────────────────

/// Parse a compound relative time string like "1h30m", "2d", "1w3h" into seconds.
/// Supports units: s, m, h, d, w.
fn parse_relative_time(input: &str) -> Option<u64> {
    let re = Regex::new(r"(\d+)\s*([smhdw])").ok()?;
    let mut total: u64 = 0;
    let mut found = false;
    for cap in re.captures_iter(input) {
        let num: u64 = cap.get(1)?.as_str().parse().ok()?;
        let unit = cap.get(2)?.as_str();
        let secs = match unit {
            "s" => num,
            "m" => num * 60,
            "h" => num * 3600,
            "d" => num * 86400,
            "w" => num * 604800,
            _ => continue,
        };
        total += secs;
        found = true;
    }
    if found {
        Some(total)
    } else {
        None
    }
}

/// Resolve time filter cutoffs into epoch seconds.
/// Returns (from_epoch, to_epoch) where each may be None if unbounded.
fn resolve_time_filter(
    time_op: Option<TimeOp>,
    time_value: &Option<String>,
    time_to: &Option<String>,
    now_epoch: i64,
) -> (Option<i64>, Option<i64>) {
    let op = match time_op {
        Some(op) => op,
        None => return (None, None),
    };

    match op {
        TimeOp::Relative => {
            let dur_secs = time_value
                .as_deref()
                .and_then(parse_relative_time)
                .unwrap_or(0) as i64;
            (Some(now_epoch.saturating_sub(dur_secs)), None)
        }
        TimeOp::Before => {
            let from = time_value.as_deref().and_then(|v| v.parse::<i64>().ok());
            (from, None)
        }
        TimeOp::After => {
            let from = time_value.as_deref().and_then(|v| v.parse::<i64>().ok());
            (from, None)
        }
        TimeOp::Interval => {
            let from = time_value.as_deref().and_then(|v| v.parse::<i64>().ok());
            let to = time_to.as_deref().and_then(|v| v.parse::<i64>().ok());
            (from, to)
        }
    }
}

// ─── Content matching ─────────────────────────────────────────────────────────

fn file_contains_query(path: &str, query_lower: &str, cancel: &watch::Receiver<bool>) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; CONTENT_READ_BUF];
    // We keep a carry-over of (query.len()-1) bytes between chunks to handle
    // matches that straddle a chunk boundary.
    let carry = query_lower.len().saturating_sub(1);
    let mut prev_tail = Vec::new();

    loop {
        if *cancel.borrow() {
            return false;
        }
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        // Build search window: prev_tail + current chunk, lowercased.
        let chunk_lower = {
            let mut combined = prev_tail.clone();
            combined.extend_from_slice(&buf[..n]);
            combined
                .iter()
                .map(|b| b.to_ascii_lowercase())
                .collect::<Vec<u8>>()
        };
        if let Ok(s) = std::str::from_utf8(&chunk_lower) {
            if s.contains(query_lower) {
                return true;
            }
        }
        // Save carry-over tail for next iteration
        let total = prev_tail.len() + n;
        if total >= carry {
            let tail_start = total - carry;
            let combined: Vec<u8> = prev_tail.iter().chain(buf[..n].iter()).cloned().collect();
            prev_tail = combined[tail_start..].to_vec();
        } else {
            prev_tail.extend_from_slice(&buf[..n]);
        }
    }
    false
}

// ─── Work item ────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct WorkItem {
    dir_path: String,
    depth: u32,
    /// Inherited source_id for this directory.
    source_id: String,
}

/// Activity is independent of matches: even a no-hit search reports the work
/// done by its blocking readers. Folder state is sampled atomically so a
/// concurrently discovered child cannot produce an inconsistent estimate.
#[derive(Default)]
struct SearchActivity {
    scanned: AtomicU64,
    folders: SyncMutex<FolderActivity>,
}

#[derive(Default)]
struct FolderActivity {
    discovered: u64,
    completed: u64,
    active: Vec<(usize, String)>,
}

impl SearchActivity {
    fn discover(&self) {
        self.folders.lock().unwrap().discovered += 1;
    }

    fn abandon_discovery(&self) {
        self.folders.lock().unwrap().discovered -= 1;
    }

    fn start(&self, worker: usize, path: &str) {
        self.folders
            .lock()
            .unwrap()
            .active
            .push((worker, path.to_string()));
    }

    fn finish(&self, worker: usize, completed: bool) {
        let mut folders = self.folders.lock().unwrap();
        folders.active.retain(|(id, _)| *id != worker);
        if completed {
            folders.completed += 1;
        }
    }

    fn snapshot(&self, matched: u64, completed: bool) -> SearchProgressSchema {
        let folders = self.folders.lock().unwrap();
        let estimated_percent = if completed {
            100
        } else if folders.discovered == 0 {
            0
        } else {
            (folders.completed.saturating_mul(100) / folders.discovered).min(99) as u8
        };
        SearchProgressSchema {
            scanned: self.scanned.load(Ordering::Relaxed),
            matched,
            current_path: folders
                .active
                .last()
                .map(|(_, path)| path.clone())
                .unwrap_or_default(),
            estimated_percent,
            new_results: Vec::new(),
        }
    }
}

// ─── Main worker ─────────────────────────────────────────────────────────────

/// Resolves the best-matching source_id for a filesystem path given a
/// sorted (longest-first) list of (root_path, source_id) pairs.
pub fn resolve_source_id_for_path_local(
    path_source_map: &[(String, String)],
    path: &str,
) -> String {
    for (root, source_id) in path_source_map {
        if path.starts_with(root.as_str()) {
            return source_id.clone();
        }
    }
    String::new()
}

pub async fn run_search(
    job_id: String,
    params: SearchParams,
    // Sorted longest-first: (root_path, source_id) pairs for resolving
    // which real source each result belongs to.
    path_source_map: Arc<Vec<(String, String)>>,
    store: Arc<Mutex<SearchResultStore>>,
    tx: mpsc::Sender<ServerMessage>,
    cancel_rx: watch::Receiver<bool>,
) {
    let mask_fn = compile_mask(params.file_mask.as_deref(), &params.mask_mode);
    let content_query_lower = params
        .content_query
        .as_deref()
        .map(|q| q.to_lowercase())
        .filter(|q| !q.is_empty());

    let max_depth: Option<u32> = match &params.depth {
        SearchDepth::Unlimited => None,
        SearchDepth::Current => Some(0),
        SearchDepth::Depth(d) => Some(*d as u32),
    };

    let now_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let (time_from, time_to) = resolve_time_filter(
        params.time_op,
        &params.time_value,
        &params.time_to,
        now_epoch,
    );

    // Workers both produce and consume directory work. A bounded frontier
    // deadlocks when every worker is waiting to enqueue children into a full
    // queue. Keep the frontier nonblocking; active I/O remains limited to eight
    // workers and the independent result collector provides backpressure.
    let (work_tx, work_rx) = async_channel::unbounded::<WorkItem>();
    let (result_tx, result_rx) = mpsc::channel::<SearchResultEntry>(1024);

    // Pending counter: tracks work items that have been queued but not yet
    // fully processed (including their subdirs). When this reaches zero and
    // the queue is empty the search is done.
    let pending = Arc::new(AtomicUsize::new(0));
    let activity = Arc::new(SearchActivity::default());

    // Seed the work queue with root paths
    for root in &params.paths {
        let source_id = resolve_source_id_for_path_local(&path_source_map, root);
        pending.fetch_add(1, Ordering::SeqCst);
        activity.discover();
        if work_tx
            .try_send(WorkItem {
                dir_path: root.clone(),
                depth: 0,
                source_id,
            })
            .is_err()
        {
            pending.fetch_sub(1, Ordering::SeqCst);
            activity.abandon_discovery();
        }
    }

    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT));
    let work_tx_arc = Arc::new(work_tx.clone());
    let mask_fn = Arc::clone(&mask_fn);
    let content_query_lower = Arc::new(content_query_lower);
    let cancel_rx_clone = cancel_rx.clone();

    // Spawn worker tasks
    let worker_count = MAX_CONCURRENT;
    let mut worker_handles = Vec::with_capacity(worker_count);

    for worker in 0..worker_count {
        let work_rx = work_rx.clone();
        let work_tx = Arc::clone(&work_tx_arc);
        let result_tx = result_tx.clone();
        let semaphore = Arc::clone(&semaphore);
        let mask_fn = Arc::clone(&mask_fn);
        let content_query_lower = Arc::clone(&content_query_lower);
        let path_source_map = Arc::clone(&path_source_map);
        let pending = Arc::clone(&pending);
        let activity = Arc::clone(&activity);
        let max_depth = max_depth;
        let cancel_rx = cancel_rx_clone.clone();
        let size_op = params.size_op;
        let size_value = params.size_value;
        let size_max = params.size_max;
        let time_op = params.time_op;
        let time_from = time_from;
        let time_to = time_to;
        let time_date_field = params.time_date_field.clone();

        let handle = tokio::spawn(async move {
            loop {
                if *cancel_rx.borrow() {
                    break;
                }

                // Try to receive a work item with a short timeout
                let item =
                    match tokio::time::timeout(Duration::from_millis(50), work_rx.recv()).await {
                        Ok(Ok(item)) => item,
                        Ok(Err(_)) => break, // channel closed
                        Err(_) => {
                            // Timeout: if no more work is pending, we're done
                            if pending.load(Ordering::SeqCst) == 0 {
                                break;
                            }
                            continue;
                        }
                    };

                if *cancel_rx.borrow() {
                    // Put pending count back — we're abandoning the item
                    pending.fetch_sub(1, Ordering::SeqCst);
                    break;
                }

                let _permit = semaphore.acquire().await.unwrap();

                let dir_path = item.dir_path.clone();
                let current_depth = item.depth;
                let item_source_id = item.source_id.clone();
                let mask_fn = Arc::clone(&mask_fn);
                let content_query_lower = Arc::clone(&content_query_lower);
                let cancel_check = cancel_rx.clone();
                let time_date_field = time_date_field.clone();

                activity.start(worker, &dir_path);
                let reader_activity = Arc::clone(&activity);
                let entries = tokio::task::spawn_blocking(move || {
                    read_dir_entries(
                        &dir_path,
                        &item_source_id,
                        mask_fn.as_ref(),
                        &content_query_lower,
                        &cancel_check,
                        size_op,
                        size_value,
                        size_max,
                        time_op,
                        time_from,
                        time_to,
                        time_date_field.as_deref(),
                        &reader_activity,
                    )
                })
                .await;

                let (matched, subdirs) = match entries {
                    Ok(Some(r)) => r,
                    _ => {
                        activity.finish(worker, !*cancel_rx.borrow());
                        pending.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                };

                for entry in matched {
                    if result_tx.send(entry).await.is_err() {
                        activity.finish(worker, false);
                        pending.fetch_sub(1, Ordering::SeqCst);
                        return;
                    }
                }

                // Push subdirs onto work queue if depth allows
                if max_depth.map_or(true, |max| current_depth < max) {
                    for subdir in subdirs {
                        if *cancel_rx.borrow() {
                            break;
                        }
                        let sub_source_id =
                            resolve_source_id_for_path_local(&path_source_map, &subdir);
                        let source_id = if sub_source_id.is_empty() {
                            item.source_id.clone()
                        } else {
                            sub_source_id
                        };
                        pending.fetch_add(1, Ordering::SeqCst);
                        activity.discover();
                        let queued = work_tx
                            .try_send(WorkItem {
                                dir_path: subdir,
                                depth: current_depth + 1,
                                source_id,
                            })
                            .is_ok();
                        if !queued {
                            pending.fetch_sub(1, Ordering::SeqCst);
                            activity.abandon_discovery();
                            break;
                        }
                    }
                }

                // This work item is fully processed
                activity.finish(worker, true);
                pending.fetch_sub(1, Ordering::SeqCst);
            }
        });

        worker_handles.push(handle);
    }

    // Drop our sender copies so the channel closes once all workers exit
    drop(work_tx);
    drop(result_tx);

    let matched_count = collect_results(&job_id, result_rx, &activity, &store, &tx).await;
    for h in worker_handles {
        let _ = h.await;
    }
    let cancelled = *cancel_rx.borrow();

    // Mark store done
    {
        let mut s = store.lock().await;
        s.done = true;
    }

    let event = if cancelled { "Cancelled" } else { "Completed" };
    let _ = tx
        .send(ServerMessage::JobEvent {
            timestamp: now(),
            job_id,
            event: event.to_string(),
            files: None,
            progress: None,
            search_progress: Some(activity.snapshot(matched_count, !cancelled)),
            decision: None,
            error: None,
            result_path: None,
            result_panel_id: None,
        })
        .await;
}

async fn collect_results(
    job_id: &str,
    mut result_rx: mpsc::Receiver<SearchResultEntry>,
    activity: &SearchActivity,
    store: &Arc<Mutex<SearchResultStore>>,
    tx: &mpsc::Sender<ServerMessage>,
) -> u64 {
    // The timer runs independently of result arrivals, including while a
    // worker is reading a large directory or file that has no matches.
    let mut batch: Vec<SearchResultEntry> = Vec::new();
    let mut matched_count: u64 = 0;
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    flush_batch(job_id, &mut batch, activity, matched_count, store, tx).await;
    // interval's first tick is immediate; the initial snapshot was sent above.
    interval.tick().await;

    loop {
        tokio::select! {
            entry = result_rx.recv() => {
                let Some(entry) = entry else { break };
                matched_count += 1;
                batch.push(entry);
                if batch.len() >= FLUSH_BATCH {
                    flush_batch(job_id, &mut batch, activity, matched_count, store, tx).await;
                }
            }
            _ = interval.tick() => {
                flush_batch(job_id, &mut batch, activity, matched_count, store, tx).await;
            }
        }
        // Keep draining after cancellation so workers cannot get stuck sending
        // results to a full channel while we wait for them to exit.
    }

    // Always send final counters, even if there were no matching entries.
    flush_batch(job_id, &mut batch, activity, matched_count, store, tx).await;
    matched_count
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Read a single directory and return (matched entries, subdir paths).
/// Returns None if cancelled or unreadable.
fn read_dir_entries(
    dir_path: &str,
    source_id: &str,
    mask_fn: &dyn Fn(&str) -> bool,
    content_query_lower: &Option<String>,
    cancel: &watch::Receiver<bool>,
    size_op: Option<SizeOp>,
    size_value: Option<u64>,
    size_max: Option<u64>,
    time_op: Option<TimeOp>,
    time_from: Option<i64>,
    time_to: Option<i64>,
    time_date_field: Option<&str>,
    activity: &SearchActivity,
) -> Option<(Vec<SearchResultEntry>, Vec<String>)> {
    if *cancel.borrow() {
        return None;
    }

    let rd = std::fs::read_dir(dir_path).ok()?;
    let mut matched = Vec::new();
    let mut subdirs = Vec::new();

    for entry in rd.flatten() {
        if *cancel.borrow() {
            return None;
        }
        activity.scanned.fetch_add(1, Ordering::Relaxed);
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let full_path = entry.path().to_string_lossy().into_owned();
        let is_dir = meta.is_dir();

        if is_dir {
            subdirs.push(full_path.clone());
        }

        // Name filter
        if !mask_fn(&name) {
            continue;
        }

        // Content filter (only for files)
        if !is_dir {
            if let Some(ref query) = content_query_lower {
                // Pipes, sockets, devices, and symlinks are not regular content
                // files. Opening them may block waiting for an external writer.
                if !meta.is_file() {
                    continue;
                }
                if !file_contains_query(&full_path, query, cancel) {
                    continue;
                }
            }
        }

        let kind = if is_dir {
            "directory".to_string()
        } else {
            "file".to_string()
        };
        let size = if is_dir { None } else { Some(meta.len()) };
        let mtime = meta.modified().ok().and_then(format_mtime);
        let btime = meta.created().ok().and_then(format_mtime);

        // Size filter: directories excluded when active
        if let Some(op) = size_op {
            if is_dir {
                continue;
            }
            let entry_size = meta.len();
            let val = size_value.unwrap_or(0);
            let pass = match op {
                SizeOp::Gt => entry_size > val,
                SizeOp::Lt => entry_size < val,
                SizeOp::Eq => entry_size == val,
                SizeOp::Interval => {
                    entry_size >= val && size_max.map_or(true, |max| entry_size <= max)
                }
            };
            if !pass {
                continue;
            }
        }

        // Time filter
        if let Some(op) = time_op {
            let ts_opt = if time_date_field == Some("created") {
                &btime
            } else {
                &mtime
            };
            let Some(ref ts) = ts_opt else {
                continue;
            };
            let Ok(entry_epoch) = ts.parse::<i64>() else {
                continue;
            };
            let pass = match op {
                TimeOp::Before => time_from.map_or(true, |from| entry_epoch < from),
                TimeOp::After => time_from.map_or(false, |from| entry_epoch > from),
                TimeOp::Relative => time_from.map_or(false, |from| entry_epoch >= from),
                TimeOp::Interval => {
                    let from_ok = time_from.map_or(true, |from| entry_epoch >= from);
                    let to_ok = time_to.map_or(true, |to| entry_epoch <= to);
                    from_ok && to_ok
                }
            };
            if !pass {
                continue;
            }
        }

        matched.push(SearchResultEntry {
            name,
            kind,
            size,
            mtime,
            parent_path: dir_path.to_string(),
            full_path,
            source_id: source_id.to_string(),
        });
    }

    // Cancellation can arrive during the last entry's content read, with no
    // next iteration to observe it. That folder was abandoned, not completed.
    if *cancel.borrow() {
        None
    } else {
        Some((matched, subdirs))
    }
}

async fn flush_batch(
    job_id: &str,
    batch: &mut Vec<SearchResultEntry>,
    activity: &SearchActivity,
    matched: u64,
    store: &Arc<Mutex<SearchResultStore>>,
    tx: &mpsc::Sender<ServerMessage>,
) {
    let new_results: Vec<SearchResultSchema> = batch
        .iter()
        .map(|e| SearchResultSchema {
            name: e.name.clone(),
            kind: e.kind.clone(),
            size: e.size,
            mtime: e.mtime.clone(),
            parent_path: e.parent_path.clone(),
            full_path: e.full_path.clone(),
            source_id: e.source_id.clone(),
        })
        .collect();

    {
        let mut s = store.lock().await;
        s.results.append(batch);
    }

    let _ = tx
        .send(ServerMessage::JobEvent {
            timestamp: now(),
            job_id: job_id.to_string(),
            event: "Progress".to_string(),
            files: None,
            progress: None,
            search_progress: Some(SearchProgressSchema {
                new_results,
                ..activity.snapshot(matched, false)
            }),
            decision: None,
            error: None,
            result_path: None,
            result_panel_id: None,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "fbro-search-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn params(root: &Path, depth: SearchDepth) -> SearchParams {
        SearchParams {
            paths: vec![root.to_string_lossy().into_owned()],
            depth,
            file_mask: Some("*.txt".into()),
            mask_mode: MaskMode::Wildcard,
            content_query: None,
            size_op: None,
            size_value: None,
            size_max: None,
            time_op: None,
            time_value: None,
            time_to: None,
            time_date_field: None,
        }
    }

    async fn search_messages(params: SearchParams, cancelled: bool) -> (Vec<ServerMessage>, usize) {
        let (tx, mut rx) = mpsc::channel(256);
        let (cancel_tx, cancel_rx) = watch::channel(cancelled);
        let store = Arc::new(Mutex::new(SearchResultStore::new()));
        let worker_store = Arc::clone(&store);
        let worker = tokio::spawn(run_search(
            "search-test".into(),
            params,
            Arc::new(vec![]),
            worker_store,
            tx,
            cancel_rx,
        ));
        let mut messages = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(message) = rx.recv().await {
                messages.push(message);
            }
            worker.await.unwrap();
        })
        .await
        .expect("search did not terminate");
        drop(cancel_tx);
        let results = store.lock().await;
        assert!(results.done);
        (messages, results.results.len())
    }

    fn terminal(messages: &[ServerMessage]) -> (&str, &SearchProgressSchema) {
        match messages.last().unwrap() {
            ServerMessage::JobEvent {
                event,
                search_progress: Some(progress),
                ..
            } => (event, progress),
            message => panic!("missing final search activity: {message:?}"),
        }
    }

    #[test]
    fn folder_estimate_revises_and_current_folder_remains_active() {
        let activity = SearchActivity::default();
        activity.discover();
        activity.start(0, "first");
        activity.discover();
        activity.start(1, "second");
        assert_eq!(activity.snapshot(0, false).current_path, "second");
        activity.finish(1, true);
        let snapshot = activity.snapshot(0, false);
        assert_eq!(snapshot.current_path, "first");
        assert_eq!(snapshot.estimated_percent, 50);
        activity.discover();
        assert_eq!(activity.snapshot(0, false).estimated_percent, 33);
        activity.abandon_discovery();
        activity.finish(0, true);
        assert!(activity.snapshot(0, false).current_path.is_empty());
        assert_eq!(activity.snapshot(0, false).estimated_percent, 99);
        assert_eq!(activity.snapshot(0, true).estimated_percent, 100);
        assert_eq!(
            SearchActivity::default()
                .snapshot(0, false)
                .estimated_percent,
            0
        );
    }

    #[test]
    fn cancelled_folder_does_not_advance_the_estimate() {
        let activity = SearchActivity::default();
        activity.discover();
        activity.start(0, "partially-read");
        assert_eq!(activity.snapshot(0, false).estimated_percent, 0);
        activity.finish(0, false);
        let cancelled = activity.snapshot(0, false);
        assert_eq!(cancelled.estimated_percent, 0);
        assert!(cancelled.current_path.is_empty());
    }

    #[tokio::test]
    async fn activity_ticks_without_results_while_a_folder_is_active() {
        let activity = Arc::new(SearchActivity::default());
        activity.discover();
        activity.start(0, "slow-folder");
        activity.scanned.store(1, Ordering::Relaxed);
        let worker_activity = Arc::clone(&activity);
        let store = Arc::new(Mutex::new(SearchResultStore::new()));
        // Keep the results channel open without sending a match, as happens
        // while a blocking worker is reading a large file or directory.
        let (results_tx, results_rx) = mpsc::channel(1);
        let (tx, mut rx) = mpsc::channel(8);
        let collector = tokio::spawn(async move {
            collect_results("slow-search", results_rx, &worker_activity, &store, &tx).await
        });
        for _ in 0..3 {
            let message = tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap();
            match message {
                ServerMessage::JobEvent {
                    event,
                    search_progress: Some(progress),
                    ..
                } => {
                    assert_eq!(event, "Progress");
                    assert_eq!(progress.current_path, "slow-folder");
                    assert_eq!(progress.scanned, 1);
                    assert_eq!(progress.matched, 0);
                    assert_eq!(progress.estimated_percent, 0);
                    assert!(progress.new_results.is_empty());
                }
                message => panic!("unexpected activity: {message:?}"),
            }
        }
        activity.finish(0, true);
        drop(results_tx);
        assert_eq!(collector.await.unwrap(), 0);
    }

    #[tokio::test]
    async fn inspected_counts_include_nonmatches_and_depth_limits() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join("match.txt"), "hit").unwrap();
        std::fs::write(dir.0.join("other.bin"), "miss").unwrap();
        std::fs::create_dir(dir.0.join("child")).unwrap();
        std::fs::write(dir.0.join("child/nested.txt"), "hit").unwrap();
        for (depth, scanned, matched) in [
            (SearchDepth::Current, 3, 1),
            (SearchDepth::Depth(1), 4, 2),
            (SearchDepth::Unlimited, 4, 2),
        ] {
            let (messages, results) = search_messages(params(&dir.0, depth), false).await;
            let (event, progress) = terminal(&messages);
            assert_eq!(event, "Completed");
            assert_eq!(progress.scanned, scanned);
            assert_eq!(progress.matched, matched);
            assert_eq!(results as u64, matched);
            assert_eq!(progress.estimated_percent, 100);
            assert!(progress.current_path.is_empty());
            for message in &messages[..messages.len() - 1] {
                if let ServerMessage::JobEvent {
                    search_progress: Some(progress),
                    ..
                } = message
                {
                    assert!(progress.estimated_percent < 100);
                }
            }
        }
    }

    #[tokio::test]
    async fn no_hits_multiple_roots_and_unreadable_folder_have_final_counts() {
        let first = TempDir::new();
        let second = TempDir::new();
        std::fs::write(first.0.join("one.bin"), "miss").unwrap();
        std::fs::write(second.0.join("two.bin"), "miss").unwrap();
        let mut search = params(&first.0, SearchDepth::Unlimited);
        search.paths.push(second.0.to_string_lossy().into_owned());
        search
            .paths
            .push(first.0.join("missing").to_string_lossy().into_owned());
        let (messages, results) = search_messages(search, false).await;
        let (event, progress) = terminal(&messages);
        assert_eq!(event, "Completed");
        assert_eq!(progress.scanned, 2);
        assert_eq!(progress.matched, 0);
        assert_eq!(results, 0);
        assert_eq!(progress.estimated_percent, 100);
    }

    #[tokio::test]
    async fn directory_backlog_over_the_old_capacity_does_not_drop_work() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join("marker.txt"), "hit").unwrap();
        let mut search = params(&dir.0, SearchDepth::Current);
        // Each requested root must get processed, even while every worker is
        // busy. Repeated roots keep this saturation fixture small on disk.
        search.paths = vec![dir.0.to_string_lossy().into_owned(); 4097];
        let (messages, results) = search_messages(search, false).await;
        let (event, progress) = terminal(&messages);
        assert_eq!(event, "Completed");
        assert_eq!(progress.scanned, 4097);
        assert_eq!(progress.matched, 4097);
        assert_eq!(results, 4097);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn inaccessible_directory_is_skipped_and_accessible_sibling_is_searched() {
        use std::os::unix::fs::PermissionsExt;

        // Root can bypass mode bits; this fixture exercises real denial under
        // the ordinary desktop user's identity.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        struct RestorePermissions(PathBuf);
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
            }
        }
        let dir = TempDir::new();
        let blocked = dir.0.join("blocked");
        let accessible = dir.0.join("accessible");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::create_dir(&accessible).unwrap();
        std::fs::write(blocked.join("hidden.txt"), "hidden").unwrap();
        std::fs::write(accessible.join("visible.txt"), "hit").unwrap();
        let _restore = RestorePermissions(blocked.clone());
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0)).unwrap();
        assert!(std::fs::read_dir(&blocked).is_err());
        let (messages, results) =
            search_messages(params(&dir.0, SearchDepth::Unlimited), false).await;
        let (event, progress) = terminal(&messages);
        assert_eq!(event, "Completed");
        assert_eq!(progress.scanned, 3);
        assert_eq!(progress.matched, 1);
        assert_eq!(results, 1);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn content_search_skips_a_fifo_without_waiting_for_a_writer() {
        let dir = TempDir::new();
        let path = std::ffi::CString::new(dir.0.join("pipe.txt").to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        std::fs::write(dir.0.join("regular.txt"), "needle").unwrap();
        let mut search = params(&dir.0, SearchDepth::Unlimited);
        search.content_query = Some("needle".into());
        let (messages, results) = search_messages(search, false).await;
        let (event, progress) = terminal(&messages);
        assert_eq!(event, "Completed");
        assert_eq!(progress.scanned, 2);
        assert_eq!(progress.matched, 1);
        assert_eq!(results, 1);
    }

    #[tokio::test]
    async fn cancelled_and_empty_searches_report_the_correct_terminal_estimate() {
        let dir = TempDir::new();
        let (messages, _) = search_messages(params(&dir.0, SearchDepth::Current), false).await;
        assert_eq!(terminal(&messages).1.estimated_percent, 100);
        let (messages, _) = search_messages(params(&dir.0, SearchDepth::Current), true).await;
        let (event, progress) = terminal(&messages);
        assert_eq!(event, "Cancelled");
        assert_eq!(progress.scanned, 0);
        assert_eq!(progress.estimated_percent, 0);
    }

    #[test]
    fn content_reader_observes_cancellation() {
        let dir = TempDir::new();
        let path = dir.0.join("content.txt");
        std::fs::write(&path, "needle").unwrap();
        let (tx, rx) = watch::channel(false);
        assert!(file_contains_query(path.to_str().unwrap(), "needle", &rx));
        tx.send(true).unwrap();
        assert!(!file_contains_query(path.to_str().unwrap(), "needle", &rx));
    }

    #[test]
    fn cancellation_during_the_last_entry_abandons_the_folder() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join("last.txt"), "miss").unwrap();
        let (tx, rx) = watch::channel(false);
        let activity = SearchActivity::default();
        let result = read_dir_entries(
            dir.0.to_str().unwrap(),
            "",
            &|_| {
                tx.send(true).unwrap();
                false
            },
            &None,
            &rx,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            &activity,
        );
        assert!(result.is_none());
        assert_eq!(activity.scanned.load(Ordering::Relaxed), 1);
    }
}
