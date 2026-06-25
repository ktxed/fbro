use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::UNIX_EPOCH;

use tokio::sync::{mpsc, watch};

use crate::bro::BroInputItem;
use crate::vfs::hash_local_file_blake3;
use crate::ws::protocol::{
    now, FedupBroErrorSchema, FedupBroFileSchema, FedupBroGroupSchema, FedupBroResultSchema,
    FileOpProgressSchema, ServerMessage,
};

pub const FEDUP_REPORT_NAME: &str = "fbro-fedup-report.txt";

#[derive(Clone, Debug)]
struct Candidate {
    path: PathBuf,
    size: u64,
    mtime: Option<u64>,
}

#[derive(Clone, Debug)]
struct HashedCandidate {
    candidate: Candidate,
    hash: String,
}

#[derive(Clone, Debug, Default)]
struct ScanBuild {
    buckets: HashMap<u64, Vec<Candidate>>,
    errors: Vec<FedupBroErrorSchema>,
    total_files: usize,
    total_bytes: u64,
}

pub async fn run_fedup_bro(
    job_id: String,
    inputs: Vec<BroInputItem>,
    tx: mpsc::Sender<ServerMessage>,
    cancel_rx: watch::Receiver<bool>,
) {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_task = Arc::clone(&cancel);
    let mut cancel_watch = cancel_rx.clone();
    let cancel_task = tokio::spawn(async move {
        while cancel_watch.changed().await.is_ok() {
            if *cancel_watch.borrow() {
                cancel_for_task.store(true, Ordering::SeqCst);
                break;
            }
        }
    });

    let worker_job_id = job_id.clone();
    let worker_tx = tx.clone();
    let worker_cancel = Arc::clone(&cancel);
    let result = tokio::task::spawn_blocking(move || {
        run_fedup_report_blocking(&worker_job_id, &inputs, &worker_tx, &worker_cancel)
    })
    .await;
    cancel_task.abort();

    match result {
        Ok(Ok(result)) => {
            let _ = tx
                .send(ServerMessage::FedupBroResult {
                    timestamp: now(),
                    job_id: job_id.clone(),
                    result,
                })
                .await;
            let _ = send_terminal(&tx, &job_id, "Completed", None).await;
        }
        Ok(Err(error)) if error == "Cancelled" => {
            let _ = send_terminal(&tx, &job_id, "Cancelled", None).await;
        }
        Ok(Err(error)) => {
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error)).await;
        }
        Err(error) => {
            let _ = send_terminal(
                &tx,
                &job_id,
                "Failed",
                Some(format!("Fedup Bro failed: {error}")),
            )
            .await;
        }
    }
}

