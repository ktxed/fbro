use std::path::Path;
use std::sync::{atomic::AtomicBool, Arc};

use serde_json::Value;
use tokio::sync::{mpsc, watch, Mutex};

use crate::jobs::file_ops::{generate_dry_run_script, run_file_op, same_resolved_path};
use crate::jobs::search::{
    run_search, MaskMode, SearchDepth, SearchParams, SearchResultStore, SizeOp, TimeOp,
};
use crate::jobs::{
    AppJob, ConflictPolicy, DecisionResponse, FileOpEntry, FileOpJobMeta, FileOpType, JobKind,
    JobState, QueuedFileOpLaunch, SearchJobMeta, VerificationPolicy,
};
use crate::platform::{ExternalClipboardOp, ExternalClipboardPayload};
use crate::state::{ShadowSource, Source};
use crate::vfs::{search::SearchResultsDriver, VfsError};
use crate::ws::intents::{
    arg_bool, arg_str, arg_u64, is_global_selection_source, is_search_source,
};
use crate::ws::protocol::SearchParamsSchema;
use crate::ws::protocol::{
    now, FileOpFileSchema, FileOpProgressSchema, JobStartSchema, ServerMessage,
};
use crate::ws::runtime::{
    build_global_selection_patch, build_job_queue, build_sources_json,
    ensure_global_selection_source, navigate_to, quiet_rescan_panel, rescan_panel,
    resolve_panel_id, selected_names_for, IntentContext,
};

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "StartFileOp" => start_file_op(args, ctx).await,
        "PasteExternalClipboard" => paste_external_clipboard(args, ctx).await,
        "WriteExternalClipboard" => write_external_clipboard(args, ctx).await,
        "ProvideJobDecision" => provide_job_decision(args, ctx).await,
        "CancelJob" => cancel_job(args, ctx).await,
        "SkipCurrentFile" => skip_current_file(args, ctx).await,
        "PauseJob" => pause_job(args, ctx).await,
        "ResumeJob" => resume_job(args, ctx).await,
        "DeleteQueuedJob" => delete_queued_job(args, ctx).await,
        "ReorderQueuedJob" => reorder_queued_job(args, ctx).await,
        "RenameFile" => rename_file(args, ctx).await,
        "MakeDir" => make_dir(args, ctx).await,
        "TouchFile" => touch_file(args, ctx).await,
        "SetPermissions" => set_permissions(args, ctx).await,
        "StartSearch" => start_search(args, ctx).await,
        "NavigateToSearchResult" => navigate_to_search_result(args, ctx).await,
        "MountSearchResults" => mount_search_results(args, ctx).await,
        "RemoveSearchSource" => remove_search_source(scope, args, ctx).await,
        _ => {}
    }
}

struct StartFileOpArgs {
    source_panel: String,
    dest_panel: Option<String>,
    dest_path: Option<String>,
    dest_name: Option<String>,
    op_type: String,
    conflict_policy: String,
    verification_policy: String,
    save_digest_file: bool,
    background: bool,
    start_paused: bool,
    enqueue: bool,
    dry_mode: bool,
    files: Vec<FileOpFileSchema>,
    /// Virtual VFS driver id (e.g. git-blob clipboard). When set, files' sourcePath
    /// are paths *within* that driver, not absolute host paths.
    source_id: Option<String>,
}

struct PasteExternalClipboardArgs {
    panel_id: String,
    preferred_op: Option<String>,
}

struct WriteExternalClipboardArgs {
    panel_id: String,
    op_type: String,
}

impl StartFileOpArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            source_panel: arg_str(args, "sourcePanel").unwrap_or("").to_string(),
            dest_panel: arg_str(args, "destPanel").map(String::from),
            dest_path: arg_str(args, "destPath").map(String::from),
            dest_name: arg_str(args, "destName").map(String::from),
            op_type: arg_str(args, "opType").unwrap_or("copy").to_string(),
            conflict_policy: arg_str(args, "conflictPolicy").unwrap_or("ask").to_string(),
            verification_policy: arg_str(args, "verificationPolicy")
                .unwrap_or("fail")
                .to_string(),
            save_digest_file: arg_bool(args, "saveDigestFile", true),
            background: arg_bool(args, "background", false),
            start_paused: arg_bool(args, "startPaused", false),
            enqueue: arg_bool(args, "enqueue", false),
            dry_mode: arg_bool(args, "dryMode", false),
            files: args
                .get("files")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default(),
            source_id: arg_str(args, "sourceId").map(String::from),
        }
    }
}

impl PasteExternalClipboardArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            panel_id: arg_str(args, "panelId").unwrap_or("").to_string(),
            preferred_op: arg_str(args, "preferredOp").map(str::to_string),
        }
    }
}

impl WriteExternalClipboardArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            panel_id: arg_str(args, "panelId").unwrap_or("").to_string(),
            op_type: arg_str(args, "opType").unwrap_or("copy").to_string(),
        }
    }
}

fn validate_copy_dest_name(
    op_type: &FileOpType,
    files: &[FileOpEntry],
    dest_name: &str,
) -> Result<String, String> {
    if dest_name.is_empty() {
        return Err("Destination name is required.".to_string());
    }
    if dest_name.contains('/') || dest_name.contains('\\') {
        return Err("Destination name cannot contain path separators.".to_string());
    }
    if *op_type != FileOpType::Copy {
        return Err("Destination name can only be used with copy.".to_string());
    }
    if files.len() != 1 {
        return Err("Destination name can only be used when copying one item.".to_string());
    }
    if files[0].kind != "file" && files[0].kind != "directory" {
        return Err("Destination name can only be used for files and folders.".to_string());
    }
    Ok(dest_name.to_string())
}

struct JobDecisionArgs {
    job_id: String,
    decision: String,
    apply_to_all: bool,
}

impl JobDecisionArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            job_id: arg_str(args, "jobId").unwrap_or("").to_string(),
            decision: arg_str(args, "decision").unwrap_or("skip").to_string(),
            apply_to_all: arg_bool(args, "applyToAll", false),
        }
    }
}

struct JobIdArgs {
    job_id: String,
    keep_partial_files: bool,
}

impl JobIdArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            job_id: arg_str(args, "jobId").unwrap_or("").to_string(),
            keep_partial_files: arg_bool(args, "keepPartialFiles", false),
        }
    }
}

struct RenameArgs {
    source_panel: String,
    source_path: String,
    dest_name: String,
}

impl RenameArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            source_panel: arg_str(args, "sourcePanel").unwrap_or("").to_string(),
            source_path: arg_str(args, "sourcePath").unwrap_or("").to_string(),
            dest_name: arg_str(args, "destName").unwrap_or("").to_string(),
        }
    }
}

struct MakeDirArgs {
    source_panel: String,
    dir_path: String,
    name: String,
}

impl MakeDirArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            source_panel: arg_str(args, "sourcePanel").unwrap_or("").to_string(),
            dir_path: arg_str(args, "dirPath").unwrap_or("").to_string(),
            name: arg_str(args, "name").unwrap_or("").to_string(),
        }
    }
}

struct TouchFileArgs {
    source_panel: String,
    dir_path: String,
    name: String,
}

impl TouchFileArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            source_panel: arg_str(args, "sourcePanel").unwrap_or("").to_string(),
            dir_path: arg_str(args, "dirPath").unwrap_or("").to_string(),
            name: arg_str(args, "name").unwrap_or("").to_string(),
        }
    }
}

fn normalize_source_path(path: &str) -> String {
    #[cfg(windows)]
    {
        path.replace('\\', "/").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.replace('\\', "/")
    }
}

fn source_root_matches_path(root_path: &str, source_path: &str) -> bool {
    let normalized_root = normalize_source_path(root_path)
        .trim_end_matches('/')
        .to_string();
    let normalized_path = normalize_source_path(source_path);

    if normalized_root.is_empty() || normalized_root == "/" {
        return normalized_path.starts_with('/');
    }

    normalized_path == normalized_root
        || normalized_path
            .strip_prefix(&normalized_root)
            .map(|rest| rest.starts_with('/'))
            .unwrap_or(false)
}

fn resolve_source_id_for_path(state: &crate::state::AppState, source_path: &str) -> Option<String> {
    state
        .sources
        .values()
        .filter(|source| source.mounted && !source.shadow && state.drivers.contains_key(&source.id))
        .filter(|source| source_root_matches_path(&source.root_path, source_path))
        .max_by_key(|source| source.root_path.len())
        .map(|source| source.id.clone())
        .or_else(|| fallback_local_source_id(state, source_path))
}

#[cfg(target_os = "macos")]
fn fallback_local_source_id(state: &crate::state::AppState, source_path: &str) -> Option<String> {
    if !Path::new(source_path).is_absolute() {
        return None;
    }

    state
        .sources
        .values()
        .filter(|source| {
            source.mounted && source.kind == "filesystem" && state.drivers.contains_key(&source.id)
        })
        .min_by(|a, b| {
            a.root_path
                .len()
                .cmp(&b.root_path.len())
                .then_with(|| a.id.cmp(&b.id))
        })
        .map(|source| source.id.clone())
}

#[cfg(not(target_os = "macos"))]
fn fallback_local_source_id(_state: &crate::state::AppState, _source_path: &str) -> Option<String> {
    None
}

fn resolve_common_driver_for_paths(
    state: &crate::state::AppState,
    source_paths: &[String],
) -> Result<(String, Arc<dyn crate::vfs::VfsDriver>), String> {
    let mut source_id: Option<String> = None;

    for source_path in source_paths {
        let resolved = resolve_source_id_for_path(state, source_path)
            .ok_or_else(|| format!("Clipboard item is not on a mounted source: {source_path}"))?;
        if let Some(current) = &source_id {
            if current != &resolved {
                return Err(
                    "Clipboard items from multiple mounted sources are not supported yet."
                        .to_string(),
                );
            }
        } else {
            source_id = Some(resolved);
        }
    }

    let source_id = source_id.ok_or_else(|| "Clipboard has no external files.".to_string())?;
    let driver = state
        .drivers
        .get(&source_id)
        .cloned()
        .ok_or_else(|| "Clipboard source is unavailable.".to_string())?;

    Ok((source_id, driver))
}

fn schema_from_source_path(source_path: &str) -> Result<FileOpFileSchema, String> {
    let metadata = std::fs::symlink_metadata(source_path)
        .map_err(|error| format!("Clipboard item is no longer available: {error}"))?;
    let file_name = Path::new(source_path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Clipboard item has an invalid path.".to_string())?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        "symlink"
    } else if file_type.is_dir() {
        "directory"
    } else if file_type.is_file() {
        "file"
    } else {
        "other"
    };

    Ok(FileOpFileSchema {
        name: file_name,
        size: if file_type.is_file() {
            Some(metadata.len())
        } else {
            None
        },
        kind: kind.to_string(),
        source_path: Some(source_path.replace('\\', "/")),
    })
}

