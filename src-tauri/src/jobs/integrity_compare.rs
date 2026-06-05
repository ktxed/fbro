use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::sync::{mpsc, watch};

use crate::vfs::{VfsDriver, VfsManifest};
use crate::ws::protocol::{now, FileOpProgressSchema, ServerMessage};

const CHUNK_SIZE: usize = 1024 * 1024;
pub const SYNC_REPORT_NAME: &str = "fbro-sync-report.txt";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ManifestEntry {
    pub path: String,
    pub size: u64,
    pub hash: String,
}

#[derive(Clone, Debug, Default)]
struct ManifestBuild {
    entries: Vec<ManifestEntry>,
    errors: Vec<ManifestError>,
}

#[derive(Clone, Debug)]
struct ManifestError {
    path: String,
    message: String,
}

#[derive(Clone, Debug, Default)]
struct CompareSummary {
    matched: usize,
    differing: Vec<DiffEntry>,
    local_only: Vec<String>,
    remote_only: Vec<String>,
    local_errors: Vec<ManifestError>,
    remote_errors: Vec<ManifestError>,
}

#[derive(Clone, Debug)]
struct DiffEntry {
    rel_path: String,
    local_size: u64,
    remote_size: u64,
    local_hash: String,
    remote_hash: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ManifestSide {
    Local,
    Remote,
}

#[derive(Clone, Debug)]
enum ManifestProgressEvent {
    Started(ManifestSide),
    RemoteManifestRequested(ManifestSide),
    FileHashed {
        side: ManifestSide,
        path: String,
        processed: usize,
    },
    Done {
        side: ManifestSide,
        processed: usize,
    },
}

#[derive(Clone, Debug, Default)]
struct ManifestSideProgress {
    started: bool,
    remote_manifest: bool,
    done: bool,
    processed: usize,
    current_file: Option<String>,
}

#[derive(Clone, Debug)]
struct CombinedManifestProgress {
    status_text: String,
    current_file: Option<String>,
    completed_files: usize,
    overall_percent: Option<u8>,
}

pub async fn run_sync_bro(
    job_id: String,
    local_panel: String,
    _remote_panel: String,
    local_root: String,
    remote_root: String,
    report_path: String,
    local_driver: Arc<dyn VfsDriver>,
    remote_driver: Arc<dyn VfsDriver>,
    tx: mpsc::Sender<ServerMessage>,
    cancel_rx: watch::Receiver<bool>,
) {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let cancel_for_task = Arc::clone(&cancel_flag);
    let mut cancel_watch = cancel_rx.clone();
    let cancel_task = tokio::spawn(async move {
        while cancel_watch.changed().await.is_ok() {
            if *cancel_watch.borrow() {
                cancel_for_task.store(true, Ordering::SeqCst);
                break;
            }
        }
    });

    let (progress_tx, progress_rx) = mpsc::channel(128);
    let progress_task = tokio::spawn(sync_bro_progress_aggregator(
        job_id.clone(),
        tx.clone(),
        progress_rx,
    ));

    let local_root_for_worker = local_root.clone();
    let local_driver_for_worker = Arc::clone(&local_driver);
    let local_cancel = Arc::clone(&cancel_flag);
    let local_progress_tx = progress_tx.clone();
    let local_worker = tokio::task::spawn_blocking(move || {
        send_manifest_progress_sync(
            &local_progress_tx,
            ManifestProgressEvent::Started(ManifestSide::Local),
        );
        build_manifest(
            ManifestSide::Local,
            "local",
            &local_root_for_worker,
            &*local_driver_for_worker,
            &local_progress_tx,
            &local_cancel,
        )
    });

    let remote_root_for_worker = remote_root.clone();
    let remote_driver_for_worker = Arc::clone(&remote_driver);
    let remote_cancel = Arc::clone(&cancel_flag);
    let remote_progress_tx = progress_tx.clone();
    let remote_worker = tokio::task::spawn_blocking(move || {
        send_manifest_progress_sync(
            &remote_progress_tx,
            ManifestProgressEvent::Started(ManifestSide::Remote),
        );
        build_manifest(
            ManifestSide::Remote,
            "remote",
            &remote_root_for_worker,
            &*remote_driver_for_worker,
            &remote_progress_tx,
            &remote_cancel,
        )
    });
    drop(progress_tx);

    let (local_result, remote_result) = tokio::join!(local_worker, remote_worker);
    cancel_task.abort();

    let local_build = match manifest_worker_result(local_result) {
        Ok(build) => build,
        Err(error) if error == "Cancelled" => {
            let _ = progress_task.await;
            let _ = send_terminal(&tx, &job_id, "Cancelled", None, None, None).await;
            return;
        }
        Err(error) => {
            let _ = progress_task.await;
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None, None).await;
            return;
        }
    };
    let remote_build = match manifest_worker_result(remote_result) {
        Ok(build) => build,
        Err(error) if error == "Cancelled" => {
            let _ = progress_task.await;
            let _ = send_terminal(&tx, &job_id, "Cancelled", None, None, None).await;
            return;
        }
        Err(error) => {
            let _ = progress_task.await;
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None, None).await;
            return;
        }
    };
    let _ = progress_task.await;

    let _ = send_progress(
        &tx,
        &job_id,
        FileOpProgressSchema {
            total_files: local_build.entries.len().max(remote_build.entries.len()),
            completed_files: local_build.entries.len().max(remote_build.entries.len()),
            current_file: None,
            status_text: Some("Comparing manifests".to_string()),
            current_file_bytes_total: 0,
            current_file_bytes_copied: 0,
            total_bytes_total: 0,
            total_bytes_copied: 0,
            current_file_percent: None,
            overall_percent: None,
        },
    )
    .await;

    let mut summary = compare_manifests(&local_build.entries, &remote_build.entries);
    summary.local_errors = local_build.errors;
    summary.remote_errors = remote_build.errors;
    let status = format!(
        "{} matched, {} differing, {} local-only, {} remote-only, {} errors",
        summary.matched,
        summary.differing.len(),
        summary.local_only.len(),
        summary.remote_only.len(),
        summary.local_errors.len() + summary.remote_errors.len()
    );
    let _ = send_progress(
        &tx,
        &job_id,
        FileOpProgressSchema {
            total_files: local_build.entries.len().max(remote_build.entries.len()),
            completed_files: local_build.entries.len().max(remote_build.entries.len()),
            current_file: None,
            status_text: Some(status.clone()),
            current_file_bytes_total: 0,
            current_file_bytes_copied: 0,
            total_bytes_total: 0,
            total_bytes_copied: 0,
            current_file_percent: Some(100),
            overall_percent: Some(100),
        },
    )
    .await;

    match write_report(
        &*local_driver,
        &report_path,
        &local_root,
        &remote_root,
        &summary,
    ) {
        Ok(()) => {
            let _ = send_terminal(
                &tx,
                &job_id,
                "Completed",
                None,
                Some(report_path),
                Some(local_panel),
            )
            .await;
        }
        Err(error) => {
            let _ = send_terminal(&tx, &job_id, "Failed", Some(error), None, None).await;
        }
    }
}

