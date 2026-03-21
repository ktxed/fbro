use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use tokio::sync::{mpsc, watch};

use crate::jobs::{ConflictPolicy, DecisionRx, FileOpEntry, FileOpProgress, FileOpType};
use crate::ws::protocol::{
    now, ConflictDecisionSchema, FileOpProgressSchema, ServerMessage,
};

const COPY_BUF_SIZE: usize = 64 * 1024; // 64 KiB

/// Run a file operation (copy/move/delete) in a blocking context.
/// Sends JobEvent messages for progress, decisions, completion, and errors.
pub async fn run_file_op(
    job_id: String,
    op_type: FileOpType,
    files: Vec<FileOpEntry>,
    dest_path: Option<String>,
    conflict_policy: ConflictPolicy,
    tx: mpsc::Sender<ServerMessage>,
    cancel_rx: watch::Receiver<bool>,
    mut decision_rx: DecisionRx,
) {
    let job_id_clone = job_id.clone();
    let tx_clone = tx.clone();
    let result = tokio::task::spawn_blocking(move || {
        run_file_op_blocking(
            &job_id_clone,
            &op_type,
            &files,
            dest_path.as_deref(),
            &conflict_policy,
            &tx_clone,
            &cancel_rx,
            &mut decision_rx,
        )
    })
    .await;

    match result {
        Ok(Ok(())) => {
            let _ = tx_event(&tx, &job_id, "Completed", None, None, None).await;
        }
        Ok(Err(err)) => {
            let _ = tx_event(&tx, &job_id, "Failed", None, None, Some(err)).await;
        }
        Err(_) => {
            let _ = tx_event(&tx, &job_id, "Cancelled", None, None, None).await;
        }
    }
}

fn run_file_op_blocking(
    job_id: &str,
    op_type: &FileOpType,
    files: &[FileOpEntry],
    dest_path: Option<&str>,
    conflict_policy: &ConflictPolicy,
    tx: &mpsc::Sender<ServerMessage>,
    cancel_rx: &watch::Receiver<bool>,
    decision_rx: &mut DecisionRx,
) -> Result<(), String> {
    let total_files = files.len();
    let mut completed_files = 0usize;

    for entry in files {
        // Check cancellation
        if *cancel_rx.borrow() {
            send_event_sync(tx, job_id, "Cancelled", None, None, None);
            return Ok(());
        }

        match op_type {
            FileOpType::Copy | FileOpType::Move => {
                let dest_dir = dest_path.ok_or("No destination path for copy/move")?;
                let src = &entry.source_path;
                let dst = Path::new(dest_dir).join(&entry.name);
                let dst_str = dst.to_string_lossy().to_string();

                if entry.kind == "directory" {
                    copy_dir_recursive(
                        job_id,
                        src,
                        &dst_str,
                        conflict_policy,
                        tx,
                        cancel_rx,
                        total_files,
                        completed_files,
                    )?;
                } else {
                    // Check conflict
                    if dst.exists() {
                        match conflict_policy {
                            ConflictPolicy::Skip => {
                                completed_files += 1;
                                send_progress_sync(
                                    tx, job_id, total_files, completed_files, None, 0, 0,
                                );
                                continue;
                            }
                            ConflictPolicy::Overwrite => {
                                // Proceed — will overwrite
                            }
                            ConflictPolicy::Ask => {
                                // Send decision request, then block waiting for answer
                                send_event_sync(
                                    tx,
                                    job_id,
                                    "DecisionRequired",
                                    None,
                                    Some(ConflictDecisionSchema {
                                        source: src.clone(),
                                        destination: dst_str.clone(),
                                        options: vec![
                                            "overwrite".to_string(),
                                            "skip".to_string(),
                                            "cancel".to_string(),
                                        ],
                                        supports_apply_to_all: true,
                                    }),
                                    None,
                                );

                                // Block until decision arrives
                                let decision = decision_rx.blocking_recv();
                                match decision.as_deref() {
                                    Some("overwrite") => { /* proceed */ }
                                    Some("skip") => {
                                        completed_files += 1;
                                        send_progress_sync(
                                            tx, job_id, total_files, completed_files,
                                            None, 0, 0,
                                        );
                                        continue;
                                    }
                                    Some("cancel") | None => {
                                        send_event_sync(
                                            tx, job_id, "Cancelled", None, None, None,
                                        );
                                        return Ok(());
                                    }
                                    _ => {
                                        // Unknown decision, skip
                                        completed_files += 1;
                                        continue;
                                    }
                                }
                            }
                        }
                    }

                    copy_file_with_progress(
                        job_id,
                        src,
                        &dst_str,
                        entry.size.unwrap_or(0),
                        tx,
                        cancel_rx,
                        total_files,
                        completed_files,
                    )?;
                }

                // For move: remove source after successful copy
                if *op_type == FileOpType::Move {
                    if entry.kind == "directory" {
                        let _ = fs::remove_dir_all(src);
                    } else {
                        let _ = fs::remove_file(src);
                    }
                }

                completed_files += 1;
                send_progress_sync(
                    tx, job_id, total_files, completed_files, None, 0, 0,
                );
            }

            FileOpType::Delete => {
                let src = &entry.source_path;

                send_progress_sync(
                    tx,
                    job_id,
                    total_files,
                    completed_files,
                    Some(entry.name.clone()),
                    0,
                    0,
                );

                if entry.kind == "directory" {
                    fs::remove_dir_all(src)
                        .map_err(|e| format!("Failed to delete {}: {e}", src))?;
                } else {
                    fs::remove_file(src)
                        .map_err(|e| format!("Failed to delete {}: {e}", src))?;
                }

                completed_files += 1;
                send_progress_sync(
                    tx, job_id, total_files, completed_files, None, 0, 0,
                );
            }
        }
    }

    Ok(())
}

