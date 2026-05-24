use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

use crate::jobs::{
    ConflictPolicy, DecisionResponse, DecisionRx, FileOpEntry, FileOpType, VerificationPolicy,
};
use crate::vfs::{VfsDriver, VfsTransferDigest};
use crate::ws::protocol::{now, ConflictDecisionSchema, FileOpProgressSchema, ServerMessage};

const COPY_BUF_SIZE: usize = 256 * 1024; // 256 KiB
const COPY_PROGRESS_INTERVAL: Duration = Duration::from_millis(80);

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
) {
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
        )
    })
    .await;

    match result {
        Ok(Ok(())) => {
            let _ = tx_event(&tx, &job_id, "Completed", None, None, None).await;
        }
        Ok(Err(err)) => {
            if err == CANCELLED_ERR {
                let _ = tx_event(&tx, &job_id, "Cancelled", None, None, None).await;
            } else {
                let _ = tx_event(&tx, &job_id, "Failed", None, None, Some(err)).await;
            }
        }
        Err(_) => {
            let _ = tx_event(&tx, &job_id, "Cancelled", None, None, None).await;
        }
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

                // For move with same driver: only use O(1) rename when the
                // destination entry does not already exist. Existing targets
                // must go through the regular conflict-aware copy+delete path.
                if *op_type == FileOpType::Move && same_driver && !dst_drv.exists(&dst_str) {
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
                            &mut apply_to_all,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConflictOutcome {
    Overwrite,
    Skip,
    Cancel,
}

fn apply_all_outcome(policy: Option<&ConflictPolicy>) -> Option<ConflictOutcome> {
    match policy {
        Some(ConflictPolicy::Overwrite) => Some(ConflictOutcome::Overwrite),
        Some(ConflictPolicy::Skip) => Some(ConflictOutcome::Skip),
        _ => None,
    }
}

fn resolve_conflict(
    policy: &ConflictPolicy,
    src: &str,
    dst: &str,
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    decision_rx: &mut DecisionRx,
    apply_to_all: &mut Option<ConflictOutcome>,
) -> Result<ConflictOutcome, String> {
    match policy {
        ConflictPolicy::Skip => Ok(ConflictOutcome::Skip),
        ConflictPolicy::Overwrite => Ok(ConflictOutcome::Overwrite),
        ConflictPolicy::Ask => {
            if let Some(outcome) = *apply_to_all {
                return Ok(outcome);
            }
            send_event_sync(
                tx,
                job_id,
                "DecisionRequired",
                None,
                Some(ConflictDecisionSchema {
                    kind: Some("conflict".to_string()),
                    source: src.to_string(),
                    destination: dst.to_string(),
                    message: None,
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
            match decision {
                Some(DecisionResponse {
                    decision,
                    apply_to_all: true,
                }) => {
                    let outcome = match decision.as_str() {
                        "overwrite" => ConflictOutcome::Overwrite,
                        "skip" => ConflictOutcome::Skip,
                        "cancel" => ConflictOutcome::Cancel,
                        _ => ConflictOutcome::Skip,
                    };
                    if matches!(outcome, ConflictOutcome::Overwrite | ConflictOutcome::Skip) {
                        *apply_to_all = Some(outcome);
                    }
                    Ok(outcome)
                }
                Some(DecisionResponse { decision, .. }) => match decision.as_str() {
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
) -> Result<bool, String> {
    let src_name = Path::new(src)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

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
        ) {
            Ok(()) => return Ok(true),
            Err(CopyFileError::Fatal(e)) => return Err(e),
            Err(CopyFileError::Verification(e)) => {
                if *verification_policy == VerificationPolicy::Fail {
                    return Err(e);
                }
                match resolve_verification_mismatch(src, dst, &e, tx, job_id, decision_rx)? {
                    VerificationOutcome::Retry => {
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
) -> Result<(), CopyFileError> {
    let mut reader = src_driver
        .open_read(src)
        .map_err(|e| CopyFileError::Fatal(format!("Cannot open {src}: {e}")))?;

    let file_size = if total_bytes > 0 {
        total_bytes
    } else {
        src_driver.file_size(src).unwrap_or(0)
    };

    let mut writer = dst_driver
        .open_write(dst)
        .map_err(|e| CopyFileError::Fatal(format!("Cannot create {dst}: {e}")))?;

    let mut buf = vec![0u8; COPY_BUF_SIZE];
    let mut copied: u64 = 0;
    let mut stream_hasher = blake3::Hasher::new();
    let mut last_progress_at = Instant::now();
    loop {
        if wait_if_paused(pause_rx, cancel_rx) {
            drop(writer);
            let _ = dst_driver.remove_file(dst);
            return Err(CopyFileError::Fatal(CANCELLED_ERR.to_string()));
        }

        let n = reader
            .read(&mut buf)
            .map_err(|e| CopyFileError::Fatal(format!("Read error on {src}: {e}")))?;
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

    if let Some(remote_digest) = src_driver
        .transfer_digest(job_id, src)
        .map_err(|e| CopyFileError::Verification(format!("Verification failed for {src}: {e}")))?
    {
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

    if save_digest_file {
        let digest = stream_hasher.finalize().to_hex().to_string();
        write_digest_sidecar(dst_driver, dst, src_name, &digest).map_err(CopyFileError::Fatal)?;
    }

    Ok(())
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
            )?;
        } else {
            if dst_driver.exists(&child_dst) {
                match resolve_conflict(
                    conflict_policy,
                    &child_src,
                    &child_dst,
                    tx,
                    job_id,
                    decision_rx,
                    apply_to_all,
                )? {
                    ConflictOutcome::Overwrite => {}
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
