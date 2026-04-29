use serde_json::Value;

use crate::bookmarks::BookmarkEntry;
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::IntentContext;

use super::arg_str;

pub async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "AddBookmark" => add_bookmark(args, ctx).await,
        "RemoveBookmark" => remove_bookmark(args, ctx).await,
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
    // Also send to the current client if it's not already in gui_senders
    // (viewer clients are not in gui_senders).
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
