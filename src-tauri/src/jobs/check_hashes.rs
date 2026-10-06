use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::{mpsc, watch};

use crate::bro::BroInputItem;
use crate::jobs::progress::ProgressReporter;
use crate::vfs::{VfsDriver, VfsError, VfsKind};
use crate::ws::protocol::{
    now, HashCheckBroFileSchema, HashCheckBroResultSchema, HashCheckStatus, ServerMessage, SortSpec,
};

use crate::jobs::hash::hash_reader;
#[cfg(test)]
use crate::jobs::hash::HASH_BUFFER_SIZE;
const SIDECAR_LIMIT: u64 = 64 * 1024;

pub struct HashCheckOptions {
    pub driver: Arc<dyn VfsDriver>,
    pub folder: String,
    pub inputs: Vec<BroInputItem>,
    pub save_result: bool,
}

pub async fn run_check_hashes(
    job_id: String,
    options: HashCheckOptions,
    tx: mpsc::Sender<ServerMessage>,
    mut cancel_rx: watch::Receiver<bool>,
) {
    let cancel = Arc::new(AtomicBool::new(*cancel_rx.borrow()));
    let watched_cancel = cancel.clone();
    let watcher = tokio::spawn(async move {
        while cancel_rx.changed().await.is_ok() {
            if *cancel_rx.borrow() {
                watched_cancel.store(true, Ordering::SeqCst);
                break;
            }
        }
    });
    let worker_job = job_id.clone();
    let worker_tx = tx.clone();
    let worker_cancel = cancel.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        check_selection(&worker_job, options, &worker_tx, &worker_cancel)
    })
    .await;
    watcher.abort();
    let (event, error) = match outcome {
        Ok(Ok(true)) => ("Cancelled", None),
        Ok(Ok(false)) => ("Completed", None),
        Ok(Err(message)) => ("Failed", Some(message)),
        Err(error) => ("Failed", Some(format!("Hash check failed: {error}"))),
    };
    let _ = tx
        .send(ServerMessage::JobEvent {
            timestamp: now(),
            job_id,
            event: event.to_string(),
            files: None,
            progress: None,
            search_progress: None,
            decision: None,
            error,
            result_path: None,
            result_panel_id: None,
        })
        .await;
}