fn manifest_worker_result(
    result: Result<Result<ManifestBuild, String>, tokio::task::JoinError>,
) -> Result<ManifestBuild, String> {
    match result {
        Ok(result) => result,
        Err(error) => Err(error.to_string()),
    }
}

fn build_manifest(
    side: ManifestSide,
    label: &str,
    root: &str,
    driver: &dyn VfsDriver,
    progress_tx: &mpsc::Sender<ManifestProgressEvent>,
    cancel: &AtomicBool,
) -> Result<ManifestBuild, String> {
    if side == ManifestSide::Remote {
        send_manifest_progress_sync(
            progress_tx,
            ManifestProgressEvent::RemoteManifestRequested(side),
        );
    }
    if let Some(manifest) = driver
        .remote_manifest(root)
        .map_err(|error| format!("Cannot request {label} manifest for {root}: {error}"))?
    {
        let build = manifest_from_vfs(manifest);
        send_manifest_progress_sync(
            progress_tx,
            ManifestProgressEvent::Done {
                side,
                processed: build.entries.len() + build.errors.len(),
            },
        );
        return Ok(build);
    }

    let mut out = Vec::new();
    let mut errors = Vec::new();
    walk_dir(
        side,
        label,
        root,
        "",
        driver,
        progress_tx,
        cancel,
        &mut out,
        &mut errors,
    )?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    errors.sort_by(|a, b| a.path.cmp(&b.path));
    let build = ManifestBuild {
        entries: out,
        errors,
    };
    send_manifest_progress_sync(
        progress_tx,
        ManifestProgressEvent::Done {
            side,
            processed: build.entries.len() + build.errors.len(),
        },
    );
    Ok(build)
}

