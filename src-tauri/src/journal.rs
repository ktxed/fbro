//! Durable, append-only audit events and their read-only VFS projection.
//!
//! This intentionally records fbro's semantic commands and completed effects,
//! never websocket state/listing traffic.  The journal source itself is also
//! excluded by the caller so inspecting history cannot create history noise.

use std::any::Any;
use std::io::{Cursor, Read, Write};

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::vfs::{DirChild, VfsDriver, VfsEntry, VfsError, VfsKind};
use crate::ws::protocol::{FilterSpec, SortSpec};

pub const SOURCE_ID: &str = "journal";

pub(crate) fn database_path() -> std::path::PathBuf {
    crate::config::config_dir().join("journal.sqlite3")
}

fn open() -> Result<Connection, VfsError> {
    let path = database_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| VfsError::IOError(e.to_string()))?;
    }
    let conn = Connection::open(path).map_err(|e| VfsError::IOError(e.to_string()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS journal_events (
           event_id TEXT PRIMARY KEY,
           timestamp_ms INTEGER NOT NULL,
           operation_id TEXT,
           job_id TEXT,
           event_type TEXT NOT NULL,
           version INTEGER NOT NULL,
           source_id TEXT,
           outcome TEXT,
           payload_json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS journal_events_time ON journal_events(timestamp_ms DESC);
         CREATE INDEX IF NOT EXISTS journal_events_operation ON journal_events(operation_id);
         CREATE INDEX IF NOT EXISTS journal_events_type ON journal_events(event_type);",
    )
    .map_err(|e| VfsError::IOError(e.to_string()))?;
    Ok(conn)
}

pub fn record(
    event_type: &str,
    operation_id: Option<&str>,
    job_id: Option<&str>,
    source_id: Option<&str>,
    outcome: Option<&str>,
    payload: Value,
) {
    let Ok(conn) = open() else { return };
    let _ = conn.execute(
        "INSERT INTO journal_events
         (event_id, timestamp_ms, operation_id, job_id, event_type, version, source_id, outcome, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8)",
        params![
            uuid::Uuid::new_v4().to_string(),
            crate::ws::protocol::now() as i64,
            operation_id,
            job_id,
            event_type,
            source_id,
            outcome,
            payload.to_string(),
        ],
    );
}

pub fn record_command(name: &str, scope: &Value, args: &Value, source_id: Option<&str>) {
    record(
        "intent.command.v1",
        None,
        None,
        source_id,
        None,
        json!({ "command": name, "scope": scope, "args": args }),
    );
}

#[derive(Clone, Debug)]
pub struct JournalOperation {
    operation_id: String,
    command: String,
    kind: &'static str,
    source_id: Option<String>,
    context: Value,
}

pub fn new_operation(command: &str, source_id: Option<&str>) -> Option<JournalOperation> {
    new_operation_with_context(command, source_id, Value::Null)
}

fn new_operation_with_context(command: &str, source_id: Option<&str>, context: Value) -> Option<JournalOperation> {
    Some(JournalOperation {
        operation_id: uuid::Uuid::new_v4().to_string(),
        command: command.to_string(),
        kind: command_operation(command)?,
        source_id: source_id.map(str::to_string),
        context,
    })
}

fn record_correlated_intent(operation: &JournalOperation, scope: Value, args: &Value, context: Value) {
    let payload = correlated_intent_payload(operation, scope, args, context);
    record(
        "intent.command.v1",
        Some(&operation.operation_id),
        None,
        operation.source_id.as_deref(),
        None,
        payload,
    );
}

fn correlated_intent_payload(operation: &JournalOperation, scope: Value, args: &Value, context: Value) -> Value {
    let mut payload = json!({
        "operationId": operation.operation_id,
        "command": operation.command,
        "scope": scope,
        "args": args,
        "kind": operation.kind,
    });
    merge_payload_context(&mut payload, operation.context.clone());
    merge_payload_context(&mut payload, context);
    payload
}

fn record_correlated_result(operation: &JournalOperation, outcome: &str, error: Option<&str>, context: Value) {
    let payload = correlated_result_payload(operation, outcome, error, context);
    record(
        "operation.finished.v1",
        Some(&operation.operation_id),
        None,
        operation.source_id.as_deref(),
        Some(outcome),
        payload,
    );
}

fn correlated_result_payload(operation: &JournalOperation, outcome: &str, error: Option<&str>, context: Value) -> Value {
    let mut payload = json!({
        "operationId": operation.operation_id,
        "kind": operation.kind,
        "command": operation.command,
        "outcome": outcome,
        "error": error,
    });
    merge_payload_context(&mut payload, operation.context.clone());
    merge_payload_context(&mut payload, context);
    payload
}

fn merge_payload_context(payload: &mut Value, context: Value) {
    let (Some(payload), Value::Object(context)) = (payload.as_object_mut(), context) else { return };
    payload.extend(context);
}

pub fn record_git_intent(
    command: &str,
    args: &Value,
    repository: Option<&str>,
    source_id: Option<&str>,
) -> Option<JournalOperation> {
    let operation = new_operation_with_context(command, source_id, json!({ "repository": repository }))?;
    record_correlated_intent(&operation, Value::Null, args, Value::Null);
    Some(operation)
}

pub fn record_git_result(operation: &JournalOperation, outcome: &str, error: Option<&str>) {
    record_correlated_result(operation, outcome, error, Value::Null);
}

pub fn record_location_sort_reset_intent(cached_count: usize) -> JournalOperation {
    let operation = new_operation_with_context(
        "ResetLocationSortSettings",
        None,
        json!({ "cachedCount": cached_count }),
    )
    .expect("ResetLocationSortSettings is a journal operation");
    record_correlated_intent(&operation, Value::Null, &Value::Null, Value::Null);
    operation
}

pub fn record_location_sort_reset_result(
    operation: &JournalOperation,
    outcome: &str,
    removed_count: usize,
    error: Option<&str>,
) {
    record_correlated_result(
        operation,
        outcome,
        error,
        json!({ "removedCount": removed_count }),
    );
}

pub fn record_location_sort_expiry(
    outcome: &str,
    removed_count: usize,
    error: Option<&str>,
) {
    record(
        "operation.finished.v1",
        None,
        None,
        None,
        Some(outcome),
        json!({
            "kind": "expire-location-sort-settings",
            "outcome": outcome,
            "removedCount": removed_count,
            "retentionDays": 30,
            "error": error,
        }),
    );
}

pub fn record_quick_edit_intent(
    operation: &JournalOperation,
    panel_id: &str,
    path: Option<&str>,
    requested_editor: Option<&str>,
    editor_candidates: &[&str],
) {
    record_correlated_intent(operation, Value::String(panel_id.to_string()), &Value::Null, json!({
        "path": path,
        "requestedEditor": requested_editor,
        "editorCandidates": editor_candidates,
    }));
}

#[allow(clippy::too_many_arguments)]
pub fn record_quick_edit_result(
    operation: &JournalOperation,
    path: Option<&str>,
    editor: Option<&str>,
    session_id: Option<u32>,
    outcome: &str,
    error: Option<&str>,
    changed: Option<bool>,
    exit_code: Option<u32>,
    signal: Option<&str>,
) {
    record_correlated_result(operation, outcome, error, json!({
        "path": path,
        "editor": editor,
        "terminalSessionId": session_id,
        "changed": changed,
        "exitCode": exit_code,
        "signal": signal,
    }));
}

pub fn record_mutation(job_id: &str, kind: &str, source: &str, destination: Option<&str>) {
    record(
        "mutation.entry.v1",
        Some(job_id),
        Some(job_id),
        None,
        Some("completed"),
        json!({ "kind": kind, "source": source, "destination": destination, "undoEligible": false }),
    );
}

pub fn record_operation_finished(job_id: &str, kind: &str, outcome: &str, error: Option<&str>) {
    record(
        "operation.finished.v1",
        Some(job_id),
        Some(job_id),
        None,
        Some(outcome),
        json!({ "kind": kind, "outcome": outcome, "error": error }),
    );
}

#[derive(Debug)]
struct EventRow {
    id: String,
    timestamp_ms: i64,
    event_type: String,
    outcome: Option<String>,
    payload: String,
}

fn event_name(row: &EventRow) -> String {
    let payload = serde_json::from_str::<Value>(&row.payload).unwrap_or(Value::Null);
    let operation = payload
        .get("kind")
        .and_then(Value::as_str)
        .or_else(|| payload.get("args").and_then(|args| args.get("opType")).and_then(Value::as_str))
        .or_else(|| payload.get("command").and_then(Value::as_str).and_then(command_operation))
        .unwrap_or("operation");
    let event = match row.event_type.as_str() {
        "intent.command.v1" => "intent",
        "mutation.entry.v1" => "mutation",
        "operation.finished.v1" => row.outcome.as_deref().unwrap_or("finished"),
        _ => "event",
    };
    format!("{operation}_v1_{event}_{}.log", row.id)
}

fn command_operation(command: &str) -> Option<&'static str> {
    match command {
        "RenameFile" => Some("rename"),
        "MakeDir" => Some("mkdir"),
        "TouchFile" => Some("touch"),
        "SetPermissions" => Some("permissions"),
        "WriteExternalClipboard" => Some("clipboard-copy"),
        "PasteExternalClipboard" => Some("clipboard-paste"),
        "ViewFile" => Some("view"),
        "EditFile" => Some("edit"),
        "OpenFile" => Some("open"),
        "LaunchBro" => Some("bro"),
        "KillProcess" => Some("kill-process"),
        "OpenTerminal" => Some("terminal"),
        "QuickEditFile" => Some("quick-edit"),
        "ResetLocationSortSettings" => Some("reset-location-sort-settings"),
        "GitFetch" => Some("git-fetch"),
        "GitStagePaths" => Some("git-stage"),
        "GitUnstagePaths" => Some("git-unstage"),
        "GitRollbackPaths" => Some("git-rollback"),
        "GitCommit" => Some("git-commit"),
        "GitCreateBranch" => Some("git-create-branch"),
        "GitSwitchBranch" => Some("git-switch-branch"),
        "GitPull" => Some("git-pull"),
        "GitPush" => Some("git-push"),
        "GitSetUpstream" => Some("git-set-upstream"),
        "GitViewDiff" => Some("git-view-diff"),
        _ => None,
    }
}

