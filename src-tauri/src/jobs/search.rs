use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use regex::Regex;
use tokio::sync::{mpsc, watch, Mutex, Semaphore};

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

#[derive(Clone, Debug)]
pub struct SearchParams {
    pub paths: Vec<String>,
    pub depth: SearchDepth,
    pub file_mask: Option<String>,
    pub mask_mode: MaskMode,
    /// Content query; always matched case-insensitively.
    pub content_query: Option<String>,
}

// ─── Search results ────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SearchResultEntry {
    pub name: String,
    pub kind: String, // "file" | "directory"
    pub size: Option<u64>,
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
            let combined: Vec<u8> = prev_tail
                .iter()
                .chain(buf[..n].iter())
                .cloned()
                .collect();
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

    // Work queue: (dir_path, depth)
    let work_tx_cap = 4096;
    let (work_tx, work_rx) = async_channel::bounded::<WorkItem>(work_tx_cap);
    let (result_tx, mut result_rx) = mpsc::channel::<SearchResultEntry>(1024);

    // Seed the work queue with root paths
    for root in &params.paths {
        let source_id = resolve_source_id_for_path_local(&path_source_map, root);
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
        let max_depth = max_depth;
        let cancel_rx = cancel_rx_clone.clone();

        let handle = tokio::spawn(async move {
            loop {
                if *cancel_rx.borrow() {
                    break;
                }

                // Try to receive a work item with a short timeout
                let item = match tokio::time::timeout(
                    Duration::from_millis(100),
                    work_rx.recv(),
                )
                .await
                {
                    Ok(Ok(item)) => item,
                    Ok(Err(_)) => break, // channel closed
                    Err(_) => continue,  // timeout, loop and check cancel
                };

                if *cancel_rx.borrow() {
                    break;
                }

                let _permit = semaphore.acquire().await.unwrap();

                let dir_path = item.dir_path.clone();
                let current_depth = item.depth;
                let item_source_id = item.source_id.clone();
                let mask_fn = Arc::clone(&mask_fn);
                let content_query_lower = Arc::clone(&content_query_lower);
                let cancel_check = cancel_rx.clone();

                let entries = tokio::task::spawn_blocking(move || {
                    read_dir_entries(
                        &dir_path,
                        &item_source_id,
                        mask_fn.as_ref(),
                        &content_query_lower,
                        &cancel_check,
                    )
                })
                .await;

                let (matched, subdirs) = match entries {
                    Ok(Some(r)) => r,
                    _ => continue,
                };

                for entry in matched {
                    if result_tx.send(entry).await.is_err() {
                        return;
                    }
                }

                // Push subdirs onto work queue if depth allows
                if max_depth.map_or(true, |max| current_depth < max) {
                    for subdir in subdirs {
                        // Inherit source_id from parent; re-resolve if needed
                        let sub_source_id =
                            resolve_source_id_for_path_local(&path_source_map, &subdir);
                        let source_id = if sub_source_id.is_empty() {
                            item.source_id.clone()
                        } else {
                            sub_source_id
                        };
                        let _ = work_tx
                            .send(WorkItem {
                                dir_path: subdir,
                                depth: current_depth + 1,
                                source_id,
                            })
                            .await;
                    }
                }
            }
        });

        worker_handles.push(handle);
    }

    // Drop our sender copies so the channel closes when workers are done
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

        matched.push(SearchResultEntry {
            name,
            kind,
            size,
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
