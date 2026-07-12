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

fn database_path() -> std::path::PathBuf {
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
            VfsEntry { name, kind: VfsKind::File, size: Some(event_detail(&row).len() as u64), mtime: Some((row.timestamp_ms / 1_000).to_string()), btime: None, permissions: None, icon_name: Some("text-x-generic".into()), cc: Some("cc-file".into()), source_path: Some(row.id) }
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
