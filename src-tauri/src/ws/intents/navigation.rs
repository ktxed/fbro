use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::state::{Panel, PanelStatus, PanelViewMode, ShadowSource, Source, TabKind};
use crate::vfs::VfsDriver;
use crate::watcher::WatcherCommand;
use crate::ws::intents::{archive_file_path, arg_str, is_archive_source, is_search_source};
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
        "OpenPathInRightPanel" => open_path_in_right_panel(args, ctx).await,
        "NavigateUp" => navigate_up(scope, ctx).await,
        "RevealInFileManager" => reveal_in_file_manager(args, ctx).await,
        "ListOpenWithApps" => list_open_with_apps(args, ctx).await,
        "LaunchOpenWithApp" => launch_open_with_app(args, ctx).await,
        "ProvideInput" => provide_input(args, ctx).await,
        "NavigateToHistoryEntry" => navigate_to_history_entry(args, ctx).await,
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

struct OpenPathArgs {
    path: String,
    cwd: String,
    create_tab: bool,
}

impl OpenPathArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            path: arg_str(args, "path").unwrap_or(".").to_string(),
            cwd: arg_str(args, "cwd").unwrap_or("").to_string(),
            create_tab: args
                .get("createTab")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        }
    }
}

struct SourceArgs {
    source_id: String,
}

fn expand_tilde(path: &str, home: &str) -> String {
    if path == "~" {
        return home.to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return Path::new(home).join(rest).to_string_lossy().to_string();
    }
    path.to_string()
}

fn normalize_requested_path(raw_path: &str, cwd: &str, home: &str) -> PathBuf {
    let raw = raw_path.trim();
    let raw = if raw.is_empty() { "." } else { raw };
    let expanded = expand_tilde(raw, home);
    let path = PathBuf::from(&expanded);
    if path.is_absolute() {
        path
    } else if cwd.is_empty() {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from(home))
            .join(path)
    } else {
        PathBuf::from(cwd).join(path)
    }
}

fn path_starts_with(path: &str, root: &str) -> bool {
    let root_path = Path::new(root);
    !root.is_empty() && Path::new(path).starts_with(root_path)
}

fn most_specific_source_for_path<'a, I>(sources: I, path: &str) -> Option<String>
where
    I: IntoIterator<Item = (&'a String, &'a str)>,
{
    sources
        .into_iter()
        .filter(|(_, root_path)| path_starts_with(path, root_path))
        .max_by_key(|(_, root_path)| root_path.len())
        .map(|(id, _)| id.clone())
}

fn source_for_local_path(state: &crate::state::AppState, path: &str) -> Option<String> {
    most_specific_source_for_path(
        state.sources.iter().filter_map(|(id, source)| {
            (source.kind == "filesystem"
                && source.mounted
                && !source.shadow
                && state
                    .drivers
                    .get(id)
                    .map(|driver| driver.is_local())
                    .unwrap_or(false))
            .then_some((id, source.root_path.as_str()))
        }),
        path,
    )
}

#[cfg(test)]
mod tests {
    use super::most_specific_source_for_path;

    fn pick_source<'a>(sources: &'a [(&'a String, &'a str)], path: &str) -> Option<String> {
        most_specific_source_for_path(sources.iter().copied(), path)
    }

    #[test]
    fn absolute_path_uses_most_specific_source_root() {
        let root = "vol-root".to_string();
        let home = "vol-home".to_string();
        let media = "vol-media".to_string();
        let sources = [
            (&root, "/"),
            (&home, "/home/user"),
            (&media, "/run/media/user/Extreme SSD"),
        ];

        assert_eq!(
            pick_source(&sources, "/home/user/projects/fbro"),
            Some("vol-home".to_string())
        );
        assert_eq!(
            pick_source(&sources, "/run/media/user/Extreme SSD/photos"),
            Some("vol-media".to_string())
        );
        assert_eq!(pick_source(&sources, "/etc"), Some("vol-root".to_string()));
    }

    #[test]
    fn source_root_must_match_path_boundary() {
        let home = "vol-home".to_string();
        let sources = [(&home, "/home/user")];

        assert_eq!(
            pick_source(&sources, "/home/user"),
            Some("vol-home".to_string())
        );
        assert_eq!(pick_source(&sources, "/home/user2"), None);
    }
}

async fn emit_cli_open_error(message: impl Into<String>, ctx: &IntentContext<'_>) {
    let _ = ctx
        .tx
        .send(ServerMessage::Error {
            timestamp: now(),
            code: "CLI_OPEN_FAILED".to_string(),
            message: message.into(),
            recoverable: true,
            extra: None,
        })
        .await;
}

