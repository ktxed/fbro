use tokio::process::Command;
use tokio::sync::{mpsc, watch};

use crate::bro::{BroInputItem, ResolvedBro};
use crate::ws::protocol::{now, FileOpProgressSchema, ServerMessage};

pub async fn run_fp_bro(
    job_id: String,
    bro: ResolvedBro,
    inputs: Vec<BroInputItem>,
    output_path: String,
    params: std::collections::HashMap<String, String>,
    tx: mpsc::Sender<ServerMessage>,
    mut cancel_rx: watch::Receiver<bool>,
) {
    let total_files = inputs.len();
    let initial_progress = FileOpProgressSchema {
        total_files,
        completed_files: 0,
        current_file: Some(format!("Running {}", bro.option.name)),
        current_file_bytes_total: 0,
        current_file_bytes_copied: 0,
        total_bytes_total: 0,
        total_bytes_copied: 0,
    };
    let _ = tx
        .send(ServerMessage::JobEvent {
            timestamp: now(),
            job_id: job_id.clone(),
            event: "Progress".to_string(),
            progress: Some(initial_progress),
            decision: None,
            error: None,
            result_path: None,
            result_panel_id: None,
        })
        .await;

    let (program, args, cwd) = match bro.build_command(&inputs, Some(&output_path), &params) {
        Ok(command) => command,
        Err(error) => {
            let _ = tx
                .send(ServerMessage::JobEvent {
                    timestamp: now(),
                    job_id,
                    event: "Failed".to_string(),
                    progress: None,
                    decision: None,
                    error: Some(error),
                    result_path: None,
                    result_panel_id: None,
                })
                .await;
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
            let _ = tx
                .send(ServerMessage::JobEvent {
                    timestamp: now(),
                    job_id,
                    event: "Failed".to_string(),
                    progress: None,
                    decision: None,
                    error: Some(error.to_string()),
                    result_path: None,
                    result_panel_id: None,
                })
                .await;
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
            let final_progress = FileOpProgressSchema {
                total_files,
                completed_files: total_files,
                current_file: None,
                current_file_bytes_total: 0,
                current_file_bytes_copied: 0,
                total_bytes_total: 0,
                total_bytes_copied: 0,
            };
            let _ = tx
                .send(ServerMessage::JobEvent {
                    timestamp: now(),
                    job_id: job_id.clone(),
                    event: "Progress".to_string(),
                    progress: Some(final_progress),
                    decision: None,
                    error: None,
                    result_path: Some(output_path.clone()),
                    result_panel_id: None,
                })
                .await;
            let _ = tx
                .send(ServerMessage::JobEvent {
                    timestamp: now(),
                    job_id,
                    event: "Completed".to_string(),
                    progress: None,
                    decision: None,
                    error: None,
                    result_path: Some(output_path),
                    result_panel_id: None,
                })
                .await;
        }
        Err(error) if error == "Cancelled" => {
            let _ = tx
                .send(ServerMessage::JobEvent {
                    timestamp: now(),
                    job_id,
                    event: "Cancelled".to_string(),
                    progress: None,
                    decision: None,
                    error: None,
                    result_path: None,
                    result_panel_id: None,
                })
                .await;
        }
        Ok(status) => {
            let error = status
                .code()
                .map(|code| format!("{} exited with status {}", bro.option.name, code))
                .unwrap_or_else(|| format!("{} failed", bro.option.name));
            let _ = tx
                .send(ServerMessage::JobEvent {
                    timestamp: now(),
                    job_id,
                    event: "Failed".to_string(),
                    progress: None,
                    decision: None,
                    error: Some(error),
                    result_path: None,
                    result_panel_id: None,
                })
                .await;
        }
        Err(error) => {
            let _ = tx
                .send(ServerMessage::JobEvent {
                    timestamp: now(),
                    job_id,
                    event: "Failed".to_string(),
                    progress: None,
                    decision: None,
                    error: Some(error),
                    result_path: None,
                    result_panel_id: None,
                })
                .await;
        }
    }
}
