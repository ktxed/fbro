use std::collections::HashMap;
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tokio::time::Duration;

use crate::bro::{BroInputItem, LaunchCwd, LaunchRecipe, ResolvedBro};
use crate::fp_host_protocol::{
    FpHostInput, FpHostProgress, FpHostToRunnerMessage, FpRunnerToHostMessage,
    FP_HOST_PROTOCOL_VERSION,
};
use crate::ws::protocol::{now, FileOpProgressSchema, ServerMessage};

const FP_HOST_HELLO_TIMEOUT: Duration = Duration::from_secs(3);
const FP_HOST_CANCEL_GRACE: Duration = Duration::from_secs(2);

pub async fn run_fp_bro(
    job_id: String,
    bro: ResolvedBro,
    inputs: Vec<BroInputItem>,
    output_path: String,
    params: HashMap<String, String>,
    tx: mpsc::Sender<ServerMessage>,
    cancel_rx: watch::Receiver<bool>,
) {
    let total_files = inputs.len();
    let _ = send_progress(
        &tx,
        &job_id,
        FileOpProgressSchema {
            total_files,
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
        LaunchRecipe::HostedStdio { program, args } => {
            run_hosted_fp_bro(
                job_id,
                bro,
                inputs,
                output_path,
                params,
                program,
                args,
                tx,
                cancel_rx,
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

async fn run_hosted_fp_bro(
    job_id: String,
    bro: ResolvedBro,
    inputs: Vec<BroInputItem>,
    output_path: String,
    params: HashMap<String, String>,
    host_program: String,
    host_args: Vec<String>,
    tx: mpsc::Sender<ServerMessage>,
    mut cancel_rx: watch::Receiver<bool>,
) {
    let total_files = inputs.len();
    let working_dir = working_dir_for_inputs(&bro, &inputs);

    let mut host = Command::new(&host_program);
    host.args(&host_args);
    host.stdin(std::process::Stdio::piped());
    host.stdout(std::process::Stdio::piped());
    host.stderr(std::process::Stdio::piped());

    let mut child = match host.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = send_terminal(
                &tx,
                &job_id,
                "Failed",
                Some(format!("Failed to start fp host: {error}")),
                None,
            )
            .await;
            return;
        }
    };

    let Some(mut stdin) = child.stdin.take() else {
        let _ = send_terminal(&tx, &job_id, "Failed", Some("fp host stdin was unavailable".to_string()), None).await;
        let _ = child.kill().await;
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = send_terminal(&tx, &job_id, "Failed", Some("fp host stdout was unavailable".to_string()), None).await;
        let _ = child.kill().await;
        return;
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = send_terminal(&tx, &job_id, "Failed", Some("fp host stderr was unavailable".to_string()), None).await;
        let _ = child.kill().await;
        return;
    };

    let mut stdout_lines = BufReader::new(stdout).lines();
    let hello = match tokio::time::timeout(FP_HOST_HELLO_TIMEOUT, stdout_lines.next_line()).await {
        Ok(Ok(Some(line))) => match parse_host_message(&line) {
            Ok(FpHostToRunnerMessage::Hello {
                protocol_version,
                capabilities,
            }) => {
                if protocol_version != FP_HOST_PROTOCOL_VERSION {
                    let _ = send_terminal(
                        &tx,
                        &job_id,
                        "Failed",
                        Some(format!("fp host uses unsupported protocol version {protocol_version}")),
                        None,
                    )
                    .await;
                    let _ = child.kill().await;
                    return;
                }
                Some(capabilities)
            }
            Ok(_) => {
                let _ = send_terminal(
                    &tx,
                    &job_id,
                    "Failed",
                    Some("fp host did not send an initial hello frame".to_string()),
                    None,
                )
                .await;
                let _ = child.kill().await;
                return;
            }
            Err(error) => {
                let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None).await;
                let _ = child.kill().await;
                return;
            }
        },
        Ok(Ok(None)) => {
            let _ = send_terminal(
                &tx,
                &job_id,
                "Failed",
                Some("fp host exited before handshake".to_string()),
                None,
            )
            .await;
            return;
        }
        Ok(Err(error)) => {
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error.to_string()), None).await;
            let _ = child.kill().await;
            return;
        }
        Err(_) => {
            let _ = send_terminal(
                &tx,
                &job_id,
                "Failed",
                Some("Timed out waiting for fp host handshake".to_string()),
                None,
            )
            .await;
            let _ = child.kill().await;
            return;
        }
    };

    if !hello.unwrap_or_default().progress {
        let _ = send_terminal(
            &tx,
            &job_id,
            "Failed",
            Some("fp host does not advertise progress support".to_string()),
            None,
        )
        .await;
        let _ = child.kill().await;
        return;
    }

    let start_message = FpRunnerToHostMessage::Start {
        job_id: job_id.clone(),
        bro_id: bro.option.id.clone(),
        bro_name: bro.option.name.clone(),
        inputs: inputs
            .iter()
            .map(|input| FpHostInput {
                path: input.path.clone(),
                name: input.name.clone(),
                kind: input.kind.clone(),
            })
            .collect(),
        output_path: output_path.clone(),
        params,
        working_dir,
    };

    if let Err(error) = write_host_message(&mut stdin, &start_message).await {
        let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None).await;
        let _ = child.kill().await;
        return;
    }

    tokio::spawn(async move {
        let mut stderr_lines = BufReader::new(stderr).lines();
        while let Ok(Some(_)) = stderr_lines.next_line().await {}
    });

    let (msg_tx, mut msg_rx) = mpsc::channel::<Result<FpHostToRunnerMessage, String>>(32);
    tokio::spawn(async move {
        while let Ok(Some(line)) = stdout_lines.next_line().await {
            if msg_tx.send(parse_host_message(&line)).await.is_err() {
                return;
            }
        }
    });

    let mut cancel_sent = false;
    let mut terminal: Option<Result<Option<String>, String>> = None;
    let mut host_status: Option<Result<std::process::ExitStatus, std::io::Error>> = None;

    loop {
        tokio::select! {
            maybe_msg = msg_rx.recv() => {
                match maybe_msg {
                    Some(Ok(FpHostToRunnerMessage::Progress(progress))) => {
                        if let Some(progress) = normalize_progress(&progress, total_files) {
                            let _ = send_progress(&tx, &job_id, progress).await;
                        }
                    }
                    Some(Ok(FpHostToRunnerMessage::Completed { result_path })) => {
                        terminal = Some(Ok(result_path.or_else(|| Some(output_path.clone()))));
                        break;
                    }
                    Some(Ok(FpHostToRunnerMessage::Failed { error })) => {
                        terminal = Some(Err(error));
                        break;
                    }
                    Some(Ok(FpHostToRunnerMessage::Log { .. })) => {}
                    Some(Ok(FpHostToRunnerMessage::Hello { .. })) => {
                        terminal = Some(Err("fp host sent a duplicate hello frame".to_string()));
                        break;
                    }
                    Some(Err(error)) => {
                        terminal = Some(Err(error));
                        break;
                    }
                    None => {}
                }
            }
            status = child.wait() => {
                host_status = Some(status);
                break;
            }
            changed = cancel_rx.changed(), if !cancel_sent => {
                match changed {
                    Ok(_) if *cancel_rx.borrow() => {
                        cancel_sent = true;
                        let _ = write_host_message(&mut stdin, &FpRunnerToHostMessage::Cancel).await;
                        tokio::time::sleep(FP_HOST_CANCEL_GRACE).await;
                        let _ = child.kill().await;
                    }
                    Ok(_) => {}
                    Err(_) => {}
                }
            }
        }
    }

    if cancel_sent {
        let _ = send_terminal(&tx, &job_id, "Cancelled", None, None).await;
        return;
    }

    match terminal {
        Some(Ok(result_path)) => {
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
            let _ = send_terminal(
                &tx,
                &job_id,
                "Completed",
                None,
                result_path.or(Some(output_path)),
            )
            .await;
        }
        Some(Err(error)) => {
            let _ = child.kill().await;
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None).await;
        }
        None => {
            match host_status {
                Some(Ok(status)) if status.success() => {
                    let _ = send_terminal(
                        &tx,
                        &job_id,
                        "Failed",
                        Some("fp host exited without a terminal completion frame".to_string()),
                        None,
                    )
                    .await;
                }
                Some(Ok(status)) => {
                    let error = status
                        .code()
                        .map(|code| format!("fp host exited with status {}", code))
                        .unwrap_or_else(|| "fp host failed".to_string());
                    let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None).await;
                }
                Some(Err(error)) => {
                    let _ = send_terminal(&tx, &job_id, "Failed", Some(error.to_string()), None).await;
                }
                None => {
                    let _ = send_terminal(
                        &tx,
                        &job_id,
                        "Failed",
                        Some("fp host stopped unexpectedly".to_string()),
                        None,
                    )
                    .await;
                }
            }
        }
    }
}

