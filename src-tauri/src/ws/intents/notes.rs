//! Note intents. They talk to the active `NoteStore` only through its trait and
//! speak `storeId` / `noteId` on the wire; the editor layer opens the local file
//! the store returns. See `crate::notes`.

use serde_json::Value;

use crate::editor::CreatedNote;
use crate::notes::{NewNoteRequest, NoteError};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::IntentContext;

use super::arg_str;

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "CreateNote" => create_note(args, ctx).await,
        "OpenNotes" => open_notes(args, ctx).await,
        _ => {}
    }
}

async fn create_note(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = arg_str(args, "requestId").unwrap_or("").to_string();
    let panel_id = arg_str(args, "panelId").unwrap_or("").to_string();
    let request = NewNoteRequest {
        title: arg_str(args, "title").map(str::to_string).filter(|t| !t.trim().is_empty()),
    };
    let fail = |code: &str, message: String| ServerMessage::NoteCreateFailed {
        timestamp: now(),
        request_id: request_id.clone(),
        code: code.into(),
        message,
    };

    let store = ctx.state.lock().await.note_store.clone();
    let creating = store.clone();
    let created = tokio::task::spawn_blocking(move || creating.create_note(&request))
        .await
        .unwrap_or_else(|error| Err(NoteError::Io(format!("Note task failed: {error}"))));
    let note = match created {
        Ok(note) => note,
        Err(error) => {
            let _ = ctx.send_feedback(fail(error.code(), error.message().to_string())).await;
            return;
        }
    };
    let local_path = crate::vfs::local::normalize_path(note.local_path.clone());
    let marker = CreatedNote { store_id: store.id().to_string(), note_id: note.note_id.clone() };

    match super::editor::open_editor_tab(ctx, &panel_id, &local_path, Some(marker)).await {
        Ok(editor_panel_id) => {
            crate::journal::record(
                "mutation.entry.v1",
                None,
                None,
                None,
                Some("completed"),
                serde_json::json!({
                    "kind": "note-create",
                    "storeId": store.id(),
                    "noteId": note.note_id,
                    "destination": local_path,
                    "undoEligible": false,
                }),
            );
            let _ = ctx
                .tx
                .send(ServerMessage::NoteCreated {
                    timestamp: now(),
                    request_id,
                    panel_id,
                    store_id: store.id().to_string(),
                    note_id: note.note_id,
                    editor_panel_id,
                })
                .await;
        }
        Err(error) => {
            // Don't leave an empty note behind when it cannot be edited.
            let note_id = note.note_id.clone();
            let discarding = store.clone();
            let path = note.local_path.clone();
            let _ = tokio::task::spawn_blocking(move || discarding.discard_empty_note(&note_id, &path)).await;
            let _ = ctx.send_feedback(fail(error.code(), error.message())).await;
        }
    }
}

async fn open_notes(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = arg_str(args, "requestId").unwrap_or("").to_string();
    let panel_id = arg_str(args, "panelId").unwrap_or("").to_string();
    let store = ctx.state.lock().await.note_store.clone();
    let location = tokio::task::spawn_blocking(move || store.browse_location())
        .await
        .unwrap_or_else(|error| Err(NoteError::Io(format!("Note task failed: {error}"))));
    let folder = match location {
        Ok(folder) => crate::vfs::local::normalize_path(folder),
        Err(error) => {
            let _ = ctx
                .send_feedback(ServerMessage::OpenNotesFailed {
                    timestamp: now(),
                    request_id,
                    code: error.code().into(),
                    message: error.message().to_string(),
                })
                .await;
            return;
        }
    };

    // Notes open in a listing: the requesting tab when it is one, else the
    // listing an editor or terminal tab belongs to, else a new listing tab.
    let (slot_id, target, source_id) = {
        let state = ctx.state.lock().await;
        let Some(panel) = state.panels.get(&panel_id) else { return };
        (
            panel.slot_id.clone(),
            super::editor::resolve_origin(&state, &panel_id),
            super::navigation::source_for_local_path(&state, &folder),
        )
    };
    let Some(target) = target else {
        if let Some(source_id) = source_id {
            super::tabs::open_listing_tab_after(ctx, &slot_id, &panel_id, &source_id, &folder, None).await;
        }
        return;
    };
    if target != panel_id {
        super::tabs::activate_tab(ctx, &slot_id, &target).await;
    }
    super::navigation::teardown_shadow_source(&target, ctx).await;
    super::navigation::navigate_panel_to(&serde_json::json!({ "panelId": target, "path": folder }), ctx).await;
}

#[cfg(test)]
mod tests {
    /// Swapping the store (e.g. for an Obsidian vault) must not require
    /// touching the note intents: they may only use the `NoteStore` trait.
    #[test]
    fn note_intents_know_nothing_store_specific() {
        let source = include_str!("notes.rs");
        let body = source.split("#[cfg(test)]").next().unwrap();
        for word in ["scratch", "config_dir", "FolderNoteStore", ".md"] {
            assert!(!body.contains(word), "notes.rs mentions {word:?}");
        }
    }
}
