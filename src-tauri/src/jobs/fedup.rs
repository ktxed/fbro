use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::UNIX_EPOCH;

use tokio::sync::{mpsc, watch};

use crate::bro::BroInputItem;
use crate::jobs::JobKind;
use crate::state::SharedState;
use crate::vfs::hash_local_file_blake3;
use crate::ws::protocol::{
    now, FedupBroErrorSchema, FedupBroFileSchema, FedupBroGroupSchema, FedupBroResultPageSchema,
    FedupBroResultSchema, FileOpProgressSchema, ServerMessage,
};

pub const FEDUP_REPORT_NAME: &str = "fbro-fedup-report.txt";
pub const FEDUP_DELETE_DRY_RUN_SCRIPT_NAME: &str = "fbro-fedup-delete-dry-run.sh";
pub const FEDUP_DELETE_DRY_RUN_MANIFEST_NAME: &str = "fbro-fedup-delete-manifest.txt";
pub const FEDUP_RESULT_PAGE_SIZE: usize = 20;
pub const FEDUP_RESULT_MAX_PAGE_SIZE: usize = 100;

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
    state: SharedState,
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
            let result = Arc::new(result);
            {
                let mut state_guard = state.lock().await;
                if let Some(job) = state_guard.job_store.jobs.get_mut(&job_id) {
                    if let JobKind::FedupBro(meta) = &mut job.kind {
                        meta.result = Some(Arc::clone(&result));
                    }
                }
            }
            let _ = tx
                .send(ServerMessage::FedupBroResult {
                    timestamp: now(),
                    job_id: job_id.clone(),
                    result: fedup_result_page(
                        &result,
                        0,
                        FEDUP_RESULT_PAGE_SIZE,
                        0,
                        FEDUP_RESULT_PAGE_SIZE,
                    ),
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

pub fn fedup_result_page(
    result: &FedupBroResultSchema,
    group_offset: usize,
    group_limit: usize,
    error_offset: usize,
    error_limit: usize,
) -> FedupBroResultPageSchema {
    let group_limit = clamp_page_limit(group_limit);
    let error_limit = clamp_page_limit(error_limit);
    let group_offset = group_offset.min(result.groups.len());
    let error_offset = error_offset.min(result.errors.len());
    let group_end = group_offset
        .saturating_add(group_limit)
        .min(result.groups.len());
    let error_end = error_offset
        .saturating_add(error_limit)
        .min(result.errors.len());

    FedupBroResultPageSchema {
        duplicate_groups: result.duplicate_groups,
        duplicate_files: result.duplicate_files,
        reclaimable_bytes: result.reclaimable_bytes,
        scanned_files: result.scanned_files,
        scanned_bytes: result.scanned_bytes,
        groups: result.groups[group_offset..group_end].to_vec(),
        errors: result.errors[error_offset..error_end].to_vec(),
        group_offset,
        group_limit,
        group_total: result.groups.len(),
        error_offset,
        error_limit,
        error_total: result.errors.len(),
    }
}

fn clamp_page_limit(limit: usize) -> usize {
    limit.clamp(1, FEDUP_RESULT_MAX_PAGE_SIZE)
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

pub fn format_fedup_delete_dry_run_script(result: &FedupBroResultSchema) -> Result<String, String> {
    if result.duplicate_files == 0 {
        return Err("No duplicate files are available for delete dry-run.".to_string());
    }

    let mut out = String::new();
    out.push_str("#!/usr/bin/env bash\n");
    out.push_str("# Generated by fbro Fedup Bro delete dry-run.\n");
    out.push_str("# Review carefully. Running without --apply does not delete anything.\n");
    out.push_str("# Keeper files are not deleted.\n\n");
    out.push_str("set -euo pipefail\n\n");
    out.push_str("apply=0\n");
    out.push_str("if [[ \"${1:-}\" == \"--apply\" ]]; then\n");
    out.push_str("  apply=1\n");
    out.push_str("elif [[ $# -gt 0 ]]; then\n");
    out.push_str("  echo \"Usage: $0 [--apply]\" >&2\n");
    out.push_str("  exit 2\n");
    out.push_str("fi\n\n");
    out.push_str("delete_candidate() {\n");
    out.push_str("  local path=\"$1\"\n");
    out.push_str("  if [[ \"$apply\" -eq 1 ]]; then\n");
    out.push_str("    rm -f -- \"$path\"\n");
    out.push_str("  else\n");
    out.push_str("    printf 'dry-run: rm -f -- %q\\n' \"$path\"\n");
    out.push_str("  fi\n");
    out.push_str("}\n\n");
    for (group_index, group) in result.groups.iter().enumerate() {
        if group.duplicates.is_empty() {
            continue;
        }
        out.push_str(&format!("# Group {}\n", group_index + 1));
        out.push_str(&format!("# Hash: {}\n", group.hash));
        out.push_str(&format!("# Size: {}\n", group.size));
        out.push_str(&format!("# Keep: {}\n", group.keeper.path));
        for duplicate in &group.duplicates {
            out.push_str(&format!("# Delete candidate: {}\n", duplicate.path));
            out.push_str(&format!(
                "delete_candidate {}\n",
                shell_quote(&duplicate.path)
            ));
        }
        out.push('\n');
    }
    Ok(out)
}

pub fn format_fedup_delete_manifest(result: &FedupBroResultSchema) -> Result<String, String> {
    if result.duplicate_files == 0 {
        return Err("No duplicate files are available for delete dry-run.".to_string());
    }

    let mut out = String::new();
    out.push_str("Fedup Bro Delete Dry-Run Manifest\n");
    out.push_str("=================================\n\n");
    out.push_str(&format!("Duplicate groups: {}\n", result.duplicate_groups));
    out.push_str(&format!("Duplicate files: {}\n", result.duplicate_files));
    out.push_str(&format!(
        "Reclaimable bytes: {}\n\n",
        result.reclaimable_bytes
    ));
    for (group_index, group) in result.groups.iter().enumerate() {
        if group.duplicates.is_empty() {
            continue;
        }
        out.push_str(&format!("Group {}\n", group_index + 1));
        out.push_str(&format!("Hash: {}\n", group.hash));
        out.push_str(&format!("Size: {}\n", group.size));
        out.push_str(&format!("Keeper: {}\n", group.keeper.path));
        for duplicate in &group.duplicates {
            out.push_str(&format!("Duplicate: {}\n", duplicate.path));
            out.push_str(&format!("  size: {}\n", duplicate.size));
            out.push_str(&format!("  mtime: {}\n", duplicate.mtime.unwrap_or(0)));
            out.push_str(&format!("  hash: {}\n", duplicate.hash));
        }
        out.push('\n');
    }
    Ok(out)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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

    #[test]
    fn result_page_slices_groups_and_errors_with_clamped_limits() {
        let groups = (0..5)
            .map(|index| FedupBroGroupSchema {
                hash: format!("hash-{index}"),
                size: 4,
                keeper: FedupBroFileSchema {
                    path: format!("/tmp/keep-{index}.txt"),
                    size: 4,
                    mtime: Some(index),
                    hash: format!("hash-{index}"),
                },
                duplicates: vec![FedupBroFileSchema {
                    path: format!("/tmp/delete-{index}.txt"),
                    size: 4,
                    mtime: Some(index + 1),
                    hash: format!("hash-{index}"),
                }],
            })
            .collect::<Vec<_>>();
        let errors = (0..3)
            .map(|index| FedupBroErrorSchema {
                path: format!("/tmp/error-{index}.txt"),
                message: "cannot read".to_string(),
            })
            .collect::<Vec<_>>();
        let result = FedupBroResultSchema {
            duplicate_groups: 5,
            duplicate_files: 5,
            reclaimable_bytes: 20,
            scanned_files: 12,
            scanned_bytes: 48,
            groups,
            errors,
        };

        let page = fedup_result_page(&result, 2, 2, 1, 50);
        assert_eq!(page.group_offset, 2);
        assert_eq!(page.group_limit, 2);
        assert_eq!(page.group_total, 5);
        assert_eq!(page.groups.len(), 2);
        assert_eq!(page.groups[0].hash, "hash-2");
        assert_eq!(page.error_offset, 1);
        assert_eq!(page.error_limit, 50);
        assert_eq!(page.error_total, 3);
        assert_eq!(page.errors.len(), 2);

        let clamped = fedup_result_page(&result, 50, 0, 50, usize::MAX);
        assert_eq!(clamped.group_offset, 5);
        assert_eq!(clamped.group_limit, 1);
        assert!(clamped.groups.is_empty());
        assert_eq!(clamped.error_offset, 3);
        assert_eq!(clamped.error_limit, FEDUP_RESULT_MAX_PAGE_SIZE);
        assert!(clamped.errors.is_empty());
    }

    #[test]
    fn delete_dry_run_includes_duplicates_only_with_context() {
        let result = FedupBroResultSchema {
            duplicate_groups: 1,
            duplicate_files: 1,
            reclaimable_bytes: 4,
            scanned_files: 2,
            scanned_bytes: 8,
            groups: vec![FedupBroGroupSchema {
                hash: "abc123".to_string(),
                size: 4,
                keeper: FedupBroFileSchema {
                    path: "/tmp/keep's file.txt".to_string(),
                    size: 4,
                    mtime: Some(1),
                    hash: "abc123".to_string(),
                },
                duplicates: vec![FedupBroFileSchema {
                    path: "/tmp/delete's file.txt".to_string(),
                    size: 4,
                    mtime: Some(2),
                    hash: "abc123".to_string(),
                }],
            }],
            errors: Vec::new(),
        };

        let script = format_fedup_delete_dry_run_script(&result).unwrap();
        assert!(script.contains("Usage: $0 [--apply]"));
        assert!(script.contains("rm -f -- \"$path\""));
        assert!(script.contains("# Keep: /tmp/keep's file.txt"));
        assert!(script.contains("# Delete candidate: /tmp/delete's file.txt"));
        assert!(script.contains("delete_candidate '/tmp/delete'\\''s file.txt'"));
        assert!(!script.contains("delete_candidate '/tmp/keep"));

        let manifest = format_fedup_delete_manifest(&result).unwrap();
        assert!(manifest.contains("Keeper: /tmp/keep's file.txt"));
        assert!(manifest.contains("Duplicate: /tmp/delete's file.txt"));
        assert!(manifest.contains("hash: abc123"));
    }

    #[test]
    fn delete_dry_run_rejects_empty_results() {
        let result = FedupBroResultSchema {
            duplicate_groups: 0,
            duplicate_files: 0,
            reclaimable_bytes: 0,
            scanned_files: 1,
            scanned_bytes: 1,
            groups: Vec::new(),
            errors: Vec::new(),
        };

        assert!(format_fedup_delete_dry_run_script(&result).is_err());
        assert!(format_fedup_delete_manifest(&result).is_err());
    }
}
