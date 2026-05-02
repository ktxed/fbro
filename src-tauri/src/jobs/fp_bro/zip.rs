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
            ArchiveItem::Directory { archive_name, mtime, .. } => {
                let mut opts = SimpleFileOptions::default();
                if let Some(secs) = mtime {
                    if let Some(dt) = epoch_to_zip_datetime(*secs) {
                        opts = opts.last_modified_time(dt);
                    }
                }
                writer
                    .add_directory(archive_name.as_str(), opts)
                    .map_err(|error| ArchiveRunError::Failed(error.to_string()))?;
            }
            ArchiveItem::File {
                source_path,
                archive_name,
                size,
                mtime,
                #[cfg(unix)]
                unix_mode,
            } => {
                let mut opts = zip_file_options(password, *size, params);
                if let Some(secs) = mtime {
                    if let Some(dt) = epoch_to_zip_datetime(*secs) {
                        opts = opts.last_modified_time(dt);
                    }
                    opts = add_unix_timestamp(opts, *secs);
                }
                #[cfg(unix)]
                if let Some(mode) = unix_mode {
                    opts = opts.unix_permissions(*mode);
                }
                writer
                    .start_file(archive_name, opts)
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

/// Convert Unix epoch seconds to a zip::DateTime (DOS time, UTC).
fn epoch_to_zip_datetime(secs: u64) -> Option<zip::DateTime> {
    use std::time::{Duration, UNIX_EPOCH};
    let t = UNIX_EPOCH + Duration::from_secs(secs);
    // Use chrono-free manual conversion: days since epoch → year/month/day.
    let mut remaining = secs as i64;
    let mut year = 1970i32;
    loop {
        let days = if is_leap(year) { 366 } else { 365 };
        if remaining < days * 86400 {
            break;
        }
        remaining -= days * 86400;
        year += 1;
    }
    let month_days = [
        31u8,
        if is_leap(year) { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    let mut month = 1u8;
    for &d in &month_days {
        if remaining < d as i64 * 86400 {
            break;
        }
        remaining -= d as i64 * 86400;
        month += 1;
    }
    let day = (remaining / 86400 + 1) as u8;
    remaining %= 86400;
    let hour = (remaining / 3600) as u8;
    remaining %= 3600;
    let minute = (remaining / 60) as u8;
    let second = (remaining % 60) as u8;
    let _ = t; // suppress unused warning
    zip::DateTime::from_date_and_time(year as u16, month, day, hour, minute, second).ok()
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Append a Unix extended timestamp (0x5455) extra field with just the mtime.
fn add_unix_timestamp(mut opts: FullFileOptions<'_>, secs: u64) -> FullFileOptions<'_> {
    let mtime = secs.min(u32::MAX as u64) as u32;
    let payload: [u8; 5] = [
        0x01, // flags: modification time present
        (mtime & 0xff) as u8,
        ((mtime >> 8) & 0xff) as u8,
        ((mtime >> 16) & 0xff) as u8,
        ((mtime >> 24) & 0xff) as u8,
    ];
    let _ = opts.add_extra_data(0x5455, &payload, false);
    opts
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
