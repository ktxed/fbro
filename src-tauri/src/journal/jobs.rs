//! Remote job audit bookkeeping, separate from notification delivery policy.

use super::{RemoteKind, RemoteOperation, RemotePhase};
use crate::jobs::JobKind;
use crate::state::AppState;
use crate::ws::protocol::FileOpFileSchema;
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Default)]
pub struct RemoteJobTracker {
    jobs: HashMap<String, JobAudit>,
    transfers: HashMap<(String, String), RemoteOperation>,
    origins: HashMap<String, (String, Vec<Value>)>,
}

struct JobAudit {
    operation: RemoteOperation,
    files: Vec<Value>,
    result: Option<Value>,
    hash_indexes: HashMap<String, usize>,
}

fn ensure(state: &mut AppState, job_id: &str) -> bool {
    if state.remote_job_journals.jobs.contains_key(job_id) {
        return true;
    }
    let Some(job) = state.job_store.jobs.get(job_id) else {
        return false;
    };
    let (kind, peer, direction, identity) = match &job.kind {
        JobKind::OutgoingTransfer(meta) => (
            RemoteKind::Transfer,
            Some(meta.peer_fp.clone()),
            "outgoing",
            meta.transfer_key
                .starts_with("client:")
                .then(|| (meta.peer_fp.clone(), meta.transfer_key.clone())),
        ),
        JobKind::FileOp(_) if job.notification.remote_peer.is_some() => (
            RemoteKind::Transfer,
            job.notification.remote_peer.clone(),
            state
                .remote_job_journals
                .origins
                .get(job_id)
                .map(|origin| origin.0.as_str())
                .unwrap_or("initiating"),
            None,
        ),
        JobKind::RemoteManifest(meta) => (
            RemoteKind::Hash,
            Some(meta.peer_fp.clone()),
            "incoming",
            None,
        ),
        JobKind::SyncBro(_) => {
            let peer = job
                .dest_panel
                .as_ref()
                .and_then(|id| state.panels.get(id))
                .and_then(|panel| state.sources.get(&panel.source_id))
                .and_then(|source| source.peer_id.clone());
            (RemoteKind::Sync, peer, "initiating", None)
        }
        _ => return false,
    };
    let name = peer
        .as_deref()
        .and_then(|fp| state.share_config.paired_peers.get(fp))
        .map(|peer| peer.name.as_str());
    let operation = identity
        .as_ref()
        .and_then(|key| state.remote_job_journals.transfers.get(key))
        .cloned()
        .unwrap_or_else(|| {
            state
                .remote_journal
                .begin(kind, peer.as_deref(), name, direction)
        });
    if let Some(key) = identity {
        state
            .remote_job_journals
            .transfers
            .entry(key)
            .or_insert_with(|| operation.clone());
    }
    let files = state
        .remote_job_journals
        .origins
        .get(job_id)
        .map(|origin| origin.1.clone())
        .unwrap_or_else(|| match &job.kind {
            JobKind::FileOp(meta) => meta
                .files
                .iter()
                .map(|file| {
                    json!({
                        "name": file.name, "sourcePath": file.source_path, "size": file.size,
                        "kind": file.kind, "destName": file.dest_name,
                    })
                })
                .collect(),
            _ => Vec::new(),
        });
    state.remote_job_journals.jobs.insert(
        job_id.into(),
        JobAudit {
            operation,
            files,
            result: None,
            hash_indexes: HashMap::new(),
        },
    );
    true
}

fn details(state: &AppState, job_id: &str, error: Option<&str>) -> Value {
    let job = &state.job_store.jobs[job_id];
    let audit = &state.remote_job_journals.jobs[job_id];
    let p = &job.progress;
    let (source, side) = match &job.kind {
        JobKind::OutgoingTransfer(_) => (None, "serving"),
        JobKind::RemoteManifest(meta) => (Some(meta.root.as_str()), "serving"),
        _ => (Some(job.source_path.as_str()), "initiating"),
    };
    let mut files = audit.files.clone();
    let mut served_bytes = p.total_bytes_copied;
    if matches!(job.kind, JobKind::OutgoingTransfer(_)) {
        let mut history: Vec<_> = state
            .remote_job_journals
            .jobs
            .iter()
            .filter(|(id, previous)| {
                id.as_str() != job_id
                    && previous.operation.operation_id() == audit.operation.operation_id()
            })
            .collect();
        history.sort_by_key(|(id, _)| {
            id.strip_prefix("job-")
                .and_then(|id| id.parse::<u64>().ok())
                .unwrap_or(0)
        });
        files = Vec::new();
        for (id, previous) in history {
            if let Some(job) = state.job_store.jobs.get(id) {
                served_bytes = served_bytes.saturating_add(job.progress.total_bytes_copied);
            }
            merge_files(&mut files, previous.files.clone());
        }
        merge_files(&mut files, audit.files.clone());
    }
    let mut payload = json!({
        "jobId": job_id, "side": side, "sourcePath": source, "destPath": job.dest_path,
        "files": files, "result": audit.result, "error": error.or(job.error.as_deref()),
        "bytesHashed": audit.files.iter().filter_map(|file| file["bytesHashed"].as_u64()).sum::<u64>(),
        "progress": {
            "totalFiles": p.total_files, "completedFiles": p.completed_files,
            "totalBytesTotal": p.total_bytes_total, "totalBytesCopied": p.total_bytes_copied,
            "currentFile": p.current_file, "currentFileBytesTotal": p.current_file_bytes_total,
            "currentFileBytesCopied": p.current_file_bytes_copied,
        },
    });
    if matches!(job.kind, JobKind::OutgoingTransfer(_)) {
        payload["operationTotals"] = json!({"bytesServed": served_bytes});
    }
    if job.dest_path.is_none() {
        payload.as_object_mut().unwrap().remove("destPath");
    }
    if source.is_none() {
        payload.as_object_mut().unwrap().remove("sourcePath");
    }
    if let Some(source_id) = audit
        .files
        .first()
        .and_then(|file| file["location"]["sourceId"].as_str())
    {
        payload["sourceId"] = json!(source_id);
    }
    payload
}