fn event_detail(row: &EventRow) -> String {
    let pretty_payload = serde_json::from_str::<Value>(&row.payload)
        .and_then(|v| serde_json::to_string_pretty(&v))
        .unwrap_or_else(|_| row.payload.clone());
    format!(
        "fbro journal event\n\nID:       {}\nTimestamp: {}\nType:     {}\nOutcome:  {}\n\nPayload:\n{}\n",
        row.id,
        row.timestamp_ms,
        row.event_type,
        row.outcome.as_deref().unwrap_or("—"),
        pretty_payload
    )
}

fn query_event(id: &str) -> Result<EventRow, VfsError> {
    let conn = open()?;
    conn.query_row(
        "SELECT event_id, timestamp_ms, event_type, outcome, payload_json FROM journal_events WHERE event_id = ?1",
        [id],
        |r| Ok(EventRow { id: r.get(0)?, timestamp_ms: r.get(1)?, event_type: r.get(2)?, outcome: r.get(3)?, payload: r.get(4)? }),
    )
    .map_err(|e| VfsError::NotFound(e.to_string()))
}

pub struct JournalDriver;

impl VfsDriver for JournalDriver {
    fn as_any(&self) -> &dyn Any { self }

    fn list(&self, _path: &str, sort: &SortSpec, filter: Option<&FilterSpec>, _show_hidden: bool) -> Result<Vec<VfsEntry>, VfsError> {
        let conn = open()?;
        let mut stmt = conn.prepare("SELECT event_id, timestamp_ms, event_type, outcome, payload_json FROM journal_events ORDER BY timestamp_ms DESC")
            .map_err(|e| VfsError::IOError(e.to_string()))?;
        let rows = stmt.query_map([], |r| Ok(EventRow { id: r.get(0)?, timestamp_ms: r.get(1)?, event_type: r.get(2)?, outcome: r.get(3)?, payload: r.get(4)? }))
            .map_err(|e| VfsError::IOError(e.to_string()))?;
        let needle = filter.map(|f| f.value.to_lowercase());
        let mut entries = rows.filter_map(Result::ok).filter(|row| {
            needle.as_ref().map_or(true, |n| event_name(row).to_lowercase().contains(n) || row.payload.to_lowercase().contains(n))
        }).map(|row| {
            let name = event_name(&row);
            // fbro's shared VFS wire format uses Unix seconds as a string;
            // the panel formats it with `new Date(seconds * 1000)`.
            VfsEntry { name, kind: VfsKind::File, target_kind: None, path_hint: None, size: Some(event_detail(&row).len() as u64), mtime: Some((row.timestamp_ms / 1_000).to_string()), btime: None, permissions: None, icon_name: Some("text-x-generic".into()), cc: Some("cc-file".into()), source_path: Some(row.id) }
        }).collect::<Vec<_>>();
        if sort.by == "name" && sort.order == "asc" { entries.sort_by(|a, b| a.name.cmp(&b.name)); }
        if sort.by == "name" && sort.order == "desc" { entries.sort_by(|a, b| b.name.cmp(&a.name)); }
        Ok(entries)
    }

