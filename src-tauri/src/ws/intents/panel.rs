use std::collections::HashSet;

use serde_json::Value;

use crate::ws::intents::{arg_bool, arg_i64, arg_str, arg_u64, arg_usize_vec};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{
    build_state_snapshot, handle_volumes_changed, rescan_panel, resolve_panel_id,
    selected_entries_for_panel, selected_names_for, send_window, IntentContext,
};

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "RefreshPanels" => refresh_panels(ctx).await,
        "Refresh" => refresh(scope, ctx).await,
        "SetSort" => set_sort(scope, args, ctx).await,
        "SetFilter" => set_filter(scope, args, ctx).await,
        "SetFilterMode" => set_filter_mode(scope, args, ctx).await,
        "ToggleHidden" => toggle_hidden(scope, args, ctx).await,
        "MoveCursor" => move_cursor(scope, args, ctx).await,
        "SetSelection" => set_selection(scope, args, ctx).await,
        "SelectAtIndex" => select_at_index(scope, args, ctx).await,
        "DeselectAtIndex" => deselect_at_index(scope, args, ctx).await,
        "ToggleSelectionAtIndex" => toggle_selection_at_index(scope, args, ctx).await,
        "ToggleSelection" => toggle_selection(scope, args, ctx).await,
        "SelectAll" => select_all(scope, ctx).await,
        "ClearSelection" => clear_selection(scope, ctx).await,
        "InvertSelection" => invert_selection(scope, ctx).await,
        "SelectByMask" => select_by_mask(scope, args, true, ctx).await,
        "DeselectByMask" => select_by_mask(scope, args, false, ctx).await,
        "ExtendSelectionTo" => extend_selection_to(scope, args, ctx).await,
        "MoveCursorWithSelect" => move_cursor_with_select(scope, args, ctx).await,
        "MoveCursorBy" => move_cursor_by(scope, args, ctx).await,
        "MoveCursorToEdge" => move_cursor_to_edge(scope, args, ctx).await,
        "SetCursor" => set_cursor(scope, args, ctx).await,
        "CalculateSize" => calculate_size(scope, args, ctx).await,
        "ToggleDirectorySizes" => toggle_directory_sizes(scope, ctx).await,
        "CancelSizeCalculation" => cancel_size_calculation(scope, args, ctx).await,
        "RequestWindow" => request_window(scope, args, ctx).await,
        "SwitchFocus" => switch_focus(args, ctx).await,
        "SetPanelViewMode" => set_panel_view_mode(scope, args, ctx).await,
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

/// Refresh the focused panel's listing and re-query mounted volumes / free
/// space. Triggered by the `panel.refresh` command (default key `r`).
async fn refresh(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (cursor_hint, selection_names) = {
        let state_guard = ctx.state.lock().await;
        let names = selected_names_for(&state_guard, &panel_id);
        let hint = state_guard
            .panels
            .get(&panel_id)
            .and_then(|panel| panel.entries.get(panel.cursor).map(|e| e.name.clone()));
        (hint, names)
    };
    rescan_panel(
        &panel_id,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        cursor_hint,
        selection_names,
        ctx.dir_size_queue,
        ctx.watcher_cmd_tx,
    )
    .await;

    let volumes = ctx.volume_watcher.list_volumes().await;
    handle_volumes_changed(
        volumes,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        ctx.watcher_cmd_tx,
        ctx.dir_size_queue,
    )
    .await;
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
            ctx.watcher_cmd_tx,
        )
        .await;
    }
}

