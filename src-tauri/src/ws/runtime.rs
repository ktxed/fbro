use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::jobs::dirsize::{DirSizeQueue, DirSizeTaskSnapshot};
use crate::jobs::viewer_prefetch::ViewerPrefetchQueue;
use crate::jobs::scan::run_scan;
use crate::jobs::{DecisionResponse, JobState, ScanHandle};
use crate::persistence::{PanelSession, SessionState, SlotSession, TabSession};
use crate::state::{AppState, Panel, PanelStatus, SharedState};
use crate::vfs::VfsDriver;
use crate::watcher::WatcherCommand;
use crate::ws::commands::build_interactive_commands;
use crate::ws::protocol::{
    now, DirSizeQueueSnapshot, DirSizeTaskSchema, FileOpFileSchema, GeneralSettingsSchema,
    LayoutDomain, PanelSlotSchema, PanelSnapshot, ServerMessage, SettingsSnapshot,
    ShadowSourceSchema, SourceCapabilities, SourceSchema, StateDomains,
};

pub(crate) struct IntentContext<'a> {
    pub state: &'a SharedState,
    pub tx: &'a mpsc::Sender<ServerMessage>,
    pub scan_handles: &'a mut HashMap<String, ScanHandle>,
    pub decision_senders: &'a mut HashMap<String, mpsc::Sender<DecisionResponse>>,
    pub app_handle: &'a tauri::AppHandle,
    pub dir_size_queue: &'a Arc<DirSizeQueue>,
    pub viewer_prefetch: &'a Arc<ViewerPrefetchQueue>,
    pub watcher_cmd_tx: &'a mpsc::Sender<WatcherCommand>,
    pub volume_watcher: &'a Arc<dyn crate::platform::VolumeWatcher>,
}