fn manifest_from_vfs(manifest: VfsManifest) -> ManifestBuild {
    let mut entries = manifest
        .entries
        .into_iter()
        .map(|entry| ManifestEntry {
            path: entry.path,
            size: entry.size,
            hash: entry.hash,
        })
        .collect::<Vec<_>>();
    let mut errors = manifest
        .errors
        .into_iter()
        .map(|error| ManifestError {
            path: error.path,
            message: error.message,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    errors.sort_by(|a, b| a.path.cmp(&b.path));
    ManifestBuild { entries, errors }
}

fn walk_dir(
    side: ManifestSide,
    label: &str,
    abs_dir: &str,
    rel_dir: &str,
    driver: &dyn VfsDriver,
    progress_tx: &mpsc::Sender<ManifestProgressEvent>,
    cancel: &AtomicBool,
    out: &mut Vec<ManifestEntry>,
    errors: &mut Vec<ManifestError>,
) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("Cancelled".to_string());
    }
    let children = match driver.read_dir_entries(abs_dir) {
        Ok(children) => children,
        Err(error) if rel_dir.is_empty() => {
            return Err(format!("Cannot read {label} directory {abs_dir}: {error}"));
        }
        Err(error) => {
            errors.push(ManifestError {
                path: rel_dir.to_string(),
                message: format!("Cannot read {label} directory {abs_dir}: {error}"),
            });
            return Ok(());
        }
    };
    for child in children {
        if cancel.load(Ordering::SeqCst) {
            return Err("Cancelled".to_string());
        }
        if rel_dir.is_empty() && child.name == SYNC_REPORT_NAME {
            continue;
        }
        let child_abs = driver.resolve_child(abs_dir, &child.name);
        let child_rel = join_rel(rel_dir, &child.name);
        if child.is_dir {
            walk_dir(
                side,
                label,
                &child_abs,
                &child_rel,
                driver,
                progress_tx,
                cancel,
                out,
                errors,
            )?;
        } else {
            match digest_file(driver, &child_abs, child.size) {
                Ok(digest) => out.push(ManifestEntry {
                    path: child_rel.clone(),
                    size: digest.size,
                    hash: digest.hash,
                }),
                Err(message) => errors.push(ManifestError {
                    path: child_rel.clone(),
                    message,
                }),
            }
            send_manifest_progress_sync(
                progress_tx,
                ManifestProgressEvent::FileHashed {
                    side,
                    path: child_rel,
                    processed: out.len() + errors.len(),
                },
            );
        }
    }
    Ok(())
}

fn digest_file(
    driver: &dyn VfsDriver,
    path: &str,
    size_hint: u64,
) -> Result<ManifestEntry, String> {
    if let Some(digest) = driver
        .file_digest(path)
        .map_err(|e| format!("Cannot digest {path}: {e}"))?
    {
        return Ok(ManifestEntry {
            path: String::new(),
            size: digest.size,
            hash: digest.hash,
        });
    }
    let size = driver.file_size(path).unwrap_or(size_hint);
    let mut hasher = blake3::Hasher::new();
    let mut offset = 0u64;
    while offset < size {
        let remaining = size.saturating_sub(offset).min(CHUNK_SIZE as u64) as usize;
        let chunk = driver
            .read_chunk(path, offset, remaining)
            .map_err(|e| format!("Cannot read {path}: {e}"))?;
        if chunk.is_empty() {
            break;
        }
        offset = offset.saturating_add(chunk.len() as u64);
        hasher.update(&chunk);
    }
    if offset != size {
        return Err(format!(
            "digest read ended at {offset} of {size} bytes for {path}"
        ));
    }
    Ok(ManifestEntry {
        path: String::new(),
        size,
        hash: hasher.finalize().to_hex().to_string(),
    })
}

