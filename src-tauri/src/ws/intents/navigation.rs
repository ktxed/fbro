use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::vfs::VfsDriver;
use crate::watcher::WatcherCommand;
use crate::ws::intents::arg_str;
use crate::ws::protocol::{now, OpenWithAppSchema, ServerMessage};
use crate::ws::runtime::{
    build_sources_json, navigate_to, rescan_panel, resolve_panel_id, IntentContext,
};

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "OpenEntry" => open_entry(scope, ctx).await,
        "NavigateHome" => navigate_home(scope, ctx).await,
        "NavigateSourceRoot" => navigate_source_root(scope, ctx).await,
        "NavigatePanelTo" => navigate_panel_to(args, ctx).await,
        "OpenFile" => open_file(scope, ctx).await,
        "OpenTerminal" => open_terminal(scope, ctx).await,
        "NavigateToSource" => navigate_to_source(scope, args, ctx).await,
        "MountSource" => mount_source(scope, args, ctx).await,
        "NavigateUp" => navigate_up(scope, ctx).await,
        "RevealInFileManager" => reveal_in_file_manager(args, ctx).await,
        "ListOpenWithApps" => list_open_with_apps(args, ctx).await,
        "LaunchOpenWithApp" => launch_open_with_app(args, ctx).await,
        _ => {}
    }
}

struct NavigatePanelArgs {
    panel_id: String,
    path: String,
    cursor_hint: Option<String>,
}

impl NavigatePanelArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            panel_id: arg_str(args, "panelId").unwrap_or("").to_string(),
            path: arg_str(args, "path").unwrap_or("").to_string(),
            cursor_hint: arg_str(args, "cursorHint").map(String::from),
        }
    }
}

struct SourceArgs {
    source_id: String,
}

impl SourceArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            source_id: arg_str(args, "sourceId").unwrap_or("").to_string(),
        }
    }
}

struct RevealArgs {
    path: String,
    kind: String,
}

struct ListOpenWithArgs {
    request_id: String,
    path: String,
    kind: String,
}

impl ListOpenWithArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            request_id: arg_str(args, "requestId").unwrap_or("").to_string(),
            path: arg_str(args, "path").unwrap_or("").to_string(),
            kind: arg_str(args, "kind").unwrap_or("file").to_string(),
        }
    }
}

struct LaunchOpenWithArgs {
    request_id: String,
    app_id: String,
}

impl LaunchOpenWithArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            request_id: arg_str(args, "requestId").unwrap_or("").to_string(),
            app_id: arg_str(args, "appId").unwrap_or("").to_string(),
        }
    }
}

impl RevealArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            path: arg_str(args, "path").unwrap_or("").to_string(),
            kind: arg_str(args, "kind").unwrap_or("file").to_string(),
        }
    }
}

async fn open_entry(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (entry_kind, new_path) = {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&panel_id) {
            if let Some(entry) = panel.entries.get(panel.cursor) {
                let driver = state_guard.drivers.get(&panel.source_id).unwrap();
                (
                    entry.kind.clone(),
                    driver.resolve_child(&panel.path, &entry.name),
                )
            } else {
                return;
            }
        } else {
            return;
        }
    };

    if entry_kind == "directory" {
        navigate_to(
            &panel_id,
            &new_path,
            None,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            ctx.dir_size_queue,
            ctx.watcher_cmd_tx,
        )
        .await;
    }
}

async fn navigate_home(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let home = ctx.state.lock().await.platform.home_dir();
    navigate_to(
        &panel_id,
        &home,
        None,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        ctx.dir_size_queue,
        ctx.watcher_cmd_tx,
    )
    .await;
}

async fn navigate_source_root(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let target = {
        let state_guard = ctx.state.lock().await;
        let panel = match state_guard.panels.get(&panel_id) {
            Some(panel) => panel,
            None => return,
        };
        let current_path = panel.path.as_str();
        let root = state_guard
            .sources
            .values()
            .filter(|source| current_path.starts_with(&source.root_path))
            .max_by_key(|source| source.root_path.len())
            .map(|source| source.root_path.clone())
            .unwrap_or_else(|| "/".to_string());
        let home = state_guard.platform.home_dir();
        if home.starts_with(&root) && current_path.starts_with(&home) {
            home
        } else {
            root
        }
    };
    navigate_to(
        &panel_id,
        &target,
        None,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        ctx.dir_size_queue,
        ctx.watcher_cmd_tx,
    )
    .await;
}