fn merge_files(existing: &mut Vec<Value>, incoming: Vec<Value>) {
    let key = |file: &Value| {
        (
            file["sourcePath"]
                .as_str()
                .or_else(|| file["name"].as_str())
                .unwrap_or("")
                .to_string(),
            file["side"].as_str().unwrap_or("").to_string(),
        )
    };
    let mut indexes: HashMap<_, _> = existing
        .iter()
        .enumerate()
        .map(|(index, file)| (key(file), index))
        .collect();
    for mut file in incoming {
        let file_key = key(&file);
        if let Some(index) = indexes.get(&file_key) {
            let previous = &mut existing[*index];
            // Error/terminal metadata must not erase bytes already observed.
            for key in ["bytesHashed"] {
                if let Some(value) = previous[key].as_u64() {
                    if file[key].as_u64().unwrap_or(0) < value {
                        file[key] = json!(value);
                    }
                }
            }
            if file.get("error").is_some_and(|error| !error.is_null())
                && file["size"].as_u64().unwrap_or(0) == 0
            {
                if let Some(value) = previous["size"].as_u64() {
                    file["size"] = json!(value);
                }
            }
            *previous = file;
        } else {
            indexes.insert(file_key, existing.len());
            existing.push(file);
        }
    }
}

fn file_details(file: &FileOpFileSchema) -> Value {
    let mut value = serde_json::to_value(file).expect("file metadata serializes");
    if let Some(location) = &file.location {
        value["displayPath"] = json!(file.source_path);
        value["sourcePath"] = json!(location.path);
    }
    value
}

/// Capture the real launch drivers' provenance before panels can navigate.
pub(crate) fn prepare_transfer(
    state: &mut AppState,
    job_id: &str,
    direction: &str,
    files: &[FileOpFileSchema],
) {
    state.remote_job_journals.origins.insert(
        job_id.into(),
        (direction.into(), files.iter().map(file_details).collect()),
    );
}

pub(crate) fn start(state: &mut AppState, job_id: &str) {
    if !ensure(state, job_id) {
        return;
    }
    let operation = &state.remote_job_journals.jobs[job_id].operation;
    if !operation.record_with(RemotePhase::Started, || details(state, job_id, None)) {
        operation.record_with(RemotePhase::Resumed, || details(state, job_id, None));
    }
}

pub(crate) fn activity(state: &mut AppState, job_id: &str, files: Option<&[FileOpFileSchema]>) {
    if !ensure(state, job_id) {
        return;
    }
    if let Some(files) = files {
        // Serving file metadata is append-only during a job.
        let audit = state.remote_job_journals.jobs.get_mut(job_id).unwrap();
        let known = audit.files.len();
        audit
            .files
            .extend(files.iter().skip(known).map(file_details));
    }
    state.remote_job_journals.jobs[job_id]
        .operation
        .record_with(RemotePhase::Resumed, || details(state, job_id, None));
}

pub(crate) fn attach_result(
    state: &mut AppState,
    job_id: &str,
    result: Value,
    files: Option<Vec<Value>>,
) {
    if !ensure(state, job_id) {
        return;
    }
    let audit = state.remote_job_journals.jobs.get_mut(job_id).unwrap();
    audit.result = Some(result);
    if let Some(files) = files {
        merge_files(&mut audit.files, files);
    }
}

pub(crate) fn hashed_file(
    state: &mut AppState,
    job_id: &str,
    path: &str,
    size: u64,
    bytes_hashed: u64,
    error: Option<&str>,
) {
    if !ensure(state, job_id) {
        return;
    }
    let audit = state.remote_job_journals.jobs.get_mut(job_id).unwrap();
    let file =
        json!({ "sourcePath": path, "size": size, "bytesHashed": bytes_hashed, "error": error });
    if let Some(index) = audit.hash_indexes.get(path).copied() {
        let mut previous = vec![std::mem::take(&mut audit.files[index])];
        merge_files(&mut previous, vec![file]);
        audit.files[index] = previous.remove(0);
    } else {
        audit
            .hash_indexes
            .insert(path.to_string(), audit.files.len());
        audit.files.push(file);
    }
}

pub(crate) fn terminal(state: &mut AppState, job_id: &str, event: &str, error: Option<&str>) {
    if !matches!(event, "Completed" | "Cancelled" | "Failed") || !ensure(state, job_id) {
        return;
    }
    let job = &state.job_store.jobs[job_id];
    let phase = match event {
        "Cancelled" => RemotePhase::Cancelled,
        "Failed" => RemotePhase::Failed,
        _ if matches!(job.kind, JobKind::OutgoingTransfer(_)) => RemotePhase::Ended,
        _ => RemotePhase::Completed,
    };
    let operation = &state.remote_job_journals.jobs[job_id].operation;
    // A replaced SFTP session must not settle its still-running successor.
    if matches!(phase, RemotePhase::Ended)
        && state.remote_job_journals.jobs.iter().any(|(id, audit)| {
            id != job_id
                && audit.operation.operation_id() == operation.operation_id()
                && state
                    .job_store
                    .jobs
                    .get(id)
                    .is_some_and(|job| job.state == crate::jobs::JobState::Running)
        })
    {
        return;
    }
    operation.record_with(phase, || details(state, job_id, error));
}