fn source_paths_for_external_clipboard(
    state: &crate::state::AppState,
    panel_id: &str,
) -> Result<(Arc<dyn crate::platform::Platform>, ExternalClipboardPayload), (String, String)> {
    let Some(panel) = state.panels.get(panel_id) else {
        return Err((
            "WRITE_EXTERNAL_NO_PANEL".to_string(),
            "No focused panel is available for host clipboard export.".to_string(),
        ));
    };

    let Some(source) = state.sources.get(&panel.source_id) else {
        return Err((
            "WRITE_EXTERNAL_UNSUPPORTED_SOURCE".to_string(),
            "The focused panel cannot export files to the host clipboard.".to_string(),
        ));
    };

    let Some(driver) = state.drivers.get(&panel.source_id).cloned() else {
        return Err((
            "WRITE_EXTERNAL_UNSUPPORTED_SOURCE".to_string(),
            "The focused panel cannot export files to the host clipboard.".to_string(),
        ));
    };

    if source.kind != "filesystem" || !source.mounted || !driver.is_local() {
        return Err((
            "WRITE_EXTERNAL_UNSUPPORTED_SOURCE".to_string(),
            "The focused panel cannot export files to the host clipboard.".to_string(),
        ));
    }

    let indices: Vec<usize> = if panel.selection.is_empty() {
        vec![panel.cursor]
    } else {
        panel.selection.clone()
    };

    if indices.is_empty() {
        return Err((
            "WRITE_EXTERNAL_NO_ELIGIBLE_ITEMS".to_string(),
            "No files or folders are available to export to the host clipboard.".to_string(),
        ));
    }

    let mut source_paths = Vec::with_capacity(indices.len());
    for index in indices {
        let Some(entry) = panel.entries.get(index) else {
            return Err((
                "WRITE_EXTERNAL_NO_ELIGIBLE_ITEMS".to_string(),
                "A selected item is no longer available.".to_string(),
            ));
        };

        if entry.name == ".." {
            return Err((
                "WRITE_EXTERNAL_PARENT_ENTRY".to_string(),
                "The parent directory entry cannot be exported to the host clipboard.".to_string(),
            ));
        }

        let source_path = driver.resolve_child(&panel.path, &entry.name);
        if !Path::new(&source_path).is_absolute() {
            return Err((
                "WRITE_EXTERNAL_UNSUPPORTED_SOURCE".to_string(),
                "The focused panel cannot export files to the host clipboard.".to_string(),
            ));
        }
        source_paths.push(source_path);
    }

    if source_paths.is_empty() {
        return Err((
            "WRITE_EXTERNAL_NO_ELIGIBLE_ITEMS".to_string(),
            "No files or folders are available to export to the host clipboard.".to_string(),
        ));
    }

    Ok((
        Arc::clone(&state.platform),
        ExternalClipboardPayload {
            op: ExternalClipboardOp::Copy,
            source_paths,
        },
    ))
}

async fn write_external_clipboard(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = WriteExternalClipboardArgs::from_args(args);

    // Block clipboard export from shadow sources (zip entries aren't real filesystem paths).
    {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&parsed.panel_id) {
            if panel.shadow_source.is_some() {
                drop(state_guard);
                let _ = send_error(
                    ctx,
                    "OP_NOT_SUPPORTED",
                    "Cannot copy archive entries to the system clipboard.".to_string(),
                )
                .await;
                return;
            }
        }
    }

    if parsed.panel_id.is_empty() {
        let _ = send_error(
            ctx,
            "WRITE_EXTERNAL_NO_PANEL",
            "No focused panel is available for host clipboard export.".to_string(),
        )
        .await;
        return;
    }

    let requested_op = match parsed.op_type.as_str() {
        "move" => ExternalClipboardOp::Move,
        _ => ExternalClipboardOp::Copy,
    };

    let (platform, mut payload) = {
        let state_guard = ctx.state.lock().await;
        match source_paths_for_external_clipboard(&state_guard, &parsed.panel_id) {
            Ok(value) => value,
            Err((code, message)) => {
                let _ = send_error(ctx, &code, message).await;
                return;
            }
        }
    };

    payload.op = requested_op.clone();
    let item_count = payload.source_paths.len();

    let write_result =
        tokio::task::spawn_blocking(move || platform.write_external_file_clipboard(&payload)).await;

    match write_result {
        Ok(Ok(())) => {
            let op_type = match requested_op {
                ExternalClipboardOp::Move => "move",
                ExternalClipboardOp::Copy => "copy",
            };
            let _ = ctx
                .tx
                .send(ServerMessage::ExternalClipboardWritten {
                    timestamp: now(),
                    op_type: op_type.to_string(),
                    item_count,
                })
                .await;
        }
        Ok(Err(error)) => {
            let _ = send_error(ctx, "WRITE_EXTERNAL_CLIPBOARD_ERROR", error).await;
        }
        Err(error) => {
            let _ = send_error(
                ctx,
                "WRITE_EXTERNAL_CLIPBOARD_ERROR",
                format!("Failed to write the system clipboard: {error}"),
            )
            .await;
        }
    }
}

async fn paste_external_clipboard(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = PasteExternalClipboardArgs::from_args(args);

    // Block paste into shadow sources.
    {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&parsed.panel_id) {
            if panel.shadow_source.is_some() {
                drop(state_guard);
                let _ = send_error(
                    ctx,
                    "OP_NOT_SUPPORTED",
                    "Cannot paste into an archive.".to_string(),
                )
                .await;
                return;
            }
        }
    }

    if parsed.panel_id.is_empty() {
        let _ = send_error(
            ctx,
            "PASTE_EXTERNAL_NO_PANEL",
            "No focused panel is available for paste.".to_string(),
        )
        .await;
        return;
    }

    // Prefer the app-internal virtual clipboard (git raw file, …) over the OS clipboard.
    {
        let state_guard = ctx.state.lock().await;
        if let Some(clipboard) = state_guard.file_clipboard.clone() {
            let Some(panel) = state_guard.panels.get(&parsed.panel_id) else {
                let _ = send_error(
                    ctx,
                    "PASTE_EXTERNAL_NO_PANEL",
                    "No focused panel is available for paste.".to_string(),
                )
                .await;
                return;
            };
            if !state_guard.drivers.contains_key(&panel.source_id) {
                let _ = send_error(
                    ctx,
                    "PASTE_EXTERNAL_UNSUPPORTED_DESTINATION",
                    "The focused panel does not support paste.".to_string(),
                )
                .await;
                return;
            }
            if !state_guard.drivers.contains_key(&clipboard.source_id) {
                // Stale clipboard — fall through to OS paste.
            } else {
                let panel_path = panel.path.clone();
                let panel_id = panel.panel_id.clone();
                drop(state_guard);
                let _ = ctx
                    .tx
                    .send(ServerMessage::ExternalClipboardReady {
                        timestamp: now(),
                        panel_id,
                        op_type: "copy".to_string(),
                        files: clipboard.files,
                        dest_path: panel_path,
                        source_id: Some(clipboard.source_id),
                    })
                    .await;
                return;
            }
        }
    }

    let (panel_path, panel_id, platform) = {
        let state_guard = ctx.state.lock().await;
        let Some(panel) = state_guard.panels.get(&parsed.panel_id) else {
            let _ = send_error(
                ctx,
                "PASTE_EXTERNAL_NO_PANEL",
                "No focused panel is available for paste.".to_string(),
            )
            .await;
            return;
        };

        if !state_guard.drivers.contains_key(&panel.source_id) {
            let _ = send_error(
                ctx,
                "PASTE_EXTERNAL_UNSUPPORTED_DESTINATION",
                "The focused panel does not support external paste.".to_string(),
            )
            .await;
            return;
        }

        (
            panel.path.clone(),
            panel.panel_id.clone(),
            Arc::clone(&state_guard.platform),
        )
    };

    let payload =
        match tokio::task::spawn_blocking(move || platform.read_external_file_clipboard()).await {
            Ok(Ok(payload)) => payload,
            Ok(Err(error)) => {
                let _ = send_error(ctx, "PASTE_EXTERNAL_CLIPBOARD_ERROR", error).await;
                return;
            }
            Err(error) => {
                let _ = send_error(
                    ctx,
                    "PASTE_EXTERNAL_CLIPBOARD_ERROR",
                    format!("Failed to read the system clipboard: {error}"),
                )
                .await;
                return;
            }
        };

    let Some(payload) = payload else {
        let _ = send_error(
            ctx,
            "PASTE_EXTERNAL_NO_CLIPBOARD_PAYLOAD",
            "Clipboard does not contain external files to paste.".to_string(),
        )
        .await;
        return;
    };

    if payload.source_paths.is_empty() {
        let _ = send_error(
            ctx,
            "PASTE_EXTERNAL_NO_CLIPBOARD_PAYLOAD",
            "Clipboard does not contain external files to paste.".to_string(),
        )
        .await;
        return;
    }

    let files = {
        let state_guard = ctx.state.lock().await;
        if let Err(error) = resolve_common_driver_for_paths(&state_guard, &payload.source_paths) {
            let _ = send_error(ctx, "PASTE_EXTERNAL_MULTI_SOURCE", error).await;
            return;
        }

        let mut files = Vec::with_capacity(payload.source_paths.len());
        for source_path in &payload.source_paths {
            match schema_from_source_path(source_path) {
                Ok(file) => files.push(file),
                Err(error) => {
                    let _ = send_error(ctx, "PASTE_EXTERNAL_SOURCE_UNAVAILABLE", error).await;
                    return;
                }
            }
        }
        files
    };

    let op_type = match parsed.preferred_op.as_deref() {
        Some("move") => "move",
        Some("copy") => "copy",
        _ => match payload.op {
            ExternalClipboardOp::Copy => "copy",
            ExternalClipboardOp::Move => "move",
        },
    };

    let _ = ctx
        .tx
        .send(ServerMessage::ExternalClipboardReady {
            timestamp: now(),
            panel_id,
            op_type: op_type.to_string(),
            files,
            dest_path: panel_path,
            source_id: None,
        })
        .await;
}

