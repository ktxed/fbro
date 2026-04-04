use std::collections::HashSet;

use serde_json::Value;

use crate::ws::intents::{arg_bool, arg_i64, arg_str, arg_u64, arg_usize_vec};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{
    build_state_snapshot, rescan_panel, resolve_panel_id, selected_entries_for_panel,
    selected_names_for, send_window, IntentContext,
};

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "RefreshPanels" => refresh_panels(ctx).await,
        "SetSort" => set_sort(scope, args, ctx).await,
        "SetFilter" => set_filter(scope, args, ctx).await,
        "SetFilterMode" => set_filter_mode(scope, args, ctx).await,
        "MoveCursor" => move_cursor(scope, args, ctx).await,
        "SetSelection" => set_selection(scope, args, ctx).await,
        "SelectAtIndex" => select_at_index(scope, args, ctx).await,
        "DeselectAtIndex" => deselect_at_index(scope, args, ctx).await,
        "ToggleSelectionAtIndex" => toggle_selection_at_index(scope, args, ctx).await,
        "ToggleSelection" => toggle_selection(scope, ctx).await,
        "SelectAll" => select_all(scope, ctx).await,
        "ClearSelection" => clear_selection(scope, ctx).await,
        "InvertSelection" => invert_selection(scope, ctx).await,
        "MoveCursorWithSelect" => move_cursor_with_select(scope, args, ctx).await,
        "MoveCursorBy" => move_cursor_by(scope, args, ctx).await,
        "MoveCursorToEdge" => move_cursor_to_edge(scope, args, ctx).await,
        "SetCursor" => set_cursor(scope, args, ctx).await,
        "CalculateSize" => calculate_size(scope, args, ctx).await,
        "CancelSizeCalculation" => cancel_size_calculation(scope, args, ctx).await,
        "RequestWindow" => request_window(scope, args, ctx).await,
        "SwitchFocus" => switch_focus(args, ctx).await,
        "RequestStateSnapshot" => request_state_snapshot(ctx).await,
        _ => {}
    }
}

struct SortArgs {
    by: String,
    order: String,
}

impl SortArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            by: arg_str(args, "by").unwrap_or("name").to_string(),
            order: arg_str(args, "order").unwrap_or("asc").to_string(),
        }
    }
}

struct FilterArgs {
    value: String,
}

impl FilterArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            value: arg_str(args, "value").unwrap_or("").trim().to_string(),
        }
    }
}

struct FilterModeArgs {
    active: bool,
}

impl FilterModeArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            active: arg_bool(args, "active", false),
        }
    }
}

struct DirectionArgs {
    direction: String,
}

impl DirectionArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            direction: arg_str(args, "direction").unwrap_or("down").to_string(),
        }
    }
}

struct SelectionArgs {
    indices: Vec<usize>,
}

impl SelectionArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            indices: arg_usize_vec(args, "indices"),
        }
    }
}

struct IndexArgs {
    index: usize,
}

impl IndexArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            index: arg_u64(args, "index", 0) as usize,
        }
    }
}

struct DeltaArgs {
    delta: i64,
}

impl DeltaArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            delta: arg_i64(args, "delta", 0),
        }
    }
}

struct EdgeArgs {
    edge: String,
}

impl EdgeArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            edge: arg_str(args, "edge").unwrap_or("first").to_string(),
        }
    }
}

struct NameArgs {
    name: String,
}

impl NameArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            name: arg_str(args, "name").unwrap_or("").to_string(),
        }
    }
}

struct WindowArgs {
    start: usize,
    count: usize,
}

impl WindowArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            start: arg_u64(args, "start", 0) as usize,
            count: arg_u64(args, "count", 200) as usize,
        }
    }
}

struct FocusArgs {
    to: Option<String>,
}

impl FocusArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            to: arg_str(args, "to").map(String::from),
        }
    }
}

async fn refresh_panels(ctx: &mut IntentContext<'_>) {
    let panel_ids_and_names: Vec<(String, Option<Vec<String>>)> = {
        let state_guard = ctx.state.lock().await;
        state_guard
            .panels
            .keys()
            .map(|id| (id.clone(), selected_names_for(&state_guard, id)))
            .collect()
    };
    for (panel_id, names) in panel_ids_and_names {
        rescan_panel(
            &panel_id,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            None,
            names,
            ctx.dir_size_queue,
        )
        .await;
    }
}

