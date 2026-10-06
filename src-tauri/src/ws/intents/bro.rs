#[cfg(unix)]
extern crate libc;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{atomic::AtomicBool, Arc};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::sync::watch;

use crate::bro::{
    launch_bro, list_bro_catalog, list_bros, BroInputItem, BroLaunchInputKind, BroOption, BroPre,
    BroProtocol, BroSource, LaunchRecipe, ResolvedBro,
};
use crate::jobs::fedup::{
    fedup_result_page, format_fedup_delete_dry_run_script, format_fedup_delete_manifest,
    format_fedup_report_from_schema, run_fedup_bro, FEDUP_DELETE_DRY_RUN_MANIFEST_NAME,
    FEDUP_DELETE_DRY_RUN_SCRIPT_NAME, FEDUP_REPORT_NAME, FEDUP_RESULT_PAGE_SIZE,
};
use crate::jobs::fp_bro::run_fp_bro;
use crate::jobs::integrity_compare::{
    format_sync_report_from_schema, run_sync_bro, SYNC_REPORT_NAME,
};
use crate::jobs::{
    AppJob, FedupBroJobMeta, FileOpProgress, FpBroJobMeta, JobKind, JobState, SyncBroJobMeta,
};
use crate::state::ChildProcessEntry;
use crate::ws::intents::{arg_str, arg_u64, is_global_selection_source};
use crate::ws::protocol::{
    now, BroCatalogSchema, BroInputSchema, BroOutputSchema, BroParamSchema, BroPreSchema,
    BroSchema, FedupBroResultSchema, FileOpFileSchema, JobStartSchema, ServerMessage,
    SyncBroResultSchema,
};
use crate::ws::runtime::{resolve_panel_id, IntentContext};

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "ListBros" => list_bros_for_request(scope, args, ctx).await,
        "ListBroCatalog" => list_bro_catalog_for_request(args, ctx).await,
        "LaunchBro" => launch_selected_bro(args, ctx).await,
        "KillProcess" => kill_process(args, ctx).await,
        "SaveSyncReport" => save_sync_report(args, ctx).await,
        "SaveFedupReport" => save_fedup_report(args, ctx).await,
        "GenerateFedupDeleteDryRun" => generate_fedup_delete_dry_run(args, ctx).await,
        "GetFedupResultPage" => get_fedup_result_page(args, ctx).await,
        _ => {}
    }
}

async fn list_bro_catalog_for_request(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = arg_str(args, "requestId").unwrap_or("").to_string();
    if request_id.is_empty() {
        return;
    }

    match tokio::task::spawn_blocking(list_bro_catalog).await {
        Ok(bros) => {
            let bros = bros
                .into_iter()
                .map(|bro| BroCatalogSchema {
                    id: bro.id,
                    name: bro.name,
                    description: bro.description,
                })
                .collect();
            let _ = ctx
                .tx
                .send(ServerMessage::BroCatalogReady {
                    timestamp: now(),
                    request_id,
                    bros,
                })
                .await;
        }
        Err(_) => {
            let _ = ctx
                .tx
                .send(ServerMessage::BroCatalogFailed {
                    timestamp: now(),
                    request_id,
                    message: "Bro catalog discovery failed.".to_string(),
                })
                .await;
        }
    }
}

async fn generate_fedup_delete_dry_run(args: &Value, ctx: &mut IntentContext<'_>) {
    let job_id = arg_str(args, "jobId").unwrap_or("").to_string();
    let (report_path, result) = match fedup_report_state(ctx, &job_id).await {
        Ok(state) => state,
        Err(error) => {
            send_generate_fedup_delete_dry_run_error(ctx, &job_id, error).await;
            return;
        }
    };
    let Some(output_dir) = Path::new(&report_path).parent().map(Path::to_path_buf) else {
        send_generate_fedup_delete_dry_run_error(
            ctx,
            &job_id,
            "fedup report destination has no parent directory".to_string(),
        )
        .await;
        return;
    };

    let script_path = output_dir.join(FEDUP_DELETE_DRY_RUN_SCRIPT_NAME);
    let manifest_path = output_dir.join(FEDUP_DELETE_DRY_RUN_MANIFEST_NAME);
    let script_path_for_worker = script_path.clone();
    let manifest_path_for_worker = manifest_path.clone();
    let write_result = tokio::task::spawn_blocking(move || {
        let script = format_fedup_delete_dry_run_script(&result)?;
        let manifest = format_fedup_delete_manifest(&result)?;
        std::fs::write(&script_path_for_worker, script).map_err(|error| {
            format!(
                "Cannot write delete dry-run script {}: {error}",
                script_path_for_worker.display()
            )
        })?;
        std::fs::write(&manifest_path_for_worker, manifest).map_err(|error| {
            format!(
                "Cannot write delete manifest {}: {error}",
                manifest_path_for_worker.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&script_path_for_worker)
                .map_err(|error| format!("Cannot read script metadata: {error}"))?
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&script_path_for_worker, permissions)
                .map_err(|error| format!("Cannot make script executable: {error}"))?;
        }
        Ok::<(), String>(())
    })
    .await;

    match write_result {
        Ok(Ok(())) => {
            let _ = ctx
                .tx
                .send(ServerMessage::FedupDeleteDryRunReady {
                    timestamp: now(),
                    job_id,
                    script_path: script_path.to_string_lossy().to_string(),
                    manifest_path: manifest_path.to_string_lossy().to_string(),
                    script_dir: output_dir.to_string_lossy().to_string(),
                })
                .await;
        }
        Ok(Err(error)) => send_generate_fedup_delete_dry_run_error(ctx, &job_id, error).await,
        Err(error) => {
            send_generate_fedup_delete_dry_run_error(ctx, &job_id, error.to_string()).await
        }
    }
}

async fn send_generate_fedup_delete_dry_run_error(
    ctx: &IntentContext<'_>,
    job_id: &str,
    message: String,
) {
    let _ = ctx
        .tx
        .send(ServerMessage::Error {
            timestamp: now(),
            code: "GENERATE_FEDUP_DELETE_DRY_RUN_FAILED".to_string(),
            message: format!("Could not generate Fedup delete dry-run: {message}"),
            recoverable: true,
            extra: Some(serde_json::json!({ "jobId": job_id })),
        })
        .await;
}

async fn save_fedup_report(args: &Value, ctx: &mut IntentContext<'_>) {
    let job_id = arg_str(args, "jobId").unwrap_or("").to_string();
    let (report_path, result) = match fedup_report_state(ctx, &job_id).await {
        Ok(state) => state,
        Err(error) => {
            send_save_fedup_report_error(ctx, &job_id, error).await;
            return;
        }
    };

    let write_path = report_path.clone();
    let write_result = tokio::task::spawn_blocking(move || {
        let body = format_fedup_report_from_schema(&result);
        std::fs::write(&write_path, body)
            .map_err(|error| format!("Cannot write report {write_path}: {error}"))
    })
    .await;

    match write_result {
        Ok(Ok(())) => {
            let _ = ctx
                .tx
                .send(ServerMessage::FedupReportSaved {
                    timestamp: now(),
                    job_id,
                    path: report_path,
                })
                .await;
        }
        Ok(Err(error)) => send_save_fedup_report_error(ctx, &job_id, error).await,
        Err(error) => send_save_fedup_report_error(ctx, &job_id, error.to_string()).await,
    }
}

