use std::sync::Arc;

use serde_json::Value;

use crate::bookmarks::BookmarkEntry;
use crate::state::{ShadowSource, Source};
use crate::vfs::VfsDriver;
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::intents::navigation::teardown_shadow_source;
use crate::ws::runtime::{build_sources_json, navigate_to, rescan_panel, resolve_panel_id, IntentContext};

use super::arg_str;

pub async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "AddBookmark" => add_bookmark(args, ctx).await,
        "RemoveBookmark" => remove_bookmark(args, ctx).await,
        "EnterBookmarkSource" => enter_bookmark_source(scope, ctx).await,
        _ => {}
    }
}

async fn add_bookmark(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(path) = arg_str(args, "path").map(str::to_string) else { return };
    let name = arg_str(args, "name")
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_name(&path));

    let mut state_guard = ctx.state.lock().await;
    if !state_guard.bookmarks.iter().any(|b| b.path == path) {
        state_guard.bookmarks.push(BookmarkEntry { name, path });
        let _ = crate::bookmarks::save(&state_guard.bookmarks);
    }
    let patch = bookmarks_patch(&state_guard.bookmarks);
    broadcast_patch(patch, &state_guard.gui_senders, ctx.tx).await;
}

async fn remove_bookmark(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(path) = arg_str(args, "path").map(str::to_string) else { return };

    let mut state_guard = ctx.state.lock().await;
    state_guard.bookmarks.retain(|b| b.path != path);
    let _ = crate::bookmarks::save(&state_guard.bookmarks);
    let patch = bookmarks_patch(&state_guard.bookmarks);
    broadcast_patch(patch, &state_guard.gui_senders, ctx.tx).await;
}

async fn enter_bookmark_source(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;

    let source_id = "bookmarks".to_string();

    {
        let mut state_guard = ctx.state.lock().await;

        let panel = match state_guard.panels.get(&panel_id) {
            Some(p) => p,
            None => return,
        };

        // Already browsing bookmarks — treat the hotkey as a toggle and exit.
        if panel.source_id == source_id {
            drop(state_guard);
            if let Some((_, parent_path, cursor_hint, cursor_index)) =
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
            return;
        }

        let parent_source_id = panel.source_id.clone();
        let parent_path = panel.path.clone();
        let prev_shadow = panel.shadow_source.clone();
        let bookmarks = state_guard.bookmarks.clone();

        let shadow = ShadowSource {
            parent_source_id,
            parent_path,
            archive_path: "Bookmarks".to_string(),
            cursor_hint: None,
            cursor_index: None,
            parent_shadow: prev_shadow.map(Box::new),
        };

        // Always insert/overwrite with a fresh driver snapshot.
        let driver = Arc::new(crate::vfs::bookmarks::BookmarksDriver::new(bookmarks));
        state_guard.sources.insert(
            source_id.clone(),
            Source {
                id: source_id.clone(),
                name: "Bookmarks".to_string(),
                kind: "virtual".to_string(),
                supports: vec!["ListEntries".to_string()],
                root_path: String::new(),
                mounted: true,
                device_path: None,
                shadow: true,
                icon_name: None,
                sort_priority: 100,
            },
        );
        state_guard
            .drivers
            .insert(source_id.clone(), driver as Arc<dyn VfsDriver>);

        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            panel.shadow_source = Some(shadow);
            panel.source_id = source_id.clone();
            panel.path = String::new();
            panel.cursor = 0;
            panel.selection.clear();
            panel.filter = None;
            panel.filter_mode_active = false;
        }
    }

    // Notify FE of updated sources list.
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

fn bookmarks_patch(bookmarks: &[BookmarkEntry]) -> ServerMessage {
    ServerMessage::StatePatch {
        timestamp: now(),
        domain: "bookmarks".to_string(),
        entity_id: None,
        version: 0,
        patch: serde_json::json!({ "bookmarks": bookmarks }),
    }
}

async fn broadcast_patch(
    msg: ServerMessage,
    gui_senders: &[tokio::sync::mpsc::Sender<ServerMessage>],
    tx: &tokio::sync::mpsc::Sender<ServerMessage>,
) {
    for sender in gui_senders {
        let _ = sender.send(msg.clone()).await;
    }
    let _ = tx.send(msg).await;
}

fn default_name(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}
