use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::mpsc;
use zip::ZipArchive;

use crate::bro::BroInputItem;
use crate::ws::protocol::ServerMessage;

use super::archive::{
    emit_preparing_extract, map_archive_error, ArchiveRunError, ArchiveSummary, ProgressReader,
    ProgressReporter,
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
    let archive_path = inputs
        .first()
        .map(|input| input.path.clone())
        .ok_or_else(|| ArchiveRunError::Failed("No archive was resolved for this bro".to_string()))?;

    let dest = Path::new(output_path);
    std::fs::create_dir_all(dest).map_err(|error| {
        ArchiveRunError::Failed(format!("Failed to create {}: {error}", dest.display()))
    })?;

    let password = params
        .get("password")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string());

    let file =
        File::open(&archive_path).map_err(|error| ArchiveRunError::Failed(error.to_string()))?;
    let mut zip = ZipArchive::new(file)
        .map_err(|error| ArchiveRunError::Failed(format!("Failed to open archive: {error}")))?;

    // Pre-scan for progress totals (uncompressed sizes) and file count.
    let mut total_files = 0usize;
    let mut total_bytes_total = 0u64;
    for index in 0..zip.len() {
        let entry = open_entry(&mut zip, index, password.as_deref())?;
        if !entry.is_dir() {
            total_files += 1;
            total_bytes_total += entry.size();
        }
    }

    emit_preparing_extract(tx, job_id, bro_name, total_files, total_bytes_total);

    let completed_files = Arc::new(AtomicUsize::new(0));
    let total_bytes_copied = Arc::new(AtomicU64::new(0));

    for index in 0..zip.len() {
        if cancel_flag.load(Ordering::SeqCst) {
            return Err(ArchiveRunError::Cancelled);
        }

        let mut entry = open_entry(&mut zip, index, password.as_deref())?;

        // Zip-slip guard: `enclosed_name` returns None for absolute/`..` paths.
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let out_path = dest.join(&relative);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|error| map_archive_error(error, &cancel_flag))?;
            continue;
        }

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| map_archive_error(error, &cancel_flag))?;
        }

        let size = entry.size();
        let display_name = relative.to_string_lossy().to_string();
        let mut out =
            File::create(&out_path).map_err(|error| map_archive_error(error, &cancel_flag))?;
        let mut reader = ProgressReader::new(
            &mut entry,
            ProgressReporter::new(
                tx.clone(),
                job_id.to_string(),
                total_files,
                total_bytes_total,
                Arc::clone(&completed_files),
                Arc::clone(&total_bytes_copied),
                Arc::clone(&cancel_flag),
            ),
            display_name,
            size,
            "Extracting",
        );
        io::copy(&mut reader, &mut out).map_err(|error| map_archive_error(error, &cancel_flag))?;

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(mode));
        }
    }

    if cancel_flag.load(Ordering::SeqCst) {
        return Err(ArchiveRunError::Cancelled);
    }

    Ok(ArchiveSummary {
        total_files,
        total_bytes_total,
    })
}

fn open_entry<'a>(
    zip: &'a mut ZipArchive<File>,
    index: usize,
    password: Option<&str>,
) -> Result<zip::read::ZipFile<'a, File>, ArchiveRunError> {
    match password {
        Some(password) => zip.by_index_decrypt(index, password.as_bytes()),
        None => zip.by_index(index),
    }
    .map_err(|error| ArchiveRunError::Failed(format!("Failed to read archive entry: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::fp_bro::test_support::{scratch_dir, sink, write_file};
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    #[test]
    fn extracts_files_and_nested_dirs() {
        let dir = scratch_dir("zip_x");
        let archive = dir.join("a.zip");
        {
            let mut w = ZipWriter::new(File::create(&archive).unwrap());
            w.start_file("a.txt", SimpleFileOptions::default()).unwrap();
            w.write_all(b"hello").unwrap();
            w.add_directory("sub", SimpleFileOptions::default()).unwrap();
            w.start_file("sub/b.txt", SimpleFileOptions::default())
                .unwrap();
            w.write_all(b"world").unwrap();
            w.finish().unwrap();
        }

        let dest = dir.join("out");
        let inputs = vec![BroInputItem {
            path: archive.to_string_lossy().to_string(),
            name: "a.zip".to_string(),
            kind: "file".to_string(),
        }];
        let (tx, _rx) = sink();
        let summary = run_blocking(
            "job",
            "unzip",
            &inputs,
            dest.to_str().unwrap(),
            &HashMap::new(),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .expect("extraction succeeds");

        assert_eq!(summary.total_files, 2);
        assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "hello");
        assert_eq!(
            std::fs::read_to_string(dest.join("sub/b.txt")).unwrap(),
            "world"
        );
    }

    #[test]
    fn skips_zip_slip_entries() {
        let dir = scratch_dir("zip_slip");
        write_file(&dir.join("marker.txt"), b"safe");
        let archive = dir.join("evil.zip");
        {
            let mut w = ZipWriter::new(File::create(&archive).unwrap());
            // A traversal path that would land in `dir` (the parent of `out`) if honored.
            w.start_file("../pwned.txt", SimpleFileOptions::default())
                .unwrap();
            w.write_all(b"x").unwrap();
            w.start_file("ok.txt", SimpleFileOptions::default()).unwrap();
            w.write_all(b"y").unwrap();
            w.finish().unwrap();
        }

        let dest = dir.join("out");
        let inputs = vec![BroInputItem {
            path: archive.to_string_lossy().to_string(),
            name: "evil.zip".to_string(),
            kind: "file".to_string(),
        }];
        let (tx, _rx) = sink();
        run_blocking(
            "job",
            "unzip",
            &inputs,
            dest.to_str().unwrap(),
            &HashMap::new(),
            &tx,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();

        assert!(dest.join("ok.txt").exists());
        assert!(!dir.join("pwned.txt").exists(), "traversal escaped dest");
    }
}
