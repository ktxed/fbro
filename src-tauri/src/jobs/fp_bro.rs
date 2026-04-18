mod archive;
mod seven_zip;
mod zip;

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::bro::LaunchRecipe;
use crate::bro::{BroInputItem, ResolvedBro};
use crate::ws::protocol::{now, FileOpProgressSchema, ServerMessage};

use self::archive::{ArchiveRunError, ArchiveSummary};

pub async fn run_fp_bro(
    job_id: String,
    bro: ResolvedBro,
    inputs: Vec<BroInputItem>,
    output_path: String,
    params: HashMap<String, String>,
    tx: mpsc::Sender<ServerMessage>,
    cancel_rx: watch::Receiver<bool>,
) {
    let _ = send_progress(
        &tx,
        &job_id,
        FileOpProgressSchema {
            total_files: inputs.len(),
            completed_files: 0,
            current_file: Some(format!("Starting {}", bro.option.name)),
            status_text: Some(format!("Starting {}", bro.option.name)),
            current_file_bytes_total: 0,
            current_file_bytes_copied: 0,
            total_bytes_total: 0,
            total_bytes_copied: 0,
            current_file_percent: None,
            overall_percent: None,
        },
    )
    .await;

    match bro.launch.clone() {
        LaunchRecipe::Command { .. } => {
            run_legacy_fp_bro(job_id, bro, inputs, output_path, params, tx, cancel_rx).await;
        }
        LaunchRecipe::BuiltInSevenZip => {
            run_builtin_archive(
                job_id,
                bro,
                inputs,
                output_path,
                params,
                tx,
                cancel_rx,
                seven_zip::run_blocking,
            )
            .await;
        }
        LaunchRecipe::BuiltInZip => {
            run_builtin_archive(
                job_id,
                bro,
                inputs,
                output_path,
                params,
                tx,
                cancel_rx,
                zip::run_blocking,
            )
            .await;
        }
    }
}

async fn run_legacy_fp_bro(
    job_id: String,
    bro: ResolvedBro,
    inputs: Vec<BroInputItem>,
    output_path: String,
    params: HashMap<String, String>,
    tx: mpsc::Sender<ServerMessage>,
    mut cancel_rx: watch::Receiver<bool>,
) {
    let total_files = inputs.len();
    let (program, args, cwd) = match bro.build_command(&inputs, Some(&output_path), &params) {
        Ok(command) => command,
        Err(error) => {
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None).await;
            return;
        }
    };

    let mut command = Command::new(&program);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error.to_string()), None).await;
            return;
        }
    };

    let wait_result = tokio::select! {
        status = child.wait() => status.map_err(|error| error.to_string()),
        changed = cancel_rx.changed() => {
            match changed {
                Ok(_) if *cancel_rx.borrow() => {
                    let _ = child.kill().await;
                    Err("Cancelled".to_string())
                }
                Ok(_) => child.wait().await.map_err(|error| error.to_string()),
                Err(_) => child.wait().await.map_err(|error| error.to_string()),
            }
        }
    };

    match wait_result {
        Ok(status) if status.success() => {
            let _ = send_progress(
                &tx,
                &job_id,
                FileOpProgressSchema {
                    total_files,
                    completed_files: total_files,
                    current_file: None,
                    status_text: Some(format!("{} completed", bro.option.name)),
                    current_file_bytes_total: 0,
                    current_file_bytes_copied: 0,
                    total_bytes_total: 0,
                    total_bytes_copied: 0,
                    current_file_percent: None,
                    overall_percent: Some(100),
                },
            )
            .await;
            let _ = send_terminal(&tx, &job_id, "Completed", None, Some(output_path)).await;
        }
        Err(error) if error == "Cancelled" => {
            let _ = send_terminal(&tx, &job_id, "Cancelled", None, None).await;
        }
        Ok(status) => {
            let error = status
                .code()
                .map(|code| format!("{} exited with status {}", bro.option.name, code))
                .unwrap_or_else(|| format!("{} failed", bro.option.name));
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None).await;
        }
        Err(error) => {
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None).await;
        }
    }
}

async fn run_builtin_archive(
    job_id: String,
    bro: ResolvedBro,
    inputs: Vec<BroInputItem>,
    output_path: String,
    params: HashMap<String, String>,
    tx: mpsc::Sender<ServerMessage>,
    mut cancel_rx: watch::Receiver<bool>,
    run_blocking: fn(
        &str,
        &str,
        &[BroInputItem],
        &str,
        &HashMap<String, String>,
        &mpsc::Sender<ServerMessage>,
        Arc<AtomicBool>,
    ) -> Result<ArchiveSummary, ArchiveRunError>,
) {
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

    let tx_for_worker = tx.clone();
    let job_id_for_worker = job_id.clone();
    let bro_name = bro.option.name.clone();
    let output_path_for_worker = output_path.clone();
    let worker = tokio::task::spawn_blocking(move || {
        run_blocking(
            &job_id_for_worker,
            &bro_name,
            &inputs,
            &output_path_for_worker,
            &params,
            &tx_for_worker,
            cancel_flag,
        )
    });

    let result = worker.await;
    cancel_task.abort();

    match result {
        Ok(Ok(summary)) => {
            let _ = send_progress(
                &tx,
                &job_id,
                FileOpProgressSchema {
                    total_files: summary.total_files,
                    completed_files: summary.total_files,
                    current_file: None,
                    status_text: Some(format!("{} completed", bro.option.name)),
                    current_file_bytes_total: 0,
                    current_file_bytes_copied: 0,
                    total_bytes_total: summary.total_bytes_total,
                    total_bytes_copied: summary.total_bytes_total,
                    current_file_percent: Some(100),
                    overall_percent: Some(100),
                },
            )
            .await;
            let _ = send_terminal(&tx, &job_id, "Completed", None, Some(output_path)).await;
        }
        Ok(Err(ArchiveRunError::Cancelled)) => {
            let _ = send_terminal(&tx, &job_id, "Cancelled", None, None).await;
        }
        Ok(Err(ArchiveRunError::Failed(error))) => {
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None).await;
        }
        Err(error) => {
            let _ = send_terminal(
                &tx,
                &job_id,
                "Failed",
                Some(format!("{} failed: {error}", bro.option.name)),
                None,
            )
            .await;
        }
    }
}
async fn send_progress(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    progress: FileOpProgressSchema,
) -> Result<(), ()> {
    tx.send(ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: "Progress".to_string(),
        progress: Some(progress),
        search_progress: None,
        decision: None,
        error: None,
        result_path: None,
        result_panel_id: None,
    })
    .await
    .map_err(|_| ())
}

async fn send_terminal(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    event: &str,
    error: Option<String>,
    result_path: Option<String>,
) -> Result<(), ()> {
    tx.send(ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: event.to_string(),
        progress: None,
        search_progress: None,
        decision: None,
        error,
        result_path,
        result_panel_id: None,
    })
    .await
    .map_err(|_| ())
}
