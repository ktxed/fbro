use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

use crate::jobs::{ConflictPolicy, DecisionRx, FileOpEntry, FileOpType};
use crate::vfs::VfsDriver;
use crate::ws::protocol::{now, ConflictDecisionSchema, FileOpProgressSchema, ServerMessage};

const COPY_BUF_SIZE: usize = 64 * 1024; // 64 KiB

/// Run a file operation (copy/move/delete) in a blocking context.
/// Sends JobEvent messages for progress, decisions, completion, and errors.
///
/// `src_driver` is the VFS driver for the source panel.
/// `dst_driver` is the VFS driver for the destination panel (None for delete).
///
/// When both drivers point to the same OS-managed VFS (same Arc),
/// the move operation attempts `try_rename` for an O(1) fast path.
pub async fn run_file_op(
    job_id: String,
    op_type: FileOpType,
    files: Vec<FileOpEntry>,
    dest_path: Option<String>,
    conflict_policy: ConflictPolicy,
    tx: mpsc::Sender<ServerMessage>,
    cancel_rx: watch::Receiver<bool>,
    pause_rx: watch::Receiver<bool>,
    mut decision_rx: DecisionRx,
    src_driver: Arc<dyn VfsDriver>,
    dst_driver: Option<Arc<dyn VfsDriver>>,
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
            &pause_rx,
            &mut decision_rx,
            &*src_driver,
            dst_driver.as_deref(),
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

/// Block while paused. Returns `true` if the job was cancelled during the wait.
fn wait_if_paused(pause_rx: &watch::Receiver<bool>, cancel_rx: &watch::Receiver<bool>) -> bool {
    loop {
        if *cancel_rx.borrow() {
            return true;
        }
        if !*pause_rx.borrow() {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
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
    pause_rx: &watch::Receiver<bool>,
    decision_rx: &mut DecisionRx,
    src_driver: &dyn VfsDriver,
    dst_driver: Option<&dyn VfsDriver>,
) -> Result<(), String> {
    // Totals grow as directories are discovered (no pre-scan — VFS may be remote)
    let mut total_files = 0usize;
    let mut total_bytes_total: u64 = 0;
    // Count top-level non-directory entries upfront
    for entry in files {
        if entry.kind != "directory" {
            total_files += 1;
            total_bytes_total += entry.size.unwrap_or(0);
        }
    }

    let mut completed_files = 0usize;
    let mut total_bytes_completed: u64 = 0;

    // For move: check if source and dest share the same driver (ptr equality).
    // If they do, try_rename may succeed as an O(1) operation.
    let same_driver = dst_driver.map_or(false, |d| std::ptr::eq(src_driver, d));

    for entry in files {
        if wait_if_paused(pause_rx, cancel_rx) {
            send_event_sync(tx, job_id, "Cancelled", None, None, None);
            return Ok(());
        }

        match op_type {
            FileOpType::Copy | FileOpType::Move => {
                let dst_drv = dst_driver.ok_or("No destination driver for copy/move")?;
                let dest_dir = dest_path.ok_or("No destination path for copy/move")?;
                let src = &entry.source_path;
                let dst_str = dst_drv.resolve_child(dest_dir, &entry.name);

                // For move with same driver: try O(1) rename first
                if *op_type == FileOpType::Move && same_driver {
                    match src_driver.try_rename(src, &dst_str) {
                        Ok(true) => {
                            // Rename succeeded — we can't know inner file count without
                            // scanning, so just count as 1 item for rename
                            if entry.kind != "directory" {
                                completed_files += 1;
                                total_bytes_completed += entry.size.unwrap_or(0);
                            }
                            send_progress_sync(
                                tx,
                                job_id,
                                total_files,
                                completed_files,
                                None,
                                0,
                                0,
                                total_bytes_total,
                                total_bytes_completed,
                            );
                            continue;
                        }
                        Ok(false) => {
                            // Cross-device — fall through to copy+delete
                        }
                        Err(e) => {
                            return Err(format!("Rename failed for {src}: {e}"));
                        }
                    }
                }

                if entry.kind == "directory" {
                    copy_dir_recursive(
                        job_id,
                        src,
                        &dst_str,
                        conflict_policy,
                        tx,
                        cancel_rx,
                        pause_rx,
                        &mut total_files,
                        &mut completed_files,
                        &mut total_bytes_total,
                        &mut total_bytes_completed,
                        src_driver,
                        dst_drv,
                    )?;
                } else {
                    // Check conflict
                    if dst_drv.exists(&dst_str) {
                        match resolve_conflict(
                            conflict_policy,
                            src,
                            &dst_str,
                            tx,
                            job_id,
                            decision_rx,
                        )? {
                            ConflictOutcome::Overwrite => { /* proceed */ }
                            ConflictOutcome::Skip => {
                                total_bytes_completed += entry.size.unwrap_or(0);
                                completed_files += 1;
                                send_progress_sync(
                                    tx,
                                    job_id,
                                    total_files,
                                    completed_files,
                                    None,
                                    0,
                                    0,
                                    total_bytes_total,
                                    total_bytes_completed,
                                );
                                continue;
                            }
                            ConflictOutcome::Cancel => {
                                send_event_sync(tx, job_id, "Cancelled", None, None, None);
                                return Ok(());
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
                        pause_rx,
                        total_files,
                        completed_files,
                        total_bytes_total,
                        total_bytes_completed,
                        src_driver,
                        dst_drv,
                    )?;

                    total_bytes_completed += entry.size.unwrap_or(0);
                    completed_files += 1;
                    send_progress_sync(
                        tx,
                        job_id,
                        total_files,
                        completed_files,
                        None,
                        0,
                        0,
                        total_bytes_total,
                        total_bytes_completed,
                    );
                }

                // For move: remove source after successful copy
                if *op_type == FileOpType::Move {
                    if entry.kind == "directory" {
                        let _ = src_driver.remove_dir_all(src);
                    } else {
                        let _ = src_driver.remove_file(src);
                    }
                }
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
                    total_bytes_total,
                    total_bytes_completed,
                );

                if entry.kind == "directory" {
                    src_driver
                        .remove_dir_all(src)
                        .map_err(|e| format!("Failed to delete {src}: {e}"))?;
                } else {
                    src_driver
                        .remove_file(src)
                        .map_err(|e| format!("Failed to delete {src}: {e}"))?;
                }

                completed_files += 1;
                send_progress_sync(
                    tx,
                    job_id,
                    total_files,
                    completed_files,
                    None,
                    0,
                    0,
                    total_bytes_total,
                    total_bytes_completed,
                );
            }
        }
    }

    Ok(())
}

// ─── Conflict resolution ────────────────────────────────────────────────────

enum ConflictOutcome {
    Overwrite,
    Skip,
    Cancel,
}

fn resolve_conflict(
    policy: &ConflictPolicy,
    src: &str,
    dst: &str,
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    decision_rx: &mut DecisionRx,
) -> Result<ConflictOutcome, String> {
    match policy {
        ConflictPolicy::Skip => Ok(ConflictOutcome::Skip),
        ConflictPolicy::Overwrite => Ok(ConflictOutcome::Overwrite),
        ConflictPolicy::Ask => {
            send_event_sync(
                tx,
                job_id,
                "DecisionRequired",
                None,
                Some(ConflictDecisionSchema {
                    source: src.to_string(),
                    destination: dst.to_string(),
                    options: vec![
                        "overwrite".to_string(),
                        "skip".to_string(),
                        "cancel".to_string(),
                    ],
                    supports_apply_to_all: true,
                }),
                None,
            );
            let decision = decision_rx.blocking_recv();
            match decision.as_deref() {
                Some("overwrite") => Ok(ConflictOutcome::Overwrite),
                Some("skip") => Ok(ConflictOutcome::Skip),
                Some("cancel") | None => Ok(ConflictOutcome::Cancel),
                _ => Ok(ConflictOutcome::Skip),
            }
        }
    }
}

// ─── File copy via VFS ──────────────────────────────────────────────────────

fn copy_file_with_progress(
    job_id: &str,
    src: &str,
    dst: &str,
    total_bytes: u64,
    tx: &mpsc::Sender<ServerMessage>,
    cancel_rx: &watch::Receiver<bool>,
    pause_rx: &watch::Receiver<bool>,
    total_files: usize,
    completed_files: usize,
    total_bytes_total: u64,
    total_bytes_completed: u64,
    src_driver: &dyn VfsDriver,
    dst_driver: &dyn VfsDriver,
) -> Result<(), String> {
    let mut reader = src_driver
        .open_read(src)
        .map_err(|e| format!("Cannot open {src}: {e}"))?;

    let file_size = if total_bytes > 0 {
        total_bytes
    } else {
        src_driver.file_size(src).unwrap_or(0)
    };

    let mut writer = dst_driver
        .open_write(dst)
        .map_err(|e| format!("Cannot create {dst}: {e}"))?;

    let mut buf = vec![0u8; COPY_BUF_SIZE];
    let mut copied: u64 = 0;
    let src_name = Path::new(src)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    loop {
        if wait_if_paused(pause_rx, cancel_rx) {
            drop(writer);
            let _ = dst_driver.remove_file(dst);
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

        if copied % (256 * 1024) < COPY_BUF_SIZE as u64 || copied == file_size {
            send_progress_sync(
                tx,
                job_id,
                total_files,
                completed_files,
                Some(src_name.clone()),
                file_size,
                copied,
                total_bytes_total,
                total_bytes_completed + copied,
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
    pause_rx: &watch::Receiver<bool>,
    total_files: &mut usize,
    completed_files: &mut usize,
    total_bytes_total: &mut u64,
    total_bytes_completed: &mut u64,
    src_driver: &dyn VfsDriver,
    dst_driver: &dyn VfsDriver,
) -> Result<(), String> {
    dst_driver
        .create_dir_all(dst)
        .map_err(|e| format!("Cannot create dir {dst}: {e}"))?;

    let children = src_driver
        .read_dir_entries(src)
        .map_err(|e| format!("Cannot read dir {src}: {e}"))?;

    // Discover files in this directory — grow totals
    for child in &children {
        if !child.is_dir {
            *total_files += 1;
            *total_bytes_total += child.size;
        }
    }

    for child in children {
        if wait_if_paused(pause_rx, cancel_rx) {
            return Err("Cancelled".to_string());
        }

        let child_src = src_driver.resolve_child(src, &child.name);
        let child_dst = dst_driver.resolve_child(dst, &child.name);

        if child.is_dir {
            copy_dir_recursive(
                job_id,
                &child_src,
                &child_dst,
                conflict_policy,
                tx,
                cancel_rx,
                pause_rx,
                total_files,
                completed_files,
                total_bytes_total,
                total_bytes_completed,
                src_driver,
                dst_driver,
            )?;
        } else {
            if dst_driver.exists(&child_dst) {
                match conflict_policy {
                    ConflictPolicy::Skip => {
                        *completed_files += 1;
                        *total_bytes_completed += child.size;
                        send_progress_sync(
                            tx,
                            job_id,
                            *total_files,
                            *completed_files,
                            None,
                            0,
                            0,
                            *total_bytes_total,
                            *total_bytes_completed,
                        );
                        continue;
                    }
                    ConflictPolicy::Overwrite | ConflictPolicy::Ask => {
                        // Inside recursive dir copy, honour the top-level policy
                    }
                }
            }

            copy_file_with_progress(
                job_id,
                &child_src,
                &child_dst,
                child.size,
                tx,
                cancel_rx,
                pause_rx,
                *total_files,
                *completed_files,
                *total_bytes_total,
                *total_bytes_completed,
                src_driver,
                dst_driver,
            )?;

            *completed_files += 1;
            *total_bytes_completed += child.size;
            send_progress_sync(
                tx,
                job_id,
                *total_files,
                *completed_files,
                None,
                0,
                0,
                *total_bytes_total,
                *total_bytes_completed,
            );
        }
    }

    Ok(())
}

// ─── Dry-run script generation ──────────────────────────────────────────────

/// Generate a bash script that previews what a file operation would do.
/// Supports copy (`cp`/`mkdir -p`), move (`mv`), and delete (`rm`/`rmdir`).
pub fn generate_dry_run_script(
    op_type: &FileOpType,
    files: &[FileOpEntry],
    script_path: &str,
    src_driver: &dyn VfsDriver,
    dest_path: Option<&str>,
    dst_driver: Option<&dyn VfsDriver>,
) -> Result<(), String> {
    let op_label = match op_type {
        FileOpType::Copy => "copy",
        FileOpType::Move => "move",
        FileOpType::Delete => "delete",
    };

    let mut lines = vec![
        "#!/usr/bin/env bash".to_string(),
        format!("# Generated by fbro dry-{op_label} — review carefully before running!"),
        String::new(),
    ];

    for entry in files {
        match op_type {
            FileOpType::Delete => {
                collect_rm_lines(&entry.source_path, &entry.kind, src_driver, &mut lines);
            }
            FileOpType::Copy => {
                let dp = dest_path.ok_or("No destination path for copy dry-run")?;
                let dd = dst_driver.ok_or("No destination driver for copy dry-run")?;
                let dst = dd.resolve_child(dp, &entry.name);
                collect_cp_lines(
                    &entry.source_path,
                    &dst,
                    &entry.kind,
                    src_driver,
                    dd,
                    &mut lines,
                );
            }
            FileOpType::Move => {
                let dp = dest_path.ok_or("No destination path for move dry-run")?;
                let dd = dst_driver.ok_or("No destination driver for move dry-run")?;
                let dst = dd.resolve_child(dp, &entry.name);
                collect_mv_lines(&entry.source_path, &dst, &mut lines);
            }
        }
    }

    let content = lines.join("\n") + "\n";
    std::fs::write(script_path, content).map_err(|e| format!("Failed to write script: {e}"))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(script_path)
            .map_err(|e| format!("Metadata error: {e}"))?
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(script_path, perms).map_err(|e| format!("chmod error: {e}"))?;
    }

    Ok(())
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn collect_rm_lines(path: &str, kind: &str, driver: &dyn VfsDriver, lines: &mut Vec<String>) {
    if kind == "directory" {
        if let Ok(children) = driver.read_dir_entries(path) {
            for child in &children {
                let child_path = driver.resolve_child(path, &child.name);
                let child_kind = if child.is_dir { "directory" } else { "file" };
                collect_rm_lines(&child_path, child_kind, driver, lines);
            }
        }
        lines.push(format!("rmdir {}", shell_quote(path)));
    } else {
        lines.push(format!("rm {}", shell_quote(path)));
    }
}

fn collect_cp_lines(
    src: &str,
    dst: &str,
    kind: &str,
    src_driver: &dyn VfsDriver,
    dst_driver: &dyn VfsDriver,
    lines: &mut Vec<String>,
) {
    if kind == "directory" {
        lines.push(format!("mkdir -p {}", shell_quote(dst)));
        if let Ok(children) = src_driver.read_dir_entries(src) {
            for child in &children {
                let child_src = src_driver.resolve_child(src, &child.name);
                let child_dst = dst_driver.resolve_child(dst, &child.name);
                let child_kind = if child.is_dir { "directory" } else { "file" };
                collect_cp_lines(
                    &child_src, &child_dst, child_kind, src_driver, dst_driver, lines,
                );
            }
        }
    } else {
        lines.push(format!("cp {} {}", shell_quote(src), shell_quote(dst)));
    }
}

fn collect_mv_lines(src: &str, dst: &str, lines: &mut Vec<String>) {
    lines.push(format!("mv {} {}", shell_quote(src), shell_quote(dst)));
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
    total_bytes_total: u64,
    total_bytes_copied: u64,
) {
    let progress = FileOpProgressSchema {
        total_files,
        completed_files,
        current_file,
        current_file_bytes_total,
        current_file_bytes_copied,
        total_bytes_total,
        total_bytes_copied,
    };
    let msg = ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: "Progress".to_string(),
        progress: Some(progress),
        decision: None,
        error: None,
    };
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