#[cfg(target_os = "linux")]
fn thumbnail_cache_dir_string() -> String {
    crate::platform::linux::thumbnail::cache_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

#[cfg(not(target_os = "linux"))]
fn thumbnail_cache_dir_string() -> String {
    String::new()
}

#[cfg(target_os = "linux")]
fn thumbnail_cache_stats() -> (u64, u64) {
    let (count, bytes) = crate::platform::linux::thumbnail::cache_stats();
    (count as u64, bytes)
}

#[cfg(not(target_os = "linux"))]
fn thumbnail_cache_stats() -> (u64, u64) {
    (0, 0)
}

pub(crate) fn build_state_snapshot(
    state: &AppState,
    dir_size_tasks: Vec<DirSizeTaskSnapshot>,
) -> ServerMessage {
    let panels = state
        .panels
        .iter()
        .map(|(id, panel)| {
            let supports_thumbs = state
                .drivers
                .get(&panel.source_id)
                .map(|d| d.supports_thumbnails())
                .unwrap_or(false);
            (id.clone(), build_panel_snapshot(panel, supports_thumbs))
        })
        .collect();

    // Layout.panels = active-tab panel id per slot, in slot order
    // (back-compat shape until FE reads slots directly).
    let active_panel_ids: Vec<String> = state
        .layout_slots
        .iter()
        .filter_map(|sid| state.slots.get(sid).map(|s| s.active_panel_id.clone()))
        .collect();

    let slots: HashMap<String, PanelSlotSchema> = state
        .slots
        .iter()
        .map(|(id, slot)| {
            (
                id.clone(),
                PanelSlotSchema {
                    slot_id: slot.slot_id.clone(),
                    tab_panel_ids: slot.tab_panel_ids.clone(),
                    active_panel_id: slot.active_panel_id.clone(),
                },
            )
        })
        .collect();

    ServerMessage::StateSnapshot {
        timestamp: now(),
        domains: StateDomains {
            layout: LayoutDomain {
                panels: active_panel_ids,
                slots: state.layout_slots.clone(),
            },
            panels,
            slots,
            sources: build_sources(state),
            focus: state.focus.clone(),
            jobs: vec![],
            dir_size_queue: DirSizeQueueSnapshot {
                tasks: dir_size_tasks
                    .into_iter()
                    .map(|task| DirSizeTaskSchema {
                        panel_id: task.panel_id,
                        entry_name: task.entry_name,
                        full_path: task.full_path,
                        enqueued_at: task.enqueued_at,
                        status: task.status,
                    })
                    .collect(),
            },
            commands: build_interactive_commands(),
            bookmarks: state.bookmarks.clone(),
            font_size: state.font_size,
            settings: SettingsSnapshot {
                general: GeneralSettingsSchema {
                    show_hidden_default: state.settings.general.show_hidden_default,
                    remember_window_state: state.settings.general.remember_window_state,
                    preserve_attributes: state.settings.general.preserve_attributes,
                    ludicrous_mode: state.settings.general.ludicrous_mode,
                    max_image_size_mb: state.settings.general.max_image_size_mb,
                    save_panel_view_mode: state.settings.general.save_panel_view_mode,
                    enable_video_thumbnails: state.settings.general.enable_video_thumbnails,
                    thumbnail_max_image_size_mb: state
                        .settings
                        .general
                        .thumbnail_max_image_size_mb,
                    thumbnail_video_frame_skip: state
                        .settings
                        .general
                        .thumbnail_video_frame_skip,
                    session_path: crate::persistence::session_path()
                        .to_string_lossy()
                        .to_string(),
                    bookmarks_path: crate::bookmarks::bookmarks_path()
                        .to_string_lossy()
                        .to_string(),
                    thumbnail_cache_dir: thumbnail_cache_dir_string(),
                    thumbnail_cache_item_count: thumbnail_cache_stats().0,
                    thumbnail_cache_size_bytes: thumbnail_cache_stats().1,
                    persist_panel_tabs: state.settings.general.persist_panel_tabs,
                proc_tree_mode: state.settings.general.proc_tree_mode,
                },
            },
        },
    }
}

pub(crate) fn build_sources(state: &AppState) -> Vec<SourceSchema> {
    let mut sources: Vec<&crate::state::Source> = state.sources.values().collect();
    sources.sort_by(|a, b| {
        a.sort_priority
            .cmp(&b.sort_priority)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    sources
        .into_iter()
        .map(|src| {
            let space = if src.mounted && !src.shadow {
                state.platform.disk_space(&src.root_path)
            } else {
                None
            };
            SourceSchema {
                id: src.id.clone(),
                name: src.name.clone(),
                kind: src.kind.clone(),
                supports: src.supports.clone(),
                capabilities: SourceCapabilities {
                    read_only: src.shadow,
                    supports_streaming: true,
                    supports_jobs: !src.shadow,
                },
                root_path: src.root_path.clone(),
                mounted: src.mounted,
                free_bytes: space.map(|(free, _)| free),
                total_bytes: space.map(|(_, total)| total),
                shadow: src.shadow,
                icon_name: src.icon_name.clone(),
            }
        })
        .collect()
}

pub(crate) fn build_sources_json(state: &AppState) -> Vec<serde_json::Value> {
    build_sources(state)
        .into_iter()
        .map(|source| serde_json::to_value(source).unwrap_or(serde_json::Value::Null))
        .collect()
}

pub(crate) fn selected_names_for(state_guard: &AppState, panel_id: &str) -> Option<Vec<String>> {
    state_guard.panels.get(panel_id).map(|panel| {
        panel
            .selection
            .iter()
            .filter_map(|&index| panel.entries.get(index).map(|entry| entry.name.clone()))
            .collect()
    })
}

pub(crate) fn selected_entries_for_panel(panel: &Panel) -> Vec<FileOpFileSchema> {
    panel
        .selection
        .iter()
        .filter_map(|&index| {
            panel.entries.get(index).map(|entry| FileOpFileSchema {
                name: entry.name.clone(),
                size: entry.size,
                kind: entry.kind.clone(),
                source_path: entry.source_path.clone(),
            })
        })
        .collect()
}

pub(crate) async fn resolve_panel_id(scope: &serde_json::Value, state: &SharedState) -> String {
    if scope == "focused-panel" || scope.as_str() == Some("focused-panel") {
        state.lock().await.focus.clone()
    } else if let Some(id) = scope.as_str() {
        id.to_string()
    } else {
        state.lock().await.focus.clone()
    }
}

pub(crate) async fn navigate_to(
    panel_id: &str,
    new_path: &str,
    cursor_hint: Option<String>,
    cursor_index: Option<usize>,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
    dir_size_queue: &Arc<DirSizeQueue>,
    watcher_cmd_tx: &mpsc::Sender<WatcherCommand>,
) {
    if let Some(handle) = scan_handles.remove(panel_id) {
        handle.abort();
    }

    let (sort, filter, show_hidden, version, driver, shadow_source_schema, source_id, supports_thumbs, view_mode) = {
        let mut state_guard = state.lock().await;
        let (source_id, sort, filter, show_hidden, version, shadow_schema) =
            if let Some(panel) = state_guard.panels.get_mut(panel_id) {
                let source_id = panel.source_id.clone();
                panel.path = new_path.to_string();
                panel.entries.clear();
                panel.cursor = 0;
                panel.selection.clear();
                panel.status = PanelStatus::Loading;
                panel.windowed = false;
                panel.window_start = 0;
                panel.window_count = 0;
                panel.filter = None;
                panel.filter_mode_active = false;
                panel.version += 1;
                let shadow_schema = panel.shadow_source.as_ref().map(|ss| ShadowSourceSchema {
                    parent_source_id: ss.parent_source_id.clone(),
                    parent_path: ss.parent_path.clone(),
                    archive_path: ss.archive_path.clone(),
                    cursor_hint: ss.cursor_hint.clone(),
                });
                (source_id, panel.sort.clone(), panel.filter.clone(), panel.show_hidden, panel.version, shadow_schema)
            } else {
                return;
            };
        let driver = state_guard.drivers.get(&source_id).cloned().unwrap();
        let supports_thumbs = driver.supports_thumbnails();
        // Auto-fall-back: if grid is active but new source doesn't support
        // thumbnails, drop back to list mode.
        if !supports_thumbs {
            if let Some(panel) = state_guard.panels.get_mut(panel_id) {
                panel.view_mode = crate::state::PanelViewMode::List;
            }
        }
        let view_mode = state_guard
            .panels
            .get(panel_id)
            .map(|p| p.view_mode.as_str().to_string())
            .unwrap_or_else(|| "list".to_string());
        (sort, filter, show_hidden, version, driver, shadow_schema, source_id, supports_thumbs, view_mode)
    };

    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id.to_string()),
            version,
            patch: serde_json::json!({
                "path": new_path,
                "sourceId": source_id,
                "entries": [],
                "cursor": 0,
                "selection": [],
                "selectedEntries": [],
                "status": "loading",
                "filter": null,
                "filterModeActive": false,
                "totalCount": null,
                "totalSize": null,
                "windowStart": null,
                "shadowSource": shadow_source_schema,
                "supportsThumbnails": supports_thumbs,
                "viewMode": view_mode,
            }),
        })
        .await;

    // Only watch real local filesystem paths; virtual/archive drivers don't
    // have paths the OS watcher can monitor.
    if driver.is_local() {
        let _ = watcher_cmd_tx
            .send(WatcherCommand::Watch {
                panel_id: panel_id.to_string(),
                path: PathBuf::from(new_path),
            })
            .await;
    } else {
        // Entering a virtual/archive source — stop watching any previous real path.
        let _ = watcher_cmd_tx
            .send(WatcherCommand::Unwatch {
                panel_id: panel_id.to_string(),
            })
            .await;
    }

    let handle = spawn_scan(
        panel_id.to_string(),
        new_path.to_string(),
        sort,
        filter,
        version,
        tx.clone(),
        driver,
        cursor_hint,
        cursor_index,
        None,
        Arc::clone(state),
        Arc::clone(dir_size_queue),
        show_hidden,
    );
    scan_handles.insert(panel_id.to_string(), handle);
}