async fn navigate_panel_to(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = NavigatePanelArgs::from_args(args);
    if !parsed.panel_id.is_empty() && !parsed.path.is_empty() {
        let requested_path = parsed.path.trim().to_string();
        let resolved_target = {
            let state_guard = ctx.state.lock().await;
            let panel = match state_guard.panels.get(&parsed.panel_id) {
                Some(panel) => panel,
                None => return,
            };
            let driver = match state_guard.drivers.get(&panel.source_id) {
                Some(driver) => driver,
                None => return,
            };

            if !driver.exists(&requested_path) {
                Err("Path does not exist.".to_string())
            } else {
                let target_path = Path::new(&requested_path);
                if target_path.is_dir() {
                    Ok((requested_path.clone(), parsed.cursor_hint.clone()))
                } else if target_path.is_file() {
                    if let Some(parent) = target_path
                        .parent()
                        .map(|path| path.to_string_lossy().to_string())
                        .filter(|path| !path.is_empty())
                    {
                        let cursor_hint = target_path
                            .file_name()
                            .map(|name| name.to_string_lossy().to_string());
                        Ok((parent, cursor_hint))
                    } else {
                        Err("File has no parent directory.".to_string())
                    }
                } else {
                    Err("Path is not a directory.".to_string())
                }
            }
        };

        let (validated_path, cursor_hint) = match resolved_target {
            Ok(target) => target,
            Err(message) => {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        timestamp: now(),
                        code: "NAVIGATE_FAILED".to_string(),
                        message,
                        recoverable: true,
                        extra: Some(serde_json::json!({ "panelId": parsed.panel_id })),
                    })
                    .await;
                return;
            }
        };

        let _ = ctx
            .tx
            .send(ServerMessage::NavigateOk {
                timestamp: now(),
                panel_id: parsed.panel_id.clone(),
            })
            .await;

        navigate_to(
            &parsed.panel_id,
            &validated_path,
            cursor_hint,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            ctx.dir_size_queue,
            ctx.watcher_cmd_tx,
        )
        .await;
    }
}

async fn open_file(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (path, entry, platform) = {
        let state_guard = ctx.state.lock().await;
        let panel = match state_guard.panels.get(&panel_id) {
            Some(panel) => panel,
            None => return,
        };
        (
            panel.path.clone(),
            panel.entries.get(panel.cursor).cloned(),
            Arc::clone(&state_guard.platform),
        )
    };
    if let Some(entry) = entry {
        if entry.kind != "directory" {
            let full_path = format!("{}/{}", path.trim_end_matches('/'), entry.name);
            platform.open_file(&full_path);
        }
    }
}

async fn open_terminal(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (path, platform) = {
        let state_guard = ctx.state.lock().await;
        let panel = match state_guard.panels.get(&panel_id) {
            Some(panel) => panel,
            None => return,
        };
        (panel.path.clone(), Arc::clone(&state_guard.platform))
    };
    platform.open_terminal(&path);
}

async fn reveal_in_file_manager(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = RevealArgs::from_args(args);
    if parsed.path.is_empty() {
        return;
    }

    let platform = {
        let state_guard = ctx.state.lock().await;
        Arc::clone(&state_guard.platform)
    };
    platform.reveal_in_file_manager(&parsed.path, &parsed.kind);
}

async fn list_open_with_apps(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = ListOpenWithArgs::from_args(args);
    if parsed.request_id.is_empty() || parsed.path.is_empty() {
        return;
    }

    let platform = {
        let state_guard = ctx.state.lock().await;
        Arc::clone(&state_guard.platform)
    };

    match platform.list_open_with_apps(&parsed.path, &parsed.kind) {
        Ok(apps) => {
            let schemas = apps
                .iter()
                .map(|app| OpenWithAppSchema {
                    id: app.option.id.clone(),
                    label: app.option.label.clone(),
                    detail: app.option.detail.clone(),
                    is_default: app.option.is_default,
                    source: app.option.source.clone(),
                })
                .collect();

            {
                let mut state_guard = ctx.state.lock().await;
                state_guard.open_with_requests.insert(
                    parsed.request_id.clone(),
                    crate::state::OpenWithRequest {
                        path: parsed.path.clone(),
                        kind: parsed.kind.clone(),
                        apps,
                    },
                );
                if state_guard.open_with_requests.len() > 32 {
                    if let Some(oldest) = state_guard.open_with_requests.keys().next().cloned() {
                        state_guard.open_with_requests.remove(&oldest);
                    }
                }
            }

            let _ = ctx
                .tx
                .send(ServerMessage::OpenWithOptionsReady {
                    timestamp: now(),
                    request_id: parsed.request_id,
                    apps: schemas,
                })
                .await;
        }
        Err(message) => {
            let _ = ctx
                .tx
                .send(ServerMessage::OpenWithOptionsFailed {
                    timestamp: now(),
                    request_id: parsed.request_id,
                    message,
                })
                .await;
        }
    }
}

async fn launch_open_with_app(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = LaunchOpenWithArgs::from_args(args);
    if parsed.request_id.is_empty() || parsed.app_id.is_empty() {
        return;
    }

    let (platform, request) = {
        let mut state_guard = ctx.state.lock().await;
        let request = match state_guard.open_with_requests.remove(&parsed.request_id) {
            Some(request) => request,
            None => return,
        };
        (Arc::clone(&state_guard.platform), request)
    };

    let Some(app) = request
        .apps
        .iter()
        .find(|app| app.option.id == parsed.app_id)
        .cloned()
    else {
        return;
    };

    if let Err(message) = platform.launch_open_with(&request.path, &app) {
        let _ = ctx
            .tx
            .send(ServerMessage::Error {
                timestamp: now(),
                code: "OPEN_WITH_LAUNCH_FAILED".to_string(),
                message,
                recoverable: true,
                extra: Some(serde_json::json!({
                    "requestId": parsed.request_id,
                    "appId": parsed.app_id,
                    "path": request.path,
                })),
            })
            .await;
    }
}