fn run_fedup_report_blocking(
    job_id: &str,
    inputs: &[BroInputItem],
    tx: &mpsc::Sender<ServerMessage>,
    cancel: &AtomicBool,
) -> Result<FedupBroResultSchema, String> {
    send_progress_sync(
        tx,
        job_id,
        FileOpProgressSchema {
            total_files: inputs.len(),
            completed_files: 0,
            current_file: None,
            status_text: Some("Scanning inputs".to_string()),
            current_file_bytes_total: 0,
            current_file_bytes_copied: 0,
            total_bytes_total: 0,
            total_bytes_copied: 0,
            current_file_percent: None,
            overall_percent: None,
        },
    );

    let mut build = ScanBuild::default();
    for input in inputs {
        if cancel.load(Ordering::SeqCst) {
            return Err("Cancelled".to_string());
        }
        let path = PathBuf::from(&input.path);
        scan_path(&path, &mut build, cancel)?;
    }

    let candidate_count = build
        .buckets
        .values()
        .filter(|bucket| bucket.len() > 1)
        .map(Vec::len)
        .sum::<usize>();
    let mut hashed_count = 0usize;
    let mut groups = Vec::new();

    for bucket in build.buckets.values() {
        if cancel.load(Ordering::SeqCst) {
            return Err("Cancelled".to_string());
        }
        if bucket.len() <= 1 {
            continue;
        }

        let mut by_hash: HashMap<String, Vec<HashedCandidate>> = HashMap::new();
        for candidate in bucket {
            if cancel.load(Ordering::SeqCst) {
                return Err("Cancelled".to_string());
            }
            let path_text = candidate.path.to_string_lossy().to_string();
            send_progress_sync(
                tx,
                job_id,
                FileOpProgressSchema {
                    total_files: candidate_count,
                    completed_files: hashed_count,
                    current_file: Some(path_text.clone()),
                    status_text: Some("Hashing potential duplicates".to_string()),
                    current_file_bytes_total: candidate.size,
                    current_file_bytes_copied: 0,
                    total_bytes_total: 0,
                    total_bytes_copied: 0,
                    current_file_percent: None,
                    overall_percent: if candidate_count > 0 {
                        Some(((hashed_count * 100) / candidate_count) as u8)
                    } else {
                        None
                    },
                },
            );

            match hash_local_file_blake3(
                &candidate.path,
                &|| cancel.load(Ordering::SeqCst),
                |hashed| {
                    send_progress_sync(
                        tx,
                        job_id,
                        FileOpProgressSchema {
                            total_files: candidate_count,
                            completed_files: hashed_count,
                            current_file: Some(path_text.clone()),
                            status_text: Some("Hashing potential duplicates".to_string()),
                            current_file_bytes_total: candidate.size,
                            current_file_bytes_copied: hashed,
                            total_bytes_total: 0,
                            total_bytes_copied: 0,
                            current_file_percent: if candidate.size > 0 {
                                Some(((hashed.saturating_mul(100)) / candidate.size).min(100) as u8)
                            } else {
                                Some(100)
                            },
                            overall_percent: if candidate_count > 0 {
                                Some(((hashed_count * 100) / candidate_count) as u8)
                            } else {
                                None
                            },
                        },
                    );
                },
            ) {
                Ok(hash) => {
                    by_hash
                        .entry(hash.clone())
                        .or_default()
                        .push(HashedCandidate {
                            candidate: candidate.clone(),
                            hash,
                        });
                }
                Err(error) if error.to_string().contains("Cancelled") => {
                    return Err("Cancelled".to_string());
                }
                Err(error) => build.errors.push(FedupBroErrorSchema {
                    path: path_text,
                    message: format!("Cannot hash file: {error}"),
                }),
            }
            hashed_count += 1;
        }

        for group in by_hash.values() {
            if group.len() > 1 {
                groups.push(group_to_schema(group));
            }
        }
    }

    groups.sort_by(|a, b| a.keeper.path.cmp(&b.keeper.path));
    let duplicate_files = groups
        .iter()
        .map(|group| group.duplicates.len())
        .sum::<usize>();
    let reclaimable_bytes = groups
        .iter()
        .flat_map(|group| group.duplicates.iter())
        .map(|file| file.size)
        .sum::<u64>();

    send_progress_sync(
        tx,
        job_id,
        FileOpProgressSchema {
            total_files: candidate_count,
            completed_files: candidate_count,
            current_file: None,
            status_text: Some(format!(
                "{} groups, {} duplicate files",
                groups.len(),
                duplicate_files
            )),
            current_file_bytes_total: 0,
            current_file_bytes_copied: 0,
            total_bytes_total: reclaimable_bytes,
            total_bytes_copied: reclaimable_bytes,
            current_file_percent: Some(100),
            overall_percent: Some(100),
        },
    );

    Ok(FedupBroResultSchema {
        duplicate_groups: groups.len(),
        duplicate_files,
        reclaimable_bytes,
        scanned_files: build.total_files,
        scanned_bytes: build.total_bytes,
        groups,
        errors: build.errors,
    })
}

fn scan_path(path: &Path, build: &mut ScanBuild, cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("Cancelled".to_string());
    }
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            build.errors.push(FedupBroErrorSchema {
                path: path.to_string_lossy().to_string(),
                message: format!("Cannot read metadata: {error}"),
            });
            return Ok(());
        }
    };
    if metadata.is_dir() {
        let entries = match std::fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => {
                build.errors.push(FedupBroErrorSchema {
                    path: path.to_string_lossy().to_string(),
                    message: format!("Cannot read directory: {error}"),
                });
                return Ok(());
            }
        };
        for entry in entries {
            if cancel.load(Ordering::SeqCst) {
                return Err("Cancelled".to_string());
            }
            match entry {
                Ok(entry) => scan_path(&entry.path(), build, cancel)?,
                Err(error) => build.errors.push(FedupBroErrorSchema {
                    path: path.to_string_lossy().to_string(),
                    message: format!("Cannot read directory entry: {error}"),
                }),
            }
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Ok(());
    }
    if metadata.len() == 0 {
        return Ok(());
    }

    let mtime = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    let candidate = Candidate {
        path: path.to_path_buf(),
        size: metadata.len(),
        mtime,
    };
    build.total_files += 1;
    build.total_bytes = build.total_bytes.saturating_add(candidate.size);
    build
        .buckets
        .entry(candidate.size)
        .or_default()
        .push(candidate);
    Ok(())
}

fn group_to_schema(group: &[HashedCandidate]) -> FedupBroGroupSchema {
    let mut sorted = group.to_vec();
    sorted.sort_by(|left, right| {
        left.candidate
            .mtime
            .cmp(&right.candidate.mtime)
            .then_with(|| left.candidate.path.cmp(&right.candidate.path))
    });
    let keeper = sorted[0].clone();
    let duplicates = sorted
        .iter()
        .skip(1)
        .map(file_to_schema)
        .collect::<Vec<_>>();
    FedupBroGroupSchema {
        hash: keeper.hash.clone(),
        size: keeper.candidate.size,
        keeper: file_to_schema(&keeper),
        duplicates,
    }
}