fn compare_manifests(local: &[ManifestEntry], remote: &[ManifestEntry]) -> CompareSummary {
    let local_map: BTreeMap<&str, &ManifestEntry> = local
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let remote_map: BTreeMap<&str, &ManifestEntry> = remote
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let paths: BTreeSet<&str> = local_map.keys().chain(remote_map.keys()).copied().collect();
    let mut summary = CompareSummary::default();
    for path in paths {
        match (local_map.get(path), remote_map.get(path)) {
            (Some(local), Some(remote)) => {
                if local.size == remote.size && local.hash == remote.hash {
                    summary.matched += 1;
                } else {
                    summary.differing.push(DiffEntry {
                        rel_path: path.to_string(),
                        local_size: local.size,
                        remote_size: remote.size,
                        local_hash: local.hash.clone(),
                        remote_hash: remote.hash.clone(),
                    });
                }
            }
            (Some(_), None) => summary.local_only.push(path.to_string()),
            (None, Some(_)) => summary.remote_only.push(path.to_string()),
            (None, None) => {}
        }
    }
    summary
}

fn write_report(
    driver: &dyn VfsDriver,
    path: &str,
    local_root: &str,
    remote_root: &str,
    summary: &CompareSummary,
) -> Result<(), String> {
    let body = format_report(local_root, remote_root, summary);
    let mut writer = driver
        .open_write(path)
        .map_err(|e| format!("Cannot create report {path}: {e}"))?;
    use std::io::Write;
    writer
        .write_all(body.as_bytes())
        .map_err(|e| format!("Cannot write report {path}: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("Cannot flush report {path}: {e}"))?;
    Ok(())
}

fn format_report(local_root: &str, remote_root: &str, summary: &CompareSummary) -> String {
    let mut body = String::new();
    body.push_str("fbro Sync Bro report\n\n");
    body.push_str(&format!("Local:  {local_root}\n"));
    body.push_str(&format!("Remote: {remote_root}\n\n"));
    body.push_str(&format!("Matched: {}\n", summary.matched));
    body.push_str(&format!("Differing: {}\n", summary.differing.len()));
    body.push_str(&format!("Local-only: {}\n", summary.local_only.len()));
    body.push_str(&format!("Remote-only: {}\n", summary.remote_only.len()));
    body.push_str(&format!(
        "Hash errors: {}\n\n",
        summary.local_errors.len() + summary.remote_errors.len()
    ));
    if !summary.differing.is_empty() {
        body.push_str("Differing files:\n");
        for diff in &summary.differing {
            body.push_str(&format!(
                "- {} local={} remote={} local_hash={} remote_hash={}\n",
                diff.rel_path, diff.local_size, diff.remote_size, diff.local_hash, diff.remote_hash
            ));
        }
    }
    if !summary.local_only.is_empty() {
        body.push_str("\nLocal-only files:\n");
        for path in &summary.local_only {
            body.push_str(&format!("- {path}\n"));
        }
    }
    if !summary.remote_only.is_empty() {
        body.push_str("\nRemote-only files:\n");
        for path in &summary.remote_only {
            body.push_str(&format!("- {path}\n"));
        }
    }
    if !summary.local_errors.is_empty() {
        body.push_str("\nLocal hash errors:\n");
        for error in &summary.local_errors {
            body.push_str(&format!("- {}: {}\n", error.path, error.message));
        }
    }
    if !summary.remote_errors.is_empty() {
        body.push_str("\nRemote hash errors:\n");
        for error in &summary.remote_errors {
            body.push_str(&format!("- {}: {}\n", error.path, error.message));
        }
    }
    body
}

fn join_rel(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    }
}

async fn sync_bro_progress_aggregator(
    job_id: String,
    tx: mpsc::Sender<ServerMessage>,
    mut rx: mpsc::Receiver<ManifestProgressEvent>,
) {
    let mut local = ManifestSideProgress::default();
    let mut remote = ManifestSideProgress::default();
    while let Some(event) = rx.recv().await {
        apply_manifest_progress(&mut local, &mut remote, event);
        let combined = combined_manifest_progress(&local, &remote);
        let _ = send_progress(
            &tx,
            &job_id,
            FileOpProgressSchema {
                total_files: 0,
                completed_files: combined.completed_files,
                current_file: combined.current_file,
                status_text: Some(combined.status_text),
                current_file_bytes_total: 0,
                current_file_bytes_copied: 0,
                total_bytes_total: 0,
                total_bytes_copied: 0,
                current_file_percent: None,
                overall_percent: combined.overall_percent,
            },
        )
        .await;
    }
}

fn apply_manifest_progress(
    local: &mut ManifestSideProgress,
    remote: &mut ManifestSideProgress,
    event: ManifestProgressEvent,
) {
    match event {
        ManifestProgressEvent::Started(side) => {
            side_progress_mut(local, remote, side).started = true;
        }
        ManifestProgressEvent::RemoteManifestRequested(side) => {
            let progress = side_progress_mut(local, remote, side);
            progress.started = true;
            progress.remote_manifest = true;
        }
        ManifestProgressEvent::FileHashed {
            side,
            path,
            processed,
        } => {
            let progress = side_progress_mut(local, remote, side);
            progress.started = true;
            progress.remote_manifest = false;
            progress.processed = processed;
            progress.current_file = Some(path);
        }
        ManifestProgressEvent::Done { side, processed } => {
            let progress = side_progress_mut(local, remote, side);
            progress.started = true;
            progress.remote_manifest = false;
            progress.done = true;
            progress.processed = processed;
            progress.current_file = None;
        }
    }
}

fn side_progress_mut<'a>(
    local: &'a mut ManifestSideProgress,
    remote: &'a mut ManifestSideProgress,
    side: ManifestSide,
) -> &'a mut ManifestSideProgress {
    match side {
        ManifestSide::Local => local,
        ManifestSide::Remote => remote,
    }
}

