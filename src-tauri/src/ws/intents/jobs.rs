use std::path::Path;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{mpsc, watch};

use crate::jobs::file_ops::{generate_dry_run_script, run_file_op};
use crate::jobs::{
    AppJob, ConflictPolicy, DecisionResponse, FileOpEntry, FileOpJobMeta, FileOpType, JobKind,
    JobState,
};
use crate::platform::{ExternalClipboardOp, ExternalClipboardPayload};
use crate::ws::intents::{arg_bool, arg_str};
use crate::ws::protocol::{now, FileOpFileSchema, JobStartSchema, ServerMessage};
use crate::ws::runtime::{rescan_panel, selected_names_for, IntentContext};

pub(crate) async fn handle(name: &str, _scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
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
        _ => {}
    }
}

struct StartFileOpArgs {
    source_panel: String,
    dest_panel: Option<String>,
    dest_path: Option<String>,
    op_type: String,
    conflict_policy: String,
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
        .filter(|source| source.mounted && state.drivers.contains_key(&source.id))
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
                "The parent directory entry cannot be exported to the host clipboard."
                    .to_string(),
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

    let write_result = tokio::task::spawn_blocking(move || {
        platform.write_external_file_clipboard(&payload)
    })
    .await;

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
            let driver = state_guard
                .drivers
                .get(&source_panel.source_id)
                .cloned()
                .ok_or_else(|| "Source panel driver is unavailable.".to_string())?;
            let files = if parsed.files.is_empty() {
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
                            source_path: driver.resolve_child(&source_panel.path, &entry.name),
                            size: entry.size,
                            kind: entry.kind.clone(),
                        })
                    })
                    .collect()
            } else {
                parsed
                    .files
                    .iter()
                    .map(|file| FileOpEntry {
                        name: file.name.clone(),
                        source_path: driver.resolve_child(&source_panel.path, &file.name),
                        size: file.size,
                        kind: file.kind.clone(),
                    })
                    .collect()
            };
            (driver, source_panel.path.clone(), files)
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

    let job_id = {
        let mut state_guard = ctx.state.lock().await;
        state_guard.job_store.next_job_id()
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
            },
        })
        .await;

    ctx.decision_senders.insert(job_id.clone(), decision_tx);

    let handle = tokio::spawn(run_file_op(
        job_id.clone(),
        op_type.clone(),
        files.clone(),
        dest_path.clone(),
        conflict_policy.clone(),
        ctx.tx.clone(),
        cancel_rx,
        pause_rx,
        decision_rx,
        src_driver,
        dst_driver,
    ));

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
                progress: None,
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
                _ => {
                    let _ = job.cancel_tx.send(true);
                    job.state = JobState::Cancelled;
                }
            }
        }
    }
    ctx.decision_senders.remove(&parsed.job_id);
    if send_cancelled_event {
        let _ = ctx
            .tx
            .send(ServerMessage::JobEvent {
                timestamp: now(),
                job_id: parsed.job_id,
                event: "Cancelled".to_string(),
                progress: None,
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
            progress: None,
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
            progress: None,
            decision: None,
            error: None,
            result_path: None,
            result_panel_id: None,
        })
        .await;
}

async fn rename_file(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = RenameArgs::from_args(args);
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
