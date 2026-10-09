use std::io::{Read, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

use crate::jobs::{
    ConflictPolicy, DecisionResponse, DecisionRx, FileOpEntry, FileOpType, VerificationPolicy,
};
use crate::vfs::{VfsDriver, VfsTransferDigest};
use crate::ws::protocol::{now, ConflictDecisionSchema, FileOpProgressSchema, ServerMessage};

const COPY_BUF_SIZE: usize = 256 * 1024; // 256 KiB
const COPY_PROGRESS_INTERVAL: Duration = Duration::from_millis(80);
const COPY_RESUME_REWIND_BYTES: u64 = 1024 * 1024;
const COPY_RESUME_MAX_ATTEMPTS: usize = 8;

fn apply_file_attrs(dst_path: &str, mtime_secs: Option<u64>, permissions: Option<&str>) {
    if let Some(secs) = mtime_secs {
        set_mtime(dst_path, secs);
    }
    #[cfg(unix)]
    if let Some(perms) = permissions {
        use std::os::unix::fs::PermissionsExt;
        let mode = perms.chars().enumerate().fold(0u32, |acc, (i, c)| {
            let bit: u32 = match (i % 3, c) {
                (0, 'r') => 4,
                (1, 'w') => 2,
                (2, 'x') => 1,
                _ => 0,
            };
            let shift = 6u32.saturating_sub((i as u32 / 3) * 3);
            acc | (bit << shift)
        });
        let _ = std::fs::set_permissions(dst_path, std::fs::Permissions::from_mode(mode));
    }
}

#[cfg(unix)]
fn set_mtime(dst_path: &str, secs: u64) {
    use libc::{timespec, utimensat, AT_FDCWD, UTIME_OMIT};
    use std::ffi::CString;
    let Ok(c_path) = CString::new(dst_path) else {
        return;
    };
    let times = [
        timespec {
            tv_sec: 0,
            tv_nsec: UTIME_OMIT,
        },
        timespec {
            tv_sec: secs as libc::time_t,
            tv_nsec: 0,
        },
    ];
    unsafe { utimensat(AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) };
}

#[cfg(not(unix))]
fn set_mtime(dst_path: &str, secs: u64) {
    use std::time::{Duration, UNIX_EPOCH};
    let mtime = UNIX_EPOCH + Duration::from_secs(secs);
    let times = std::fs::FileTimes::new().set_modified(mtime);
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(dst_path) {
        let _ = f.set_times(times);
    }
}
const CANCELLED_ERR: &str = "Cancelled";

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
    preserve_attributes: bool,
    verification_policy: VerificationPolicy,
    save_digest_file: bool,
    auto_resume_copies: bool,
    skip_current_file: Arc<AtomicBool>,
    keep_partial_files_on_cancel: Arc<AtomicBool>,
) {
    let op_name = file_op_name(&op_type);
    let job_id_clone = job_id.clone();
    let tx_clone = tx.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _ctx = FileOpDriverContext::new(&*src_driver, &job_id_clone);
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
            preserve_attributes,
            &verification_policy,
            save_digest_file,
            auto_resume_copies,
            skip_current_file,
            keep_partial_files_on_cancel,
        )
    })
    .await;

    match result {
        Ok(Ok(())) => {
            crate::journal::record_operation_finished(&job_id, op_name, "completed", None);
            let _ = tx_event(&tx, &job_id, "Completed", None, None, None).await;
        }
        Ok(Err(err)) => {
            if err == CANCELLED_ERR {
                crate::journal::record_operation_finished(&job_id, op_name, "cancelled", None);
                let _ = tx_event(&tx, &job_id, "Cancelled", None, None, None).await;
            } else {
                crate::journal::record_operation_finished(&job_id, op_name, "failed", Some(&err));
                let _ = tx_event(&tx, &job_id, "Failed", None, None, Some(err)).await;
            }
        }
        Err(_) => {
            crate::journal::record_operation_finished(&job_id, op_name, "cancelled", None);
            let _ = tx_event(&tx, &job_id, "Cancelled", None, None, None).await;
        }
    }
}

fn file_op_name(op_type: &FileOpType) -> &'static str {
    match op_type {
        FileOpType::Copy => "copy",
        FileOpType::Move => "move",
        FileOpType::Delete => "delete",
    }
}

struct FileOpDriverContext<'a> {
    driver: &'a dyn VfsDriver,
    job_id: &'a str,
}

impl<'a> FileOpDriverContext<'a> {
    fn new(driver: &'a dyn VfsDriver, job_id: &'a str) -> Self {
        driver.begin_file_op(job_id);
        Self { driver, job_id }
    }
}

