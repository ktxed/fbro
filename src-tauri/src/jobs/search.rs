use std::io::Read;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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

fn file_contains_query(path: &str, query_lower: &str) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = vec![0u8; CONTENT_READ_BUF];
    // We keep a carry-over of (query.len()-1) bytes between chunks to handle
    // matches that straddle a chunk boundary.
    let carry = query_lower.len().saturating_sub(1);
    let mut prev_tail = Vec::new();

    loop {
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

    // Work queue: (dir_path, depth)
    let work_tx_cap = 4096;
    let (work_tx, work_rx) = async_channel::bounded::<WorkItem>(work_tx_cap);
    let (result_tx, mut result_rx) = mpsc::channel::<SearchResultEntry>(1024);

    // Pending counter: tracks work items that have been queued but not yet
    // fully processed (including their subdirs). When this reaches zero and
    // the queue is empty the search is done.
    let pending = Arc::new(AtomicUsize::new(0));

    // Seed the work queue with root paths
    for root in &params.paths {
        let source_id = resolve_source_id_for_path_local(&path_source_map, root);
        pending.fetch_add(1, Ordering::SeqCst);
        let _ = work_tx.try_send(WorkItem {
            dir_path: root.clone(),
            depth: 0,
            source_id,
        });
    }

    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT));
    let work_tx_arc = Arc::new(work_tx.clone());
    let mask_fn = Arc::clone(&mask_fn);
    let content_query_lower = Arc::new(content_query_lower);
    let cancel_rx_clone = cancel_rx.clone();

    // Spawn worker tasks
    let worker_count = MAX_CONCURRENT;
    let mut worker_handles = Vec::with_capacity(worker_count);

    for _ in 0..worker_count {
        let work_rx = work_rx.clone();
        let work_tx = Arc::clone(&work_tx_arc);
        let result_tx = result_tx.clone();
        let semaphore = Arc::clone(&semaphore);
        let mask_fn = Arc::clone(&mask_fn);
        let content_query_lower = Arc::clone(&content_query_lower);
        let path_source_map = Arc::clone(&path_source_map);
        let pending = Arc::clone(&pending);
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
                    )
                })
                .await;

                let (matched, subdirs) = match entries {
                    Ok(Some(r)) => r,
                    _ => {
                        pending.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                };

                for entry in matched {
                    if result_tx.send(entry).await.is_err() {
                        pending.fetch_sub(1, Ordering::SeqCst);
                        return;
                    }
                }

                // Push subdirs onto work queue if depth allows
                if max_depth.map_or(true, |max| current_depth < max) {
                    for subdir in subdirs {
                        let sub_source_id =
                            resolve_source_id_for_path_local(&path_source_map, &subdir);
                        let source_id = if sub_source_id.is_empty() {
                            item.source_id.clone()
                        } else {
                            sub_source_id
                        };
                        pending.fetch_add(1, Ordering::SeqCst);
                        let _ = work_tx
                            .send(WorkItem {
                                dir_path: subdir,
                                depth: current_depth + 1,
                                source_id,
                            })
                            .await;
                    }
                }

                // This work item is fully processed
                pending.fetch_sub(1, Ordering::SeqCst);
            }
        });

        worker_handles.push(handle);
    }

    // Drop our sender copies so the channel closes once all workers exit
    drop(work_tx);
    drop(result_tx);

    // Collector: accumulate results and flush to FE periodically
    let mut batch: Vec<SearchResultEntry> = Vec::new();
    let mut scanned: u64 = 0;
    let mut matched_count: u64 = 0;
    let mut last_flush = Instant::now();
    let mut current_path = String::new();
    let cancelled;

    loop {
        // Check cancel
        if *cancel_rx.borrow() {
            cancelled = true;
            break;
        }

        // Check if all workers are done AND result channel is empty
        // We detect this by try_recv exhaustion after workers finish
        match tokio::time::timeout(Duration::from_millis(50), result_rx.recv()).await {
            Ok(Some(entry)) => {
                scanned += 1;
                matched_count += 1;
                current_path = entry.parent_path.clone();
                batch.push(entry);

                let should_flush =
                    batch.len() >= FLUSH_BATCH || last_flush.elapsed() >= FLUSH_INTERVAL;
                if should_flush {
                    flush_batch(
                        &job_id,
                        &mut batch,
                        scanned,
                        matched_count,
                        &current_path,
                        &store,
                        &tx,
                    )
                    .await;
                    last_flush = Instant::now();
                }
            }
            Ok(None) => {
                // Channel closed — all workers done
                cancelled = false;
                break;
            }
            Err(_) => {
                // Timeout — flush partial batch if pending
                if !batch.is_empty() && last_flush.elapsed() >= FLUSH_INTERVAL {
                    flush_batch(
                        &job_id,
                        &mut batch,
                        scanned,
                        matched_count,
                        &current_path,
                        &store,
                        &tx,
                    )
                    .await;
                    last_flush = Instant::now();
                }
                continue;
            }
        }
    }

    // Wait for all workers to finish
    for h in worker_handles {
        let _ = h.await;
    }

    // Final flush
    if !batch.is_empty() {
        flush_batch(
            &job_id,
            &mut batch,
            scanned,
            matched_count,
            &current_path,
            &store,
            &tx,
        )
        .await;
    }

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
            search_progress: None,
            decision: None,
            error: None,
            result_path: None,
            result_panel_id: None,
        })
        .await;
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
                if !file_contains_query(&full_path, query) {
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

    Some((matched, subdirs))
}

async fn flush_batch(
    job_id: &str,
    batch: &mut Vec<SearchResultEntry>,
    scanned: u64,
    matched: u64,
    current_path: &str,
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
                scanned,
                matched,
                current_path: current_path.to_string(),
                new_results,
            }),
            decision: None,
            error: None,
            result_path: None,
            result_panel_id: None,
        })
        .await;
}