async fn start_file_op(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = StartFileOpArgs::from_args(args);
    let op_type = match parsed.op_type.as_str() {
        "move" => FileOpType::Move,
        "delete" => FileOpType::Delete,
        _ => FileOpType::Copy,
    };
    if handle_global_selection_file_op(&parsed, &op_type, ctx).await {
        return;
    }
    let conflict_policy = match parsed.conflict_policy.as_str() {
        "skip" => ConflictPolicy::Skip,
        "overwrite" => ConflictPolicy::Overwrite,
        "resume" => ConflictPolicy::Resume,
        _ => ConflictPolicy::Ask,
    };
    let verification_policy = match parsed.verification_policy.as_str() {
        "prompt" => VerificationPolicy::Prompt,
        _ => VerificationPolicy::Fail,
    };

    // Block unsupported operations on shadow (archive) sources.
    {
        let state_guard = ctx.state.lock().await;
        let src_is_shadow = state_guard
            .panels
            .get(&parsed.source_panel)
            .and_then(|p| p.shadow_source.as_ref())
            .is_some();
        let dst_is_shadow = parsed
            .dest_panel
            .as_ref()
            .and_then(|id| state_guard.panels.get(id))
            .and_then(|p| p.shadow_source.as_ref())
            .is_some();

        if src_is_shadow && (op_type == FileOpType::Move || op_type == FileOpType::Delete) {
            drop(state_guard);
            let _ = send_error(
                ctx,
                "OP_NOT_SUPPORTED",
                "Cannot move or delete files inside an archive.".to_string(),
            )
            .await;
            return;
        }
        if dst_is_shadow {
            drop(state_guard);
            let _ = send_error(
                ctx,
                "OP_NOT_SUPPORTED",
                "Cannot write into an archive.".to_string(),
            )
            .await;
            return;
        }
    }

    let prepared: Result<_, String> = async {
        let state_guard = ctx.state.lock().await;
        let Some(source_panel) = state_guard.panels.get(&parsed.source_panel) else {
            return Err("Source panel is unavailable.".to_string());
        };

        let has_virtual_source = parsed
            .source_id
            .as_ref()
            .map(|id| !id.is_empty())
            .unwrap_or(false);
        let has_explicit_sources = parsed.files.iter().any(|file| file.source_path.is_some());

        let (src_driver, source_path, files): (
            Arc<dyn crate::vfs::VfsDriver>,
            String,
            Vec<FileOpEntry>,
        ) = if has_virtual_source {
            let source_id = parsed.source_id.as_ref().unwrap();
            let driver = state_guard
                .drivers
                .get(source_id)
                .cloned()
                .ok_or_else(|| "Virtual clipboard source is no longer available.".to_string())?;
            let files = parsed
                .files
                .iter()
                .map(|file| {
                    let source_path = file
                        .source_path
                        .clone()
                        .filter(|p| !p.is_empty())
                        .unwrap_or_else(|| file.name.clone());
                    FileOpEntry {
                        name: file.name.clone(),
                        dest_name: None,
                        source_path,
                        size: file.size,
                        kind: file.kind.clone(),
                        mtime: None,
                        permissions: None,
                    }
                })
                .collect();
            (driver, String::new(), files)
        } else if has_explicit_sources {
            let explicit_paths: Vec<String> = parsed
                .files
                .iter()
                .map(|file| {
                    file.source_path.clone().ok_or_else(|| {
                        "Clipboard file is missing its absolute source path.".to_string()
                    })
                })
                .collect::<Result<_, _>>()?;
            let (_source_id, driver) =
                resolve_common_driver_for_paths(&state_guard, &explicit_paths)?;
            let files = parsed
                .files
                .iter()
                .map(|file| {
                    let source_path = file.source_path.clone().ok_or_else(|| {
                        "Clipboard file is missing its absolute source path.".to_string()
                    })?;
                    Ok(FileOpEntry {
                        name: file.name.clone(),
                        dest_name: None,
                        source_path,
                        size: file.size,
                        kind: file.kind.clone(),
                        mtime: None,
                        permissions: None,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let representative_source = explicit_paths
                .first()
                .and_then(|path| Path::new(path).parent())
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|| explicit_paths.first().cloned().unwrap_or_default());
            (driver, representative_source, files)
        } else {
            let panel_driver = state_guard
                .drivers
                .get(&source_panel.source_id)
                .cloned()
                .ok_or_else(|| "Source panel driver is unavailable.".to_string())?;

            // Build entries using the panel driver's resolve_child, which for
            // a SearchResultsDriver already returns the real full_path.
            let raw_files: Vec<FileOpEntry> = if parsed.files.is_empty() {
                let indices: Vec<usize> = if source_panel.selection.is_empty() {
                    vec![source_panel.cursor]
                } else {
                    source_panel.selection.clone()
                };
                indices
                    .iter()
                    .filter_map(|&index| {
                        source_panel.entries.get(index).map(|entry| FileOpEntry {
                            name: entry.name.clone(),
                            dest_name: None,
                            source_path: panel_driver
                                .resolve_child(&source_panel.path, &entry.name),
                            size: entry.size,
                            kind: entry.kind.clone(),
                            mtime: entry.mtime.as_ref().and_then(|s| s.parse::<u64>().ok()),
                            permissions: entry.permissions.clone(),
                        })
                    })
                    .collect()
            } else {
                parsed
                    .files
                    .iter()
                    .map(|file| {
                        let panel_entry = source_panel.entries.iter().find(|e| e.name == file.name);
                        FileOpEntry {
                            name: file.name.clone(),
                            dest_name: None,
                            source_path: panel_driver.resolve_child(&source_panel.path, &file.name),
                            size: file.size,
                            kind: file.kind.clone(),
                            mtime: panel_entry
                                .and_then(|e| e.mtime.as_ref())
                                .and_then(|s| s.parse::<u64>().ok()),
                            permissions: panel_entry.and_then(|e| e.permissions.clone()),
                        }
                    })
                    .collect()
            };

            // When the source panel is on a search results source, the
            // resolve_child calls above already return real filesystem paths.
            // We must resolve the real driver from those paths rather than
            // using the search driver (which can't do I/O).
            let (driver, representative_source) = if is_search_source(&source_panel.source_id) {
                let real_paths: Vec<String> =
                    raw_files.iter().map(|f| f.source_path.clone()).collect();
                let (real_source_id, real_driver) =
                    resolve_common_driver_for_paths(&state_guard, &real_paths)?;
                let rep_source = real_paths
                    .first()
                    .and_then(|p| Path::new(p).parent())
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| {
                        state_guard
                            .sources
                            .get(&real_source_id)
                            .map(|s| s.root_path.clone())
                            .unwrap_or_default()
                    });
                (real_driver, rep_source)
            } else {
                (panel_driver, source_panel.path.clone())
            };

            (driver, representative_source, raw_files)
        };

        let (dest_path, dst_driver) = if op_type != FileOpType::Delete {
            if let Some(dest_panel) = parsed
                .dest_panel
                .as_ref()
                .and_then(|panel_id| state_guard.panels.get(panel_id))
            {
                let driver = state_guard
                    .drivers
                    .get(&dest_panel.source_id)
                    .cloned()
                    .ok_or_else(|| "Destination panel driver is unavailable.".to_string())?;
                (
                    parsed
                        .dest_path
                        .clone()
                        .or_else(|| Some(dest_panel.path.clone())),
                    Some(driver),
                )
            } else if parsed.dest_path.is_some() {
                (parsed.dest_path.clone(), Some(Arc::clone(&src_driver)))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };
        Ok((files, source_path, dest_path, src_driver, dst_driver))
    }
    .await;

    let (mut files, source_path, dest_path, src_driver, dst_driver) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = send_error(ctx, "START_FILE_OP_FAILED", error).await;
            return;
        }
    };

    if files.is_empty() {
        return;
    }

    if let Some(dest_name) = parsed.dest_name.as_deref() {
        match validate_copy_dest_name(&op_type, &files, dest_name) {
            Ok(dest_name) => {
                files[0].dest_name = Some(dest_name);
            }
            Err(error) => {
                let _ = send_error(ctx, "START_FILE_OP_FAILED", error).await;
                return;
            }
        }
    }

    if op_type == FileOpType::Copy {
        if let (Some(dest_dir), Some(dst_driver)) = (dest_path.as_deref(), dst_driver.as_deref()) {
            for file in &files {
                let dest_name = file.dest_name.as_deref().unwrap_or(&file.name);
                let dst = dst_driver.resolve_child(dest_dir, dest_name);
                if same_resolved_path(&file.source_path, &dst) {
                    let _ = send_error(
                        ctx,
                        "START_FILE_OP_FAILED",
                        format!("Cannot copy {} onto itself", file.source_path),
                    )
                    .await;
                    return;
                }
            }
        }
    }

    if parsed.enqueue {
        if let Err(error) = validate_queued_file_op_support(&parsed, ctx).await {
            let _ = send_error(ctx, "START_FILE_OP_FAILED", error).await;
            return;
        }
    }

    if parsed.dry_mode {
        let home = {
            #[cfg(unix)]
            {
                std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string())
            }
            #[cfg(windows)]
            {
                std::env::var("USERPROFILE")
                    .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().to_string())
            }
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let op_label = match op_type {
            FileOpType::Copy => "copy",
            FileOpType::Move => "move",
            FileOpType::Delete => "delete",
        };
        let script_name = format!("{timestamp}_dry_{op_label}.sh");
        let script_path = format!("{}/{}", home, script_name);
        let script_path_clone = script_path.clone();
        let files_clone = files.clone();
        let op_type_clone = op_type.clone();
        let src_driver_clone = Arc::clone(&src_driver);
        let dest_path_clone = dest_path.clone();
        let dst_driver_clone = dst_driver.as_ref().map(Arc::clone);
        let result = tokio::task::spawn_blocking(move || {
            generate_dry_run_script(
                &op_type_clone,
                &files_clone,
                &script_path_clone,
                &*src_driver_clone,
                dest_path_clone.as_deref(),
                dst_driver_clone.as_deref(),
            )
        })
        .await;
        match result {
            Ok(Ok(())) => {
                let _ = ctx
                    .tx
                    .send(ServerMessage::DryRunReady {
                        timestamp: now(),
                        script_path,
                        script_dir: home,
                    })
                    .await;
            }
            Ok(Err(error)) => {
                let _ = send_error(ctx, "DRY_RUN_FAILED", error).await;
            }
            Err(error) => {
                let _ = send_error(ctx, "DRY_RUN_FAILED", error.to_string()).await;
            }
        }
        return;
    }

    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (pause_tx, pause_rx) = watch::channel(parsed.start_paused);
    let (decision_tx, decision_rx) = mpsc::channel::<DecisionResponse>(4);
    let skip_current_file = Arc::new(AtomicBool::new(false));
    let keep_partial_files_on_cancel = Arc::new(AtomicBool::new(false));

    let (job_id, preserve_attributes, auto_resume_copies, sevenzip_mode, sevenzip_custom) = {
        let mut state_guard = ctx.state.lock().await;
        let job_id = state_guard.job_store.next_job_id();
        let preserve = state_guard.settings.general.preserve_attributes;
        let auto_resume = state_guard.settings.general.auto_resume_copies;
        let mode = state_guard.settings.general.sevenzip_extractor;
        let custom = state_guard.settings.general.sevenzip_custom_path.clone();
        (job_id, preserve, auto_resume, mode, custom)
    };

    let file_schemas: Vec<FileOpFileSchema> = files
        .iter()
        .map(|file| FileOpFileSchema {
            name: file.name.clone(),
            size: file.size,
            kind: file.kind.clone(),
            source_path: Some(file.source_path.clone()),
        })
        .collect();

    if parsed.enqueue {
        let total_bytes_total: u64 = files.iter().map(|file| file.size.unwrap_or(0)).sum();
        let total_files_for_job = files.len();
        let should_start_now = {
            let mut state_guard = ctx.state.lock().await;
            let should_start = state_guard.job_store.file_queue_running.is_none();
            let initial_state = if should_start {
                state_guard.job_store.file_queue_running = Some(job_id.clone());
                JobState::Running
            } else {
                JobState::Queued
            };
            state_guard.job_store.file_queue.push_back(job_id.clone());
            state_guard.job_store.queued_file_ops.insert(
                job_id.clone(),
                QueuedFileOpLaunch {
                    op_type: op_type.clone(),
                    files: files.clone(),
                    dest_path: dest_path.clone(),
                    conflict_policy: conflict_policy.clone(),
                    verification_policy: verification_policy.clone(),
                    save_digest_file: parsed.save_digest_file,
                    src_driver: Arc::clone(&src_driver),
                    dst_driver: dst_driver.as_ref().map(Arc::clone),
                    preserve_attributes,
                    auto_resume_copies,
                    cancel_rx,
                    pause_rx,
                    decision_rx,
                    skip_current_file: Arc::clone(&skip_current_file),
                    keep_partial_files_on_cancel: Arc::clone(&keep_partial_files_on_cancel),
                },
            );
            state_guard.job_store.jobs.insert(
                job_id.clone(),
                AppJob {
                    job_id: job_id.clone(),
                    kind: JobKind::FileOp(FileOpJobMeta {
                        op_type,
                        files,
                        conflict_policy,
                        apply_to_all_decision: None,
                    }),
                    source_panel: parsed.source_panel.clone(),
                    dest_panel: parsed.dest_panel.clone(),
                    source_path: source_path.clone(),
                    dest_path: dest_path.clone(),
                    state: initial_state,
                    progress: crate::jobs::FileOpProgress {
                        total_files: total_files_for_job,
                        completed_files: 0,
                        current_file: None,
                        status_text: None,
                        current_file_bytes_total: 0,
                        current_file_bytes_copied: 0,
                        total_bytes_total,
                        total_bytes_copied: 0,
                        current_file_percent: None,
                        overall_percent: None,
                    },
                    decision: None,
                    error: None,
                    background: true,
                    paused: false,
                    handle: None,
                    cancel_tx,
                    pause_tx,
                    skip_current_file,
                    keep_partial_files_on_cancel,
                },
            );
            should_start
        };

        let _ = ctx
            .tx
            .send(ServerMessage::JobStarted {
                timestamp: now(),
                job: JobStartSchema {
                    job_id: job_id.clone(),
                    kind: "file-op".to_string(),
                    op_type: parsed.op_type.clone(),
                    files: file_schemas,
                    source_panel: parsed.source_panel,
                    source_path,
                    dest_panel: parsed.dest_panel,
                    dest_path,
                    initial_state: if should_start_now {
                        "Running".to_string()
                    } else {
                        "Queued".to_string()
                    },
                    total_files: total_files_for_job,
                    total_bytes_total,
                    background: true,
                    supports_pause: true,
                    result_path: None,
                    result_panel_id: None,
                    request_id: None,
                    search_source_id: None,
                    search_params: None,
                },
            })
            .await;

        ctx.decision_senders.insert(job_id.clone(), decision_tx);
        send_job_queue_patch(ctx.state, ctx.tx).await;
        if should_start_now {
            let _ = start_queued_file_op(job_id, ctx.state, ctx.tx).await;
        }
        return;
    }

    // Archive-source Copy → batch streaming extraction. The generic file-op
    // worker calls `src_driver.open_read(path)` per file, which on a 7z source
    // resolves to extract_to_temp — O(N²) decompression over the solid stream
    // and no progress until each file is fully extracted to temp. We detour to
    // a single-pass extractor that walks the archive once.
    let archive_extract: Option<(Vec<(String, std::path::PathBuf)>, u64)> = if op_type
        == FileOpType::Copy
        && dest_path.is_some()
        && crate::jobs::extract_archive::driver_is_archive(&src_driver)
    {
        let dest_root = std::path::PathBuf::from(dest_path.as_ref().unwrap());
        let base_prefix = if source_path.is_empty() {
            String::new()
        } else {
            format!("{}/", source_path.trim_end_matches('/'))
        };
        let single_dest_name = if files.len() == 1 {
            files[0].dest_name.clone()
        } else {
            None
        };
        let single_source_inner = if files.len() == 1 {
            Some(files[0].source_path.trim_matches('/').to_string())
        } else {
            None
        };
        let mut targets: Vec<(String, std::path::PathBuf)> = Vec::new();
        let mut total: u64 = 0;
        for file in &files {
            let inner = file.source_path.clone();
            let expanded: Vec<(String, u64)> = if file.kind == "directory" {
                if let Some(sz) = src_driver
                    .as_ref()
                    .as_any()
                    .downcast_ref::<crate::vfs::seven_zip::SevenZipFsDriver>()
                {
                    sz.flatten_files(&inner)
                } else if let Some(zp) = src_driver
                    .as_ref()
                    .as_any()
                    .downcast_ref::<crate::vfs::zip::ZipFsDriver>()
                {
                    zp.flatten_files(&inner)
                } else {
                    Vec::new()
                }
            } else {
                let size = file.size.unwrap_or(0);
                vec![(inner.clone(), size)]
            };
            for (child_inner, size) in expanded {
                let mut rel = child_inner
                    .strip_prefix(&base_prefix)
                    .unwrap_or(&child_inner)
                    .to_string();
                if let Some(dest_name) = single_dest_name.as_deref() {
                    if file.kind == "directory" {
                        if let Some(source_inner) = single_source_inner.as_deref() {
                            let source_prefix = format!("{}/", source_inner.trim_end_matches('/'));
                            if let Some(child_rel) = child_inner.strip_prefix(&source_prefix) {
                                rel = if child_rel.is_empty() {
                                    dest_name.to_string()
                                } else {
                                    format!("{dest_name}/{child_rel}")
                                };
                            } else if child_inner == source_inner {
                                rel = dest_name.to_string();
                            }
                        }
                    } else {
                        rel = dest_name.to_string();
                    }
                }
                let dest = dest_root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
                total = total.saturating_add(size);
                targets.push((child_inner, dest));
            }
        }
        Some((targets, total))
    } else {
        None
    };

    let total_bytes_total: u64 = archive_extract
        .as_ref()
        .map(|(_, total)| *total)
        .unwrap_or_else(|| files.iter().map(|file| file.size.unwrap_or(0)).sum());
    let total_files_for_job: usize = archive_extract
        .as_ref()
        .map(|(targets, _)| targets.len())
        .unwrap_or_else(|| files.len());
    if let Some((targets, total)) = archive_extract.as_ref() {
        eprintln!(
            "[fbro] archive-extract dispatch: {} entries, {} bytes",
            targets.len(),
            total
        );
    }
    let _ = ctx
        .tx
        .send(ServerMessage::JobStarted {
            timestamp: now(),
            job: JobStartSchema {
                job_id: job_id.clone(),
                kind: "file-op".to_string(),
                op_type: parsed.op_type.clone(),
                files: file_schemas,
                source_panel: parsed.source_panel.clone(),
                source_path: source_path.clone(),
                dest_panel: parsed.dest_panel.clone(),
                dest_path: dest_path.clone(),
                initial_state: "Running".to_string(),
                total_files: total_files_for_job,
                total_bytes_total,
                background: parsed.background,
                supports_pause: true,
                result_path: None,
                result_panel_id: None,
                request_id: None,
                search_source_id: None,
                search_params: None,
            },
        })
        .await;

    ctx.decision_senders.insert(job_id.clone(), decision_tx);

    let state_for_patch = ctx.state.clone();
    let tx_for_patch = ctx.tx.clone();
    let job_id_spawn = job_id.clone();
    let op_type_spawn = op_type.clone();
    let files_spawn = files.clone();
    let dest_path_spawn = dest_path.clone();
    let conflict_policy_spawn = conflict_policy.clone();
    let verification_policy_spawn = verification_policy.clone();
    let save_digest_file_spawn = parsed.save_digest_file;
    let skip_current_file_spawn = Arc::clone(&skip_current_file);
    let keep_partial_files_on_cancel_spawn = Arc::clone(&keep_partial_files_on_cancel);

    let handle = if let Some((targets, total)) = archive_extract {
        let src_driver_extract = Arc::clone(&src_driver);
        let tx_extract = tx_for_patch.clone();
        let job_id_extract = job_id_spawn;
        let extract_options = crate::jobs::extract_archive::ExtractOptions {
            sevenzip_binary: crate::jobs::extract_archive::resolve_sevenzip_binary(
                sevenzip_mode,
                &sevenzip_custom,
            ),
        };
        tokio::spawn(async move {
            crate::jobs::extract_archive::run_extract_archive(
                job_id_extract,
                src_driver_extract,
                targets,
                total,
                extract_options,
                tx_extract,
                cancel_rx,
            )
            .await;

            let sources_json = {
                let state = state_for_patch.lock().await;
                build_sources_json(&state)
            };
            let _ = tx_for_patch
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "sources".to_string(),
                    entity_id: None,
                    version: 0,
                    patch: serde_json::json!({ "sources": sources_json }),
                })
                .await;
        })
    } else {
        tokio::spawn(async move {
            run_file_op(
                job_id_spawn,
                op_type_spawn,
                files_spawn,
                dest_path_spawn,
                conflict_policy_spawn,
                tx_for_patch.clone(),
                cancel_rx,
                pause_rx,
                decision_rx,
                src_driver,
                dst_driver,
                preserve_attributes,
                verification_policy_spawn,
                save_digest_file_spawn,
                auto_resume_copies,
                skip_current_file_spawn,
                keep_partial_files_on_cancel_spawn,
            )
            .await;

            let sources_json = {
                let state = state_for_patch.lock().await;
                build_sources_json(&state)
            };
            let _ = tx_for_patch
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "sources".to_string(),
                    entity_id: None,
                    version: 0,
                    patch: serde_json::json!({ "sources": sources_json }),
                })
                .await;
        })
    };

    {
        let mut state_guard = ctx.state.lock().await;
        state_guard.job_store.jobs.insert(
            job_id.clone(),
            AppJob {
                job_id: job_id.clone(),
                kind: JobKind::FileOp(FileOpJobMeta {
                    op_type,
                    files,
                    conflict_policy,
                    apply_to_all_decision: None,
                }),
                source_panel: parsed.source_panel,
                dest_panel: parsed.dest_panel,
                source_path,
                dest_path,
                state: JobState::Running,
                progress: Default::default(),
                decision: None,
                error: None,
                background: parsed.background,
                paused: parsed.start_paused,
                handle: Some(handle),
                cancel_tx,
                pause_tx,
                skip_current_file,
                keep_partial_files_on_cancel,
            },
        );
    }

    if parsed.start_paused {
        let _ = ctx
            .tx
            .send(ServerMessage::JobEvent {
                timestamp: now(),
                job_id,
                event: "Paused".to_string(),
                files: None,
                progress: None,
                search_progress: None,
                decision: None,
                error: None,
                result_path: None,
                result_panel_id: None,
            })
            .await;
    }
}

