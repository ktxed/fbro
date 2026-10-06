use std::collections::HashMap;
use std::io::Write;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{mpsc, watch};

use crate::bro::BroInputItem;
use crate::jobs::{hash::hash_reader, progress::ProgressReporter};
use crate::vfs::{VfsDriver, VfsError, VfsKind};
use crate::ws::protocol::{
    now, HashGenerateBroFileSchema, HashGenerateBroResultSchema, HashGenerateStatus, ServerMessage,
    SortSpec,
};

pub struct HashGenerateOptions {
    pub driver: Arc<dyn VfsDriver>,
    pub folder: String,
    pub inputs: Vec<BroInputItem>,
    pub overwrite_existing: bool,
}

pub async fn run_generate_hashes(
    job_id: String,
    options: HashGenerateOptions,
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
    let outcome = tokio::task::spawn_blocking(move || {
        generate_selection(&worker_job, options, &worker_tx, &cancel)
    })
    .await;
    watcher.abort();
    let (event, error) = match outcome {
        Ok(Ok(true)) => ("Cancelled", None),
        Ok(Ok(false)) => ("Completed", None),
        Ok(Err(error)) => ("Failed", Some(error)),
        Err(error) => ("Failed", Some(format!("Hash generation failed: {error}"))),
    };
    let _ = tx
        .send(ServerMessage::JobEvent {
            timestamp: now(),
            job_id,
            event: event.into(),
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

fn generate_selection(
    job_id: &str,
    options: HashGenerateOptions,
    tx: &mpsc::Sender<ServerMessage>,
    cancel: &Arc<AtomicBool>,
) -> Result<bool, String> {
    let driver = options.driver.as_ref();
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
        .map_err(|e| format!("Cannot inspect hash-generation folder: {e}"))?;
    let entries: HashMap<_, _> = entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry))
        .collect();
    let mut result = HashGenerateBroResultSchema::default();
    let mut sizes = Vec::new();
    let mut names = Vec::new();
    for input in &options.inputs {
        let Some(entry) = entries.get(input.name.as_str()).filter(|entry| {
            entry.kind == VfsKind::File && input.kind == "file" && !input.name.ends_with(".b3")
        }) else {
            result.ignored_count += 1;
            continue;
        };
        let hash_name = format!("{}.b3", input.name);
        let existing = entries.get(hash_name.as_str());
        let (status, error) = if existing.is_some() && !options.overwrite_existing {
            (HashGenerateStatus::Skipped, None)
        } else if existing.is_some_and(|entry| entry.kind != VfsKind::File) {
            (
                HashGenerateStatus::Error,
                Some("The .b3 destination is not a regular file".into()),
            )
        } else {
            (HashGenerateStatus::Pending, None)
        };
        sizes.push(if status == HashGenerateStatus::Pending {
            entry.size.unwrap_or(0)
        } else {
            0
        });
        names.push(input.name.clone());
        result.items.push(HashGenerateBroFileSchema {
            source_path: driver.resolve_child(&options.folder, &input.name),
            hash_path: driver.resolve_child(&options.folder, &hash_name),
            status,
            hash: None,
            error,
        });
    }
    send_result(tx, job_id, &result)?;
    let total = sizes
        .iter()
        .fold(0u64, |sum, size| sum.saturating_add(*size));
    let bytes = Arc::new(AtomicU64::new(0));
    let reporter = ProgressReporter::new(
        tx.clone(),
        job_id.into(),
        result.items.len(),
        total,
        Arc::new(AtomicUsize::new(0)),
        bytes.clone(),
        cancel.clone(),
    );
    let mut processed_bytes = 0u64;
    let mut cancelled = cancel.load(Ordering::SeqCst);
    for (index, item) in result.items.iter_mut().enumerate() {
        let size = sizes[index];
        let mut hashed = 0;
        if item.status == HashGenerateStatus::Pending {
            if cancel.load(Ordering::SeqCst) {
                cancelled = true;
                item.status = HashGenerateStatus::Cancelled;
                continue;
            }
            item.status = HashGenerateStatus::Hashing;
            send_item(tx, job_id, item)?;
            reporter.send_snapshot(
                Some(item.source_path.clone()),
                Some("Generating BLAKE3 hash".into()),
                size,
                0,
                false,
            );
            let outcome = driver
                .open_read(&item.source_path)
                .map_err(|e| format!("Cannot read file: {e}"))
                .and_then(|reader| {
                    hash_reader(reader, cancel, |count| {
                        hashed = count;
                        bytes.store(processed_bytes.saturating_add(count), Ordering::SeqCst);
                        reporter.send_snapshot(
                            Some(item.source_path.clone()),
                            Some("Generating BLAKE3 hash".into()),
                            size,
                            count,
                            false,
                        );
                    })
                })
                .and_then(|hash| {
                    item.hash = Some(hash.clone());
                    save_sidecar(
                        driver,
                        &options.folder,
                        &item.hash_path,
                        &names[index],
                        &hash,
                        options.overwrite_existing,
                        cancel,
                    )
                });
            match outcome {
                Ok(status) => {
                    item.status = status;
                    if matches!(
                        item.status,
                        HashGenerateStatus::Created | HashGenerateStatus::Replaced
                    ) {
                        crate::journal::record_mutation(
                            job_id,
                            "hash-generate",
                            &item.source_path,
                            Some(&item.hash_path),
                        );
                    }
                }
                Err(error) if cancel.load(Ordering::SeqCst) && error == "Cancelled" => {
                    cancelled = true;
                    item.status = HashGenerateStatus::Cancelled;
                }
                Err(error) => {
                    item.status = HashGenerateStatus::Error;
                    item.error = Some(error);
                }
            }
            send_item(tx, job_id, item)?;
        }
        processed_bytes = processed_bytes.saturating_add(size);
        bytes.store(processed_bytes, Ordering::SeqCst);
        reporter.file_completed(
            item.source_path.clone(),
            size,
            hashed,
            Some("Generating BLAKE3 hashes".into()),
        );
    }
    send_result(tx, job_id, &result)?;
    // Successful per-file commits remain successful even if cancellation arrives afterward.
    Ok(cancelled)
}

/// Remove only an output owned by this invocation when writing/committing fails.
struct PendingOutput<'a> {
    driver: &'a dyn VfsDriver,
    path: String,
    committed: bool,
}
impl Drop for PendingOutput<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.driver.remove_file(&self.path);
        }
    }
}