pub(crate) async fn rescan_panel(
    panel_id: &str,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
    cursor_hint: Option<String>,
    selection_names: Option<Vec<String>>,
    dir_size_queue: &Arc<DirSizeQueue>,
) {
    if let Some(handle) = scan_handles.remove(panel_id) {
        handle.abort();
    }

    let (path, sort, filter, filter_mode_active, show_hidden, version, driver, source_id, shadow_source_schema, supports_thumbs, view_mode) = {
        let mut state_guard = state.lock().await;
        let (source_id, path, sort, filter, filter_mode_active, show_hidden, version, shadow_schema) =
            if let Some(panel) = state_guard.panels.get_mut(panel_id) {
                let source_id = panel.source_id.clone();
                panel.entries.clear();
                panel.selection.clear();
                panel.status = PanelStatus::Loading;
                panel.windowed = false;
                panel.window_start = 0;
                panel.window_count = 0;
                panel.version += 1;
                let shadow_schema = panel.shadow_source.as_ref().map(|ss| ShadowSourceSchema {
                    parent_source_id: ss.parent_source_id.clone(),
                    parent_path: ss.parent_path.clone(),
                    archive_path: ss.archive_path.clone(),
                    cursor_hint: ss.cursor_hint.clone(),
                });
                (
                    source_id,
                    panel.path.clone(),
                    panel.sort.clone(),
                    panel.filter.clone(),
                    panel.filter_mode_active,
                    panel.show_hidden,
                    panel.version,
                    shadow_schema,
                )
            } else {
                return;
            };
        let driver = state_guard.drivers.get(&source_id).cloned().unwrap();
        let supports_thumbs = driver.supports_thumbnails();
        if !supports_thumbs {
            if let Some(panel) = state_guard.panels.get_mut(panel_id) {
                panel.view_mode = crate::state::PanelViewMode::List;
            }
        }
        let view_mode = state_guard
            .panels
            .get(panel_id)
            .map(|p| p.view_mode.as_str().to_string())
            .unwrap_or_else(|| "list".to_string());
        (
            path,
            sort,
            filter,
            filter_mode_active,
            show_hidden,
            version,
            driver,
            source_id,
            shadow_schema,
            supports_thumbs,
            view_mode,
        )
    };

    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id.to_string()),
            version,
            patch: serde_json::json!({
                "path": path,
                "sourceId": source_id,
                "entries": [],
                "cursor": 0,
                "selection": [],
                "selectedEntries": [],
                "status": "loading",
                "sort": { "by": sort.by, "order": sort.order },
                "filter": filter,
                "filterModeActive": filter_mode_active,
                "showHidden": show_hidden,
                "totalCount": null,
                "windowStart": null,
                "shadowSource": shadow_source_schema,
                "supportsThumbnails": supports_thumbs,
                "viewMode": view_mode,
            }),
        })
        .await;

    let handle = spawn_scan(
        panel_id.to_string(),
        path,
        sort,
        filter,
        version,
        tx.clone(),
        driver,
        cursor_hint,
        None,
        selection_names,
        Arc::clone(state),
        Arc::clone(dir_size_queue),
        show_hidden,
    );
    scan_handles.insert(panel_id.to_string(), handle);
}

