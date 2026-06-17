use std::path::Path;

use serde_json::Value;

use crate::vfs::global_selection::{GlobalSelectionItem, SOURCE_ID};
use crate::ws::intents::arg_str;
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{
    build_global_selection_patch, build_sources_json, ensure_global_selection_source, rescan_panel,
    resolve_panel_id, IntentContext,
};

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "AddToGlobalSelection" => add_to_global_selection(scope, args, ctx).await,
        "RemoveGlobalSelectionItems" => remove_global_selection_items(args, ctx).await,
        _ => {}
    }
}

struct AddArgs {
    panel_id: String,
    target_path: Option<String>,
    selection_id: Option<String>,
    name: Option<String>,
}

impl AddArgs {
    fn from_args(scope: &Value, args: &Value) -> Self {
        Self {
            panel_id: arg_str(args, "panelId")
                .map(str::to_string)
                .or_else(|| scope.as_str().map(str::to_string))
                .unwrap_or_default(),
            target_path: arg_str(args, "targetPath").map(str::to_string),
            selection_id: arg_str(args, "selectionId").map(str::to_string),
            name: arg_str(args, "name").map(str::to_string),
        }
    }
}

struct RemoveArgs {
    panel_id: String,
    selection_name: String,
    paths: Vec<String>,
}

impl RemoveArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            panel_id: arg_str(args, "panelId").unwrap_or("").to_string(),
            selection_name: arg_str(args, "selectionName").unwrap_or("").to_string(),
            paths: args
                .get("paths")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

async fn add_to_global_selection(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = AddArgs::from_args(scope, args);
    let panel_id = if parsed.panel_id.is_empty() {
        resolve_panel_id(scope, ctx.state).await
    } else {
        parsed.panel_id
    };

    let items = {
        let state_guard = ctx.state.lock().await;
        let panel = match state_guard.panels.get(&panel_id) {
            Some(panel) => panel,
            None => {
                send_error(ctx, "Global selection source panel is unavailable.").await;
                return;
            }
        };
        let source = match state_guard.sources.get(&panel.source_id) {
            Some(source) => source,
            None => {
                send_error(ctx, "Panel source is unavailable.").await;
                return;
            }
        };
        if source.kind != "filesystem" || panel.shadow_source.is_some() {
            send_error(
                ctx,
                "Only local filesystem entries can be added to a global selection.",
            )
            .await;
            return;
        }
        let Some(driver) = state_guard.drivers.get(&panel.source_id) else {
            send_error(ctx, "Panel source driver is unavailable.").await;
            return;
        };
        let indices: Vec<usize> = if let Some(target_path) = parsed.target_path.as_deref() {
            panel
                .entries
                .iter()
                .position(|entry| {
                    entry.name != ".."
                        && driver.resolve_child(&panel.path, &entry.name) == target_path
                })
                .map(|index| vec![index])
                .unwrap_or_default()
        } else if panel.selection.is_empty() {
            vec![panel.cursor]
        } else {
            panel.selection.clone()
        };

        let mut items = Vec::new();
        for index in indices {
            let Some(entry) = panel.entries.get(index) else {
                continue;
            };
            if entry.name == ".." {
                continue;
            }
            let path = driver.resolve_child(&panel.path, &entry.name);
            if !Path::new(&path).is_absolute() {
                send_error(ctx, "Only absolute local filesystem paths can be added.").await;
                return;
            }
            items.push(GlobalSelectionItem {
                path,
                name: entry.name.clone(),
                kind: entry.kind.clone(),
                size: entry.size,
                mtime: entry.mtime.clone(),
            });
        }
        items
    };

    if items.is_empty() {
        send_error(ctx, "No valid entries are selected.").await;
        return;
    }

    let mutation: Result<(Vec<serde_json::Value>, ServerMessage), String> = {
        let mut state_guard = ctx.state.lock().await;
        let store_result = {
            let store_arc = std::sync::Arc::clone(&state_guard.global_selections);
            let result = match store_arc.lock() {
                Ok(mut store) => match store
                    .ensure_selection(parsed.selection_id.as_deref(), parsed.name.as_deref())
                {
                    Ok(selection_id) => store.add_items(&selection_id, items).map(|_| ()),
                    Err(message) => Err(message),
                },
                Err(_) => Err("Global selection store is unavailable.".to_string()),
            };
            result
        };
        match store_result {
            Ok(()) => {
                ensure_global_selection_source(&mut state_guard);
                Ok((
                    build_sources_json(&state_guard),
                    build_global_selection_patch(&state_guard),
                ))
            }
            Err(message) => Err(message),
        }
    };
    let (sources_json, selection_patch) = match mutation {
        Ok(value) => value,
        Err(message) => {
            send_error(ctx, &message).await;
            return;
        }
    };

    send_sources_patch(ctx, sources_json).await;
    let _ = ctx.tx.send(selection_patch).await;
    rescan_global_selection_panels(ctx).await;
}

async fn remove_global_selection_items(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = RemoveArgs::from_args(args);
    if parsed.selection_name.is_empty() || parsed.paths.is_empty() {
        send_error(ctx, "No global selection references were selected.").await;
        return;
    }

    let mutation: Result<(Vec<serde_json::Value>, ServerMessage), String> = {
        let mut state_guard = ctx.state.lock().await;
        let store_result = {
            let store_arc = std::sync::Arc::clone(&state_guard.global_selections);
            let result = match store_arc.lock() {
                Ok(mut store) => store.remove_items_by_paths(&parsed.selection_name, &parsed.paths),
                Err(_) => Err("Global selection store is unavailable.".to_string()),
            };
            result
        };
        match store_result {
            Ok(_) => {
                ensure_global_selection_source(&mut state_guard);
                Ok((
                    build_sources_json(&state_guard),
                    build_global_selection_patch(&state_guard),
                ))
            }
            Err(message) => Err(message),
        }
    };
    let (sources_json, selection_patch) = match mutation {
        Ok(value) => value,
        Err(message) => {
            send_error(ctx, &message).await;
            return;
        }
    };

    let _ = parsed.panel_id;
    send_sources_patch(ctx, sources_json).await;
    let _ = ctx.tx.send(selection_patch).await;
    rescan_global_selection_panels(ctx).await;
}

pub(crate) async fn rescan_global_selection_panels(ctx: &mut IntentContext<'_>) {
    let panel_ids = {
        let state_guard = ctx.state.lock().await;
        state_guard
            .panels
            .iter()
            .filter(|(_, panel)| panel.source_id == SOURCE_ID)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
    };
    for panel_id in panel_ids {
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
}

async fn send_sources_patch(ctx: &IntentContext<'_>, sources_json: Vec<serde_json::Value>) {
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

async fn send_error(ctx: &IntentContext<'_>, message: &str) {
    let _ = ctx
        .tx
        .send(ServerMessage::Error {
            timestamp: now(),
            code: "GLOBAL_SELECTION_FAILED".to_string(),
            message: message.to_string(),
            recoverable: true,
            extra: None,
        })
        .await;
}