async fn navigate_to_source(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = SourceArgs::from_args(args);

    let root_path = {
        let state_guard = ctx.state.lock().await;
        state_guard
            .sources
            .get(&parsed.source_id)
            .map(|source| source.root_path.clone())
    };

    if let Some(root_path) = root_path {
        let target_path = {
            let mut state_guard = ctx.state.lock().await;
            if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
                panel
                    .source_paths
                    .insert(panel.source_id.clone(), panel.path.clone());

                let target_path = panel
                    .source_paths
                    .get(&parsed.source_id)
                    .filter(|path| Path::new(path.as_str()).is_dir())
                    .cloned()
                    .unwrap_or(root_path);

                panel.source_id = parsed.source_id;
                panel.path = target_path.clone();
                panel.cursor = 0;
                panel.selection.clear();
                panel.filter = None;
                panel.filter_mode_active = false;
                target_path
            } else {
                return;
            }
        };

        let _ = ctx
            .watcher_cmd_tx
            .send(WatcherCommand::Watch {
                panel_id: panel_id.clone(),
                path: PathBuf::from(&target_path),
            })
            .await;

        rescan_panel(
            &panel_id,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            None,
            None,
            ctx.dir_size_queue,
        )
        .await;
    }
}

async fn mount_source(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = SourceArgs::from_args(args);

    let device_path = {
        let state_guard = ctx.state.lock().await;
        state_guard
            .sources
            .get(&parsed.source_id)
            .and_then(|source| source.device_path.clone())
    };

    if let Some(device_path) = device_path {
        match ctx.volume_watcher.mount(&device_path).await {
            Ok(mount_point) => {
                eprintln!("[fbro] Mounted {} at {}", parsed.source_id, mount_point);
                let sources_json = {
                    let mut state_guard = ctx.state.lock().await;
                    if let Some(source) = state_guard.sources.get_mut(&parsed.source_id) {
                        source.mounted = true;
                        source.root_path = mount_point.clone();
                        source.supports = vec![
                            "ListEntries".to_string(),
                            "OpenFolder".to_string(),
                            "CopySelection".to_string(),
                            "MoveSelection".to_string(),
                            "DeleteSelection".to_string(),
                            "InspectItem".to_string(),
                        ];
                    }
                    if !state_guard.drivers.contains_key(&parsed.source_id) {
                        let driver: Arc<dyn VfsDriver> = Arc::new(crate::vfs::local::LocalFsDriver);
                        state_guard.drivers.insert(parsed.source_id.clone(), driver);
                    }
                    build_sources_json(&state_guard)
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

                {
                    let mut state_guard = ctx.state.lock().await;
                    if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
                        panel
                            .source_paths
                            .insert(panel.source_id.clone(), panel.path.clone());
                        panel.source_id = parsed.source_id.clone();
                        panel.path = mount_point.clone();
                        panel.cursor = 0;
                        panel.selection.clear();
                        panel.filter = None;
                        panel.filter_mode_active = false;
                    }
                }

                let _ = ctx
                    .watcher_cmd_tx
                    .send(WatcherCommand::Watch {
                        panel_id: panel_id.clone(),
                        path: PathBuf::from(&mount_point),
                    })
                    .await;

                rescan_panel(
                    &panel_id,
                    ctx.state,
                    ctx.tx,
                    ctx.scan_handles,
                    None,
                    None,
                    ctx.dir_size_queue,
                )
                .await;
            }
            Err(error) => {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        timestamp: now(),
                        code: "MOUNT_FAILED".to_string(),
                        message: error,
                        recoverable: true,
                        extra: None,
                    })
                    .await;
            }
        }
    } else {
        let _ = ctx
            .tx
            .send(ServerMessage::Error {
                timestamp: now(),
                code: "MOUNT_FAILED".to_string(),
                message: "Source has no device path for mounting".to_string(),
                recoverable: true,
                extra: None,
            })
            .await;
    }
}

async fn navigate_up(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (current_path, parent_path) = {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&panel_id) {
            let driver = state_guard.drivers.get(&panel.source_id).unwrap();
            (Some(panel.path.clone()), driver.parent_path(&panel.path))
        } else {
            (None, None)
        }
    };
    if let Some(current_path) = current_path {
        if let Some(parent) = parent_path {
            if parent != current_path {
                let dir_name = current_path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .map(str::to_string);
                navigate_to(
                    &panel_id,
                    &parent,
                    dir_name,
                    ctx.state,
                    ctx.tx,
                    ctx.scan_handles,
                    ctx.dir_size_queue,
                    ctx.watcher_cmd_tx,
                )
                .await;
            }
        }
    }
}