pub(crate) async fn quiet_rescan_panel(
    panel_id: &str,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
    selection_names: Option<Vec<String>>,
    dir_size_queue: &Arc<DirSizeQueue>,
) {
    if let Some(handle) = scan_handles.remove(panel_id) {
        handle.abort();
    }

    let (path, sort, filter, show_hidden, version, driver) = {
        let mut state_guard = state.lock().await;
        let (source_id, path, sort, filter, show_hidden, version) =
            if let Some(panel) = state_guard.panels.get_mut(panel_id) {
                panel.version += 1;
                (
                    panel.source_id.clone(),
                    panel.path.clone(),
                    panel.sort.clone(),
                    panel.filter.clone(),
                    panel.show_hidden,
                    panel.version,
                )
            } else {
                return;
            };
        let driver = state_guard.drivers.get(&source_id).cloned().unwrap();
        (path, sort, filter, show_hidden, version, driver)
    };

    let cursor_hint = {
        let state_guard = state.lock().await;
        state_guard.panels.get(panel_id).and_then(|panel| {
            panel
                .entries
                .get(panel.cursor)
                .map(|entry| entry.name.clone())
        })
    };

    let handle = spawn_scan_atomic(
        panel_id.to_string(),
        path,
        sort,
        filter,
        version,
        tx.clone(),
        driver,
        cursor_hint,
        selection_names,
        Arc::clone(state),
        Arc::clone(dir_size_queue),
        show_hidden,
    );
    scan_handles.insert(panel_id.to_string(), handle);
}