fn combined_manifest_progress(
    local: &ManifestSideProgress,
    remote: &ManifestSideProgress,
) -> CombinedManifestProgress {
    let both_started = local.started && remote.started;
    let any_processed = local.processed > 0 || remote.processed > 0;
    let status_text = if local.done && remote.done {
        "Hashing complete".to_string()
    } else if !any_processed
        && both_started
        && !local.done
        && !remote.done
        && !remote.remote_manifest
    {
        "Hashing local and remote trees".to_string()
    } else {
        format!(
            "{}; {}",
            format_side_progress("local", local),
            format_side_progress("remote", remote)
        )
    };

    CombinedManifestProgress {
        status_text,
        current_file: local
            .current_file
            .clone()
            .or_else(|| remote.current_file.clone()),
        completed_files: local.processed.max(remote.processed),
        overall_percent: if local.done && remote.done {
            Some(100)
        } else {
            None
        },
    }
}

fn format_side_progress(label: &str, progress: &ManifestSideProgress) -> String {
    if !progress.started {
        return format!("{label} tree starting");
    }
    if progress.done {
        if progress.processed > 0 {
            return format!("{label} tree complete ({} files)", progress.processed);
        }
        return format!("{label} tree complete");
    }
    if progress.remote_manifest && label == "remote" {
        return "remote tree hashing on remote fbro".to_string();
    }
    if progress.processed > 0 {
        return format!("{label} tree hashing ({} files)", progress.processed);
    }
    format!("{label} tree hashing")
}

fn send_manifest_progress_sync(
    tx: &mpsc::Sender<ManifestProgressEvent>,
    event: ManifestProgressEvent,
) {
    let _ = tx.blocking_send(event);
}

