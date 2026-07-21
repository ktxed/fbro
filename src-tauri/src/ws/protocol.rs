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
        files: Option<Vec<FileOpFileSchema>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        progress: Option<FileOpProgressSchema>,
        #[serde(rename = "searchProgress", skip_serializing_if = "Option::is_none")]
        search_progress: Option<SearchProgressSchema>,
        #[serde(skip_serializing_if = "Option::is_none")]
        decision: Option<ConflictDecisionSchema>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(rename = "resultPath", skip_serializing_if = "Option::is_none")]
        result_path: Option<String>,
        #[serde(rename = "resultPanelId", skip_serializing_if = "Option::is_none")]
        result_panel_id: Option<String>,
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
    TouchFileOk {
        timestamp: u64,
        #[serde(rename = "panelId")]
        panel_id: String,
    },
    SetPermissionsOk {
        timestamp: u64,
        #[serde(rename = "panelId")]
        panel_id: String,
    },
    NavigateOk {
        timestamp: u64,
        #[serde(rename = "panelId")]
        panel_id: String,
    },
    SyncBroResult {
        timestamp: u64,
        #[serde(rename = "jobId")]
        job_id: String,
        result: SyncBroResultSchema,
    },
    FedupBroResult {
        timestamp: u64,
        #[serde(rename = "jobId")]
        job_id: String,
        result: FedupBroResultPageSchema,
    },
    FedupBroResultPage {
        timestamp: u64,
        #[serde(rename = "jobId")]
        job_id: String,
        result: FedupBroResultPageSchema,
    },
    FedupReportSaved {
        timestamp: u64,
        #[serde(rename = "jobId")]
        job_id: String,
        path: String,
    },
    FedupDeleteDryRunReady {
        timestamp: u64,
        #[serde(rename = "jobId")]
        job_id: String,
        #[serde(rename = "scriptPath")]
        script_path: String,
        #[serde(rename = "manifestPath")]
        manifest_path: String,
        #[serde(rename = "scriptDir")]
        script_dir: String,
    },
    SyncReportSaved {
        timestamp: u64,
        #[serde(rename = "jobId")]
        job_id: String,
        path: String,
    },
    ExternalClipboardReady {
        timestamp: u64,
        #[serde(rename = "panelId")]
        panel_id: String,
        #[serde(rename = "opType")]
        op_type: String,
        files: Vec<FileOpFileSchema>,
        #[serde(rename = "destPath")]
        dest_path: String,
    },
    ExternalClipboardWritten {
        timestamp: u64,
        #[serde(rename = "opType")]
        op_type: String,
        #[serde(rename = "itemCount")]
        item_count: usize,
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
        #[serde(rename = "isBinary", skip_serializing_if = "Option::is_none")]
        is_binary: Option<bool>,
    },
    DryRunReady {
        timestamp: u64,
        #[serde(rename = "scriptPath")]
        script_path: String,
        #[serde(rename = "scriptDir")]
        script_dir: String,
    },
    OpenWithOptionsReady {
        timestamp: u64,
        #[serde(rename = "requestId")]
        request_id: String,
        apps: Vec<OpenWithAppSchema>,
    },
    OpenWithOptionsFailed {
        timestamp: u64,
        #[serde(rename = "requestId")]
        request_id: String,
        message: String,
    },
    BroOptionsReady {
        timestamp: u64,
        #[serde(rename = "requestId")]
        request_id: String,
        inputs: Vec<BroInputSchema>,
        bros: Vec<BroSchema>,
    },
    BroOptionsFailed {
        timestamp: u64,
        #[serde(rename = "requestId")]
        request_id: String,
        message: String,
    },
    /// Request text input from the FE (e.g. archive password).
    InputRequired {
        timestamp: u64,
        #[serde(rename = "panelId")]
        panel_id: String,
        #[serde(rename = "requestId")]
        request_id: String,
        /// The kind of input: "password", "text", etc.
        kind: String,
        prompt: String,
    },
    CacheStatusReady {
        timestamp: u64,
        #[serde(rename = "prefetchCache")]
        prefetch_cache: PrefetchCacheSchema,
    },
    TerminalSessionOpened {
        timestamp: u64,
        #[serde(rename = "panelId")]
        panel_id: String,
        #[serde(rename = "sessionId")]
        session_id: u32,
    },
    TerminalClosed {
        timestamp: u64,
        #[serde(rename = "sessionId")]
        session_id: u32,
        #[serde(rename = "panelId")]
        panel_id: String,
    },
    /// Marker variant — never serialized as JSON. The WS sink intercepts it
    /// and emits a binary WebSocket frame: [0x01, sid_le_u32, data...].
    #[serde(skip)]
    TerminalOutput {
        session_id: u32,
        data: Vec<u8>,
    },
    /// A paired-handshake is in progress and needs the user to compare the
    /// 6-digit `code` against the other device. Fires on both peers
    /// simultaneously. The FE shows a modal; the user clicks Confirm/Cancel,
    /// driving a `ConfirmPairing` intent.
    PairingRequested {
        timestamp: u64,
        #[serde(rename = "handshakeId")]
        handshake_id: String,
        /// Zero-padded 6-digit string (e.g. "047392").
        code: String,
        /// Self-reported machine name of the other side.
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        os: Option<String>,
        #[serde(rename = "pubkeyAlgo")]
        pubkey_algo: String,
        /// Unix millis when the handshake auto-aborts if not confirmed.
        #[serde(rename = "expiresAt")]
        expires_at: u64,
    },
    /// A pairing handshake has terminated. Closes the modal on both sides.
    PairingResolved {
        timestamp: u64,
        #[serde(rename = "handshakeId")]
        handshake_id: String,
        /// One of: `"paired"`, `"rejected"`, `"timeout"`.
        outcome: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        fingerprint: Option<String>,
    },
    /// A remote SFTP session was established. Used by the FE to render an
    /// "active connections" indicator in Settings → Sharing.
    RemoteSessionStarted {
        timestamp: u64,
        fingerprint: String,
        #[serde(rename = "sourceId")]
        source_id: String,
    },
    /// A remote SFTP session ended (clean disconnect, revoke, or error).
    RemoteSessionEnded {
        timestamp: u64,
        fingerprint: String,
        #[serde(rename = "sourceId")]
        source_id: String,
    },
    /// Live status of a mounted remote source. Emitted by the SFTP driver
    /// supervisor on every transition (initial connect, heartbeat
    /// failure, reconnect attempt, success). The FE uses this to render
    /// a "Reconnecting…" spinner on the volume-bar button and a panel
    /// overlay banner.
    RemoteSourceStatus {
        timestamp: u64,
        #[serde(rename = "sourceId")]
        source_id: String,
        /// One of: `"connected"`, `"reconnecting"`, `"disconnected"`,
        /// `"dead"`.
        status: String,
        /// Human-readable reason. Set when `status != "connected"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// Unix millis of the next scheduled reconnect attempt. Set
        /// while `status == "reconnecting"` so the FE can show a
        /// countdown.
        #[serde(rename = "nextRetryAt", skip_serializing_if = "Option::is_none")]
        next_retry_at: Option<u64>,
    },
    CompareDiffReady {
        timestamp: u64,
        #[serde(rename = "requestId")]
        request_id: String,
        result: CompareDiffResultSchema,
    },
    CompareDiffFailed {
        timestamp: u64,
        #[serde(rename = "requestId")]
        request_id: String,
        message: String,
    },
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CompareDiffResultSchema {
    pub left: CompareTargetSchema,
    pub right: CompareTargetSchema,
    pub rows: Vec<CompareDiffRowSchema>,
    pub unified: String,
    pub identical: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CompareTargetSchema {
    pub panel_id: String,
    pub source_id: String,
    pub path: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CompareDiffRowSchema {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub right_text: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct PrefetchCacheSchema {
    #[serde(rename = "entryCount")]
    pub entry_count: usize,
    #[serde(rename = "totalBytes")]
    pub total_bytes: usize,
    #[serde(rename = "maxEntries")]
    pub max_entries: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct StateDomains {
    pub layout: LayoutDomain,
    pub panels: HashMap<String, PanelSnapshot>,
    /// Slots keyed by slotId. Each slot owns one or more panel ids (tabs).
    pub slots: HashMap<String, PanelSlotSchema>,
    pub sources: Vec<SourceSchema>,
    pub focus: String,
    pub jobs: Vec<serde_json::Value>,
    #[serde(rename = "jobQueue")]
    pub job_queue: JobQueueSchema,
    #[serde(rename = "dirSizeQueue")]
    pub dir_size_queue: DirSizeQueueSnapshot,
    pub commands: Vec<InteractiveCommandSchema>,
    pub bookmarks: Vec<crate::bookmarks::BookmarkEntry>,
    #[serde(rename = "fontSize")]
    pub font_size: u8,
    pub settings: SettingsSnapshot,
    #[serde(rename = "discoveredPeers")]
    pub discovered_peers: Vec<crate::remote::discovery::DiscoveredPeerSchema>,
    #[serde(rename = "shareConfig")]
    pub share_config: serde_json::Value,
    #[serde(rename = "globalSelections")]
    pub global_selections: Vec<crate::vfs::global_selection::GlobalSelectionSummary>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JobQueueSchema {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_job_id: Option<String>,
    pub job_ids: Vec<String>,
    pub completed_count: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct SettingsSnapshot {
    pub general: GeneralSettingsSchema,
    #[serde(rename = "modalSizes")]
    pub modal_sizes: std::collections::HashMap<String, ModalSizePreferenceSchema>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ModalSizePreferenceSchema {
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GeneralSettingsSchema {
    pub show_hidden_default: bool,
    pub remember_window_state: bool,
    pub preserve_attributes: bool,
    pub auto_resume_copies: bool,
    pub ludicrous_mode: bool,
    pub max_image_size_mb: u64,
    pub save_panel_view_mode: bool,
    pub enable_video_thumbnails: bool,
    pub thumbnail_max_image_size_mb: u64,
    pub thumbnail_video_frame_skip: u32,
    pub stream_thumbnails: bool,
    pub session_path: String,
    pub bookmarks_path: String,
    pub thumbnail_cache_dir: String,
    pub thumbnail_cache_item_count: u64,
    pub thumbnail_cache_size_bytes: u64,
    pub persist_panel_tabs: bool,
    pub proc_tree_mode: bool,
    pub color_scheme: String,
    pub sevenzip_extractor: String,
    pub sevenzip_custom_path: String,
    /// Effective binary picked given the current mode + custom path. Empty
    /// string means "fall back to internal extractor". Computed by the backend
    /// so the settings UI can show "Found at <path>" / "Not found" without
    /// re-running probe logic in the FE.
    pub sevenzip_resolved_path: String,
    pub fedup_default_ignore_patterns: String,
    pub compare_max_file_size_mb: u64,
    pub compare_differ_mode: String,
    pub compare_external_command: String,
    pub quick_terminal_editor: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct DirSizeQueueSnapshot {
    pub tasks: Vec<DirSizeTaskSchema>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DirSizeTaskSchema {
    #[serde(rename = "taskId")]
    pub task_id: u64,
    pub panel_id: String,
    pub entry_name: String,
    pub full_path: String,
    pub enqueued_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_progress_at: Option<u64>,
    pub status: String,
    pub dirs_scanned: u64,
    pub files_scanned: u64,
    pub bytes_scanned: u64,
    pub errors: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_action: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct LayoutDomain {
    /// Active panel id per slot, in slot order. Maintained for back-compat —
    /// FE will switch to reading `slots` once tabbed mode lands.
    pub panels: Vec<String>,
    /// Ordered slot ids — drives the visible layout left-to-right.
    pub slots: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PanelSlotSchema {
    pub slot_id: String,
    pub tab_panel_ids: Vec<String>,
    pub active_panel_id: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct PanelSnapshot {
    #[serde(rename = "panelId")]
    pub panel_id: String,
    #[serde(rename = "slotId")]
    pub slot_id: String,
    #[serde(rename = "tabKind")]
    pub tab_kind: String,
    #[serde(rename = "tabTitle", skip_serializing_if = "Option::is_none")]
    pub tab_title: Option<String>,
    pub path: String,
    pub status: String,
    #[serde(rename = "sourceId")]
    pub source_id: String,
    pub sort: SortSpec,
    pub filter: Option<FilterSpec>,
    #[serde(rename = "filterModeActive")]
    pub filter_mode_active: bool,
    #[serde(rename = "showHidden")]
    pub show_hidden: bool,
    #[serde(rename = "viewMode")]
    pub view_mode: String,
    #[serde(rename = "supportsThumbnails")]
    pub supports_thumbnails: bool,
    pub cursor: usize,
    pub selection: Vec<usize>,
    #[serde(rename = "selectedEntries")]
    pub selected_entries: Vec<FileOpFileSchema>,
    pub entries: Vec<EntrySchema>,
    pub version: u64,
    #[serde(rename = "panelEpoch")]
    pub panel_epoch: u64,
    #[serde(rename = "calculatingSize")]
    pub calculating_size: Vec<String>,
    #[serde(rename = "totalCount", skip_serializing_if = "Option::is_none")]
    pub total_count: Option<usize>,
    #[serde(rename = "windowStart", skip_serializing_if = "Option::is_none")]
    pub window_start: Option<usize>,
    #[serde(rename = "shadowSource", skip_serializing_if = "Option::is_none")]
    pub shadow_source: Option<ShadowSourceSchema>,
    #[serde(rename = "terminalSessionId", skip_serializing_if = "Option::is_none")]
    pub terminal_session_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<crate::git::GitRepoStatus>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ShadowSourceSchema {
    pub parent_source_id: String,
    pub parent_path: String,
    pub archive_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_hint: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    /// Real absolute path for entries whose logical name differs from the
    /// filesystem path (e.g. search-result entries). Used by the FE to build
    /// correct drag-out paths without a BE round-trip.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "sourcePath")]
    pub source_path: Option<String>,
    #[serde(rename = "gitStatus", skip_serializing_if = "Option::is_none")]
    pub git_status: Option<String>,
    #[serde(rename = "gitDetails", skip_serializing_if = "Option::is_none")]
    pub git_details: Option<crate::git::GitEntryDetails>,
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shadow: bool,
    #[serde(rename = "iconName", skip_serializing_if = "Option::is_none")]
    pub icon_name: Option<String>,
    /// Only set for `kind == "remote"` sources: name/fingerprint of the
    /// paired peer that owns this source. Drives the volume-bar label and
    /// the read-only badge tooltip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer: Option<SourcePeerSchema>,
}

#[derive(Serialize, Clone, Debug)]
pub struct SourcePeerSchema {
    pub name: String,
    pub fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
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
    /// When non-empty, the binding only matches if the focused panel's
    /// viewMode is in this list. Empty = applies in all modes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<String>,
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
#[serde(rename_all = "camelCase")]
pub struct OpenWithAppSchema {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub is_default: bool,
    pub source: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BroInputSchema {
    pub path: String,
    pub name: String,
    pub kind: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BroOutputSchema {
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BroPreSchema {
    pub score: i32,
    pub reason: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BroParamSchema {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_value: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_value: Option<i32>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BroSchema {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub protocol: String,
    pub pre: BroPreSchema,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<BroOutputSchema>,
    #[serde(default)]
    pub params: Vec<BroParamSchema>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status_text: Option<String>,
    pub current_file_bytes_total: u64,
    pub current_file_bytes_copied: u64,
    pub total_bytes_total: u64,
    pub total_bytes_copied: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_file_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overall_percent: Option<u8>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDecisionSchema {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub source: String,
    pub destination: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub options: Vec<String>,
    pub supports_apply_to_all: bool,
}

/// Summary sent when a job starts, so FE knows about the job.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct JobStartSchema {
    pub job_id: String,
    pub kind: String,
    pub op_type: String,
    pub files: Vec<FileOpFileSchema>,
    pub source_panel: String,
    pub source_path: String,
    pub dest_panel: Option<String>,
    pub dest_path: Option<String>,
    pub initial_state: String,
    pub total_files: usize,
    pub total_bytes_total: u64,
    pub background: bool,
    pub supports_pause: bool,
    pub result_path: Option<String>,
    pub result_panel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(rename = "searchSourceId", skip_serializing_if = "Option::is_none")]
    pub search_source_id: Option<String>,
    #[serde(rename = "searchParams", skip_serializing_if = "Option::is_none")]
    pub search_params: Option<SearchParamsSchema>,
}

// ─── Search schemas ───────────────────────────────────────────────────────────

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchParamsSchema {
    pub paths: Vec<String>,
    pub depth: String,
    pub file_mask: Option<String>,
    pub mask_mode: String,
    pub content_query: Option<String>,
    pub size_op: Option<String>,
    pub size_value: Option<u64>,
    pub size_max: Option<u64>,
    pub size_unit: Option<String>,
    pub time_op: Option<String>,
    pub time_value: Option<String>,
    pub time_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_date_field: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchResultSchema {
    pub name: String,
    pub kind: String,
    pub size: Option<u64>,
    pub mtime: Option<String>,
    pub parent_path: String,
    pub full_path: String,
    pub source_id: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchProgressSchema {
    pub scanned: u64,
    pub matched: u64,
    pub current_path: String,
    pub new_results: Vec<SearchResultSchema>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileOpFileSchema {
    pub name: String,
    pub size: Option<u64>,
    pub kind: String,
    #[serde(rename = "sourcePath", skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SyncBroDiffSchema {
    pub path: String,
    pub local_size: u64,
    pub remote_size: u64,
    pub local_hash: String,
    pub remote_hash: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SyncBroErrorSchema {
    pub path: String,
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SyncBroResultSchema {
    pub matched: usize,
    pub differing: Vec<SyncBroDiffSchema>,
    pub local_only: Vec<String>,
    pub remote_only: Vec<String>,
    pub unverified: Vec<String>,
    pub local_errors: Vec<SyncBroErrorSchema>,
    pub remote_errors: Vec<SyncBroErrorSchema>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FedupBroFileSchema {
    pub path: String,
    pub size: u64,
    pub mtime: Option<u64>,
    pub hash: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FedupBroGroupSchema {
    pub hash: String,
    pub size: u64,
    pub keeper: FedupBroFileSchema,
    pub duplicates: Vec<FedupBroFileSchema>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FedupBroErrorSchema {
    pub path: String,
    pub message: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FedupBroResultSchema {
    pub duplicate_groups: usize,
    pub duplicate_files: usize,
    pub reclaimable_bytes: u64,
    pub scanned_files: usize,
    pub scanned_bytes: u64,
    pub groups: Vec<FedupBroGroupSchema>,
    pub errors: Vec<FedupBroErrorSchema>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FedupBroResultPageSchema {
    pub duplicate_groups: usize,
    pub duplicate_files: usize,
    pub reclaimable_bytes: u64,
    pub scanned_files: usize,
    pub scanned_bytes: u64,
    pub groups: Vec<FedupBroGroupSchema>,
    pub errors: Vec<FedupBroErrorSchema>,
    pub group_offset: usize,
    pub group_limit: usize,
    pub group_total: usize,
    pub error_offset: usize,
    pub error_limit: usize,
    pub error_total: usize,
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