fn check_selection(
    job_id: &str,
    options: HashCheckOptions,
    tx: &mpsc::Sender<ServerMessage>,
    cancel: &Arc<AtomicBool>,
) -> Result<bool, String> {
    let driver = options.driver.as_ref();
    // One fresh directory listing validates entry types without following links.
    let entries = driver
        .list(
            &options.folder,
            &SortSpec {
                by: "name".into(),
                order: "asc".into(),
            },
            None,
            true,
        )
        .map_err(|e| format!("Cannot inspect hash-check folder: {e}"))?;
    let files: HashMap<_, _> = entries
        .iter()
        .filter(|entry| entry.kind == VfsKind::File)
        .map(|entry| (entry.name.as_str(), entry.size.unwrap_or(0)))
        .collect();
    let mut result = HashCheckBroResultSchema::default();
    let mut sizes = Vec::new();
    let mut counterpart_names = Vec::new();
    for input in &options.inputs {
        let counterpart = input
            .name
            .strip_suffix(".b3")
            .filter(|name| !name.is_empty());
        let pair = counterpart
            .and_then(|name| files.get(name).map(|size| (name, *size)))
            .filter(|_| input.kind == "file" && files.contains_key(input.name.as_str()));
        if let Some((name, size)) = pair {
            result.items.push(HashCheckBroFileSchema {
                hash_path: driver.resolve_child(&options.folder, &input.name),
                counterpart_path: driver.resolve_child(&options.folder, name),
                status: HashCheckStatus::Pending,
                expected_hash: None,
                actual_hash: None,
                error: None,
            });
            sizes.push(size);
            counterpart_names.push(name.to_string());
        } else {
            result.ignored_count += 1;
        }
    }
    send_result(tx, job_id, &result)?;
    let total = sizes
        .iter()
        .fold(0u64, |sum, size| sum.saturating_add(*size));
    let bytes = Arc::new(AtomicU64::new(0));
    let reporter = ProgressReporter::new(
        tx.clone(),
        job_id.to_string(),
        result.items.len(),
        total,
        Arc::new(AtomicUsize::new(0)),
        bytes.clone(),
        cancel.clone(),
    );
    let mut processed_bytes = 0u64;
    for (index, item) in result.items.iter_mut().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            item.status = HashCheckStatus::Cancelled;
            continue;
        }
        item.status = HashCheckStatus::Checking;
        send_item(tx, job_id, item)?;
        let size = sizes[index];
        let mut hashed = 0;
        reporter.send_snapshot(
            Some(item.counterpart_path.clone()),
            Some("Checking BLAKE3 hash".into()),
            size,
            0,
            false,
        );
        let checked = read_expected_hash(driver, &item.hash_path, &counterpart_names[index])
            .and_then(|expected| {
                item.expected_hash = Some(expected.clone());
                let reader = driver
                    .open_read(&item.counterpart_path)
                    .map_err(|e| format!("Cannot read counterpart: {e}"))?;
                let actual = hash_reader(reader, cancel, |count| {
                    hashed = count;
                    bytes.store(processed_bytes.saturating_add(count), Ordering::SeqCst);
                    reporter.send_snapshot(
                        Some(item.counterpart_path.clone()),
                        Some("Checking BLAKE3 hash".into()),
                        size,
                        count,
                        false,
                    );
                })?;
                item.status = if actual == expected {
                    HashCheckStatus::Matched
                } else {
                    HashCheckStatus::Mismatched
                };
                item.actual_hash = Some(actual);
                Ok(())
            });
        if let Err(error) = checked {
            if cancel.load(Ordering::SeqCst) {
                item.status = HashCheckStatus::Cancelled;
            } else {
                item.status = HashCheckStatus::Error;
                item.error = Some(error);
            }
        }
        send_item(tx, job_id, item)?;
        processed_bytes = processed_bytes.saturating_add(size);
        // Errors finish an attempted file; byte progress advances to its budget.
        bytes.store(processed_bytes, Ordering::SeqCst);
        reporter.file_completed(
            item.counterpart_path.clone(),
            size,
            hashed,
            Some("Checking BLAKE3 hashes".into()),
        );
    }
    send_result(tx, job_id, &result)?;
    // Freeze completion at the report commit boundary. A late cancel after
    // writing starts must not label a successfully saved report as cancelled.
    let cancelled = cancel.load(Ordering::SeqCst);
    if options.save_result && !cancelled {
        let timestamp = now();
        let body = format_report(&result, timestamp);
        let message = match save_report(driver, &options.folder, timestamp, &body) {
            Ok(path) => ServerMessage::HashCheckReportSaved {
                timestamp: now(),
                job_id: job_id.into(),
                path,
            },
            Err(message) => ServerMessage::HashCheckReportFailed {
                timestamp: now(),
                job_id: job_id.into(),
                message,
            },
        };
        tx.blocking_send(message)
            .map_err(|e| format!("Cannot send hash-check save result: {e}"))?;
    }
    Ok(cancelled)
}

fn read_expected_hash(
    driver: &dyn VfsDriver,
    path: &str,
    counterpart_name: &str,
) -> Result<String, String> {
    let mut reader = driver
        .open_read(path)
        .map_err(|e| format!("Cannot read sidecar: {e}"))?
        .take(SIDECAR_LIMIT + 1);
    let mut body = String::new();
    reader
        .read_to_string(&mut body)
        .map_err(|e| format!("Cannot read sidecar text: {e}"))?;
    if body.len() as u64 > SIDECAR_LIMIT {
        return Err("Sidecar exceeds 64 KiB".into());
    }
    parse_expected_hash(&body, counterpart_name)
}

