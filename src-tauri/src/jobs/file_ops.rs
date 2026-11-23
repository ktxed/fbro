use std::io::{Read, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

use crate::jobs::{
    ConflictPolicy, DecisionResponse, DecisionRx, ErrorPolicy, FileOpEntry,
    FileOpType, SkippedItem, VerificationPolicy,
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
    error_policy: ErrorPolicy,
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
    // Remote results are captured with peer/files/progress at the authoritative
    // job transition. Retain the existing single result for local operations.
    let remote = src_driver.as_any().is::<crate::vfs::sftp::SftpFsDriver>()
        || dst_driver.as_ref().is_some_and(|driver| driver.as_any().is::<crate::vfs::sftp::SftpFsDriver>());
    let job_id_clone = job_id.clone();
    let tx_clone = tx.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _driver_ctx = FileOpDriverContext::new(&*src_driver, &job_id_clone);
        let ctx = FileOpCtx {
            job_id: &job_id_clone,
            op_type: &op_type,
            files: &files,
            dest_path: dest_path.as_deref(),
            tx: &tx_clone,
            cancel_rx: &cancel_rx,
            pause_rx: &pause_rx,
            src_driver: &*src_driver,
            dst_driver: dst_driver.as_deref(),
            conflict_policy: &conflict_policy,
            error_policy: &error_policy,
            verification_policy: &verification_policy,
            preserve_attributes,
            save_digest_file,
            auto_resume_copies,
            skip_current_file,
            keep_partial_files_on_cancel,
        };
        let mut st = FileOpState::new(&mut decision_rx);
        let result = run_file_op_blocking(&ctx, &mut st);
        (result, st.skipped_count)
    })
    .await;

    let (result, skipped_count) = result.unwrap_or((Err(CANCELLED_ERR.to_string()), 0));
    match result {
        Ok(()) => {
            if !remote {
                crate::journal::record_operation_finished(&job_id, op_name, "completed", None, skipped_count);
            }
            let _ = tx_event(&tx, &job_id, "Completed", None, None, None).await;
        }
        Err(err) if err == CANCELLED_ERR => {
            if !remote {
                crate::journal::record_operation_finished(&job_id, op_name, "cancelled", None, skipped_count);
            }
            let _ = tx_event(&tx, &job_id, "Cancelled", None, None, None).await;
        }
        Err(err) => {
            if !remote {
                crate::journal::record_operation_finished(&job_id, op_name, "failed", Some(&err), skipped_count);
            }
            let _ = tx_event(&tx, &job_id, "Failed", None, None, Some(err)).await;
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

/// Values that stay constant for the lifetime of one file operation bundle.
struct FileOpCtx<'a> {
    job_id: &'a str,
    op_type: &'a FileOpType,
    files: &'a [FileOpEntry],
    dest_path: Option<&'a str>,
    tx: &'a mpsc::Sender<ServerMessage>,
    cancel_rx: &'a watch::Receiver<bool>,
    pause_rx: &'a watch::Receiver<bool>,
    src_driver: &'a dyn VfsDriver,
    dst_driver: Option<&'a dyn VfsDriver>,
    conflict_policy: &'a ConflictPolicy,
    error_policy: &'a ErrorPolicy,
    verification_policy: &'a VerificationPolicy,
    preserve_attributes: bool,
    save_digest_file: bool,
    auto_resume_copies: bool,
    skip_current_file: Arc<AtomicBool>,
    keep_partial_files_on_cancel: Arc<AtomicBool>,
}

/// Decisions and counters that change while the operation runs.
struct FileOpState<'a> {
    decision_rx: &'a mut DecisionRx,
    apply_to_all: Option<ConflictOutcome>,
    skip_all_errors: bool,
    total_files: usize,
    completed_files: usize,
    total_bytes_total: u64,
    total_bytes_completed: u64,
    /// Every skipped item, uncapped. The per-item list lives on the job.
    skipped_count: usize,
}