impl Drop for FileOpDriverContext<'_> {
    fn drop(&mut self) {
        self.driver.end_file_op(self.job_id);
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
    preserve_attributes: bool,
    verification_policy: &VerificationPolicy,
    save_digest_file: bool,
    auto_resume_copies: bool,
    skip_current_file: Arc<AtomicBool>,
    keep_partial_files_on_cancel: Arc<AtomicBool>,
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
    let mut apply_to_all = apply_all_outcome(match conflict_policy {
        ConflictPolicy::Ask => None,
        other => Some(other),
    });

    // Independently registered volume aliases each have their own driver Arc,
    // even when both paths belong to the same local filesystem. Local drivers
    // can safely attempt rename across those aliases: the OS returns EXDEV and
    // LocalFsDriver falls back to copy + delete when they are truly on separate
    // filesystems. Non-local drivers still require identical driver identity.
    let can_try_rename = dst_driver.is_some_and(|dst| {
        std::ptr::eq(src_driver, dst) || (src_driver.is_local() && dst.is_local())
    });

    for entry in files {
        if wait_if_paused(pause_rx, cancel_rx) {
            return Err(CANCELLED_ERR.to_string());
        }

        match op_type {
            FileOpType::Copy | FileOpType::Move => {
                let dst_drv = dst_driver.ok_or("No destination driver for copy/move")?;
                let dest_dir = dest_path.ok_or("No destination path for copy/move")?;
                let src = &entry.source_path;
                let dst_name = destination_name(entry);
                let dst_str = dst_drv.resolve_child(dest_dir, dst_name);

                if (*op_type == FileOpType::Copy || entry.kind == "symlink") && same_resolved_path(src, &dst_str) {
                    return Err(format!("Cannot place {src} onto itself or its target"));
                }

                // Try O(1) rename when the drivers can address the same local
                // filesystem and the destination does not exist. Existing
                // targets still use the conflict-aware copy+delete path.
                if *op_type == FileOpType::Move && can_try_rename && !dst_drv.exists_no_follow(&dst_str) {
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
                            crate::journal::record_mutation(job_id, "move", src, Some(&dst_str));
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

                if entry.kind == "symlink" && src_driver.is_local() {
                    let copied = copy_symlink(
                        job_id, src, &dst_str, conflict_policy, tx, decision_rx,
                        &mut apply_to_all, src_driver, dst_drv,
                    )?;
                    if !copied {
                        completed_files += 1;
                        send_progress_sync(tx, job_id, total_files, completed_files,
                            None, 0, 0, total_bytes_total, total_bytes_completed);
                        continue;
                    }
                    completed_files += 1;
                    send_progress_sync(tx, job_id, total_files, completed_files,
                        None, 0, 0, total_bytes_total, total_bytes_completed);
                } else if entry.kind == "directory" {
                    copy_dir_recursive(
                        job_id,
                        src,
                        &dst_str,
                        entry.mtime,
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
                        decision_rx,
                        &mut apply_to_all,
                        preserve_attributes,
                        verification_policy,
                        save_digest_file,
                        auto_resume_copies,
                        Arc::clone(&skip_current_file),
                        Arc::clone(&keep_partial_files_on_cancel),
                    )?;
                } else {
                    let mut resume_requested = false;
                    if dst_drv.exists_no_follow(&dst_str) {
                        let destination_is_link = dst_drv.read_link(&dst_str).is_ok();
                        let resume_candidate = (!destination_is_link).then(|| resume_candidate(
                            src, &dst_str, entry.size.unwrap_or(0), src_driver, dst_drv,
                        )).flatten();
                        match resolve_conflict(
                            conflict_policy,
                            src,
                            &dst_str,
                            resume_candidate.as_ref(),
                            tx,
                            job_id,
                            decision_rx,
                            &mut apply_to_all,
                        )? {
                            ConflictOutcome::Overwrite => {
                                if destination_is_link {
                                    dst_drv.remove_file(&dst_str).map_err(|error| format!("Cannot replace destination link {dst_str}: {error}"))?;
                                }
                            }
                            ConflictOutcome::Resume => {
                                resume_requested = true;
                            }
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
                                return Err(CANCELLED_ERR.to_string());
                            }
                        }
                    }

                    let copied = copy_file_with_progress(
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
                        decision_rx,
                        verification_policy,
                        save_digest_file,
                        auto_resume_copies,
                        Arc::clone(&skip_current_file),
                        resume_requested,
                        Arc::clone(&keep_partial_files_on_cancel),
                    )?;

                    if copied && preserve_attributes && dst_drv.is_local() {
                        apply_file_attrs(&dst_str, entry.mtime, entry.permissions.as_deref());
                    }

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
                crate::journal::record_mutation(
                    job_id,
                    match op_type { FileOpType::Copy => "copy", FileOpType::Move => "move", FileOpType::Delete => unreachable!() },
                    src,
                    Some(&dst_str),
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
                crate::journal::record_mutation(job_id, "delete", src, None);
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

fn destination_name(entry: &FileOpEntry) -> &str {
    entry.dest_name.as_deref().unwrap_or(&entry.name)
}

/// Recreate the link itself. The temporary link keeps an existing destination
/// intact if link creation is denied (notably on Windows).
fn copy_symlink(
    job_id: &str,
    src: &str,
    dst: &str,
    conflict_policy: &ConflictPolicy,
    tx: &mpsc::Sender<ServerMessage>,
    decision_rx: &mut DecisionRx,
    apply_to_all: &mut Option<ConflictOutcome>,
    src_driver: &dyn VfsDriver,
    dst_driver: &dyn VfsDriver,
) -> Result<bool, String> {
    if !dst_driver.is_local() {
        return Err(format!("Cannot copy symbolic link to a destination that does not support links: {dst}"));
    }
    let link = src_driver.read_link(src).map_err(|error| format!("Cannot read symbolic link {src}: {error}"))?;
    let exists = dst_driver.exists_no_follow(dst);
    if exists {
        match resolve_conflict(conflict_policy, src, dst, None, tx, job_id, decision_rx, apply_to_all)? {
            ConflictOutcome::Skip => return Ok(false),
            ConflictOutcome::Cancel => return Err(CANCELLED_ERR.into()),
            ConflictOutcome::Overwrite | ConflictOutcome::Resume => {},
        }
    }
    if let Some(parent) = dst_driver.parent_path(dst).filter(|parent| !parent.is_empty()) {
        dst_driver.create_dir_all(&parent)
            .map_err(|error| format!("Cannot create destination directory {parent}: {error}"))?;
    }
    let temp = format!("{dst}.fbro-link-{}", uuid::Uuid::new_v4());
    dst_driver.create_symlink(&link.target, &temp, link.is_dir)
        .map_err(|error| format!("Cannot create symbolic link {dst}: {error}"))?;
    if exists {
        if let Err(error) = dst_driver.remove_file(dst) {
            let _ = dst_driver.remove_file(&temp);
            return Err(format!("Cannot replace destination {dst}: {error}"));
        }
    }
    match dst_driver.try_rename(&temp, dst) {
        Ok(true) => Ok(true),
        Ok(false) => {
            let _ = dst_driver.remove_file(&temp);
            Err(format!("Cannot place symbolic link at {dst}"))
        }
        Err(error) => {
            let _ = dst_driver.remove_file(&temp);
            Err(format!("Cannot place symbolic link at {dst}: {error}"))
        }
    }
}

pub(crate) fn same_resolved_path(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let left_path = Path::new(left);
    let right_path = Path::new(right);
    if let (Ok(left_canon), Ok(right_canon)) = (left_path.canonicalize(), right_path.canonicalize())
    {
        return left_canon == right_canon;
    }
    normalize_path_for_compare(left) == normalize_path_for_compare(right)
}

fn normalize_path_for_compare(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let trimmed = normalized.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

// ─── Conflict resolution ────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConflictOutcome {
    Overwrite,
    Resume,
    Skip,
    Cancel,
}

fn apply_all_outcome(policy: Option<&ConflictPolicy>) -> Option<ConflictOutcome> {
    match policy {
        Some(ConflictPolicy::Overwrite) => Some(ConflictOutcome::Overwrite),
        Some(ConflictPolicy::Resume) => Some(ConflictOutcome::Resume),
        Some(ConflictPolicy::Skip) => Some(ConflictOutcome::Skip),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug)]
struct ResumeCandidate {
    dest_size: u64,
    source_size: u64,
}

impl ResumeCandidate {
    fn offset(self) -> u64 {
        self.dest_size.saturating_sub(COPY_RESUME_REWIND_BYTES)
    }
}

fn resume_candidate(
    src: &str,
    dst: &str,
    known_source_size: u64,
    src_driver: &dyn VfsDriver,
    dst_driver: &dyn VfsDriver,
) -> Option<ResumeCandidate> {
    if !src_driver.supports_read_at() || !dst_driver.supports_write_at() {
        return None;
    }
    let source_size = if known_source_size > 0 {
        known_source_size
    } else {
        src_driver.file_size(src)?
    };
    let dest_size = dst_driver.file_size(dst)?;
    if dest_size > 0 && dest_size < source_size {
        Some(ResumeCandidate {
            dest_size,
            source_size,
        })
    } else {
        None
    }
}

fn resolve_conflict(
    policy: &ConflictPolicy,
    src: &str,
    dst: &str,
    resume_candidate: Option<&ResumeCandidate>,
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    decision_rx: &mut DecisionRx,
    apply_to_all: &mut Option<ConflictOutcome>,
) -> Result<ConflictOutcome, String> {
    match policy {
        ConflictPolicy::Skip => Ok(ConflictOutcome::Skip),
        ConflictPolicy::Overwrite => Ok(ConflictOutcome::Overwrite),
        ConflictPolicy::Resume => {
            if resume_candidate.is_some() {
                Ok(ConflictOutcome::Resume)
            } else {
                Ok(ConflictOutcome::Overwrite)
            }
        }
        ConflictPolicy::Ask => {
            if let Some(outcome) = *apply_to_all {
                if outcome == ConflictOutcome::Resume && resume_candidate.is_none() {
                    return Ok(ConflictOutcome::Overwrite);
                }
                return Ok(outcome);
            }
            let mut options = vec![
                "overwrite".to_string(),
                "skip".to_string(),
                "cancel".to_string(),
            ];
            let message = resume_candidate.map(|candidate| {
                options.insert(0, "resume".to_string());
                format!(
                    "Destination is {} of {} bytes; resume will rewind to byte {}.",
                    candidate.dest_size,
                    candidate.source_size,
                    candidate.offset()
                )
            });
            send_event_sync(
                tx,
                job_id,
                "DecisionRequired",
                None,
                Some(ConflictDecisionSchema {
                    kind: Some("conflict".to_string()),
                    source: src.to_string(),
                    destination: dst.to_string(),
                    message,
                    options,
                    supports_apply_to_all: true,
                }),
                None,
            );
            let decision = decision_rx.blocking_recv();
            match decision {
                Some(DecisionResponse {
                    decision,
                    apply_to_all: true,
                }) => {
                    let outcome = match decision.as_str() {
                        "resume" if resume_candidate.is_some() => ConflictOutcome::Resume,
                        "overwrite" => ConflictOutcome::Overwrite,
                        "skip" => ConflictOutcome::Skip,
                        "cancel" => ConflictOutcome::Cancel,
                        _ => ConflictOutcome::Skip,
                    };
                    if matches!(
                        outcome,
                        ConflictOutcome::Overwrite
                            | ConflictOutcome::Resume
                            | ConflictOutcome::Skip
                    ) {
                        *apply_to_all = Some(outcome);
                    }
                    Ok(outcome)
                }
                Some(DecisionResponse { decision, .. }) => match decision.as_str() {
                    "resume" if resume_candidate.is_some() => Ok(ConflictOutcome::Resume),
                    "overwrite" => Ok(ConflictOutcome::Overwrite),
                    "skip" => Ok(ConflictOutcome::Skip),
                    "cancel" => Ok(ConflictOutcome::Cancel),
                    _ => Ok(ConflictOutcome::Skip),
                },
                None => Ok(ConflictOutcome::Cancel),
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
    decision_rx: &mut DecisionRx,
    verification_policy: &VerificationPolicy,
    save_digest_file: bool,
    auto_resume_copies: bool,
    skip_current_file: Arc<AtomicBool>,
    resume_requested: bool,
    keep_partial_files_on_cancel: Arc<AtomicBool>,
) -> Result<bool, String> {
    let src_name = Path::new(src)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let can_resume = src_driver.supports_read_at() && dst_driver.supports_write_at();
    let mut resume_next_attempt = resume_requested;
    let mut resume_attempts = 0usize;
    loop {
        match copy_file_once_with_progress(
            job_id,
            src,
            dst,
            total_bytes,
            tx,
            cancel_rx,
            pause_rx,
            total_files,
            completed_files,
            total_bytes_total,
            total_bytes_completed,
            src_driver,
            dst_driver,
            &src_name,
            save_digest_file,
            resume_next_attempt,
            Arc::clone(&skip_current_file),
            Arc::clone(&keep_partial_files_on_cancel),
        ) {
            Ok(()) => return Ok(true),
            Err(CopyFileError::Skipped(file_name)) => {
                skip_current_file.store(false, Ordering::SeqCst);
                send_event_sync(tx, job_id, "FileSkipped", None, None, Some(file_name));
                return Ok(false);
            }
            Err(CopyFileError::Fatal(e)) => return Err(e),
            Err(CopyFileError::Recoverable(e)) => {
                if *cancel_rx.borrow() {
                    return Err(CANCELLED_ERR.to_string());
                }
                if !auto_resume_copies || !can_resume {
                    return Err(e);
                }
                resume_attempts += 1;
                if resume_attempts > COPY_RESUME_MAX_ATTEMPTS {
                    return Err(e);
                }
                let partial = dst_driver.file_size(dst).unwrap_or(0).min(total_bytes);
                send_progress_sync_with_status(
                    tx,
                    job_id,
                    total_files,
                    completed_files,
                    Some(src_name.clone()),
                    total_bytes,
                    partial,
                    total_bytes_total,
                    total_bytes_completed + partial,
                    Some(format!(
                        "Resuming transfer after read error ({resume_attempts}/{COPY_RESUME_MAX_ATTEMPTS})"
                    )),
                );
                resume_next_attempt = true;
                continue;
            }
            Err(CopyFileError::Verification(e)) => {
                if *verification_policy == VerificationPolicy::Fail {
                    return Err(e);
                }
                match resolve_verification_mismatch(src, dst, &e, tx, job_id, decision_rx)? {
                    VerificationOutcome::Retry => {
                        if *cancel_rx.borrow() {
                            return Err(CANCELLED_ERR.to_string());
                        }
                        resume_next_attempt = false;
                        send_progress_sync_with_status(
                            tx,
                            job_id,
                            total_files,
                            completed_files,
                            Some(src_name.clone()),
                            total_bytes,
                            0,
                            total_bytes_total,
                            total_bytes_completed,
                            Some("Retrying transfer".to_string()),
                        );
                        continue;
                    }
                    VerificationOutcome::Skip => return Ok(false),
                    VerificationOutcome::Abort => return Err(e),
                }
            }
        }
    }
}

enum CopyFileError {
    Fatal(String),
    Recoverable(String),
    Skipped(String),
    Verification(String),
}

fn copy_file_once_with_progress(
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
    src_name: &str,
    save_digest_file: bool,
    resume_requested: bool,
    skip_current_file: Arc<AtomicBool>,
    keep_partial_files_on_cancel: Arc<AtomicBool>,
) -> Result<(), CopyFileError> {
    let file_size = if total_bytes > 0 {
        total_bytes
    } else {
        src_driver.file_size(src).unwrap_or(0)
    };

    let resume_offset = if resume_requested {
        resume_candidate(src, dst, file_size, src_driver, dst_driver)
            .map(ResumeCandidate::offset)
            .unwrap_or(0)
    } else {
        0
    };

    if resume_offset > 0 {
        send_progress_sync_with_status(
            tx,
            job_id,
            total_files,
            completed_files,
            Some(src_name.to_string()),
            file_size,
            resume_offset,
            total_bytes_total,
            total_bytes_completed + resume_offset,
            Some(format!("Resuming from byte {resume_offset}")),
        );
    }

    if skip_current_file.load(Ordering::SeqCst) {
        if resume_offset == 0 {
            let _ = dst_driver.remove_file(dst);
        }
        return Err(CopyFileError::Skipped(src_name.to_string()));
    }

    let mut reader = if resume_offset > 0 {
        src_driver.open_read_at(src, resume_offset).map_err(|e| {
            if matches!(e, crate::vfs::VfsError::Cancelled) {
                CopyFileError::Fatal(CANCELLED_ERR.to_string())
            } else {
                CopyFileError::Recoverable(format!(
                    "Cannot resume {src} at byte {resume_offset}: {e}"
                ))
            }
        })?
    } else {
        src_driver.open_read(src).map_err(|e| {
            if matches!(e, crate::vfs::VfsError::Cancelled) {
                CopyFileError::Fatal(CANCELLED_ERR.to_string())
            } else {
                CopyFileError::Fatal(format!("Cannot open {src}: {e}"))
            }
        })?
    };

    if resume_offset == 0 {
        if let Some(parent) = dst_driver.parent_path(dst) {
            if !parent.is_empty() {
                dst_driver.create_dir_all(&parent).map_err(|e| {
                    CopyFileError::Fatal(format!(
                        "Cannot create destination directory {parent}: {e}"
                    ))
                })?;
            }
        }
    }

    let mut writer = if resume_offset > 0 {
        dst_driver.open_write_at(dst, resume_offset).map_err(|e| {
            CopyFileError::Fatal(format!(
                "Cannot resume writing {dst} at byte {resume_offset}: {e}"
            ))
        })?
    } else {
        dst_driver
            .open_write(dst)
            .map_err(|e| CopyFileError::Fatal(format!("Cannot create {dst}: {e}")))?
    };

    let mut buf = vec![0u8; COPY_BUF_SIZE];
    let mut copied: u64 = resume_offset;
    let mut stream_hasher =
        hash_existing_prefix(dst_driver, dst, resume_offset).map_err(CopyFileError::Fatal)?;
    let mut last_progress_at = Instant::now();
    loop {
        if skip_current_file.load(Ordering::SeqCst) {
            drop(writer);
            if resume_offset == 0 {
                let _ = dst_driver.remove_file(dst);
            }
            return Err(CopyFileError::Skipped(src_name.to_string()));
        }

        if wait_if_paused(pause_rx, cancel_rx) {
            drop(writer);
            if resume_offset == 0 && !keep_partial_files_on_cancel.load(Ordering::Relaxed) {
                let _ = dst_driver.remove_file(dst);
            }
            return Err(CopyFileError::Fatal(CANCELLED_ERR.to_string()));
        }

        let read = reader.read(&mut buf);
        let remote_cancelled = read.as_ref().err().is_some_and(|error| {
            error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<crate::vfs::VfsError>())
                .is_some_and(|error| matches!(error, crate::vfs::VfsError::Cancelled))
        });
        if remote_cancelled || *cancel_rx.borrow() {
            drop(writer);
            if resume_offset == 0 && !keep_partial_files_on_cancel.load(Ordering::Relaxed) {
                let _ = dst_driver.remove_file(dst);
            }
            return Err(CopyFileError::Fatal(CANCELLED_ERR.to_string()));
        }
        let n =
            read.map_err(|e| CopyFileError::Recoverable(format!("Read error on {src}: {e}")))?;
        if n == 0 {
            break;
        }

        writer
            .write_all(&buf[..n])
            .map_err(|e| CopyFileError::Fatal(format!("Write error on {dst}: {e}")))?;

        stream_hasher.update(&buf[..n]);
        copied += n as u64;

        if copied == file_size || last_progress_at.elapsed() >= COPY_PROGRESS_INTERVAL {
            send_progress_sync(
                tx,
                job_id,
                total_files,
                completed_files,
                Some(src_name.to_string()),
                file_size,
                copied,
                total_bytes_total,
                total_bytes_completed + copied,
            );
            last_progress_at = Instant::now();
        }
    }

    writer
        .flush()
        .map_err(|e| CopyFileError::Fatal(format!("Flush error on {dst}: {e}")))?;
    drop(writer);

    let remote_digest = if resume_offset > 0 {
        src_driver.file_digest(src).map_err(|e| {
            if matches!(e, crate::vfs::VfsError::Cancelled) {
                CopyFileError::Fatal(CANCELLED_ERR.to_string())
            } else {
                CopyFileError::Verification(format!("Verification failed for {src}: {e}"))
            }
        })?
    } else {
        src_driver.transfer_digest(job_id, src).map_err(|e| {
            if matches!(e, crate::vfs::VfsError::Cancelled) {
                CopyFileError::Fatal(CANCELLED_ERR.to_string())
            } else {
                CopyFileError::Verification(format!("Verification failed for {src}: {e}"))
            }
        })?
    };

    if let Some(remote_digest) = remote_digest {
        send_progress_sync_with_status(
            tx,
            job_id,
            total_files,
            completed_files,
            Some(src_name.to_string()),
            file_size,
            copied,
            total_bytes_total,
            total_bytes_completed + copied,
            Some("Verifying transfer".to_string()),
        );
        let stream_digest = VfsTransferDigest {
            algorithm: "blake3".to_string(),
            size: copied,
            hash: stream_hasher.finalize().to_hex().to_string(),
        };
        let dest_digest = digest_driver_file(dst_driver, dst).map_err(|e| {
            CopyFileError::Verification(format!("Verification failed for {dst}: {e}"))
        })?;
        compare_transfer_digests(src, dst, &remote_digest, &stream_digest, &dest_digest)
            .map_err(CopyFileError::Verification)?;
    }

    if *cancel_rx.borrow() {
        return Err(CopyFileError::Fatal(CANCELLED_ERR.to_string()));
    }

    if save_digest_file {
        let digest = stream_hasher.finalize().to_hex().to_string();
        write_digest_sidecar(dst_driver, dst, src_name, &digest).map_err(CopyFileError::Fatal)?;
    }

    Ok(())
}

fn hash_existing_prefix(
    driver: &dyn VfsDriver,
    path: &str,
    len: u64,
) -> Result<blake3::Hasher, String> {
    let mut hasher = blake3::Hasher::new();
    if len == 0 {
        return Ok(hasher);
    }
    let mut reader = driver
        .open_read(path)
        .map_err(|e| format!("Cannot open {path} for resume hashing: {e}"))?;
    let mut remaining = len;
    let mut buf = vec![0u8; COPY_BUF_SIZE];
    while remaining > 0 {
        let want = (remaining as usize).min(buf.len());
        let n = reader
            .read(&mut buf[..want])
            .map_err(|e| format!("Read error hashing resume prefix {path}: {e}"))?;
        if n == 0 {
            return Err(format!(
                "Cannot resume {path}: destination ended before byte {len}"
            ));
        }
        hasher.update(&buf[..n]);
        remaining -= n as u64;
    }
    Ok(hasher)
}

enum VerificationOutcome {
    Retry,
    Skip,
    Abort,
}

fn resolve_verification_mismatch(
    src: &str,
    dst: &str,
    message: &str,
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    decision_rx: &mut DecisionRx,
) -> Result<VerificationOutcome, String> {
    send_event_sync(
        tx,
        job_id,
        "DecisionRequired",
        None,
        Some(ConflictDecisionSchema {
            kind: Some("verification".to_string()),
            source: src.to_string(),
            destination: dst.to_string(),
            message: Some(message.to_string()),
            options: vec!["retry".to_string(), "abort".to_string(), "skip".to_string()],
            supports_apply_to_all: false,
        }),
        Some(message.to_string()),
    );
    match decision_rx.blocking_recv() {
        Some(DecisionResponse { decision, .. }) => match decision.as_str() {
            "retry" => Ok(VerificationOutcome::Retry),
            "skip" => Ok(VerificationOutcome::Skip),
            _ => Ok(VerificationOutcome::Abort),
        },
        None => Ok(VerificationOutcome::Abort),
    }
}

fn digest_driver_file(driver: &dyn VfsDriver, path: &str) -> Result<VfsTransferDigest, String> {
    let mut reader = driver
        .open_read(path)
        .map_err(|e| format!("Cannot open for verification: {e}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; COPY_BUF_SIZE];
    let mut size = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Read error during verification: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size = size.saturating_add(n as u64);
    }
    Ok(VfsTransferDigest {
        algorithm: "blake3".to_string(),
        size,
        hash: hasher.finalize().to_hex().to_string(),
    })
}

fn write_digest_sidecar(
    driver: &dyn VfsDriver,
    dst: &str,
    file_name: &str,
    hash: &str,
) -> Result<(), String> {
    let sidecar_path = format!("{dst}.b3");
    let digest_name = if file_name.is_empty() {
        dst.rsplit(['/', '\\'])
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(dst)
    } else {
        file_name
    };
    let body = format!("{hash}  {digest_name}\n");
    let mut writer = driver
        .open_write(&sidecar_path)
        .map_err(|e| format!("Cannot create digest file {sidecar_path}: {e}"))?;
    writer
        .write_all(body.as_bytes())
        .map_err(|e| format!("Write error on digest file {sidecar_path}: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("Flush error on digest file {sidecar_path}: {e}"))?;
    Ok(())
}

fn compare_transfer_digests(
    src: &str,
    dst: &str,
    remote: &VfsTransferDigest,
    stream: &VfsTransferDigest,
    dest: &VfsTransferDigest,
) -> Result<(), String> {
    if remote.algorithm != "blake3" || stream.algorithm != "blake3" || dest.algorithm != "blake3" {
        return Err(format!(
            "Verification failed for {src}: unsupported digest algorithm"
        ));
    }
    if remote.size != stream.size || stream.size != dest.size {
        return Err(format!(
            "Verification failed for {src} -> {dst}: byte count mismatch (remote={}, streamed={}, destination={})",
            remote.size, stream.size, dest.size
        ));
    }
    if remote.hash != stream.hash || stream.hash != dest.hash {
        return Err(format!(
            "Verification failed for {src} -> {dst}: digest mismatch"
        ));
    }
    Ok(())
}

fn copy_dir_recursive(
    job_id: &str,
    src: &str,
    dst: &str,
    src_mtime: Option<u64>,
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
    decision_rx: &mut DecisionRx,
    apply_to_all: &mut Option<ConflictOutcome>,
    preserve_attributes: bool,
    verification_policy: &VerificationPolicy,
    save_digest_file: bool,
    auto_resume_copies: bool,
    skip_current_file: Arc<AtomicBool>,
    keep_partial_files_on_cancel: Arc<AtomicBool>,
) -> Result<(), String> {
    dst_driver
        .create_dir_all(dst)
        .map_err(|e| format!("Cannot create dir {dst}: {e}"))?;

    if preserve_attributes && dst_driver.is_local() {
        apply_file_attrs(dst, src_mtime, None);
    }

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
            return Err(CANCELLED_ERR.to_string());
        }

        let child_src = src_driver.resolve_child(src, &child.name);
        let child_dst = dst_driver.resolve_child(dst, &child.name);

        if child.is_dir {
            copy_dir_recursive(
                job_id,
                &child_src,
                &child_dst,
                child.mtime,
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
                decision_rx,
                apply_to_all,
                preserve_attributes,
                verification_policy,
                save_digest_file,
                auto_resume_copies,
                Arc::clone(&skip_current_file),
                Arc::clone(&keep_partial_files_on_cancel),
            )?;
        } else {
            let mut resume_requested = false;
            if child.is_symlink && src_driver.is_local() {
                let copied = copy_symlink(job_id, &child_src, &child_dst, conflict_policy,
                    tx, decision_rx, apply_to_all, src_driver, dst_driver)?;
                *completed_files += 1;
                send_progress_sync(tx, job_id, *total_files, *completed_files,
                    None, 0, 0, *total_bytes_total, *total_bytes_completed);
                if !copied { continue; }
                continue;
            }
            if dst_driver.exists_no_follow(&child_dst) {
                let destination_is_link = dst_driver.read_link(&child_dst).is_ok();
                let resume_candidate = (!destination_is_link).then(||
                    resume_candidate(&child_src, &child_dst, child.size, src_driver, dst_driver)
                ).flatten();
                match resolve_conflict(
                    conflict_policy,
                    &child_src,
                    &child_dst,
                    resume_candidate.as_ref(),
                    tx,
                    job_id,
                    decision_rx,
                    apply_to_all,
                )? {
                    ConflictOutcome::Overwrite => {
                        if destination_is_link {
                            dst_driver.remove_file(&child_dst).map_err(|error| format!("Cannot replace destination link {child_dst}: {error}"))?;
                        }
                    }
                    ConflictOutcome::Resume => {
                        resume_requested = true;
                    }
                    ConflictOutcome::Skip => {
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
                    ConflictOutcome::Cancel => {
                        return Err(CANCELLED_ERR.to_string());
                    }
                }
            }

            let copied = copy_file_with_progress(
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
                decision_rx,
                verification_policy,
                save_digest_file,
                auto_resume_copies,
                Arc::clone(&skip_current_file),
                resume_requested,
                Arc::clone(&keep_partial_files_on_cancel),
            )?;

            if copied && preserve_attributes && dst_driver.is_local() {
                apply_file_attrs(&child_dst, child.mtime, child.permissions.as_deref());
            }

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
                let dst = dd.resolve_child(dp, destination_name(entry));
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
                let dst = dd.resolve_child(dp, destination_name(entry));
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
                let child_kind = if child.is_dir { "directory" } else if child.is_symlink { "symlink" } else { "file" };
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
                let child_kind = if child.is_dir { "directory" } else if child.is_symlink { "symlink" } else { "file" };
                collect_cp_lines(
                    &child_src, &child_dst, child_kind, src_driver, dst_driver, lines,
                );
            }
        }
    } else {
        let preserve_link = if kind == "symlink" { "-P " } else { "" };
        lines.push(format!("cp {preserve_link}{} {}", shell_quote(src), shell_quote(dst)));
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
    send_progress_sync_with_status(
        tx,
        job_id,
        total_files,
        completed_files,
        current_file,
        current_file_bytes_total,
        current_file_bytes_copied,
        total_bytes_total,
        total_bytes_copied,
        None,
    );
}

fn send_progress_sync_with_status(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    total_files: usize,
    completed_files: usize,
    current_file: Option<String>,
    current_file_bytes_total: u64,
    current_file_bytes_copied: u64,
    total_bytes_total: u64,
    total_bytes_copied: u64,
    status_text: Option<String>,
) {
    let progress = FileOpProgressSchema {
        total_files,
        completed_files,
        current_file,
        status_text,
        current_file_bytes_total,
        current_file_bytes_copied,
        total_bytes_total,
        total_bytes_copied,
        current_file_percent: None,
        overall_percent: None,
    };
    let msg = ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: "Progress".to_string(),
        files: None,
        progress: Some(progress),
        search_progress: None,
        decision: None,
        error: None,
        result_path: None,
        result_panel_id: None,
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
        files: None,
        progress,
        search_progress: None,
        decision,
        error,
        result_path: None,
        result_panel_id: None,
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
        files: None,
        progress,
        search_progress: None,
        decision,
        error,
        result_path: None,
        result_panel_id: None,
    })
    .await
    .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::{DirChild, VfsEntry, VfsError};
    use crate::ws::protocol::{FilterSpec, ServerMessage, SortSpec};
    use std::any::Any;
    use std::fs;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("fbro-{name}-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[cfg(unix)]
    fn run_local_entry_op(op_type: FileOpType, src: &Path, kind: &str, dest: Option<&Path>) {
        let driver = crate::vfs::local::LocalFsDriver;
        let (tx, _rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let entry = FileOpEntry {
            name: src.file_name().unwrap().to_string_lossy().to_string(),
            dest_name: None,
            source_path: src.to_string_lossy().to_string(),
            size: None,
            kind: kind.into(),
            mtime: None,
            permissions: None,
        };
        let destination = dest.map(|path| path.to_string_lossy().to_string());
        run_file_op_blocking(
            "link-test", &op_type, &[entry], destination.as_deref(),
            &ConflictPolicy::Overwrite, &tx, &cancel_rx, &pause_rx,
            &mut decision_rx, &driver,
            dest.map(|_| &driver as &dyn VfsDriver), false,
            &VerificationPolicy::Fail, false, false,
            Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)),
        ).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn local_link_copy_move_rename_and_delete_preserve_target() {
        use std::os::unix::fs::symlink;
        let dir = TestDir::new("link-operations");
        let src_dir = dir.path.join("src");
        let copy_dir = dir.path.join("copy");
        let move_dir = dir.path.join("move");
        for path in [&src_dir, &copy_dir, &move_dir] { fs::create_dir_all(path).unwrap(); }
        let target = src_dir.join("target.txt");
        fs::write(&target, "target contents").unwrap();
        let link = src_dir.join("link.txt");
        symlink("target.txt", &link).unwrap();
        run_local_entry_op(FileOpType::Copy, &link, "symlink", Some(&copy_dir));
        let copied = copy_dir.join("link.txt");
        assert_eq!(fs::read_link(&copied).unwrap(), PathBuf::from("target.txt"));
        assert!(fs::symlink_metadata(&copied).unwrap().is_symlink());
        assert!(!copied.exists()); // Stored relative target text is unchanged.
        let renamed = copy_dir.join("renamed.txt");
        assert!(crate::vfs::local::LocalFsDriver.try_rename(&copied.to_string_lossy(), &renamed.to_string_lossy()).unwrap());
        assert_eq!(fs::read_link(&renamed).unwrap(), PathBuf::from("target.txt"));
        run_local_entry_op(FileOpType::Move, &link, "symlink", Some(&move_dir));
        assert!(fs::symlink_metadata(&link).is_err());
        assert_eq!(fs::read_link(move_dir.join("link.txt")).unwrap(), PathBuf::from("target.txt"));
        run_local_entry_op(FileOpType::Delete, &move_dir.join("link.txt"), "symlink", None);
        assert_eq!(fs::read_to_string(target).unwrap(), "target contents");
    }

    #[cfg(unix)]
    #[test]
    fn directory_copy_keeps_nested_cyclic_link_without_traversal() {
        use std::os::unix::fs::symlink;
        let dir = TestDir::new("nested-link-copy");
        let src = dir.path.join("src");
        let dst = dir.path.join("dst");
        fs::create_dir_all(src.join("folder")).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(src.join("folder/note.txt"), "payload").unwrap();
        symlink(".", src.join("folder/loop")).unwrap();
        run_local_entry_op(FileOpType::Copy, &src.join("folder"), "directory", Some(&dst));
        assert_eq!(fs::read_to_string(dst.join("folder/note.txt")).unwrap(), "payload");
        assert_eq!(fs::read_link(dst.join("folder/loop")).unwrap(), PathBuf::from("."));
    }

    #[cfg(unix)]
    #[test]
    fn file_overwrite_replaces_broken_link_without_creating_its_target() {
        use std::os::unix::fs::symlink;
        let dir = TestDir::new("broken-destination-link");
        let src_dir = dir.path.join("src");
        let dst_dir = dir.path.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();
        let src = src_dir.join("file.txt");
        fs::write(&src, "payload").unwrap();
        symlink("missing.txt", dst_dir.join("file.txt")).unwrap();
        run_local_entry_op(FileOpType::Copy, &src, "file", Some(&dst_dir));
        assert_eq!(fs::read_to_string(dst_dir.join("file.txt")).unwrap(), "payload");
        assert!(!fs::symlink_metadata(dst_dir.join("file.txt")).unwrap().is_symlink());
        assert!(!dst_dir.join("missing.txt").exists());
    }

    struct FlakyReadDriver {
        fail_after: Option<u64>,
        failed: Arc<AtomicBool>,
        cancel_reads: bool,
        cancel_on_open: bool,
        opens: std::sync::atomic::AtomicUsize,
    }

    struct FlakyReader {
        file: fs::File,
        pos: u64,
        fail_at: Option<u64>,
        failed: Arc<AtomicBool>,
        cancel_reads: bool,
    }

    impl Read for FlakyReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if let Some(fail_at) = self.fail_at {
                if self.pos >= fail_at {
                    self.failed.store(true, Ordering::SeqCst);
                    if self.cancel_reads {
                        return Err(std::io::Error::other(VfsError::Cancelled));
                    }
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "test timeout",
                    ));
                }
                let allowed = (fail_at - self.pos) as usize;
                let want = allowed.min(buf.len());
                let n = self.file.read(&mut buf[..want])?;
                self.pos += n as u64;
                return Ok(n);
            }
            let n = self.file.read(buf)?;
            self.pos += n as u64;
            Ok(n)
        }
    }

    impl FlakyReadDriver {
        fn new(fail_after: Option<u64>) -> Self {
            Self {
                fail_after,
                failed: Arc::new(AtomicBool::new(false)),
                cancel_reads: false,
                cancel_on_open: false,
                opens: std::sync::atomic::AtomicUsize::new(0),
            }
        }

        fn digest_path(path: &str) -> Result<VfsTransferDigest, VfsError> {
            let mut file = fs::File::open(path).map_err(|e| VfsError::IOError(e.to_string()))?;
            let mut hasher = blake3::Hasher::new();
            let mut buf = vec![0u8; COPY_BUF_SIZE];
            let mut size = 0u64;
            loop {
                let n = file
                    .read(&mut buf)
                    .map_err(|e| VfsError::IOError(e.to_string()))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                size += n as u64;
            }
            Ok(VfsTransferDigest {
                algorithm: "blake3".to_string(),
                size,
                hash: hasher.finalize().to_hex().to_string(),
            })
        }
    }

    impl VfsDriver for FlakyReadDriver {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn list(
            &self,
            _path: &str,
            _sort: &SortSpec,
            _filter: Option<&FilterSpec>,
            _show_hidden: bool,
        ) -> Result<Vec<VfsEntry>, VfsError> {
            Ok(Vec::new())
        }

        fn resolve_child(&self, base: &str, name: &str) -> String {
            Path::new(base).join(name).to_string_lossy().to_string()
        }

        fn parent_path(&self, path: &str) -> Option<String> {
            Path::new(path)
                .parent()
                .map(|path| path.to_string_lossy().to_string())
        }

        fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
            self.open_read_at(path, 0)
        }

        fn supports_read_at(&self) -> bool {
            true
        }

        fn open_read_at(&self, path: &str, offset: u64) -> Result<Box<dyn Read + Send>, VfsError> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            if self.cancel_on_open {
                return Err(VfsError::Cancelled);
            }
            let mut file = fs::File::open(path).map_err(|e| VfsError::IOError(e.to_string()))?;
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| VfsError::IOError(e.to_string()))?;
            let fail_at = self
                .fail_after
                .filter(|fail_after| !self.failed.load(Ordering::SeqCst) && offset < *fail_after);
            Ok(Box::new(FlakyReader {
                file,
                pos: offset,
                fail_at,
                failed: self.failed.clone(),
                cancel_reads: self.cancel_reads,
            }))
        }

        fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
            Err(VfsError::NotSupported(
                "test source is read-only".to_string(),
            ))
        }

        fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
            Err(VfsError::NotSupported(
                "test source is read-only".to_string(),
            ))
        }

        fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
            Err(VfsError::NotSupported(
                "test source is read-only".to_string(),
            ))
        }

        fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
            Err(VfsError::NotSupported(
                "test source is read-only".to_string(),
            ))
        }

        fn exists(&self, path: &str) -> bool {
            Path::new(path).exists()
        }

        fn file_size(&self, path: &str) -> Option<u64> {
            fs::metadata(path).ok().map(|meta| meta.len())
        }

        fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> {
            Ok(Vec::new())
        }

        fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
            let mut file = fs::File::open(path).map_err(|e| VfsError::IOError(e.to_string()))?;
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| VfsError::IOError(e.to_string()))?;
            let mut buf = vec![0u8; size];
            let n = file
                .read(&mut buf)
                .map_err(|e| VfsError::IOError(e.to_string()))?;
            buf.truncate(n);
            Ok(buf)
        }

        fn file_digest(&self, path: &str) -> Result<Option<VfsTransferDigest>, VfsError> {
            Ok(Some(Self::digest_path(path)?))
        }
    }

    fn bytes(len: usize) -> Vec<u8> {
        (0..len).map(|i| ((i * 31 + 7) % 251) as u8).collect()
    }

    #[test]
    fn remote_cancellation_never_retries_or_prompts_and_honors_partial_policy() {
        let dir = TestDir::new("remote-cancel");
        let data = bytes(2 * 1024 * 1024 + 113);
        let src = dir.path.join("source.bin");
        fs::write(&src, &data).unwrap();
        for (resume, keep, cancel_on_open) in [
            (false, false, false),
            (false, true, false),
            (true, false, false),
            (false, false, true),
        ] {
            let dst = dir
                .path
                .join(format!("dest-{resume}-{keep}-{cancel_on_open}.bin"));
            if resume {
                fs::write(&dst, &data[..1536 * 1024]).unwrap();
            }
            let mut driver = FlakyReadDriver::new(Some(1152 * 1024));
            driver.cancel_reads = true;
            driver.cancel_on_open = cancel_on_open;
            let (tx, mut rx) = mpsc::channel(64);
            let (_cancel_tx, cancel_rx) = watch::channel(false);
            let (_pause_tx, pause_rx) = watch::channel(false);
            let (_decision_tx, mut decision_rx) = mpsc::channel(1);
            let err = copy_file_with_progress(
                "job-test",
                &src.to_string_lossy(),
                &dst.to_string_lossy(),
                data.len() as u64,
                &tx,
                &cancel_rx,
                &pause_rx,
                1,
                0,
                data.len() as u64,
                0,
                &driver,
                &crate::vfs::local::LocalFsDriver,
                &mut decision_rx,
                &VerificationPolicy::Prompt,
                false,
                true,
                Arc::new(AtomicBool::new(false)),
                resume,
                Arc::new(AtomicBool::new(keep)),
            )
            .unwrap_err();
            assert_eq!(err, CANCELLED_ERR);
            assert_eq!(
                driver.opens.load(Ordering::SeqCst),
                1,
                "cancellation must bypass automatic resume"
            );
            assert_eq!(dst.exists(), resume || keep);
            if dst.exists() {
                assert_eq!(fs::read(&dst).unwrap(), data[..1152 * 1024]);
            }
            while let Ok(msg) = rx.try_recv() {
                if let ServerMessage::JobEvent {
                    event, progress, ..
                } = msg
                {
                    assert_ne!(event, "DecisionRequired");
                    assert!(!progress
                        .and_then(|p| p.status_text)
                        .is_some_and(|s| s.contains("Resuming transfer after read error")));
                }
            }
        }
    }

    #[test]
    fn auto_resume_recovers_after_read_timeout() {
        let dir = TestDir::new("auto-resume");
        let src = dir.path.join("source.bin");
        let dst = dir.path.join("dest.bin");
        let data = bytes(2 * 1024 * 1024 + 17);
        fs::write(&src, &data).unwrap();

        let src_driver = FlakyReadDriver::new(Some(1536 * 1024));
        let dst_driver = crate::vfs::local::LocalFsDriver;
        let (tx, _rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let copied = copy_file_with_progress(
            "job-test",
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            data.len() as u64,
            &tx,
            &cancel_rx,
            &pause_rx,
            1,
            0,
            data.len() as u64,
            0,
            &src_driver,
            &dst_driver,
            &mut decision_rx,
            &VerificationPolicy::Fail,
            false,
            true,
            Arc::new(AtomicBool::new(false)),
            false,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(copied);
        assert!(src_driver.failed.load(Ordering::SeqCst));
        assert_eq!(fs::read(dst).unwrap(), data);
    }

    #[test]
    fn manual_resume_uses_existing_partial_destination() {
        let dir = TestDir::new("manual-resume");
        let src = dir.path.join("source.bin");
        let dst = dir.path.join("dest.bin");
        let data = bytes(2 * 1024 * 1024 + 113);
        fs::write(&src, &data).unwrap();
        fs::write(&dst, &data[..1536 * 1024]).unwrap();

        let src_driver = FlakyReadDriver::new(None);
        let dst_driver = crate::vfs::local::LocalFsDriver;
        let (tx, _rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let copied = copy_file_with_progress(
            "job-test",
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            data.len() as u64,
            &tx,
            &cancel_rx,
            &pause_rx,
            1,
            0,
            data.len() as u64,
            0,
            &src_driver,
            &dst_driver,
            &mut decision_rx,
            &VerificationPolicy::Fail,
            false,
            true,
            Arc::new(AtomicBool::new(false)),
            true,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(copied);
        assert_eq!(fs::read(dst).unwrap(), data);
    }

    #[test]
    fn cancelling_resumed_copy_preserves_partial_destination() {
        let dir = TestDir::new("cancel-resume");
        let src = dir.path.join("source.bin");
        let dst = dir.path.join("dest.bin");
        let data = bytes(2 * 1024 * 1024 + 113);
        let partial_len = 1536 * 1024;
        fs::write(&src, &data).unwrap();
        fs::write(&dst, &data[..partial_len]).unwrap();

        let src_driver = FlakyReadDriver::new(None);
        let dst_driver = crate::vfs::local::LocalFsDriver;
        let (tx, _rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(true);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let err = copy_file_with_progress(
            "job-test",
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            data.len() as u64,
            &tx,
            &cancel_rx,
            &pause_rx,
            1,
            0,
            data.len() as u64,
            0,
            &src_driver,
            &dst_driver,
            &mut decision_rx,
            &VerificationPolicy::Fail,
            false,
            true,
            Arc::new(AtomicBool::new(false)),
            true,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap_err();

        assert_eq!(err, CANCELLED_ERR);
        assert!(dst.exists());
        assert_eq!(fs::metadata(&dst).unwrap().len(), 512 * 1024);
        assert_eq!(fs::read(dst).unwrap(), data[..512 * 1024]);
    }

    #[test]
    fn cancelling_fresh_copy_honors_keep_partial_choice() {
        let dir = TestDir::new("cancel-keep-partial");
        let src = dir.path.join("source.bin");
        let dst_delete = dir.path.join("dest-delete.bin");
        let dst_keep = dir.path.join("dest-keep.bin");
        let data = bytes(1024);
        fs::write(&src, &data).unwrap();

        let src_driver = FlakyReadDriver::new(None);
        let dst_driver = crate::vfs::local::LocalFsDriver;
        let (tx, _rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(true);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);

        let err = copy_file_with_progress(
            "job-test",
            &src.to_string_lossy(),
            &dst_delete.to_string_lossy(),
            data.len() as u64,
            &tx,
            &cancel_rx,
            &pause_rx,
            1,
            0,
            data.len() as u64,
            0,
            &src_driver,
            &dst_driver,
            &mut decision_rx,
            &VerificationPolicy::Fail,
            false,
            true,
            Arc::new(AtomicBool::new(false)),
            false,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap_err();
        assert_eq!(err, CANCELLED_ERR);
        assert!(!dst_delete.exists());

        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let err = copy_file_with_progress(
            "job-test",
            &src.to_string_lossy(),
            &dst_keep.to_string_lossy(),
            data.len() as u64,
            &tx,
            &cancel_rx,
            &pause_rx,
            1,
            0,
            data.len() as u64,
            0,
            &src_driver,
            &dst_driver,
            &mut decision_rx,
            &VerificationPolicy::Fail,
            false,
            true,
            Arc::new(AtomicBool::new(false)),
            false,
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap_err();
        assert_eq!(err, CANCELLED_ERR);
        assert!(dst_keep.exists());
    }

    #[test]
    fn skip_fresh_copy_removes_partial_destination_and_emits_event() {
        let dir = TestDir::new("skip-fresh");
        let src = dir.path.join("source.bin");
        let dst = dir.path.join("dest.bin");
        let data = bytes(1024);
        fs::write(&src, &data).unwrap();
        fs::write(&dst, b"old partial").unwrap();

        let src_driver = FlakyReadDriver::new(None);
        let dst_driver = crate::vfs::local::LocalFsDriver;
        let (tx, mut rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let skip_current_file = Arc::new(AtomicBool::new(true));

        let copied = copy_file_with_progress(
            "job-test",
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            data.len() as u64,
            &tx,
            &cancel_rx,
            &pause_rx,
            1,
            0,
            data.len() as u64,
            0,
            &src_driver,
            &dst_driver,
            &mut decision_rx,
            &VerificationPolicy::Fail,
            false,
            false,
            Arc::clone(&skip_current_file),
            false,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(!copied);
        assert!(!skip_current_file.load(Ordering::SeqCst));
        assert!(!dst.exists());
        assert_eq!(skipped_event_file_name(&mut rx).as_deref(), Some("source.bin"));
    }

    #[test]
    fn skip_resumed_copy_preserves_existing_partial_destination() {
        let dir = TestDir::new("skip-resume");
        let src = dir.path.join("source.bin");
        let dst = dir.path.join("dest.bin");
        let data = bytes(2 * 1024 * 1024 + 113);
        let partial_len = 1536 * 1024;
        fs::write(&src, &data).unwrap();
        fs::write(&dst, &data[..partial_len]).unwrap();

        let src_driver = FlakyReadDriver::new(None);
        let dst_driver = crate::vfs::local::LocalFsDriver;
        let (tx, _rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let skip_current_file = Arc::new(AtomicBool::new(true));

        let copied = copy_file_with_progress(
            "job-test",
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            data.len() as u64,
            &tx,
            &cancel_rx,
            &pause_rx,
            1,
            0,
            data.len() as u64,
            0,
            &src_driver,
            &dst_driver,
            &mut decision_rx,
            &VerificationPolicy::Fail,
            false,
            false,
            Arc::clone(&skip_current_file),
            true,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(!copied);
        assert!(!skip_current_file.load(Ordering::SeqCst));
        assert_eq!(fs::metadata(&dst).unwrap().len(), partial_len as u64);
        assert_eq!(fs::read(dst).unwrap(), data[..partial_len]);
    }

    #[test]
    fn skip_current_file_continues_to_next_file() {
        let dir = TestDir::new("skip-continues");
        let src_dir = dir.path.join("src");
        let dst_dir = dir.path.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();
        let first = src_dir.join("first.bin");
        let second = src_dir.join("second.bin");
        fs::write(&first, b"first").unwrap();
        fs::write(&second, b"second").unwrap();

        let driver = crate::vfs::local::LocalFsDriver;
        let (tx, mut rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let skip_current_file = Arc::new(AtomicBool::new(true));
        let entries = vec![
            FileOpEntry {
                name: "first.bin".to_string(),
                dest_name: None,
                source_path: first.to_string_lossy().to_string(),
                size: Some(5),
                kind: "file".to_string(),
                mtime: None,
                permissions: None,
            },
            FileOpEntry {
                name: "second.bin".to_string(),
                dest_name: None,
                source_path: second.to_string_lossy().to_string(),
                size: Some(6),
                kind: "file".to_string(),
                mtime: None,
                permissions: None,
            },
        ];

        run_file_op_blocking(
            "job-test",
            &FileOpType::Copy,
            &entries,
            Some(&dst_dir.to_string_lossy()),
            &ConflictPolicy::Overwrite,
            &tx,
            &cancel_rx,
            &pause_rx,
            &mut decision_rx,
            &driver,
            Some(&driver),
            false,
            &VerificationPolicy::Fail,
            false,
            false,
            Arc::clone(&skip_current_file),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(!skip_current_file.load(Ordering::SeqCst));
        assert!(!dst_dir.join("first.bin").exists());
        assert_eq!(fs::read_to_string(dst_dir.join("second.bin")).unwrap(), "second");
        assert_eq!(skipped_event_file_name(&mut rx).as_deref(), Some("first.bin"));
    }

    fn skipped_event_file_name(rx: &mut mpsc::Receiver<ServerMessage>) -> Option<String> {
        while let Ok(message) = rx.try_recv() {
            if let ServerMessage::JobEvent { event, error, .. } = message {
                if event == "FileSkipped" {
                    return error;
                }
            }
        }
        None
    }

    #[test]
    fn copy_single_file_uses_destination_name() {
        let dir = TestDir::new("copy-dest-name-file");
        let src_dir = dir.path.join("src");
        let dst_dir = dir.path.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();
        let src = src_dir.join("source.txt");
        fs::write(&src, "payload\n").unwrap();

        let driver = crate::vfs::local::LocalFsDriver;
        let (tx, _rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let entry = FileOpEntry {
            name: "source.txt".to_string(),
            dest_name: Some("renamed.txt".to_string()),
            source_path: src.to_string_lossy().to_string(),
            size: Some(8),
            kind: "file".to_string(),
            mtime: None,
            permissions: None,
        };

        run_file_op_blocking(
            "job-test",
            &FileOpType::Copy,
            &[entry],
            Some(&dst_dir.to_string_lossy()),
            &ConflictPolicy::Overwrite,
            &tx,
            &cancel_rx,
            &pause_rx,
            &mut decision_rx,
            &driver,
            Some(&driver),
            false,
            &VerificationPolicy::Fail,
            false,
            false,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(dst_dir.join("renamed.txt")).unwrap(),
            "payload\n"
        );
        assert!(src.exists());
        assert!(!dst_dir.join("source.txt").exists());
    }

    #[test]
    fn copy_single_directory_uses_destination_name() {
        let dir = TestDir::new("copy-dest-name-dir");
        let src_dir = dir.path.join("src");
        let dst_dir = dir.path.join("dst");
        fs::create_dir_all(src_dir.join("folder/nested")).unwrap();
        fs::create_dir_all(&dst_dir).unwrap();
        fs::write(src_dir.join("folder/nested/file.txt"), "payload\n").unwrap();

        let driver = crate::vfs::local::LocalFsDriver;
        let (tx, _rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let entry = FileOpEntry {
            name: "folder".to_string(),
            dest_name: Some("renamed-folder".to_string()),
            source_path: src_dir.join("folder").to_string_lossy().to_string(),
            size: None,
            kind: "directory".to_string(),
            mtime: None,
            permissions: None,
        };

        run_file_op_blocking(
            "job-test",
            &FileOpType::Copy,
            &[entry],
            Some(&dst_dir.to_string_lossy()),
            &ConflictPolicy::Overwrite,
            &tx,
            &cancel_rx,
            &pause_rx,
            &mut decision_rx,
            &driver,
            Some(&driver),
            false,
            &VerificationPolicy::Fail,
            false,
            false,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(dst_dir.join("renamed-folder/nested/file.txt")).unwrap(),
            "payload\n"
        );
        assert!(src_dir.join("folder/nested/file.txt").exists());
        assert!(!dst_dir.join("folder").exists());
    }

    #[test]
    fn copy_rejects_exact_self_copy() {
        let dir = TestDir::new("copy-self");
        let src = dir.path.join("same.txt");
        fs::write(&src, "payload\n").unwrap();

        let driver = crate::vfs::local::LocalFsDriver;
        let (tx, _rx) = mpsc::channel(64);
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let (_decision_tx, mut decision_rx) = mpsc::channel(1);
        let entry = FileOpEntry {
            name: "same.txt".to_string(),
            dest_name: None,
            source_path: src.to_string_lossy().to_string(),
            size: Some(8),
            kind: "file".to_string(),
            mtime: None,
            permissions: None,
        };

        let err = run_file_op_blocking(
            "job-test",
            &FileOpType::Copy,
            &[entry],
            Some(&dir.path.to_string_lossy()),
            &ConflictPolicy::Overwrite,
            &tx,
            &cancel_rx,
            &pause_rx,
            &mut decision_rx,
            &driver,
            Some(&driver),
            false,
            &VerificationPolicy::Fail,
            false,
            false,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap_err();

        assert!(err.contains("onto itself"));
        assert_eq!(fs::read_to_string(src).unwrap(), "payload\n");
    }
}
