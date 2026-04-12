use std::sync::Arc;
use tokio::sync::mpsc;

use crate::jobs::dirsize::DirSizeQueue;
use crate::state::SharedState;
use crate::vfs::VfsDriver;
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

pub async fn run_scan(
    panel_id: String,
    path: String,
    sort: SortSpec,
    filter: Option<FilterSpec>,
    version_base: u64,
    tx: mpsc::Sender<ServerMessage>,
    driver: Arc<dyn VfsDriver>,
    cursor_hint: Option<String>,
    selection_names: Option<Vec<String>>,
    state: SharedState,
    dir_size_queue: Arc<DirSizeQueue>,
    atomic: bool,
    show_hidden: bool,
) {
    let path_clone = path.clone();
    let result =
        tokio::task::spawn_blocking(move || driver.list(&path_clone, &sort, filter.as_ref(), show_hidden)).await;

    let entries = match result {
        Ok(Ok(e)) => e,
        Ok(Err(err)) => {
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

    // Sum sizes of all file entries (directories excluded — their size is optional/computed separately).
    let total_file_size: u64 = schema_entries
        .iter()
        .filter(|e| e.kind == "file")
        .filter_map(|e| e.size)
        .sum();

    let cursor = cursor_hint
        .as_deref()
        .and_then(|hint| schema_entries.iter().position(|e| e.name == hint))
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
        let window_start = cursor
            .saturating_sub(window_count / 2)
            .min(total.saturating_sub(window_count));
        let window_json =
            serde_json::json!(&schema_entries[window_start..window_start + window_count]);
        let selected_entries = build_selected_entries(&schema_entries, &selection);

        // Store full entries directly in BE state
        {
            let mut s = state.lock().await;
            if let Some(panel) = s.panels.get_mut(&panel_id) {
                panel.entries = schema_entries;
                panel.cursor = cursor;
                panel.selection = selection.clone();
                panel.windowed = true;
                panel.window_start = window_start;
                panel.window_count = window_count;
                panel.status = crate::state::PanelStatus::Ready;
                panel.version = version_base + 1;
            }
        }

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