async fn broadcast_slot_change(
    ctx: &IntentContext<'_>,
    slot_id: &str,
    new_panel_id: Option<&str>,
    focus_to: Option<&str>,
) {
    let (panel_upsert, slot_patch, layout_patch, focus_patch, gui_senders) = {
        let state_guard = ctx.state.lock().await;
        let slot = state_guard.slots.get(slot_id).cloned();
        let gui = state_guard.gui_senders.clone();

        let panel_upsert = new_panel_id.and_then(|pid| {
            let p = state_guard.panels.get(pid)?;
            let supports_thumbs = state_guard
                .drivers
                .get(&p.source_id)
                .map(|d| d.supports_thumbnails())
                .unwrap_or(false);
            Some(ServerMessage::StatePatch {
                timestamp: now(),
                domain: "panels".to_string(),
                entity_id: Some(pid.to_string()),
                version: p.version,
                patch: serde_json::json!({
                    "create": true,
                    "slotId": p.slot_id,
                    "tabKind": p.tab_kind.as_str(),
                    "tabTitle": p.tab_title_override,
                    "path": p.path,
                    "sourceId": p.source_id,
                    "status": p.status.as_str(),
                    "sort": p.sort,
                    "showHidden": p.show_hidden,
                    "viewMode": p.view_mode.as_str(),
                    "supportsThumbnails": supports_thumbs,
                    "terminalSessionId": p.terminal_session,
                    "entries": [],
                    "cursor": 0,
                    "selection": [],
                    "selectedEntries": [],
                }),
            })
        });

        let slot_patch = slot.map(|s| ServerMessage::StatePatch {
            timestamp: now(),
            domain: "slots".to_string(),
            entity_id: Some(s.slot_id.clone()),
            version: 0,
            patch: serde_json::json!({
                "tabPanelIds": s.tab_panel_ids,
                "activePanelId": s.active_panel_id,
            }),
        });

        let active_per_slot: Vec<String> = state_guard
            .layout_slots
            .iter()
            .filter_map(|sid| {
                state_guard
                    .slots
                    .get(sid)
                    .map(|s| s.active_panel_id.clone())
            })
            .collect();
        let layout_patch = ServerMessage::StatePatch {
            timestamp: now(),
            domain: "layout".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({
                "panels": active_per_slot,
                "slots": state_guard.layout_slots,
            }),
        };

        let focus_patch = focus_to.map(|fid| ServerMessage::StatePatch {
            timestamp: now(),
            domain: "focus".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "focus": fid }),
        });

        (panel_upsert, slot_patch, layout_patch, focus_patch, gui)
    };

    for patch in [panel_upsert, slot_patch, Some(layout_patch), focus_patch]
        .into_iter()
        .flatten()
    {
        let _ = ctx.tx.send(patch.clone()).await;
        for sender in &gui_senders {
            let _ = sender.send(patch.clone()).await;
        }
    }
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
    let (entry_kind, entry_name, new_path, is_in_shadow, source_id) = {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&panel_id) {
            if let Some(entry) = panel.entries.get(panel.cursor) {
                let driver = state_guard.drivers.get(&panel.source_id).unwrap();
                (
                    entry.kind.clone(),
                    entry.name.clone(),
                    driver.resolve_child(&panel.path, &entry.name),
                    panel.shadow_source.is_some(),
                    panel.source_id.clone(),
                )
            } else {
                return;
            }
        } else {
            return;
        }
    };

    if entry_kind == "directory" {
        // Virtual sources whose entries point to real filesystem paths must
        // hand off to the local FS driver on directory open. Sources that
        // advertise InternalNavigation (archives, procs-hierarchical) keep
        // directory navigation within the same driver.
        let is_virtual_nav = {
            let state_guard = ctx.state.lock().await;
            state_guard.sources.get(&source_id).map_or(false, |s| {
                s.kind == "virtual"
                    && !s
                        .supports
                        .iter()
                        .any(|c| c == crate::ws::intents::CAP_INTERNAL_NAV)
            })
        };
        if is_virtual_nav {
            // Switch the panel to the real source for this path before navigating.
            let real_source_id = {
                let state_guard = ctx.state.lock().await;
                source_for_local_path(&state_guard, &new_path).unwrap_or_default()
            };
            if !real_source_id.is_empty() {
                let mut state_guard = ctx.state.lock().await;
                if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
                    panel.source_id = real_source_id;
                    panel.shadow_source = None;
                    // Search sources persist until explicitly removed; all other
                    // virtual sources are transient and cleaned up when unused.
                    if !is_search_source(&source_id) {
                        let still_in_use = state_guard
                            .panels
                            .values()
                            .any(|p| p.source_id == source_id);
                        if !still_in_use {
                            state_guard.sources.remove(&source_id);
                            state_guard.drivers.remove(&source_id);
                        }
                    }
                }
            }
        }
        navigate_to(
            &panel_id,
            &new_path,
            None,
            None,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            ctx.dir_size_queue,
            ctx.watcher_cmd_tx,
        )
        .await;
    } else if entry_kind == "file"
        && is_zip_like(&entry_name)
        && (!is_in_shadow || is_search_source(&source_id))
    {
        // Enter zip archive as a shadow filesystem.
        enter_zip_archive(&panel_id, &new_path, &entry_name, ctx).await;
    } else if entry_kind == "file"
        && is_7z_like(&entry_name)
        && (!is_in_shadow || is_search_source(&source_id))
    {
        // Enter 7z archive as a shadow filesystem.
        enter_7z_archive(&panel_id, &new_path, &entry_name, ctx).await;
    }
}

fn is_zip_like(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".zip")
        || lower.ends_with(".apk")
        || lower.ends_with(".jar")
        || lower.ends_with(".war")
        || lower.ends_with(".ear")
        || lower.ends_with(".docx")
        || lower.ends_with(".xlsx")
        || lower.ends_with(".pptx")
        || lower.ends_with(".odt")
        || lower.ends_with(".ods")
        || lower.ends_with(".odp")
        || lower.ends_with(".epub")
        || lower.ends_with(".xpi")
        || lower.ends_with(".crx")
}

fn is_7z_like(name: &str) -> bool {
    name.to_lowercase().ends_with(".7z")
}