fn normalize_progress(progress: &FpHostProgress, fallback_total_files: usize) -> Option<FileOpProgressSchema> {
    let total_files = progress.total_files.unwrap_or(fallback_total_files);
    let completed_files = progress.completed_files.unwrap_or(0).min(total_files);
    let current_file_percent = progress.current_file_percent.map(clamp_percent);
    let overall_percent = progress.overall_percent.map(clamp_percent);

    if total_files == 0
        && completed_files == 0
        && progress.current_file.is_none()
        && progress.status_text.is_none()
        && progress.current_bytes.is_none()
        && progress.current_bytes_total.is_none()
        && progress.total_bytes.is_none()
        && progress.total_bytes_total.is_none()
        && current_file_percent.is_none()
        && overall_percent.is_none()
    {
        return None;
    }

    Some(FileOpProgressSchema {
        total_files,
        completed_files,
        current_file: progress.current_file.clone(),
        status_text: progress.status_text.clone(),
        current_file_bytes_total: progress.current_bytes_total.unwrap_or(0),
        current_file_bytes_copied: progress.current_bytes.unwrap_or(0),
        total_bytes_total: progress.total_bytes_total.unwrap_or(0),
        total_bytes_copied: progress.total_bytes.unwrap_or(0),
        current_file_percent,
        overall_percent,
    })
}

