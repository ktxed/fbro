use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;
use tokio::sync::watch;

use crate::bro::{launch_bro, list_bros, BroInputItem, BroProtocol, ResolvedBro};
use crate::jobs::fp_bro::run_fp_bro;
use crate::jobs::{AppJob, FileOpProgress, FpBroJobMeta, JobKind, JobState};
use crate::ws::intents::arg_str;
use crate::ws::protocol::{
    now, BroInputSchema, BroLaunchInputSchema, BroSchema, FileOpFileSchema, JobStartSchema,
    ServerMessage,
};
use crate::ws::runtime::{resolve_panel_id, IntentContext};

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "ListBros" => list_bros_for_request(scope, args, ctx).await,
        "LaunchBro" => launch_selected_bro(args, ctx).await,
        _ => {}
    }
}

struct ListBrosArgs {
    request_id: String,
    target_path: Option<String>,
    suggested_output_dir: Option<String>,
}

impl ListBrosArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            request_id: arg_str(args, "requestId").unwrap_or("").to_string(),
            target_path: arg_str(args, "targetPath").map(String::from),
            suggested_output_dir: arg_str(args, "suggestedOutputDir").map(String::from),
        }
    }
}

struct LaunchBroArgs {
    request_id: String,
    bro_id: String,
    inputs: HashMap<String, String>,
}

impl LaunchBroArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            request_id: arg_str(args, "requestId").unwrap_or("").to_string(),
            bro_id: arg_str(args, "broId").unwrap_or("").to_string(),
            inputs: args
                .get("inputs")
                .and_then(Value::as_object)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

async fn list_bros_for_request(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = ListBrosArgs::from_args(args);
    if parsed.request_id.is_empty() {
        return;
    }

    let (result, result_panel_id) = {
        let state_guard = ctx.state.lock().await;
        let result_panel_id = Some(other_panel_id(&state_guard, &panel_id).unwrap_or_else(|| panel_id.clone()));
        let result = match state_guard.panels.get(&panel_id) {
            Some(panel) => match state_guard.drivers.get(&panel.source_id) {
                Some(driver) => resolve_inputs(panel, driver.as_ref(), parsed.target_path.as_deref()),
                None => Err("Source driver is not available.".to_string()),
            },
            None => Err("Panel not found.".to_string()),
        };
        (result, result_panel_id)
    };

    let inputs = match result {
        Ok(inputs) => inputs,
        Err(message) => {
            let _ = ctx
                .tx
                .send(ServerMessage::BroOptionsFailed {
                    timestamp: now(),
                    request_id: parsed.request_id,
                    message,
                })
                .await;
            return;
        }
    };

    let lookup_inputs = inputs.clone();
    let suggested_output_dir = parsed.suggested_output_dir.clone();
    let bros =
        match tokio::task::spawn_blocking(move || list_bros(&lookup_inputs, suggested_output_dir.as_deref())).await {
            Ok(bros) => bros,
            Err(_) => {
                let _ = ctx
                    .tx
                    .send(ServerMessage::BroOptionsFailed {
                        timestamp: now(),
                        request_id: parsed.request_id,
                        message: "Bro discovery failed.".to_string(),
                    })
                    .await;
                return;
            }
        };
    let input_schemas = inputs
        .iter()
        .map(|input| BroInputSchema {
            path: input.path.clone(),
            name: input.name.clone(),
            kind: input.kind.clone(),
        })
        .collect::<Vec<_>>();
    let bro_schemas = bros
        .iter()
        .map(|bro| BroSchema {
            id: bro.option.id.clone(),
            name: bro.option.name.clone(),
            version: bro.option.version.clone(),
            source: bro.option.source.as_str().to_string(),
            detail: bro.option.detail.clone(),
            protocol: bro.option.protocol.as_str().to_string(),
            inputs: bro
                .option
                .inputs
                .iter()
                .map(|input| BroLaunchInputSchema {
                    id: input.id.clone(),
                    label: input.label.clone(),
                    kind: input.kind.as_str().to_string(),
                    required: input.required,
                    default_value: input.default_value.clone(),
                })
                .collect(),
        })
        .collect::<Vec<_>>();

    {
        let mut state_guard = ctx.state.lock().await;
        state_guard.bro_requests.insert(
            parsed.request_id.clone(),
            crate::state::BroRequest {
                source_panel_id: panel_id,
                result_panel_id,
                inputs,
                bros,
            },
        );
        if state_guard.bro_requests.len() > 32 {
            if let Some(oldest) = state_guard.bro_requests.keys().next().cloned() {
                state_guard.bro_requests.remove(&oldest);
            }
        }
    }

    let _ = ctx
        .tx
        .send(ServerMessage::BroOptionsReady {
            timestamp: now(),
            request_id: parsed.request_id,
            inputs: input_schemas,
            bros: bro_schemas,
        })
        .await;
}