async fn send_save_fedup_report_error(ctx: &IntentContext<'_>, job_id: &str, message: String) {
    let _ = ctx
        .tx
        .send(ServerMessage::Error {
            timestamp: now(),
            code: "SAVE_FEDUP_REPORT_FAILED".to_string(),
            message: format!("Could not save Fedup report: {message}"),
            recoverable: true,
            extra: Some(serde_json::json!({ "jobId": job_id })),
        })
        .await;
}

async fn get_fedup_result_page(args: &Value, ctx: &mut IntentContext<'_>) {
    let job_id = arg_str(args, "jobId").unwrap_or("").to_string();
    let group_offset = arg_u64(args, "groupOffset", 0) as usize;
    let group_limit = arg_u64(args, "groupLimit", FEDUP_RESULT_PAGE_SIZE as u64) as usize;
    let error_offset = arg_u64(args, "errorOffset", 0) as usize;
    let error_limit = arg_u64(args, "errorLimit", FEDUP_RESULT_PAGE_SIZE as u64) as usize;

    let result = {
        let state_guard = ctx.state.lock().await;
        match state_guard.job_store.jobs.get(&job_id).map(|job| &job.kind) {
            Some(JobKind::FedupBro(meta)) => meta.result.clone(),
            _ => None,
        }
    };
    let Some(result) = result else {
        send_get_fedup_result_page_error(
            ctx,
            &job_id,
            "fedup result is no longer available".to_string(),
        )
        .await;
        return;
    };

    let _ = ctx
        .tx
        .send(ServerMessage::FedupBroResultPage {
            timestamp: now(),
            job_id,
            result: fedup_result_page(
                &result,
                group_offset,
                group_limit,
                error_offset,
                error_limit,
            ),
        })
        .await;
}

async fn send_get_fedup_result_page_error(ctx: &IntentContext<'_>, job_id: &str, message: String) {
    let _ = ctx
        .tx
        .send(ServerMessage::Error {
            timestamp: now(),
            code: "GET_FEDUP_RESULT_PAGE_FAILED".to_string(),
            message: format!("Could not load Fedup result page: {message}"),
            recoverable: true,
            extra: Some(serde_json::json!({ "jobId": job_id })),
        })
        .await;
}

async fn fedup_report_state(
    ctx: &IntentContext<'_>,
    job_id: &str,
) -> Result<(String, Arc<FedupBroResultSchema>), String> {
    let state_guard = ctx.state.lock().await;
    match state_guard.job_store.jobs.get(job_id).map(|job| &job.kind) {
        Some(JobKind::FedupBro(meta)) => meta
            .result
            .clone()
            .map(|result| (meta.report_path.clone(), result))
            .ok_or_else(|| "fedup result is no longer available".to_string()),
        _ => Err("fedup job is no longer available".to_string()),
    }
}

async fn save_sync_report(args: &Value, ctx: &mut IntentContext<'_>) {
    let job_id = arg_str(args, "jobId").unwrap_or("").to_string();
    let result: SyncBroResultSchema = match args
        .get("result")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
    {
        Some(result) => result,
        None => {
            send_save_report_error(ctx, &job_id, "missing or invalid sync result".to_string())
                .await;
            return;
        }
    };

    // Recover the local folder, roots and intended report path from the job meta.
    let resolved = {
        let state_guard = ctx.state.lock().await;
        match state_guard.job_store.jobs.get(&job_id).map(|job| &job.kind) {
            Some(JobKind::SyncBro(meta)) => state_guard
                .panels
                .get(&meta.local_panel)
                .and_then(|panel| state_guard.drivers.get(&panel.source_id).cloned())
                .map(|driver| {
                    (
                        driver,
                        meta.local_root.clone(),
                        meta.remote_root.clone(),
                        meta.report_path.clone(),
                    )
                }),
            _ => None,
        }
    };
    let (driver, local_root, remote_root, report_path) = match resolved {
        Some(resolved) => resolved,
        None => {
            send_save_report_error(ctx, &job_id, "sync job is no longer available".to_string())
                .await;
            return;
        }
    };

    let write_path = report_path.clone();
    let write_result = tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let body = format_sync_report_from_schema(&local_root, &remote_root, &result);
        let mut writer = driver
            .open_write(&write_path)
            .map_err(|e| format!("Cannot create report {write_path}: {e}"))?;
        writer
            .write_all(body.as_bytes())
            .map_err(|e| format!("Cannot write report {write_path}: {e}"))?;
        writer
            .flush()
            .map_err(|e| format!("Cannot flush report {write_path}: {e}"))?;
        Ok::<(), String>(())
    })
    .await;

    match write_result {
        Ok(Ok(())) => {
            let _ = ctx
                .tx
                .send(ServerMessage::SyncReportSaved {
                    timestamp: now(),
                    job_id,
                    path: report_path,
                })
                .await;
        }
        Ok(Err(error)) => send_save_report_error(ctx, &job_id, error).await,
        Err(error) => send_save_report_error(ctx, &job_id, error.to_string()).await,
    }
}

async fn send_save_report_error(ctx: &IntentContext<'_>, job_id: &str, message: String) {
    let _ = ctx
        .tx
        .send(ServerMessage::Error {
            timestamp: now(),
            code: "SAVE_SYNC_REPORT_FAILED".to_string(),
            message: format!("Could not save sync report: {message}"),
            recoverable: true,
            extra: Some(serde_json::json!({ "jobId": job_id })),
        })
        .await;
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
    output: Option<String>,
    params: HashMap<String, String>,
}

impl LaunchBroArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            request_id: arg_str(args, "requestId").unwrap_or("").to_string(),
            bro_id: arg_str(args, "broId").unwrap_or("").to_string(),
            output: arg_str(args, "output").map(String::from),
            params: args
                .get("params")
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

type SyncRoots = (
    String,
    String,
    String,
    String,
    Arc<dyn crate::vfs::VfsDriver>,
    Arc<dyn crate::vfs::VfsDriver>,
);

fn sync_bro() -> ResolvedBro {
    ResolvedBro {
        option: BroOption {
            id: "builtin:sync-bro".to_string(),
            name: "Sync Bro".to_string(),
            version: "1".to_string(),
            source: BroSource::Native,
            detail: Some("Compare local and remote folders with BLAKE3".to_string()),
            protocol: BroProtocol::Fnf,
            output: None,
            params: Vec::new(),
            pre: BroPre {
                score: 100,
                reason: "Local/remote panel pair".to_string(),
            },
        },
        launch: LaunchRecipe::BuiltInSyncBro,
    }
}