fn clamp_percent(value: u8) -> u8 {
    value.min(100)
}

fn working_dir_for_inputs(bro: &ResolvedBro, inputs: &[BroInputItem]) -> Option<String> {
    match &bro.launch {
        LaunchRecipe::Command { cwd: Some(LaunchCwd::FirstInputParent), .. } => inputs
            .first()
            .and_then(|input| Path::new(&input.path).parent())
            .map(|path| path.to_string_lossy().to_string()),
        LaunchRecipe::HostedStdio { .. } => inputs
            .first()
            .and_then(|input| Path::new(&input.path).parent())
            .map(|path| path.to_string_lossy().to_string()),
        _ => None,
    }
}

fn parse_host_message(line: &str) -> Result<FpHostToRunnerMessage, String> {
    serde_json::from_str(line).map_err(|error| format!("Invalid fp host message: {error}"))
}

async fn write_host_message(
    stdin: &mut tokio::process::ChildStdin,
    message: &FpRunnerToHostMessage,
) -> Result<(), String> {
    let payload = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    stdin.write_all(&payload).await.map_err(|error| error.to_string())?;
    stdin.write_all(b"\n").await.map_err(|error| error.to_string())
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
        decision: None,
        error,
        result_path,
        result_panel_id: None,
    })
    .await
    .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_progress_keeps_explicit_percents() {
        let progress = normalize_progress(
            &FpHostProgress {
                current_file: Some("movie.mkv".to_string()),
                status_text: Some("Compressing".to_string()),
                completed_files: None,
                total_files: Some(4),
                current_bytes: None,
                current_bytes_total: None,
                total_bytes: None,
                total_bytes_total: None,
                current_file_percent: Some(12),
                overall_percent: Some(54),
            },
            4,
        )
        .expect("progress should normalize");

        assert_eq!(progress.total_files, 4);
        assert_eq!(progress.completed_files, 0);
        assert_eq!(progress.current_file.as_deref(), Some("movie.mkv"));
        assert_eq!(progress.current_file_percent, Some(12));
        assert_eq!(progress.overall_percent, Some(54));
    }

    #[test]
    fn normalize_progress_rejects_empty_payload() {
        let progress = normalize_progress(&FpHostProgress::default(), 0);
        assert!(progress.is_none());
    }
}