async fn send_progress(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    progress: FileOpProgressSchema,
) -> Result<(), ()> {
    tx.send(ServerMessage::JobEvent {
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
    })
    .await
    .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64, hash: &str) -> ManifestEntry {
        ManifestEntry {
            path: path.to_string(),
            size,
            hash: hash.to_string(),
        }
    }

    #[test]
    fn compare_manifests_classifies_matches_and_differences() {
        let local = vec![
            entry("same.txt", 4, "aaaa"),
            entry("changed.txt", 4, "bbbb"),
        ];
        let remote = vec![
            entry("same.txt", 4, "aaaa"),
            entry("changed.txt", 4, "cccc"),
        ];

        let summary = compare_manifests(&local, &remote);

        assert_eq!(summary.matched, 1);
        assert_eq!(summary.differing.len(), 1);
        assert_eq!(summary.differing[0].rel_path, "changed.txt");
        assert!(summary.local_only.is_empty());
        assert!(summary.remote_only.is_empty());
    }

    #[test]
    fn compare_manifests_reports_missing_files_on_both_sides() {
        let local = vec![entry("local-only.txt", 4, "aaaa")];
        let remote = vec![entry("remote-only.txt", 4, "bbbb")];

        let summary = compare_manifests(&local, &remote);

        assert_eq!(summary.matched, 0);
        assert!(summary.differing.is_empty());
        assert_eq!(summary.local_only, vec!["local-only.txt"]);
        assert_eq!(summary.remote_only, vec!["remote-only.txt"]);
    }

    #[test]
    fn report_includes_hash_errors() {
        let mut summary = CompareSummary::default();
        summary.remote_errors.push(ManifestError {
            path: "some_file.mkv".to_string(),
            message: "Cannot digest remote/some_file.mkv: IOError: disconnected: Timeout"
                .to_string(),
        });

        let report = format_report("/local", "/remote", &summary);

        assert!(report.contains("Hash errors: 1"));
        assert!(report.contains("Remote hash errors:"));
        assert!(report.contains("some_file.mkv"));
        assert!(report.contains("Timeout"));
    }

    #[test]
    fn combined_progress_shows_both_trees_when_both_are_running() {
        let local = ManifestSideProgress {
            started: true,
            ..Default::default()
        };
        let remote = ManifestSideProgress {
            started: true,
            ..Default::default()
        };

        let combined = combined_manifest_progress(&local, &remote);

        assert_eq!(combined.status_text, "Hashing local and remote trees");
    }

    #[test]
    fn combined_progress_keeps_remote_manifest_visible_during_local_hashing() {
        let local = ManifestSideProgress {
            started: true,
            processed: 245,
            current_file: Some("movie.mkv".to_string()),
            ..Default::default()
        };
        let remote = ManifestSideProgress {
            started: true,
            remote_manifest: true,
            ..Default::default()
        };

        let combined = combined_manifest_progress(&local, &remote);

        assert_eq!(
            combined.status_text,
            "local tree hashing (245 files); remote tree hashing on remote fbro"
        );
        assert_eq!(combined.current_file.as_deref(), Some("movie.mkv"));
    }

    #[test]
    fn combined_progress_reports_one_side_complete_while_other_continues() {
        let local = ManifestSideProgress {
            started: true,
            processed: 912,
            ..Default::default()
        };
        let remote = ManifestSideProgress {
            started: true,
            done: true,
            processed: 904,
            ..Default::default()
        };

        let combined = combined_manifest_progress(&local, &remote);

        assert_eq!(
            combined.status_text,
            "local tree hashing (912 files); remote tree complete (904 files)"
        );
        assert_eq!(combined.overall_percent, None);
    }

    #[test]
    fn combined_progress_is_complete_only_after_both_sides_finish() {
        let local = ManifestSideProgress {
            started: true,
            done: true,
            processed: 10,
            ..Default::default()
        };
        let remote = ManifestSideProgress {
            started: true,
            done: true,
            processed: 10,
            ..Default::default()
        };

        let combined = combined_manifest_progress(&local, &remote);

        assert_eq!(combined.status_text, "Hashing complete");
        assert_eq!(combined.overall_percent, Some(100));
    }
}

async fn send_terminal(
    tx: &mpsc::Sender<ServerMessage>,
    job_id: &str,
    event: &str,
    error: Option<String>,
    result_path: Option<String>,
    result_panel_id: Option<String>,
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
        result_path,
        result_panel_id,
    })
    .await
    .map_err(|_| ())
}