fn compare_files_bro() -> ResolvedBro {
    ResolvedBro {
        option: BroOption {
            id: "builtin:compare-files".to_string(),
            name: "Compare files".to_string(),
            version: "1".to_string(),
            source: BroSource::Native,
            detail: Some("Compare two text files".to_string()),
            protocol: BroProtocol::Fnf,
            output: None,
            params: Vec::new(),
            pre: BroPre {
                score: 100,
                reason: "Two text files selected".to_string(),
            },
        },
        launch: LaunchRecipe::BuiltInCompareFiles,
    }
}

fn sync_roots_for_panels(
    state: &crate::state::AppState,
    panel_id: &str,
    other_panel_id: Option<&str>,
) -> Result<SyncRoots, String> {
    let other_panel_id =
        other_panel_id.ok_or_else(|| "Sync Bro requires another visible panel.".to_string())?;
    let panel = state
        .panels
        .get(panel_id)
        .ok_or_else(|| "Sync panel is unavailable.".to_string())?;
    let other_panel = state
        .panels
        .get(other_panel_id)
        .ok_or_else(|| "Other panel is unavailable.".to_string())?;
    let source = state
        .sources
        .get(&panel.source_id)
        .ok_or_else(|| "Sync source is unavailable.".to_string())?;
    let other_source = state
        .sources
        .get(&other_panel.source_id)
        .ok_or_else(|| "Other source is unavailable.".to_string())?;
    let driver = state
        .drivers
        .get(&panel.source_id)
        .cloned()
        .ok_or_else(|| "Sync driver is unavailable.".to_string())?;
    let other_driver = state
        .drivers
        .get(&other_panel.source_id)
        .cloned()
        .ok_or_else(|| "Other driver is unavailable.".to_string())?;

    match (source.kind.as_str(), other_source.kind.as_str()) {
        ("filesystem", "remote") => Ok((
            panel.panel_id.clone(),
            other_panel.panel_id.clone(),
            panel.path.clone(),
            other_panel.path.clone(),
            driver,
            other_driver,
        )),
        ("remote", "filesystem") => Ok((
            other_panel.panel_id.clone(),
            panel.panel_id.clone(),
            other_panel.path.clone(),
            panel.path.clone(),
            other_driver,
            driver,
        )),
        _ => Err("Sync Bro requires one local filesystem panel and one remote panel.".to_string()),
    }
}