async fn set_sort(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = SortArgs::from_args(args);
    if !crate::state::location_settings::valid_sort(&parsed.by, &parsed.order) {
        return;
    }
    let sort = crate::ws::protocol::SortSpec {
        by: parsed.by,
        order: parsed.order,
    };
    let (rescans, persisted, count_patch) = {
        let mut state_guard = ctx.state.lock().await;
        let remember_per_location = state_guard.settings.general.remember_sort_per_location;
        let location_key = remember_per_location
            .then(|| {
                state_guard.panels.get(&panel_id).and_then(|panel| {
                    state_guard
                        .drivers
                        .get(&panel.source_id)
                        .and_then(|driver| {
                            crate::state::location_settings::key_for(&panel.source_id, &panel.path, driver.is_local())
                        })
                })
            })
            .flatten();
        let affected_panel_ids = if let Some(location) = location_key.as_deref() {
            state_guard
                .panels
                .iter()
                .filter(|(_, panel)| {
                    panel.tab_kind == crate::state::TabKind::Listing
                        && state_guard
                            .drivers
                            .get(&panel.source_id)
                            .and_then(|driver| crate::state::location_settings::key_for(&panel.source_id, &panel.path, driver.is_local()))
                            .as_deref() == Some(location)
                })
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
        } else if state_guard.panels.contains_key(&panel_id) {
            vec![panel_id.clone()]
        } else {
            Vec::new()
        };
        let rescans = affected_panel_ids
            .iter()
            .map(|id| (id.clone(), selected_names_for(&state_guard, id)))
            .collect::<Vec<_>>();
        for id in &affected_panel_ids {
            if let Some(panel) = state_guard.panels.get_mut(id) {
                panel.sort = sort.clone();
            }
        }
        let persisted = location_key.map(|location| {
            let settings = crate::state::location_settings::LocationSettings {
                sort: Some(sort.clone()),
            };
            let is_new = !state_guard.location_settings.contains_key(&location);
            state_guard
                .location_settings
                .insert(location.clone(), settings.clone());
            (location, settings, is_new)
        });
        let count_patch = persisted
            .as_ref()
            .and_then(|(_, _, is_new)| is_new.then_some((
                state_guard.location_settings.len(),
                state_guard.gui_senders.clone(),
            )));
        (rescans, persisted, count_patch)
    };
    for (affected_panel_id, names) in rescans {
        rescan_panel(
            &affected_panel_id,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            None,
            names,
            ctx.dir_size_queue,
            ctx.watcher_cmd_tx,
        )
        .await;
    }

    if let Some((location, settings, _)) = persisted {
        let result = tokio::task::spawn_blocking(move || {
            crate::state::location_settings::save(&location, &settings)
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("[fbro] Failed to save location settings: {error}"),
            Err(error) => eprintln!("[fbro] Location settings task failed: {error}"),
        }
    }
    if let Some((location_settings_count, gui_senders)) = count_patch {
        let patch = crate::ws::protocol::ServerMessage::StatePatch {
            timestamp: crate::ws::protocol::now(),
            domain: "settings".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({
                "general": { "locationSettingsCount": location_settings_count }
            }),
        };
        for sender in &gui_senders {
            let _ = sender.send(patch.clone()).await;
        }
        let _ = ctx.tx.send(patch).await;
    }
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
        ctx.watcher_cmd_tx,
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

async fn toggle_hidden(scope: &Value, _args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (new_value, version) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            panel.show_hidden = !panel.show_hidden;
            panel.version += 1;
            (panel.show_hidden, panel.version)
        } else {
            return;
        }
    };
    let selection_names = {
        let state_guard = ctx.state.lock().await;
        selected_names_for(&state_guard, &panel_id)
    };
    send_panel_patch(
        ctx,
        panel_id.clone(),
        version,
        serde_json::json!({ "showHidden": new_value }),
    )
    .await;

    rescan_panel(
        &panel_id,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        None,
        selection_names,
        ctx.dir_size_queue,
        ctx.watcher_cmd_tx,
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

    // Kick off background prefetch without blocking the message loop.
    // Spawning detaches the state-lock acquisition so the select! loop can
    // immediately process the next intent (and flush the StatePatch above).
    let state = std::sync::Arc::clone(ctx.state);
    let prefetch = std::sync::Arc::clone(ctx.viewer_prefetch);
    tokio::spawn(async move {
        let prefetch_info = {
            let state_guard = state.lock().await;
            // Only warm the prefetch cache while at least one viewer (embedded
            // or external window) is open — plain panel navigation shouldn't
            // pay disk I/O for files the user hasn't asked to view.
            if state_guard.active_viewers == 0 {
                return;
            }
            let general = &state_guard.settings.general;
            let image_cap = if general.ludicrous_mode {
                (general.max_image_size_mb as usize).saturating_mul(1024 * 1024)
            } else {
                crate::jobs::viewer_prefetch::PREFETCH_IMAGE_CAP
            };
            if let Some(panel) = state_guard.panels.get(&panel_id) {
                let source_id = panel.source_id.clone();
                let driver = state_guard.drivers.get(&source_id).cloned();
                if let Some(drv) = driver {
                    if drv.is_local() {
                        let neighbors = neighbors_for_prefetch(
                            &panel.entries,
                            panel.cursor,
                            &panel.path,
                            &source_id,
                        );
                        Some((neighbors, source_id, drv, image_cap))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };
        if let Some((neighbors, source_id, driver, image_cap)) = prefetch_info {
            prefetch.schedule_neighbors(panel_id, neighbors, source_id, driver, image_cap);
        }
    });
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

/// Return a size request only when Space newly selects an unknown, idle folder.
fn toggle_entry_selection(
    entries: &[crate::ws::protocol::EntrySchema],
    cursor: usize,
    selection: &mut Vec<usize>,
    calculating: &HashSet<String>,
    calculate_size: bool,
) -> Option<String> {
    let entry = entries.get(cursor)?;
    if let Some(position) = selection.iter().position(|&index| index == cursor) {
        selection.remove(position);
        return None;
    }
    selection.push(cursor);
    (calculate_size
        && entry.kind == "directory"
        && entry.size.is_none()
        && !calculating.contains(&entry.name))
    .then(|| entry.name.clone())
}

fn directory_size_actions(
    entries: &[crate::ws::protocol::EntrySchema],
    selection: &[usize],
    calculating: &HashSet<String>,
) -> Vec<(String, bool)> {
    let indices: Vec<usize> = if selection.is_empty() {
        (0..entries.len()).collect()
    } else {
        selection.to_vec()
    };
    indices
        .into_iter()
        .filter_map(|index| {
            let entry = entries.get(index)?;
            (entry.kind == "directory")
                .then(|| (entry.name.clone(), calculating.contains(&entry.name)))
        })
        .collect()
}

async fn toggle_selection(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (new_selection, version, selected_entries, size_target, panel_epoch) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            if panel.entries.get(panel.cursor).is_none() {
                return;
            }
            let size_target = toggle_entry_selection(
                &panel.entries,
                panel.cursor,
                &mut panel.selection,
                &panel.calculating_size,
                arg_bool(args, "calculateSize", false),
            );
            panel.version += 1;
            (
                panel.selection.clone(),
                panel.version,
                selected_entries_for_panel(panel),
                size_target,
                panel.panel_epoch,
            )
        } else {
            return;
        }
    };
    send_panel_patch(
        ctx,
        panel_id.clone(),
        version,
        serde_json::json!({ "panelEpoch": panel_epoch, "selection": new_selection, "selectedEntries": selected_entries }),
    )
    .await;
    if let Some(name) = size_target {
        calculate_entry_size(&panel_id, &name, Some(panel_epoch), ctx).await;
    }
}

fn compile_selection_mask(mask: &str) -> Option<regex::Regex> {
    let patterns: Vec<_> = mask
        .split_whitespace()
        .map(|pattern| {
            let pattern = if pattern == "*.*" { "*" } else { pattern };
            regex::escape(pattern)
                .replace("\\*", ".*")
                .replace("\\?", ".")
        })
        .collect();
    if patterns.is_empty() {
        return None;
    }
    regex::RegexBuilder::new(&format!(r"\A(?:{})\z", patterns.join("|")))
        .case_insensitive(true)
        .dot_matches_new_line(true)
        .build()
        .ok()
}

fn selection_mask_matches(entries: &[crate::ws::protocol::EntrySchema], mask: &str) -> Vec<usize> {
    let Some(matcher) = compile_selection_mask(mask) else {
        return vec![];
    };
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let is_file = entry.kind == "file"
                || (entry.kind == "symlink" && entry.target_kind.as_deref() == Some("file"));
            (is_file && entry.name != ".." && matcher.is_match(&entry.name)).then_some(index)
        })
        .collect()
}

fn merge_mask_selection(selection: &[usize], matches: &[usize], select: bool) -> Vec<usize> {
    let mut selected: HashSet<_> = selection.iter().copied().collect();
    for index in matches {
        if select {
            selected.insert(*index);
        } else {
            selected.remove(index);
        }
    }
    let mut selection: Vec<_> = selected.into_iter().collect();
    selection.sort_unstable();
    selection
}

fn apply_mask_selection(
    panel: &mut crate::state::Panel,
    epoch: u64,
    matches: &[usize],
    select: bool,
) -> bool {
    if panel.panel_epoch != epoch || panel.status != crate::state::PanelStatus::Ready {
        return false;
    }
    panel.selection = merge_mask_selection(&panel.selection, matches, select);
    panel.version += 1;
    true
}

async fn select_by_mask(scope: &Value, args: &Value, select: bool, ctx: &mut IntentContext<'_>) {
    let Some(mask) = arg_str(args, "mask").filter(|mask| !mask.trim().is_empty()) else {
        return;
    };
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (entries, epoch) = {
        let state = ctx.state.lock().await;
        let Some(panel) = state.panels.get(&panel_id) else {
            return;
        };
        if panel.status != crate::state::PanelStatus::Ready {
            return;
        }
        (panel.entries.clone(), panel.panel_epoch)
    };
    let mask = mask.to_owned();
    let matches =
        match tokio::task::spawn_blocking(move || selection_mask_matches(&entries, &mask)).await {
            Ok(matches) => matches,
            Err(error) => {
                eprintln!("[selection] mask matching failed: {error}");
                return;
            }
        };
    let (selection, version, selected_entries) = {
        let mut state = ctx.state.lock().await;
        let Some(panel) = state.panels.get_mut(&panel_id) else {
            return;
        };
        if !apply_mask_selection(panel, epoch, &matches, select) {
            return;
        }
        (
            panel.selection.clone(),
            panel.version,
            selected_entries_for_panel(panel),
        )
    };
    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({
            "panelEpoch": epoch, "selection": selection, "selectedEntries": selected_entries,
        }),
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

/// Union the contiguous absolute-index range from `cursor` to `target` into
/// `selection`. Returns the clamped target cursor and the sorted selection.
/// Used by Shift+click range extend (linear in both list and grid modes).
fn extend_selection_range(
    cursor: usize,
    selection: &[usize],
    target: usize,
    len: usize,
) -> (usize, Vec<usize>) {
    if len == 0 {
        return (0, selection.to_vec());
    }
    let to = target.min(len - 1);
    let from = cursor.min(len - 1);
    let lo = from.min(to);
    let hi = from.max(to);
    let mut selected: HashSet<usize> = selection.iter().copied().collect();
    for index in lo..=hi {
        selected.insert(index);
    }
    let mut next: Vec<usize> = selected.into_iter().collect();
    next.sort_unstable();
    (to, next)
}

async fn extend_selection_to(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = IndexArgs::from_args(args);
    let (new_cursor, new_selection, version, selected_entries) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let len = panel.entries.len();
            if len == 0 {
                return;
            }
            let (cursor, selection) =
                extend_selection_range(panel.cursor, &panel.selection, parsed.index, len);
            panel.cursor = cursor;
            panel.selection = selection.clone();
            panel.version += 1;
            (
                cursor,
                selection,
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
        serde_json::json!({
            "cursor": new_cursor,
            "selection": new_selection,
            "selectedEntries": selected_entries
        }),
    )
    .await;
}

async fn move_cursor_with_select(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    // Prefer `delta` (used by grid mode for ±cols / ±1 cell jumps); fall
    // back to edge selection, then single-step direction.
    let delta_opt = args.get("delta").and_then(Value::as_i64);
    let edge_opt = arg_str(args, "edge");
    let direction = arg_str(args, "direction").unwrap_or("down");
    let (new_cursor, new_selection, version, needs_window, selected_entries) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            let len = panel.entries.len();
            if len == 0 {
                return;
            }
            if let Some(edge) = edge_opt {
                let target = if edge == "last" { len - 1 } else { 0 };
                let start = panel.cursor.min(target);
                let end = panel.cursor.max(target);
                let mut selected: HashSet<usize> = panel.selection.iter().copied().collect();
                for index in start..=end {
                    selected.insert(index);
                }
                panel.selection = selected.into_iter().collect();
                panel.selection.sort_unstable();
                panel.cursor = target;
            } else {
                let step: i64 = match delta_opt {
                    Some(d) if d != 0 => d,
                    _ => {
                        if direction == "up" {
                            -1
                        } else {
                            1
                        }
                    }
                };
                let target = (panel.cursor as i64 + step).clamp(0, (len - 1) as i64) as usize;
                // Apply the single-step toggle for each step between current and target.
                let dir: i64 = if target as i64 >= panel.cursor as i64 {
                    1
                } else {
                    -1
                };
                while panel.cursor != target {
                    let previous = panel.cursor;
                    let next = (panel.cursor as i64 + dir) as usize;
                    panel.cursor = next;
                    if panel.selection.contains(&next) {
                        panel
                            .selection
                            .retain(|&index| index != next && index != previous);
                    } else {
                        if !panel.selection.contains(&previous) {
                            panel.selection.push(previous);
                        }
                        panel.selection.push(next);
                    }
                }
            }
            let cursor = panel.cursor;
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

async fn toggle_directory_sizes(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (actions, panel_epoch) = {
        let state = ctx.state.lock().await;
        let Some(panel) = state.panels.get(&panel_id) else {
            return;
        };
        (
            directory_size_actions(&panel.entries, &panel.selection, &panel.calculating_size),
            panel.panel_epoch,
        )
    };
    for (name, cancel) in actions {
        if !ctx
            .state
            .lock()
            .await
            .panels
            .get(&panel_id)
            .is_some_and(|panel| panel.panel_epoch == panel_epoch)
        {
            return;
        }
        if cancel {
            cancel_size_calculation(
                &serde_json::json!(panel_id),
                &serde_json::json!({ "name": name }),
                ctx,
            )
            .await;
        } else {
            calculate_entry_size(&panel_id, &name, Some(panel_epoch), ctx).await;
        }
    }
}

fn supports_directory_sizes(driver: &dyn crate::vfs::VfsDriver) -> bool {
    driver.is_local()
        || driver.as_any().is::<crate::vfs::zip::ZipFsDriver>()
        || driver
            .as_any()
            .is::<crate::vfs::seven_zip::SevenZipFsDriver>()
}

fn apply_archive_size(
    panel: &mut crate::state::Panel,
    path: &str,
    source_id: &str,
    panel_epoch: u64,
    name: &str,
    size: u64,
) -> Option<u64> {
    if panel.panel_epoch != panel_epoch || panel.source_id != source_id || panel.path != path {
        return None;
    }
    let entry = panel
        .entries
        .iter_mut()
        .find(|entry| entry.name == name && entry.kind == "directory")?;
    entry.size = Some(size);
    panel.version += 1;
    Some(panel.version)
}

async fn calculate_size(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    calculate_entry_size(&panel_id, &NameArgs::from_args(args).name, None, ctx).await;
}

async fn calculate_entry_size(
    panel_id: &str,
    name: &str,
    expected_epoch: Option<u64>,
    ctx: &mut IntentContext<'_>,
) {
    let (panel_path, full_path, source_id, panel_epoch, driver) = {
        let state = ctx.state.lock().await;
        let Some(panel) = state.panels.get(panel_id) else {
            return;
        };
        if expected_epoch.is_some_and(|epoch| epoch != panel.panel_epoch)
            || !panel
                .entries
                .iter()
                .any(|entry| entry.name == name && entry.kind == "directory")
        {
            return;
        }
        let Some(driver) = state.drivers.get(&panel.source_id) else {
            return;
        };
        // Remote and virtual paths must never reach the local filesystem walker.
        if !supports_directory_sizes(driver.as_ref()) {
            return;
        }
        let full_path = if panel.path.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", panel.path.trim_end_matches('/'), name)
        };
        (
            panel.path.clone(),
            full_path,
            panel.source_id.clone(),
            panel.panel_epoch,
            std::sync::Arc::clone(driver),
        )
    };

    if !driver.is_local() {
        // Archives calculate from their in-memory tree, outside the state lock.
        let archive_path = full_path.clone();
        let size = tokio::task::spawn_blocking(move || {
            driver
                .as_any()
                .downcast_ref::<crate::vfs::zip::ZipFsDriver>()
                .map(|driver| driver.dir_size(&archive_path))
                .or_else(|| {
                    driver
                        .as_any()
                        .downcast_ref::<crate::vfs::seven_zip::SevenZipFsDriver>()
                        .map(|driver| driver.dir_size(&archive_path))
                })
        })
        .await;
        let Ok(Some(size)) = size else { return };
        let version = {
            let mut state = ctx.state.lock().await;
            let Some(panel) = state.panels.get_mut(panel_id) else {
                return;
            };
            let Some(version) =
                apply_archive_size(panel, &panel_path, &source_id, panel_epoch, name, size)
            else {
                return;
            };
            version
        };
        send_panel_patch(
            ctx,
            panel_id.to_string(),
            version,
            serde_json::json!({
                "panelEpoch": panel_epoch,
                "updateEntrySize": { "name": name, "size": size }
            }),
        )
        .await;
        return;
    }

    ctx.dir_size_queue.invalidate_cache(&full_path).await;
    ctx.dir_size_queue
        .enqueue(
            panel_id.to_string(),
            panel_path,
            source_id,
            panel_epoch,
            name.to_string(),
            full_path,
            std::sync::Arc::clone(ctx.state),
            ctx.tx.clone(),
        )
        .await;
}

async fn cancel_size_calculation(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = NameArgs::from_args(args);
    ctx.dir_size_queue
        .cancel(&panel_id, &parsed.name, ctx.tx)
        .await;

    let (calculating_size, version, panel_epoch) = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            panel.calculating_size.remove(&parsed.name);
            panel.version += 1;
            let mut calculating_size: Vec<String> =
                panel.calculating_size.iter().cloned().collect();
            calculating_size.sort();
            (calculating_size, panel.version, panel.panel_epoch)
        } else {
            return;
        }
    };

    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({
            "panelEpoch": panel_epoch,
            "calculatingSize": calculating_size
        }),
    )
    .await;
}