pub(crate) async fn navigate_up_safe(
    panel_id: &str,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
    watcher_cmd_tx: &mpsc::Sender<WatcherCommand>,
    dir_size_queue: &Arc<DirSizeQueue>,
) {
    let (current_path, home) = {
        let state_guard = state.lock().await;
        let current_path = state_guard
            .panels
            .get(panel_id)
            .map(|panel| panel.path.clone())
            .unwrap_or_default();
        (current_path, state_guard.platform.home_dir())
    };

    let mut target = PathBuf::from(&current_path);
    loop {
        match target.parent() {
            Some(parent) if parent != target => {
                if parent.is_dir() {
                    let dir_name = target
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(String::from);
                    navigate_to(
                        panel_id,
                        &parent.to_string_lossy(),
                        dir_name,
                        None,
                        state,
                        tx,
                        scan_handles,
                        dir_size_queue,
                        watcher_cmd_tx,
                    )
                    .await;
                    return;
                }
                target = parent.to_path_buf();
            }
            _ => break,
        }
    }

    navigate_to(
        panel_id,
        &home,
        None,
        None,
        state,
        tx,
        scan_handles,
        dir_size_queue,
        watcher_cmd_tx,
    )
    .await;
}

pub(crate) async fn handle_volumes_changed(
    volumes: Vec<crate::platform::VolumeInfo>,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
    watcher_cmd_tx: &mpsc::Sender<WatcherCommand>,
    dir_size_queue: &Arc<DirSizeQueue>,
) {
    let (removed, sources_json, panels_to_migrate) = {
        let mut state_guard = state.lock().await;
        let removed = state_guard.rebuild_sources(volumes);

        let panels_to_migrate: Vec<String> = state_guard
            .panels
            .iter()
            .filter(|(_, panel)| removed.contains(&panel.source_id))
            .map(|(panel_id, _)| panel_id.clone())
            .collect();

        if !panels_to_migrate.is_empty() {
            let fallback_source = state_guard
                .sources
                .keys()
                .next()
                .cloned()
                .unwrap_or_default();
            let home = state_guard.platform.home_dir();
            for panel_id in &panels_to_migrate {
                if let Some(panel) = state_guard.panels.get_mut(panel_id) {
                    panel.source_id = fallback_source.clone();
                    panel.path = home.clone();
                }
            }
        }

        let sources_json = build_sources_json(&state_guard);
        (removed, sources_json, panels_to_migrate)
    };

    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "sources".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "sources": sources_json }),
        })
        .await;

    for panel_id in panels_to_migrate {
        rescan_panel(
            panel_id.as_str(),
            state,
            tx,
            scan_handles,
            None,
            None,
            dir_size_queue,
        )
        .await;
        let path_if_local = {
            let guard = state.lock().await;
            guard.panels.get(&panel_id).and_then(|panel| {
                let is_local = guard
                    .drivers
                    .get(&panel.source_id)
                    .map(|d| d.is_local())
                    .unwrap_or(false);
                if is_local {
                    Some(panel.path.clone())
                } else {
                    None
                }
            })
        };
        if let Some(path) = path_if_local {
            let _ = watcher_cmd_tx
                .send(WatcherCommand::Watch {
                    panel_id,
                    path: PathBuf::from(&path),
                })
                .await;
        }
    }

    if !removed.is_empty() {
        eprintln!("[fbro] Volumes changed: removed {:?}", removed);
    }
}