async fn provide_job_decision(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobDecisionArgs::from_args(args);
    if crate::ws::intents::bro::handle_fp_bro_job_decision(
        &parsed.job_id,
        &parsed.decision,
        ctx.state,
        ctx.tx,
    )
    .await
    .unwrap_or(false)
    {
        return;
    }

    {
        let mut state_guard = ctx.state.lock().await;
        if let Some(job) = state_guard.job_store.jobs.get_mut(&parsed.job_id) {
            if let JobKind::FileOp(meta) = &mut job.kind {
                meta.apply_to_all_decision = if parsed.apply_to_all {
                    match parsed.decision.as_str() {
                        "overwrite" => Some(ConflictPolicy::Overwrite),
                        "resume" => Some(ConflictPolicy::Resume),
                        "skip" => Some(ConflictPolicy::Skip),
                        _ => None,
                    }
                } else {
                    None
                };
            }
        }
    }
    if let Some(sender) = ctx.decision_senders.get(&parsed.job_id) {
        let _ = sender
            .send(DecisionResponse {
                decision: parsed.decision,
                apply_to_all: parsed.apply_to_all,
            })
            .await;
    }
}

async fn cancel_job(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobIdArgs::from_args(args);
    let mut send_cancelled_event = false;
    let mut queue_changed = false;
    {
        let mut state_guard = ctx.state.lock().await;
        let file_queued_pending = state_guard
            .job_store
            .file_queue
            .iter()
            .any(|job_id| job_id == &parsed.job_id)
            && state_guard.job_store.file_queue_running.as_deref() != Some(parsed.job_id.as_str());
        if file_queued_pending {
            state_guard.job_store.remove_from_file_queue(&parsed.job_id);
            queue_changed = true;
        }
        let fp_queued = state_guard
            .job_store
            .jobs
            .get(&parsed.job_id)
            .map(|job| matches!(job.kind, JobKind::FpBro(_)) && job.state == JobState::Queued)
            .unwrap_or(false);
        if fp_queued {
            state_guard.job_store.remove_from_fp_queue(&parsed.job_id);
        }

        if let Some(job) = state_guard.job_store.jobs.get_mut(&parsed.job_id) {
            job.set_keep_partial_files_on_cancel(parsed.keep_partial_files);
            match &job.kind {
                JobKind::FileOp(_) if file_queued_pending => {
                    job.state = JobState::Cancelled;
                    job.paused = false;
                    job.decision = None;
                    send_cancelled_event = true;
                }
                JobKind::FpBro(_)
                    if job.state == JobState::Queued || job.state == JobState::AwaitingDecision =>
                {
                    job.state = JobState::Cancelled;
                    send_cancelled_event = true;
                }
                JobKind::Search(_) => {
                    // Signal the cancel watch so workers stop gracefully, then
                    // abort the spawned task immediately for a fast stop.
                    let _ = job.cancel_tx.send(true);
                    if let Some(handle) = job.handle.take() {
                        handle.abort();
                    }
                    job.state = JobState::Cancelled;
                    send_cancelled_event = true;
                }
                JobKind::OutgoingTransfer(meta) => {
                    let _ = &meta.peer_fp;
                    let _ = job.cancel_tx.send(true);
                    job.state = JobState::Cancelled;
                    send_cancelled_event = true;
                }
                JobKind::RemoteManifest(meta) => {
                    let _ = (&meta.root, &meta.peer_fp);
                    let _ = job.cancel_tx.send(true);
                    job.state = JobState::Cancelled;
                }
                _ => {
                    let _ = job.cancel_tx.send(true);
                    job.state = JobState::Cancelled;
                }
            }
        }
    }
    ctx.decision_senders.remove(&parsed.job_id);
    if send_cancelled_event {
        // For search jobs: mark the store done so the VFS driver knows the
        // result set is finalised.
        let search_source_id = format!("search:{}", &parsed.job_id);
        {
            let state_guard = ctx.state.lock().await;
            if let Some(store) = state_guard.search_stores.get(&search_source_id) {
                let store = store.clone();
                drop(state_guard);
                store.lock().await.done = true;
            }
        }

        let _ = ctx
            .tx
            .send(ServerMessage::JobEvent {
                timestamp: now(),
                job_id: parsed.job_id,
                event: "Cancelled".to_string(),
                files: None,
                progress: None,
                search_progress: None,
                decision: None,
                error: None,
                result_path: None,
                result_panel_id: None,
            })
            .await;
    }
    if queue_changed {
        send_job_queue_patch(ctx.state, ctx.tx).await;
    }
}

