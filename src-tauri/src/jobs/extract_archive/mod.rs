use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::{mpsc, watch};

use crate::jobs::progress::{percent, send_progress_sync, ProgressReporter};
use crate::state::settings::SevenzipExtractorMode;
use crate::vfs::{VfsDriver, VfsError};
use crate::ws::protocol::{now, FileOpProgressSchema, ServerMessage};

mod native_7z;

/// Options passed from the WS dispatch into the extract worker. Lets the
/// caller resolve settings (which binary to use, etc.) once, in async land,
/// and hand a snapshot to the blocking thread.
#[derive(Clone, Debug, Default)]
pub struct ExtractOptions {
    /// When Some, run extraction through this 7z-CLI binary instead of the
    /// in-process sevenz-rust2 path. Only consulted for 7z archives — zip
    /// always uses the in-process zip crate.
    pub sevenzip_binary: Option<PathBuf>,
}

/// Resolve the 7-Zip CLI binary the current settings ask for. Returns None
/// if the user picked Internal mode or no binary is installed in Auto mode.
///
/// In Custom mode we honour the path even if it doesn't exist — the FE
/// surfaces the missing-file state in the settings panel and the extract
/// job will fall back to internal at run time anyway.
pub fn resolve_sevenzip_binary(
    mode: SevenzipExtractorMode,
    custom_path: &str,
) -> Option<PathBuf> {
    match mode {
        SevenzipExtractorMode::Internal => None,
        SevenzipExtractorMode::Custom => {
            let trimmed = custom_path.trim();
            if trimmed.is_empty() {
                None
            } else {
                let p = PathBuf::from(trimmed);
                if p.exists() {
                    Some(p)
                } else {
                    None
                }
            }
        }
        SevenzipExtractorMode::Auto => find_sevenzip_on_path(),
    }
}