async fn request_window(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = WindowArgs::from_args(args);
    // Viewport geometry is separate from the buffered render window, including
    // for small directories where send_window is otherwise a no-op.
    if let (Some(epoch), Some(start), Some(count)) = (
        args.get("panelEpoch").and_then(Value::as_u64),
        args.get("visibleStart").and_then(Value::as_u64),
        args.get("visibleCount").and_then(Value::as_u64),
    ) {
        let mut state = ctx.state.lock().await;
        if let Some(panel) = state.panels.get_mut(&panel_id) {
            if panel.panel_epoch != epoch {
                return;
            }
            panel.thumbnail_viewport = Some((epoch, start as usize, count as usize));
        }
        state
            .thumb_pool
            .set_visible(crate::state::collect_visible_thumb_paths(&state));
    }
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

fn neighbors_for_prefetch(
    entries: &[crate::ws::protocol::EntrySchema],
    cursor: usize,
    dir_path: &str,
    source_id: &str,
) -> Vec<(String, Option<u64>)> {
    super::viewer::collect_neighbors(entries, cursor, dir_path, source_id, "next")
}

async fn set_panel_view_mode(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let mode_str = arg_str(args, "mode").unwrap_or("list");
    let target = match crate::state::PanelViewMode::from_str(mode_str) {
        Some(m) => m,
        None => return,
    };

    let (version, persist_payload) = {
        let mut state_guard = ctx.state.lock().await;
        // Validate against source capability for grid mode.
        if target == crate::state::PanelViewMode::Grid {
            let source_id = match state_guard.panels.get(&panel_id) {
                Some(p) => p.source_id.clone(),
                None => return,
            };
            let supports = state_guard
                .drivers
                .get(&source_id)
                .map(|d| d.supports_thumbnails())
                .unwrap_or(false);
            if !supports {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        timestamp: now(),
                        code: "InvalidIntent".to_string(),
                        message: "Grid view is not available for this source".to_string(),
                        recoverable: true,
                        extra: None,
                    })
                    .await;
                return;
            }
        }

        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            if panel.view_mode == target {
                return;
            }
            panel.view_mode = target;
            panel.version += 1;
            let version = panel.version;
            // Refresh thumb-pool visibility — switching to/from grid changes
            // which entries should be considered visible.
            let visible = crate::state::collect_visible_thumb_paths(&state_guard);
            state_guard.thumb_pool.set_visible(visible);
            // Stage a settings snapshot for disk-write only when persistence
            // is enabled. Updating the in-memory map is cheap and keeps the
            // settings file consistent on the next save_general() too.
            let persist = if state_guard.settings.general.save_panel_view_mode {
                state_guard
                    .settings
                    .panel_view_modes
                    .insert(panel_id.clone(), target.as_str().to_string());
                Some(state_guard.settings.clone())
            } else {
                None
            };
            (version, persist)
        } else {
            return;
        }
    };

    send_panel_patch(
        ctx,
        panel_id,
        version,
        serde_json::json!({ "viewMode": target.as_str() }),
    )
    .await;

    if let Some(settings) = persist_payload {
        let _ = tokio::task::spawn_blocking(move || {
            let _ = crate::state::settings::save(&settings);
        });
    }
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

