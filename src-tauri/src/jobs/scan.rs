use std::sync::Arc;
use tokio::sync::mpsc;

use crate::jobs::dirsize::DirSizeQueue;
use crate::jobs::ScanHandle;
use crate::state::SharedState;
use crate::vfs::{VfsDriver, VfsError};
use crate::watcher::WatcherCommand;
use crate::ws::protocol::{EntrySchema, FileOpFileSchema, FilterSpec, ServerMessage, SortSpec};

const BATCH_SIZE: usize = 50;
pub const WINDOWED_THRESHOLD: usize = 1500;
pub const INITIAL_WINDOW_SIZE: usize = 200;

fn build_selected_entries(entries: &[EntrySchema], selection: &[usize]) -> Vec<FileOpFileSchema> {
    selection
        .iter()
        .filter_map(|&index| {
            entries.get(index).map(|entry| FileOpFileSchema {
                name: entry.name.clone(),
                size: entry.size,
                kind: entry.kind.clone(),
                source_path: None,
            })
        })
        .collect()
}

fn should_recover_scan_error(error: &VfsError) -> bool {
    matches!(error, VfsError::NotFound(_) | VfsError::IOError(_))
}

struct RecoveryScan {
    path: String,
    sort: SortSpec,
    version: u64,
    driver: Arc<dyn VfsDriver>,
    show_hidden: bool,
}

async fn prepare_panel_home_recovery(
    panel_id: &str,
    reason: impl Into<String>,
    tx: &mpsc::Sender<ServerMessage>,
    state: &SharedState,
    watcher_cmd_tx: &mpsc::Sender<WatcherCommand>,
) -> Option<RecoveryScan> {
    let reason = reason.into();
    let (path, sort, show_hidden, version, driver, source_id, supports_thumbs, view_mode) = {
        let mut state_guard = state.lock().await;
        let fallback_source_id = state_guard.fallback_source_id_for_home()?;
        let home = state_guard.platform.home_dir();
        let driver = state_guard.drivers.get(&fallback_source_id)?.clone();
        let supports_thumbs = driver.supports_thumbnails();

        let panel = state_guard.panels.get_mut(panel_id)?;
        panel.source_id = fallback_source_id.clone();
        panel.path = home.clone();
        panel.entries.clear();
        panel.cursor = 0;
        panel.selection.clear();
        panel.status = crate::state::PanelStatus::Loading;
        panel.windowed = false;
        panel.window_start = 0;
        panel.window_count = 0;
        panel.filter = None;
        panel.filter_mode_active = false;
        panel.shadow_source = None;
        panel.version += 1;
        if !supports_thumbs {
            panel.view_mode = crate::state::PanelViewMode::List;
        }

        (
            home,
            panel.sort.clone(),
            panel.show_hidden,
            panel.version,
            driver,
            fallback_source_id,
            supports_thumbs,
            panel.view_mode.as_str().to_string(),
        )
    };

    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: crate::ws::protocol::now(),
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
                "filter": null,
                "filterModeActive": false,
                "totalCount": null,
                "totalSize": null,
                "windowStart": null,
                "shadowSource": null,
                "supportsThumbnails": supports_thumbs,
                "viewMode": view_mode,
            }),
        })
        .await;

    let message = if reason.is_empty() {
        "Panel source became unavailable. Recovered to home.".to_string()
    } else {
        format!("Panel source became unavailable. Recovered to home. {reason}")
    };
    let _ = tx
        .send(ServerMessage::Error {
            timestamp: crate::ws::protocol::now(),
            code: "PANEL_RECOVERED_HOME".to_string(),
            message,
            recoverable: true,
            extra: Some(serde_json::json!({ "panelId": panel_id })),
        })
        .await;

    if driver.is_local() {
        let _ = watcher_cmd_tx
            .send(WatcherCommand::Watch {
                panel_id: panel_id.to_string(),
                path: std::path::PathBuf::from(&path),
            })
            .await;
    } else {
        let _ = watcher_cmd_tx
            .send(WatcherCommand::Unwatch {
                panel_id: panel_id.to_string(),
            })
            .await;
    }

    Some(RecoveryScan {
        path,
        sort,
        version,
        driver,
        show_hidden,
    })
}

