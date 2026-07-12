use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::UNIX_EPOCH;

use regex::Regex;
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
const FEDUP_FINGERPRINT_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct FedupScanOptions {
    pub exclude_patterns: Vec<String>,
    pub include_patterns: Vec<String>,
}

struct PatternMatcher {
    name_regex: Regex,
    path_regex: Regex,
}

impl FedupScanOptions {
    pub fn new(exclude_patterns: Vec<String>, include_patterns: Vec<String>) -> Self {
        Self {
            exclude_patterns,
            include_patterns,
        }
    }

    fn exclude_matchers(&self) -> Vec<PatternMatcher> {
        compile_wildcard_patterns(&self.exclude_patterns)
    }

    fn include_matchers(&self) -> Vec<PatternMatcher> {
        compile_wildcard_patterns(&self.include_patterns)
    }
}

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

#[derive(Clone, Debug)]
struct FingerprintedCandidate {
    candidate: Candidate,
    full_hash: Option<String>,
}

#[derive(Clone, Debug)]
struct FedupFingerprint {
    key: String,
    full_hash: Option<String>,
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
    options: FedupScanOptions,
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
        run_fedup_report_blocking(
            &worker_job_id,
            &inputs,
            &options,
            &worker_tx,
            &worker_cancel,
        )
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
    options: &FedupScanOptions,
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
    let exclude_matchers = options.exclude_matchers();
    let include_matchers = options.include_matchers();
    for input in inputs {
        if cancel.load(Ordering::SeqCst) {
            return Err("Cancelled".to_string());
        }
        let path = PathBuf::from(&input.path);
        scan_path(
            &path,
            &path,
            &mut build,
            &exclude_matchers,
            &include_matchers,
            cancel,
        )?;
    }

    let candidate_count = build
        .buckets
        .values()
        .filter(|bucket| bucket.len() > 1)
        .map(Vec::len)
        .sum::<usize>();
    let mut groups = Vec::new();
    let mut fingerprinted_count = 0usize;
    let mut fingerprint_groups = Vec::new();

    for bucket in build.buckets.values() {
        if cancel.load(Ordering::SeqCst) {
            return Err("Cancelled".to_string());
        }
        if bucket.len() <= 1 {
            continue;
        }

        let mut by_fingerprint: HashMap<String, Vec<FingerprintedCandidate>> = HashMap::new();
        for candidate in bucket {
            if cancel.load(Ordering::SeqCst) {
                return Err("Cancelled".to_string());
            }
            let path_text = candidate.path.to_string_lossy().to_string();
            let fingerprint_bytes = candidate
                .size
                .min(FEDUP_FINGERPRINT_BYTES.saturating_mul(2));
            send_progress_sync(
                tx,
                job_id,
                FileOpProgressSchema {
                    total_files: candidate_count,
                    completed_files: fingerprinted_count,
                    current_file: Some(path_text.clone()),
                    status_text: Some("Fingerprinting potential duplicates".to_string()),
                    current_file_bytes_total: fingerprint_bytes,
                    current_file_bytes_copied: 0,
                    total_bytes_total: 0,
                    total_bytes_copied: 0,
                    current_file_percent: None,
                    overall_percent: if candidate_count > 0 {
                        Some(((fingerprinted_count * 100) / candidate_count) as u8)
                    } else {
                        None
                    },
                },
            );

            match hash_fedup_fingerprint(candidate, &|| cancel.load(Ordering::SeqCst), |hashed| {
                send_progress_sync(
                    tx,
                    job_id,
                    FileOpProgressSchema {
                        total_files: candidate_count,
                        completed_files: fingerprinted_count,
                        current_file: Some(path_text.clone()),
                        status_text: Some("Fingerprinting potential duplicates".to_string()),
                        current_file_bytes_total: fingerprint_bytes,
                        current_file_bytes_copied: hashed,
                        total_bytes_total: 0,
                        total_bytes_copied: 0,
                        current_file_percent: if fingerprint_bytes > 0 {
                            Some(((hashed.saturating_mul(100)) / fingerprint_bytes).min(100) as u8)
                        } else {
                            Some(100)
                        },
                        overall_percent: if candidate_count > 0 {
                            Some(((fingerprinted_count * 100) / candidate_count) as u8)
                        } else {
                            None
                        },
                    },
                );
            }) {
                Ok(fingerprint) => {
                    by_fingerprint.entry(fingerprint.key).or_default().push(
                        FingerprintedCandidate {
                            candidate: candidate.clone(),
                            full_hash: fingerprint.full_hash,
                        },
                    );
                }
                Err(error) if error.contains("Cancelled") => {
                    return Err("Cancelled".to_string());
                }
                Err(error) => build.errors.push(FedupBroErrorSchema {
                    path: path_text,
                    message: format!("Cannot fingerprint file: {error}"),
                }),
            }
            fingerprinted_count += 1;
        }

        fingerprint_groups.extend(by_fingerprint.into_values().filter(|group| group.len() > 1));
    }