    fn resolve_child(&self, _base: &str, name: &str) -> String {
        let without_extension = name.strip_suffix(".log").unwrap_or(name);
        let candidate = without_extension.rsplit('_').next().unwrap_or(without_extension);
        if uuid::Uuid::parse_str(candidate).is_ok() {
            candidate.to_string()
        } else {
            // Compatibility with journal entries rendered before semantic names.
            name.split("__").next().unwrap_or(name).to_string()
        }
    }
    fn parent_path(&self, _path: &str) -> Option<String> { None }
    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> { Ok(Box::new(Cursor::new(event_detail(&query_event(path)?).into_bytes()))) }
    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> { Err(VfsError::NotSupported("journal source is read-only".into())) }
    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> { Err(VfsError::NotSupported("journal source is read-only".into())) }
    fn remove_file(&self, _path: &str) -> Result<(), VfsError> { Err(VfsError::NotSupported("journal source is read-only".into())) }
    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> { Err(VfsError::NotSupported("journal source is read-only".into())) }
    fn exists(&self, path: &str) -> bool { query_event(path).is_ok() }
    fn file_size(&self, path: &str) -> Option<u64> { query_event(path).ok().map(|r| event_detail(&r).len() as u64) }
    fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> { Ok(vec![]) }
    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> { let bytes = event_detail(&query_event(path)?).into_bytes(); let start = (offset as usize).min(bytes.len()); let end = (start + size).min(bytes.len()); Ok(bytes[start..end].to_vec()) }
}