async fn skip_current_file(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobIdArgs::from_args(args);
    let mut state_guard = ctx.state.lock().await;
    let Some(job) = state_guard.job_store.jobs.get_mut(&parsed.job_id) else {
        return;
    };
    if job.state != JobState::Running || job.paused {
        return;
    }
    if !matches!(
        &job.kind,
        JobKind::FileOp(FileOpJobMeta {
            op_type: FileOpType::Copy,
            ..
        })
    ) {
        return;
    }
    job.request_skip_current_file();
}

async fn pause_job(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobIdArgs::from_args(args);
    let mut state_guard = ctx.state.lock().await;
    if let Some(job) = state_guard.job_store.jobs.get_mut(&parsed.job_id) {
        if matches!(job.kind, JobKind::FileOp(_)) && job.state == JobState::Running {
            let _ = job.pause_tx.send(true);
            job.paused = true;
        } else {
            return;
        }
    } else {
        return;
    }
    drop(state_guard);
    let _ = ctx
        .tx
        .send(ServerMessage::JobEvent {
            timestamp: now(),
            job_id: parsed.job_id,
            event: "Paused".to_string(),
            files: None,
            progress: None,
            search_progress: None,
            decision: None,
            error: None,
            result_path: None,
            result_panel_id: None,
        })
        .await;
}

async fn resume_job(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobIdArgs::from_args(args);
    let mut state_guard = ctx.state.lock().await;
    if let Some(job) = state_guard.job_store.jobs.get_mut(&parsed.job_id) {
        if matches!(job.kind, JobKind::FileOp(_)) && job.paused {
            let _ = job.pause_tx.send(false);
            job.paused = false;
        } else {
            return;
        }
    } else {
        return;
    }
    drop(state_guard);
    let _ = ctx
        .tx
        .send(ServerMessage::JobEvent {
            timestamp: now(),
            job_id: parsed.job_id,
            event: "Resumed".to_string(),
            files: None,
            progress: None,
            search_progress: None,
            decision: None,
            error: None,
            result_path: None,
            result_panel_id: None,
        })
        .await;
}

async fn send_job_queue_patch(
    state: &crate::state::SharedState,
    tx: &tokio::sync::mpsc::Sender<ServerMessage>,
) {
    let queue = {
        let state_guard = state.lock().await;
        build_job_queue(&state_guard)
    };
    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "jobQueue".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!(queue),
        })
        .await;
}

fn job_progress_schema(job: &AppJob) -> FileOpProgressSchema {
    FileOpProgressSchema {
        total_files: job.progress.total_files,
        completed_files: job.progress.completed_files,
        current_file: job.progress.current_file.clone(),
        status_text: job.progress.status_text.clone(),
        current_file_bytes_total: job.progress.current_file_bytes_total,
        current_file_bytes_copied: job.progress.current_file_bytes_copied,
        total_bytes_total: job.progress.total_bytes_total,
        total_bytes_copied: job.progress.total_bytes_copied,
        current_file_percent: job.progress.current_file_percent,
        overall_percent: job.progress.overall_percent,
    }
}

async fn start_queued_file_op(
    job_id: String,
    state: &crate::state::SharedState,
    tx: &tokio::sync::mpsc::Sender<ServerMessage>,
) -> Result<(), String> {
    let launch = {
        let mut state_guard = state.lock().await;
        let Some(job) = state_guard.job_store.jobs.get_mut(&job_id) else {
            return Err("Queued copy job no longer exists.".to_string());
        };
        if !matches!(job.kind, JobKind::FileOp(_)) {
            return Err("Queued job is not a file operation.".to_string());
        }
        job.state = JobState::Running;
        job.paused = false;
        job.error = None;
        job.decision = None;
        state_guard.job_store.queued_file_ops.remove(&job_id)
    };
    let Some(launch) = launch else {
        return Ok(());
    };

    let state_for_patch = state.clone();
    let tx_for_patch = tx.clone();
    let job_id_spawn = job_id.clone();
    let handle = tokio::spawn(async move {
        run_file_op(
            job_id_spawn,
            launch.op_type,
            launch.files,
            launch.dest_path,
            launch.conflict_policy,
            tx_for_patch.clone(),
            launch.cancel_rx,
            launch.pause_rx,
            launch.decision_rx,
            launch.src_driver,
            launch.dst_driver,
            launch.preserve_attributes,
            launch.verification_policy,
            launch.save_digest_file,
            launch.auto_resume_copies,
            launch.skip_current_file,
            launch.keep_partial_files_on_cancel,
        )
        .await;

        let sources_json = {
            let state = state_for_patch.lock().await;
            build_sources_json(&state)
        };
        let _ = tx_for_patch
            .send(ServerMessage::StatePatch {
                timestamp: now(),
                domain: "sources".to_string(),
                entity_id: None,
                version: 0,
                patch: serde_json::json!({ "sources": sources_json }),
            })
            .await;
    });

    let progress = {
        let mut state_guard = state.lock().await;
        if let Some(job) = state_guard.job_store.jobs.get_mut(&job_id) {
            job.handle = Some(handle);
            Some(job_progress_schema(job))
        } else {
            None
        }
    };

    let _ = tx
        .send(ServerMessage::JobEvent {
            timestamp: now(),
            job_id,
            event: "Progress".to_string(),
            files: None,
            progress,
            search_progress: None,
            decision: None,
            error: None,
            result_path: None,
            result_panel_id: None,
        })
        .await;

    Ok(())
}

pub(crate) async fn maybe_start_next_queued_file_op(
    completed_job_id: &str,
    state: &crate::state::SharedState,
    tx: &tokio::sync::mpsc::Sender<ServerMessage>,
) -> Result<(), String> {
    let next_job_id = {
        let mut state_guard = state.lock().await;
        if state_guard.job_store.file_queue_running.as_deref() == Some(completed_job_id) {
            state_guard.job_store.file_queue_running = None;
            state_guard
                .job_store
                .remove_from_file_queue(completed_job_id);
            if !state_guard.job_store.file_queue.is_empty() {
                state_guard.job_store.file_queue_completed += 1;
            }
        } else {
            state_guard
                .job_store
                .remove_from_file_queue(completed_job_id);
        }

        if state_guard.job_store.file_queue_running.is_some() {
            None
        } else {
            let next = state_guard.job_store.file_queue.front().cloned();
            if let Some(job_id) = &next {
                state_guard.job_store.file_queue_running = Some(job_id.clone());
            }
            next
        }
    };

    if let Some(job_id) = next_job_id {
        start_queued_file_op(job_id, state, tx).await?;
    }
    send_job_queue_patch(state, tx).await;
    Ok(())
}

async fn validate_queued_file_op_support(
    parsed: &StartFileOpArgs,
    ctx: &mut IntentContext<'_>,
) -> Result<(), String> {
    if parsed.op_type != "copy" {
        return Err("Only copy jobs can be queued.".to_string());
    }
    if parsed.dry_mode {
        return Err("Dry-run jobs cannot be queued.".to_string());
    }
    let Some(dest_panel_id) = parsed.dest_panel.as_deref() else {
        return Err("Queued copies require a destination panel.".to_string());
    };

    let state_guard = ctx.state.lock().await;
    let Some(source_panel) = state_guard.panels.get(&parsed.source_panel) else {
        return Err("Source panel is unavailable.".to_string());
    };
    let Some(dest_panel) = state_guard.panels.get(dest_panel_id) else {
        return Err("Destination panel is unavailable.".to_string());
    };
    let Some(source) = state_guard.sources.get(&source_panel.source_id) else {
        return Err("Source is unavailable.".to_string());
    };
    let Some(destination) = state_guard.sources.get(&dest_panel.source_id) else {
        return Err("Destination is unavailable.".to_string());
    };
    if source.kind != "filesystem"
        || source.shadow
        || destination.kind != "filesystem"
        || destination.shadow
    {
        return Err("Only local filesystem copies can be queued.".to_string());
    }
    let Some(src_driver) = state_guard.drivers.get(&source_panel.source_id) else {
        return Err("Source driver is unavailable.".to_string());
    };
    let Some(dst_driver) = state_guard.drivers.get(&dest_panel.source_id) else {
        return Err("Destination driver is unavailable.".to_string());
    };
    if !src_driver.is_local() || !dst_driver.is_local() {
        return Err("Only local filesystem copies can be queued.".to_string());
    }
    Ok(())
}