fn file_to_schema(file: &HashedCandidate) -> FedupBroFileSchema {
    FedupBroFileSchema {
        path: file.candidate.path.to_string_lossy().to_string(),
        size: file.candidate.size,
        mtime: file.candidate.mtime,
        hash: file.hash.clone(),
    }
}

fn send_progress_sync(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    progress: FileOpProgressSchema,
) {
    let _ = tx.blocking_send(ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: "Progress".to_string(),
        files: None,
        progress: Some(progress),
        search_progress: None,
        decision: None,
        error: None,
        result_path: None,
        result_panel_id: None,
    });
}

async fn send_terminal(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    event: &str,
    error: Option<String>,
) -> Result<(), ()> {
    tx.send(ServerMessage::JobEvent {
        timestamp: now(),
        job_id: job_id.to_string(),
        event: event.to_string(),
        files: None,
        progress: None,
        search_progress: None,
        decision: None,
        error,
        result_path: None,
        result_panel_id: None,
    })
    .await
    .map_err(|_| ())
}

pub fn format_fedup_report_from_schema(result: &FedupBroResultSchema) -> String {
    let mut out = String::new();
    out.push_str("Fedup Bro Report\n");
    out.push_str("=================\n\n");
    out.push_str(&format!("Scanned files: {}\n", result.scanned_files));
    out.push_str(&format!("Scanned bytes: {}\n", result.scanned_bytes));
    out.push_str(&format!("Duplicate groups: {}\n", result.duplicate_groups));
    out.push_str(&format!("Duplicate files: {}\n", result.duplicate_files));
    out.push_str(&format!(
        "Reclaimable bytes: {}\n\n",
        result.reclaimable_bytes
    ));

    for (index, group) in result.groups.iter().enumerate() {
        out.push_str(&format!("Group {}\n", index + 1));
        out.push_str(&format!("Hash: {}\n", group.hash));
        out.push_str(&format!("Size: {}\n", group.size));
        out.push_str(&format!("Keep: {}\n", group.keeper.path));
        for duplicate in &group.duplicates {
            out.push_str(&format!("Duplicate: {}\n", duplicate.path));
        }
        out.push('\n');
    }

    if !result.errors.is_empty() {
        out.push_str("Errors\n");
        out.push_str("------\n");
        for error in &result.errors {
            out.push_str(&format!("{}: {}\n", error.path, error.message));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("fbro-fedup-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn report_groups_exact_duplicates_by_hash() {
        let dir = TempDir::new("duplicates");
        std::fs::write(dir.path.join("a.txt"), b"same").unwrap();
        std::fs::write(dir.path.join("b.txt"), b"same").unwrap();
        std::fs::write(dir.path.join("c.txt"), b"diff").unwrap();

        let (tx, _rx) = mpsc::channel(64);
        let cancel = AtomicBool::new(false);
        let result = run_fedup_report_blocking(
            "job-test",
            &[BroInputItem {
                path: dir.path.to_string_lossy().to_string(),
                name: "duplicates".to_string(),
                kind: "directory".to_string(),
            }],
            &tx,
            &cancel,
        )
        .unwrap();

        assert_eq!(result.scanned_files, 3);
        assert_eq!(result.duplicate_groups, 1);
        assert_eq!(result.duplicate_files, 1);
        assert_eq!(result.reclaimable_bytes, 4);
        assert!(result.errors.is_empty());
        assert!(result.groups[0].keeper.path.ends_with("a.txt"));
        assert!(result.groups[0].duplicates[0].path.ends_with("b.txt"));
    }

    #[test]
    fn report_ignores_empty_files() {
        let dir = TempDir::new("empty");
        std::fs::write(dir.path.join("empty-a.txt"), b"").unwrap();
        std::fs::write(dir.path.join("empty-b.txt"), b"").unwrap();
        std::fs::write(dir.path.join("non-empty.txt"), b"x").unwrap();

        let (tx, _rx) = mpsc::channel(64);
        let cancel = AtomicBool::new(false);
        let result = run_fedup_report_blocking(
            "job-test",
            &[BroInputItem {
                path: dir.path.to_string_lossy().to_string(),
                name: "empty".to_string(),
                kind: "directory".to_string(),
            }],
            &tx,
            &cancel,
        )
        .unwrap();

        assert_eq!(result.scanned_files, 1);
        assert_eq!(result.duplicate_groups, 0);
        assert_eq!(result.duplicate_files, 0);
        assert_eq!(result.reclaimable_bytes, 0);
    }
}