pub async fn recover_panel_to_home(
    panel_id: &str,
    reason: impl Into<String>,
    tx: &mpsc::Sender<ServerMessage>,
    state: &SharedState,
    dir_size_queue: &Arc<DirSizeQueue>,
    watcher_cmd_tx: &mpsc::Sender<WatcherCommand>,
) -> Option<ScanHandle> {
    let recovery = prepare_panel_home_recovery(panel_id, reason, tx, state, watcher_cmd_tx).await?;
    let handle = tokio::spawn(run_scan(
        panel_id.to_string(),
        recovery.path,
        recovery.sort,
        None,
        recovery.version,
        tx.clone(),
        recovery.driver,
        None,
        None,
        None,
        Arc::clone(state),
        Arc::clone(dir_size_queue),
        false,
        recovery.show_hidden,
        watcher_cmd_tx.clone(),
        false,
    ));
    Some(ScanHandle { handle })
}

pub async fn run_scan(
    panel_id: String,
    mut path: String,
    mut sort: SortSpec,
    mut filter: Option<FilterSpec>,
    mut version_base: u64,
    tx: mpsc::Sender<ServerMessage>,
    mut driver: Arc<dyn VfsDriver>,
    mut cursor_hint: Option<String>,
    mut cursor_index: Option<usize>,
    mut selection_names: Option<Vec<String>>,
    state: SharedState,
    dir_size_queue: Arc<DirSizeQueue>,
    atomic: bool,
    mut show_hidden: bool,
    watcher_cmd_tx: mpsc::Sender<WatcherCommand>,
    mut allow_recovery: bool,
) {
    let entries = loop {
        let path_clone = path.clone();
        let sort_clone = sort.clone();
        let filter_clone = filter.clone();
        let scan_driver = Arc::clone(&driver);
        let result = tokio::task::spawn_blocking(move || {
            scan_driver.list(&path_clone, &sort_clone, filter_clone.as_ref(), show_hidden)
        })
        .await;

        match result {
            Ok(Ok(e)) => break e,
            Ok(Err(err)) => {
                if allow_recovery && should_recover_scan_error(&err) {
                    if let Some(recovery) = prepare_panel_home_recovery(
                        &panel_id,
                        format!("Scan failed: {err}"),
                        &tx,
                        &state,
                        &watcher_cmd_tx,
                    )
                    .await
                    {
                        path = recovery.path;
                        sort = recovery.sort;
                        filter = None;
                        version_base = recovery.version;
                        driver = recovery.driver;
                        cursor_hint = None;
                        cursor_index = None;
                        selection_names = None;
                        show_hidden = recovery.show_hidden;
                        allow_recovery = false;
                        continue;
                    }
                }
                let _ = tx
                    .send(ServerMessage::StatePatch {
                        timestamp: crate::ws::protocol::now(),
                        domain: "panels".to_string(),
                        entity_id: Some(panel_id.clone()),
                        version: version_base + 1,
                        patch: serde_json::json!({ "status": "error", "error": err.to_string() }),
                    })
                    .await;
                return;
            }
            Err(_) => return,
        }
    };

    let mut schema_entries: Vec<EntrySchema> = entries
        .into_iter()
        .map(|e| EntrySchema {
            name: e.name,
            kind: e.kind.as_str().to_string(),
            size: e.size,
            mtime: e.mtime,
            btime: e.btime,
            permissions: e.permissions,
            icon_name: e.icon_name,
            cc: e.cc,
            source_path: e.source_path,
        })
        .collect();

    let total = schema_entries.len();

    // Apply cached directory sizes so returning to a folder restores previously
    // computed sizes without requiring another Shift+Space.
    let panel_path = path.trim_end_matches('/');
    for entry in &mut schema_entries {
        if entry.kind == "directory" {
            let full = format!("{}/{}", panel_path, entry.name);
            if let Some(sz) = dir_size_queue.get_cached_size(&full).await {
                entry.size = Some(sz);
            }
        }
    }

    // When sorting by size, re-sort the directory prefix using the now-populated
    // cached sizes. Directories without a computed size sort last within the group
    // (u64::MAX so they appear "largest" in ascending, avoiding them crowding the
    // front where the small/known-size folders should be).
    if sort.by == "size" {
        let dir_end = schema_entries
            .iter()
            .position(|e| e.kind != "directory")
            .unwrap_or(schema_entries.len());
        let desc = sort.order == "desc";
        schema_entries[..dir_end].sort_by(|a, b| {
            let ord = a.size.unwrap_or(u64::MAX).cmp(&b.size.unwrap_or(u64::MAX));
            if desc {
                ord.reverse()
            } else {
                ord
            }
        });
    }

    // Sum sizes of all file entries (directories excluded — their size is optional/computed separately).
    let total_file_size: u64 = schema_entries
        .iter()
        .filter(|e| e.kind == "file")
        .filter_map(|e| e.size)
        .sum();

    let cursor = cursor_index
        .filter(|&i| i < schema_entries.len())
        .or_else(|| {
            cursor_hint
                .as_deref()
                .and_then(|hint| schema_entries.iter().position(|e| e.name == hint))
        })
        .unwrap_or(0);

    let selection: Vec<usize> = match selection_names {
        Some(names) => {
            let name_set: std::collections::HashSet<_> = names.into_iter().collect();
            schema_entries
                .iter()
                .enumerate()
                .filter(|(_, e)| name_set.contains(&e.name))
                .map(|(i, _)| i)
                .collect()
        }
        None => vec![],
    };

    if total > WINDOWED_THRESHOLD {
        // ── Mode B: windowed ─────────────────────────────────────────────
        let window_count = INITIAL_WINDOW_SIZE.min(total);
        let max_start = total.saturating_sub(window_count);
        let cursor_centered_start = cursor.saturating_sub(window_count / 2).min(max_start);
        let selected_entries = build_selected_entries(&schema_entries, &selection);

        // Decide window_start AND store entries in one lock block. If the panel
        // already has a non-zero window_start the user has scrolled away from
        // the cursor (this is a quiet/background rescan, not a fresh navigation
        // or sort) — preserve that scroll position so the visible content
        // doesn't snap back to the top.
        let (window_start, window_json, window_version) = {
            let mut s = state.lock().await;
            if let Some(panel) = s.panels.get_mut(&panel_id) {
                let existing_start = panel.window_start;
                let window_start = if existing_start > 0 && existing_start <= max_start {
                    existing_start
                } else {
                    cursor_centered_start
                };
                let window_json =
                    serde_json::json!(&schema_entries[window_start..window_start + window_count]);
                panel.entries = schema_entries;
                panel.cursor = cursor;
                panel.selection = selection.clone();
                panel.windowed = true;
                panel.window_start = window_start;
                panel.window_count = window_count;
                panel.window_version += 1;
                panel.status = crate::state::PanelStatus::Ready;
                panel.version = version_base + 1;
                (window_start, window_json, panel.window_version)
            } else {
                return;
            }
        };

        let _ = tx
            .send(ServerMessage::StatePatch {
                timestamp: crate::ws::protocol::now(),
                domain: "panels".to_string(),
                entity_id: Some(panel_id),
                version: version_base + 1,
                patch: serde_json::json!({
                    "status": "ready",
                    "totalCount": total,
                    "totalSize": total_file_size,
                    "windowStart": window_start,
                    "windowEntries": window_json,
                    "windowVersion": window_version,
                    "cursor": cursor,
                    "selection": selection,
                    "selectedEntries": selected_entries,
                }),
            })
            .await;
    } else if atomic {
        // ── Mode A-atomic: single replacement (no flash for watcher rescans) ─
        let version = version_base + 1;

        // Update BE state atomically
        {
            let mut s = state.lock().await;
            if let Some(panel) = s.panels.get_mut(&panel_id) {
                panel.entries = schema_entries.clone();
                panel.cursor = cursor;
                panel.selection = selection.clone();
                panel.windowed = false;
                panel.status = crate::state::PanelStatus::Ready;
                panel.version = version;
            }
        }

        let _ = tx
            .send(ServerMessage::StatePatch {
                timestamp: crate::ws::protocol::now(),
                domain: "panels".to_string(),
                entity_id: Some(panel_id),
                version,
                patch: serde_json::json!({
                    "entries": schema_entries,
                    "status": "ready",
                    "total": total,
                    "totalSize": total_file_size,
                    "cursor": cursor,
                    "selection": selection,
                    "selectedEntries": build_selected_entries(&schema_entries, &selection),
                }),
            })
            .await;
    } else {
        // ── Mode A: full list (streamed) ─────────────────────────────────
        let mut version = version_base;

        for (batch_idx, chunk) in schema_entries.chunks(BATCH_SIZE).enumerate() {
            version += 1;
            let patch = serde_json::json!({ "appendEntries": chunk });
            if tx
                .send(ServerMessage::StatePatch {
                    timestamp: crate::ws::protocol::now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id.clone()),
                    version,
                    patch,
                })
                .await
                .is_err()
            {
                return;
            }

            if batch_idx % 4 == 3 {
                tokio::task::yield_now().await;
            }
        }

        version += 1;
        let selected_entries = build_selected_entries(&schema_entries, &selection);
        let _ = tx
            .send(ServerMessage::StatePatch {
                timestamp: crate::ws::protocol::now(),
                domain: "panels".to_string(),
                entity_id: Some(panel_id),
                version,
                patch: serde_json::json!({
                    "status": "ready",
                    "total": total,
                    "totalSize": total_file_size,
                    "cursor": cursor,
                    "selection": selection,
                    "selectedEntries": selected_entries,
                }),
            })
            .await;
    }
}
