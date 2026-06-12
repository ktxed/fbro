use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc,
};

use sevenz_rust2::{ArchiveReader, Password};
use tokio::sync::mpsc;

use crate::bro::BroInputItem;
use crate::ws::protocol::ServerMessage;

use super::archive::{
    emit_preparing_extract, ArchiveRunError, ArchiveSummary, ProgressReader, ProgressReporter,
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

    let dest = Path::new(output_path).to_path_buf();
    std::fs::create_dir_all(&dest).map_err(|error| {
        ArchiveRunError::Failed(format!("Failed to create {}: {error}", dest.display()))
    })?;

    let password = params
        .get("password")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(Password::from)
        .unwrap_or_else(Password::empty);

    let mut reader = ArchiveReader::open(&archive_path, password)
        .map_err(|error| ArchiveRunError::Failed(format!("Failed to open archive: {error}")))?;

    // Pre-scan for progress totals and file count.
    let mut total_files = 0usize;
    let mut total_bytes_total = 0u64;
    for entry in &reader.archive().files {
        if !entry.is_directory {
            total_files += 1;
            total_bytes_total += entry.size;
        }
    }

    emit_preparing_extract(tx, job_id, bro_name, total_files, total_bytes_total);

    let completed_files = Arc::new(AtomicUsize::new(0));
    let total_bytes_copied = Arc::new(AtomicU64::new(0));
    let mut failure: Option<ArchiveRunError> = None;

    let iterate = reader.for_each_entries(|entry, entry_reader| {
        if cancel_flag.load(Ordering::SeqCst) {
            return Ok(false);
        }

        // 7z entry names carry no zip-slip protection, so sanitize manually.
        let Some(out_path) = safe_join(&dest, entry.name()) else {
            return Ok(true); // skip entries that would escape the destination
        };

        if entry.is_directory {
            if let Err(error) = std::fs::create_dir_all(&out_path) {
                failure = Some(ArchiveRunError::Failed(error.to_string()));
                return Ok(false);
            }
            return Ok(true);
        }

        if let Some(parent) = out_path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                failure = Some(ArchiveRunError::Failed(error.to_string()));
                return Ok(false);
            }
        }

        let mut out = match File::create(&out_path) {
            Ok(out) => out,
            Err(error) => {
                failure = Some(ArchiveRunError::Failed(error.to_string()));
                return Ok(false);
            }
        };

        let mut progress = ProgressReader::new(
            entry_reader,
            ProgressReporter::new(
                tx.clone(),
                job_id.to_string(),
                total_files,
                total_bytes_total,
                Arc::clone(&completed_files),
                Arc::clone(&total_bytes_copied),
                Arc::clone(&cancel_flag),
            ),
            entry.name().to_string(),
            entry.size,
            "Extracting",
        );

        if let Err(error) = io::copy(&mut progress, &mut out) {
            if !cancel_flag.load(Ordering::SeqCst) {
                failure = Some(ArchiveRunError::Failed(error.to_string()));
            }
            return Ok(false);
        }

        Ok(true)
    });

    if let Some(error) = failure {
        return Err(error);
    }
    if cancel_flag.load(Ordering::SeqCst) {
        return Err(ArchiveRunError::Cancelled);
    }
    iterate.map_err(|error| ArchiveRunError::Failed(format!("Extraction failed: {error}")))?;

    Ok(ArchiveSummary {
        total_files,
        total_bytes_total,
    })
}

/// Join an archive-relative entry name onto `dest`, rejecting absolute paths and any `..`
/// traversal so a malicious archive cannot write outside the destination directory.
fn safe_join(dest: &Path, name: &str) -> Option<PathBuf> {
    let relative = Path::new(name);
    if relative.is_absolute() {
        return None;
    }
    let mut out = dest.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::fp_bro::test_support::{scratch_dir, sink, write_file};
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter};

    #[test]
    fn safe_join_blocks_traversal_and_absolute() {
        let dest = Path::new("/tmp/out");
        assert_eq!(safe_join(dest, "a/b.txt"), Some(PathBuf::from("/tmp/out/a/b.txt")));
        assert!(safe_join(dest, "../escape").is_none());
        assert!(safe_join(dest, "/etc/passwd").is_none());
    }

    #[test]
    fn extracts_files_and_nested_dirs() {
        let dir = scratch_dir("7z_x");
        let src = dir.join("src");
        write_file(&src.join("a.txt"), b"hello");
        write_file(&src.join("sub/b.txt"), b"world");

        let archive = dir.join("a.7z");
        {
            let mut w = ArchiveWriter::create(&archive).unwrap();
            w.push_archive_entry(
                ArchiveEntry::from_path(src.join("a.txt"), "a.txt".to_string()),
                Some(File::open(src.join("a.txt")).unwrap()),
            )
            .unwrap();
            w.push_archive_entry(
                ArchiveEntry::from_path(src.join("sub/b.txt"), "sub/b.txt".to_string()),
                Some(File::open(src.join("sub/b.txt")).unwrap()),
            )
            .unwrap();
            w.finish().unwrap();
        }

        let dest = dir.join("out");
        let inputs = vec![BroInputItem {
            path: archive.to_string_lossy().to_string(),
            name: "a.7z".to_string(),
            kind: "file".to_string(),
        }];
        let (tx, _rx) = sink();
        let summary = run_blocking(
            "job",
            "un7z",
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
}