async fn delete_queued_job(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobIdArgs::from_args(args);
    let mut should_send = false;
    {
        let mut state_guard = ctx.state.lock().await;
        let is_running =
            state_guard.job_store.file_queue_running.as_deref() == Some(parsed.job_id.as_str());
        if is_running {
            return;
        }
        if !state_guard
            .job_store
            .file_queue
            .iter()
            .any(|job_id| job_id == &parsed.job_id)
        {
            return;
        }
        state_guard.job_store.remove_from_file_queue(&parsed.job_id);
        if let Some(job) = state_guard.job_store.jobs.get_mut(&parsed.job_id) {
            job.state = JobState::Cancelled;
            job.paused = false;
            job.decision = None;
            should_send = true;
        }
    }
    if should_send {
        ctx.decision_senders.remove(&parsed.job_id);
        let _ = ctx
            .tx
            .send(ServerMessage::JobEvent {
                timestamp: now(),
                job_id: parsed.job_id,
                event: "Cancelled".to_string(),
                files: None,
                progress: None,
                search_progress: None,
                decision: None,
                error: None,
                result_path: None,
                result_panel_id: None,
            })
            .await;
        send_job_queue_patch(ctx.state, ctx.tx).await;
    }
}

async fn reorder_queued_job(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobIdArgs::from_args(args);
    let action = arg_str(args, "action").unwrap_or("");
    let changed = {
        let mut state_guard = ctx.state.lock().await;
        if state_guard.job_store.file_queue_running.as_deref() == Some(parsed.job_id.as_str()) {
            return;
        }
        let Some(index) = state_guard
            .job_store
            .file_queue
            .iter()
            .position(|job_id| job_id == &parsed.job_id)
        else {
            return;
        };
        let active_offset = usize::from(state_guard.job_store.file_queue_running.is_some());
        let min_index = active_offset;
        match action {
            "up" if index > min_index => {
                state_guard.job_store.file_queue.swap(index, index - 1);
                true
            }
            "down" if index + 1 < state_guard.job_store.file_queue.len() => {
                state_guard.job_store.file_queue.swap(index, index + 1);
                true
            }
            "next" if index > min_index => {
                if let Some(job_id) = state_guard.job_store.file_queue.remove(index) {
                    state_guard.job_store.file_queue.insert(min_index, job_id);
                    true
                } else {
                    false
                }
            }
            "last" if index + 1 < state_guard.job_store.file_queue.len() => {
                if let Some(job_id) = state_guard.job_store.file_queue.remove(index) {
                    state_guard.job_store.file_queue.push_back(job_id);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    };
    if changed {
        send_job_queue_patch(ctx.state, ctx.tx).await;
    }
}

async fn handle_global_selection_file_op(
    parsed: &StartFileOpArgs,
    op_type: &FileOpType,
    ctx: &mut IntentContext<'_>,
) -> bool {
    let (is_global, selection_name, selection_names, paths) = {
        let state_guard = ctx.state.lock().await;
        let Some(panel) = state_guard.panels.get(&parsed.source_panel) else {
            return false;
        };
        if !is_global_selection_source(&panel.source_id) {
            return false;
        }
        if *op_type != FileOpType::Delete {
            drop(state_guard);
            let _ = send_error(
                ctx,
                "OP_NOT_SUPPORTED",
                "Only deleting global selection references is supported.".to_string(),
            )
            .await;
            return true;
        }
        let indices = || {
            if panel.selection.is_empty() {
                vec![panel.cursor]
            } else {
                panel.selection.clone()
            }
        };
        let selection_names = if panel.path.is_empty() {
            if parsed.files.is_empty() {
                indices()
                    .iter()
                    .filter_map(|index| panel.entries.get(*index))
                    .map(|entry| entry.name.clone())
                    .collect::<Vec<_>>()
            } else {
                parsed
                    .files
                    .iter()
                    .map(|file| file.name.clone())
                    .collect::<Vec<_>>()
            }
        } else {
            Vec::new()
        };
        let paths = if panel.path.is_empty() {
            Vec::new()
        } else if parsed.files.is_empty() {
            let indices = if panel.selection.is_empty() {
                vec![panel.cursor]
            } else {
                panel.selection.clone()
            };
            indices
                .iter()
                .filter_map(|index| panel.entries.get(*index))
                .filter_map(|entry| {
                    entry
                        .source_path
                        .clone()
                        .or_else(|| Some(entry.name.clone()))
                })
                .collect::<Vec<_>>()
        } else {
            parsed
                .files
                .iter()
                .filter_map(|file| file.source_path.clone().or_else(|| Some(file.name.clone())))
                .collect::<Vec<_>>()
        };
        (true, panel.path.clone(), selection_names, paths)
    };
    if !is_global {
        return false;
    }
    if selection_name.is_empty() && selection_names.is_empty() {
        let _ = send_error(
            ctx,
            "OP_NOT_SUPPORTED",
            "No global selections were selected.".to_string(),
        )
        .await;
        return true;
    }
    if !selection_name.is_empty() && paths.is_empty() {
        let _ = send_error(
            ctx,
            "OP_NOT_SUPPORTED",
            "No global selection references were selected.".to_string(),
        )
        .await;
        return true;
    }

    let patch_result: Result<(Option<Vec<serde_json::Value>>, ServerMessage), String> = {
        let mut state_guard = ctx.state.lock().await;
        let store_result = {
            let store_arc = std::sync::Arc::clone(&state_guard.global_selections);
            let result = match store_arc.lock() {
                Ok(mut store) => {
                    if selection_name.is_empty() {
                        store.remove_selections_by_names(&selection_names)
                    } else {
                        store.remove_items_by_paths(&selection_name, &paths)
                    }
                }
                Err(_) => Err("Global selection store is unavailable.".to_string()),
            };
            result
        };
        match store_result {
            Ok(_) => {
                if selection_name.is_empty() {
                    ensure_global_selection_source(&mut state_guard);
                    Ok((
                        Some(build_sources_json(&state_guard)),
                        build_global_selection_patch(&state_guard),
                    ))
                } else {
                    Ok((None, build_global_selection_patch(&state_guard)))
                }
            }
            Err(message) => Err(message),
        }
    };
    let (sources_json, patch) = match patch_result {
        Ok(patches) => patches,
        Err(message) => {
            let _ = send_error(ctx, "OP_NOT_SUPPORTED", message).await;
            return true;
        }
    };
    if let Some(sources_json) = sources_json {
        let _ = ctx
            .tx
            .send(ServerMessage::StatePatch {
                timestamp: now(),
                domain: "sources".to_string(),
                entity_id: None,
                version: 0,
                patch: serde_json::json!({ "sources": sources_json }),
            })
            .await;
    }
    let _ = ctx.tx.send(patch).await;
    crate::ws::intents::global_selection::rescan_global_selection_panels(ctx).await;
    true
}

async fn rename_global_selection_if_needed(
    parsed: &RenameArgs,
    ctx: &mut IntentContext<'_>,
) -> bool {
    let (is_global, panel_path) = {
        let state_guard = ctx.state.lock().await;
        let Some(panel) = state_guard.panels.get(&parsed.source_panel) else {
            return false;
        };
        if !is_global_selection_source(&panel.source_id) {
            return false;
        }
        (true, panel.path.clone())
    };
    if !is_global {
        return false;
    }
    if !panel_path.is_empty() {
        let _ = send_error(
            ctx,
            "RENAME_FAILED",
            "Cannot rename global selection entries.".to_string(),
        )
        .await;
        return true;
    }
    if parsed.dest_name.contains('/') || parsed.dest_name.contains('\\') {
        let _ = send_error(
            ctx,
            "RENAME_FAILED",
            "Selection name cannot contain path separators.".to_string(),
        )
        .await;
        return true;
    }

    let old_name = parsed.source_path.trim_start_matches('/').to_string();
    let new_name = parsed.dest_name.trim().to_string();
    let patch_result: Result<ServerMessage, String> = {
        let mut state_guard = ctx.state.lock().await;
        let store_result = {
            let store_arc = std::sync::Arc::clone(&state_guard.global_selections);
            let result = match store_arc.lock() {
                Ok(mut store) => store.rename_selection(&old_name, &new_name),
                Err(_) => Err("Global selection store is unavailable.".to_string()),
            };
            result
        };
        match store_result {
            Ok(_) => {
                for panel in state_guard.panels.values_mut() {
                    if panel.source_id == crate::vfs::global_selection::SOURCE_ID
                        && panel.path == old_name
                    {
                        panel.path = new_name.clone();
                    }
                }
                Ok(build_global_selection_patch(&state_guard))
            }
            Err(message) => Err(message),
        }
    };
    let patch = match patch_result {
        Ok(patch) => patch,
        Err(message) => {
            let _ = send_error(ctx, "RENAME_FAILED", message).await;
            return true;
        }
    };
    let _ = ctx
        .tx
        .send(ServerMessage::RenameOk {
            timestamp: now(),
            panel_id: parsed.source_panel.clone(),
        })
        .await;
    let _ = ctx.tx.send(patch).await;
    crate::ws::intents::global_selection::rescan_global_selection_panels(ctx).await;
    true
}

async fn rename_file(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = RenameArgs::from_args(args);

    if rename_global_selection_if_needed(&parsed, ctx).await {
        return;
    }

    // Block rename inside shadow sources (zip archives).
    {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&parsed.source_panel) {
            if panel.shadow_source.is_some() {
                drop(state_guard);
                let _ = send_error(
                    ctx,
                    "OP_NOT_SUPPORTED",
                    "Cannot rename inside an archive.".to_string(),
                )
                .await;
                return;
            }
        }
    }

    if parsed.dest_name.contains('/') || parsed.dest_name.contains('\\') {
        let _ = ctx
            .tx
            .send(ServerMessage::Error {
                timestamp: now(),
                code: "RENAME_FAILED".to_string(),
                message: "Rename cannot change directories — use Move for that.".to_string(),
                recoverable: true,
                extra: None,
            })
            .await;
        return;
    }

    let source_dir = std::path::Path::new(&parsed.source_path)
        .parent()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let dest_path = format!("{}/{}", source_dir.trim_end_matches('/'), parsed.dest_name);

    let (driver, selection_names) = {
        let state_guard = ctx.state.lock().await;
        let driver = state_guard
            .panels
            .get(&parsed.source_panel)
            .and_then(|panel| state_guard.drivers.get(&panel.source_id).cloned());
        let names = selected_names_for(&state_guard, &parsed.source_panel);
        (driver, names)
    };

    if let Some(driver) = driver {
        let source_path = parsed.source_path.clone();
        let dest_path_clone = dest_path.clone();
        let result =
            tokio::task::spawn_blocking(move || driver.try_rename(&source_path, &dest_path_clone))
                .await;
        match result {
            Ok(Ok(true)) => {
                crate::journal::record(
                    "mutation.entry.v1", None, None, None, Some("completed"),
                    serde_json::json!({ "kind": "rename", "source": parsed.source_path, "destination": dest_path, "undoEligible": false }),
                );
                let _ = ctx
                    .tx
                    .send(ServerMessage::RenameOk {
                        timestamp: now(),
                        panel_id: parsed.source_panel.clone(),
                    })
                    .await;
                rescan_panel(
                    &parsed.source_panel,
                    ctx.state,
                    ctx.tx,
                    ctx.scan_handles,
                    Some(parsed.dest_name),
                    selection_names,
                    ctx.dir_size_queue,
                    ctx.watcher_cmd_tx,
                )
                .await;
            }
            Ok(Ok(false)) => {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        timestamp: now(),
                        code: "RENAME_FAILED".to_string(),
                        message: "Cannot rename across filesystems — use Move instead.".to_string(),
                        recoverable: true,
                        extra: None,
                    })
                    .await;
            }
            Ok(Err(error)) => {
                let _ = send_error(ctx, "RENAME_FAILED", error.to_string()).await;
            }
            Err(_) => {}
        }
    }
}

