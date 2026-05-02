use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use std::{fs::File, io, io::Read};

use tokio::sync::mpsc;

use crate::bro::BroInputItem;
use crate::ws::protocol::{now, FileOpProgressSchema, ServerMessage};

const ARCHIVE_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

pub(crate) struct ArchivePlan {
    pub(crate) items: Vec<ArchiveItem>,
    pub(crate) total_files: usize,
    pub(crate) total_bytes_total: u64,
}

pub(crate) enum ArchiveItem {
    Directory {
        source_path: PathBuf,
        archive_name: String,
        mtime: Option<u64>,
    },
    File {
        source_path: PathBuf,
        archive_name: String,
        size: u64,
        mtime: Option<u64>,
        #[cfg(unix)]
        unix_mode: Option<u32>,
    },
}

pub(crate) struct ArchiveSummary {
    pub(crate) total_files: usize,
    pub(crate) total_bytes_total: u64,
}

pub(crate) enum ArchiveRunError {
    Cancelled,
    Failed(String),
}

#[derive(Clone)]
pub(crate) struct ProgressReporter {
    tx: mpsc::Sender<ServerMessage>,
    job_id: String,
    total_files: usize,
    total_bytes_total: u64,
    completed_files: Arc<AtomicUsize>,
    pub(crate) total_bytes_copied: Arc<AtomicU64>,
    cancel_flag: Arc<AtomicBool>,
}

impl ProgressReporter {
    pub(crate) fn new(
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

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }

    pub(crate) fn send_snapshot(
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

    pub(crate) fn file_completed(
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

pub(crate) struct ProgressReader<R> {
    inner: R,
    reporter: ProgressReporter,
    current_file: String,
    current_file_total: u64,
    current_file_copied: u64,
    last_emit: Instant,
}

impl<R: Read> ProgressReader<R> {
    pub(crate) fn new(
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

pub(crate) fn build_archive_plan(inputs: &[BroInputItem]) -> Result<ArchivePlan, String> {
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

pub(crate) fn emit_preparing_progress(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    bro_name: &str,
    plan: &ArchivePlan,
) {
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
}

pub(crate) fn map_archive_error(error: impl ToString, cancel_flag: &AtomicBool) -> ArchiveRunError {
    if cancel_flag.load(Ordering::SeqCst) {
        ArchiveRunError::Cancelled
    } else {
        ArchiveRunError::Failed(error.to_string())
    }
}

pub(crate) fn open_progress_reader(
    source_path: &Path,
    archive_name: &str,
    size: u64,
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    plan: &ArchivePlan,
    completed_files: &Arc<AtomicUsize>,
    total_bytes_copied: &Arc<AtomicU64>,
    cancel_flag: &Arc<AtomicBool>,
) -> Result<ProgressReader<File>, ArchiveRunError> {
    let file =
        File::open(source_path).map_err(|error| ArchiveRunError::Failed(error.to_string()))?;
    Ok(ProgressReader::new(
        file,
        ProgressReporter::new(
            tx.clone(),
            job_id.to_string(),
            plan.total_files,
            plan.total_bytes_total,
            Arc::clone(completed_files),
            Arc::clone(total_bytes_copied),
            Arc::clone(cancel_flag),
        ),
        archive_name.to_string(),
        size,
    ))
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

    let mtime = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    if metadata.is_dir() {
        items.push(ArchiveItem::Directory {
            source_path: source_path.to_path_buf(),
            archive_name,
            mtime,
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
        #[cfg(unix)]
        let unix_mode = {
            use std::os::unix::fs::PermissionsExt;
            Some(metadata.permissions().mode())
        };
        *total_files += 1;
        *total_bytes_total += metadata.len();
        items.push(ArchiveItem::File {
            source_path: source_path.to_path_buf(),
            archive_name,
            size: metadata.len(),
            mtime,
            #[cfg(unix)]
            unix_mode,
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
        search_progress: None,
        decision: None,
        error: None,
        result_path: None,
        result_panel_id: None,
    });
}

#[cfg(test)]
mod tests {
    use super::percent;

    #[test]
    fn percent_clamps_to_hundred() {
        assert_eq!(percent(15, 10), Some(100));
        assert_eq!(percent(5, 10), Some(50));
        assert_eq!(percent(0, 0), None);
    }
}