pub(crate) async fn send_window(
    panel_id: &str,
    requested_start: usize,
    count: usize,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
) {
    let (window_entries, window_start, total, version, window_version) = {
        let mut state_guard = state.lock().await;
        let result = {
            let panel = match state_guard.panels.get_mut(panel_id) {
                Some(panel) if panel.windowed => panel,
                _ => return,
            };
            let total = panel.entries.len();
            let start = requested_start.min(total.saturating_sub(1));
            let end = (start + count).min(total);
            let entries: Vec<&crate::ws::protocol::EntrySchema> =
                panel.entries[start..end].iter().collect();
            let window_entries = serde_json::json!(entries);
            panel.window_start = start;
            panel.window_count = end - start;
            // window_version is independent from `version` so a fast scroll
            // cannot be dropped by the FE structural-version guard.
            panel.window_version += 1;
            (window_entries, start, total, panel.version, panel.window_version)
        };
        // Window slice changed → refresh thumb-pool visibility while still
        // holding the lock.
        refresh_thumb_visibility(&state_guard);
        result
    };

    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id.to_string()),
            version,
            patch: serde_json::json!({
                "totalCount": total,
                "windowStart": window_start,
                "windowEntries": window_entries,
                "windowVersion": window_version,
            }),
        })
        .await;
}

pub(crate) async fn save_session(state: &SharedState, app_handle: &tauri::AppHandle) {
    use tauri::Manager;
    let state_guard = state.lock().await;
    let persist_tabs = state_guard.settings.general.persist_panel_tabs;

    // Always emit the legacy `panels` map for back-compat with older
    // builds: one entry per slot holding a listing path. If the live
    // active tab is a terminal (not persisted), fall back to any
    // surviving listing tab in the same slot.
    let panels = state_guard
        .layout_slots
        .iter()
        .filter_map(|sid| {
            let slot = state_guard.slots.get(sid)?;
            let active = state_guard.panels.get(&slot.active_panel_id);
            let panel = match active {
                Some(p) if p.tab_kind != crate::state::TabKind::Terminal => Some(p),
                _ => slot
                    .tab_panel_ids
                    .iter()
                    .filter_map(|pid| state_guard.panels.get(pid))
                    .find(|p| p.tab_kind != crate::state::TabKind::Terminal),
            }?;
            Some((
                slot.slot_id.clone(),
                PanelSession {
                    path: panel.path.clone(),
                },
            ))
        })
        .collect::<std::collections::HashMap<_, _>>();

    let slots: Vec<SlotSession> = state_guard
        .layout_slots
        .iter()
        .filter_map(|sid| {
            let slot = state_guard.slots.get(sid)?;
            let ids: Vec<&String> = if persist_tabs {
                slot.tab_panel_ids.iter().collect()
            } else {
                vec![&slot.active_panel_id]
            };
            // Build the kept (listing-only) tab list and remember each kept
            // tab's original panel id so we can find the saved active index
            // after terminal tabs are dropped.
            let mut tabs: Vec<TabSession> = Vec::with_capacity(ids.len());
            let mut kept_ids: Vec<&String> = Vec::with_capacity(ids.len());
            for pid in &ids {
                let Some(panel) = state_guard.panels.get(*pid) else {
                    continue;
                };
                // Terminal tabs are intentionally dropped — the PTY can't
                // survive a process restart and the user shouldn't see
                // resurrected stand-ins for it.
                if panel.tab_kind == crate::state::TabKind::Terminal {
                    continue;
                }
                tabs.push(TabSession::Listing {
                    source_id: Some(panel.source_id.clone()),
                    path: panel.path.clone(),
                });
                kept_ids.push(*pid);
            }
            if tabs.is_empty() {
                return None;
            }
            // Map the live active panel id onto an index in the kept list.
            // If the active tab was a terminal (and thus dropped), fall back
            // to the nearest surviving sibling.
            let active_index = kept_ids
                .iter()
                .position(|id| *id == &slot.active_panel_id)
                .or_else(|| {
                    let active_idx = slot
                        .tab_panel_ids
                        .iter()
                        .position(|id| id == &slot.active_panel_id)?;
                    // Walk left, then right, for the nearest kept panel id.
                    for i in (0..active_idx).rev() {
                        let id = &slot.tab_panel_ids[i];
                        if let Some(p) = kept_ids.iter().position(|k| *k == id) {
                            return Some(p);
                        }
                    }
                    for i in (active_idx + 1)..slot.tab_panel_ids.len() {
                        let id = &slot.tab_panel_ids[i];
                        if let Some(p) = kept_ids.iter().position(|k| *k == id) {
                            return Some(p);
                        }
                    }
                    Some(0)
                })
                .unwrap_or(0);
            Some(SlotSession {
                slot_id: slot.slot_id.clone(),
                active_index,
                tabs,
            })
        })
        .collect();

    let window_session = if state_guard.settings.general.remember_window_state {
        app_handle.get_webview_window("main").and_then(|w| {
            let maximized = w.is_maximized().unwrap_or(false);
            let pos = w.outer_position().ok()?;
            let size = w.outer_size().ok()?;
            Some(crate::persistence::WindowSession {
                x: pos.x,
                y: pos.y,
                width: size.width,
                height: size.height,
                maximized,
            })
        })
    } else {
        None
    };

    crate::persistence::save(&SessionState {
        panels,
        slots,
        font_size: Some(state_guard.font_size),
        window: window_session,
    });
}

