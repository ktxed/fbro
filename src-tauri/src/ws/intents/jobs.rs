use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{mpsc, watch, Mutex};

use crate::jobs::file_ops::{generate_dry_run_script, run_file_op};
use crate::jobs::search::{
    run_search, MaskMode, SearchDepth, SearchParams, SearchResultStore, SizeOp, TimeOp,
};
use crate::jobs::{
    AppJob, ConflictPolicy, DecisionResponse, FileOpEntry, FileOpJobMeta, FileOpType, JobKind,
    JobState, SearchJobMeta, VerificationPolicy,
};
use crate::platform::{ExternalClipboardOp, ExternalClipboardPayload};
use crate::state::{ShadowSource, Source};
use crate::vfs::{search::SearchResultsDriver, VfsError};
use crate::ws::intents::{arg_bool, arg_str, arg_u64, is_search_source};
use crate::ws::protocol::SearchParamsSchema;
use crate::ws::protocol::{now, FileOpFileSchema, JobStartSchema, ServerMessage};
use crate::ws::runtime::{
    build_sources_json, navigate_to, quiet_rescan_panel, rescan_panel, resolve_panel_id,
    selected_names_for, IntentContext,
};

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "StartFileOp" => start_file_op(args, ctx).await,
        "PasteExternalClipboard" => paste_external_clipboard(args, ctx).await,
        "WriteExternalClipboard" => write_external_clipboard(args, ctx).await,
        "ProvideJobDecision" => provide_job_decision(args, ctx).await,
        "CancelJob" => cancel_job(args, ctx).await,
        "PauseJob" => pause_job(args, ctx).await,
        "ResumeJob" => resume_job(args, ctx).await,
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
    op_type: String,
    conflict_policy: String,
    verification_policy: String,
    save_digest_file: bool,
    background: bool,
    start_paused: bool,
    dry_mode: bool,
    files: Vec<FileOpFileSchema>,
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
            op_type: arg_str(args, "opType").unwrap_or("copy").to_string(),
            conflict_policy: arg_str(args, "conflictPolicy").unwrap_or("ask").to_string(),
            verification_policy: arg_str(args, "verificationPolicy")
                .unwrap_or("fail")
                .to_string(),
            save_digest_file: arg_bool(args, "saveDigestFile", true),
            background: arg_bool(args, "background", false),
            start_paused: arg_bool(args, "startPaused", false),
            dry_mode: arg_bool(args, "dryMode", false),
            files: args
                .get("files")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default(),
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
}

impl JobIdArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            job_id: arg_str(args, "jobId").unwrap_or("").to_string(),
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
    let conflict_policy = match parsed.conflict_policy.as_str() {
        "skip" => ConflictPolicy::Skip,
        "overwrite" => ConflictPolicy::Overwrite,
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

        let has_explicit_sources = parsed.files.iter().any(|file| file.source_path.is_some());

        let (src_driver, source_path, files): (
            Arc<dyn crate::vfs::VfsDriver>,
            String,
            Vec<FileOpEntry>,
        ) = if has_explicit_sources {
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

    let (files, source_path, dest_path, src_driver, dst_driver) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            let _ = send_error(ctx, "START_FILE_OP_FAILED", error).await;
            return;
        }
    };

    if files.is_empty() {
        return;
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

    let (job_id, preserve_attributes) = {
        let mut state_guard = ctx.state.lock().await;
        let job_id = state_guard.job_store.next_job_id();
        let preserve = state_guard.settings.general.preserve_attributes;
        (job_id, preserve)
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
    let total_bytes_total: u64 = files.iter().map(|file| file.size.unwrap_or(0)).sum();
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
                total_files: files.len(),
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

    let handle = tokio::spawn(async move {
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
    {
        let mut state_guard = ctx.state.lock().await;
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
            match &job.kind {
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

async fn rename_file(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = RenameArgs::from_args(args);

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
        let result =
            tokio::task::spawn_blocking(move || driver.create_dir_all(&full_path_clone)).await;
        match result {
            Ok(Ok(())) => {
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
                let _ = send_error(ctx, "MKDIR_FAILED", error.to_string()).await;
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