async fn launch_selected_bro(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = LaunchBroArgs::from_args(args);
    if parsed.request_id.is_empty() || parsed.bro_id.is_empty() {
        return;
    }

    let request = {
        let mut state_guard = ctx.state.lock().await;
        state_guard.bro_requests.remove(&parsed.request_id)
    };

    let Some(request) = request else {
        return;
    };

    let Some(bro) = request
        .bros
        .iter()
        .find(|bro| bro.option.id == parsed.bro_id)
        .cloned()
    else {
        return;
    };

    match bro.option.protocol {
        BroProtocol::Fnf => {
            if let Err(message) = launch_bro(&request.inputs, &bro, &parsed.inputs) {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        timestamp: now(),
                        code: "BRO_LAUNCH_FAILED".to_string(),
                        message,
                        recoverable: true,
                        extra: Some(serde_json::json!({
                            "requestId": parsed.request_id,
                            "broId": parsed.bro_id,
                        })),
                    })
                    .await;
            }
        }
        BroProtocol::Fp => {
            if let Err(message) = launch_fp_bro(request, bro, parsed.inputs, ctx).await {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        timestamp: now(),
                        code: "BRO_LAUNCH_FAILED".to_string(),
                        message,
                        recoverable: true,
                        extra: Some(serde_json::json!({
                            "requestId": parsed.request_id,
                            "broId": parsed.bro_id,
                        })),
                    })
                    .await;
            }
        }
    }
}

async fn launch_fp_bro(
    request: crate::state::BroRequest,
    bro: ResolvedBro,
    launch_inputs: HashMap<String, String>,
    ctx: &mut IntentContext<'_>,
) -> Result<(), String> {
    let output_path = bro
        .output_path(&launch_inputs)?
        .ok_or_else(|| "This bro does not declare an output file path.".to_string())?;
    validate_output_path(&output_path)?;

    let output_exists = Path::new(&output_path).exists();
    let source_path = first_input_parent(&request.inputs)
        .unwrap_or_else(|| request.inputs.first().map(|input| input.path.clone()).unwrap_or_default());
    let file_schemas = bro_input_files(&request.inputs);

    let job_id = {
        let mut state_guard = ctx.state.lock().await;
        state_guard.job_store.next_job_id()
    };

    let initial_state = if output_exists {
        JobState::AwaitingDecision
    } else {
        let state_guard = ctx.state.lock().await;
        if state_guard.job_store.fp_running.is_none() {
            JobState::Running
        } else {
            JobState::Queued
        }
    };

    let _ = ctx
        .tx
        .send(ServerMessage::JobStarted {
            timestamp: now(),
            job: JobStartSchema {
                job_id: job_id.clone(),
                kind: "fp-bro".to_string(),
                op_type: bro.option.name.clone(),
                files: file_schemas,
                source_panel: request.source_panel_id.clone(),
                source_path: source_path.clone(),
                dest_panel: request.result_panel_id.clone(),
                dest_path: Some(output_path.clone()),
                initial_state: job_state_wire(&initial_state).to_string(),
                total_files: request.inputs.len(),
                total_bytes_total: 0,
                background: false,
                supports_pause: false,
                result_path: Some(output_path.clone()),
                result_panel_id: request.result_panel_id.clone(),
            },
        })
        .await;

    let (cancel_tx, _cancel_rx) = watch::channel(false);
    let (pause_tx, _pause_rx) = watch::channel(false);

    let should_start_now = {
        let mut state_guard = ctx.state.lock().await;
        state_guard.job_store.jobs.insert(
            job_id.clone(),
            AppJob {
                job_id: job_id.clone(),
                kind: JobKind::FpBro(FpBroJobMeta {
                    bro_id: bro.option.id.clone(),
                    bro_name: bro.option.name.clone(),
                    output_path: output_path.clone(),
                    inputs: request.inputs.clone(),
                    launch_inputs,
                    bro,
                }),
                source_panel: request.source_panel_id,
                dest_panel: request.result_panel_id,
                source_path,
                dest_path: Some(output_path.clone()),
                state: initial_state.clone(),
                progress: FileOpProgress {
                    total_files: request.inputs.len(),
                    completed_files: 0,
                    current_file: None,
                    current_file_bytes_total: 0,
                    current_file_bytes_copied: 0,
                },
                decision: None,
                error: None,
                background: false,
                paused: false,
                handle: None,
                cancel_tx,
                pause_tx,
            },
        );

        if output_exists {
            false
        } else if initial_state == JobState::Running {
            state_guard.job_store.fp_running = Some(job_id.clone());
            true
        } else {
            state_guard.job_store.fp_queue.push_back(job_id.clone());
            false
        }
    };

    if output_exists {
        let _ = ctx
            .tx
            .send(ServerMessage::JobEvent {
                timestamp: now(),
                job_id,
                event: "DecisionRequired".to_string(),
                progress: None,
                decision: Some(crate::ws::protocol::ConflictDecisionSchema {
                    source: output_path.clone(),
                    destination: output_path,
                    options: vec!["overwrite".to_string(), "skip".to_string()],
                    supports_apply_to_all: false,
                }),
                error: None,
                result_path: None,
                result_panel_id: None,
            })
            .await;
        return Ok(());
    }

    if should_start_now {
        start_fp_job(job_id, ctx.state, ctx.tx).await?;
    }

    Ok(())
}