fn save_sidecar(
    driver: &dyn VfsDriver,
    folder: &str,
    path: &str,
    name: &str,
    hash: &str,
    overwrite: bool,
    cancel: &AtomicBool,
) -> Result<HashGenerateStatus, String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("Cancelled".into());
    }
    let output = if overwrite {
        driver.resolve_child(folder, &format!(".fbro-hash-{}.tmp", uuid::Uuid::new_v4()))
    } else {
        path.into()
    };
    let mut writer = match driver.open_write_new(&output) {
        Err(VfsError::AlreadyExists(_)) if !overwrite => return Ok(HashGenerateStatus::Skipped),
        outcome => outcome.map_err(|e| format!("Cannot create hash file: {e}"))?,
    };
    let mut pending = PendingOutput {
        driver,
        path: output.clone(),
        committed: false,
    };
    let body = if name.contains(['\r', '\n']) {
        format!("{hash}\n")
    } else {
        format!("{hash}  {name}\n")
    };
    let written = writer
        .write_all(body.as_bytes())
        .and_then(|_| writer.flush());
    // Close the writer before cleanup or rename, including on Windows.
    drop(writer);
    written.map_err(|e| format!("Cannot write hash file: {e}"))?;
    if cancel.load(Ordering::SeqCst) {
        return Err("Cancelled".into());
    }
    let status = if overwrite {
        let existed = driver.exists_no_follow(path);
        if !driver
            .try_rename(&output, path)
            .map_err(|e| format!("Cannot replace hash file: {e}"))?
        {
            return Err("The source cannot atomically replace hash files".into());
        }
        if existed {
            HashGenerateStatus::Replaced
        } else {
            HashGenerateStatus::Created
        }
    } else {
        HashGenerateStatus::Created
    };
    pending.committed = true;
    Ok(status)
}

fn send_result(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    result: &HashGenerateBroResultSchema,
) -> Result<(), String> {
    tx.blocking_send(ServerMessage::HashGenerateBroResult {
        timestamp: now(),
        job_id: job_id.into(),
        result: result.clone(),
    })
    .map_err(|e| format!("Cannot send hash-generation results: {e}"))
}
fn send_item(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    item: &HashGenerateBroFileSchema,
) -> Result<(), String> {
    tx.blocking_send(ServerMessage::HashGenerateBroFileResult {
        timestamp: now(),
        job_id: job_id.into(),
        item: item.clone(),
    })
    .map_err(|e| format!("Cannot send hash-generation file result: {e}"))
}

#[cfg(test)]
mod tests;
