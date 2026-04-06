#[path = "../fp_host_protocol.rs"]
mod fp_host_protocol;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;

use fp_host_protocol::{
    FpHostCapabilities, FpHostProgress, FpHostToRunnerMessage, FpRunnerToHostMessage,
    FP_HOST_PROTOCOL_VERSION,
};
use regex::Regex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

#[derive(Clone, Debug)]
enum AdapterKind {
    SevenZip { tool: String },
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        let _ = send_message(&FpHostToRunnerMessage::Failed { error }).await;
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    let adapter = parse_args(std::env::args().skip(1).collect())?;
    send_message(&FpHostToRunnerMessage::Hello {
        protocol_version: FP_HOST_PROTOCOL_VERSION.to_string(),
        capabilities: FpHostCapabilities { progress: true },
    })
    .await?;

    let stdin = tokio::io::stdin();
    let mut stdin_lines = BufReader::new(stdin).lines();
    let start = match stdin_lines.next_line().await {
        Ok(Some(line)) => parse_runner_message(&line)?,
        Ok(None) => return Err("fbro closed the host stdin before start".to_string()),
        Err(error) => return Err(error.to_string()),
    };

    let FpRunnerToHostMessage::Start {
        output_path,
        inputs,
        params,
        working_dir,
        ..
    } = start
    else {
        return Err("expected start message".to_string());
    };

    let (cancel_tx, mut cancel_rx) = mpsc::channel::<()>(1);
    tokio::spawn(async move {
        while let Ok(Some(line)) = stdin_lines.next_line().await {
            match parse_runner_message(&line) {
                Ok(FpRunnerToHostMessage::Cancel) => {
                    let _ = cancel_tx.send(()).await;
                    return;
                }
                Ok(FpRunnerToHostMessage::Start { .. }) => {}
                Err(_) => {}
            }
        }
    });

    match adapter {
        AdapterKind::SevenZip { tool } => {
            run_seven_zip(
                tool,
                output_path,
                inputs.into_iter().map(|input| input.name).collect(),
                params,
                working_dir.map(PathBuf::from),
                &mut cancel_rx,
            )
            .await
        }
    }
}

async fn run_seven_zip(
    tool: String,
    output_path: String,
    input_names: Vec<String>,
    params: HashMap<String, String>,
    working_dir: Option<PathBuf>,
    cancel_rx: &mut mpsc::Receiver<()>,
) -> Result<(), String> {
    let mut command = Command::new(&tool);
    command.arg("a");
    command.arg("-bsp1");
    command.arg(format!(
        "-mx{}",
        params
            .get("compressionLevel")
            .map(String::as_str)
            .unwrap_or("5")
    ));
    if let Some(password) = params.get("password").map(String::as_str).filter(|value| !value.is_empty()) {
        command.arg(format!("-p{password}"));
    }
    command.arg(&output_path);
    command.arg("--");
    command.args(&input_names);
    if let Some(cwd) = working_dir {
        command.current_dir(cwd);
    }
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "7z stdout was unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "7z stderr was unavailable".to_string())?;

    let (line_tx, mut line_rx) = mpsc::channel::<String>(64);
    spawn_line_reader(stdout, line_tx.clone());
    spawn_line_reader(stderr, line_tx);

    let progress_re = Regex::new(r"^\s*(\d{1,3})%\s*(.*)$").map_err(|error| error.to_string())?;
    let mut last_percent: Option<u8> = None;
    let mut last_file: Option<String> = None;
    let mut cancel_sent = false;

    loop {
        tokio::select! {
            maybe_line = line_rx.recv() => {
                let Some(line) = maybe_line else { break; };
                if let Some(progress) = parse_seven_zip_progress(&progress_re, &line, input_names.len(), &mut last_percent, &mut last_file) {
                    send_message(&FpHostToRunnerMessage::Progress(progress)).await?;
                }
            }
            status = child.wait() => {
                let status = status.map_err(|error| error.to_string())?;
                if cancel_sent {
                    return Err("Cancelled".to_string());
                }
                if status.success() {
                    send_message(&FpHostToRunnerMessage::Completed {
                        result_path: Some(output_path),
                    })
                    .await?;
                    return Ok(());
                }
                return Err(
                    status
                        .code()
                        .map(|code| format!("7z exited with status {code}"))
                        .unwrap_or_else(|| "7z failed".to_string()),
                );
            }
            _ = cancel_rx.recv() => {
                cancel_sent = true;
                let _ = child.kill().await;
            }
        }
    }

    Err("7z stopped without a terminal status".to_string())
}