fn copy_file_with_progress(
    job_id: &str,
    src: &str,
    dst: &str,
    total_bytes: u64,
    tx: &mpsc::Sender<ServerMessage>,
    cancel_rx: &watch::Receiver<bool>,
    total_files: usize,
    completed_files: usize,
) -> Result<(), String> {
    let mut reader =
        fs::File::open(src).map_err(|e| format!("Cannot open {src}: {e}"))?;

    // Get actual size from metadata if total_bytes is 0
    let file_size = if total_bytes > 0 {
        total_bytes
    } else {
        reader.metadata().map(|m| m.len()).unwrap_or(0)
    };

    let mut writer =
        fs::File::create(dst).map_err(|e| format!("Cannot create {dst}: {e}"))?;

    let mut buf = vec![0u8; COPY_BUF_SIZE];
    let mut copied: u64 = 0;
    let src_name = Path::new(src)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    loop {
        if *cancel_rx.borrow() {
            // Clean up partial file
            drop(writer);
            let _ = fs::remove_file(dst);
            return Err("Cancelled".to_string());
        }

        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Read error on {src}: {e}"))?;
        if n == 0 {
            break;
        }

        writer
            .write_all(&buf[..n])
            .map_err(|e| format!("Write error on {dst}: {e}"))?;

        copied += n as u64;

        // Send progress every 256 KiB to avoid flooding
        if copied % (256 * 1024) < COPY_BUF_SIZE as u64 || copied == file_size {
            send_progress_sync(
                tx,
                job_id,
                total_files,
                completed_files,
                Some(src_name.clone()),
                file_size,
                copied,
            );
        }
    }

    Ok(())
}

fn copy_dir_recursive(
    job_id: &str,
    src: &str,
    dst: &str,
    conflict_policy: &ConflictPolicy,
    tx: &mpsc::Sender<ServerMessage>,
    cancel_rx: &watch::Receiver<bool>,
    total_files: usize,
    completed_files: usize,
) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("Cannot create dir {dst}: {e}"))?;

    let entries =
        fs::read_dir(src).map_err(|e| format!("Cannot read dir {src}: {e}"))?;

    for entry_result in entries {
        if *cancel_rx.borrow() {
            return Err("Cancelled".to_string());
        }

        let entry = match entry_result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.file_name().to_string_lossy().to_string();
        let child_src = Path::new(src).join(&name).to_string_lossy().to_string();
        let child_dst = Path::new(dst).join(&name).to_string_lossy().to_string();

        let meta = entry.metadata().ok();
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);

        if is_dir {
            copy_dir_recursive(
                job_id,
                &child_src,
                &child_dst,
                conflict_policy,
                tx,
                cancel_rx,
                total_files,
                completed_files,
            )?;
        } else {
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);

            // Check conflict for files inside directory
            if Path::new(&child_dst).exists() {
                match conflict_policy {
                    ConflictPolicy::Skip => continue,
                    ConflictPolicy::Overwrite => {}
                    ConflictPolicy::Ask => {
                        // Inside recursive copy, default to overwrite for subdirectories
                        // (the top-level decision covers the directory)
                    }
                }
            }

            copy_file_with_progress(
                job_id,
                &child_src,
                &child_dst,
                size,
                tx,
                cancel_rx,
                total_files,
                completed_files,
            )?;
        }
    }

    Ok(())
}

// ─── Sync helpers (used from spawn_blocking context) ────────────────────────

fn send_progress_sync(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    total_files: usize,
    completed_files: usize,
    current_file: Option<String>,
    current_file_bytes_total: u64,
    current_file_bytes_copied: u64,
) {
    let progress = FileOpProgressSchema {
        total_files,
        completed_files,
        current_file,
        current_file_bytes_total,
        current_file_bytes_copied,
    };
    let msg = ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: "Progress".to_string(),
        progress: Some(progress),
        decision: None,
        error: None,
    };
    // Use try_send to avoid blocking — if channel is full, drop the progress update
    let _ = tx.try_send(msg);
}

fn send_event_sync(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    event: &str,
    progress: Option<FileOpProgressSchema>,
    decision: Option<ConflictDecisionSchema>,
    error: Option<String>,
) {
    let msg = ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: event.to_string(),
        progress,
        decision,
        error,
    };
    let _ = tx.try_send(msg);
}

async fn tx_event(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    event: &str,
    progress: Option<FileOpProgressSchema>,
    decision: Option<ConflictDecisionSchema>,
    error: Option<String>,
) -> Result<(), ()> {
    tx.send(ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: event.to_string(),
        progress,
        decision,
        error,
    })
    .await
    .map_err(|_| ())
}
