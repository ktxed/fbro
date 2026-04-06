use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::mpsc;
use zip::write::{FullFileOptions, SimpleFileOptions};
use zip::{AesMode, CompressionMethod, ZipWriter};

use crate::bro::BroInputItem;
use crate::ws::protocol::ServerMessage;

use super::archive::{
    build_archive_plan, emit_preparing_progress, map_archive_error, open_progress_reader,
    ArchiveItem, ArchiveRunError, ArchiveSummary,
};

pub(super) fn run_blocking(
    job_id: &str,
    bro_name: &str,
    inputs: &[BroInputItem],
    output_path: &str,
    params: &HashMap<String, String>,
    tx: &mpsc::Sender<ServerMessage>,
    cancel_flag: Arc<AtomicBool>,
) -> Result<ArchiveSummary, ArchiveRunError> {
    let plan = build_archive_plan(inputs).map_err(ArchiveRunError::Failed)?;
    emit_preparing_progress(tx, job_id, bro_name, &plan);

    let output = std::fs::File::create(output_path)
        .map_err(|error| ArchiveRunError::Failed(error.to_string()))?;
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
                writer
                    .start_file(archive_name, zip_file_options(password, *size, params))
                    .map_err(|error| map_archive_error(error, &cancel_flag))?;
                let mut reader = open_progress_reader(
                    source_path,
                    archive_name,
                    *size,
                    tx,
                    job_id,
                    &plan,
                    &completed_files,
                    &total_bytes_copied,
                    &cancel_flag,
                )?;
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