async fn make_dir(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = MakeDirArgs::from_args(args);

    // Block mkdir inside shadow sources (zip archives).
    {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&parsed.source_panel) {
            if panel.shadow_source.is_some() {
                drop(state_guard);
                let _ = send_error(
                    ctx,
                    "OP_NOT_SUPPORTED",
                    "Cannot create directories inside an archive.".to_string(),
                )
                .await;
                return;
            }
        }
    }

    let full_path = format!("{}/{}", parsed.dir_path.trim_end_matches('/'), parsed.name);

    let (driver, selection_names) = {
        let state_guard = ctx.state.lock().await;
        let driver = state_guard
            .panels
            .get(&parsed.source_panel)
            .and_then(|panel| state_guard.drivers.get(&panel.source_id).cloned());
        let names = selected_names_for(&state_guard, &parsed.source_panel);
        (driver, names)
    };

    if let Some(driver) = driver {
        let full_path_clone = full_path.clone();
        // create_dir_all is mkdir -p style and succeeds when the path already
        // exists as a directory. F7 must reject that as a name conflict.
        let result = tokio::task::spawn_blocking(move || {
            if driver.exists(&full_path_clone) {
                return Err(VfsError::AlreadyExists(full_path_clone));
            }
            driver.create_dir_all(&full_path_clone)
        })
        .await;
        match result {
            Ok(Ok(())) => {
                crate::journal::record(
                    "mutation.entry.v1", None, None, None, Some("completed"),
                    serde_json::json!({ "kind": "mkdir", "destination": full_path, "undoEligible": false }),
                );
                let _ = ctx
                    .tx
                    .send(ServerMessage::MkdirOk {
                        timestamp: now(),
                        panel_id: parsed.source_panel.clone(),
                    })
                    .await;
                rescan_panel(
                    &parsed.source_panel,
                    ctx.state,
                    ctx.tx,
                    ctx.scan_handles,
                    Some(parsed.name),
                    selection_names,
                    ctx.dir_size_queue,
                    ctx.watcher_cmd_tx,
                )
                .await;
            }
            Ok(Err(error)) => {
                let message = match &error {
                    VfsError::AlreadyExists(_) => format!(
                        "A file or directory named \"{}\" already exists.",
                        parsed.name
                    ),
                    other => other.to_string(),
                };
                let _ = send_error(ctx, "MKDIR_FAILED", message).await;
            }
            Err(_) => {}
        }
    }
}

async fn touch_file(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = TouchFileArgs::from_args(args);

    // Block inside shadow sources (zip archives).
    {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&parsed.source_panel) {
            if panel.shadow_source.is_some() {
                drop(state_guard);
                let _ = send_error(
                    ctx,
                    "OP_NOT_SUPPORTED",
                    "Cannot create files inside an archive.".to_string(),
                )
                .await;
                return;
            }
        }
    }

    let full_path = format!("{}/{}", parsed.dir_path.trim_end_matches('/'), parsed.name);

    let (driver, selection_names) = {
        let state_guard = ctx.state.lock().await;
        let driver = state_guard
            .panels
            .get(&parsed.source_panel)
            .and_then(|panel| state_guard.drivers.get(&panel.source_id).cloned());
        let names = selected_names_for(&state_guard, &parsed.source_panel);
        (driver, names)
    };

    if let Some(driver) = driver {
        let full_path_clone = full_path.clone();
        let result = tokio::task::spawn_blocking(move || driver.open_write(&full_path_clone)).await;
        match result {
            Ok(Ok(_writer)) => {
                // writer dropped here — file created as zero bytes
                crate::journal::record(
                    "mutation.entry.v1", None, None, None, Some("completed"),
                    serde_json::json!({ "kind": "touch", "destination": full_path, "undoEligible": false }),
                );
                let _ = ctx
                    .tx
                    .send(ServerMessage::TouchFileOk {
                        timestamp: now(),
                        panel_id: parsed.source_panel.clone(),
                    })
                    .await;
                rescan_panel(
                    &parsed.source_panel,
                    ctx.state,
                    ctx.tx,
                    ctx.scan_handles,
                    Some(parsed.name),
                    selection_names,
                    ctx.dir_size_queue,
                    ctx.watcher_cmd_tx,
                )
                .await;
            }
            Ok(Err(VfsError::NotSupported(msg))) => {
                let _ = send_error(ctx, "TOUCH_NOT_SUPPORTED", msg).await;
            }
            Ok(Err(error)) => {
                let _ = send_error(ctx, "TOUCH_FAILED", error.to_string()).await;
            }
            Err(_) => {}
        }
    }
}

// ─── Set permissions (chmod) ────────────────────────────────────────────────

async fn set_permissions(args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = arg_str(args, "panelId").unwrap_or("").to_string();
    let paths: Vec<String> = args
        .get("paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let raw_mode = args.get("mode").and_then(Value::as_u64);

    if panel_id.is_empty() || paths.is_empty() || raw_mode.is_none_or(|mode| mode > 0o777) {
        let _ = send_error(
            ctx,
            "SET_PERMISSIONS_FAILED",
            "Invalid arguments for SetPermissions.".to_string(),
        )
        .await;
        return;
    }
    let mode = raw_mode.unwrap() as u32;

    // Block inside shadow sources (archives).
    {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&panel_id) {
            if panel.shadow_source.is_some() {
                drop(state_guard);
                let _ = send_error(
                    ctx,
                    "SET_PERMISSIONS_FAILED",
                    "Cannot change permissions inside an archive.".to_string(),
                )
                .await;
                return;
            }
        }
    }

    let (driver, selection_names) = {
        let state_guard = ctx.state.lock().await;
        let driver = state_guard
            .panels
            .get(&panel_id)
            .and_then(|panel| state_guard.drivers.get(&panel.source_id).cloned());
        let names = selected_names_for(&state_guard, &panel_id);
        (driver, names)
    };

    let Some(driver) = driver else {
        let _ = send_error(
            ctx,
            "SET_PERMISSIONS_FAILED",
            "Panel driver is unavailable.".to_string(),
        )
        .await;
        return;
    };

    // Only local filesystem sources support permissions changes.
    if !driver.is_local() {
        let _ = send_error(
            ctx,
            "SET_PERMISSIONS_FAILED",
            "Cannot change permissions on a non-local source.".to_string(),
        )
        .await;
        return;
    }

    let mut last_error = None;
    for path in &paths {
        let path = path.clone();
        let driver_ref = Arc::clone(&driver);
        let result =
            tokio::task::spawn_blocking(move || driver_ref.set_permissions(&path, mode)).await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                last_error = Some(e.to_string());
                break;
            }
            Err(e) => {
                last_error = Some(e.to_string());
                break;
            }
        }
    }

    if let Some(error) = last_error {
        let _ = send_error(ctx, "SET_PERMISSIONS_FAILED", error).await;
    } else {
        for path in &paths {
            crate::journal::record(
                "mutation.entry.v1", None, None, None, Some("completed"),
                serde_json::json!({ "kind": "permissions", "source": path, "mode": mode, "undoEligible": false }),
            );
        }
        let _ = ctx
            .tx
            .send(ServerMessage::SetPermissionsOk {
                timestamp: now(),
                panel_id: panel_id.clone(),
            })
            .await;
        quiet_rescan_panel(
            &panel_id,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            selection_names,
            ctx.dir_size_queue,
            ctx.watcher_cmd_tx,
        )
        .await;
    }
}

// ─── Search intents ──────────────────────────────────────────────────────────