async fn set_sort(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = SortArgs::from_args(args);
    let names = {
        let mut state_guard = ctx.state.lock().await;
        let names = selected_names_for(&state_guard, &panel_id);
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            panel.sort = crate::ws::protocol::SortSpec {
                by: parsed.by,
                order: parsed.order,
            };
        }
        names
    };
    rescan_panel(
        &panel_id,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        None,
        names,
        ctx.dir_size_queue,
    )
    .await;
}

async fn set_filter(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = FilterArgs::from_args(args);
    let (names, changed) = {
        let mut state_guard = ctx.state.lock().await;
        let names = selected_names_for(&state_guard, &panel_id);
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let next_filter = if parsed.value.is_empty() {
                None
            } else {
                Some(crate::ws::protocol::FilterSpec {
                    mode: "glob".to_string(),
                    value: parsed.value,
                })
            };
            let changed = panel.filter != next_filter;
            panel.filter = next_filter.clone();
            (names, changed)
        } else {
            return;
        }
    };
    if !changed {
        return;
    }
    rescan_panel(
        &panel_id,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        None,
        names,
        ctx.dir_size_queue,
    )
    .await;
}

async fn set_filter_mode(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = FilterModeArgs::from_args(args);
    let version = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            if panel.filter_mode_active == parsed.active {
                return;
            }
            panel.filter_mode_active = parsed.active;
            panel.version += 1;
            panel.version
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "filterModeActive": parsed.active }),
    )
    .await;
}

async fn move_cursor(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = DirectionArgs::from_args(args);
    let (new_cursor, version, needs_window) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let len = panel.entries.len();
            if len == 0 {
                return;
            }
            let cursor = match parsed.direction.as_str() {
                "up" => panel.cursor.saturating_sub(1),
                _ => (panel.cursor + 1).min(len - 1),
            };
            panel.cursor = cursor;
            panel.version += 1;
            let needs_window = panel.windowed
                && (cursor < panel.window_start
                    || cursor >= panel.window_start + panel.window_count);
            (cursor, panel.version, needs_window)
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id.clone(),
        version,
        serde_json::json!({ "cursor": new_cursor }),
    )
    .await;
    if needs_window {
        let count = ctx
            .state
            .lock()
            .await
            .panels
            .get(&panel_id)
            .map(|panel| panel.window_count)
            .unwrap_or(200);
        let start = new_cursor.saturating_sub(count / 2);
        send_window(&panel_id, start, count, ctx.state, ctx.tx).await;
    }
}

async fn set_selection(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = SelectionArgs::from_args(args);
    let (version, selected_entries) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            panel.selection = parsed.indices.clone();
            panel.version += 1;
            (panel.version, selected_entries_for_panel(panel))
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "selection": parsed.indices, "selectedEntries": selected_entries }),
    )
    .await;
}

async fn select_at_index(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = IndexArgs::from_args(args);
    let (new_selection, version, selected_entries) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            if parsed.index >= panel.entries.len() || panel.selection.contains(&parsed.index) {
                return;
            }
            panel.selection.push(parsed.index);
            panel.version += 1;
            (
                panel.selection.clone(),
                panel.version,
                selected_entries_for_panel(panel),
            )
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "selection": new_selection, "selectedEntries": selected_entries }),
    )
    .await;
}

async fn deselect_at_index(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = IndexArgs::from_args(args);
    let (new_selection, version, selected_entries) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let before = panel.selection.len();
            panel.selection.retain(|&index| index != parsed.index);
            if panel.selection.len() == before {
                return;
            }
            panel.version += 1;
            (
                panel.selection.clone(),
                panel.version,
                selected_entries_for_panel(panel),
            )
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "selection": new_selection, "selectedEntries": selected_entries }),
    )
    .await;
}