#[cfg(test)]
mod tests {
    use super::{directory_size_actions, extend_selection_range, toggle_entry_selection};
    use crate::ws::protocol::EntrySchema;
    use std::collections::HashSet;

    fn entry(name: &str, kind: &str, size: Option<u64>) -> EntrySchema {
        EntrySchema {
            name: name.into(),
            kind: kind.into(),
            size,
            target_kind: None,
            browsable_archive: false,
            mtime: None,
            btime: None,
            permissions: None,
            icon_name: None,
            cc: None,
            source_path: None,
            path_hint: None,
            git_status: None,
            git_details: None,
        }
    }

    #[test]
    fn selection_masks_match_whole_names_with_literal_punctuation() {
        for (mask, name, expected) in [
            ("*.txt", "Notes.TXT", true),
            ("*.txt", "notes.txt.bak", false),
            ("a?.txt", "ab.txt", true),
            ("a?.txt", "abc.txt", false),
            ("*.txt\t*.jpg\n*.png", "PHOTO.JPG", true),
            ("[a]+.txt", "[a]+.txt", true),
            ("[a]+.txt", "aaa.txt", false),
            ("*.TXT", "line\nbreak.txt", true),
            ("?.txt", "é.txt", true),
            ("É*.txt", "été.TXT", true),
            ("*.*", "README", true),
            ("*.*", ".bashrc", true),
            ("*", "README", true),
        ] {
            let matcher = super::compile_selection_mask(mask).unwrap();
            assert_eq!(matcher.is_match(name), expected, "{mask:?}: {name:?}");
        }
        assert!(super::compile_selection_mask(" \t\n").is_none());
    }