async fn start_search(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = arg_str(args, "requestId").map(String::from);
    let background = arg_bool(args, "background", false);

    // Parse search paths
    let paths: Vec<String> = args
        .get("paths")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    if paths.is_empty() {
        return;
    }

    // Parse depth
    let depth = match arg_str(args, "depth").unwrap_or("unlimited") {
        "unlimited" => SearchDepth::Unlimited,
        "current" => SearchDepth::Current,
        other => other
            .parse::<u8>()
            .map(SearchDepth::Depth)
            .unwrap_or(SearchDepth::Unlimited),
    };

    // Parse mask mode
    let mask_mode = match arg_str(args, "maskMode").unwrap_or("wildcard") {
        "substring" => MaskMode::Substring,
        "regex" => MaskMode::Regex,
        _ => MaskMode::Wildcard,
    };

    let file_mask = arg_str(args, "fileMask")
        .filter(|s| !s.is_empty())
        .map(String::from);

    let content_query = arg_str(args, "contentQuery")
        .filter(|s| !s.is_empty())
        .map(String::from);

    // Parse size filter
    let size_op = match arg_str(args, "sizeOp") {
        Some("interval") => Some(SizeOp::Interval),
        Some("gt") => Some(SizeOp::Gt),
        Some("lt") => Some(SizeOp::Lt),
        Some("eq") => Some(SizeOp::Eq),
        _ => None,
    };
    let size_unit = arg_str(args, "sizeUnit").unwrap_or("B");
    let size_mult = match size_unit {
        "KiB" => 1024u64,
        "MiB" => 1024 * 1024,
        "GiB" => 1024 * 1024 * 1024,
        _ => 1,
    };
    let size_value_raw = size_op.map(|_| arg_u64(args, "sizeValue", 0));
    let size_value = size_value_raw.map(|v| v * size_mult);
    let size_max_raw = size_op
        .filter(|op| matches!(op, SizeOp::Interval))
        .map(|_| arg_u64(args, "sizeMax", 0));
    let size_max = size_max_raw.map(|v| v * size_mult);

    // Parse time filter
    let time_op = match arg_str(args, "timeOp") {
        Some("interval") => Some(TimeOp::Interval),
        Some("before") => Some(TimeOp::Before),
        Some("after") => Some(TimeOp::After),
        Some("relative") => Some(TimeOp::Relative),
        _ => None,
    };
    let time_value = arg_str(args, "timeValue")
        .filter(|s| !s.is_empty())
        .map(String::from);
    let time_to = arg_str(args, "timeTo")
        .filter(|s| !s.is_empty())
        .map(String::from);
    let time_date_field = arg_str(args, "timeDateField")
        .filter(|s| !s.is_empty())
        .map(String::from);

    let params = SearchParams {
        paths: paths.clone(),
        depth,
        file_mask,
        mask_mode,
        content_query,
        size_op,
        size_value,
        size_max,
        time_op,
        time_value,
        time_to,
        time_date_field,
    };

    // Capture params as a serialisable schema before params is moved into run_search.
    let params_schema = SearchParamsSchema {
        paths: params.paths.clone(),
        depth: match &params.depth {
            SearchDepth::Unlimited => "unlimited".to_string(),
            SearchDepth::Current => "current".to_string(),
            SearchDepth::Depth(d) => d.to_string(),
        },
        file_mask: params.file_mask.clone(),
        mask_mode: match &params.mask_mode {
            MaskMode::Substring => "substring".to_string(),
            MaskMode::Wildcard => "wildcard".to_string(),
            MaskMode::Regex => "regex".to_string(),
        },
        content_query: params.content_query.clone(),
        size_op: params.size_op.map(|op| {
            match op {
                SizeOp::Interval => "interval",
                SizeOp::Gt => "gt",
                SizeOp::Lt => "lt",
                SizeOp::Eq => "eq",
            }
            .to_string()
        }),
        size_value: size_value_raw,
        size_max: size_max_raw,
        size_unit: size_op.map(|_| size_unit.to_string()),
        time_op: params.time_op.map(|op| {
            match op {
                TimeOp::Interval => "interval",
                TimeOp::Before => "before",
                TimeOp::After => "after",
                TimeOp::Relative => "relative",
            }
            .to_string()
        }),
        time_value: params.time_value.clone(),
        time_to: params.time_to.clone(),
        time_date_field: params.time_date_field.clone(),
    };

    let (cancel_tx, cancel_rx) = watch::channel(false);
    let store = Arc::new(Mutex::new(SearchResultStore::new()));
    let source_path = paths.first().cloned().unwrap_or_default();

    let (job_id, source_id) = {
        let mut state = ctx.state.lock().await;
        let job_id = state.job_store.next_job_id();
        let source_id = format!("search:{}", job_id);

        // Register the search result source (shadow)
        let source = Source {
            id: source_id.clone(),
            name: "Search Results".to_string(),
            kind: "virtual".to_string(),
            supports: vec![],
            root_path: "/".to_string(),
            mounted: true,
            device_path: None,
            shadow: true,
            icon_name: None,
            sort_priority: 100,
            read_only: false,
            peer_id: None,
        };
        state.sources.insert(source_id.clone(), source);

        let driver: Arc<dyn crate::vfs::VfsDriver> =
            Arc::new(SearchResultsDriver::new(Arc::clone(&store)));
        state.drivers.insert(source_id.clone(), driver);
        state
            .search_stores
            .insert(source_id.clone(), Arc::clone(&store));

        let job = AppJob {
            job_id: job_id.clone(),
            kind: JobKind::Search(SearchJobMeta {
                params: params.clone(),
                source_id: source_id.clone(),
            }),
            source_panel: String::new(),
            dest_panel: None,
            source_path: source_path.clone(),
            dest_path: None,
            state: JobState::Running,
            progress: Default::default(),
            decision: None,
            error: None,
            background,
            paused: false,
            handle: None,
            cancel_tx,
            pause_tx: watch::channel(false).0,
            skip_current_file: Arc::new(AtomicBool::new(false)),
            keep_partial_files_on_cancel: Arc::new(AtomicBool::new(false)),
        };
        state.job_store.jobs.insert(job_id.clone(), job);

        (job_id, source_id)
    };

    // Send sources patch so FE knows about the new shadow source
    let sources_json = {
        let state = ctx.state.lock().await;
        build_sources_json(&state)
    };
    let _ = ctx
        .tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "sources".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "sources": sources_json }),
        })
        .await;

    // Send JobStarted
    let _ = ctx
        .tx
        .send(ServerMessage::JobStarted {
            timestamp: now(),
            job: JobStartSchema {
                job_id: job_id.clone(),
                kind: "search".to_string(),
                op_type: "search".to_string(),
                files: vec![],
                source_panel: String::new(),
                source_path: source_path.clone(),
                dest_panel: None,
                dest_path: None,
                initial_state: "Running".to_string(),
                total_files: 0,
                total_bytes_total: 0,
                background,
                supports_pause: false,
                result_path: None,
                result_panel_id: None,
                request_id,
                search_source_id: Some(source_id),
                search_params: Some(params_schema),
            },
        })
        .await;

    // Build path→source_id map for the worker (sorted longest-first so
    // the most-specific source is matched first).
    let path_source_map: Arc<Vec<(String, String)>> = {
        let state = ctx.state.lock().await;
        let mut map: Vec<(String, String)> = state
            .sources
            .values()
            .filter(|s| !s.shadow && s.mounted && !s.root_path.is_empty())
            .map(|s| (s.root_path.trim_end_matches('/').to_string(), s.id.clone()))
            .collect();
        // Sort longest root_path first so the most-specific match wins
        map.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        // macOS exposes the boot volume's contents under /Users, /System, etc.
        // rather than under any /Volumes/* source root, so those paths won't
        // match anything in the map above. Add a catch-all (empty root) entry
        // pointing at any local filesystem source so search hits get a
        // resolvable source_id for later viewer/copy operations.
        if !map.iter().any(|(root, _)| root.is_empty()) {
            if let Some(seed) = paths.first() {
                if let Some(fallback) = fallback_local_source_id(&state, seed) {
                    map.push((String::new(), fallback));
                }
            }
        }
        Arc::new(map)
    };

    // Spawn the search worker
    let tx = ctx.tx.clone();
    let handle = tokio::spawn(run_search(
        job_id.clone(),
        params,
        path_source_map,
        store,
        tx,
        cancel_rx,
    ));

    // Store the join handle
    {
        let mut state = ctx.state.lock().await;
        if let Some(job) = state.job_store.jobs.get_mut(&job_id) {
            job.handle = Some(handle);
        }
    }
}

async fn navigate_to_search_result(args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id_arg = arg_str(args, "panelId").unwrap_or("").to_string();
    let full_path = arg_str(args, "fullPath").unwrap_or("").to_string();
    let kind = arg_str(args, "kind").unwrap_or("file");

    if full_path.is_empty() {
        return;
    }

    // Fall back to the currently focused panel when no panelId is supplied.
    let panel_id = if panel_id_arg.is_empty() {
        ctx.state.lock().await.focus.clone()
    } else {
        panel_id_arg
    };

    // Determine target path and cursor hint
    let (nav_path, cursor_hint) = if kind == "directory" {
        (full_path.clone(), None)
    } else {
        // Navigate to the parent directory with cursor on the file
        let parent = std::path::Path::new(&full_path)
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_string());
        let name = std::path::Path::new(&full_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        (parent, name)
    };

    // Tear down the search shadow source from the panel before navigating
    {
        let mut state = ctx.state.lock().await;
        // Find the best real source for the target path (before taking panel mut ref)
        let real_source_id = state
            .sources
            .iter()
            .filter(|(_, s)| !s.shadow && s.mounted)
            .max_by_key(|(_, s)| {
                if nav_path.starts_with(&s.root_path) {
                    s.root_path.len()
                } else {
                    0
                }
            })
            .map(|(id, _)| id.clone())
            .unwrap_or_default();

        if let Some(panel) = state.panels.get_mut(&panel_id) {
            if !real_source_id.is_empty() {
                panel.source_id = real_source_id;
            }
            panel.shadow_source = None;
        }
    }

    navigate_to(
        &panel_id,
        &nav_path,
        cursor_hint,
        None,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        ctx.dir_size_queue,
        ctx.watcher_cmd_tx,
    )
    .await;
}

async fn mount_search_results(args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id_arg = arg_str(args, "panelId").unwrap_or("").to_string();
    let job_id = arg_str(args, "jobId").unwrap_or("").to_string();

    let panel_id = if panel_id_arg.is_empty() {
        resolve_panel_id(&serde_json::Value::Null, ctx.state).await
    } else {
        panel_id_arg
    };

    let source_id = format!("search:{}", job_id);

    // Verify the source exists
    let source_exists = {
        let state = ctx.state.lock().await;
        state.sources.contains_key(&source_id)
    };
    if !source_exists {
        return;
    }

    // Switch the panel to the search results source
    {
        let mut state = ctx.state.lock().await;
        if let Some(panel) = state.panels.get_mut(&panel_id) {
            panel.shadow_source = Some(ShadowSource {
                parent_source_id: panel.source_id.clone(),
                parent_path: panel.path.clone(),
                archive_path: source_id.clone(),
                cursor_hint: None,
                cursor_index: None,
                parent_shadow: None,
            });
            panel.source_id = source_id.clone();
            panel.path = "/".to_string();
            panel.filter = None;
            panel.filter_mode_active = false;
        }
    }

    rescan_panel(
        &panel_id,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        None,
        None,
        ctx.dir_size_queue,
        ctx.watcher_cmd_tx,
    )
    .await;
}

async fn remove_search_source(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let source_id = arg_str(args, "sourceId")
        .or_else(|| scope.as_str())
        .unwrap_or("")
        .to_string();

    if source_id.is_empty() || !is_search_source(&source_id) {
        return;
    }

    let job_id = source_id.strip_prefix("search:").unwrap_or("").to_string();

    // Cancel running job if any
    {
        let state = ctx.state.lock().await;
        if let Some(job) = state.job_store.jobs.get(&job_id) {
            let _ = job.cancel_tx.send(true);
        }
    }

    // Find panels using this source and navigate them back to parent
    let panels_to_migrate: Vec<(String, String, String)> = {
        let state = ctx.state.lock().await;
        state
            .panels
            .iter()
            .filter(|(_, p)| p.source_id == source_id)
            .map(|(id, p)| {
                let (parent_source, parent_path) = p
                    .shadow_source
                    .as_ref()
                    .map(|ss| (ss.parent_source_id.clone(), ss.parent_path.clone()))
                    .unwrap_or_else(|| {
                        // Fallback: find any non-shadow mounted source
                        let fallback = state
                            .sources
                            .iter()
                            .find(|(_, s)| !s.shadow && s.mounted)
                            .map(|(id, s)| (id.clone(), s.root_path.clone()))
                            .unwrap_or_default();
                        fallback
                    });
                (id.clone(), parent_source, parent_path)
            })
            .collect()
    };

    for (panel_id, parent_source_id, parent_path) in panels_to_migrate {
        {
            let mut state = ctx.state.lock().await;
            if let Some(panel) = state.panels.get_mut(&panel_id) {
                panel.source_id = parent_source_id;
                panel.shadow_source = None;
                panel.path = parent_path.clone();
            }
        }
        navigate_to(
            &panel_id,
            &parent_path,
            None,
            None,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            ctx.dir_size_queue,
            ctx.watcher_cmd_tx,
        )
        .await;
    }

    // Remove the source, driver, and store
    {
        let mut state = ctx.state.lock().await;
        state.sources.remove(&source_id);
        state.drivers.remove(&source_id);
        state.search_stores.remove(&source_id);
        // Remove the job
        state.job_store.jobs.remove(&job_id);
    }

    // Tell the FE to drop this job from its jobs map
    let _ = ctx
        .tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "jobs".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "removeJobId": job_id }),
        })
        .await;

    // Send updated sources patch
    let sources_json = {
        let state = ctx.state.lock().await;
        build_sources_json(&state)
    };
    let _ = ctx
        .tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "sources".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "sources": sources_json }),
        })
        .await;
}

async fn send_error(
    ctx: &IntentContext<'_>,
    code: &str,
    message: String,
) -> Result<(), mpsc::error::SendError<ServerMessage>> {
    ctx.tx
        .send(ServerMessage::Error {
            timestamp: now(),
            code: code.to_string(),
            message,
            recoverable: true,
            extra: None,
        })
        .await
}
