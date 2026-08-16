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

#[derive(Debug, PartialEq)]
struct LiveListingState {
    cursor: usize,
    selection: Vec<usize>,
    anchor_delta: Option<i64>,
}

/// Reconcile interaction state against a replacement listing at commit time.
/// Using the panel's current entries ensures interactions made while the scan
/// was running are not overwritten by scan-start state.
fn reconcile_live_listing(
    old_entries: &[EntrySchema],
    old_cursor: usize,
    old_selection: &[usize],
    new_entries: &[EntrySchema],
) -> LiveListingState {
    let old_cursor = old_cursor.min(old_entries.len().saturating_sub(1));
    let cursor_name = old_entries.get(old_cursor).map(|entry| entry.name.as_str());
    let anchored_index =
        cursor_name.and_then(|name| new_entries.iter().position(|entry| entry.name == name));
    let cursor =
        anchored_index.unwrap_or_else(|| old_cursor.min(new_entries.len().saturating_sub(1)));

    let selected_names: std::collections::HashSet<&str> = old_selection
        .iter()
        .filter_map(|&index| old_entries.get(index).map(|entry| entry.name.as_str()))
        .collect();
    let selection = new_entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| selected_names.contains(entry.name.as_str()))
        .map(|(index, _)| index)
        .collect();

    LiveListingState {
        cursor,
        selection,
        anchor_delta: anchored_index.map(|index| index as i64 - old_cursor as i64),
    }
}

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
    source_id: String,
    path: String,
    sort: SortSpec,
    version: u64,
    panel_epoch: u64,
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
    let (
        path,
        sort,
        show_hidden,
        version,
        panel_epoch,
        driver,
        source_id,
        supports_thumbs,
        view_mode,
    ) = {
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
        panel.panel_epoch += 1;
        if !supports_thumbs {
            panel.view_mode = crate::state::PanelViewMode::List;
        }

        (
            home,
            panel.sort.clone(),
            panel.show_hidden,
            panel.version,
            panel.panel_epoch,
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
                "panelEpoch": panel_epoch,
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
        source_id,
        path,
        sort,
        version,
        panel_epoch,
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
        recovery.source_id,
        recovery.panel_epoch,
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
    mut source_id: String,
    mut panel_epoch: u64,
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
    eprintln!(
        "[scan] started: panel={panel_id} source={source_id} path={path} epoch={panel_epoch} version_base={version_base} atomic={atomic}"
    );
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
            Ok(Ok(e)) => {
                eprintln!(
                    "[scan] listed: panel={panel_id} source={source_id} path={path} epoch={panel_epoch} entries={}",
                    e.len()
                );
                break e;
            }
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
                        source_id = recovery.source_id;
                        panel_epoch = recovery.panel_epoch;
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
                        patch: serde_json::json!({
                            "panelEpoch": panel_epoch,
                            "status": "error",
                            "error": err.to_string()
                        }),
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
            git_status: None,
            git_details: None,
        })
        .collect();

    let git_status = if driver.is_local() {
        let path = path.clone();
        tokio::task::spawn_blocking(move || crate::git::inspect(&path)).await.ok().flatten()
    } else { None };
    crate::git::decorate_entries(&mut schema_entries, &path, git_status.as_ref());

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

    let initial_cursor = cursor_index
        .filter(|&i| i < schema_entries.len())
        .or_else(|| {
            cursor_hint
                .as_deref()
                .and_then(|hint| schema_entries.iter().position(|e| e.name == hint))
        })
        .unwrap_or(0);

    let initial_selection: Vec<usize> = match selection_names {
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

        // Decide window_start AND store entries in one lock block. If the panel
        // already has a non-zero window_start the user has scrolled away from
        // the cursor (this is a quiet/background rescan, not a fresh navigation
        // or sort) — preserve that scroll position so the visible content
        // doesn't snap back to the top.
        let (
            window_start,
            window_json,
            window_version,
            version,
            cursor,
            selection,
            selected_entries,
            anchor_delta,
        ) = {
            let mut s = state.lock().await;
            if let Some(panel) = s.panels.get_mut(&panel_id) {
                if panel.panel_epoch != panel_epoch
                    || panel.source_id != source_id
                    || panel.path != path
                {
                    eprintln!(
                        "[scan] stale windowed scan skipped: panel={panel_id} source={source_id} path={path} epoch={panel_epoch} current_source={} current_path={} current_epoch={}",
                        panel.source_id,
                        panel.path,
                        panel.panel_epoch
                    );
                    return;
                }
                let live = atomic.then(|| {
                    reconcile_live_listing(
                        &panel.entries,
                        panel.cursor,
                        &panel.selection,
                        &schema_entries,
                    )
                });
                let cursor = live.as_ref().map_or(initial_cursor, |state| state.cursor);
                let selection = live.as_ref().map_or_else(
                    || initial_selection.clone(),
                    |state| state.selection.clone(),
                );
                let anchor_delta = live.as_ref().and_then(|state| state.anchor_delta);
                let cursor_centered_start = cursor.saturating_sub(window_count / 2).min(max_start);
                let existing_start = panel.window_start;
                let anchored_start = anchor_delta.map(|delta| {
                    existing_start
                        .saturating_add_signed(delta as isize)
                        .min(max_start)
                });
                let window_start = if let Some(start) = anchored_start {
                    start
                } else if existing_start > 0 && existing_start <= max_start {
                    existing_start
                } else {
                    cursor_centered_start
                };
                let window_json =
                    serde_json::json!(&schema_entries[window_start..window_start + window_count]);
                let selected_entries = build_selected_entries(&schema_entries, &selection);
                panel.entries = schema_entries;
                panel.cursor = cursor;
                panel.selection = selection.clone();
                panel.windowed = true;
                panel.window_start = window_start;
                panel.window_count = window_count;
                panel.window_version += 1;
                panel.status = crate::state::PanelStatus::Ready;
                panel.git = git_status.clone();
                panel.version = if atomic {
                    panel.version + 1
                } else {
                    version_base + 1
                };
                (
                    window_start,
                    window_json,
                    panel.window_version,
                    panel.version,
                    cursor,
                    selection,
                    selected_entries,
                    anchor_delta,
                )
            } else {
                return;
            }
        };

        let _ = tx
            .send(ServerMessage::StatePatch {
                timestamp: crate::ws::protocol::now(),
                domain: "panels".to_string(),
                entity_id: Some(panel_id.clone()),
                version,
                patch: serde_json::json!({
                    "status": "ready",
                    "panelEpoch": panel_epoch,
                    "totalCount": total,
                    "totalSize": total_file_size,
                    "windowStart": window_start,
                    "windowEntries": window_json,
                    "windowVersion": window_version,
                    "cursor": cursor,
                    "selection": selection,
                    "selectedEntries": selected_entries,
                    "viewportAnchor": anchor_delta.map(|delta| serde_json::json!({
                        "version": version,
                        "delta": delta,
                    })),
                    "git": git_status,
                }),
            })
            .await;
        eprintln!(
            "[scan] ready sent: panel={panel_id} source={source_id} path={path} epoch={panel_epoch} version={} mode=windowed total={total}",
            version
        );
    } else if atomic {
        // ── Mode A-atomic: single replacement (no flash for watcher rescans) ─
        // Reconcile and update under one lock so interactions made while the
        // scan ran win over the scan-start snapshot.
        let (version, cursor, selection, anchor_delta) = {
            let mut s = state.lock().await;
            if let Some(panel) = s.panels.get_mut(&panel_id) {
                if panel.panel_epoch != panel_epoch
                    || panel.source_id != source_id
                    || panel.path != path
                {
                    eprintln!(
                        "[scan] stale atomic scan skipped: panel={panel_id} source={source_id} path={path} epoch={panel_epoch} current_source={} current_path={} current_epoch={}",
                        panel.source_id,
                        panel.path,
                        panel.panel_epoch
                    );
                    return;
                }
                let live = reconcile_live_listing(
                    &panel.entries,
                    panel.cursor,
                    &panel.selection,
                    &schema_entries,
                );
                let version = panel.version + 1;
                panel.entries = schema_entries.clone();
                panel.cursor = live.cursor;
                panel.selection = live.selection.clone();
                panel.windowed = false;
                panel.status = crate::state::PanelStatus::Ready;
                panel.version = version;
                panel.git = git_status.clone();
                (version, live.cursor, live.selection, live.anchor_delta)
            } else {
                return;
            }
        };

        let sent = tx
            .send(ServerMessage::StatePatch {
                timestamp: crate::ws::protocol::now(),
                domain: "panels".to_string(),
                entity_id: Some(panel_id.clone()),
                version,
                patch: serde_json::json!({
                    "panelEpoch": panel_epoch,
                    "entries": schema_entries,
                    "status": "ready",
                    "total": total,
                    "totalSize": total_file_size,
                    "cursor": cursor,
                    "selection": selection,
                    "selectedEntries": build_selected_entries(&schema_entries, &selection),
                    "viewportAnchor": anchor_delta.map(|delta| serde_json::json!({
                        "version": version,
                        "delta": delta,
                    })),
                    "git": git_status,
                }),
            })
            .await;
        match sent {
            Ok(()) => eprintln!(
                "[scan] ready sent: panel={panel_id} source={source_id} path={path} epoch={panel_epoch} version={version} mode=atomic total={total}"
            ),
            Err(error) => eprintln!(
                "[scan] ready send failed: panel={panel_id} source={source_id} path={path} epoch={panel_epoch} version={version} mode=atomic error={error}"
            ),
        }
    } else {
        // ── Mode A: full list (streamed) ─────────────────────────────────
        let mut version = version_base;

        for (batch_idx, chunk) in schema_entries.chunks(BATCH_SIZE).enumerate() {
            version += 1;
            let patch = serde_json::json!({ "panelEpoch": panel_epoch, "appendEntries": chunk });
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
        let selected_entries = build_selected_entries(&schema_entries, &initial_selection);
        {
            let mut s = state.lock().await;
            let Some(panel) = s.panels.get_mut(&panel_id) else {
                return;
            };
            if panel.panel_epoch != panel_epoch
                || panel.source_id != source_id
                || panel.path != path
            {
                eprintln!(
                    "[scan] stale streamed scan skipped: panel={panel_id} source={source_id} path={path} epoch={panel_epoch} current_source={} current_path={} current_epoch={}",
                    panel.source_id,
                    panel.path,
                    panel.panel_epoch
                );
                return;
            }
            panel.git = git_status.clone();
        }
        let sent = tx
            .send(ServerMessage::StatePatch {
                timestamp: crate::ws::protocol::now(),
                domain: "panels".to_string(),
                entity_id: Some(panel_id.clone()),
                version,
                patch: serde_json::json!({
                    "panelEpoch": panel_epoch,
                    "status": "ready",
                    "total": total,
                    "totalSize": total_file_size,
                    "cursor": initial_cursor,
                    "selection": initial_selection,
                    "selectedEntries": selected_entries,
                    "git": git_status,
                }),
            })
            .await;
        match sent {
            Ok(()) => eprintln!(
                "[scan] ready sent: panel={panel_id} source={source_id} path={path} epoch={panel_epoch} version={version} mode=streamed total={total}"
            ),
            Err(error) => eprintln!(
                "[scan] ready send failed: panel={panel_id} source={source_id} path={path} epoch={panel_epoch} version={version} mode=streamed error={error}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> EntrySchema {
        EntrySchema {
            name: name.to_string(),
            kind: "file".to_string(),
            size: None,
            mtime: None,
            btime: None,
            permissions: None,
            icon_name: None,
            cc: None,
            source_path: None,
            git_status: None,
            git_details: None,
        }
    }

    fn entries(names: &[&str]) -> Vec<EntrySchema> {
        names.iter().map(|name| entry(name)).collect()
    }

    #[test]
    fn live_listing_tracks_cursor_and_selection_across_prepends() {
        let old = entries(&["c", "b", "a"]);
        let new = entries(&["e", "d", "c", "b", "a"]);

        assert_eq!(
            reconcile_live_listing(&old, 1, &[0, 1], &new),
            LiveListingState {
                cursor: 3,
                selection: vec![2, 3],
                anchor_delta: Some(2),
            }
        );
    }

    #[test]
    fn live_listing_uses_the_interaction_state_present_at_commit() {
        let old = entries(&["c", "b", "a"]);
        let new = entries(&["e", "d", "c", "b", "a"]);

        assert_eq!(
            reconcile_live_listing(&old, 2, &[2], &new),
            LiveListingState {
                cursor: 4,
                selection: vec![4],
                anchor_delta: Some(2),
            }
        );
    }

    #[test]
    fn live_listing_falls_back_when_cursor_entry_disappears() {
        let old = entries(&["c", "b", "a"]);
        let new = entries(&["c", "a"]);

        assert_eq!(
            reconcile_live_listing(&old, 1, &[0, 1], &new),
            LiveListingState {
                cursor: 1,
                selection: vec![0],
                anchor_delta: None,
            }
        );
    }
}