/// Walk $PATH looking for a 7-Zip CLI binary. Tries the common executable
/// names (and Windows variants of each).
pub fn find_sevenzip_on_path() -> Option<PathBuf> {
    #[cfg(windows)]
    let exe_suffixes: &[&str] = &[".exe", ""];
    #[cfg(not(windows))]
    let exe_suffixes: &[&str] = &[""];

    // 7z = full 7-Zip, 7zz = standalone v23+, 7za = reduced standalone.
    let candidates = ["7z", "7zz", "7za"];

    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in &candidates {
            for suffix in exe_suffixes {
                let candidate = dir.join(format!("{name}{suffix}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    // Windows: common install location not always on PATH.
    #[cfg(windows)]
    {
        for base in [
            std::env::var_os("ProgramFiles").map(PathBuf::from),
            std::env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        ]
        .into_iter()
        .flatten()
        {
            let candidate = base.join("7-Zip").join("7z.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

/// Walk an archive in one streaming pass, extracting all `targets` directly to
/// their `dest_path` on disk. Reuses the same `JobEvent` shape as `run_file_op`
/// so the FE Operation modal and Jobs bar work without changes.
///
/// `targets` is `(inner_path_in_archive, absolute_dest_path)`. The caller is
/// responsible for expanding directory selections into the flat file list.
/// Monotonic counter of archive extractions since process start. Bumped at
/// the head of `run_extract_archive` so every extraction — native 7z,
/// internal 7z, internal zip — is accounted for in stderr regardless of
/// which path the dispatcher took.
static EXTRACT_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub async fn run_extract_archive(
    job_id: String,
    src_driver: Arc<dyn VfsDriver>,
    targets: Vec<(String, PathBuf)>,
    total_bytes: u64,
    options: ExtractOptions,
    tx: mpsc::Sender<ServerMessage>,
    mut cancel_rx: watch::Receiver<bool>,
) {
    if targets.is_empty() {
        send_terminal(&tx, &job_id, "Completed", None, None).await;
        return;
    }

    let extract_seq = EXTRACT_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
    let driver_kind = describe_driver(&src_driver);
    let extractor_kind = if options.sevenzip_binary.is_some() && driver_kind == "7z" {
        format!("native({})", options
            .sevenzip_binary
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default())
    } else {
        format!("internal-{driver_kind}")
    };
    let start = std::time::Instant::now();
    eprintln!(
        "[fbro] extract #{extract_seq} START job={job_id} driver={driver_kind} extractor={extractor_kind} files={} bytes={total_bytes}",
        targets.len()
    );

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_flag_for_task = Arc::clone(&cancel_flag);
    let cancel_task = tokio::spawn(async move {
        while cancel_rx.changed().await.is_ok() {
            if *cancel_rx.borrow() {
                cancel_flag_for_task.store(true, Ordering::SeqCst);
                break;
            }
        }
    });

    let total_files = targets.len();
    let reporter = ProgressReporter::new(
        tx.clone(),
        job_id.clone(),
        total_files,
        total_bytes,
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicU64::new(0)),
        Arc::clone(&cancel_flag),
    );

    let driver_for_worker = Arc::clone(&src_driver);
    let reporter_for_worker = reporter.clone();
    let targets_for_worker = targets.clone();
    let options_for_worker = options.clone();
    let cancel_flag_for_worker = Arc::clone(&cancel_flag);

    let worker = tokio::task::spawn_blocking(move || {
        extract_blocking(
            &driver_for_worker,
            &targets_for_worker,
            &reporter_for_worker,
            &options_for_worker,
            &cancel_flag_for_worker,
        )
    });

    let outcome = worker.await;
    cancel_task.abort();

    let elapsed = start.elapsed();
    match outcome {
        Ok(Ok(())) => {
            eprintln!(
                "[fbro] extract #{extract_seq} OK   job={job_id} files={total_files} bytes={total_bytes} elapsed={elapsed:?}"
            );
            send_progress_sync(
                &tx,
                &job_id,
                FileOpProgressSchema {
                    total_files,
                    completed_files: total_files,
                    current_file: None,
                    status_text: Some("Extraction complete".to_string()),
                    current_file_bytes_total: total_bytes,
                    current_file_bytes_copied: total_bytes,
                    total_bytes_total: total_bytes,
                    total_bytes_copied: total_bytes,
                    current_file_percent: Some(100),
                    overall_percent: percent(total_bytes, total_bytes).or(Some(100)),
                },
            );
            send_terminal(&tx, &job_id, "Completed", None, None).await;
        }
        Ok(Err(e)) => {
            let cancelled = cancel_flag.load(Ordering::SeqCst)
                || matches!(&e, VfsError::IOError(msg) if msg == "Cancelled");
            if cancelled {
                eprintln!(
                    "[fbro] extract #{extract_seq} CANCEL job={job_id} elapsed={elapsed:?}"
                );
                send_terminal(&tx, &job_id, "Cancelled", None, None).await;
            } else {
                eprintln!(
                    "[fbro] extract #{extract_seq} FAIL job={job_id} elapsed={elapsed:?} err={e}"
                );
                send_terminal(&tx, &job_id, "Failed", Some(format!("{e}")), None).await;
            }
        }
        Err(join_err) => {
            eprintln!(
                "[fbro] extract #{extract_seq} PANIC job={job_id} elapsed={elapsed:?} err={join_err}"
            );
            send_terminal(
                &tx,
                &job_id,
                "Failed",
                Some(format!("Extraction worker panicked: {join_err}")),
                None,
            )
            .await;
        }
    }
}

fn extract_blocking(
    driver: &Arc<dyn VfsDriver>,
    targets: &[(String, PathBuf)],
    reporter: &ProgressReporter,
    options: &ExtractOptions,
    cancel_flag: &AtomicBool,
) -> Result<(), VfsError> {
    if let Some(sz) = driver
        .as_ref()
        .as_any()
        .downcast_ref::<crate::vfs::seven_zip::SevenZipFsDriver>()
    {
        if let Some(binary) = options.sevenzip_binary.as_deref() {
            match native_7z::extract_via_native_7z(
                binary,
                Path::new(sz.archive_path()),
                targets,
                reporter,
                cancel_flag,
            ) {
                Ok(()) => return Ok(()),
                Err(native_7z::NativeError::Unsupported(reason)) => {
                    eprintln!(
                        "[fbro] native 7z extract not usable ({reason}); using internal extractor"
                    );
                }
                Err(native_7z::NativeError::Fatal(e)) => return Err(e),
            }
        }
        return sz.extract_many(targets, reporter);
    }
    if let Some(zp) = driver
        .as_ref()
        .as_any()
        .downcast_ref::<crate::vfs::zip::ZipFsDriver>()
    {
        return zp.extract_many(targets, reporter);
    }
    Err(VfsError::IOError(
        "Driver does not support batch archive extraction".to_string(),
    ))
}

/// Returns true if `driver` is a 7z or zip archive driver — used by the
/// dispatcher in `ws::intents::jobs` to pick the extract path over the
/// generic file-op copy worker.
pub fn driver_is_archive(driver: &Arc<dyn VfsDriver>) -> bool {
    let any = driver.as_ref().as_any();
    any.downcast_ref::<crate::vfs::seven_zip::SevenZipFsDriver>()
        .is_some()
        || any.downcast_ref::<crate::vfs::zip::ZipFsDriver>().is_some()
}

/// Short string describing the archive type, used for the per-extraction
/// stderr log line.
fn describe_driver(driver: &Arc<dyn VfsDriver>) -> &'static str {
    let any = driver.as_ref().as_any();
    if any
        .downcast_ref::<crate::vfs::seven_zip::SevenZipFsDriver>()
        .is_some()
    {
        "7z"
    } else if any
        .downcast_ref::<crate::vfs::zip::ZipFsDriver>()
        .is_some()
    {
        "zip"
    } else {
        "unknown"
    }
}

async fn send_terminal(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    event: &str,
    error: Option<String>,
    result_path: Option<String>,
) {
    let _ = tx
        .send(ServerMessage::JobEvent {
            timestamp: now(),
            job_id: job_id.to_string(),
            event: event.to_string(),
            files: None,
            progress: None,
            search_progress: None,
            decision: None,
            error,
            result_path,
            result_panel_id: None,
        })
        .await;
}
