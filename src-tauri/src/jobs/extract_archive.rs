use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::{mpsc, watch};

use crate::jobs::progress::{percent, send_progress_sync, ProgressReporter};
use crate::vfs::{VfsDriver, VfsError};
use crate::ws::protocol::{now, FileOpProgressSchema, ServerMessage};

/// Walk an archive in one streaming pass, extracting all `targets` directly to
/// their `dest_path` on disk. Reuses the same `JobEvent` shape as `run_file_op`
/// so the FE Operation modal and Jobs bar work without changes.
///
/// `targets` is `(inner_path_in_archive, absolute_dest_path)`. The caller is
/// responsible for expanding directory selections into the flat file list.
pub async fn run_extract_archive(
    job_id: String,
    src_driver: Arc<dyn VfsDriver>,
    targets: Vec<(String, PathBuf)>,
    total_bytes: u64,
    tx: mpsc::Sender<ServerMessage>,
    mut cancel_rx: watch::Receiver<bool>,
) {
    if targets.is_empty() {
        send_terminal(&tx, &job_id, "Completed", None, None).await;
        return;
    }

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

    let worker = tokio::task::spawn_blocking(move || {
        extract_blocking(&driver_for_worker, &targets_for_worker, &reporter_for_worker)
    });

    let outcome = worker.await;
    cancel_task.abort();

    match outcome {
        Ok(Ok(())) => {
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
                send_terminal(&tx, &job_id, "Cancelled", None, None).await;
            } else {
                send_terminal(&tx, &job_id, "Failed", Some(format!("{e}")), None).await;
            }
        }
        Err(join_err) => {
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
) -> Result<(), VfsError> {
    if let Some(sz) = driver
        .as_ref()
        .as_any()
        .downcast_ref::<crate::vfs::seven_zip::SevenZipFsDriver>()
    {
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