fn spawn_line_reader<T>(stream: T, tx: mpsc::Sender<String>)
where
    T: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if tx.send(line).await.is_err() {
                return;
            }
        }
    });
}

fn parse_seven_zip_progress(
    progress_re: &Regex,
    line: &str,
    total_files: usize,
    last_percent: &mut Option<u8>,
    last_file: &mut Option<String>,
) -> Option<FpHostProgress> {
    let captures = progress_re.captures(line)?;
    let percent = captures.get(1)?.as_str().parse::<u8>().ok()?.min(100);
    let raw_tail = captures.get(2).map(|value| value.as_str()).unwrap_or("").trim();
    let current_file = if raw_tail.is_empty() || raw_tail.starts_with("files,") {
        None
    } else {
        Some(
            raw_tail
                .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch.is_whitespace() || ch == '+' || ch == '-')
                .trim()
                .to_string(),
        )
        .filter(|value| !value.is_empty())
    };

    if *last_percent == Some(percent) && *last_file == current_file {
        return None;
    }
    *last_percent = Some(percent);
    *last_file = current_file.clone();

    Some(FpHostProgress {
        current_file,
        status_text: Some(format!("Compressing {percent}%")),
        completed_files: None,
        total_files: Some(total_files),
        current_bytes: None,
        current_bytes_total: None,
        total_bytes: None,
        total_bytes_total: None,
        current_file_percent: None,
        overall_percent: Some(percent),
    })
}

fn parse_args(args: Vec<String>) -> Result<AdapterKind, String> {
    let mut adapter: Option<String> = None;
    let mut tool: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--adapter" => {
                index += 1;
                adapter = args.get(index).cloned();
            }
            "--tool" => {
                index += 1;
                tool = args.get(index).cloned();
            }
            other => return Err(format!("Unknown argument: {other}")),
        }
        index += 1;
    }

    match adapter.as_deref() {
        Some("seven-zip") => Ok(AdapterKind::SevenZip {
            tool: tool.ok_or_else(|| "Missing --tool for seven-zip adapter".to_string())?,
        }),
        Some(other) => Err(format!("Unknown adapter: {other}")),
        None => Err("Missing --adapter".to_string()),
    }
}

fn parse_runner_message(line: &str) -> Result<FpRunnerToHostMessage, String> {
    serde_json::from_str(line).map_err(|error| format!("Invalid runner message: {error}"))
}

async fn send_message(message: &FpHostToRunnerMessage) -> Result<(), String> {
    let mut stdout = tokio::io::stdout();
    let payload = serde_json::to_vec(message).map_err(|error| error.to_string())?;
    stdout.write_all(&payload).await.map_err(|error| error.to_string())?;
    stdout.write_all(b"\n").await.map_err(|error| error.to_string())?;
    stdout.flush().await.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_seven_zip_adapter_args() {
        let adapter = parse_args(vec![
            "--adapter".to_string(),
            "seven-zip".to_string(),
            "--tool".to_string(),
            "/usr/bin/7z".to_string(),
        ])
        .expect("args should parse");

        match adapter {
            AdapterKind::SevenZip { tool } => assert_eq!(tool, "/usr/bin/7z"),
        }
    }

    #[test]
    fn extracts_percent_from_seven_zip_output() {
        let regex = Regex::new(r"^\s*(\d{1,3})%\s*(.*)$").expect("regex");
        let mut last_percent = None;
        let mut last_file = None;

        let progress = parse_seven_zip_progress(
            &regex,
            " 42% + archive/input/file.txt",
            3,
            &mut last_percent,
            &mut last_file,
        )
        .expect("progress should parse");

        assert_eq!(progress.overall_percent, Some(42));
        assert_eq!(progress.current_file.as_deref(), Some("archive/input/file.txt"));
        assert_eq!(progress.total_files, Some(3));
    }
}