impl<'a> FileOpState<'a> {
    fn new(decision_rx: &'a mut DecisionRx) -> Self {
        Self {
            decision_rx,
            apply_to_all: None,
            skip_all_errors: false,
            total_files: 0,
            completed_files: 0,
            total_bytes_total: 0,
            total_bytes_completed: 0,
            skipped_count: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ErrorOutcome {
    Retry,
    /// `notify` is true only for an explicit Skip choice; skip-all and the
    /// Skip policy are silent.
    Skip { notify: bool },
    Cancel,
}

/// Run `op`, prompting on failure and repeating it when the user picks Retry.
/// A skipped item is recorded and reported; `None` means "not transferred".
fn attempt<T>(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    dst: &str,
    op: impl Fn() -> Result<T, String>,
) -> Result<Option<T>, String> {
    loop {
        match op() {
            Ok(value) => return Ok(Some(value)),
            Err(err) => {
                if err == CANCELLED_ERR || *ctx.cancel_rx.borrow() {
                    return Err(CANCELLED_ERR.to_string());
                }
                match resolve_error(ctx, st, src, dst, &err) {
                    ErrorOutcome::Retry => {
                        if wait_if_paused(ctx.pause_rx, ctx.cancel_rx) {
                            return Err(CANCELLED_ERR.to_string());
                        }
                        continue;
                    }
                    ErrorOutcome::Skip { notify } => {
                        record_skip(ctx, st, src, err, notify);
                        return Ok(None);
                    }
                    ErrorOutcome::Cancel => return Err(CANCELLED_ERR.to_string()),
                }
            }
        }
    }
}

fn resolve_error(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    source: &str,
    destination: &str,
    message: &str,
) -> ErrorOutcome {
    if *ctx.error_policy == ErrorPolicy::Skip || st.skip_all_errors {
        return ErrorOutcome::Skip { notify: false };
    }
    send_event_sync(
        ctx.tx,
        ctx.job_id,
        "DecisionRequired",
        None,
        Some(ConflictDecisionSchema {
            kind: Some("error".to_string()),
            source: source.to_string(),
            destination: destination.to_string(),
            message: Some(message.to_string()),
            options: vec![
                "retry".to_string(),
                "skip".to_string(),
                "skip-all".to_string(),
                "cancel".to_string(),
            ],
            supports_apply_to_all: false,
        }),
        None,
        None,
    );
    match st.decision_rx.blocking_recv() {
        Some(DecisionResponse { decision, .. }) => match decision.as_str() {
            "retry" => ErrorOutcome::Retry,
            "skip" => ErrorOutcome::Skip { notify: true },
            "skip-all" => {
                st.skip_all_errors = true;
                ErrorOutcome::Skip { notify: false }
            }
            _ => ErrorOutcome::Cancel,
        },
        None => ErrorOutcome::Cancel,
    }
}

/// Record a skipped item and notify the client. Callers update the progress
/// counters so a skipped item advances the job like a transferred one.
fn record_skip(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    path: &str,
    reason: impl Into<String>,
    notify: bool,
) {
    let item = SkippedItem {
        path: path.to_string(),
        reason: reason.into(),
        op_type: file_op_name(ctx.op_type).to_string(),
        notify,
    };
    let file_name = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());
    send_event_sync(
        ctx.tx,
        ctx.job_id,
        "FileSkipped",
        None,
        None,
        Some(file_name),
        Some(item),
    );
    st.skipped_count += 1;
}

/// A conflict Skip is reported too. Only a Skip answered in the prompt for
/// this one item notifies; the policy and Apply to all are silent, like
/// Skip all for errors.
fn record_conflict_skip(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    dst: &str,
) {
    let notify = *ctx.conflict_policy == ConflictPolicy::Ask && st.apply_to_all.is_none();
    record_skip(
        ctx,
        st,
        src,
        format!("Destination already exists: {dst}"),
        notify,
    );
}

/// Remove a source file after a complete transfer. Removal failures are
/// skippable, but the source is never removed when its copy was skipped.
fn remove_source_file(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    dst: &str,
) -> Result<bool, String> {
    if *ctx.op_type != FileOpType::Move {
        return Ok(true);
    }
    let src_driver = ctx.src_driver;
    let removed = attempt(ctx, st, src, dst, || {
        src_driver
            .remove_file(src)
            .map_err(|e| format!("Cannot remove source {src}: {e}"))
    })?;
    Ok(removed.is_some())
}

/// A source directory is removed only once every child transferred and the
/// directory is empty. Skipped children leave the whole subtree in place.
fn remove_source_dir(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    dst: &str,
    complete: bool,
) -> Result<bool, String> {
    if *ctx.op_type != FileOpType::Move || !complete {
        return Ok(false);
    }
    let src_driver = ctx.src_driver;
    let empty = match attempt(ctx, st, src, dst, || {
        src_driver
            .read_dir_entries(src)
            .map(|entries| entries.is_empty())
            .map_err(|e| format!("Cannot read source directory {src}: {e}"))
    })? {
        Some(empty) => empty,
        None => return Ok(false),
    };
    if !empty {
        return Ok(false);
    }
    let removed = attempt(ctx, st, src, dst, || {
        src_driver
            .remove_dir_all(src)
            .map_err(|e| format!("Cannot remove source directory {src}: {e}"))
    })?;
    Ok(removed.is_some())
}

fn run_file_op_blocking(ctx: &FileOpCtx<'_>, st: &mut FileOpState<'_>) -> Result<(), String> {
    // Totals grow as directories are discovered (no pre-scan — VFS may be remote)
    for entry in ctx.files {
        if entry.kind != "directory" {
            st.total_files += 1;
            st.total_bytes_total += entry.size.unwrap_or(0);
        }
    }
    st.apply_to_all = apply_all_outcome(match ctx.conflict_policy {
        ConflictPolicy::Ask => None,
        other => Some(other),
    });

    let src_driver = ctx.src_driver;
    // Independently registered volume aliases each have their own driver Arc,
    // even when both paths belong to the same local filesystem. Local drivers
    // can safely attempt rename across those aliases: the OS returns EXDEV and
    // LocalFsDriver falls back to copy + delete when they are truly on separate
    // filesystems. Non-local drivers still require identical driver identity.
    let can_try_rename = ctx.dst_driver.is_some_and(|dst| {
        std::ptr::eq(src_driver, dst) || (src_driver.is_local() && dst.is_local())
    });

    for entry in ctx.files {
        if wait_if_paused(ctx.pause_rx, ctx.cancel_rx) {
            return Err(CANCELLED_ERR.to_string());
        }

        match ctx.op_type {
            FileOpType::Copy | FileOpType::Move => {
                let dst_drv = ctx.dst_driver.ok_or("No destination driver for copy/move")?;
                let dest_dir = ctx.dest_path.ok_or("No destination path for copy/move")?;
                let src = &entry.source_path;
                let dst_str = dst_drv.resolve_child(dest_dir, destination_name(entry));

                if (*ctx.op_type == FileOpType::Copy || entry.kind == "symlink")
                    && same_resolved_path(src, &dst_str)
                {
                    return Err(format!("Cannot place {src} onto itself or its target"));
                }

                let copied;
                // Try O(1) rename when the drivers can address the same local
                // filesystem and the destination does not exist. A failure is
                // skippable, but an existing target uses the conflict-aware path.
                if *ctx.op_type == FileOpType::Move
                    && can_try_rename
                    && !dst_drv.exists_no_follow(&dst_str)
                {
                    let renamed = attempt(ctx, st, src, &dst_str, || {
                        src_driver
                            .try_rename(src, &dst_str)
                            .map_err(|e| format!("Rename failed for {src}: {e}"))
                    })?;
                    match renamed {
                        // Cross-device: fall through to copy + delete.
                        Some(false) => {}
                        Some(true) | None => {
                            if entry.kind != "directory" {
                                st.completed_files += 1;
                                st.total_bytes_completed += entry.size.unwrap_or(0);
                            }
                            send_progress(ctx, st, None, 0, 0, None);
                            // `None` is a skipped rename: the source stays put.
                            if renamed.is_some() {
                                crate::journal::record_mutation(ctx.job_id, "move", src, Some(&dst_str));
                            }
                            continue;
                        }
                    }
                }

                if entry.kind == "symlink" && src_driver.is_local() {
                    copied = copy_symlink(ctx, st, src, &dst_str)?;
                } else if entry.kind == "directory" {
                    copied = copy_dir_recursive(ctx, st, src, &dst_str, entry.mtime)?;
                } else {
                    let mut resume_requested = false;
                    if dst_drv.exists_no_follow(&dst_str) {
                        let destination_is_link = dst_drv.read_link(&dst_str).is_ok();
                        let candidate = (!destination_is_link).then(|| {
                            resume_candidate(
                                src,
                                &dst_str,
                                entry.size.unwrap_or(0),
                                src_driver,
                                dst_drv,
                            )
                        }).flatten();
                        match resolve_conflict(ctx, st, src, &dst_str, candidate.as_ref())? {
                            ConflictOutcome::Overwrite => {
                                if destination_is_link {
                                    let replaced = attempt(ctx, st, &dst_str, &dst_str, || {
                                        dst_drv.remove_file(&dst_str).map_err(|error| {
                                            format!("Cannot replace destination link {dst_str}: {error}")
                                        })
                                    })?;
                                    if replaced.is_none() {
                                        st.completed_files += 1;
                                        st.total_bytes_completed += entry.size.unwrap_or(0);
                                        send_progress(ctx, st, None, 0, 0, None);
                                        continue;
                                    }
                                }
                            }
                            ConflictOutcome::Resume => resume_requested = true,
                            ConflictOutcome::Skip => {
                                record_conflict_skip(ctx, st, src, &dst_str);
                                st.completed_files += 1;
                                st.total_bytes_completed += entry.size.unwrap_or(0);
                                send_progress(ctx, st, None, 0, 0, None);
                                continue;
                            }
                            ConflictOutcome::Cancel => return Err(CANCELLED_ERR.to_string()),
                        }
                    }

                    copied = copy_file_with_progress(
                        ctx,
                        st,
                        src,
                        &dst_str,
                        entry.size.unwrap_or(0),
                        resume_requested,
                    )?;
                    if copied && ctx.preserve_attributes && dst_drv.is_local() {
                        apply_file_attrs(&dst_str, entry.mtime, entry.permissions.as_deref());
                    }
                }

                // A move never removes its source unless the transfer completed
                // in full — a skipped child leaves the subtree in place.
                let source_removed = if copied && *ctx.op_type == FileOpType::Move {
                    if entry.kind == "directory" {
                        remove_source_dir(ctx, st, src, &dst_str, true)?
                    } else {
                        remove_source_file(ctx, st, src, &dst_str)?
                    }
                } else {
                    false
                };
                // A partial folder copy is still journaled; a move whose source
                // stayed behind is recorded as the copy it actually was.
                let partial_dir = !copied
                    && entry.kind == "directory"
                    && dst_drv.exists_no_follow(&dst_str);
                if copied || partial_dir {
                    let kind = if source_removed { "move" } else { "copy" };
                    crate::journal::record_mutation(ctx.job_id, kind, src, Some(&dst_str));
                }

                if entry.kind != "directory" {
                    st.completed_files += 1;
                    st.total_bytes_completed += entry.size.unwrap_or(0);
                }
                send_progress(ctx, st, None, 0, 0, None);
            }
            FileOpType::Delete => {
                let src = &entry.source_path;
                send_progress(ctx, st, Some(entry.name.clone()), 0, 0, None);
                let removed = delete_tree(ctx, st, src, &entry.name, entry.kind == "directory")?;
                if removed {
                    crate::journal::record_mutation(ctx.job_id, "delete", src, None);
                }
                // Folder entries are not in `total_files`; their files are
                // counted by `delete_tree` as they are discovered.
                if entry.kind != "directory" {
                    st.completed_files += 1;
                }
                send_progress(ctx, st, None, 0, 0, None);
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
fn copy_symlink(ctx: &FileOpCtx<'_>, st: &mut FileOpState<'_>, src: &str, dst: &str) -> Result<bool, String> {
    let dst_driver = ctx
        .dst_driver
        .ok_or_else(|| "No destination driver for copy/move".to_string())?;
    if !dst_driver.is_local() {
        return Err(format!(
            "Cannot copy symbolic link to a destination that does not support links: {dst}"
        ));
    }
    let src_driver = ctx.src_driver;
    let link = match attempt(ctx, st, src, dst, || {
        src_driver
            .read_link(src)
            .map_err(|error| format!("Cannot read symbolic link {src}: {error}"))
    })? {
        Some(link) => link,
        None => return Ok(false),
    };

    let exists = dst_driver.exists_no_follow(dst);
    if exists {
        match resolve_conflict(ctx, st, src, dst, None)? {
            ConflictOutcome::Skip => {
                record_conflict_skip(ctx, st, src, dst);
                return Ok(false);
            }
            ConflictOutcome::Cancel => return Err(CANCELLED_ERR.into()),
            ConflictOutcome::Overwrite | ConflictOutcome::Resume => {}
        }
    }

    let temp = format!("{dst}.fbro-link-{}", uuid::Uuid::new_v4());
    let backup = format!("{dst}.fbro-old-{}", uuid::Uuid::new_v4());
    let result = attempt(ctx, st, src, dst, || {
        // A previous attempt may have left a temporary link behind.
        let _ = dst_driver.remove_file(&temp);
        if let Some(parent) = dst_driver.parent_path(dst).filter(|p| !p.is_empty()) {
            dst_driver
                .create_dir_all(&parent)
                .map_err(|error| format!("Cannot create destination directory {parent}: {error}"))?;
        }
        dst_driver
            .create_symlink(&link.target, &temp, link.is_dir)
            .map_err(|error| format!("Cannot create symbolic link {dst}: {error}"))?;
        // Re-check on every attempt: the destination is moved aside only while
        // the new link is placed, and restored if that fails, so a retry or a
        // skip never finds it missing.
        let replacing = exists && dst_driver.exists_no_follow(dst);
        if replacing {
            if dst_driver.read_link(dst).is_err() && dst_driver.read_dir_entries(dst).is_ok() {
                return Err(format!("Cannot replace folder {dst} with a symbolic link"));
            }
            match dst_driver.try_rename(dst, &backup) {
                Ok(true) => {}
                Ok(false) => return Err(format!("Cannot replace destination {dst}")),
                Err(error) => return Err(format!("Cannot replace destination {dst}: {error}")),
            }
        }
        let placed = match dst_driver.try_rename(&temp, dst) {
            Ok(true) => Ok(()),
            Ok(false) => Err(format!("Cannot place symbolic link at {dst}")),
            Err(error) => Err(format!("Cannot place symbolic link at {dst}: {error}")),
        };
        if replacing {
            if placed.is_ok() {
                let _ = dst_driver.remove_file(&backup);
            } else if !matches!(dst_driver.try_rename(&backup, dst), Ok(true)) {
                eprintln!("[file_ops] could not restore {dst} from {backup}");
            }
        }
        placed
    })?;
    if result.is_none() {
        let _ = dst_driver.remove_file(&temp);
        return Ok(false);
    }
    Ok(true)
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
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    dst: &str,
    resume_candidate: Option<&ResumeCandidate>,
) -> Result<ConflictOutcome, String> {
    match ctx.conflict_policy {
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
            if let Some(outcome) = st.apply_to_all {
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
                ctx.tx,
                ctx.job_id,
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
                None,
            );
            match st.decision_rx.blocking_recv() {
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
                        st.apply_to_all = Some(outcome);
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
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    dst: &str,
    total_bytes: u64,
    resume_requested: bool,
) -> Result<bool, String> {
    let src_name = Path::new(src)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let dst_driver = ctx
        .dst_driver
        .ok_or_else(|| "No destination driver for copy/move".to_string())?;

    let can_resume = ctx.src_driver.supports_read_at() && dst_driver.supports_write_at();
    let mut resume_next_attempt = resume_requested;
    let mut resume_attempts = 0usize;
    // Set once any attempt truncated `dst`: from then on its content is ours
    // and a skip may remove it. Before that, `dst` is the user's file.
    let mut dst_written = false;
    loop {
        match copy_file_once_with_progress(
            ctx,
            st,
            src,
            dst,
            total_bytes,
            &src_name,
            resume_next_attempt,
            &mut dst_written,
        ) {
            Ok(()) => return Ok(true),
            Err(CopyFileError::Skipped) => {
                ctx.skip_current_file.store(false, Ordering::SeqCst);
                record_skip(ctx, st, src, "Skipped by user", true);
                return Ok(false);
            }
            Err(CopyFileError::Cancelled) => return Err(CANCELLED_ERR.to_string()),
            Err(CopyFileError::Recoverable(err)) => {
                if *ctx.cancel_rx.borrow() {
                    return Err(CANCELLED_ERR.to_string());
                }
                if ctx.auto_resume_copies && can_resume && resume_attempts < COPY_RESUME_MAX_ATTEMPTS {
                    resume_attempts += 1;
                    let partial = dst_driver.file_size(dst).unwrap_or(0).min(total_bytes);
                    send_progress(
                        ctx,
                        st,
                        Some(src_name.clone()),
                        total_bytes,
                        partial,
                        Some(format!(
                            "Resuming transfer after read error ({resume_attempts}/{COPY_RESUME_MAX_ATTEMPTS})"
                        )),
                    );
                    resume_next_attempt = true;
                    continue;
                }
                if !handle_copy_error(
                    ctx,
                    st,
                    src,
                    dst,
                    &err,
                    &mut resume_next_attempt,
                    dst_written,
                )? {
                    return Ok(false);
                }
            }
            Err(CopyFileError::Io(err)) => {
                if !handle_copy_error(
                    ctx,
                    st,
                    src,
                    dst,
                    &err,
                    &mut resume_next_attempt,
                    dst_written,
                )? {
                    return Ok(false);
                }
            }
            Err(CopyFileError::Verification(err)) => {
                if *ctx.verification_policy == VerificationPolicy::Fail {
                    if !handle_copy_error(
                        ctx,
                        st,
                        src,
                        dst,
                        &err,
                        &mut resume_next_attempt,
                        dst_written,
                    )? {
                        return Ok(false);
                    }
                    continue;
                }
                match resolve_verification_mismatch(ctx, st, src, dst, &err)? {
                    VerificationOutcome::Retry => {
                        if *ctx.cancel_rx.borrow() {
                            return Err(CANCELLED_ERR.to_string());
                        }
                        resume_next_attempt = false;
                        send_progress(
                            ctx,
                            st,
                            Some(src_name.clone()),
                            total_bytes,
                            0,
                            Some("Retrying transfer".to_string()),
                        );
                    }
                    VerificationOutcome::Skip { notify } => {
                        record_skip(ctx, st, src, err, notify);
                        return Ok(false);
                    }
                    VerificationOutcome::Abort => return Err(err),
                }
            }
        }
    }
}

/// Apply the shared error decision to a failed item. Returns `false` when the
/// item was skipped and the job should move on. A retry starts the file over
/// from byte zero (the next `open_write` truncates; nothing is removed first).
/// A skip removes `dst` only when this copy wrote to it, so a destination the
/// user asked to overwrite or resume survives an error raised before writing.
fn handle_copy_error(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    dst: &str,
    err: &str,
    resume_next_attempt: &mut bool,
    dst_written: bool,
) -> Result<bool, String> {
    match resolve_error(ctx, st, src, dst, err) {
        ErrorOutcome::Retry => {
            *resume_next_attempt = false;
            Ok(true)
        }
        ErrorOutcome::Skip { notify } => {
            if dst_written {
                if let Some(driver) = ctx.dst_driver {
                    let _ = driver.remove_file(dst);
                }
            }
            record_skip(ctx, st, src, err, notify);
            Ok(false)
        }
        ErrorOutcome::Cancel => Err(CANCELLED_ERR.to_string()),
    }
}

enum CopyFileError {
    Cancelled,
    Io(String),
    Recoverable(String),
    Skipped,
    Verification(String),
}

fn copy_file_once_with_progress(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    dst: &str,
    total_bytes: u64,
    src_name: &str,
    resume_requested: bool,
    dst_written: &mut bool,
) -> Result<(), CopyFileError> {
    let src_driver = ctx.src_driver;
    let dst_driver = ctx
        .dst_driver
        .ok_or_else(|| CopyFileError::Io("No destination driver for copy/move".to_string()))?;
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
        send_progress(
            ctx,
            st,
            Some(src_name.to_string()),
            file_size,
            resume_offset,
            Some(format!("Resuming from byte {resume_offset}")),
        );
    }

    if ctx.skip_current_file.load(Ordering::SeqCst) {
        if *dst_written {
            let _ = dst_driver.remove_file(dst);
        }
        return Err(CopyFileError::Skipped);
    }

    let mut reader = if resume_offset > 0 {
        src_driver.open_read_at(src, resume_offset).map_err(|e| {
            if matches!(e, crate::vfs::VfsError::Cancelled) {
                CopyFileError::Cancelled
            } else {
                CopyFileError::Recoverable(format!(
                    "Cannot resume {src} at byte {resume_offset}: {e}"
                ))
            }
        })?
    } else {
        src_driver.open_read(src).map_err(|e| {
            if matches!(e, crate::vfs::VfsError::Cancelled) {
                CopyFileError::Cancelled
            } else {
                CopyFileError::Io(format!("Cannot open {src}: {e}"))
            }
        })?
    };

    if resume_offset == 0 {
        if let Some(parent) = dst_driver.parent_path(dst) {
            if !parent.is_empty() {
                dst_driver.create_dir_all(&parent).map_err(|e| {
                    CopyFileError::Io(format!("Cannot create destination directory {parent}: {e}"))
                })?;
            }
        }
    }

    let mut writer = if resume_offset > 0 {
        dst_driver.open_write_at(dst, resume_offset).map_err(|e| {
            CopyFileError::Io(format!(
                "Cannot resume writing {dst} at byte {resume_offset}: {e}"
            ))
        })?
    } else {
        let writer = dst_driver
            .open_write(dst)
            .map_err(|e| CopyFileError::Io(format!("Cannot create {dst}: {e}")))?;
        *dst_written = true;
        writer
    };

    let mut buf = vec![0u8; COPY_BUF_SIZE];
    let mut copied: u64 = resume_offset;
    let mut stream_hasher =
        hash_existing_prefix(dst_driver, dst, resume_offset).map_err(CopyFileError::Io)?;
    let mut last_progress_at = Instant::now();
    loop {
        if ctx.skip_current_file.load(Ordering::SeqCst) {
            drop(writer);
            if *dst_written {
                let _ = dst_driver.remove_file(dst);
            }
            return Err(CopyFileError::Skipped);
        }

        if wait_if_paused(ctx.pause_rx, ctx.cancel_rx) {
            drop(writer);
            if resume_offset == 0 && !ctx.keep_partial_files_on_cancel.load(Ordering::Relaxed) {
                let _ = dst_driver.remove_file(dst);
            }
            return Err(CopyFileError::Cancelled);
        }

        let read = reader.read(&mut buf);
        let remote_cancelled = read.as_ref().err().is_some_and(|error| {
            error
                .get_ref()
                .and_then(|inner| inner.downcast_ref::<crate::vfs::VfsError>())
                .is_some_and(|error| matches!(error, crate::vfs::VfsError::Cancelled))
        });
        if remote_cancelled || *ctx.cancel_rx.borrow() {
            drop(writer);
            if resume_offset == 0 && !ctx.keep_partial_files_on_cancel.load(Ordering::Relaxed) {
                let _ = dst_driver.remove_file(dst);
            }
            return Err(CopyFileError::Cancelled);
        }
        let n = read.map_err(|e| CopyFileError::Recoverable(format!("Read error on {src}: {e}")))?;
        if n == 0 {
            break;
        }

        writer
            .write_all(&buf[..n])
            .map_err(|e| CopyFileError::Io(format!("Write error on {dst}: {e}")))?;

        stream_hasher.update(&buf[..n]);
        copied += n as u64;

        if copied == file_size || last_progress_at.elapsed() >= COPY_PROGRESS_INTERVAL {
            send_progress(
                ctx,
                st,
                Some(src_name.to_string()),
                file_size,
                copied,
                None,
            );
            last_progress_at = Instant::now();
        }
    }

    writer
        .flush()
        .map_err(|e| CopyFileError::Io(format!("Flush error on {dst}: {e}")))?;
    drop(writer);

    let remote_digest = if resume_offset > 0 {
        src_driver.file_digest(src).map_err(|e| {
            if matches!(e, crate::vfs::VfsError::Cancelled) {
                CopyFileError::Cancelled
            } else {
                CopyFileError::Verification(format!("Verification failed for {src}: {e}"))
            }
        })?
    } else {
        src_driver.transfer_digest(ctx.job_id, src).map_err(|e| {
            if matches!(e, crate::vfs::VfsError::Cancelled) {
                CopyFileError::Cancelled
            } else {
                CopyFileError::Verification(format!("Verification failed for {src}: {e}"))
            }
        })?
    };

    if let Some(remote_digest) = remote_digest {
        send_progress(
            ctx,
            st,
            Some(src_name.to_string()),
            file_size,
            copied,
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

    if *ctx.cancel_rx.borrow() {
        return Err(CopyFileError::Cancelled);
    }

    if ctx.save_digest_file {
        let digest = stream_hasher.finalize().to_hex().to_string();
        write_digest_sidecar(dst_driver, dst, src_name, &digest).map_err(CopyFileError::Io)?;
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
    Skip { notify: bool },
    Abort,
}

fn resolve_verification_mismatch(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    dst: &str,
    message: &str,
) -> Result<VerificationOutcome, String> {
    // Verification has its own policy: "Ask" always prompts, even when the
    // error policy or Skip all would skip I/O errors silently. Skipping a
    // mismatch quietly would leave an unverified copy looking like a good one.
    send_event_sync(
        ctx.tx,
        ctx.job_id,
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
        None,
    );
    match st.decision_rx.blocking_recv() {
        Some(DecisionResponse { decision, .. }) => match decision.as_str() {
            "retry" => Ok(VerificationOutcome::Retry),
            "skip" => Ok(VerificationOutcome::Skip { notify: true }),
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

/// Copy a directory recursively. Returns `false` when any child was skipped,
/// so a move can leave the whole source subtree in place.
fn copy_dir_recursive(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    dst: &str,
    src_mtime: Option<u64>,
) -> Result<bool, String> {
    let src_driver = ctx.src_driver;
    let dst_driver = ctx
        .dst_driver
        .ok_or_else(|| "No destination driver for copy/move".to_string())?;

    let created = attempt(ctx, st, src, dst, || {
        dst_driver
            .create_dir_all(dst)
            .map_err(|e| format!("Cannot create dir {dst}: {e}"))
    })?;
    if created.is_none() {
        return Ok(false);
    }

    if ctx.preserve_attributes && dst_driver.is_local() {
        apply_file_attrs(dst, src_mtime, None);
    }

    let children = match attempt(ctx, st, src, dst, || {
        src_driver
            .read_dir_entries(src)
            .map_err(|e| format!("Cannot read dir {src}: {e}"))
    })? {
        Some(children) => children,
        None => return Ok(false),
    };

    // Discover files in this directory — grow totals.
    for child in &children {
        if !child.is_dir {
            st.total_files += 1;
            st.total_bytes_total += child.size;
        }
    }

    let mut complete = true;
    for child in children {
        if wait_if_paused(ctx.pause_rx, ctx.cancel_rx) {
            return Err(CANCELLED_ERR.to_string());
        }

        let child_src = src_driver.resolve_child(src, &child.name);
        let child_dst = dst_driver.resolve_child(dst, &child.name);

        if child.is_dir {
            let child_complete =
                copy_dir_recursive(ctx, st, &child_src, &child_dst, child.mtime)?;
            if !child_complete {
                complete = false;
            } else {
                let _ = remove_source_dir(ctx, st, &child_src, &child_dst, true)?;
            }
            continue;
        }

        let copied;
        if child.is_symlink && src_driver.is_local() {
            copied = copy_symlink(ctx, st, &child_src, &child_dst)?;
        } else {
            let mut resume_requested = false;
            if dst_driver.exists_no_follow(&child_dst) {
                let destination_is_link = dst_driver.read_link(&child_dst).is_ok();
                let candidate = (!destination_is_link)
                    .then(|| {
                        resume_candidate(
                            &child_src,
                            &child_dst,
                            child.size,
                            src_driver,
                            dst_driver,
                        )
                    })
                    .flatten();
                match resolve_conflict(ctx, st, &child_src, &child_dst, candidate.as_ref())? {
                    ConflictOutcome::Overwrite => {
                        if destination_is_link {
                            let replaced = attempt(ctx, st, &child_dst, &child_dst, || {
                                dst_driver.remove_file(&child_dst).map_err(|error| {
                                    format!("Cannot replace destination link {child_dst}: {error}")
                                })
                            })?;
                            if replaced.is_none() {
                                st.completed_files += 1;
                                st.total_bytes_completed += child.size;
                                send_progress(ctx, st, None, 0, 0, None);
                                complete = false;
                                continue;
                            }
                        }
                    }
                    ConflictOutcome::Resume => resume_requested = true,
                    ConflictOutcome::Skip => {
                        record_conflict_skip(ctx, st, &child_src, &child_dst);
                        st.completed_files += 1;
                        st.total_bytes_completed += child.size;
                        send_progress(ctx, st, None, 0, 0, None);
                        complete = false;
                        continue;
                    }
                    ConflictOutcome::Cancel => return Err(CANCELLED_ERR.to_string()),
                }
            }

            copied = copy_file_with_progress(ctx, st, &child_src, &child_dst, child.size, resume_requested)?;
            if copied && ctx.preserve_attributes && dst_driver.is_local() {
                apply_file_attrs(&child_dst, child.mtime, child.permissions.as_deref());
            }
        }

        if copied {
            let _ = remove_source_file(ctx, st, &child_src, &child_dst)?;
        }

        if !copied {
            complete = false;
        }
        st.completed_files += 1;
        st.total_bytes_completed += child.size;
        send_progress(ctx, st, None, 0, 0, None);
    }

    Ok(complete)
}

/// Delete a tree child by child so an individual failure is skippable. A
/// directory is left in place when any of its children could not be deleted.
fn delete_tree(
    ctx: &FileOpCtx<'_>,
    st: &mut FileOpState<'_>,
    src: &str,
    display_name: &str,
    is_dir: bool,
) -> Result<bool, String> {
    let driver = ctx.src_driver;
    send_progress(ctx, st, Some(display_name.to_string()), 0, 0, None);

    if is_dir {
        let children = match attempt(ctx, st, src, src, || {
            driver
                .read_dir_entries(src)
                .map_err(|e| format!("Cannot read dir {src}: {e}"))
        })? {
            Some(children) => children,
            None => return Ok(false),
        };
        let mut complete = true;
        for child in children {
            if wait_if_paused(ctx.pause_rx, ctx.cancel_rx) {
                return Err(CANCELLED_ERR.to_string());
            }
            let child_path = driver.resolve_child(src, &child.name);
            // Symlinks to directories are removed as links, never traversed.
            let child_is_dir = child.is_dir && !child.is_symlink;
            if !child_is_dir {
                st.total_files += 1;
                st.total_bytes_total += child.size;
            }
            if !delete_tree(ctx, st, &child_path, &child.name, child_is_dir)? {
                complete = false;
            }
            if !child_is_dir {
                st.completed_files += 1;
                st.total_bytes_completed += child.size;
            }
            send_progress(ctx, st, None, 0, 0, None);
        }
        if !complete {
            return Ok(false);
        }
    }

    let removed = attempt(ctx, st, src, src, || {
        let result = if is_dir {
            driver.remove_dir_all(src)
        } else {
            driver.remove_file(src)
        };
        result.map_err(|e| format!("Failed to delete {src}: {e}"))
    })?;
    Ok(removed.is_some())
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

fn send_progress(
    ctx: &FileOpCtx<'_>,
    st: &FileOpState<'_>,
    current_file: Option<String>,
    current_file_bytes_total: u64,
    current_file_bytes_copied: u64,
    status_text: Option<String>,
) {
    let progress = FileOpProgressSchema {
        total_files: st.total_files,
        completed_files: st.completed_files,
        current_file,
        status_text,
        current_file_bytes_total,
        current_file_bytes_copied,
        total_bytes_total: st.total_bytes_total,
        total_bytes_copied: st.total_bytes_completed,
        current_file_percent: None,
        overall_percent: None,
        skipped_files: st.skipped_count,
    };
    send_event_sync(ctx.tx, ctx.job_id, "Progress", Some(progress), None, None, None);
}

fn send_event_sync(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    event: &str,
    progress: Option<FileOpProgressSchema>,
    decision: Option<ConflictDecisionSchema>,
    error: Option<String>,
    skipped: Option<SkippedItem>,
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
        skipped,
    };
    // Progress is a sample and may be dropped under backpressure. Skips and
    // decisions are not: they feed the job's skipped list and the prompt.
    // The worker runs in spawn_blocking, so waiting for capacity is safe.
    match tx.try_send(msg) {
        Err(mpsc::error::TrySendError::Full(msg)) if event != "Progress" => {
            let _ = tx.blocking_send(msg);
        }
        _ => {}
    }
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
        skipped: None,
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

    // Compatibility shims: existing tests exercise the operation through the
    // original argument lists; they build the same context/state pair used by
    // the production entry point.
    #[allow(clippy::too_many_arguments)]
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
        let error_policy = ErrorPolicy::Ask;
        let ctx = FileOpCtx {
            job_id,
            op_type,
            files,
            dest_path,
            tx,
            cancel_rx,
            pause_rx,
            src_driver,
            dst_driver,
            conflict_policy,
            error_policy: &error_policy,
            verification_policy,
            preserve_attributes,
            save_digest_file,
            auto_resume_copies,
            skip_current_file,
            keep_partial_files_on_cancel,
        };
        let mut st = FileOpState::new(decision_rx);
        super::run_file_op_blocking(&ctx, &mut st)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_file_op_blocking_with_policy(
        job_id: &str,
        op_type: &FileOpType,
        files: &[FileOpEntry],
        dest_path: Option<&str>,
        conflict_policy: &ConflictPolicy,
        error_policy: &ErrorPolicy,
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
        let ctx = FileOpCtx {
            job_id,
            op_type,
            files,
            dest_path,
            tx,
            cancel_rx,
            pause_rx,
            src_driver,
            dst_driver,
            conflict_policy,
            error_policy,
            verification_policy,
            preserve_attributes,
            save_digest_file,
            auto_resume_copies,
            skip_current_file,
            keep_partial_files_on_cancel,
        };
        let mut st = FileOpState::new(decision_rx);
        super::run_file_op_blocking(&ctx, &mut st)
    }

    #[allow(clippy::too_many_arguments)]
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
        let error_policy = ErrorPolicy::Ask;
        let op_type = FileOpType::Copy;
        let ctx = FileOpCtx {
            job_id,
            op_type: &op_type,
            files: &[],
            dest_path: None,
            tx,
            cancel_rx,
            pause_rx,
            src_driver,
            dst_driver: Some(dst_driver),
            conflict_policy: &ConflictPolicy::Ask,
            error_policy: &error_policy,
            verification_policy,
            preserve_attributes: false,
            save_digest_file,
            auto_resume_copies,
            skip_current_file,
            keep_partial_files_on_cancel,
        };
        let mut st = FileOpState::new(decision_rx);
        st.total_files = total_files;
        st.completed_files = completed_files;
        st.total_bytes_total = total_bytes_total;
        st.total_bytes_completed = total_bytes_completed;
        super::copy_file_with_progress(&ctx, &mut st, src, dst, total_bytes, resume_requested)
    }

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

    #[test]
    fn search_copy_duplicates_use_same_plan_for_copy_and_dry_run() {
        use crate::jobs::copy_duplicates::{prepare_search_copy, DuplicateNamePolicy};
        let fixture = TestDir::new("search-copy-duplicates");
        let driver = crate::vfs::local::LocalFsDriver;
        let mut sources = Vec::new();
        for folder in ["one", "two"] {
            let directory = fixture.path.join(folder);
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join("README.md");
            fs::write(&path, folder).unwrap();
            sources.push(FileOpEntry {
                name: "README.md".into(), dest_name: None,
                source_path: path.to_string_lossy().into_owned(), size: Some(3),
                kind: "file".into(), mtime: None, permissions: None,
            });
        }
        for (label, policy, expected_count) in [
            ("exclude", DuplicateNamePolicy::Exclude, 1),
            ("suffix", DuplicateNamePolicy::Suffix, 2),
        ] {
            let destination = fixture.path.join(label);
            fs::create_dir_all(&destination).unwrap();
            let dest = destination.to_string_lossy().into_owned();
            let files = prepare_search_copy(sources.clone(), policy, &dest, &driver).unwrap();
            assert_eq!(files.len(), expected_count);
            let script = fixture.path.join(format!("{label}.sh"));
            generate_dry_run_script(&FileOpType::Copy, &files, &script.to_string_lossy(), &driver, Some(&dest), Some(&driver)).unwrap();
            let script_text = fs::read_to_string(script).unwrap();
            assert_eq!(script_text.lines().filter(|line| line.starts_with("cp ")).count(), expected_count);
            for file in &files {
                assert!(script_text.contains(&shell_quote(&driver.resolve_child(&dest, destination_name(file)))));
            }

            let (tx, _rx) = mpsc::channel(128);
            let (_cancel_tx, cancel_rx) = watch::channel(false);
            let (_pause_tx, pause_rx) = watch::channel(false);
            let (_decision_tx, mut decision_rx) = mpsc::channel(1);
            run_file_op_blocking(
                "search-copy-test", &FileOpType::Copy, &files, Some(&dest),
                &ConflictPolicy::Overwrite, &tx, &cancel_rx, &pause_rx, &mut decision_rx,
                &driver, Some(&driver), false, &VerificationPolicy::Fail, false, false,
                Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)),
            ).unwrap();
            assert_eq!(fs::read_dir(&destination).unwrap().count(), expected_count);
            assert_eq!(fs::read_to_string(destination.join("README.md")).unwrap(), "one");
            if policy == DuplicateNamePolicy::Suffix {
                let renamed = files[1].dest_name.as_ref().unwrap();
                assert!(renamed.starts_with("README-") && renamed.ends_with(".md"));
                assert_eq!(fs::read_to_string(destination.join(renamed)).unwrap(), "two");
            }
            assert!(sources.iter().all(|file| Path::new(&file.source_path).exists()));
        }
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

    /// Local driver that fails reads/removals for chosen paths. `is_local()` is
    /// false so copy/move always exercise the copy path rather than `try_rename`.
    struct FaultyDriver {
        local: crate::vfs::local::LocalFsDriver,
        fail_read: Option<String>,
        fail_read_all: bool,
        fail_remove: Option<String>,
        /// Report as local (so moves try `try_rename`) and fail every rename.
        rename_fails: bool,
    }

    impl FaultyDriver {
        fn new(fail_read: Option<&Path>, fail_remove: Option<&Path>) -> Self {
            Self {
                local: crate::vfs::local::LocalFsDriver,
                fail_read: fail_read.map(|path| path.to_string_lossy().into_owned()),
                fail_read_all: false,
                fail_remove: fail_remove.map(|path| path.to_string_lossy().into_owned()),
                rename_fails: false,
            }
        }

        fn fail_read(path: &Path) -> Self {
            Self::new(Some(path), None)
        }

        fn fail_remove(path: &Path) -> Self {
            Self::new(None, Some(path))
        }

        fn healthy() -> Self {
            Self::new(None, None)
        }

        fn all_reads_fail() -> Self {
            let mut driver = Self::healthy();
            driver.fail_read_all = true;
            driver
        }

        fn renames_fail() -> Self {
            let mut driver = Self::healthy();
            driver.rename_fails = true;
            driver
        }
    }

    impl VfsDriver for FaultyDriver {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn list(
            &self,
            path: &str,
            sort: &SortSpec,
            filter: Option<&FilterSpec>,
            show_hidden: bool,
        ) -> Result<Vec<VfsEntry>, VfsError> {
            self.local.list(path, sort, filter, show_hidden)
        }

        fn resolve_child(&self, base: &str, name: &str) -> String {
            self.local.resolve_child(base, name)
        }

        fn parent_path(&self, path: &str) -> Option<String> {
            self.local.parent_path(path)
        }

        fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
            if self.fail_read_all || self.fail_read.as_deref() == Some(path) {
                return Err(VfsError::IOError("injected read failure".to_string()));
            }
            self.local.open_read(path)
        }

        fn supports_read_at(&self) -> bool {
            true
        }

        fn open_read_at(&self, path: &str, _offset: u64) -> Result<Box<dyn Read + Send>, VfsError> {
            self.open_read(path)
        }

        fn open_write(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
            self.local.open_write(path)
        }

        fn supports_write_at(&self) -> bool {
            true
        }

        fn open_write_at(&self, path: &str, offset: u64) -> Result<Box<dyn Write + Send>, VfsError> {
            self.local.open_write_at(path, offset)
        }

        fn create_dir_all(&self, path: &str) -> Result<(), VfsError> {
            self.local.create_dir_all(path)
        }

        fn remove_file(&self, path: &str) -> Result<(), VfsError> {
            if self.fail_remove.as_deref() == Some(path) {
                return Err(VfsError::PermissionDenied(
                    "injected removal failure".to_string(),
                ));
            }
            self.local.remove_file(path)
        }

        fn remove_dir_all(&self, path: &str) -> Result<(), VfsError> {
            if self.fail_remove.as_deref() == Some(path) {
                return Err(VfsError::PermissionDenied(
                    "injected removal failure".to_string(),
                ));
            }
            self.local.remove_dir_all(path)
        }

        fn exists(&self, path: &str) -> bool {
            self.local.exists(path)
        }

        fn exists_no_follow(&self, path: &str) -> bool {
            self.local.exists_no_follow(path)
        }

        fn read_link(&self, path: &str) -> Result<crate::vfs::LinkInfo, VfsError> {
            self.local.read_link(path)
        }

        fn create_symlink(
            &self,
            target: &Path,
            link: &str,
            is_dir: bool,
        ) -> Result<(), VfsError> {
            self.local.create_symlink(target, link, is_dir)
        }

        fn file_size(&self, path: &str) -> Option<u64> {
            self.local.file_size(path)
        }

        fn read_dir_entries(&self, path: &str) -> Result<Vec<DirChild>, VfsError> {
            self.local.read_dir_entries(path)
        }

        fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
            self.local.read_chunk(path, offset, size)
        }

        fn is_local(&self) -> bool {
            self.rename_fails
        }

        fn try_rename(&self, src: &str, dst: &str) -> Result<bool, VfsError> {
            if self.rename_fails {
                return Err(VfsError::PermissionDenied("injected rename failure".to_string()));
            }
            self.local.try_rename(src, dst)
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
    fn skip_before_first_write_keeps_existing_destination_and_emits_event() {
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
        // Nothing was written yet, so the user's file is not ours to delete.
        assert_eq!(fs::read(&dst).unwrap(), b"old partial");
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

    fn file_entry(name: &str, path: &Path, size: u64) -> FileOpEntry {
        FileOpEntry {
            name: name.to_string(),
            dest_name: None,
            source_path: path.to_string_lossy().into_owned(),
            size: Some(size),
            kind: "file".to_string(),
            mtime: None,
            permissions: None,
        }
    }

    fn dir_entry(name: &str, path: &Path) -> FileOpEntry {
        FileOpEntry {
            name: name.to_string(),
            dest_name: None,
            source_path: path.to_string_lossy().into_owned(),
            size: None,
            kind: "directory".to_string(),
            mtime: None,
            permissions: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_with_driver(
        op: FileOpType,
        files: &[FileOpEntry],
        dest: Option<&str>,
        conflict_policy: ConflictPolicy,
        error_policy: ErrorPolicy,
        tx: &mpsc::Sender<ServerMessage>,
        decision_rx: &mut DecisionRx,
        driver: &dyn VfsDriver,
        skip_current: Arc<AtomicBool>,
    ) -> Result<(), String> {
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let (_pause_tx, pause_rx) = watch::channel(false);
        let local = crate::vfs::local::LocalFsDriver;
        run_file_op_blocking_with_policy(
            "job-test",
            &op,
            files,
            dest,
            &conflict_policy,
            &error_policy,
            tx,
            &cancel_rx,
            &pause_rx,
            decision_rx,
            driver,
            if matches!(op, FileOpType::Delete) {
                None
            } else {
                Some(&local)
            },
            false,
            &VerificationPolicy::Fail,
            false,
            false,
            skip_current,
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[derive(Default)]
    struct EventSummary {
        error_prompts: usize,
        conflict_prompts: usize,
        skipped: Vec<SkippedItem>,
    }

    fn collect_events(rx: &mut mpsc::Receiver<ServerMessage>) -> EventSummary {
        let mut summary = EventSummary::default();
        while let Ok(message) = rx.try_recv() {
            if let ServerMessage::JobEvent {
                event,
                decision,
                skipped,
                ..
            } = message
            {
                if event == "DecisionRequired" {
                    match decision.and_then(|d| d.kind).as_deref() {
                        Some("error") => summary.error_prompts += 1,
                        Some("conflict") => summary.conflict_prompts += 1,
                        _ => {}
                    }
                }
                if event == "FileSkipped" {
                    if let Some(item) = skipped {
                        summary.skipped.push(item);
                    }
                }
            }
        }
        summary
    }

    fn respond(tx: &mpsc::Sender<DecisionResponse>, decision: &str) {
        tx.try_send(DecisionResponse {
            decision: decision.to_string(),
            apply_to_all: false,
        })
        .unwrap();
    }

    #[test]
    fn error_retry_completes_the_file() {
        let dir = TestDir::new("error-retry");
        let src = dir.path.join("source.bin");
        let data = bytes(5);
        fs::write(&src, &data).unwrap();

        let driver = FlakyReadDriver::new(Some(3));
        let (tx, mut rx) = mpsc::channel(64);
        let (decision_tx, mut decision_rx) = mpsc::channel(2);
        respond(&decision_tx, "retry");
        let dest = dir.path.join("dst");
        fs::create_dir_all(&dest).unwrap();

        run_with_driver(
            FileOpType::Copy,
            &[file_entry("source.bin", &src, data.len() as u64)],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(fs::read(dest.join("source.bin")).unwrap(), data);
        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 1);
        assert!(events.skipped.is_empty());
    }

    #[test]
    fn error_retry_restarts_from_byte_zero_and_drops_partial_copy() {
        let dir = TestDir::new("error-retry-restart");
        let src = dir.path.join("source.bin");
        let data = bytes(2 * 1024 * 1024 + 113);
        fs::write(&src, &data).unwrap();

        let driver = FlakyReadDriver::new(Some(1152 * 1024));
        let (tx, mut rx) = mpsc::channel(64);
        let (decision_tx, mut decision_rx) = mpsc::channel(2);
        respond(&decision_tx, "retry");
        let dest = dir.path.join("dst");
        fs::create_dir_all(&dest).unwrap();

        run_with_driver(
            FileOpType::Copy,
            &[file_entry("source.bin", &src, data.len() as u64)],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(fs::read(dest.join("source.bin")).unwrap(), data);
        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 1);
        assert!(events.skipped.is_empty());
    }

    #[test]
    fn skipping_after_a_partial_copy_removes_the_fresh_destination() {
        let dir = TestDir::new("error-skip-partial");
        let src = dir.path.join("source.bin");
        let data = bytes(2 * 1024 * 1024 + 113);
        fs::write(&src, &data).unwrap();

        let driver = FlakyReadDriver::new(Some(1152 * 1024));
        let (tx, mut rx) = mpsc::channel(64);
        let (decision_tx, mut decision_rx) = mpsc::channel(2);
        respond(&decision_tx, "skip");
        let dest = dir.path.join("dst");
        fs::create_dir_all(&dest).unwrap();

        run_with_driver(
            FileOpType::Copy,
            &[file_entry("source.bin", &src, data.len() as u64)],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(!dest.join("source.bin").exists());
        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 1);
        assert_eq!(events.skipped.len(), 1);
        assert!(events.skipped[0].notify);
    }

    #[test]
    fn explicit_skip_continues_with_the_next_file_and_reports_notify() {
        let dir = TestDir::new("error-skip");
        let src_dir = dir.path.join("src");
        let dest = dir.path.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dest).unwrap();
        let first = src_dir.join("first.bin");
        let second = src_dir.join("second.bin");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();

        let driver = FaultyDriver::fail_read(&first);
        let (tx, mut rx) = mpsc::channel(64);
        let (decision_tx, mut decision_rx) = mpsc::channel(2);
        respond(&decision_tx, "skip");

        run_with_driver(
            FileOpType::Copy,
            &[
                file_entry("first.bin", &first, 5),
                file_entry("second.bin", &second, 6),
            ],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(!dest.join("first.bin").exists());
        assert_eq!(
            fs::read_to_string(dest.join("second.bin")).unwrap(),
            "second"
        );
        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 1);
        assert_eq!(events.skipped.len(), 1);
        assert_eq!(events.skipped[0].path, first.to_string_lossy());
        assert!(events.skipped[0].notify);
    }

    #[test]
    fn skip_all_errors_does_not_prompt_for_later_files() {
        let dir = TestDir::new("error-skip-all");
        let src_dir = dir.path.join("src");
        let dest = dir.path.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dest).unwrap();
        let first = src_dir.join("first.bin");
        let second = src_dir.join("second.bin");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();

        let driver = FaultyDriver::all_reads_fail();
        let (tx, mut rx) = mpsc::channel(64);
        let (decision_tx, mut decision_rx) = mpsc::channel(2);
        respond(&decision_tx, "skip-all");

        run_with_driver(
            FileOpType::Copy,
            &[
                file_entry("first.bin", &first, 5),
                file_entry("second.bin", &second, 6),
            ],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 1);
        assert_eq!(events.skipped.len(), 2);
        assert!(events.skipped.iter().all(|item| !item.notify));
    }

    /// Copies against real permission errors from the local filesystem, not
    /// injected driver failures. Root ignores mode bits, so these skip as root.
    #[cfg(unix)]
    mod access {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn is_root() -> bool {
            // SAFETY: geteuid has no preconditions and cannot fail.
            unsafe { libc::geteuid() == 0 }
        }

        /// Sets a mode and restores a removable one on drop, so the temp dir
        /// cleans up even when an assertion fails.
        struct Mode(PathBuf);

        impl Mode {
            fn set(path: &Path, mode: u32) -> Self {
                fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
                Self(path.to_path_buf())
            }
        }

        impl Drop for Mode {
            fn drop(&mut self) {
                let mode = if self.0.is_dir() { 0o755 } else { 0o644 };
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(mode));
            }
        }

        fn run_local(
            files: &[FileOpEntry],
            dest: &Path,
            error_policy: ErrorPolicy,
            tx: &mpsc::Sender<ServerMessage>,
            decision_rx: &mut DecisionRx,
        ) -> Result<(), String> {
            let (_cancel_tx, cancel_rx) = watch::channel(false);
            let (_pause_tx, pause_rx) = watch::channel(false);
            let local = crate::vfs::local::LocalFsDriver;
            run_file_op_blocking_with_policy(
                "job-test",
                &FileOpType::Copy,
                files,
                Some(&dest.to_string_lossy()),
                &ConflictPolicy::Ask,
                &error_policy,
                tx,
                &cancel_rx,
                &pause_rx,
                decision_rx,
                &local,
                Some(&local),
                false,
                &VerificationPolicy::Fail,
                false,
                false,
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicBool::new(false)),
            )
        }

        /// Wait for the next decision and return its kind and message.
        fn next_decision(rx: &mut mpsc::Receiver<ServerMessage>) -> (String, String) {
            loop {
                match rx.blocking_recv() {
                    Some(ServerMessage::JobEvent { event, decision: Some(decision), .. })
                        if event == "DecisionRequired" =>
                    {
                        return (
                            decision.kind.unwrap_or_default(),
                            decision.message.unwrap_or_default(),
                        );
                    }
                    Some(_) => {}
                    None => panic!("job ended without asking for a decision"),
                }
            }
        }

        fn fixture(name: &str) -> (TestDir, PathBuf, PathBuf) {
            let dir = TestDir::new(name);
            let tree = dir.path.join("src").join("tree");
            let dest = dir.path.join("dst");
            fs::create_dir_all(&tree).unwrap();
            fs::create_dir_all(&dest).unwrap();
            fs::write(tree.join("blocked.bin"), "blocked payload\n").unwrap();
            fs::write(tree.join("good.txt"), "copied payload\n").unwrap();
            (dir, tree, dest)
        }

        #[test]
        fn unreadable_source_prompts_then_skip_copies_the_rest() {
            if is_root() {
                eprintln!("[file_ops] skipped: root ignores file modes");
                return;
            }
            let (_dir, tree, dest) = fixture("access-skip");
            let blocked = tree.join("blocked.bin");
            let mode = Mode::set(&blocked, 0o000);

            let (tx, mut rx) = mpsc::channel(256);
            let (decision_tx, mut decision_rx) = mpsc::channel(2);
            std::thread::scope(|scope| {
                let job = scope.spawn(|| {
                    run_local(&[dir_entry("tree", &tree)], &dest, ErrorPolicy::Ask, &tx, &mut decision_rx)
                });
                let (kind, message) = next_decision(&mut rx);
                assert_eq!(kind, "error");
                assert!(message.contains("blocked.bin"), "{message}");
                assert!(message.to_lowercase().contains("permission"), "{message}");
                decision_tx
                    .blocking_send(DecisionResponse { decision: "skip".into(), apply_to_all: false })
                    .unwrap();
                job.join().unwrap().unwrap();
            });

            let copied = dest.join("tree");
            assert_eq!(fs::read_to_string(copied.join("good.txt")).unwrap(), "copied payload\n");
            assert!(!copied.join("blocked.bin").exists(), "skip must not leave a destination file");
            drop(mode);
            assert_eq!(fs::read_to_string(&blocked).unwrap(), "blocked payload\n");
        }

        #[test]
        fn unreadable_source_retry_succeeds_after_access_is_fixed() {
            if is_root() {
                eprintln!("[file_ops] skipped: root ignores file modes");
                return;
            }
            let (_dir, tree, dest) = fixture("access-retry");
            let blocked = tree.join("blocked.bin");
            let mode = Mode::set(&blocked, 0o000);

            let (tx, mut rx) = mpsc::channel(256);
            let (decision_tx, mut decision_rx) = mpsc::channel(2);
            std::thread::scope(|scope| {
                let job = scope.spawn(|| {
                    run_local(&[dir_entry("tree", &tree)], &dest, ErrorPolicy::Ask, &tx, &mut decision_rx)
                });
                let (kind, _) = next_decision(&mut rx);
                assert_eq!(kind, "error");
                fs::set_permissions(&blocked, fs::Permissions::from_mode(0o644)).unwrap();
                decision_tx
                    .blocking_send(DecisionResponse { decision: "retry".into(), apply_to_all: false })
                    .unwrap();
                job.join().unwrap().unwrap();
            });
            drop(mode);

            let copied = dest.join("tree");
            assert_eq!(fs::read_to_string(copied.join("blocked.bin")).unwrap(), "blocked payload\n");
            assert_eq!(fs::read_to_string(copied.join("good.txt")).unwrap(), "copied payload\n");
        }

        #[test]
        fn read_only_destination_with_skip_policy_records_every_file() {
            if is_root() {
                eprintln!("[file_ops] skipped: root ignores file modes");
                return;
            }
            let (_dir, tree, dest) = fixture("access-readonly-dest");
            let target = dest.join("tree");
            fs::create_dir_all(&target).unwrap();
            let mode = Mode::set(&target, 0o555);

            let (tx, mut rx) = mpsc::channel(256);
            let (_decision_tx, mut decision_rx) = mpsc::channel(2);
            run_local(&[dir_entry("tree", &tree)], &dest, ErrorPolicy::Skip, &tx, &mut decision_rx)
                .unwrap();
            drop(mode);

            let events = collect_events(&mut rx);
            assert_eq!(events.error_prompts, 0);
            assert_eq!(events.skipped.len(), 2);
            assert!(events.skipped.iter().all(|item| !item.notify));
            assert!(events
                .skipped
                .iter()
                .all(|item| item.reason.to_lowercase().contains("permission")));
            assert!(fs::read_dir(&target).unwrap().next().is_none());
            assert!(tree.join("blocked.bin").exists() && tree.join("good.txt").exists());
        }
    }

    #[test]
    fn move_rename_error_skip_keeps_source_and_copies_nothing() {
        let dir = TestDir::new("move-rename-skip");
        let src_dir = dir.path.join("src");
        let dest = dir.path.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dest).unwrap();
        let file = src_dir.join("keep.txt");
        fs::write(&file, "keep").unwrap();

        let driver = FaultyDriver::renames_fail();
        let (tx, mut rx) = mpsc::channel(64);
        let (_decision_tx, mut decision_rx) = mpsc::channel(2);
        run_with_driver(
            FileOpType::Move,
            &[file_entry("keep.txt", &file, 4)],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Ask,
            ErrorPolicy::Skip,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&file).unwrap(), "keep");
        assert!(!dest.join("keep.txt").exists());
        assert_eq!(collect_events(&mut rx).skipped.len(), 1);
    }

    #[test]
    fn error_before_first_write_skip_keeps_overwritten_destination() {
        let dir = TestDir::new("skip-keeps-destination");
        let src_dir = dir.path.join("src");
        let dest = dir.path.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dest).unwrap();
        let file = src_dir.join("report.pdf");
        fs::write(&file, "new").unwrap();
        fs::write(dest.join("report.pdf"), "existing").unwrap();

        let driver = FaultyDriver::fail_read(&file);
        let (tx, _rx) = mpsc::channel(64);
        let (_decision_tx, mut decision_rx) = mpsc::channel(2);
        run_with_driver(
            FileOpType::Copy,
            &[file_entry("report.pdf", &file, 3)],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Skip,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(fs::read_to_string(dest.join("report.pdf")).unwrap(), "existing");
    }

    #[test]
    fn conflict_skip_with_apply_to_all_is_silent() {
        let dir = TestDir::new("conflict-skip-all-silent");
        let src_dir = dir.path.join("src");
        let dest = dir.path.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dest).unwrap();
        let mut entries = Vec::new();
        for name in ["a.txt", "b.txt", "c.txt"] {
            fs::write(src_dir.join(name), "new").unwrap();
            fs::write(dest.join(name), "old").unwrap();
            entries.push(file_entry(name, &src_dir.join(name), 3));
        }

        let driver = FaultyDriver::healthy();
        let (tx, mut rx) = mpsc::channel(64);
        let (decision_tx, mut decision_rx) = mpsc::channel(2);
        decision_tx
            .try_send(DecisionResponse { decision: "skip".to_string(), apply_to_all: true })
            .unwrap();
        run_with_driver(
            FileOpType::Copy,
            &entries,
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Ask,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        let events = collect_events(&mut rx);
        assert_eq!(events.conflict_prompts, 1);
        assert_eq!(events.skipped.len(), 3);
        assert!(events.skipped.iter().all(|item| !item.notify));
        assert_eq!(fs::read_to_string(dest.join("c.txt")).unwrap(), "old");
    }

    #[test]
    fn delete_folder_progress_never_exceeds_total() {
        let dir = TestDir::new("delete-progress");
        let folder = dir.path.join("folder");
        fs::create_dir_all(&folder).unwrap();
        for name in ["1", "2", "3"] {
            fs::write(folder.join(name), name).unwrap();
        }

        let driver = FaultyDriver::healthy();
        let (tx, mut rx) = mpsc::channel(64);
        let (_decision_tx, mut decision_rx) = mpsc::channel(2);
        run_with_driver(
            FileOpType::Delete,
            &[dir_entry("folder", &folder)],
            None,
            ConflictPolicy::Ask,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(!folder.exists());
        let mut last = None;
        while let Ok(message) = rx.try_recv() {
            if let ServerMessage::JobEvent { progress: Some(progress), .. } = message {
                assert!(progress.completed_files <= progress.total_files);
                last = Some((progress.completed_files, progress.total_files));
            }
        }
        assert_eq!(last, Some((3, 3)));
    }

    #[test]
    fn error_policy_skip_never_prompts() {
        let dir = TestDir::new("error-policy-skip");
        let src_dir = dir.path.join("src");
        let dest = dir.path.join("dst");
        fs::create_dir_all(&src_dir).unwrap();
        fs::create_dir_all(&dest).unwrap();
        let first = src_dir.join("first.bin");
        let second = src_dir.join("second.bin");
        fs::write(&first, "first").unwrap();
        fs::write(&second, "second").unwrap();

        let driver = FaultyDriver::all_reads_fail();
        let (tx, mut rx) = mpsc::channel(64);
        let (_decision_tx, mut decision_rx) = mpsc::channel(2);

        run_with_driver(
            FileOpType::Copy,
            &[
                file_entry("first.bin", &first, 5),
                file_entry("second.bin", &second, 6),
            ],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Skip,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 0);
        assert_eq!(events.skipped.len(), 2);
        assert!(events.skipped.iter().all(|item| !item.notify));
    }

    #[test]
    fn delete_skips_failing_child_and_keeps_its_parents() {
        let dir = TestDir::new("delete-skip-child");
        let folder = dir.path.join("folder");
        fs::create_dir_all(&folder).unwrap();
        let good = folder.join("good.txt");
        let bad = folder.join("bad.txt");
        fs::write(&good, "good").unwrap();
        fs::write(&bad, "bad").unwrap();

        let driver = FaultyDriver::fail_remove(&bad);
        let (tx, mut rx) = mpsc::channel(64);
        let (decision_tx, mut decision_rx) = mpsc::channel(2);
        respond(&decision_tx, "skip");

        run_with_driver(
            FileOpType::Delete,
            &[dir_entry("folder", &folder)],
            None,
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(folder.exists());
        assert!(!good.exists());
        assert!(bad.exists());
        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 1);
        assert_eq!(events.skipped.len(), 1);
        assert_eq!(events.skipped[0].path, bad.to_string_lossy());
        assert_eq!(events.skipped[0].op_type, "delete");
        assert!(events.skipped[0].notify);
    }

    #[test]
    fn move_conflict_skipped_child_keeps_source() {
        let dir = TestDir::new("move-conflict-skip");
        let src_dir = dir.path.join("src");
        let dest = dir.path.join("dst");
        fs::create_dir_all(src_dir.join("folder")).unwrap();
        fs::create_dir_all(dest.join("folder")).unwrap();
        fs::write(src_dir.join("folder/keep.txt"), "new").unwrap();
        fs::write(dest.join("folder/keep.txt"), "old").unwrap();
        let folder = src_dir.join("folder");

        let driver = FaultyDriver::healthy();
        let (tx, mut rx) = mpsc::channel(64);
        let (decision_tx, mut decision_rx) = mpsc::channel(2);
        respond(&decision_tx, "skip");

        run_with_driver(
            FileOpType::Move,
            &[dir_entry("folder", &folder)],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Ask,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(folder.join("keep.txt")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(dest.join("folder/keep.txt")).unwrap(),
            "old"
        );
        let events = collect_events(&mut rx);
        assert_eq!(events.conflict_prompts, 1);
        assert_eq!(events.skipped.len(), 1);
        assert_eq!(events.skipped[0].path, folder.join("keep.txt").to_string_lossy());
        assert!(events.skipped[0].notify);
    }

    #[test]
    fn move_progress_skipped_file_keeps_source() {
        let dir = TestDir::new("move-progress-skip");
        let src_dir = dir.path.join("src");
        let dest = dir.path.join("dst");
        fs::create_dir_all(src_dir.join("folder")).unwrap();
        fs::create_dir_all(&dest).unwrap();
        let file = src_dir.join("folder/skipped.txt");
        fs::write(&file, "payload").unwrap();

        let driver = FaultyDriver::healthy();
        let (tx, mut rx) = mpsc::channel(64);
        let (_decision_tx, mut decision_rx) = mpsc::channel(2);

        run_with_driver(
            FileOpType::Move,
            &[dir_entry("folder", &src_dir.join("folder"))],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(true)),
        )
        .unwrap();

        assert!(file.exists());
        assert!(!dest.join("folder/skipped.txt").exists());
        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 0);
        assert_eq!(events.skipped.len(), 1);
        assert!(events.skipped[0].notify);
    }

    #[test]
    fn move_error_skipped_child_keeps_source() {
        let dir = TestDir::new("move-error-skip");
        let src_dir = dir.path.join("src");
        let dest = dir.path.join("dst");
        fs::create_dir_all(src_dir.join("folder")).unwrap();
        fs::create_dir_all(&dest).unwrap();
        let good = src_dir.join("folder/good.txt");
        let bad = src_dir.join("folder/bad.txt");
        fs::write(&good, "good").unwrap();
        fs::write(&bad, "bad").unwrap();

        let driver = FaultyDriver::fail_read(&bad);
        let (tx, mut rx) = mpsc::channel(64);
        let (decision_tx, mut decision_rx) = mpsc::channel(2);
        respond(&decision_tx, "skip");

        run_with_driver(
            FileOpType::Move,
            &[dir_entry("folder", &src_dir.join("folder"))],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(bad.exists());
        assert!(!dest.join("folder/bad.txt").exists());
        assert!(dest.join("folder/good.txt").exists());
        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 1);
        assert_eq!(events.skipped.len(), 1);
        assert_eq!(events.skipped[0].path, bad.to_string_lossy());
    }

    #[test]
    fn full_move_removes_the_source_folder_after_copy_path() {
        let dir = TestDir::new("move-full");
        let src_dir = dir.path.join("src");
        let dest = dir.path.join("dst");
        fs::create_dir_all(src_dir.join("folder/nested")).unwrap();
        fs::create_dir_all(&dest).unwrap();
        fs::write(src_dir.join("folder/top.txt"), "top").unwrap();
        fs::write(src_dir.join("folder/nested/deep.txt"), "deep").unwrap();
        let folder = src_dir.join("folder");

        let driver = FaultyDriver::healthy();
        let (tx, mut rx) = mpsc::channel(64);
        let (_decision_tx, mut decision_rx) = mpsc::channel(2);

        run_with_driver(
            FileOpType::Move,
            &[dir_entry("folder", &folder)],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(!folder.exists());
        assert_eq!(
            fs::read_to_string(dest.join("folder/top.txt")).unwrap(),
            "top"
        );
        assert_eq!(
            fs::read_to_string(dest.join("folder/nested/deep.txt")).unwrap(),
            "deep"
        );
        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 0);
        assert!(events.skipped.is_empty());
    }

    #[test]
    fn move_skips_a_source_removal_failure_without_deleting_the_destination() {
        let dir = TestDir::new("move-remove-source-failure");
        let src = dir.path.join("source.txt");
        let dest = dir.path.join("dst");
        fs::write(&src, "payload").unwrap();
        fs::create_dir_all(&dest).unwrap();

        let driver = FaultyDriver::fail_remove(&src);
        let (tx, mut rx) = mpsc::channel(64);
        let (decision_tx, mut decision_rx) = mpsc::channel(2);
        respond(&decision_tx, "skip");

        run_with_driver(
            FileOpType::Move,
            &[file_entry("source.txt", &src, 7)],
            Some(&dest.to_string_lossy()),
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(src.exists());
        assert_eq!(
            fs::read_to_string(dest.join("source.txt")).unwrap(),
            "payload"
        );
        let events = collect_events(&mut rx);
        assert_eq!(events.error_prompts, 1);
        assert_eq!(events.skipped.len(), 1);
        assert!(events.skipped[0].path.ends_with("source.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn delete_removes_symlink_to_folder_without_following_it() {
        use std::os::unix::fs::symlink;

        let dir = TestDir::new("delete-symlink-folder");
        let folder = dir.path.join("folder");
        let target = dir.path.join("target");
        fs::create_dir_all(&folder).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("kept.txt"), "kept").unwrap();
        symlink(&target, folder.join("link")).unwrap();

        let driver = crate::vfs::local::LocalFsDriver;
        let (tx, mut rx) = mpsc::channel(64);
        let (_decision_tx, mut decision_rx) = mpsc::channel(2);

        run_with_driver(
            FileOpType::Delete,
            &[dir_entry("folder", &folder)],
            None,
            ConflictPolicy::Overwrite,
            ErrorPolicy::Ask,
            &tx,
            &mut decision_rx,
            &driver,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(!folder.exists());
        assert_eq!(fs::read_to_string(target.join("kept.txt")).unwrap(), "kept");
        assert!(collect_events(&mut rx).skipped.is_empty());
    }
}