pub(crate) async fn maybe_start_next_fp_job(
    state: &crate::state::SharedState,
    tx: &tokio::sync::mpsc::Sender<ServerMessage>,
) -> Result<(), String> {
    let next_job_id = {
        let mut state_guard = state.lock().await;
        if state_guard.job_store.fp_running.is_some() {
            return Ok(());
        }
        let Some(job_id) = state_guard.job_store.fp_queue.pop_front() else {
            return Ok(());
        };
        state_guard.job_store.fp_running = Some(job_id.clone());
        if let Some(job) = state_guard.job_store.jobs.get_mut(&job_id) {
            job.state = JobState::Running;
            job.error = None;
            job.decision = None;
        }
        job_id
    };

    start_fp_job(next_job_id, state, tx).await
}

pub(crate) async fn handle_fp_bro_job_decision(
    job_id: &str,
    decision: &str,
    state: &crate::state::SharedState,
    tx: &tokio::sync::mpsc::Sender<ServerMessage>,
) -> Result<bool, String> {
    enum FpDecisionOutcome {
        StartNow,
        Queue,
        Cancel,
    }

    let outcome = {
        let mut state_guard = state.lock().await;
        let is_running = state_guard.job_store.fp_running.is_some();
        let Some(job) = state_guard.job_store.jobs.get_mut(job_id) else {
            return Ok(false);
        };
        let JobKind::FpBro(_) = &job.kind else {
            return Ok(false);
        };
        if job.state != JobState::AwaitingDecision {
            return Ok(false);
        }

        match decision {
            "overwrite" => {
                let outcome = if !is_running {
                    job.state = JobState::Running;
                    FpDecisionOutcome::StartNow
                } else {
                    job.state = JobState::Queued;
                    FpDecisionOutcome::Queue
                };
                job.decision = None;
                job.error = None;
                outcome
            }
            _ => {
                job.state = JobState::Cancelled;
                job.decision = None;
                FpDecisionOutcome::Cancel
            }
        }
    };

    match outcome {
        FpDecisionOutcome::StartNow => {
            let mut state_guard = state.lock().await;
            state_guard.job_store.fp_running = Some(job_id.to_string());
            drop(state_guard);
            start_fp_job(job_id.to_string(), state, tx).await?;
        }
        FpDecisionOutcome::Queue => {
            let mut state_guard = state.lock().await;
            state_guard.job_store.fp_queue.push_back(job_id.to_string());
            drop(state_guard);
            let _ = tx
                .send(ServerMessage::JobEvent {
                    timestamp: now(),
                    job_id: job_id.to_string(),
                    event: "Queued".to_string(),
                    progress: None,
                    decision: None,
                    error: None,
                    result_path: None,
                    result_panel_id: None,
                })
                .await;
        }
        FpDecisionOutcome::Cancel => {
            let _ = tx
                .send(ServerMessage::JobEvent {
                    timestamp: now(),
                    job_id: job_id.to_string(),
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
    Ok(true)
}

async fn start_fp_job(
    job_id: String,
    state: &crate::state::SharedState,
    tx: &tokio::sync::mpsc::Sender<ServerMessage>,
) -> Result<(), String> {
    let (bro, inputs, launch_inputs, cancel_rx) = {
        let mut state_guard = state.lock().await;
        let Some(job) = state_guard.job_store.jobs.get_mut(&job_id) else {
            return Err("Queued bro job no longer exists.".to_string());
        };
        let JobKind::FpBro(meta) = &job.kind else {
            return Err("Requested job is not an fp bro job.".to_string());
        };
        (
            meta.bro.clone(),
            meta.inputs.clone(),
            meta.launch_inputs.clone(),
            job.cancel_tx.subscribe(),
        )
    };

    let handle = tokio::spawn(run_fp_bro(
        job_id.clone(),
        bro,
        inputs,
        launch_inputs,
        tx.clone(),
        cancel_rx,
    ));

    let mut state_guard = state.lock().await;
    if let Some(job) = state_guard.job_store.jobs.get_mut(&job_id) {
        job.handle = Some(handle);
    }
    Ok(())
}

fn validate_output_path(output_path: &str) -> Result<(), String> {
    let path = Path::new(output_path);
    let Some(parent) = path.parent() else {
        return Err("The output path must include a parent directory.".to_string());
    };
    if !parent.is_dir() {
        return Err("The output directory does not exist.".to_string());
    }
    Ok(())
}

fn job_state_wire(state: &JobState) -> &'static str {
    match state {
        JobState::Queued => "Queued",
        JobState::Running => "Running",
        JobState::AwaitingDecision => "AwaitingDecision",
        JobState::Completed => "Completed",
        JobState::Cancelled => "Cancelled",
        JobState::Failed => "Failed",
    }
}

fn other_panel_id(state: &crate::state::AppState, panel_id: &str) -> Option<String> {
    state.panels.keys().find(|id| id.as_str() != panel_id).cloned()
}

fn first_input_parent(inputs: &[BroInputItem]) -> Option<String> {
    let first = inputs.first()?;
    Path::new(&first.path)
        .parent()
        .map(|path| path.to_string_lossy().to_string())
}

fn bro_input_files(inputs: &[BroInputItem]) -> Vec<FileOpFileSchema> {
    inputs
        .iter()
        .map(|input| FileOpFileSchema {
            name: input.name.clone(),
            size: None,
            kind: input.kind.clone(),
        })
        .collect()
}

fn resolve_inputs(
    panel: &crate::state::Panel,
    driver: &dyn crate::vfs::VfsDriver,
    target_path: Option<&str>,
) -> Result<Vec<BroInputItem>, String> {
    if let Some(target_path) = target_path {
        return panel
            .entries
            .iter()
            .find(|entry| {
                entry.name != ".." && driver.resolve_child(&panel.path, &entry.name) == target_path
            })
            .map(|entry| vec![bro_input_for_entry(panel, driver, entry)])
            .ok_or_else(|| "The requested item is no longer available.".to_string());
    }

    let inputs = if panel.selection.is_empty() {
        entry_at_abs_index(panel, panel.cursor)
            .filter(|entry| entry.name != "..")
            .map(|entry| vec![bro_input_for_entry(panel, driver, entry)])
            .unwrap_or_default()
    } else {
        panel
            .selection
            .iter()
            .filter_map(|index| entry_at_abs_index(panel, *index))
            .filter(|entry| entry.name != "..")
            .map(|entry| bro_input_for_entry(panel, driver, entry))
            .collect()
    };

    if inputs.is_empty() {
        Err("No valid input entries are selected.".to_string())
    } else {
        Ok(inputs)
    }
}

fn entry_at_abs_index<'a>(
    panel: &'a crate::state::Panel,
    abs_index: usize,
) -> Option<&'a crate::ws::protocol::EntrySchema> {
    let local_index = abs_index.saturating_sub(panel.window_start);
    panel.entries.get(local_index)
}

fn bro_input_for_entry(
    panel: &crate::state::Panel,
    driver: &dyn crate::vfs::VfsDriver,
    entry: &crate::ws::protocol::EntrySchema,
) -> BroInputItem {
    BroInputItem {
        path: driver.resolve_child(&panel.path, &entry.name),
        name: entry.name.clone(),
        kind: entry.kind.clone(),
    }
}
