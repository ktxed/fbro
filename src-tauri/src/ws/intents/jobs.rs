use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{mpsc, watch};

use crate::jobs::file_ops::{generate_dry_run_script, run_file_op};
use crate::jobs::{ConflictPolicy, FileOpEntry, FileOpJob, FileOpType, JobState};
use crate::ws::intents::{arg_bool, arg_str};
use crate::ws::protocol::{now, FileOpFileSchema, JobStartSchema, ServerMessage};
use crate::ws::runtime::{rescan_panel, selected_names_for, IntentContext};

pub(crate) async fn handle(name: &str, _scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "StartFileOp" => start_file_op(args, ctx).await,
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
    op_type: String,
    conflict_policy: String,
    background: bool,
    start_paused: bool,
    dry_mode: bool,
}

impl StartFileOpArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            source_panel: arg_str(args, "sourcePanel").unwrap_or("").to_string(),
            dest_panel: arg_str(args, "destPanel").map(String::from),
            op_type: arg_str(args, "opType").unwrap_or("copy").to_string(),
            conflict_policy: arg_str(args, "conflictPolicy").unwrap_or("ask").to_string(),
            background: arg_bool(args, "background", false),
            start_paused: arg_bool(args, "startPaused", false),
            dry_mode: arg_bool(args, "dryMode", false),
        }
    }
}

struct JobDecisionArgs {
    job_id: String,
    decision: String,
}

impl JobDecisionArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            job_id: arg_str(args, "jobId").unwrap_or("").to_string(),
            decision: arg_str(args, "decision").unwrap_or("skip").to_string(),
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

    let (files, source_path, dest_path, src_driver, dst_driver) = {
        let state_guard = ctx.state.lock().await;
        let Some(source_panel) = state_guard.panels.get(&parsed.source_panel) else {
            return;
        };
        let src_driver = state_guard
            .drivers
            .get(&source_panel.source_id)
            .cloned()
            .unwrap();

        let indices: Vec<usize> = if source_panel.selection.is_empty() {
            vec![source_panel.cursor]
        } else {
            source_panel.selection.clone()
        };

        let files: Vec<FileOpEntry> = indices
            .iter()
            .filter_map(|&index| {
                source_panel.entries.get(index).map(|entry| FileOpEntry {
                    name: entry.name.clone(),
                    source_path: src_driver.resolve_child(&source_panel.path, &entry.name),
                    size: entry.size,
                    kind: entry.kind.clone(),
                })
            })
            .collect();

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
                    .unwrap();
                (Some(dest_panel.path.clone()), Some(driver))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        (
            files,
            source_panel.path.clone(),
            dest_path,
            src_driver,
            dst_driver,
        )
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
    let (decision_tx, decision_rx) = mpsc::channel::<String>(4);

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
        })
        .collect();
    let total_bytes_total: u64 = files.iter().map(|file| file.size.unwrap_or(0)).sum();
    let _ = ctx
        .tx
        .send(ServerMessage::JobStarted {
            timestamp: now(),
            job: JobStartSchema {
                job_id: job_id.clone(),
                op_type: parsed.op_type.clone(),
                files: file_schemas,
                source_panel: parsed.source_panel.clone(),
                source_path,
                dest_panel: parsed.dest_panel.clone(),
                dest_path: dest_path.clone(),
                total_files: files.len(),
                total_bytes_total,
                background: parsed.background,
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
            FileOpJob {
                job_id: job_id.clone(),
                op_type,
                source_panel: parsed.source_panel,
                dest_panel: parsed.dest_panel,
                dest_path,
                files,
                conflict_policy,
                state: JobState::Running,
                progress: Default::default(),
                decision: None,
                error: None,
                background: parsed.background,
                paused: parsed.start_paused,
                handle,
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
            })
            .await;
    }
}

async fn provide_job_decision(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobDecisionArgs::from_args(args);
    if let Some(sender) = ctx.decision_senders.get(&parsed.job_id) {
        let _ = sender.send(parsed.decision).await;
    }
}

async fn cancel_job(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobIdArgs::from_args(args);
    let mut state_guard = ctx.state.lock().await;
    if let Some(job) = state_guard.job_store.jobs.get_mut(&parsed.job_id) {
        let _ = job.cancel_tx.send(true);
        job.state = JobState::Cancelled;
    }
    drop(state_guard);
    ctx.decision_senders.remove(&parsed.job_id);
}

async fn pause_job(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobIdArgs::from_args(args);
    let mut state_guard = ctx.state.lock().await;
    if let Some(job) = state_guard.job_store.jobs.get_mut(&parsed.job_id) {
        if job.state == JobState::Running {
            let _ = job.pause_tx.send(true);
            job.paused = true;
        }
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
        })
        .await;
}

async fn resume_job(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = JobIdArgs::from_args(args);
    let mut state_guard = ctx.state.lock().await;
    if let Some(job) = state_guard.job_store.jobs.get_mut(&parsed.job_id) {
        if job.paused {
            let _ = job.pause_tx.send(false);
            job.paused = false;
        }
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
                        message: "Cannot rename across filesystems — use Move instead."
                            .to_string(),
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
