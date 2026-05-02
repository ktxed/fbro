use std::collections::HashMap;
use std::io;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
use sevenz_rust2::{ArchiveEntry, ArchiveWriter, Password};
use tokio::sync::mpsc;

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
                ..
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
                ..
            } => {
                let reader = open_progress_reader(
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::seven_zip_methods;

    #[test]
    fn build_methods_keeps_password_optional() {
        let mut params = HashMap::new();
        params.insert("compressionLevel".to_string(), "9".to_string());
        params.insert("password".to_string(), "secret".to_string());

        let methods = seven_zip_methods(&params);
        assert_eq!(methods.len(), 2);
    }
}
