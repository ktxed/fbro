#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Outgoing (BE → FE) ───────────────────────────────────────────────────────

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum ServerMessage {
    Welcome {
        timestamp: u64,
        #[serde(rename = "protocolVersion")]
        protocol_version: String,
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "fileServerPort")]
        file_server_port: u16,
    },
    StateSnapshot {
        timestamp: u64,
        domains: StateDomains,
    },
    StatePatch {
        timestamp: u64,
        domain: String,
        #[serde(rename = "entityId", skip_serializing_if = "Option::is_none")]
        entity_id: Option<String>,
        version: u64,
        patch: serde_json::Value,
    },
    JobEvent {
        timestamp: u64,
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
        timestamp: u64,
        job: JobStartSchema,
    },
    Error {
        timestamp: u64,
        code: String,
        message: String,
        recoverable: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        extra: Option<serde_json::Value>,
    },
    QuitConfirmRequired {
        timestamp: u64,
        #[serde(rename = "activeJobs")]
        active_jobs: usize,
    },
    FileChunk {
        timestamp: u64,
        path: String,
        offset: u64,
        /// Hex-encoded bytes.
        data: String,
        eof: bool,
        #[serde(rename = "fileSize", skip_serializing_if = "Option::is_none")]
        file_size: Option<u64>,
    },
    RenameOk {
        timestamp: u64,
        #[serde(rename = "panelId")]
        panel_id: String,
    },
    MkdirOk {
        timestamp: u64,
        #[serde(rename = "panelId")]
        panel_id: String,
    },
    ViewerFileChanged {
        timestamp: u64,
        path: String,
        filename: String,
        #[serde(rename = "sourceId")]
        source_id: String,
        #[serde(rename = "panelId")]
        panel_id: String,
        #[serde(rename = "fileSize", skip_serializing_if = "Option::is_none")]
        file_size: Option<u64>,
        #[serde(rename = "isLocal")]
        is_local: bool,
    },
    DryRunReady {
        timestamp: u64,
        #[serde(rename = "scriptPath")]
        script_path: String,
        #[serde(rename = "scriptDir")]
        script_dir: String,
    },
}

#[derive(Serialize, Clone, Debug)]
pub struct StateDomains {
    pub layout: LayoutDomain,
    pub panels: HashMap<String, PanelSnapshot>,
    pub sources: Vec<SourceSchema>,
    pub focus: String,
    pub jobs: Vec<serde_json::Value>,
    pub commands: Vec<InteractiveCommandSchema>,
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
    #[serde(rename = "filterModeActive")]
    pub filter_mode_active: bool,
    pub cursor: usize,
    pub selection: Vec<usize>,
    #[serde(rename = "selectedEntries")]
    pub selected_entries: Vec<FileOpFileSchema>,
    pub entries: Vec<EntrySchema>,
    pub version: u64,
    #[serde(rename = "calculatingSize")]
    pub calculating_size: Vec<String>,
    #[serde(rename = "totalCount", skip_serializing_if = "Option::is_none")]
    pub total_count: Option<usize>,
    #[serde(rename = "windowStart", skip_serializing_if = "Option::is_none")]
    pub window_start: Option<usize>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SortSpec {
    pub by: String,
    pub order: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub btime: Option<String>, // birth time — file creation; not available on all platforms/filesystems
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "iconName")]
    pub icon_name: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct SourceSchema {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub supports: Vec<String>,
    pub capabilities: SourceCapabilities,
    #[serde(rename = "rootPath")]
    pub root_path: String,
    pub mounted: bool,
    #[serde(rename = "freeBytes", skip_serializing_if = "Option::is_none")]
    pub free_bytes: Option<u64>,
    #[serde(rename = "totalBytes", skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
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
#[serde(rename_all = "camelCase")]
pub struct KeyBindingSchema {
    pub key: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shift: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ctrl: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub alt: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub meta: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct InteractiveCommandSchema {
    pub id: String,
    pub category: String,
    pub title: String,
    pub bindings: Vec<KeyBindingSchema>,
    pub default_bindings: Vec<KeyBindingSchema>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_name: Option<String>,
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
    pub background: bool,
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

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