/// Create a SevenZipFsDriver, register a shadow source, and navigate the panel into the archive root.
async fn enter_7z_archive(
    panel_id: &str,
    archive_path: &str,
    archive_name: &str,
    ctx: &mut IntentContext<'_>,
) {
    let driver = match crate::vfs::seven_zip::SevenZipFsDriver::new(archive_path) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            let _ = ctx
                .tx
                .send(ServerMessage::Error {
                    timestamp: now(),
                    code: "7Z_OPEN_FAILED".to_string(),
                    message: format!("Cannot open archive: {e}"),
                    recoverable: true,
                    extra: Some(serde_json::json!({ "panelId": panel_id })),
                })
                .await;
            return;
        }
    };

    let source_id = format!("7z:{}", archive_path);

    {
        let mut state_guard = ctx.state.lock().await;

        let (parent_source_id, parent_path, prev_shadow, cursor_pos) =
            match state_guard.panels.get(panel_id) {
                Some(p) => (
                    p.source_id.clone(),
                    p.path.clone(),
                    p.shadow_source.clone(),
                    p.cursor,
                ),
                None => return,
            };

        let shadow = ShadowSource {
            parent_source_id: parent_source_id.clone(),
            parent_path,
            archive_path: archive_path.to_string(),
            cursor_hint: Some(archive_name.to_string()),
            cursor_index: if is_search_source(&parent_source_id) {
                Some(cursor_pos)
            } else {
                None
            },
            parent_shadow: prev_shadow.map(Box::new),
        };

        if !state_guard.sources.contains_key(&source_id) {
            state_guard.sources.insert(
                source_id.clone(),
                Source {
                    id: source_id.clone(),
                    name: archive_name.to_string(),
                    kind: "virtual".to_string(),
                    supports: vec![
                        "ListEntries".to_string(),
                        "OpenFolder".to_string(),
                        "CopySelection".to_string(),
                        crate::ws::intents::CAP_INTERNAL_NAV.to_string(),
                    ],
                    root_path: String::new(),
                    mounted: true,
                    device_path: None,
                    shadow: true,
                    icon_name: None,
                    sort_priority: 100,
                    read_only: false,
                    peer_id: None,
                },
            );
        }
        if !state_guard.drivers.contains_key(&source_id) {
            state_guard
                .drivers
                .insert(source_id.clone(), driver as Arc<dyn VfsDriver>);
        }

        if let Some(panel) = state_guard.panels.get_mut(panel_id) {
            panel.shadow_source = Some(shadow);
            panel.source_id = source_id.clone();
            panel.path = String::new();
            panel.cursor = 0;
            panel.selection.clear();
            panel.filter = None;
            panel.filter_mode_active = false;
        }
    }

    let sources_json = {
        let state_guard = ctx.state.lock().await;
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

    rescan_panel(
        panel_id,
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

/// Create a ZipFsDriver, register a shadow source, and navigate the panel into the archive root.
async fn enter_zip_archive(
    panel_id: &str,
    archive_path: &str,
    archive_name: &str,
    ctx: &mut IntentContext<'_>,
) {
    // Try to create the zip driver.
    let driver = match crate::vfs::zip::ZipFsDriver::new(archive_path) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            let _ = ctx
                .tx
                .send(ServerMessage::Error {
                    timestamp: now(),
                    code: "ZIP_OPEN_FAILED".to_string(),
                    message: format!("Cannot open archive: {e}"),
                    recoverable: true,
                    extra: Some(serde_json::json!({ "panelId": panel_id })),
                })
                .await;
            return;
        }
    };

    let source_id = format!("zip:{}", archive_path);

    // Register shadow source + driver, update panel state.
    {
        let mut state_guard = ctx.state.lock().await;

        // Read panel info before mutating.
        let (parent_source_id, parent_path, prev_shadow, cursor_pos) =
            match state_guard.panels.get(panel_id) {
                Some(p) => (
                    p.source_id.clone(),
                    p.path.clone(),
                    p.shadow_source.clone(),
                    p.cursor,
                ),
                None => return,
            };

        let shadow = ShadowSource {
            parent_source_id: parent_source_id.clone(),
            parent_path,
            archive_path: archive_path.to_string(),
            cursor_hint: Some(archive_name.to_string()),
            cursor_index: if is_search_source(&parent_source_id) {
                Some(cursor_pos)
            } else {
                None
            },
            parent_shadow: prev_shadow.map(Box::new),
        };

        // Register source if not already present (another panel might have it open).
        if !state_guard.sources.contains_key(&source_id) {
            state_guard.sources.insert(
                source_id.clone(),
                Source {
                    id: source_id.clone(),
                    name: archive_name.to_string(),
                    kind: "virtual".to_string(),
                    supports: vec![
                        "ListEntries".to_string(),
                        "OpenFolder".to_string(),
                        "CopySelection".to_string(),
                        crate::ws::intents::CAP_INTERNAL_NAV.to_string(),
                    ],
                    root_path: String::new(), // root inside the archive is ""
                    mounted: true,
                    device_path: None,
                    shadow: true,
                    icon_name: None,
                    sort_priority: 100,
                    read_only: false,
                    peer_id: None,
                },
            );
        }
        if !state_guard.drivers.contains_key(&source_id) {
            state_guard
                .drivers
                .insert(source_id.clone(), driver as Arc<dyn VfsDriver>);
        }

        // Now mutate the panel.
        if let Some(panel) = state_guard.panels.get_mut(panel_id) {
            panel.shadow_source = Some(shadow);
            panel.source_id = source_id.clone();
            panel.path = String::new(); // archive root
            panel.cursor = 0;
            panel.selection.clear();
            panel.filter = None;
            panel.filter_mode_active = false;
        }
    }

    // Send a sources patch so FE knows about the new shadow source.
    let sources_json = {
        let state_guard = ctx.state.lock().await;
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

    // Navigate into the archive root — this triggers a scan.
    rescan_panel(
        panel_id,
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

/// Tear down a shadow source for a panel and return to the parent filesystem.
/// Returns (parent_source_id, parent_path, cursor_hint, cursor_index) if the panel was in a shadow source.
pub(crate) async fn teardown_shadow_source(
    panel_id: &str,
    ctx: &mut IntentContext<'_>,
) -> Option<(String, String, Option<String>, Option<usize>)> {
    let shadow_info = {
        let mut state_guard = ctx.state.lock().await;
        let panel = state_guard.panels.get_mut(panel_id)?;
        let shadow = panel.shadow_source.take()?;
        let old_source_id = panel.source_id.clone();

        // Restore parent context.
        panel.source_id = shadow.parent_source_id.clone();
        panel.path = shadow.parent_path.clone();
        panel.cursor = 0;
        panel.selection.clear();
        panel.filter = None;
        panel.filter_mode_active = false;

        // Restore the stashed shadow (e.g. search results context when exiting
        // a zip that was opened from within search results).
        panel.shadow_source = shadow.parent_shadow.map(|b| *b);

        // Check if any other panel is still using this shadow source.
        let still_in_use = state_guard
            .panels
            .values()
            .any(|p| p.panel_id != panel_id && p.source_id == old_source_id);

        // Search result sources have their own lifecycle (RemoveSearchSource /
        // the jobs bar × button) — never auto-remove them on navigate-away.
        if !still_in_use && !is_search_source(&old_source_id) {
            state_guard.sources.remove(&old_source_id);
            state_guard.drivers.remove(&old_source_id);
        }

        Some((
            shadow.parent_source_id,
            shadow.parent_path,
            shadow.cursor_hint,
            shadow.cursor_index,
        ))
    };

    if shadow_info.is_some() {
        // Send updated sources to FE.
        let sources_json = {
            let state_guard = ctx.state.lock().await;
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
    }

    shadow_info
}

async fn navigate_home(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    // If inside a shadow source, tear it down first.
    teardown_shadow_source(&panel_id, ctx).await;
    let home = ctx.state.lock().await.platform.home_dir();
    navigate_to(
        &panel_id,
        &home,
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

async fn navigate_source_root(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    // If inside a shadow source, tear it down first.
    teardown_shadow_source(&panel_id, ctx).await;
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
        None,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        ctx.dir_size_queue,
        ctx.watcher_cmd_tx,
    )
    .await;
}

async fn open_path_in_right_panel(args: &Value, ctx: &mut IntentContext<'_>) {
    use tauri::Manager;

    let parsed = OpenPathArgs::from_args(args);
    let requested = {
        let state_guard = ctx.state.lock().await;
        normalize_requested_path(&parsed.path, &parsed.cwd, &state_guard.platform.home_dir())
    };

    let metadata = match std::fs::metadata(&requested) {
        Ok(metadata) => metadata,
        Err(_) => {
            emit_cli_open_error(
                format!("Path does not exist: {}", requested.to_string_lossy()),
                ctx,
            )
            .await;
            return;
        }
    };

    let (target_dir, cursor_hint) = if metadata.is_dir() {
        (requested, None)
    } else if metadata.is_file() {
        let Some(parent) = requested
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        else {
            emit_cli_open_error("File has no parent directory.", ctx).await;
            return;
        };
        (
            parent.to_path_buf(),
            requested
                .file_name()
                .map(|name| name.to_string_lossy().to_string()),
        )
    } else {
        emit_cli_open_error("Path is not a directory or file.", ctx).await;
        return;
    };

    let target_dir = std::fs::canonicalize(&target_dir).unwrap_or(target_dir);
    let target_path = target_dir.to_string_lossy().to_string();

    let (slot_id, source_id) = {
        let state_guard = ctx.state.lock().await;
        let source_id = match source_for_local_path(&state_guard, &target_path) {
            Some(source_id) => source_id,
            None => {
                drop(state_guard);
                emit_cli_open_error(
                    format!("No mounted local source contains: {target_path}"),
                    ctx,
                )
                .await;
                return;
            }
        };
        let slot_id = if state_guard.slots.contains_key("right") {
            "right".to_string()
        } else {
            state_guard
                .layout_slots
                .last()
                .cloned()
                .unwrap_or_else(|| "right".to_string())
        };
        (slot_id, source_id)
    };

    let _ = ctx.app_handle.get_webview_window("main").map(|window| {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    });

    let target_panel_id = if parsed.create_tab {
        match create_right_listing_tab(&slot_id, &source_id, &target_path, ctx).await {
            Some(panel_id) => panel_id,
            None => {
                emit_cli_open_error("Could not create a right-panel tab.", ctx).await;
                return;
            }
        }
    } else {
        match reuse_right_listing_panel(&slot_id, &source_id, ctx).await {
            Some(panel_id) => panel_id,
            None => {
                emit_cli_open_error("Could not find a right panel to open the path.", ctx).await;
                return;
            }
        }
    };

    let _ = ctx
        .tx
        .send(ServerMessage::NavigateOk {
            timestamp: now(),
            panel_id: target_panel_id.clone(),
        })
        .await;

    navigate_to(
        &target_panel_id,
        &target_path,
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

async fn reuse_right_listing_panel(
    slot_id: &str,
    source_id: &str,
    ctx: &mut IntentContext<'_>,
) -> Option<String> {
    let panel_id = {
        let mut state_guard = ctx.state.lock().await;
        let slot = state_guard.slots.get(slot_id)?.clone();
        let panel_id = slot
            .tab_panel_ids
            .iter()
            .find(|id| {
                state_guard
                    .panels
                    .get(*id)
                    .map(|panel| panel.tab_kind == TabKind::Listing)
                    .unwrap_or(false)
            })
            .cloned()
            .unwrap_or(slot.active_panel_id);
        if let Some(slot) = state_guard.slots.get_mut(slot_id) {
            slot.active_panel_id = panel_id.clone();
        }
        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            panel
                .source_paths
                .insert(panel.source_id.clone(), panel.path.clone());
            panel.source_id = source_id.to_string();
            panel.shadow_source = None;
            panel.tab_kind = TabKind::Listing;
            panel.terminal_session = None;
        }
        state_guard.focus = panel_id.clone();
        panel_id
    };

    broadcast_slot_change(ctx, slot_id, None, Some(&panel_id)).await;
    Some(panel_id)
}

async fn create_right_listing_tab(
    slot_id: &str,
    source_id: &str,
    path: &str,
    ctx: &mut IntentContext<'_>,
) -> Option<String> {
    let (panel_id, driver_is_local) = {
        let mut state_guard = ctx.state.lock().await;
        let slot = state_guard.slots.get(slot_id)?.clone();
        let panel_ids: HashSet<String> = state_guard.panels.keys().cloned().collect();
        let mut n = 1u64;
        let panel_id = loop {
            let candidate = format!("{slot_id}-tab-{n}");
            if !panel_ids.contains(&candidate) {
                break candidate;
            }
            n += 1;
        };

        let base = state_guard.panels.get(&slot.active_panel_id).cloned();
        let driver = state_guard.drivers.get(source_id).cloned()?;
        let supports_thumbs = driver.supports_thumbnails();
        let default_sort = crate::ws::protocol::SortSpec {
            by: "name".to_string(),
            order: "asc".to_string(),
        };
        let view_mode = if supports_thumbs {
            base.as_ref()
                .map(|panel| panel.view_mode)
                .unwrap_or(PanelViewMode::List)
        } else {
            PanelViewMode::List
        };
        let panel = Panel {
            panel_id: panel_id.clone(),
            slot_id: slot_id.to_string(),
            tab_kind: TabKind::Listing,
            tab_title_override: None,
            source_id: source_id.to_string(),
            path: path.to_string(),
            status: PanelStatus::Loading,
            sort: base
                .as_ref()
                .map(|panel| panel.sort.clone())
                .unwrap_or(default_sort),
            filter: None,
            filter_mode_active: false,
            show_hidden: base
                .as_ref()
                .map(|panel| panel.show_hidden)
                .unwrap_or(state_guard.settings.general.show_hidden_default),
            view_mode,
            cursor: 0,
            selection: vec![],
            entries: vec![],
            version: 0,
            calculating_size: Default::default(),
            windowed: false,
            window_start: 0,
            window_count: 0,
            window_version: 0,
            source_paths: base
                .as_ref()
                .map(|panel| panel.source_paths.clone())
                .unwrap_or_default(),
            shadow_source: None,
            terminal_session: None,
        };

        state_guard.panels.insert(panel_id.clone(), panel);
        if let Some(slot) = state_guard.slots.get_mut(slot_id) {
            let active_idx = slot
                .tab_panel_ids
                .iter()
                .position(|id| id == &slot.active_panel_id)
                .unwrap_or(slot.tab_panel_ids.len().saturating_sub(1));
            slot.tab_panel_ids.insert(active_idx + 1, panel_id.clone());
            slot.active_panel_id = panel_id.clone();
        }
        state_guard.focus = panel_id.clone();
        (panel_id, driver.is_local())
    };

    broadcast_slot_change(ctx, slot_id, Some(&panel_id), Some(&panel_id)).await;
    if driver_is_local {
        let _ = ctx
            .watcher_cmd_tx
            .send(WatcherCommand::Watch {
                panel_id: panel_id.clone(),
                path: PathBuf::from(path),
            })
            .await;
    }
    Some(panel_id)
}

async fn navigate_panel_to(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = NavigatePanelArgs::from_args(args);
    if parsed.panel_id.is_empty() {
        return;
    }

    // For sources with intra-driver navigation (zip, 7z, procs-hierarchical),
    // navigate directly within the current driver — this enables breadcrumb
    // navigation inside archives and the procs tree without going through the
    // real-filesystem path-resolution below.
    let archive_nav_path: Option<String> = {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&parsed.panel_id) {
            if panel.shadow_source.is_some() {
                let is_archive = state_guard
                    .sources
                    .get(&panel.source_id)
                    .map(|s| {
                        s.supports
                            .iter()
                            .any(|c| c == crate::ws::intents::CAP_INTERNAL_NAV)
                    })
                    .unwrap_or(false);
                if is_archive {
                    Some(parsed.path.trim().to_string())
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

    if let Some(path) = archive_nav_path {
        let _ = ctx
            .tx
            .send(ServerMessage::NavigateOk {
                timestamp: now(),
                panel_id: parsed.panel_id.clone(),
            })
            .await;
        navigate_to(
            &parsed.panel_id,
            &path,
            parsed.cursor_hint,
            None,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            ctx.dir_size_queue,
            ctx.watcher_cmd_tx,
        )
        .await;
        return;
    }

    if !parsed.panel_id.is_empty() && !parsed.path.is_empty() {
        // Block navigation for non-archive shadow sources (virtual sources like
        // history, bookmarks whose entries don't map to navigable sub-paths).
        {
            let state_guard = ctx.state.lock().await;
            if let Some(panel) = state_guard.panels.get(&parsed.panel_id) {
                if panel.shadow_source.is_some() {
                    let _ = ctx
                        .tx
                        .send(ServerMessage::Error {
                            timestamp: now(),
                            code: "NAVIGATE_FAILED".to_string(),
                            message: "Cannot navigate to a path while browsing an archive."
                                .to_string(),
                            recoverable: true,
                            extra: Some(serde_json::json!({ "panelId": parsed.panel_id })),
                        })
                        .await;
                    return;
                }
            }
        }

        // On Windows, a bare drive letter like "C:" is a relative path (resolves to CWD
        // on that drive). Always normalise it to "C:/" so it refers to the drive root.
        #[cfg(windows)]
        let raw = parsed.path.trim().to_string();
        #[cfg(windows)]
        let requested_path = if raw.len() == 2
            && raw.as_bytes()[1] == b':'
            && raw.as_bytes()[0].is_ascii_alphabetic()
        {
            format!("{}/", raw)
        } else {
            raw
        };
        #[cfg(not(windows))]
        let requested_path = parsed.path.trim().to_string();
        let resolved_target = {
            let state_guard = ctx.state.lock().await;
            let panel = match state_guard.panels.get(&parsed.panel_id) {
                Some(panel) => panel,
                None => return,
            };
            let target_source_id = Path::new(&requested_path)
                .is_absolute()
                .then(|| source_for_local_path(&state_guard, &requested_path))
                .flatten()
                .unwrap_or_else(|| panel.source_id.clone());
            let driver = match state_guard.drivers.get(&target_source_id) {
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
                .map(|(path, cursor)| (target_source_id, path, cursor))
            }
        };

        let (target_source_id, validated_path, cursor_hint) = match resolved_target {
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

        {
            let mut state_guard = ctx.state.lock().await;
            if let Some(panel) = state_guard.panels.get_mut(&parsed.panel_id) {
                if panel.source_id != target_source_id {
                    let previous_source_id = panel.source_id.clone();
                    let previous_path = panel.path.clone();
                    panel.source_paths.insert(previous_source_id, previous_path);
                    panel.source_id = target_source_id;
                    panel.selection.clear();
                    panel.filter = None;
                    panel.filter_mode_active = false;
                }
            } else {
                return;
            }
        }

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

async fn open_file(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (path, entry, platform, shadow_source, source_id, source_kind) = {
        let state_guard = ctx.state.lock().await;
        let panel = match state_guard.panels.get(&panel_id) {
            Some(panel) => panel,
            None => return,
        };
        let source_kind = state_guard
            .sources
            .get(&panel.source_id)
            .map(|source| source.kind.clone())
            .unwrap_or_default();
        (
            panel.path.clone(),
            panel.entries.get(panel.cursor).cloned(),
            Arc::clone(&state_guard.platform),
            panel.shadow_source.clone(),
            panel.source_id.clone(),
            source_kind,
        )
    };
    if let Some(entry) = entry {
        if entry.kind != "directory" {
            if is_search_source(&source_id) {
                // Search results source: resolve_real_info gives the real
                // full_path, so open it directly via the platform.
                let real_full_path = {
                    let state_guard = ctx.state.lock().await;
                    state_guard
                        .drivers
                        .get(&source_id)
                        .and_then(|drv| {
                            drv.as_any()
                                .downcast_ref::<crate::vfs::search::SearchResultsDriver>()
                        })
                        .and_then(|sd| sd.resolve_real_info(&entry.name))
                        .map(|(fp, _)| fp)
                        .unwrap_or_else(|| format!("{}/{}", path.trim_end_matches('/'), entry.name))
                };
                platform.open_file(&real_full_path);
            } else if source_kind == "remote" {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        timestamp: now(),
                        code: "REMOTE_OPEN_UNSUPPORTED".to_string(),
                        message: "Remote files can be viewed in fbro, but cannot be opened by the OS directly".to_string(),
                        recoverable: true,
                        extra: None,
                    })
                    .await;
            } else if shadow_source.is_some() {
                // Inside an archive: extract to temp file and open.
                let inner_path = if path.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{}/{}", path, entry.name)
                };
                let state_guard = ctx.state.lock().await;
                if let Some(driver) = state_guard.drivers.get(&source_id) {
                    // Try zip driver first, then 7z driver.
                    let extract_result = if let Some(zip_driver) = driver
                        .as_ref()
                        .as_any()
                        .downcast_ref::<crate::vfs::zip::ZipFsDriver>(
                    ) {
                        Some(zip_driver.extract_to_temp(&inner_path))
                    } else if let Some(sz_driver) = driver
                        .as_ref()
                        .as_any()
                        .downcast_ref::<crate::vfs::seven_zip::SevenZipFsDriver>(
                    ) {
                        Some(sz_driver.extract_to_temp(&inner_path))
                    } else {
                        None
                    };

                    match extract_result {
                        Some(Ok(temp_path)) => {
                            drop(state_guard);
                            platform.open_file(&temp_path.to_string_lossy());
                        }
                        Some(Err(e)) => {
                            drop(state_guard);
                            let _ = ctx
                                .tx
                                .send(ServerMessage::Error {
                                    timestamp: now(),
                                    code: "ARCHIVE_EXTRACT_FAILED".to_string(),
                                    message: format!("Cannot extract file: {e}"),
                                    recoverable: true,
                                    extra: None,
                                })
                                .await;
                        }
                        None => {}
                    }
                }
            } else {
                let full_path = format!("{}/{}", path.trim_end_matches('/'), entry.name);
                platform.open_file(&full_path);
            }
        }
    }
}

async fn open_terminal(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let (path, platform, is_shadow) = {
        let state_guard = ctx.state.lock().await;
        let panel = match state_guard.panels.get(&panel_id) {
            Some(panel) => panel,
            None => return,
        };
        (
            panel.path.clone(),
            Arc::clone(&state_guard.platform),
            panel.shadow_source.is_some(),
        )
    };
    if is_shadow {
        return; // Cannot open a terminal inside an archive.
    }
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
    // If inside a shadow source, tear it down first.
    teardown_shadow_source(&panel_id, ctx).await;
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

        if target_path.is_empty() {
            let _ = ctx
                .watcher_cmd_tx
                .send(WatcherCommand::Unwatch {
                    panel_id: panel_id.clone(),
                })
                .await;
        } else {
            let _ = ctx
                .watcher_cmd_tx
                .send(WatcherCommand::Watch {
                    panel_id: panel_id.clone(),
                    path: PathBuf::from(&target_path),
                })
                .await;
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
                    ctx.watcher_cmd_tx,
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
    let (current_path, parent_path, has_shadow) = {
        let state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get(&panel_id) {
            let driver = state_guard.drivers.get(&panel.source_id).unwrap();
            (
                Some(panel.path.clone()),
                driver.parent_path(&panel.path),
                panel.shadow_source.is_some(),
            )
        } else {
            (None, None, false)
        }
    };

    if let Some(current_path) = current_path {
        if let Some(parent) = parent_path {
            if parent != current_path {
                // Normal navigate up within current source (local FS or inside zip subdirectory).
                let dir_name = current_path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .map(str::to_string);
                navigate_to(
                    &panel_id,
                    &parent,
                    dir_name,
                    None,
                    ctx.state,
                    ctx.tx,
                    ctx.scan_handles,
                    ctx.dir_size_queue,
                    ctx.watcher_cmd_tx,
                )
                .await;
            }
        } else if has_shadow {
            // parent_path is None and we're in a shadow source → exit the archive.
            if let Some((_parent_source_id, parent_path, cursor_hint, cursor_index)) =
                teardown_shadow_source(&panel_id, ctx).await
            {
                navigate_to(
                    &panel_id,
                    &parent_path,
                    cursor_hint,
                    cursor_index,
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

/// Navigate to a specific (path, sourceId) history entry, handling source switching,
/// archive re-entry, and search source restoration.
async fn navigate_to_history_entry(args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = arg_str(args, "panelId").unwrap_or("").to_string();
    let target_path = arg_str(args, "path").unwrap_or("").to_string();
    let target_source_id = arg_str(args, "sourceId").unwrap_or("").to_string();

    if panel_id.is_empty() || target_source_id.is_empty() {
        return;
    }

    // Read current panel state.
    let (current_source_id, in_shadow) = {
        let guard = ctx.state.lock().await;
        let panel = match guard.panels.get(&panel_id) {
            Some(p) => p,
            None => return,
        };
        (panel.source_id.clone(), panel.shadow_source.is_some())
    };

    // If target is the same source, navigate within it directly (works for archives too).
    if target_source_id == current_source_id {
        navigate_to(
            &panel_id,
            &target_path,
            None,
            None,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            ctx.dir_size_queue,
            ctx.watcher_cmd_tx,
        )
        .await;
        return;
    }

    // Exit any current shadow context before switching to a different source.
    if in_shadow {
        teardown_shadow_source(&panel_id, ctx).await;

        // After teardown, re-check: if we're now on the target source, navigate there.
        let post_teardown_source = {
            let guard = ctx.state.lock().await;
            guard.panels.get(&panel_id).map(|p| p.source_id.clone())
        };
        if post_teardown_source.as_deref() == Some(&target_source_id) {
            navigate_to(
                &panel_id,
                &target_path,
                None,
                None,
                ctx.state,
                ctx.tx,
                ctx.scan_handles,
                ctx.dir_size_queue,
                ctx.watcher_cmd_tx,
            )
            .await;
            return;
        }
    }

    // Helper: send "unavailable" error.
    let send_unavailable = |message: &str| {
        ctx.tx.try_send(ServerMessage::Error {
            timestamp: now(),
            code: "HISTORY_ENTRY_UNAVAILABLE".to_string(),
            message: message.to_string(),
            recoverable: true,
            extra: Some(serde_json::json!({ "panelId": panel_id })),
        })
    };

    if is_archive_source(&target_source_id) {
        let archive_path = archive_file_path(&target_source_id).to_string();
        let archive_name = Path::new(&archive_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&archive_path)
            .to_string();

        if !Path::new(&archive_path).exists() {
            let _ = send_unavailable("Archive file no longer exists.");
            return;
        }

        // The driver is removed when the user exits an archive (teardown_shadow_source),
        // so re-mount it from disk if it is no longer in state.
        let needs_mount = !ctx
            .state
            .lock()
            .await
            .drivers
            .contains_key(&target_source_id);
        if needs_mount {
            let driver: Arc<dyn VfsDriver> = if target_source_id.starts_with("7z:") {
                match crate::vfs::seven_zip::SevenZipFsDriver::new(&archive_path) {
                    Ok(d) => Arc::new(d),
                    Err(e) => {
                        let _ = send_unavailable(&format!("Cannot open archive: {e}"));
                        return;
                    }
                }
            } else {
                match crate::vfs::zip::ZipFsDriver::new(&archive_path) {
                    Ok(d) => Arc::new(d),
                    Err(e) => {
                        let _ = send_unavailable(&format!("Cannot open archive: {e}"));
                        return;
                    }
                }
            };
            let mut guard = ctx.state.lock().await;
            guard
                .sources
                .entry(target_source_id.clone())
                .or_insert_with(|| Source {
                    id: target_source_id.clone(),
                    name: archive_name.clone(),
                    kind: "virtual".to_string(),
                    supports: vec![
                        "ListEntries".to_string(),
                        "OpenFolder".to_string(),
                        "CopySelection".to_string(),
                        crate::ws::intents::CAP_INTERNAL_NAV.to_string(),
                    ],
                    root_path: String::new(),
                    mounted: true,
                    device_path: None,
                    shadow: true,
                    icon_name: None,
                    sort_priority: 100,
                    read_only: false,
                    peer_id: None,
                });
            guard
                .drivers
                .entry(target_source_id.clone())
                .or_insert(driver);
        }

        // Notify the FE of the (possibly newly registered) archive source.
        let sources_json = {
            let guard = ctx.state.lock().await;
            build_sources_json(&guard)
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

        let (parent_source_id, parent_path) = {
            let guard = ctx.state.lock().await;
            match guard.panels.get(&panel_id) {
                Some(p) => (p.source_id.clone(), p.path.clone()),
                None => return,
            }
        };
        {
            let mut guard = ctx.state.lock().await;
            let panel = match guard.panels.get_mut(&panel_id) {
                Some(p) => p,
                None => return,
            };
            panel.source_id = target_source_id.clone();
            panel.shadow_source = Some(ShadowSource {
                parent_source_id,
                parent_path,
                archive_path,
                cursor_hint: None,
                cursor_index: None,
                parent_shadow: None,
            });
        }
        navigate_to(
            &panel_id,
            &target_path,
            None,
            None,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            ctx.dir_size_queue,
            ctx.watcher_cmd_tx,
        )
        .await;
    } else if is_search_source(&target_source_id) {
        // Search source: must still be available.
        let driver_exists = ctx
            .state
            .lock()
            .await
            .drivers
            .contains_key(&target_source_id);
        if !driver_exists {
            let _ = send_unavailable("Search results are no longer available.");
            return;
        }
        let (parent_source_id, parent_path) = {
            let guard = ctx.state.lock().await;
            let panel = match guard.panels.get(&panel_id) {
                Some(p) => p,
                None => return,
            };
            (panel.source_id.clone(), panel.path.clone())
        };
        {
            let mut guard = ctx.state.lock().await;
            let panel = match guard.panels.get_mut(&panel_id) {
                Some(p) => p,
                None => return,
            };
            panel.source_id = target_source_id.clone();
            panel.shadow_source = Some(ShadowSource {
                parent_source_id,
                parent_path,
                archive_path: target_source_id.clone(),
                cursor_hint: None,
                cursor_index: None,
                parent_shadow: None,
            });
        }
        navigate_to(
            &panel_id,
            &target_path,
            None,
            None,
            ctx.state,
            ctx.tx,
            ctx.scan_handles,
            ctx.dir_size_queue,
            ctx.watcher_cmd_tx,
        )
        .await;
    } else {
        // Regular source switch.
        let driver_exists = ctx
            .state
            .lock()
            .await
            .drivers
            .contains_key(&target_source_id);
        if !driver_exists {
            let _ = send_unavailable("Source is no longer available.");
            return;
        }
        {
            let mut guard = ctx.state.lock().await;
            let panel = match guard.panels.get_mut(&panel_id) {
                Some(p) => p,
                None => return,
            };
            panel.source_id = target_source_id.clone();
            panel.shadow_source = None;
        }
        navigate_to(
            &panel_id,
            &target_path,
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
}

/// Handle the ProvideInput intent — currently used to provide passwords for encrypted zip archives.
async fn provide_input(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = arg_str(args, "requestId").unwrap_or("").to_string();
    let value = arg_str(args, "value").unwrap_or("").to_string();

    if request_id.is_empty() || value.is_empty() {
        return;
    }

    if let Some(source_id) = request_id.strip_prefix("zip-password:") {
        let state_guard = ctx.state.lock().await;
        if let Some(driver) = state_guard.drivers.get(source_id) {
            if let Some(zip_driver) = driver
                .as_ref()
                .as_any()
                .downcast_ref::<crate::vfs::zip::ZipFsDriver>()
            {
                zip_driver.set_password(&value);
            }
        }
    } else if let Some(source_id) = request_id.strip_prefix("7z-password:") {
        let state_guard = ctx.state.lock().await;
        if let Some(driver) = state_guard.drivers.get(source_id) {
            if let Some(sz_driver) = driver
                .as_ref()
                .as_any()
                .downcast_ref::<crate::vfs::seven_zip::SevenZipFsDriver>()
            {
                sz_driver.set_password(&value);
            }
        }
    }
}