pub(crate) async fn active_job_count(state: &SharedState) -> usize {
    let state_guard = state.lock().await;
    state_guard
        .job_store
        .jobs
        .values()
        .filter(|job| {
            job.state == JobState::Queued
                || job.state == JobState::Running
                || job.state == JobState::AwaitingDecision
        })
        .count()
}

pub(crate) fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' | b'~' | b':' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

pub(crate) async fn update_panel_from_patch(
    state: &SharedState,
    panel_id: &str,
    patch: &serde_json::Value,
    version: u64,
) {
    let mut state_guard = state.lock().await;
    if let Some(panel) = state_guard.panels.get_mut(panel_id) {
        // Treat null-valued fields (sent by rescan/navigate to *clear* state) as
        // "not present" — those patches are structural resets, not window slices.
        let has_total = patch
            .get("totalCount")
            .is_some_and(|v| !v.is_null());
        let has_window_entries = patch
            .get("windowEntries")
            .is_some_and(|v| !v.is_null());
        let is_window_patch = has_total || has_window_entries;
        // A "window-only" patch (the kind send_window emits) has no structural
        // fields — its sole purpose is to update the visible slice and must not
        // bump the structural version, otherwise it can race with concurrent
        // structural patches and leave the FE with mismatched windowStart/entries.
        let is_window_only = has_window_entries
            && patch.get("status").is_none()
            && patch.get("cursor").is_none()
            && patch.get("selection").is_none()
            && patch.get("entries").is_none()
            && patch.get("appendEntries").is_none();

        if !is_window_patch {
            if let Some(entries) = patch
                .get("appendEntries")
                .and_then(|value| value.as_array())
            {
                for entry in entries {
                    panel.entries.push(crate::ws::protocol::EntrySchema {
                        name: entry["name"].as_str().unwrap_or("").to_string(),
                        kind: entry["kind"].as_str().unwrap_or("file").to_string(),
                        size: entry["size"].as_u64(),
                        mtime: entry["mtime"].as_str().map(String::from),
                        btime: entry["btime"].as_str().map(String::from),
                        permissions: entry["permissions"].as_str().map(String::from),
                        icon_name: entry["iconName"].as_str().map(String::from),
                        cc: entry["cc"].as_str().map(String::from),
                        source_path: entry["sourcePath"].as_str().map(String::from),
                    });
                }
            }
        }

        if let Some(status) = patch.get("status").and_then(|value| value.as_str()) {
            panel.status = match status {
                "ready" => PanelStatus::Ready,
                "error" => PanelStatus::Error(String::new()),
                _ => PanelStatus::Loading,
            };
        }
        if let Some(cursor) = patch.get("cursor").and_then(|value| value.as_u64()) {
            panel.cursor = cursor as usize;
        }
        if let Some(selection) = patch.get("selection").and_then(|value| value.as_array()) {
            panel.selection = selection
                .iter()
                .filter_map(|value| value.as_u64().map(|index| index as usize))
                .collect();
        }
        if let Some(filter) = patch.get("filter") {
            panel.filter = serde_json::from_value(filter.clone()).ok().flatten();
        }
        if let Some(filter_mode_active) = patch
            .get("filterModeActive")
            .and_then(|value| value.as_bool())
        {
            panel.filter_mode_active = filter_mode_active;
        }
        if let Some(show_hidden) = patch.get("showHidden").and_then(|value| value.as_bool()) {
            panel.show_hidden = show_hidden;
        }
        if let Some(window_start) = patch.get("windowStart").and_then(|value| value.as_u64()) {
            panel.window_start = window_start as usize;
        }
        if let Some(window_version) = patch.get("windowVersion").and_then(|value| value.as_u64()) {
            panel.window_version = window_version;
        }
        if !is_window_only {
            panel.version = version;
        }
    }
    // Visibility set may have changed (new entries, new window slice, mode
    // change) — let the thumb pool re-prioritize.
    refresh_thumb_visibility(&state_guard);
}