    #[test]
    fn selection_masks_only_match_files_and_known_file_symlinks() {
        let mut file_link = entry("link.txt", "symlink", None);
        file_link.target_kind = Some("file".into());
        let mut directory_link = entry("folder.txt", "symlink", None);
        directory_link.target_kind = Some("directory".into());
        let entries = vec![
            entry("..", "directory", None),
            entry("folder.txt", "directory", None),
            entry("file.txt", "file", None),
            file_link,
            directory_link,
            entry("broken.txt", "symlink", None),
            entry("device.txt", "other", None),
        ];
        assert_eq!(super::selection_mask_matches(&entries, "*.*"), vec![2, 3]);
        assert!(super::selection_mask_matches(&entries, " ").is_empty());
    }

    #[test]
    fn selection_masks_add_and_remove_matches_without_touching_other_selection() {
        assert_eq!(
            super::merge_mask_selection(&[0, 3], &[1, 3, 4], true),
            vec![0, 1, 3, 4]
        );
        assert_eq!(
            super::merge_mask_selection(&[0, 1, 3, 4], &[1, 3], false),
            vec![0, 4]
        );
        assert_eq!(super::merge_mask_selection(&[0, 3], &[], true), vec![0, 3]);
        assert_eq!(super::merge_mask_selection(&[0, 3], &[], false), vec![0, 3]);
    }

