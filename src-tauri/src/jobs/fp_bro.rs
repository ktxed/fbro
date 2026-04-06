use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
use sevenz_rust2::{ArchiveEntry, ArchiveWriter, Password};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use zip::write::{FullFileOptions, SimpleFileOptions};
use zip::{AesMode, CompressionMethod, ZipWriter};

use crate::bro::LaunchRecipe;
use crate::bro::{BroInputItem, ResolvedBro};
use crate::ws::protocol::{now, FileOpProgressSchema, ServerMessage};

const ARCHIVE_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

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
            run_builtin_seven_zip(job_id, bro, inputs, output_path, params, tx, cancel_rx).await;
        }
        LaunchRecipe::BuiltInZip => {
            run_builtin_zip(job_id, bro, inputs, output_path, params, tx, cancel_rx).await;
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

async fn run_builtin_seven_zip(
    job_id: String,
    bro: ResolvedBro,
    inputs: Vec<BroInputItem>,
    output_path: String,
    params: HashMap<String, String>,
    tx: mpsc::Sender<ServerMessage>,
    mut cancel_rx: watch::Receiver<bool>,
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
        run_builtin_seven_zip_blocking(
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

fn run_builtin_seven_zip_blocking(
    job_id: &str,
    bro_name: &str,
    inputs: &[BroInputItem],
    output_path: &str,
    params: &HashMap<String, String>,
    tx: &mpsc::Sender<ServerMessage>,
    cancel_flag: Arc<AtomicBool>,
) -> Result<ArchiveSummary, ArchiveRunError> {
    let plan = build_archive_plan(inputs).map_err(ArchiveRunError::Failed)?;
    send_progress_sync(
        tx,
        job_id,
        FileOpProgressSchema {
            total_files: plan.total_files,
            completed_files: 0,
            current_file: None,
            status_text: Some(format!("Preparing {bro_name} archive")),
            current_file_bytes_total: 0,
            current_file_bytes_copied: 0,
            total_bytes_total: plan.total_bytes_total,
            total_bytes_copied: 0,
            current_file_percent: None,
            overall_percent: None,
        },
    );

    let mut writer = ArchiveWriter::create(output_path)
        .map_err(|error| ArchiveRunError::Failed(error.to_string()))?;
    writer.set_content_methods(seven_zip_methods(params));

    let completed_files = Arc::new(AtomicUsize::new(0));
    let total_bytes_copied = Arc::new(AtomicU64::new(0));

    for item in &plan.items {
        if cancel_flag.load(Ordering::SeqCst) {
            return Err(ArchiveRunError::Cancelled);
        }

        match item {
            ArchiveItem::Directory {
                source_path,
                archive_name,
            } => {
                writer
                    .push_archive_entry::<io::Empty>(
                        ArchiveEntry::from_path(source_path, archive_name.clone()),
                        None,
                    )
                    .map_err(|error| ArchiveRunError::Failed(error.to_string()))?;
            }
            ArchiveItem::File {
                source_path,
                archive_name,
                size,
            } => {
                let file = File::open(source_path)
                    .map_err(|error| ArchiveRunError::Failed(error.to_string()))?;
                let reader = ProgressReader::new(
                    file,
                    ProgressReporter::new(
                        tx.clone(),
                        job_id.to_string(),
                        plan.total_files,
                        plan.total_bytes_total,
                        Arc::clone(&completed_files),
                        Arc::clone(&total_bytes_copied),
                        Arc::clone(&cancel_flag),
                    ),
                    archive_name.clone(),
                    *size,
                );
                writer
                    .push_archive_entry(
                        ArchiveEntry::from_path(source_path, archive_name.clone()),
                        Some(reader),
                    )
                    .map_err(|error| map_archive_error(error, &cancel_flag))?;
            }
        }
    }

    if cancel_flag.load(Ordering::SeqCst) {
        return Err(ArchiveRunError::Cancelled);
    }

    writer
        .finish()
        .map_err(|error| map_archive_error(error, &cancel_flag))?;

    Ok(ArchiveSummary {
        total_files: plan.total_files,
        total_bytes_total: plan.total_bytes_total,
    })
}

async fn run_builtin_zip(
    job_id: String,
    bro: ResolvedBro,
    inputs: Vec<BroInputItem>,
    output_path: String,
    params: HashMap<String, String>,
    tx: mpsc::Sender<ServerMessage>,
    mut cancel_rx: watch::Receiver<bool>,
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
        run_builtin_zip_blocking(
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

fn run_builtin_zip_blocking(
    job_id: &str,
    bro_name: &str,
    inputs: &[BroInputItem],
    output_path: &str,
    params: &HashMap<String, String>,
    tx: &mpsc::Sender<ServerMessage>,
    cancel_flag: Arc<AtomicBool>,
) -> Result<ArchiveSummary, ArchiveRunError> {
    let plan = build_archive_plan(inputs).map_err(ArchiveRunError::Failed)?;
    send_progress_sync(
        tx,
        job_id,
        FileOpProgressSchema {
            total_files: plan.total_files,
            completed_files: 0,
            current_file: None,
            status_text: Some(format!("Preparing {bro_name} archive")),
            current_file_bytes_total: 0,
            current_file_bytes_copied: 0,
            total_bytes_total: plan.total_bytes_total,
            total_bytes_copied: 0,
            current_file_percent: None,
            overall_percent: None,
        },
    );

    let output =
        File::create(output_path).map_err(|error| ArchiveRunError::Failed(error.to_string()))?;
    let mut writer = ZipWriter::new(output);
    let completed_files = Arc::new(AtomicUsize::new(0));
    let total_bytes_copied = Arc::new(AtomicU64::new(0));
    let password = params
        .get("password")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty());

    for item in &plan.items {
        if cancel_flag.load(Ordering::SeqCst) {
            return Err(ArchiveRunError::Cancelled);
        }

        match item {
            ArchiveItem::Directory { archive_name, .. } => {
                writer
                    .add_directory(archive_name.as_str(), SimpleFileOptions::default())
                    .map_err(|error| ArchiveRunError::Failed(error.to_string()))?;
            }
            ArchiveItem::File {
                source_path,
                archive_name,
                size,
            } => {
                let file = File::open(source_path)
                    .map_err(|error| ArchiveRunError::Failed(error.to_string()))?;
                writer
                    .start_file(archive_name, zip_file_options(password, *size, params))
                    .map_err(|error| map_archive_error(error, &cancel_flag))?;
                let mut reader = ProgressReader::new(
                    file,
                    ProgressReporter::new(
                        tx.clone(),
                        job_id.to_string(),
                        plan.total_files,
                        plan.total_bytes_total,
                        Arc::clone(&completed_files),
                        Arc::clone(&total_bytes_copied),
                        Arc::clone(&cancel_flag),
                    ),
                    archive_name.clone(),
                    *size,
                );
                io::copy(&mut reader, &mut writer)
                    .map_err(|error| map_archive_error(error, &cancel_flag))?;
                writer
                    .flush()
                    .map_err(|error| map_archive_error(error, &cancel_flag))?;
            }
        }
    }

    if cancel_flag.load(Ordering::SeqCst) {
        return Err(ArchiveRunError::Cancelled);
    }

    writer
        .finish()
        .map_err(|error| map_archive_error(error, &cancel_flag))?;

    Ok(ArchiveSummary {
        total_files: plan.total_files,
        total_bytes_total: plan.total_bytes_total,
    })
}

fn map_archive_error(error: impl ToString, cancel_flag: &AtomicBool) -> ArchiveRunError {
    if cancel_flag.load(Ordering::SeqCst) {
        ArchiveRunError::Cancelled
    } else {
        ArchiveRunError::Failed(error.to_string())
    }
}

fn seven_zip_methods(params: &HashMap<String, String>) -> Vec<sevenz_rust2::EncoderConfiguration> {
    let level = params
        .get("compressionLevel")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(5);
    let password = params
        .get("password")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(Password::from)
        .unwrap_or_else(Password::empty);

    let mut methods = Vec::new();
    if !password.is_empty() {
        methods.push(AesEncoderOptions::new(password).into());
    }
    methods.push(Lzma2Options::from_level(level).into());
    methods
}

fn zip_file_options<'a>(
    password: Option<&'a str>,
    size: u64,
    params: &HashMap<String, String>,
) -> FullFileOptions<'a> {
    let level = params
        .get("compressionLevel")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(6);
    let options = FullFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(level))
        .large_file(size > u32::MAX as u64);
    match password {
        Some(password) => options.with_aes_encryption(AesMode::Aes256, password),
        None => options,
    }
}

fn build_archive_plan(inputs: &[BroInputItem]) -> Result<ArchivePlan, String> {
    let root = first_input_parent(inputs)
        .ok_or_else(|| "No input entries were resolved for this bro".to_string())?;
    let mut items = Vec::new();
    let mut total_files = 0usize;
    let mut total_bytes_total = 0u64;

    for input in inputs {
        let source_path = PathBuf::from(&input.path);
        collect_plan_item(
            &source_path,
            Path::new(&root),
            &mut items,
            &mut total_files,
            &mut total_bytes_total,
        )?;
    }

    Ok(ArchivePlan {
        items,
        total_files,
        total_bytes_total,
    })
}

fn collect_plan_item(
    source_path: &Path,
    root: &Path,
    items: &mut Vec<ArchiveItem>,
    total_files: &mut usize,
    total_bytes_total: &mut u64,
) -> Result<(), String> {
    let metadata = source_path
        .metadata()
        .map_err(|error| format!("Failed to stat {}: {error}", source_path.display()))?;
    let archive_name = archive_entry_name(root, source_path)?;

    if metadata.is_dir() {
        items.push(ArchiveItem::Directory {
            source_path: source_path.to_path_buf(),
            archive_name,
        });

        let mut children = source_path
            .read_dir()
            .map_err(|error| format!("Failed to read {}: {error}", source_path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read {}: {error}", source_path.display()))?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            collect_plan_item(&child.path(), root, items, total_files, total_bytes_total)?;
        }
    } else if metadata.is_file() {
        *total_files += 1;
        *total_bytes_total += metadata.len();
        items.push(ArchiveItem::File {
            source_path: source_path.to_path_buf(),
            archive_name,
            size: metadata.len(),
        });
    }

    Ok(())
}

fn archive_entry_name(root: &Path, source_path: &Path) -> Result<String, String> {
    let relative = source_path
        .strip_prefix(root)
        .map_err(|error| format!("Failed to derive archive name: {error}"))?;
    let mut name = relative.to_string_lossy().replace('\\', "/");
    if name.is_empty() {
        name = source_path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
    }
    Ok(name)
}

fn first_input_parent(inputs: &[BroInputItem]) -> Option<String> {
    let first = inputs.first()?;
    Path::new(&first.path)
        .parent()
        .map(|path| path.to_string_lossy().to_string())
}

struct ProgressReader<R> {
    inner: R,
    reporter: ProgressReporter,
    current_file: String,
    current_file_total: u64,
    current_file_copied: u64,
    last_emit: Instant,
}

impl<R: Read> ProgressReader<R> {
    fn new(
        inner: R,
        reporter: ProgressReporter,
        current_file: String,
        current_file_total: u64,
    ) -> Self {
        let now = Instant::now();
        reporter.send_snapshot(
            Some(current_file.clone()),
            Some(format!("Compressing {}", current_file)),
            current_file_total,
            0,
            false,
        );
        Self {
            inner,
            reporter,
            current_file,
            current_file_total,
            current_file_copied: 0,
            last_emit: now,
        }
    }
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.reporter.is_cancelled() {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Cancelled"));
        }

        let n = self.inner.read(buf)?;
        if n == 0 {
            self.reporter.file_completed(
                self.current_file.clone(),
                self.current_file_total,
                self.current_file_copied,
            );
            return Ok(0);
        }

        let n = n as u64;
        self.current_file_copied += n;
        self.reporter
            .total_bytes_copied
            .fetch_add(n, Ordering::SeqCst);

        if self.last_emit.elapsed() >= ARCHIVE_PROGRESS_INTERVAL
            || self.current_file_copied >= self.current_file_total
        {
            self.reporter.send_snapshot(
                Some(self.current_file.clone()),
                Some(format!("Compressing {}", self.current_file)),
                self.current_file_total,
                self.current_file_copied,
                false,
            );
            self.last_emit = Instant::now();
        }

        Ok(n as usize)
    }
}

#[derive(Clone)]
struct ProgressReporter {
    tx: mpsc::Sender<ServerMessage>,
    job_id: String,
    total_files: usize,
    total_bytes_total: u64,
    completed_files: Arc<AtomicUsize>,
    total_bytes_copied: Arc<AtomicU64>,
    cancel_flag: Arc<AtomicBool>,
}

impl ProgressReporter {
    fn new(
        tx: mpsc::Sender<ServerMessage>,
        job_id: String,
        total_files: usize,
        total_bytes_total: u64,
        completed_files: Arc<AtomicUsize>,
        total_bytes_copied: Arc<AtomicU64>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            tx,
            job_id,
            total_files,
            total_bytes_total,
            completed_files,
            total_bytes_copied,
            cancel_flag,
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }

    fn send_snapshot(
        &self,
        current_file: Option<String>,
        status_text: Option<String>,
        current_file_total: u64,
        current_file_copied: u64,
        completed_file: bool,
    ) {
        let completed_files = if completed_file {
            self.completed_files.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            self.completed_files.load(Ordering::SeqCst)
        };
        let total_bytes_copied = self.total_bytes_copied.load(Ordering::SeqCst);

        send_progress_sync(
            &self.tx,
            &self.job_id,
            FileOpProgressSchema {
                total_files: self.total_files,
                completed_files,
                current_file,
                status_text,
                current_file_bytes_total: current_file_total,
                current_file_bytes_copied: current_file_copied,
                total_bytes_total: self.total_bytes_total,
                total_bytes_copied,
                current_file_percent: percent(current_file_copied, current_file_total),
                overall_percent: percent(total_bytes_copied, self.total_bytes_total),
            },
        );
    }

    fn file_completed(
        &self,
        current_file: String,
        current_file_total: u64,
        current_file_copied: u64,
    ) {
        self.send_snapshot(
            Some(current_file),
            Some("File compressed".to_string()),
            current_file_total,
            current_file_copied,
            true,
        );
    }
}

fn percent(numerator: u64, denominator: u64) -> Option<u8> {
    if denominator == 0 {
        return None;
    }
    let pct = ((numerator.saturating_mul(100)) / denominator).min(100);
    u8::try_from(pct).ok()
}

fn send_progress_sync(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    progress: FileOpProgressSchema,
) {
    let _ = tx.try_send(ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: "Progress".to_string(),
        progress: Some(progress),
        decision: None,
        error: None,
        result_path: None,
        result_panel_id: None,
    });
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

struct ArchivePlan {
    items: Vec<ArchiveItem>,
    total_files: usize,
    total_bytes_total: u64,
}

enum ArchiveItem {
    Directory {
        source_path: PathBuf,
        archive_name: String,
    },
    File {
        source_path: PathBuf,
        archive_name: String,
        size: u64,
    },
}

struct ArchiveSummary {
    total_files: usize,
    total_bytes_total: u64,
}

enum ArchiveRunError {
    Cancelled,
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_clamps_to_hundred() {
        assert_eq!(percent(15, 10), Some(100));
        assert_eq!(percent(5, 10), Some(50));
        assert_eq!(percent(0, 0), None);
    }

    #[test]
    fn build_methods_keeps_password_optional() {
        let mut params = HashMap::new();
        params.insert("compressionLevel".to_string(), "9".to_string());
        params.insert("password".to_string(), "secret".to_string());

        let methods = seven_zip_methods(&params);
        assert_eq!(methods.len(), 2);
    }
}