async fn list_bros_for_request(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = ListBrosArgs::from_args(args);
    if parsed.request_id.is_empty() {
        return;
    }

    let (
        ranking_inputs,
        result_panel_id,
        panel_source_id,
        sync_available,
        compare_available,
        resolved_inputs,
        hash_check_location,
        hash_generate_location,
    ) = {
        let state_guard = ctx.state.lock().await;
        let result_panel_id =
            Some(other_panel_id(&state_guard, &panel_id).unwrap_or_else(|| panel_id.clone()));
        let sync_available = parsed.target_path.is_none()
            && sync_roots_for_panels(&state_guard, &panel_id, result_panel_id.as_deref()).is_ok();
        let compare_available = super::compare::can_compare_from_state(
            &state_guard,
            &panel_id,
            parsed.target_path.as_deref(),
        );
        let panel_source_id = state_guard
            .panels
            .get(&panel_id)
            .map(|p| p.source_id.clone());
        let hash_check_location = state_guard
            .panels
            .get(&panel_id)
            .filter(|panel| {
                state_guard
                    .sources
                    .get(&panel.source_id)
                    .is_some_and(|source| source.kind == "filesystem")
                    && state_guard
                        .drivers
                        .get(&panel.source_id)
                        .is_some_and(|driver| driver.is_local())
                    && has_selected_b3(panel, parsed.target_path.as_deref())
            })
            .map(|panel| (panel.source_id.clone(), panel.path.clone()));
        let hash_generate_location = state_guard
            .panels
            .get(&panel_id)
            .filter(|panel| {
                state_guard
                    .sources
                    .get(&panel.source_id)
                    .is_some_and(|source| source.kind == "filesystem")
                    && state_guard
                        .drivers
                        .get(&panel.source_id)
                        .is_some_and(|driver| driver.is_local())
                    && has_selected_hash_input(panel, parsed.target_path.as_deref(), false)
            })
            .map(|panel| (panel.source_id.clone(), panel.path.clone()));
        let result = match state_guard.panels.get(&panel_id) {
            Some(panel) if is_global_selection_source(&panel.source_id) => {
                resolve_global_selection_inputs(&state_guard, panel, parsed.target_path.as_deref())
            }
            Some(panel) => match state_guard.drivers.get(&panel.source_id) {
                Some(driver) => {
                    resolve_ranking_inputs(panel, driver.as_ref(), parsed.target_path.as_deref())
                }
                None => Err("Source driver is not available.".to_string()),
            },
            None => Err("Panel not found.".to_string()),
        };
        let resolved_inputs =
            if panel_source_id.as_deref() == Some(crate::vfs::global_selection::SOURCE_ID) {
                result.as_ref().ok().cloned()
            } else {
                None
            };
        (
            result,
            result_panel_id,
            panel_source_id,
            sync_available,
            compare_available,
            resolved_inputs,
            hash_check_location,
            hash_generate_location,
        )
    };

    let ranking_inputs = match ranking_inputs {
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

    let suggested_output_dir = parsed.suggested_output_dir.clone();
    let lookup_inputs = ranking_inputs.clone();
    let hash_check_available = hash_check_location.is_some();
    let hash_generate_available = hash_generate_location.is_some();
    let mut bros = match tokio::task::spawn_blocking(move || {
        list_bros(
            &lookup_inputs,
            suggested_output_dir.as_deref(),
            panel_source_id.as_deref(),
            hash_check_available,
            hash_generate_available,
        )
    })
    .await
    {
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
    if sync_available {
        bros.insert(0, sync_bro());
    }
    if compare_available {
        bros.insert(0, compare_files_bro());
    }
    // Sync/Compare are injected after ranking; the hash checker still goes first.
    if let Some(index) = bros
        .iter()
        .position(|bro| bro.option.id == "builtin:check-hashes")
    {
        let hash_bro = bros.remove(index);
        bros.insert(0, hash_bro);
    }
    let input_schemas = ranking_inputs
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
            pre: BroPreSchema {
                score: bro.option.pre.score,
                reason: bro.option.pre.reason.clone(),
            },
            output: bro.option.output.as_ref().map(|output| BroOutputSchema {
                label: output.label.clone(),
                default_value: output.default_value.clone(),
            }),
            params: bro
                .option
                .params
                .iter()
                .map(|input| BroParamSchema {
                    id: input.id.clone(),
                    label: input.label.clone(),
                    kind: input.kind.as_str().to_string(),
                    required: input.required,
                    default_value: input.default_value.clone(),
                    min_value: input.min_value,
                    max_value: input.max_value,
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
                target_path: parsed.target_path.clone(),
                resolved_inputs,
                bros,
                hash_check_location,
                hash_generate_location,
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
        eprintln!("[bro] no request found for id={}", parsed.request_id);
        return;
    };

    let Some(bro) = request
        .bros
        .iter()
        .find(|bro| bro.option.id == parsed.bro_id)
        .cloned()
    else {
        eprintln!(
            "[bro] bro id={} not found in request bros={:?}",
            parsed.bro_id,
            request
                .bros
                .iter()
                .map(|b| &b.option.id)
                .collect::<Vec<_>>()
        );
        return;
    };

    if matches!(
        bro.launch,
        LaunchRecipe::BuiltInCheckHashes | LaunchRecipe::BuiltInGenerateHashes
    ) {
        if let Err(message) = launch_hash_bro(request, &bro, &parsed, ctx).await {
            let _ = ctx
                .tx
                .send(ServerMessage::Error {
                    timestamp: now(),
                    code: "BRO_LAUNCH_FAILED".into(),
                    message,
                    recoverable: true,
                    extra: Some(
                        serde_json::json!({"requestId": parsed.request_id, "broId": parsed.bro_id}),
                    ),
                })
                .await;
        }
        return;
    }

    if matches!(bro.launch, LaunchRecipe::BuiltInSyncBro) {
        journal_resolved_bro_launch(
            &parsed,
            &bro,
            request.resolved_inputs.as_deref().unwrap_or(&[]),
        );
        if let Err(message) = launch_sync_bro(request, ctx).await {
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
        return;
    }
    if matches!(bro.launch, LaunchRecipe::BuiltInFedupBro) {
        journal_resolved_bro_launch(
            &parsed,
            &bro,
            request.resolved_inputs.as_deref().unwrap_or(&[]),
        );
        if let Err(message) = launch_fedup_bro(request, parsed.params.clone(), ctx).await {
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
        return;
    }
    if matches!(bro.launch, LaunchRecipe::BuiltInCompareFiles) {
        if let Err(message) = super::compare::launch_compare_for_panel(
            &request.source_panel_id,
            request.target_path.as_deref(),
            ctx,
        )
        .await
        {
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
        return;
    }

    // Re-resolve the full inputs at launch time. Clone the raw (name, kind) pairs and the
    // driver Arc under a brief lock, then build the BroInputItems (path resolution) outside
    // the lock so large selections don't block panel intents.
    type RawEntry = (String, String); // (name, kind)
    let inputs = if let Some(inputs) = request.resolved_inputs.clone() {
        if inputs.is_empty() {
            eprintln!("[bro] no valid entries in global selection");
            let _ = ctx
                .tx
                .send(ServerMessage::Error {
                    timestamp: now(),
                    code: "BRO_LAUNCH_FAILED".to_string(),
                    message: "No valid input entries are selected.".to_string(),
                    recoverable: true,
                    extra: Some(serde_json::json!({
                        "requestId": parsed.request_id,
                        "broId": parsed.bro_id,
                    })),
                })
                .await;
            return;
        }
        inputs
    } else {
        let raw_inputs: Result<
            (
                std::sync::Arc<dyn crate::vfs::VfsDriver>,
                String,
                Vec<RawEntry>,
            ),
            String,
        > = {
            let state_guard = ctx.state.lock().await;
            match state_guard.panels.get(&request.source_panel_id) {
                Some(panel) => match state_guard.drivers.get(&panel.source_id) {
                    Some(driver) => {
                        let driver = driver.clone();
                        let panel_path = panel.path.clone();
                        let raw: Vec<RawEntry> = if let Some(tp) = request.target_path.as_deref() {
                            panel
                                .entries
                                .iter()
                                .find(|e| {
                                    e.name != ".."
                                        && driver.resolve_child(&panel_path, &e.name) == tp
                                })
                                .map(|e| vec![(e.name.clone(), e.kind.clone())])
                                .unwrap_or_default()
                        } else if panel.selection.is_empty() {
                            panel
                                .entries
                                .get(panel.cursor)
                                .filter(|e| e.name != "..")
                                .map(|e| vec![(e.name.clone(), e.kind.clone())])
                                .unwrap_or_default()
                        } else {
                            panel
                                .selection
                                .iter()
                                .filter_map(|&i| panel.entries.get(i))
                                .filter(|e| e.name != "..")
                                .map(|e| (e.name.clone(), e.kind.clone()))
                                .collect()
                        };
                        Ok((driver, panel_path, raw))
                    }
                    None => Err(format!(
                        "Source driver not available for source_id={}",
                        panel.source_id
                    )),
                },
                None => Err(format!("Panel not found: {}", request.source_panel_id)),
            }
        }; // state lock released here — path resolution happens below without the lock
        match raw_inputs {
            Err(message) => {
                eprintln!("[bro] input resolution failed: {message}");
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
                return;
            }
            Ok((driver, panel_path, raw)) => {
                if raw.is_empty() {
                    eprintln!("[bro] no valid entries in selection");
                    let _ = ctx
                        .tx
                        .send(ServerMessage::Error {
                            timestamp: now(),
                            code: "BRO_LAUNCH_FAILED".to_string(),
                            message: "No valid input entries are selected.".to_string(),
                            recoverable: true,
                            extra: Some(serde_json::json!({
                                "requestId": parsed.request_id,
                                "broId": parsed.bro_id,
                            })),
                        })
                        .await;
                    return;
                }
                let items = tokio::task::spawn_blocking(move || {
                    raw.into_iter()
                        .map(|(name, kind)| BroInputItem {
                            path: driver.resolve_child(&panel_path, &name),
                            name,
                            kind,
                        })
                        .collect::<Vec<BroInputItem>>()
                })
                .await
                .unwrap_or_default();
                items
            }
        }
    };

    journal_resolved_bro_launch(&parsed, &bro, &inputs);

    match bro.option.protocol {
        BroProtocol::Fnf => {
            if matches!(bro.launch, LaunchRecipe::BuiltInCopyPaths) {
                let what = copied_paths_noun(inputs.len());
                let text = inputs
                    .iter()
                    .map(|i| i.path.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let platform = {
                    let state_guard = ctx.state.lock().await;
                    state_guard.platform.clone()
                };
                let result =
                    tokio::task::spawn_blocking(move || platform.write_text_clipboard(&text))
                        .await
                        .unwrap_or_else(|_| Err("Clipboard task panicked".to_string()));
                let message = match result {
                    Ok(()) => ServerMessage::TextClipboardWritten {
                        timestamp: now(),
                        what,
                    },
                    Err(message) => ServerMessage::Error {
                        timestamp: now(),
                        code: "BRO_LAUNCH_FAILED".to_string(),
                        message,
                        recoverable: true,
                        extra: Some(serde_json::json!({
                            "requestId": parsed.request_id,
                            "broId": parsed.bro_id,
                        })),
                    },
                };
                let _ = ctx.tx.send(message).await;
                return;
            }
            if matches!(bro.launch, LaunchRecipe::BuiltInKillProc) {
                let result = tokio::task::spawn_blocking(move || {
                    use sysinfo::{Pid, System};
                    let mut sys = System::new_all();
                    sys.refresh_processes();
                    for input in &inputs {
                        let pid = input
                            .name
                            .splitn(2, " - ")
                            .next()
                            .and_then(|chain| chain.split(" | ").last())
                            .and_then(|s| s.trim().parse::<u32>().ok());
                        if let Some(pid) = pid {
                            if let Some(proc) = sys.process(Pid::from_u32(pid)) {
                                #[cfg(unix)]
                                {
                                    let _ = proc.kill_with(sysinfo::Signal::Kill);
                                }
                                #[cfg(windows)]
                                {
                                    let _ = proc.kill();
                                }
                            }
                        }
                    }
                })
                .await;
                if result.is_err() {
                    let _ = ctx
                        .tx
                        .send(ServerMessage::Error {
                            timestamp: now(),
                            code: "BRO_LAUNCH_FAILED".to_string(),
                            message: "Kill task panicked".to_string(),
                            recoverable: true,
                            extra: Some(serde_json::json!({
                                "requestId": parsed.request_id,
                                "broId": parsed.bro_id,
                            })),
                        })
                        .await;
                }
                return;
            }
            let bro_name = bro.option.name.clone();
            let first_path = inputs.first().map(|i| i.path.clone()).unwrap_or_default();
            let launch_result = {
                let output = parsed.output.clone();
                let params = parsed.params.clone();
                tokio::task::spawn_blocking(move || {
                    launch_bro(&inputs, &bro, output.as_deref(), &params)
                })
                .await
                .unwrap_or_else(|e| Err(format!("Launch task panicked: {e}")))
            };
            match launch_result {
                Err(message) => {
                    eprintln!("[bro] launch failed: {message}");
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
                Ok(child) => {
                    let pid = child.id();
                    let name = bro_name;
                    let label = std::path::Path::new(&first_path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(&first_path)
                        .to_string();
                    let started_at = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let entry = ChildProcessEntry {
                        pid,
                        name,
                        label,
                        started_at,
                    };

                    let patch = {
                        let mut sg = ctx.state.lock().await;
                        sg.child_processes.insert(pid, entry);
                        build_procs_patch(&sg.child_processes)
                    };
                    let _ = ctx.tx.send(patch).await;

                    // Monitor the child; remove from state when it exits.
                    let state_clone = ctx.state.clone();
                    let tx_clone = ctx.tx.clone();
                    tokio::spawn(async move {
                        let _ = tokio::task::spawn_blocking(move || {
                            let mut c = child;
                            let _ = c.wait();
                        })
                        .await;
                        let patch = {
                            let mut sg = state_clone.lock().await;
                            sg.child_processes.remove(&pid);
                            build_procs_patch(&sg.child_processes)
                        };
                        let _ = tx_clone.send(patch).await;
                    });
                }
            }
        }
        BroProtocol::Fp => {
            if let Err(message) =
                launch_fp_bro(request, inputs, bro, parsed.output, parsed.params, ctx).await
            {
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

fn journal_resolved_bro_launch(
    request: &LaunchBroArgs,
    bro: &ResolvedBro,
    inputs: &[BroInputItem],
) {
    let inputs = inputs
        .iter()
        .map(|input| {
            serde_json::json!({
                "path": input.path,
                "name": input.name,
                "kind": input.kind,
            })
        })
        .collect::<Vec<_>>();
    let params = bro
        .option
        .params
        .iter()
        .filter_map(|declared| {
            request.params.get(&declared.id).map(|value| {
                let value = match declared.kind {
                    BroLaunchInputKind::Password => "<redacted>".to_string(),
                    BroLaunchInputKind::Int | BroLaunchInputKind::Bool => value.clone(),
                };
                (declared.id.clone(), Value::String(value))
            })
        })
        .collect::<serde_json::Map<String, Value>>();

    crate::journal::record(
        "intent.command.v1",
        Some(&request.request_id),
        None,
        None,
        None,
        serde_json::json!({
            "command": "LaunchBro",
            "requestId": request.request_id,
            "bro": {
                "id": bro.option.id,
                "name": bro.option.name,
                "version": bro.option.version,
                "source": bro.option.source.as_str(),
                "protocol": bro.option.protocol.as_str(),
            },
            "inputs": inputs,
            "inputCount": inputs.len(),
            "output": request.output,
            "params": params,
            "launchConvention": bro.journal_launch_convention(),
        }),
    );
}

async fn launch_fp_bro(
    request: crate::state::BroRequest,
    inputs: Vec<BroInputItem>,
    bro: ResolvedBro,
    output: Option<String>,
    launch_params: HashMap<String, String>,
    ctx: &mut IntentContext<'_>,
) -> Result<(), String> {
    let output_path = bro
        .output_path(output.as_deref())?
        .ok_or_else(|| "This bro does not declare an output file path.".to_string())?;
    validate_output_path(&output_path)?;
    validate_bro_params(&bro, &launch_params)?;

    let output_exists = Path::new(&output_path).exists();
    let source_path = first_input_parent(&inputs).unwrap_or_else(|| {
        inputs
            .first()
            .map(|input| input.path.clone())
            .unwrap_or_default()
    });
    let file_schemas = bro_input_files(&inputs);

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
                total_files: inputs.len(),
                total_bytes_total: 0,
                background: false,
                supports_pause: false,
                result_path: Some(output_path.clone()),
                result_panel_id: request.result_panel_id.clone(),
                request_id: None,
                search_source_id: None,
                search_params: None,
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
                    inputs: inputs.clone(),
                    params: launch_params,
                    bro,
                }),
                source_panel: request.source_panel_id,
                dest_panel: request.result_panel_id,
                source_path,
                dest_path: Some(output_path.clone()),
                state: initial_state.clone(),
                progress: FileOpProgress {
                    total_files: inputs.len(),
                    completed_files: 0,
                    current_file: None,
                    status_text: None,
                    current_file_bytes_total: 0,
                    current_file_bytes_copied: 0,
                    total_bytes_total: 0,
                    total_bytes_copied: 0,
                    current_file_percent: None,
                    overall_percent: None,
                },
                decision: None,
                error: None,
                background: false,
                paused: false,
                handle: None,
                cancel_tx,
                pause_tx,
                skip_current_file: Arc::new(AtomicBool::new(false)),
                keep_partial_files_on_cancel: Arc::new(AtomicBool::new(false)),
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
                files: None,
                progress: None,
                search_progress: None,
                decision: Some(crate::ws::protocol::ConflictDecisionSchema {
                    kind: Some("conflict".to_string()),
                    source: output_path.clone(),
                    destination: output_path,
                    message: None,
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

async fn launch_sync_bro(
    request: crate::state::BroRequest,
    ctx: &mut IntentContext<'_>,
) -> Result<(), String> {
    let (local_panel, remote_panel, local_root, remote_root, local_driver, remote_driver) = {
        let state_guard = ctx.state.lock().await;
        sync_roots_for_panels(
            &state_guard,
            &request.source_panel_id,
            request.result_panel_id.as_deref(),
        )?
    };
    let report_path = local_driver.resolve_child(&local_root, SYNC_REPORT_NAME);
    let job_id = {
        let mut state_guard = ctx.state.lock().await;
        state_guard.job_store.next_job_id()
    };

    let _ = ctx
        .tx
        .send(ServerMessage::JobStarted {
            timestamp: now(),
            job: JobStartSchema {
                job_id: job_id.clone(),
                kind: "sync-bro".to_string(),
                op_type: "Sync Bro".to_string(),
                files: Vec::new(),
                source_panel: local_panel.clone(),
                source_path: local_root.clone(),
                dest_panel: Some(remote_panel.clone()),
                dest_path: Some(remote_root.clone()),
                initial_state: "Running".to_string(),
                total_files: 0,
                total_bytes_total: 0,
                background: false,
                supports_pause: false,
                // The report file is written only on demand (SaveSyncReport), so
                // no result path exists yet; results are shown in the modal.
                result_path: None,
                result_panel_id: Some(local_panel.clone()),
                request_id: None,
                search_source_id: None,
                search_params: None,
            },
        })
        .await;

    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (pause_tx, _pause_rx) = watch::channel(false);
    let handle = tokio::spawn(run_sync_bro(
        job_id.clone(),
        local_panel.clone(),
        remote_panel.clone(),
        local_root.clone(),
        remote_root.clone(),
        local_driver,
        remote_driver,
        ctx.tx.clone(),
        cancel_rx,
    ));

    let mut state_guard = ctx.state.lock().await;
    state_guard.job_store.jobs.insert(
        job_id.clone(),
        AppJob {
            job_id,
            kind: JobKind::SyncBro(SyncBroJobMeta {
                local_panel: local_panel.clone(),
                remote_panel: remote_panel.clone(),
                local_root: local_root.clone(),
                remote_root: remote_root.clone(),
                report_path,
            }),
            source_panel: local_panel,
            dest_panel: Some(remote_panel),
            source_path: local_root,
            dest_path: Some(remote_root),
            state: JobState::Running,
            progress: FileOpProgress::default(),
            decision: None,
            error: None,
            background: false,
            paused: false,
            handle: Some(handle),
            cancel_tx,
            pause_tx,
            skip_current_file: Arc::new(AtomicBool::new(false)),
            keep_partial_files_on_cancel: Arc::new(AtomicBool::new(false)),
        },
    );

    Ok(())
}

async fn launch_hash_bro(
    request: crate::state::BroRequest,
    bro: &ResolvedBro,
    parsed: &LaunchBroArgs,
    ctx: &mut IntentContext<'_>,
) -> Result<(), String> {
    use crate::jobs::check_hashes::{run_check_hashes, HashCheckOptions};
    use crate::jobs::generate_hashes::{run_generate_hashes, HashGenerateOptions};
    use crate::jobs::{HashCheckBroJobMeta, HashGenerateBroJobMeta};
    let generate = matches!(bro.launch, LaunchRecipe::BuiltInGenerateHashes);
    let title = if generate {
        "Generate hashes"
    } else {
        "Check hashes"
    };
    validate_bro_params(bro, &parsed.params)?;
    let location = if generate {
        &request.hash_generate_location
    } else {
        &request.hash_check_location
    };
    let (source_id, folder) = location
        .as_ref()
        .ok_or_else(|| format!("{title} requires a local filesystem folder."))?;
    let (panel, driver) = {
        let state = ctx.state.lock().await;
        let panel = state
            .panels
            .get(&request.source_panel_id)
            .ok_or_else(|| "Hash panel is unavailable.".to_string())?;
        if &panel.source_id != source_id || &panel.path != folder {
            return Err("The hash folder changed. Open Pass to Bro again.".into());
        }
        let driver = state
            .drivers
            .get(source_id)
            .filter(|driver| driver.is_local())
            .cloned()
            .ok_or_else(|| "Local hash source is unavailable.".to_string())?;
        (panel.clone(), driver)
    };
    let worker_driver = driver.clone();
    let target_path = request.target_path.clone();
    let inputs = tokio::task::spawn_blocking(move || {
        resolve_inputs(&panel, worker_driver.as_ref(), target_path.as_deref())
    })
    .await
    .map_err(|e| format!("Cannot resolve hash inputs: {e}"))??;
    journal_resolved_bro_launch(parsed, bro, &inputs);
    let source_panel = request.source_panel_id.clone();
    let files = bro_input_files(&inputs);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (pause_tx, _) = watch::channel(false);
    let job_id = {
        let mut state = ctx.state.lock().await;
        let job_id = state.job_store.next_job_id();
        state.job_store.jobs.insert(
            job_id.clone(),
            AppJob {
                job_id: job_id.clone(),
                kind: if generate {
                    JobKind::HashGenerateBro(HashGenerateBroJobMeta { result: None })
                } else {
                    JobKind::HashCheckBro(HashCheckBroJobMeta {
                        result: None,
                        report_saved_path: None,
                        report_error: None,
                    })
                },
                source_panel: source_panel.clone(),
                dest_panel: None,
                source_path: folder.clone(),
                dest_path: None,
                state: JobState::Running,
                progress: FileOpProgress::default(),
                decision: None,
                error: None,
                background: false,
                paused: false,
                handle: None,
                cancel_tx,
                pause_tx,
                skip_current_file: Arc::new(AtomicBool::new(false)),
                keep_partial_files_on_cancel: Arc::new(AtomicBool::new(false)),
            },
        );
        job_id
    };
    let _ = ctx
        .tx
        .send(ServerMessage::JobStarted {
            timestamp: now(),
            job: JobStartSchema {
                job_id: job_id.clone(),
                kind: if generate {
                    "hash-generate-bro"
                } else {
                    "hash-check-bro"
                }
                .into(),
                op_type: title.into(),
                files,
                source_panel,
                source_path: folder.clone(),
                dest_panel: None,
                dest_path: None,
                initial_state: "Running".into(),
                total_files: inputs.len(),
                total_bytes_total: 0,
                background: false,
                supports_pause: false,
                result_path: None,
                result_panel_id: None,
                request_id: Some(parsed.request_id.clone()),
                search_source_id: None,
                search_params: None,
            },
        })
        .await;
    let handle = if generate {
        tokio::spawn(run_generate_hashes(
            job_id.clone(),
            HashGenerateOptions {
                driver,
                folder: folder.clone(),
                inputs,
                overwrite_existing: parsed
                    .params
                    .get("overwriteExisting")
                    .is_some_and(|value| value.trim() == "true"),
            },
            ctx.tx.clone(),
            cancel_rx,
        ))
    } else {
        tokio::spawn(run_check_hashes(
            job_id.clone(),
            HashCheckOptions {
                driver,
                folder: folder.clone(),
                inputs,
                save_result: parsed
                    .params
                    .get("saveResult")
                    .is_some_and(|value| value.trim() == "true"),
            },
            ctx.tx.clone(),
            cancel_rx,
        ))
    };
    if let Some(job) = ctx.state.lock().await.job_store.jobs.get_mut(&job_id) {
        job.handle = Some(handle);
    }
    Ok(())
}

/// Inspect every selected entry without doing I/O or relying on the ranking sample.
fn has_selected_b3(panel: &crate::state::Panel, target_path: Option<&str>) -> bool {
    has_selected_hash_input(panel, target_path, true)
}

fn has_selected_hash_input(
    panel: &crate::state::Panel,
    target_path: Option<&str>,
    sidecars: bool,
) -> bool {
    let is_b3 = |entry: &crate::ws::protocol::EntrySchema| {
        entry.kind == "file" && entry.name.ends_with(".b3") == sidecars
    };
    if let Some(path) = target_path {
        let name = path.rsplit(['/', '\\']).next().unwrap_or("");
        panel
            .entries
            .iter()
            .any(|entry| entry.name == name && is_b3(entry))
    } else if panel.selection.is_empty() {
        panel.entries.get(panel.cursor).is_some_and(is_b3)
    } else {
        panel
            .selection
            .iter()
            .filter_map(|index| panel.entries.get(*index))
            .any(is_b3)
    }
}

async fn launch_fedup_bro(
    request: crate::state::BroRequest,
    launch_params: HashMap<String, String>,
    ctx: &mut IntentContext<'_>,
) -> Result<(), String> {
    let inputs = request
        .resolved_inputs
        .clone()
        .ok_or_else(|| "Fedup Bro requires a resolved global selection.".to_string())?;
    if inputs.is_empty() {
        return Err("No valid input entries are selected.".to_string());
    }

    let source_path = if inputs.len() == 1 {
        inputs[0].path.clone()
    } else {
        format!("{} items", inputs.len())
    };
    let report_dir = inputs
        .first()
        .and_then(|input| {
            let path = Path::new(&input.path);
            if input.kind == "directory" {
                Some(path.to_path_buf())
            } else {
                path.parent().map(Path::to_path_buf)
            }
        })
        .or_else(|| first_input_parent(&inputs).map(PathBuf::from));
    let report_path = report_dir
        .map(|dir| dir.join(FEDUP_REPORT_NAME).to_string_lossy().to_string())
        .unwrap_or_else(|| FEDUP_REPORT_NAME.to_string());
    let file_schemas = bro_input_files(&inputs);
    let source_panel_id = request.source_panel_id.clone();
    let scan_options = fedup_scan_options(&launch_params, ctx).await?;
    let job_id = {
        let mut state_guard = ctx.state.lock().await;
        state_guard.job_store.next_job_id()
    };
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (pause_tx, _pause_rx) = watch::channel(false);

    {
        let mut state_guard = ctx.state.lock().await;
        state_guard.job_store.jobs.insert(
            job_id.clone(),
            AppJob {
                job_id: job_id.clone(),
                kind: JobKind::FedupBro(FedupBroJobMeta {
                    inputs: inputs.clone(),
                    report_path: report_path.clone(),
                    result: None,
                }),
                source_panel: source_panel_id.clone(),
                dest_panel: None,
                source_path: source_path.clone(),
                dest_path: None,
                state: JobState::Running,
                progress: FileOpProgress::default(),
                decision: None,
                error: None,
                background: false,
                paused: false,
                handle: None,
                cancel_tx,
                pause_tx,
                skip_current_file: Arc::new(AtomicBool::new(false)),
                keep_partial_files_on_cancel: Arc::new(AtomicBool::new(false)),
            },
        );
    }

    let _ = ctx
        .tx
        .send(ServerMessage::JobStarted {
            timestamp: now(),
            job: JobStartSchema {
                job_id: job_id.clone(),
                kind: "fedup-bro".to_string(),
                op_type: "Fedup Bro".to_string(),
                files: file_schemas,
                source_panel: source_panel_id.clone(),
                source_path: source_path.clone(),
                dest_panel: None,
                dest_path: None,
                initial_state: "Running".to_string(),
                total_files: inputs.len(),
                total_bytes_total: 0,
                background: false,
                supports_pause: false,
                result_path: None,
                result_panel_id: Some(source_panel_id),
                request_id: None,
                search_source_id: None,
                search_params: None,
            },
        })
        .await;

    let handle = tokio::spawn(run_fedup_bro(
        job_id.clone(),
        inputs.clone(),
        scan_options,
        Arc::clone(ctx.state),
        ctx.tx.clone(),
        cancel_rx,
    ));

    let mut state_guard = ctx.state.lock().await;
    if let Some(job) = state_guard.job_store.jobs.get_mut(&job_id) {
        job.handle = Some(handle);
    }

    Ok(())
}

async fn fedup_scan_options(
    launch_params: &HashMap<String, String>,
    ctx: &IntentContext<'_>,
) -> Result<crate::jobs::fedup::FedupScanOptions, String> {
    let use_default_ignores = launch_params
        .get("useDefaultIgnores")
        .map(|value| matches!(value.trim(), "1" | "true" | "yes" | "on"))
        .unwrap_or(true);
    let include_patterns = crate::jobs::fedup::parse_fedup_patterns(
        launch_params
            .get("includePatterns")
            .map_or("", String::as_str),
    );
    let mut exclude_patterns = crate::jobs::fedup::parse_fedup_patterns(
        launch_params
            .get("excludePatterns")
            .map_or("", String::as_str),
    );
    if !include_patterns.is_empty() && !exclude_patterns.is_empty() {
        return Err("Fedup include and exclude patterns are mutually exclusive.".to_string());
    }
    if use_default_ignores {
        let default_patterns = {
            let state_guard = ctx.state.lock().await;
            crate::jobs::fedup::parse_fedup_patterns(
                &state_guard.settings.general.fedup_default_ignore_patterns,
            )
        };
        exclude_patterns.extend(default_patterns);
    }
    Ok(crate::jobs::fedup::FedupScanOptions::new(
        exclude_patterns,
        include_patterns,
    ))
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
        FpDecisionOutcome::Cancel => {
            let _ = tx
                .send(ServerMessage::JobEvent {
                    timestamp: now(),
                    job_id: job_id.to_string(),
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
    Ok(true)
}

async fn start_fp_job(
    job_id: String,
    state: &crate::state::SharedState,
    tx: &tokio::sync::mpsc::Sender<ServerMessage>,
) -> Result<(), String> {
    let (bro, inputs, output_path, params, cancel_rx) = {
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
            meta.output_path.clone(),
            meta.params.clone(),
            job.cancel_tx.subscribe(),
        )
    };

    let handle = tokio::spawn(run_fp_bro(
        job_id.clone(),
        bro,
        inputs,
        output_path,
        params,
        tx.clone(),
        cancel_rx,
    ));

    let mut state_guard = state.lock().await;
    if let Some(job) = state_guard.job_store.jobs.get_mut(&job_id) {
        job.handle = Some(handle);
    }
    Ok(())
}

/// Toast noun for the Copy Paths bro, e.g. "full path" or "3 full paths".
fn copied_paths_noun(count: usize) -> String {
    if count == 1 {
        "full path".to_string()
    } else {
        format!("{count} full paths")
    }
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

fn validate_bro_params(bro: &ResolvedBro, params: &HashMap<String, String>) -> Result<(), String> {
    for param in &bro.option.params {
        let value = params
            .get(&param.id)
            .map(|value| value.trim())
            .unwrap_or("");
        if param.required && value.is_empty() {
            return Err(format!("Missing required input: {}", param.label));
        }
        if value.is_empty() {
            continue;
        }
        match param.kind {
            BroLaunchInputKind::Int => {
                let parsed = value
                    .parse::<i32>()
                    .map_err(|_| format!("{} must be a whole number.", param.label))?;
                if let Some(min) = param.min_value {
                    if parsed < min {
                        return Err(format!("{} must be at least {}.", param.label, min));
                    }
                }
                if let Some(max) = param.max_value {
                    if parsed > max {
                        return Err(format!("{} must be at most {}.", param.label, max));
                    }
                }
            }
            BroLaunchInputKind::Password => {}
            BroLaunchInputKind::Bool => {
                if !matches!(value, "true" | "false") {
                    return Err(format!("{} must be true or false.", param.label));
                }
            }
        }
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
    state
        .panels
        .keys()
        .find(|id| id.as_str() != panel_id)
        .cloned()
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
            source_path: None,
        })
        .collect()
}

fn resolve_ranking_inputs(
    panel: &crate::state::Panel,
    driver: &dyn crate::vfs::VfsDriver,
    target_path: Option<&str>,
) -> Result<Vec<BroInputItem>, String> {
    if target_path.is_some() || panel.selection.len() <= RANKING_SAMPLE_LIMIT {
        return resolve_inputs(panel, driver, target_path);
    }

    use rand::seq::SliceRandom;
    let mut rng = rand::thread_rng();
    let inputs: Vec<BroInputItem> = panel
        .selection
        .choose_multiple(&mut rng, RANKING_SAMPLE_LIMIT)
        .copied()
        .filter_map(|index| entry_at_abs_index(panel, index))
        .filter(|entry| entry.name != "..")
        .map(|entry| bro_input_for_entry(panel, driver, entry))
        .collect();

    if inputs.is_empty() {
        Err("No valid input entries are selected.".to_string())
    } else {
        Ok(inputs)
    }
}

const RANKING_SAMPLE_LIMIT: usize = 100;

fn resolve_global_selection_inputs(
    state: &crate::state::AppState,
    panel: &crate::state::Panel,
    target_path: Option<&str>,
) -> Result<Vec<BroInputItem>, String> {
    if !panel.path.is_empty() {
        return Err("Pass to Bro is only available from the global selections root.".to_string());
    }

    let selection_names: Vec<String> = if let Some(target_path) = target_path {
        let target = target_path.trim_start_matches('/');
        panel
            .entries
            .iter()
            .find(|entry| entry.name != ".." && entry.name == target)
            .map(|entry| vec![entry.name.clone()])
            .unwrap_or_default()
    } else if panel.selection.is_empty() {
        panel
            .entries
            .get(panel.cursor)
            .filter(|entry| entry.name != "..")
            .map(|entry| vec![entry.name.clone()])
            .unwrap_or_default()
    } else {
        panel
            .selection
            .iter()
            .filter_map(|index| panel.entries.get(*index))
            .filter(|entry| entry.name != "..")
            .map(|entry| entry.name.clone())
            .collect()
    };

    if selection_names.len() != 1 {
        return Err("Choose one global selection to pass to Bro.".to_string());
    }

    let store = state
        .global_selections
        .lock()
        .map_err(|_| "Global selection store is unavailable.".to_string())?;
    let selection = store
        .selection_by_name(&selection_names[0])
        .ok_or_else(|| "Global selection not found.".to_string())?;
    let inputs = selection
        .items
        .iter()
        .map(|item| BroInputItem {
            path: item.path.clone(),
            name: item.name.clone(),
            kind: item.kind.clone(),
        })
        .collect::<Vec<_>>();
    if inputs.is_empty() {
        Err("No valid input entries are selected.".to_string())
    } else {
        Ok(inputs)
    }
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
    // panel.entries holds the full directory listing on the backend (not just the
    // scroll window); abs_index is a direct index into that full array.
    panel.entries.get(abs_index)
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

async fn kill_process(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(pid) = args.get("pid").and_then(Value::as_u64).map(|v| v as u32) else {
        return;
    };
    let patch = {
        let mut sg = ctx.state.lock().await;
        sg.child_processes.remove(&pid);
        build_procs_patch(&sg.child_processes)
    };
    let _ = ctx.tx.send(patch).await;
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGTERM);
    }
}

fn build_procs_patch(procs: &std::collections::HashMap<u32, ChildProcessEntry>) -> ServerMessage {
    let list: Vec<&ChildProcessEntry> = procs.values().collect();
    ServerMessage::StatePatch {
        timestamp: now(),
        domain: "procs".to_string(),
        entity_id: None,
        version: 0,
        patch: serde_json::json!({ "procs": list }),
    }
}

#[cfg(test)]
mod tests {
    use super::copied_paths_noun;

    #[test]
    fn generation_boolean_option_validates_wire_values() {
        let bro = crate::bro::list_bros(&[], None, None, false, true)
            .into_iter()
            .find(|bro| bro.option.id == "builtin:generate-hashes")
            .unwrap();
        assert!(super::validate_bro_params(&bro, &std::collections::HashMap::new()).is_ok());
        for value in ["true", "false"] {
            assert!(super::validate_bro_params(
                &bro,
                &std::collections::HashMap::from([("overwriteExisting".into(), value.into())])
            )
            .is_ok());
        }
        assert!(super::validate_bro_params(
            &bro,
            &std::collections::HashMap::from([("overwriteExisting".into(), "yes".into())])
        )
        .is_err());
    }

    #[test]
    fn check_hashes_boolean_option_validates_wire_values() {
        let bro = crate::bro::list_bros(&[], None, None, true, false)
            .into_iter()
            .find(|bro| bro.option.id == "builtin:check-hashes")
            .unwrap();
        assert!(super::validate_bro_params(&bro, &std::collections::HashMap::new()).is_ok());
        for value in ["true", "false"] {
            assert!(super::validate_bro_params(
                &bro,
                &std::collections::HashMap::from([("saveResult".into(), value.into())])
            )
            .is_ok());
        }
        for value in ["yes", "1", "TRUE"] {
            assert!(super::validate_bro_params(
                &bro,
                &std::collections::HashMap::from([("saveResult".into(), value.into())])
            )
            .is_err());
        }
    }

    #[test]
    fn copied_paths_noun_counts_paths() {
        assert_eq!(copied_paths_noun(1), "full path");
        assert_eq!(copied_paths_noun(3), "3 full paths");
    }
}