fn parse_expected_hash(body: &str, counterpart_name: &str) -> Result<String, String> {
    let trimmed = body.trim();
    if trimmed.len() == 64 && trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(trimmed.to_ascii_lowercase());
    }
    let line = body.strip_suffix('\n').unwrap_or(body);
    let line = line.strip_suffix('\r').unwrap_or(line);
    let hash = line
        .get(..64)
        .filter(|hash| hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| "Expected a 64-digit BLAKE3 digest".to_string())?;
    let name = line
        .get(64..)
        .and_then(|rest| rest.strip_prefix("  "))
        .ok_or_else(|| "Expected a digest followed by two spaces and the filename".to_string())?;
    if name.contains(['\r', '\n']) || name != counterpart_name {
        return Err("Sidecar filename does not match its counterpart".into());
    }
    Ok(hash.to_ascii_lowercase())
}

fn send_result(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    result: &HashCheckBroResultSchema,
) -> Result<(), String> {
    tx.blocking_send(ServerMessage::HashCheckBroResult {
        timestamp: now(),
        job_id: job_id.into(),
        result: result.clone(),
    })
    .map_err(|e| format!("Cannot send hash-check results: {e}"))
}

fn send_item(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    item: &HashCheckBroFileSchema,
) -> Result<(), String> {
    tx.blocking_send(ServerMessage::HashCheckBroFileResult {
        timestamp: now(),
        job_id: job_id.into(),
        item: item.clone(),
    })
    .map_err(|e| format!("Cannot send hash-check file result: {e}"))
}

fn format_report(result: &HashCheckBroResultSchema, timestamp: u64) -> String {
    let count = |status| {
        result
            .items
            .iter()
            .filter(|item| item.status == status)
            .count()
    };
    let mut body = format!("fbro BLAKE3 hash check\nUTC timestamp (Unix milliseconds): {timestamp}\nPairs: {}\nMatches: {}\nMismatches: {}\nErrors: {}\nIgnored entries: {}\n",
        result.items.len(), count(HashCheckStatus::Matched), count(HashCheckStatus::Mismatched), count(HashCheckStatus::Error), result.ignored_count);
    if result.items.is_empty() {
        body.push_str("\nNo matching file pairs.\n");
    }
    for item in &result.items {
        body.push_str(&format!(
            "\n{:?}: {}\nSidecar: {}\n",
            item.status, item.counterpart_path, item.hash_path
        ));
        if let Some(hash) = &item.expected_hash {
            body.push_str(&format!("Expected: {hash}\n"));
        }
        if let Some(hash) = &item.actual_hash {
            body.push_str(&format!("Actual:   {hash}\n"));
        }
        if let Some(error) = &item.error {
            body.push_str(&format!("Error: {error}\n"));
        }
    }
    body
}

fn save_report(
    driver: &dyn VfsDriver,
    folder: &str,
    timestamp: u64,
    body: &str,
) -> Result<String, String> {
    for suffix in 0..10_000 {
        let name = if suffix == 0 {
            format!("fbro-hash-check-{timestamp}.txt")
        } else {
            format!("fbro-hash-check-{timestamp}-{suffix}.txt")
        };
        let path = driver.resolve_child(folder, &name);
        let mut writer = match driver.open_write_new(&path) {
            Err(VfsError::AlreadyExists(_)) => continue,
            result => result.map_err(|e| format!("Cannot create hash check report: {e}"))?,
        };
        if let Err(error) = writer
            .write_all(body.as_bytes())
            .and_then(|_| writer.flush())
        {
            drop(writer);
            let _ = driver.remove_file(&path);
            return Err(format!("Cannot write hash check report: {error}"));
        }
        return Ok(path);
    }
    Err("Cannot find an unused hash check report filename".into())
}

#[cfg(test)]
mod tests;