async fn toggle_selection_at_index(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = IndexArgs::from_args(args);
    let (new_selection, version, selected_entries) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            if parsed.index >= panel.entries.len() {
                return;
            }
            if let Some(position) = panel
                .selection
                .iter()
                .position(|&index| index == parsed.index)
            {
                panel.selection.remove(position);
            } else {
                panel.selection.push(parsed.index);
            }
            panel.version += 1;
            (
                panel.selection.clone(),
                panel.version,
                selected_entries_for_panel(panel),
            )
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "selection": new_selection, "selectedEntries": selected_entries }),
    )
    .await;
}

async fn toggle_selection(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (new_selection, version, selected_entries) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let cursor = panel.cursor;
            if let Some(position) = panel.selection.iter().position(|&index| index == cursor) {
                panel.selection.remove(position);
            } else {
                panel.selection.push(cursor);
            }
            panel.version += 1;
            (
                panel.selection.clone(),
                panel.version,
                selected_entries_for_panel(panel),
            )
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "selection": new_selection, "selectedEntries": selected_entries }),
    )
    .await;
}

async fn select_all(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (selection, version, selected_entries) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let selection: Vec<usize> = (0..panel.entries.len()).collect();
            panel.selection = selection.clone();
            panel.version += 1;
            (selection, panel.version, selected_entries_for_panel(panel))
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "selection": selection, "selectedEntries": selected_entries }),
    )
    .await;
}

async fn clear_selection(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let version = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            panel.selection.clear();
            panel.version += 1;
            panel.version
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "selection": [], "selectedEntries": [] }),
    )
    .await;
}

async fn invert_selection(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (selection, version, selected_entries) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let selected: HashSet<usize> = panel.selection.iter().copied().collect();
            let selection: Vec<usize> = (0..panel.entries.len())
                .filter(|index| !selected.contains(index))
                .collect();
            panel.selection = selection.clone();
            panel.version += 1;
            (selection, panel.version, selected_entries_for_panel(panel))
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "selection": selection, "selectedEntries": selected_entries }),
    )
    .await;
}

async fn move_cursor_with_select(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = DirectionArgs::from_args(args);
    let (new_cursor, new_selection, version, needs_window, selected_entries) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let len = panel.entries.len();
            if len == 0 {
                return;
            }
            let cursor = match parsed.direction.as_str() {
                "up" => panel.cursor.saturating_sub(1),
                _ => (panel.cursor + 1).min(len - 1),
            };
            let previous = panel.cursor;
            panel.cursor = cursor;
            if panel.selection.contains(&cursor) {
                panel
                    .selection
                    .retain(|&index| index != cursor && index != previous);
            } else {
                if !panel.selection.contains(&previous) {
                    panel.selection.push(previous);
                }
                panel.selection.push(cursor);
            }
            panel.version += 1;
            let needs_window = panel.windowed
                && (cursor < panel.window_start
                    || cursor >= panel.window_start + panel.window_count);
            (
                cursor,
                panel.selection.clone(),
                panel.version,
                needs_window,
                selected_entries_for_panel(panel),
            )
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id.clone(),
        version,
        serde_json::json!({
            "cursor": new_cursor,
            "selection": new_selection,
            "selectedEntries": selected_entries
        }),
    )
    .await;
    if needs_window {
        let count = ctx
            .state
            .lock()
            .await
            .panels
            .get(&panel_id)
            .map(|panel| panel.window_count)
            .unwrap_or(200);
        let start = new_cursor.saturating_sub(count / 2);
        send_window(&panel_id, start, count, ctx.state, ctx.tx).await;
    }
}

async fn move_cursor_by(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = DeltaArgs::from_args(args);
    let (new_cursor, version, needs_window) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let len = panel.entries.len();
            if len == 0 {
                return;
            }
            let cursor = (panel.cursor as i64 + parsed.delta).clamp(0, (len - 1) as i64) as usize;
            if cursor == panel.cursor {
                return;
            }
            panel.cursor = cursor;
            panel.version += 1;
            let needs_window = panel.windowed
                && (cursor < panel.window_start
                    || cursor >= panel.window_start + panel.window_count);
            (cursor, panel.version, needs_window)
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id.clone(),
        version,
        serde_json::json!({ "cursor": new_cursor }),
    )
    .await;
    if needs_window {
        let count = ctx
            .state
            .lock()
            .await
            .panels
            .get(&panel_id)
            .map(|panel| panel.window_count)
            .unwrap_or(200);
        let start = new_cursor.saturating_sub(count / 2);
        send_window(&panel_id, start, count, ctx.state, ctx.tx).await;
    }
}