    #[test]
    fn selection_masks_include_files_outside_the_render_window() {
        let entries: Vec<_> = (0..3201)
            .map(|index| entry(&format!("file-{index}.txt"), "file", None))
            .collect();
        let matches = super::selection_mask_matches(&entries, "file-3200.*");
        assert_eq!(matches, vec![3200]);
        assert_eq!(super::selection_mask_matches(&entries, "*.*").len(), 3201);
        assert!(super::selection_mask_matches(&[], "*").is_empty());
    }

    #[test]
    fn selection_masks_discard_stale_results_and_merge_the_current_selection() {
        let mut panel = archive_panel();
        panel.entries.push(entry("file.txt", "file", None));
        assert!(!super::apply_mask_selection(&mut panel, 2, &[1], true));
        assert_eq!(panel.selection, vec![0]);
        assert_eq!(panel.version, 5);
        panel.status = crate::state::PanelStatus::Loading;
        assert!(!super::apply_mask_selection(&mut panel, 3, &[1], true));
        panel.status = crate::state::PanelStatus::Ready;
        assert!(super::apply_mask_selection(&mut panel, 3, &[1], true));
        assert_eq!(panel.selection, vec![0, 1]);
        assert_eq!(panel.version, 6);
        assert_eq!(panel.cursor, 0);
        assert!(super::apply_mask_selection(&mut panel, 3, &[1], false));
        assert_eq!(panel.selection, vec![0]);
        assert_eq!(panel.version, 7);
    }