#[cfg(test)]
mod tests {
    use super::{command_operation, correlated_intent_payload, correlated_result_payload, event_name, EventRow, JournalOperation};
    use serde_json::json;

    #[test]
    fn maps_semantic_git_operations_without_refresh_noise() {
        let expected = [
            ("GitFetch", "git-fetch"),
            ("GitStagePaths", "git-stage"),
            ("GitUnstagePaths", "git-unstage"),
            ("GitRollbackPaths", "git-rollback"),
            ("GitCommit", "git-commit"),
            ("GitCreateBranch", "git-create-branch"),
            ("GitSwitchBranch", "git-switch-branch"),
            ("GitPull", "git-pull"),
            ("GitPush", "git-push"),
            ("GitSetUpstream", "git-set-upstream"),
            ("GitViewDiff", "git-view-diff"),
            ("QuickEditFile", "quick-edit"),
            ("ResetLocationSortSettings", "reset-location-sort-settings"),
        ];
        for (command, operation) in expected {
            assert_eq!(command_operation(command), Some(operation));
        }
        assert_eq!(command_operation("GitRefresh"), None);
    }

    #[test]
    fn gives_existing_git_intents_semantic_names() {
        let row = EventRow {
            id: "event-id".into(),
            timestamp_ms: 0,
            event_type: "intent.command.v1".into(),
            outcome: None,
            payload: r#"{"command":"GitStagePaths","args":{"paths":["file.txt"]}}"#.into(),
        };
        assert_eq!(event_name(&row), "git-stage_v1_intent_event-id.log");
    }

    #[test]
    fn names_git_results_by_outcome() {
        let row = EventRow {
            id: "event-id".into(),
            timestamp_ms: 0,
            event_type: "operation.finished.v1".into(),
            outcome: Some("failed".into()),
            payload: r#"{"kind":"git-push","outcome":"failed"}"#.into(),
        };
        assert_eq!(event_name(&row), "git-push_v1_failed_event-id.log");
    }

    #[test]
    fn correlates_git_intent_and_result_payloads() {
        let operation = JournalOperation {
            operation_id: "operation-id".into(),
            command: "GitStagePaths".into(),
            kind: "git-stage",
            source_id: Some("filesystem".into()),
            context: json!({ "repository": "/repo" }),
        };
        let intent = correlated_intent_payload(&operation, serde_json::Value::Null, &json!({ "paths": ["file.txt"] }), serde_json::Value::Null);
        let result = correlated_result_payload(&operation, "failed", Some("failure"), serde_json::Value::Null);
        assert_eq!(intent["operationId"], result["operationId"]);
        assert_eq!(intent["repository"], "/repo");
        assert_eq!(result["outcome"], "failed");
        assert_eq!(result["error"], "failure");
    }

    #[test]
    fn builds_correlated_quick_edit_payloads() {
        let operation = JournalOperation {
            operation_id: "quick-edit-id".into(),
            command: "QuickEditFile".into(),
            kind: "quick-edit",
            source_id: Some("filesystem".into()),
            context: serde_json::Value::Null,
        };
        let intent = correlated_intent_payload(
            &operation,
            json!("left"),
            &serde_json::Value::Null,
            json!({ "path": "/repo/file.txt", "requestedEditor": "nano" }),
        );
        let result = correlated_result_payload(
            &operation,
            "completed",
            None,
            json!({ "path": "/repo/file.txt", "editor": "nano", "terminalSessionId": 7, "changed": true }),
        );
        assert_eq!(intent["operationId"], result["operationId"]);
        assert_eq!(intent["kind"], "quick-edit");
        assert_eq!(result["terminalSessionId"], 7);
        assert_eq!(result["changed"], true);
    }
}