    let full_hash_count = fingerprint_groups
        .iter()
        .flat_map(|group| group.iter())
        .filter(|candidate| candidate.full_hash.is_none())
        .count();
    let mut full_hashed_count = 0usize;

    for fingerprint_group in fingerprint_groups {
        if cancel.load(Ordering::SeqCst) {
            return Err("Cancelled".to_string());
        }

        let mut by_hash: HashMap<String, Vec<HashedCandidate>> = HashMap::new();
        for candidate in fingerprint_group {
            if cancel.load(Ordering::SeqCst) {
                return Err("Cancelled".to_string());
            }
            let path_text = candidate.candidate.path.to_string_lossy().to_string();
            let hash = if let Some(hash) = candidate.full_hash.clone() {
                hash
            } else {
                send_progress_sync(
                    tx,
                    job_id,
                    FileOpProgressSchema {
                        total_files: full_hash_count,
                        completed_files: full_hashed_count,
                        current_file: Some(path_text.clone()),
                        status_text: Some("Hashing matching fingerprints".to_string()),
                        current_file_bytes_total: candidate.candidate.size,
                        current_file_bytes_copied: 0,
                        total_bytes_total: 0,
                        total_bytes_copied: 0,
                        current_file_percent: None,
                        overall_percent: if full_hash_count > 0 {
                            Some(((full_hashed_count * 100) / full_hash_count) as u8)
                        } else {
                            None
                        },
                    },
                );
                match hash_local_file_blake3(
                    &candidate.candidate.path,
                    &|| cancel.load(Ordering::SeqCst),
                    |hashed| {
                        send_progress_sync(
                            tx,
                            job_id,
                            FileOpProgressSchema {
                                total_files: full_hash_count,
                                completed_files: full_hashed_count,
                                current_file: Some(path_text.clone()),
                                status_text: Some("Hashing matching fingerprints".to_string()),
                                current_file_bytes_total: candidate.candidate.size,
                                current_file_bytes_copied: hashed,
                                total_bytes_total: 0,
                                total_bytes_copied: 0,
                                current_file_percent: if candidate.candidate.size > 0 {
                                    Some(
                                        ((hashed.saturating_mul(100)) / candidate.candidate.size)
                                            .min(100) as u8,
                                    )
                                } else {
                                    Some(100)
                                },
                                overall_percent: if full_hash_count > 0 {
                                    Some(((full_hashed_count * 100) / full_hash_count) as u8)
                                } else {
                                    None
                                },
                            },
                        );
                    },
                ) {
                    Ok(hash) => {
                        full_hashed_count += 1;
                        hash
                    }
                    Err(error) if error.to_string().contains("Cancelled") => {
                        return Err("Cancelled".to_string());
                    }
                    Err(error) => {
                        build.errors.push(FedupBroErrorSchema {
                            path: path_text,
                            message: format!("Cannot hash file: {error}"),
                        });
                        full_hashed_count += 1;
                        continue;
                    }
                }
            };
            by_hash
                .entry(hash.clone())
                .or_default()
                .push(HashedCandidate {
                    candidate: candidate.candidate,
                    hash,
                });
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

fn hash_fedup_fingerprint(
    candidate: &Candidate,
    cancel: &dyn Fn() -> bool,
    mut progress: impl FnMut(u64),
) -> Result<FedupFingerprint, String> {
    if cancel() {
        return Err("Cancelled".to_string());
    }

    let mut file = std::fs::File::open(&candidate.path).map_err(|error| error.to_string())?;
    let mut hasher = blake3::Hasher::new();
    if candidate.size <= FEDUP_FINGERPRINT_BYTES.saturating_mul(2) {
        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|error| error.to_string())?;
        if cancel() {
            return Err("Cancelled".to_string());
        }
        hasher.update(&buffer);
        progress(buffer.len() as u64);
        let hash = hasher.finalize().to_hex().to_string();
        return Ok(FedupFingerprint {
            key: format!("full:{hash}"),
            full_hash: Some(hash),
        });
    }

    let chunk_len = FEDUP_FINGERPRINT_BYTES as usize;
    let mut head = vec![0u8; chunk_len];
    file.read_exact(&mut head)
        .map_err(|error| error.to_string())?;
    hasher.update(&head);
    progress(FEDUP_FINGERPRINT_BYTES);
    if cancel() {
        return Err("Cancelled".to_string());
    }

    file.seek(SeekFrom::End(-(FEDUP_FINGERPRINT_BYTES as i64)))
        .map_err(|error| error.to_string())?;
    let mut tail = vec![0u8; chunk_len];
    file.read_exact(&mut tail)
        .map_err(|error| error.to_string())?;
    hasher.update(&tail);
    progress(FEDUP_FINGERPRINT_BYTES.saturating_mul(2));

    Ok(FedupFingerprint {
        key: format!("partial:{}:{}", candidate.size, hasher.finalize().to_hex()),
        full_hash: None,
    })
}

fn scan_path(
    root: &Path,
    path: &Path,
    build: &mut ScanBuild,
    exclude_matchers: &[PatternMatcher],
    include_matchers: &[PatternMatcher],
    cancel: &AtomicBool,
) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("Cancelled".to_string());
    }
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let relative_path = relative_pattern_path(root, path);
    let full_path = path.to_string_lossy().replace('\\', "/");
    if matches_any(exclude_matchers, &name, &relative_path, &full_path) {
        return Ok(());
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
                Ok(entry) => scan_path(
                    root,
                    &entry.path(),
                    build,
                    exclude_matchers,
                    include_matchers,
                    cancel,
                )?,
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
    if !include_matchers.is_empty()
        && !matches_any(include_matchers, &name, &relative_path, &full_path)
    {
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

pub fn parse_fedup_patterns(input: &str) -> Vec<String> {
    input
        .split(|c| c == '\n' || c == '\r' || c == ',')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn compile_wildcard_patterns(patterns: &[String]) -> Vec<PatternMatcher> {
    patterns
        .iter()
        .filter_map(|pattern| compile_wildcard_pattern(pattern))
        .collect()
}

fn compile_wildcard_pattern(pattern: &str) -> Option<PatternMatcher> {
    let normalized = pattern.replace('\\', "/");
    let escaped = regex::escape(&normalized);
    let pattern = escaped.replace("\\*", ".*").replace("\\?", ".");
    let name_regex = Regex::new(&format!("(?i)^{}$", pattern)).ok()?;
    let path_regex = Regex::new(&format!("(?i)(?:^|.*/){}(?:/.*)?$", pattern)).ok()?;
    Some(PatternMatcher {
        name_regex,
        path_regex,
    })
}

fn matches_any(
    matchers: &[PatternMatcher],
    name: &str,
    relative_path: &str,
    full_path: &str,
) -> bool {
    matchers.iter().any(|matcher| {
        matcher.name_regex.is_match(name)
            || matcher.path_regex.is_match(relative_path)
            || matcher.path_regex.is_match(full_path)
    })
}

fn relative_pattern_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
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
            &FedupScanOptions::default(),
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
    fn report_uses_full_hash_after_matching_large_fingerprints() {
        let dir = TempDir::new("large-fingerprint-collision");
        let mut a = Vec::new();
        a.extend(vec![b'h'; FEDUP_FINGERPRINT_BYTES as usize]);
        a.extend(vec![b'a'; 1024]);
        a.extend(vec![b't'; FEDUP_FINGERPRINT_BYTES as usize]);
        let mut b = Vec::new();
        b.extend(vec![b'h'; FEDUP_FINGERPRINT_BYTES as usize]);
        b.extend(vec![b'b'; 1024]);
        b.extend(vec![b't'; FEDUP_FINGERPRINT_BYTES as usize]);

        std::fs::write(dir.path.join("a.bin"), &a).unwrap();
        std::fs::write(dir.path.join("b.bin"), &b).unwrap();
        std::fs::write(dir.path.join("c.bin"), &a).unwrap();

        let (tx, _rx) = mpsc::channel(64);
        let cancel = AtomicBool::new(false);
        let result = run_fedup_report_blocking(
            "job-test",
            &[BroInputItem {
                path: dir.path.to_string_lossy().to_string(),
                name: "large-fingerprint-collision".to_string(),
                kind: "directory".to_string(),
            }],
            &FedupScanOptions::default(),
            &tx,
            &cancel,
        )
        .unwrap();

        assert_eq!(result.scanned_files, 3);
        assert_eq!(result.duplicate_groups, 1);
        assert_eq!(result.duplicate_files, 1);
        let group_paths = result.groups[0]
            .duplicates
            .iter()
            .map(|file| file.path.as_str())
            .chain(std::iter::once(result.groups[0].keeper.path.as_str()))
            .collect::<Vec<_>>();
        assert!(group_paths.iter().any(|path| path.ends_with("a.bin")));
        assert!(group_paths.iter().any(|path| path.ends_with("c.bin")));
        assert!(!group_paths.iter().any(|path| path.ends_with("b.bin")));
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
            &FedupScanOptions::default(),
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
    fn parses_patterns_from_lines_and_commas() {
        assert_eq!(
            parse_fedup_patterns(" .git\nnode_modules,  *.tmp\r\n\n.DS_Store "),
            vec![".git", "node_modules", "*.tmp", ".DS_Store"]
        );
    }

    #[test]
    fn wildcard_matchers_are_case_insensitive() {
        let matchers = compile_wildcard_patterns(&["*.MP4".to_string(), "._*".to_string()]);

        assert!(matches_any(
            &matchers,
            "movie.mp4",
            "clips/movie.mp4",
            "/tmp/clips/movie.mp4"
        ));
        assert!(matches_any(
            &matchers,
            "._clip.mov",
            "clips/._clip.mov",
            "/tmp/clips/._clip.mov"
        ));
        assert!(!matches_any(
            &matchers,
            "movie.mkv",
            "clips/movie.mkv",
            "/tmp/clips/movie.mkv"
        ));
    }

    #[test]
    fn scan_excludes_special_directories_and_macos_metadata() {
        let dir = TempDir::new("exclude-patterns");
        std::fs::write(dir.path.join("a.txt"), b"same").unwrap();
        std::fs::write(dir.path.join("b.txt"), b"same").unwrap();
        std::fs::write(dir.path.join("._movie.mp4"), b"xxxx").unwrap();
        std::fs::create_dir_all(dir.path.join(".git")).unwrap();
        std::fs::write(dir.path.join(".git").join("hidden-a.bin"), b"same").unwrap();
        std::fs::create_dir_all(dir.path.join("node_modules")).unwrap();
        std::fs::write(dir.path.join("node_modules").join("hidden-b.bin"), b"same").unwrap();

        let (tx, _rx) = mpsc::channel(64);
        let cancel = AtomicBool::new(false);
        let result = run_fedup_report_blocking(
            "job-test",
            &[BroInputItem {
                path: dir.path.to_string_lossy().to_string(),
                name: "exclude-patterns".to_string(),
                kind: "directory".to_string(),
            }],
            &FedupScanOptions::new(
                vec![
                    ".git".to_string(),
                    "node_modules".to_string(),
                    "._*".to_string(),
                ],
                Vec::new(),
            ),
            &tx,
            &cancel,
        )
        .unwrap();

        assert_eq!(result.scanned_files, 2);
        assert_eq!(result.duplicate_groups, 1);
        assert!(result.groups[0].keeper.path.ends_with("a.txt"));
        assert!(result.groups[0].duplicates[0].path.ends_with("b.txt"));
    }

    #[test]
    fn scan_include_patterns_filter_files_only() {
        let dir = TempDir::new("include-patterns");
        std::fs::write(dir.path.join("a.mp4"), b"same").unwrap();
        std::fs::write(dir.path.join("b.MP4"), b"same").unwrap();
        std::fs::write(dir.path.join("a.txt"), b"text").unwrap();
        std::fs::write(dir.path.join("b.txt"), b"text").unwrap();

        let (tx, _rx) = mpsc::channel(64);
        let cancel = AtomicBool::new(false);
        let result = run_fedup_report_blocking(
            "job-test",
            &[BroInputItem {
                path: dir.path.to_string_lossy().to_string(),
                name: "include-patterns".to_string(),
                kind: "directory".to_string(),
            }],
            &FedupScanOptions::new(Vec::new(), vec!["*.mp4".to_string()]),
            &tx,
            &cancel,
        )
        .unwrap();

        assert_eq!(result.scanned_files, 2);
        assert_eq!(result.duplicate_groups, 1);
        assert_eq!(result.groups[0].duplicates.len(), 1);
        assert!(result.groups[0].keeper.path.ends_with("a.mp4"));
        assert!(result.groups[0].duplicates[0].path.ends_with("b.MP4"));
    }

    #[test]
    fn scan_excludes_relative_path_patterns() {
        let dir = TempDir::new("relative-exclude-patterns");
        std::fs::write(dir.path.join("a.bin"), b"same").unwrap();
        std::fs::write(dir.path.join("b.bin"), b"same").unwrap();
        std::fs::create_dir_all(dir.path.join("target").join("debug")).unwrap();
        std::fs::write(
            dir.path.join("target").join("debug").join("debug-a.bin"),
            b"same",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path.join("dist").join("assets")).unwrap();
        std::fs::write(
            dir.path.join("dist").join("assets").join("asset-a.bin"),
            b"same",
        )
        .unwrap();

        let (tx, _rx) = mpsc::channel(64);
        let cancel = AtomicBool::new(false);
        let result = run_fedup_report_blocking(
            "job-test",
            &[BroInputItem {
                path: dir.path.to_string_lossy().to_string(),
                name: "relative-exclude-patterns".to_string(),
                kind: "directory".to_string(),
            }],
            &FedupScanOptions::new(
                vec!["target/debug".to_string(), "dist/assets".to_string()],
                Vec::new(),
            ),
            &tx,
            &cancel,
        )
        .unwrap();

        assert_eq!(result.scanned_files, 2);
        assert_eq!(result.duplicate_groups, 1);
        assert!(result.groups[0].keeper.path.ends_with("a.bin"));
        assert!(result.groups[0].duplicates[0].path.ends_with("b.bin"));
    }

    #[test]
    fn scan_excludes_absolute_paths_under_ignored_path_pattern() {
        let dir = TempDir::new("absolute-exclude-patterns");
        std::fs::write(dir.path.join("a.bin"), b"same").unwrap();
        std::fs::write(dir.path.join("b.bin"), b"same").unwrap();
        let ignored_dir = dir
            .path
            .join("artifacts")
            .join("e2e")
            .join("fbro-a")
            .join("state")
            .join("home")
            .join(".cache")
            .join("dconf");
        std::fs::create_dir_all(&ignored_dir).unwrap();
        std::fs::write(ignored_dir.join("user"), b"same").unwrap();

        let (tx, _rx) = mpsc::channel(64);
        let cancel = AtomicBool::new(false);
        let result = run_fedup_report_blocking(
            "job-test",
            &[
                BroInputItem {
                    path: dir.path.join("a.bin").to_string_lossy().to_string(),
                    name: "a.bin".to_string(),
                    kind: "file".to_string(),
                },
                BroInputItem {
                    path: dir.path.join("b.bin").to_string_lossy().to_string(),
                    name: "b.bin".to_string(),
                    kind: "file".to_string(),
                },
                BroInputItem {
                    path: ignored_dir.join("user").to_string_lossy().to_string(),
                    name: "user".to_string(),
                    kind: "file".to_string(),
                },
            ],
            &FedupScanOptions::new(vec!["artifacts/e2e".to_string()], Vec::new()),
            &tx,
            &cancel,
        )
        .unwrap();

        assert_eq!(result.scanned_files, 2);
        assert_eq!(result.duplicate_groups, 1);
        assert!(result.groups[0].keeper.path.ends_with("a.bin"));
        assert!(result.groups[0].duplicates[0].path.ends_with("b.bin"));
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