    #[test]
    fn space_selects_first_folder_and_requests_its_unknown_size() {
        let entries = vec![
            entry("folder", "directory", None),
            entry("file", "file", Some(7)),
        ];
        let mut selection = vec![];
        let calculating = HashSet::new();
        assert_eq!(
            toggle_entry_selection(&entries, 0, &mut selection, &calculating, true),
            Some("folder".into())
        );
        assert_eq!(selection, vec![0]);
        assert_eq!(
            toggle_entry_selection(&entries, 1, &mut selection, &calculating, true),
            None
        );
        assert_eq!(selection, vec![0, 1]);
    }

    #[test]
    fn space_deselects_without_cancelling_or_restarting_size_work() {
        let entries = vec![entry("folder", "directory", None)];
        let calculating = HashSet::from(["folder".into()]);
        let mut selection = vec![0];
        assert_eq!(
            toggle_entry_selection(&entries, 0, &mut selection, &calculating, true),
            None
        );
        assert!(selection.is_empty());
        assert!(calculating.contains("folder"));
        assert_eq!(
            toggle_entry_selection(&entries, 0, &mut selection, &calculating, true),
            None
        );
        assert_eq!(selection, vec![0]);
    }

    #[test]
    fn space_reuses_known_sizes_including_zero_and_does_not_follow_symlinks() {
        let entries = vec![
            entry("known", "directory", Some(42)),
            entry("empty", "directory", Some(0)),
            entry("link", "symlink", None),
        ];
        let mut selection = vec![];
        for index in 0..entries.len() {
            assert_eq!(
                toggle_entry_selection(&entries, index, &mut selection, &HashSet::new(), true),
                None
            );
        }
        assert_eq!(selection, vec![0, 1, 2]);
    }

    #[test]
    fn plain_toggle_and_invalid_cursors_never_request_size() {
        let entries = vec![entry("folder", "directory", None)];
        let mut selection = vec![];
        assert_eq!(
            toggle_entry_selection(&entries, 0, &mut selection, &HashSet::new(), false),
            None
        );
        assert_eq!(selection, vec![0]);
        assert_eq!(
            toggle_entry_selection(&entries, 5, &mut selection, &HashSet::new(), true),
            None
        );
        assert_eq!(selection, vec![0]);
        assert_eq!(
            toggle_entry_selection(&[], 0, &mut selection, &HashSet::new(), true),
            None
        );
        assert_eq!(selection, vec![0]);
    }

    #[test]
    fn bulk_sizes_resolve_all_backend_selection_indices_and_toggle_individually() {
        let entries = vec![
            entry("running", "directory", None),
            entry("file", "file", Some(7)),
            entry("idle", "directory", Some(42)),
        ];
        let calculating = HashSet::from(["running".into()]);
        // Render windows must not restrict absolute backend selection indices.
        assert_eq!(
            directory_size_actions(&entries, &[0, 2, 99], &calculating),
            vec![("running".into(), true), ("idle".into(), false)]
        );
        assert_eq!(
            directory_size_actions(&entries, &[], &calculating),
            vec![("running".into(), true), ("idle".into(), false)]
        );
        assert!(directory_size_actions(&entries, &[1], &calculating).is_empty());
        assert!(directory_size_actions(&[], &[], &calculating).is_empty());
    }

