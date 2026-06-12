use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::mpsc;

use crate::bro::BroInputItem;
use crate::jobs::progress::send_progress_sync;
pub(crate) use crate::jobs::progress::{ProgressReader, ProgressReporter};
use crate::ws::protocol::{FileOpProgressSchema, ServerMessage};

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

#[derive(Debug)]
pub(crate) enum ArchiveRunError {
    Cancelled,
    Failed(String),
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

#[allow(clippy::too_many_arguments)]
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
        "Compressing",
    ))
}

/// Emit a "Preparing …" snapshot for extraction, where totals come from a pre-scan of the
/// archive entries (extraction cannot use `build_archive_plan`, which walks the filesystem).
pub(crate) fn emit_preparing_extract(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    bro_name: &str,
    total_files: usize,
    total_bytes_total: u64,
) {
    send_progress_sync(
        tx,
        job_id,
        FileOpProgressSchema {
            total_files,
            completed_files: 0,
            current_file: None,
            status_text: Some(format!("Preparing {bro_name} extraction")),
            current_file_bytes_total: 0,
            current_file_bytes_copied: 0,
            total_bytes_total,
            total_bytes_copied: 0,
            current_file_percent: None,
            overall_percent: None,
        },
    );
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