async fn move_cursor_to_edge(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = EdgeArgs::from_args(args);
    let (new_cursor, version, needs_window) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let len = panel.entries.len();
            if len == 0 {
                return;
            }
            let cursor = if parsed.edge == "last" { len - 1 } else { 0 };
            panel.cursor = cursor;
            panel.version += 1;
            let needs_window = panel.windowed
                && (cursor < panel.window_start
                    || cursor >= panel.window_start + panel.window_count);
            (cursor, panel.version, needs_window)
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id.clone(),
        version,
        serde_json::json!({ "cursor": new_cursor }),
    )
    .await;
    if needs_window {
        let count = ctx
            .state
            .lock()
            .await
            .panels
            .get(&panel_id)
            .map(|panel| panel.window_count)
            .unwrap_or(200);
        let start = new_cursor.saturating_sub(count / 2);
        send_window(&panel_id, start, count, ctx.state, ctx.tx).await;
    }
}

async fn set_cursor(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = IndexArgs::from_args(args);
    let (new_cursor, version) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let cursor = parsed.index.min(panel.entries.len().saturating_sub(1));
            panel.cursor = cursor;
            panel.version += 1;
            (cursor, panel.version)
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "cursor": new_cursor }),
    )
    .await;
}

async fn calculate_size(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = NameArgs::from_args(args);
    let full_path = {
        let state_guard = ctx.state.lock().await;
        state_guard.panels.get(&panel_id).and_then(|panel| {
            panel
                .entries
                .iter()
                .find(|entry| entry.name == parsed.name && entry.kind == "directory")
                .map(|_| format!("{}/{}", panel.path.trim_end_matches('/'), parsed.name))
        })
    };
    if let Some(path) = full_path {
        ctx.dir_size_queue.invalidate_cache(&path).await;
        ctx.dir_size_queue
            .enqueue(
                panel_id,
                parsed.name,
                path,
                std::sync::Arc::clone(ctx.state),
                ctx.tx.clone(),
            )
            .await;
    }
}

async fn cancel_size_calculation(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = NameArgs::from_args(args);
    ctx.dir_size_queue
        .cancel(&panel_id, &parsed.name, ctx.tx)
        .await;

    let (calculating_size, version) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            panel.calculating_size.remove(&parsed.name);
            panel.version += 1;
            let mut calculating_size: Vec<String> =
                panel.calculating_size.iter().cloned().collect();
            calculating_size.sort();
            (calculating_size, panel.version)
        } else {
            return;
        }
    };

    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "calculatingSize": calculating_size }),
    )
    .await;
}

async fn request_window(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = WindowArgs::from_args(args);
    send_window(&panel_id, parsed.start, parsed.count, ctx.state, ctx.tx).await;
}

async fn switch_focus(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = FocusArgs::from_args(args);
    if let Some(target) = parsed.to {
        let version = {
            let mut state_guard = ctx.state.lock().await;
            state_guard.focus = target.clone();
            1u64
        };
        let _ = ctx
            .tx
            .send(ServerMessage::StatePatch {
                timestamp: now(),
                domain: "focus".to_string(),
                entity_id: None,
                version,
                patch: serde_json::json!({ "focus": target }),
            })
            .await;
    }
}

async fn request_state_snapshot(ctx: &mut IntentContext<'_>) {
    let dir_size_tasks = ctx.dir_size_queue.snapshot().await;
    let state_guard = ctx.state.lock().await;
    let _ = ctx
        .tx
        .send(build_state_snapshot(&state_guard, dir_size_tasks))
        .await;
}

async fn send_panel_patch(
    ctx: &IntentContext<'_>,
    panel_id: String,
    version: u64,
    patch: serde_json::Value,
) {
    let _ = ctx
        .tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id),
            version,
            patch,
        })
        .await;
}