    #[test]
    fn bulk_sizes_include_entries_beyond_a_render_window() {
        let entries: Vec<_> = (0..3200)
            .map(|index| entry(&format!("folder-{index}"), "directory", None))
            .collect();
        assert_eq!(
            directory_size_actions(&entries, &[0, 3199], &HashSet::new()),
            vec![("folder-0".into(), false), ("folder-3199".into(), false)]
        );
        assert_eq!(
            directory_size_actions(&entries, &[], &HashSet::new()).len(),
            3200
        );
    }

    #[test]
    fn local_sizing_never_walks_virtual_paths() {
        assert!(super::supports_directory_sizes(
            &crate::vfs::local::LocalFsDriver
        ));
        assert!(!super::supports_directory_sizes(
            &crate::vfs::history::HistoryDriver::new(vec![])
        ));
        assert!(!super::supports_directory_sizes(
            &crate::vfs::bookmarks::BookmarksDriver::new(vec![])
        ));
    }

    fn archive_panel() -> crate::state::Panel {
        crate::state::Panel {
            panel_id: "left".into(),
            slot_id: "left".into(),
            tab_kind: crate::state::TabKind::Listing,
            tab_title_override: None,
            source_id: "zip:fixture".into(),
            path: "/inside".into(),
            status: crate::state::PanelStatus::Ready,
            sort: crate::ws::protocol::SortSpec {
                by: "name".into(),
                order: "asc".into(),
            },
            filter: None,
            filter_mode_active: false,
            show_hidden: false,
            view_mode: crate::state::PanelViewMode::List,
            cursor: 0,
            selection: vec![0],
            entries: vec![entry("folder", "directory", None)],
            version: 5,
            panel_epoch: 3,
            calculating_size: HashSet::new(),
            windowed: false,
            window_start: 0,
            window_count: 0,
            thumbnail_viewport: None,
            window_version: 0,
            source_paths: Default::default(),
            navigation_context: Default::default(),
            shadow_source: None,
            terminal_session: None,
            git: None,
        }
    }

    #[test]
    fn stale_archive_completion_never_changes_replacement_listing_or_selection() {
        for (path, source, epoch) in [
            ("/old", "zip:fixture", 3),
            ("/inside", "zip:other", 3),
            ("/inside", "zip:fixture", 2),
        ] {
            let mut panel = archive_panel();
            assert_eq!(
                super::apply_archive_size(&mut panel, path, source, epoch, "folder", 42),
                None
            );
            assert_eq!(panel.entries[0].size, None);
            assert_eq!(panel.version, 5);
            assert_eq!(panel.selection, vec![0]);
        }
        let mut panel = archive_panel();
        assert_eq!(
            super::apply_archive_size(&mut panel, "/inside", "zip:fixture", 3, "folder", 42),
            Some(6)
        );
        assert_eq!(panel.entries[0].size, Some(42));
        assert_eq!(panel.selection, vec![0]);
        assert_eq!(panel.cursor, 0);
    }

    #[test]
    fn extend_down_from_empty_selection() {
        let (cursor, selection) = extend_selection_range(2, &[], 5, 10);
        assert_eq!(cursor, 5);
        assert_eq!(selection, vec![2, 3, 4, 5]);
    }

    #[test]
    fn extend_up_from_empty_selection() {
        let (cursor, selection) = extend_selection_range(5, &[], 2, 10);
        assert_eq!(cursor, 2);
        assert_eq!(selection, vec![2, 3, 4, 5]);
    }

    #[test]
    fn extend_unions_with_existing_selection() {
        let (cursor, selection) = extend_selection_range(3, &[0, 10], 5, 12);
        assert_eq!(cursor, 5);
        assert_eq!(selection, vec![0, 3, 4, 5, 10]);
    }

    #[test]
    fn extend_to_same_index_selects_cursor() {
        let (cursor, selection) = extend_selection_range(3, &[], 3, 10);
        assert_eq!(cursor, 3);
        assert_eq!(selection, vec![3]);
    }

    #[test]
    fn extend_clamps_out_of_range_target() {
        let (cursor, selection) = extend_selection_range(1, &[], 99, 5);
        assert_eq!(cursor, 4);
        assert_eq!(selection, vec![1, 2, 3, 4]);
    }

    #[test]
    fn extend_empty_panel_is_noop() {
        let (cursor, selection) = extend_selection_range(0, &[1], 3, 0);
        assert_eq!(cursor, 0);
        assert_eq!(selection, vec![1]);
    }
}
