#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Outgoing (BE → FE) ───────────────────────────────────────────────────────

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum ServerMessage {
    Welcome {
        timestamp: String,
        #[serde(rename = "protocolVersion")]
        protocol_version: String,
        #[serde(rename = "sessionId")]
        session_id: String,
    },
    StateSnapshot {
        timestamp: String,
        domains: StateDomains,
    },
    StatePatch {
        timestamp: String,
        domain: String,
        #[serde(rename = "entityId", skip_serializing_if = "Option::is_none")]
        entity_id: Option<String>,
        version: u64,
        patch: serde_json::Value,
    },
    JobEvent {
        timestamp: String,
        #[serde(rename = "jobId")]
        job_id: String,
        event: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        progress: Option<FileOpProgressSchema>,
        #[serde(skip_serializing_if = "Option::is_none")]
        decision: Option<ConflictDecisionSchema>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    JobStarted {
        timestamp: String,
        job: JobStartSchema,
    },
    Error {
        timestamp: String,
        code: String,
        message: String,
        recoverable: bool,
    },
    QuitConfirmRequired {
        timestamp: String,
        #[serde(rename = "activeJobs")]
        active_jobs: usize,
    },
}

#[derive(Serialize, Clone, Debug)]
pub struct StateDomains {
    pub layout: LayoutDomain,
    pub panels: HashMap<String, PanelSnapshot>,
    pub sources: Vec<SourceSchema>,
    pub focus: String,
    pub jobs: Vec<serde_json::Value>,
}

#[derive(Serialize, Clone, Debug)]
pub struct LayoutDomain {
    pub panels: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct PanelSnapshot {
    #[serde(rename = "panelId")]
    pub panel_id: String,
    pub path: String,
    pub status: String,
    #[serde(rename = "sourceId")]
    pub source_id: String,
    pub sort: SortSpec,
    pub filter: Option<FilterSpec>,
    pub cursor: usize,
    pub selection: Vec<usize>,
    pub entries: Vec<EntrySchema>,
    pub version: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SortSpec {
    pub by: String,
    pub order: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FilterSpec {
    pub mode: String,
    pub value: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct EntrySchema {
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtime: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct SourceSchema {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub supports: Vec<String>,
    pub capabilities: SourceCapabilities,
}

#[derive(Serialize, Clone, Debug)]
pub struct SourceCapabilities {
    #[serde(rename = "readOnly")]
    pub read_only: bool,
    #[serde(rename = "supportsStreaming")]
    pub supports_streaming: bool,
    #[serde(rename = "supportsJobs")]
    pub supports_jobs: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct JobProgress {
    pub current: u64,
    pub total: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileOpProgressSchema {
    pub total_files: usize,
    pub completed_files: usize,
    pub current_file: Option<String>,
    pub current_file_bytes_total: u64,
    pub current_file_bytes_copied: u64,
    pub total_bytes_total: u64,
    pub total_bytes_copied: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDecisionSchema {
    pub source: String,
    pub destination: String,
    pub options: Vec<String>,
    pub supports_apply_to_all: bool,
}

/// Summary sent when a job starts, so FE knows about the job.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JobStartSchema {
    pub job_id: String,
    pub op_type: String,
    pub files: Vec<FileOpFileSchema>,
    pub source_panel: String,
    pub source_path: String,
    pub dest_panel: Option<String>,
    pub dest_path: Option<String>,
    pub total_files: usize,
    pub total_bytes_total: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct FileOpFileSchema {
    pub name: String,
    pub size: Option<u64>,
    pub kind: String,
}

// ─── Incoming (FE → BE) ───────────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum ClientMessage {
    Hello {
        #[serde(rename = "protocolVersion")]
        protocol_version: String,
        client: ClientInfo,
    },
    Intent {
        name: String,
        #[serde(default)]
        scope: serde_json::Value,
        #[serde(default)]
        args: serde_json::Value,
        #[serde(default)]
        options: serde_json::Value,
    },
}

#[derive(Deserialize, Debug)]
pub struct ClientInfo {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub supports: Vec<String>,
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

pub fn now() -> String {
    // Simple RFC 3339 timestamp without pulling in chrono
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| format!("{}.{:03}Z", d.as_secs(), d.subsec_millis()))
        .unwrap_or_else(|_| "0".to_string())
}