fn refresh_thumb_visibility(state: &crate::state::AppState) {
    let paths = crate::state::collect_visible_thumb_paths(state);
    state.thumb_pool.set_visible(paths);
}

fn build_panel_snapshot(panel: &Panel, supports_thumbnails: bool) -> PanelSnapshot {
    let mut calculating_size: Vec<String> = panel.calculating_size.iter().cloned().collect();
    calculating_size.sort();
    PanelSnapshot {
        panel_id: panel.panel_id.clone(),
        slot_id: panel.slot_id.clone(),
        tab_kind: panel.tab_kind.as_str().to_string(),
        tab_title: panel.tab_title_override.clone(),
        path: panel.path.clone(),
        status: panel.status.as_str().to_string(),
        source_id: panel.source_id.clone(),
        sort: panel.sort.clone(),
        filter: panel.filter.clone(),
        filter_mode_active: panel.filter_mode_active,
        show_hidden: panel.show_hidden,
        view_mode: panel.view_mode.as_str().to_string(),
        supports_thumbnails,
        cursor: panel.cursor,
        selection: panel.selection.clone(),
        selected_entries: selected_entries_for_panel(panel),
        entries: panel.entries.clone(),
        version: panel.version,
        calculating_size,
        total_count: None,
        window_start: None,
        shadow_source: panel.shadow_source.as_ref().map(|ss| ShadowSourceSchema {
            parent_source_id: ss.parent_source_id.clone(),
            parent_path: ss.parent_path.clone(),
            archive_path: ss.archive_path.clone(),
            cursor_hint: ss.cursor_hint.clone(),
        }),
        terminal_session_id: panel.terminal_session,
    }
}

fn spawn_scan(
    panel_id: String,
    path: String,
    sort: crate::ws::protocol::SortSpec,
    filter: Option<crate::ws::protocol::FilterSpec>,
    version_base: u64,
    tx: mpsc::Sender<ServerMessage>,
    driver: Arc<dyn VfsDriver>,
    cursor_hint: Option<String>,
    cursor_index: Option<usize>,
    selection_names: Option<Vec<String>>,
    state: SharedState,
    dir_size_queue: Arc<DirSizeQueue>,
    show_hidden: bool,
) -> ScanHandle {
    let handle = tokio::spawn(run_scan(
        panel_id,
        path,
        sort,
        filter,
        version_base,
        tx,
        driver,
        cursor_hint,
        cursor_index,
        selection_names,
        state,
        dir_size_queue,
        false,
        show_hidden,
    ));
    ScanHandle { handle }
}

fn spawn_scan_atomic(
    panel_id: String,
    path: String,
    sort: crate::ws::protocol::SortSpec,
    filter: Option<crate::ws::protocol::FilterSpec>,
    version_base: u64,
    tx: mpsc::Sender<ServerMessage>,
    driver: Arc<dyn VfsDriver>,
    cursor_hint: Option<String>,
    selection_names: Option<Vec<String>>,
    state: SharedState,
    dir_size_queue: Arc<DirSizeQueue>,
    show_hidden: bool,
) -> ScanHandle {
    let handle = tokio::spawn(run_scan(
        panel_id,
        path,
        sort,
        filter,
        version_base,
        tx,
        driver,
        cursor_hint,
        None,
        selection_names,
        state,
        dir_size_queue,
        true,
        show_hidden,
    ));
    ScanHandle { handle }
}
