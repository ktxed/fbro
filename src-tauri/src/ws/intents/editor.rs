//! Built-in editor tab intents (file level). See `crate::editor` for the disk
//! side and `spex/tabs.md` for the tab lifecycle. Nothing here knows about
//! notes beyond the `created_note` marker the notes layer passes in.

use std::path::Path;
use std::sync::Arc;

use serde_json::Value;

use crate::editor::{self as core, CreatedNote, DiskChange, EditorError, EditorTab};
use std::collections::HashMap;

use crate::state::{AppState, Panel, PanelSlot, SharedState, TabKind};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::IntentContext;

use super::{arg_bool, arg_str, arg_u64};

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "OpenEditorTab" => open_from_listing(args, ctx).await,
        "LoadEditorSession" => load_session(args, ctx, false).await,
        "ReloadEditorSession" => load_session(args, ctx, true).await,
        "SaveEditorFile" => save(args, ctx).await,
        "KeepBothEditorVersions" => keep_both(args, ctx).await,
        "ReturnToOriginTab" => return_to_origin(args, ctx).await,
        _ => {}
    }
}

fn request_id(args: &Value) -> String {
    arg_str(args, "requestId").unwrap_or("").to_string()
}

async fn broadcast(ctx: &IntentContext<'_>, message: ServerMessage) {
    let gui_senders = ctx.state.lock().await.gui_senders.clone();
    let _ = ctx.tx.send(message.clone()).await;
    for sender in gui_senders.iter().filter(|sender| !sender.same_channel(ctx.tx)) {
        let _ = sender.send(message.clone()).await;
    }
}

/// The editor tab's `editor` field, as a version-0 panel patch (always applied).
fn editor_patch(panel_id: &str, editor: &EditorTab) -> ServerMessage {
    ServerMessage::StatePatch {
        timestamp: now(),
        domain: "panels".to_string(),
        entity_id: Some(panel_id.to_string()),
        version: 0,
        patch: serde_json::json!({ "editor": editor.to_json() }),
    }
}

fn parent_of(path: &str) -> Option<String> {
    Path::new(path)
        .parent()
        .map(|parent| crate::vfs::local::normalize_path(parent.to_path_buf()))
        .filter(|parent| !parent.is_empty())
}

/// The listing tab an editor opened from `requesting_panel_id` returns to:
/// the requesting tab when it is a listing, else that editor's own origin,
/// else the nearest listing tab in the same slot (left first).
pub(super) fn resolve_origin(state: &AppState, requesting_panel_id: &str) -> Option<String> {
    resolve_origin_in(&state.panels, &state.slots, requesting_panel_id)
}

fn resolve_origin_in(
    panels: &HashMap<String, Panel>,
    slots: &HashMap<String, PanelSlot>,
    requesting_panel_id: &str,
) -> Option<String> {
    let panel = panels.get(requesting_panel_id)?;
    let slot = slots.get(&panel.slot_id)?;
    let is_listing_in_slot = |id: &str| {
        slot.tab_panel_ids.iter().any(|tab| tab == id)
            && panels.get(id).is_some_and(|p| p.tab_kind == TabKind::Listing)
    };
    if panel.tab_kind == TabKind::Listing {
        return Some(requesting_panel_id.to_string());
    }
    if let Some(origin) = panel.editor.as_ref().and_then(|e| e.origin_panel_id.as_deref()) {
        if is_listing_in_slot(origin) {
            return Some(origin.to_string());
        }
    }
    let index = slot.tab_panel_ids.iter().position(|id| id == requesting_panel_id)?;
    slot.tab_panel_ids[..index]
        .iter()
        .rev()
        .chain(slot.tab_panel_ids[index + 1..].iter())
        .find(|id| is_listing_in_slot(id))
        .cloned()
}

fn find_editor_tab(state: &AppState, file_path: &str) -> Option<(String, String)> {
    state.panels.values().find_map(|panel| {
        let editor = panel.editor.as_ref()?;
        (editor.file_path == file_path).then(|| (panel.slot_id.clone(), panel.panel_id.clone()))
    })
}

/// Open (or activate) an editor tab for `local_path`, placed right after
/// `requesting_panel_id`. Checks the file itself, never a listing.
pub(super) async fn open_editor_tab(
    ctx: &mut IntentContext<'_>,
    requesting_panel_id: &str,
    local_path: &str,
    created_note: Option<CreatedNote>,
) -> Result<String, EditorError> {
    let path = local_path.to_string();
    let file_path = tokio::task::spawn_blocking(move || {
        let canonical = core::canonical_path(&path)?;
        // Read once so encoding and size refusals happen at open time.
        core::load(&canonical)?;
        Ok::<_, EditorError>(canonical)
    })
    .await
    .map_err(|error| EditorError::Io(format!("Editor open task failed: {error}")))??;

    let existing = {
        let state = ctx.state.lock().await;
        find_editor_tab(&state, &file_path)
    };
    if let Some((slot_id, panel_id)) = existing {
        super::tabs::activate_tab(ctx, &slot_id, &panel_id).await;
        return Ok(panel_id);
    }

    let folder = parent_of(&file_path).ok_or(EditorError::NotRegularFile)?;
    let panel = {
        let mut state = ctx.state.lock().await;
        let source_id = super::navigation::source_for_local_path(&state, &folder).ok_or(EditorError::NotLocal)?;
        let requesting = state.panels.get(requesting_panel_id).ok_or_else(|| {
            EditorError::Io("The panel that opened the editor is gone.".into())
        })?;
        let slot_id = requesting.slot_id.clone();
        let origin = resolve_origin(&state, requesting_panel_id);
        let template = origin.as_deref().and_then(|id| state.panels.get(id)).cloned();
        let ids = state.panels.keys().cloned().collect();
        let panel_id = super::tabs::alloc_panel_id(&slot_id, &ids);
        let session_id = state.next_editor_session_id;
        state.next_editor_session_id += 1;
        let mut panel = super::tabs::blank_panel(
            &panel_id,
            &slot_id,
            TabKind::Editor,
            &source_id,
            &folder,
            template.as_ref(),
            state.settings.general.show_hidden_default,
        );
        panel.editor = Some(EditorTab::new(session_id, file_path, origin, created_note));
        panel
    };
    let panel_id = panel.panel_id.clone();
    if !super::tabs::insert_tab_after(ctx, panel, requesting_panel_id).await {
        return Err(EditorError::Io("The panel slot is gone.".into()));
    }
    // The hidden listing of the file's folder registers its folder watcher.
    super::tabs::spawn_panel_scan(ctx, &panel_id, None).await;
    Ok(panel_id)
}

async fn open_from_listing(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = request_id(args);
    let panel_id = arg_str(args, "panelId").unwrap_or("").to_string();
    let target = {
        let state = ctx.state.lock().await;
        listing_target(&state, &panel_id, args)
    };
    let result = match target {
        Ok(path) => open_editor_tab(ctx, &panel_id, &path, None).await.map(|_| ()),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        let _ = ctx
            .send_feedback(ServerMessage::EditorOpenFailed {
                timestamp: now(),
                request_id,
                code: error.code().into(),
                message: error.message(),
            })
            .await;
    }
}

/// Resolve `{ path, filename }` against the listing panel that sent it.
fn listing_target(state: &AppState, panel_id: &str, args: &Value) -> Result<String, EditorError> {
    let panel = state.panels.get(panel_id).ok_or_else(|| EditorError::Io("The panel is gone.".into()))?;
    if panel.tab_kind != TabKind::Listing || panel.shadow_source.is_some() {
        return Err(EditorError::NotLocal);
    }
    let path = arg_str(args, "path").unwrap_or("");
    let filename = arg_str(args, "filename").unwrap_or("");
    if path.is_empty() || filename.is_empty() || Path::new(path) != Path::new(&panel.path) {
        return Err(EditorError::Io("The panel location has changed.".into()));
    }
    let entry = panel
        .entries
        .iter()
        .find(|entry| entry.name == filename)
        .ok_or_else(|| EditorError::Io("The file is no longer in the listing.".into()))?;
    let is_file = entry.kind == "file"
        || (entry.kind == "symlink" && entry.target_kind.as_deref() == Some("file"));
    if !is_file || entry.name == ".." {
        return Err(EditorError::NotRegularFile);
    }
    let driver = state.drivers.get(&panel.source_id).ok_or(EditorError::NotLocal)?;
    if !driver.is_local() {
        return Err(EditorError::NotLocal);
    }
    let child = driver.resolve_child(path, filename);
    driver
        .local_path(&child)
        .map(crate::vfs::local::normalize_path)
        .ok_or(EditorError::NotLocal)
}

struct EditorHandle {
    file_path: String,
    session_id: u64,
    source_id: String,
    io_lock: Arc<tokio::sync::Mutex<()>>,
}

async fn editor_handle(state: &SharedState, panel_id: &str) -> Option<EditorHandle> {
    let state = state.lock().await;
    let panel = state.panels.get(panel_id)?;
    let editor = panel.editor.as_ref()?;
    Some(EditorHandle {
        file_path: editor.file_path.clone(),
        session_id: editor.session_id,
        source_id: panel.source_id.clone(),
        io_lock: Arc::clone(&editor.io_lock),
    })
}

/// Apply `update` to the panel's editor and return a patch for it.
async fn update_editor(
    state: &SharedState,
    panel_id: &str,
    update: impl FnOnce(&mut EditorTab),
) -> Option<ServerMessage> {
    let mut state = state.lock().await;
    let editor = state.panels.get_mut(panel_id)?.editor.as_mut()?;
    update(editor);
    Some(editor_patch(panel_id, editor))
}

async fn load_session(args: &Value, ctx: &mut IntentContext<'_>, reload: bool) {
    let request_id = request_id(args);
    let panel_id = arg_str(args, "panelId").unwrap_or("").to_string();
    let Some(handle) = editor_handle(ctx.state, &panel_id).await else { return };
    let _guard = handle.io_lock.lock().await;
    let path = handle.file_path.clone();
    let loaded = tokio::task::spawn_blocking(move || core::load(&path))
        .await
        .unwrap_or_else(|error| Err(EditorError::Io(format!("Editor load task failed: {error}"))));
    match loaded {
        Ok(loaded) => {
            let disk = loaded.disk.clone();
            let patch = update_editor(ctx.state, &panel_id, |editor| {
                editor.disk = Some(disk);
                if reload {
                    editor.saved_version += 1;
                }
            })
            .await;
            let _ = ctx
                .tx
                .send(ServerMessage::EditorSessionReady {
                    timestamp: now(),
                    request_id,
                    panel_id: panel_id.clone(),
                    session_id: handle.session_id,
                    text: loaded.text,
                    eol: loaded.disk.eol.as_str().into(),
                    bom: loaded.disk.bom,
                    conflict_copy_path: None,
                })
                .await;
            if let Some(patch) = patch {
                broadcast(ctx, patch).await;
            }
        }
        Err(error) => {
            let _ = ctx
                .send_feedback(ServerMessage::EditorSessionFailed {
                    timestamp: now(),
                    request_id,
                    panel_id,
                    code: error.code().into(),
                    message: error.message(),
                })
                .await;
        }
    }
}

async fn save(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = request_id(args);
    let panel_id = arg_str(args, "panelId").unwrap_or("").to_string();
    let revision = arg_u64(args, "revision", 0);
    let force = arg_bool(args, "force", false);
    let text = arg_str(args, "text").unwrap_or("").to_string();
    let fail = |code: &str, message: String| ServerMessage::SaveEditorFailed {
        timestamp: now(),
        request_id: request_id.clone(),
        panel_id: panel_id.clone(),
        revision,
        code: code.into(),
        message,
    };
    if args.get("text").and_then(Value::as_str).is_none() {
        let _ = ctx.tx.send(fail("IO", "Save request carried no text.".into())).await;
        return;
    }
    if text.len() as u64 > core::MAX_EDITOR_BYTES {
        let _ = ctx.tx.send(fail("TOO_LARGE", EditorError::TooLarge.message())).await;
        return;
    }
    let Some(handle) = editor_handle(ctx.state, &panel_id).await else { return };
    let _guard = handle.io_lock.lock().await;
    // Read the disk state after taking the lock: a previous save may have moved it.
    let (disk, driver) = {
        let state = ctx.state.lock().await;
        let disk = state
            .panels
            .get(&panel_id)
            .and_then(|panel| panel.editor.as_ref())
            .and_then(|editor| editor.disk.clone());
        (disk, state.drivers.get(&handle.source_id).cloned())
    };
    let Some(disk) = disk else {
        let _ = ctx.tx.send(fail("IO", "The file has not been loaded yet.".into())).await;
        return;
    };
    let Some(driver) = driver.filter(|driver| driver.is_local()) else {
        let _ = ctx.tx.send(fail("NOT_LOCAL", EditorError::NotLocal.message())).await;
        return;
    };
    let path = handle.file_path.clone();
    let result = tokio::task::spawn_blocking(move || core::save(driver.as_ref(), &path, &text, &disk, force))
        .await
        .unwrap_or_else(|error| Err(EditorError::Io(format!("Editor save task failed: {error}"))));
    match result {
        Ok(disk) => {
            let mut first_save = false;
            let patch = update_editor(ctx.state, &panel_id, |editor| {
                editor.disk = Some(disk);
                editor.saved_version += 1;
                first_save = !editor.ever_saved;
                editor.ever_saved = true;
            })
            .await;
            if first_save {
                // One journal entry per editing session, not one per autosave.
                crate::journal::record(
                    "mutation.entry.v1",
                    None,
                    None,
                    Some(&handle.source_id),
                    Some("completed"),
                    serde_json::json!({ "kind": "edit", "destination": handle.file_path, "undoEligible": false }),
                );
            }
            let _ = ctx
                .tx
                .send(ServerMessage::SaveEditorOk {
                    timestamp: now(),
                    request_id: request_id.clone(),
                    panel_id: panel_id.clone(),
                    revision,
                })
                .await;
            if let Some(patch) = patch {
                broadcast(ctx, patch).await;
            }
        }
        Err(error) => {
            let _ = ctx.tx.send(fail(error.code(), error.message())).await;
        }
    }
}

/// Keep both: save the local text as a sibling conflict copy, then reload.
async fn keep_both(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = request_id(args);
    let panel_id = arg_str(args, "panelId").unwrap_or("").to_string();
    let text = arg_str(args, "text").unwrap_or("").to_string();
    let Some(handle) = editor_handle(ctx.state, &panel_id).await else { return };
    let _guard = handle.io_lock.lock().await;
    let (disk, driver) = {
        let state = ctx.state.lock().await;
        let disk = state
            .panels
            .get(&panel_id)
            .and_then(|panel| panel.editor.as_ref())
            .and_then(|editor| editor.disk.clone());
        (disk, state.drivers.get(&handle.source_id).cloned())
    };
    let path = handle.file_path.clone();
    let result = tokio::task::spawn_blocking(move || {
        let disk = disk.ok_or_else(|| EditorError::Io("The file has not been loaded yet.".into()))?;
        let driver = driver.filter(|d| d.is_local()).ok_or(EditorError::NotLocal)?;
        let copy = core::write_conflict_copy(driver.as_ref(), &path, &text, &disk, &core::file_stamp())?;
        Ok::<_, EditorError>((copy, core::load(&path)))
    })
    .await
    .unwrap_or_else(|error| Err(EditorError::Io(format!("Editor task failed: {error}"))));
    match result {
        Ok((copy, Ok(loaded))) => {
            let disk = loaded.disk.clone();
            let patch = update_editor(ctx.state, &panel_id, |editor| {
                editor.disk = Some(disk);
                editor.saved_version += 1;
            })
            .await;
            crate::journal::record(
                "mutation.entry.v1",
                None,
                None,
                Some(&handle.source_id),
                Some("completed"),
                serde_json::json!({ "kind": "edit-conflict-copy", "destination": copy, "undoEligible": false }),
            );
            let _ = ctx
                .send_feedback(ServerMessage::EditorSessionReady {
                    timestamp: now(),
                    request_id,
                    panel_id: panel_id.clone(),
                    session_id: handle.session_id,
                    text: loaded.text,
                    eol: loaded.disk.eol.as_str().into(),
                    bom: loaded.disk.bom,
                    conflict_copy_path: Some(copy),
                })
                .await;
            if let Some(patch) = patch {
                broadcast(ctx, patch).await;
            }
        }
        Ok((_, Err(error))) | Err(error) => {
            let _ = ctx
                .tx
                .send(ServerMessage::SaveEditorFailed {
                    timestamp: now(),
                    request_id,
                    panel_id,
                    revision: 0,
                    code: error.code().into(),
                    message: error.message(),
                })
                .await;
        }
    }
}

async fn return_to_origin(args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = arg_str(args, "panelId").unwrap_or("").to_string();
    let target = {
        let state = ctx.state.lock().await;
        let Some(panel) = state.panels.get(&panel_id) else { return };
        let Some(editor) = panel.editor.as_ref() else { return };
        let origin = resolve_origin(&state, &panel_id);
        (
            panel.slot_id.clone(),
            origin,
            editor.file_path.clone(),
            panel.source_id.clone(),
            panel.path.clone(),
        )
    };
    let (slot_id, origin, file_path, source_id, folder) = target;
    let file_name = Path::new(&file_path).file_name().map(|n| n.to_string_lossy().to_string());
    match origin {
        Some(origin_id) => {
            super::tabs::activate_tab(ctx, &slot_id, &origin_id).await;
            move_cursor_to_file(ctx, &origin_id, &file_path).await;
        }
        None => {
            super::tabs::open_listing_tab_after(ctx, &slot_id, &panel_id, &source_id, &folder, file_name).await;
        }
    }
}

/// Put the origin's cursor on the edited file when its listing shows it;
/// otherwise leave the cursor alone.
async fn move_cursor_to_file(ctx: &mut IntentContext<'_>, panel_id: &str, file_path: &str) {
    let patch = {
        let mut state = ctx.state.lock().await;
        let Some(panel) = state.panels.get_mut(panel_id) else { return };
        let file = Path::new(file_path);
        if file.parent() != Some(Path::new(&panel.path)) {
            return;
        }
        let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let Some(index) = panel.entries.iter().position(|entry| entry.name == name) else { return };
        if panel.cursor == index {
            return;
        }
        panel.cursor = index;
        panel.version += 1;
        ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id.to_string()),
            version: panel.version,
            patch: serde_json::json!({ "cursor": index }),
        }
    };
    let _ = ctx.tx.send(patch).await;
}

/// Closing the slot's only tab: the editor tab becomes a listing of its folder.
pub(super) async fn convert_to_listing(ctx: &mut IntentContext<'_>, panel_id: &str) {
    let (editor, folder) = {
        let mut state = ctx.state.lock().await;
        let Some(panel) = state.panels.get_mut(panel_id) else { return };
        let Some(editor) = panel.editor.take() else { return };
        panel.tab_kind = TabKind::Listing;
        (editor, panel.path.clone())
    };
    let _ = ctx
        .tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id.to_string()),
            version: 0,
            patch: serde_json::json!({ "tabKind": "listing", "editor": null }),
        })
        .await;
    let file_name = Path::new(&editor.file_path).file_name().map(|n| n.to_string_lossy().to_string());
    crate::ws::runtime::navigate_to(
        panel_id,
        &folder,
        file_name,
        None,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        ctx.dir_size_queue,
        ctx.watcher_cmd_tx,
    )
    .await;
    discard_if_untouched_note(ctx, &editor).await;
}

/// A note created for this tab and never written to is removed when its tab
/// closes. Escape and app quit never get here.
pub(super) async fn discard_if_untouched_note(ctx: &mut IntentContext<'_>, editor: &EditorTab) {
    let Some(note) = editor.created_note.clone() else { return };
    if editor.ever_saved {
        return;
    }
    let store = ctx.state.lock().await.note_store.clone();
    if store.id() != note.store_id {
        return;
    }
    let path = editor.file_path.clone();
    let disk = editor.disk.clone();
    let note_id = note.note_id.clone();
    let discarded = tokio::task::spawn_blocking(move || {
        core::is_untouched_empty_note(&path, disk.as_ref())
            && store.discard_empty_note(&note_id, std::path::Path::new(&path)).is_ok()
    })
    .await
    .unwrap_or(false);
    if discarded {
        crate::journal::record(
            "mutation.entry.v1",
            None,
            None,
            None,
            Some("completed"),
            serde_json::json!({
                "kind": "note-discard",
                "storeId": note.store_id,
                "noteId": note.note_id,
                "destination": editor.file_path,
                "undoEligible": false,
            }),
        );
    }
}

/// Called when the folder watcher reports a change in an editor tab's folder.
/// Waits for any save in flight, so the editor's own writes are not reported.
pub(crate) async fn check_external_change(
    state: SharedState,
    tx: tokio::sync::mpsc::Sender<ServerMessage>,
    panel_id: String,
) {
    let Some(handle) = editor_handle(&state, &panel_id).await else { return };
    let _guard = handle.io_lock.lock().await;
    let disk = {
        let state = state.lock().await;
        state
            .panels
            .get(&panel_id)
            .and_then(|panel| panel.editor.as_ref())
            .and_then(|editor| editor.disk.clone())
    };
    let Some(disk) = disk else { return };
    let path = handle.file_path.clone();
    let change = tokio::task::spawn_blocking(move || core::disk_change(&path, &disk))
        .await
        .ok()
        .and_then(Result::ok);
    let state_name = match change {
        Some(DiskChange::Modified) => "modified",
        Some(DiskChange::Missing) => "missing",
        _ => return,
    };
    let _ = tx
        .send(ServerMessage::EditorFileChanged {
            timestamp: now(),
            panel_id,
            session_id: handle.session_id,
            state: state_name.into(),
        })
        .await;
}

/// Whether quitting should first ask the frontend to save editor tabs.
pub(crate) fn needs_flush_before_quit(state: &mut AppState) -> bool {
    let has_editors = state.panels.values().any(|panel| panel.editor.is_some());
    let recently_asked = state
        .editor_flush_requested_at
        .is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(3));
    if !has_editors || recently_asked {
        state.editor_flush_requested_at = None;
        return false;
    }
    state.editor_flush_requested_at = Some(std::time::Instant::now());
    true
}

#[cfg(test)]
mod tests {
    use super::resolve_origin_in;
    use crate::editor::EditorTab;
    use crate::state::{Panel, PanelSlot, TabKind};
    use std::collections::HashMap;

    fn layout(tabs: &[(&str, TabKind)], origin: Option<&str>) -> (HashMap<String, Panel>, HashMap<String, PanelSlot>) {
        let mut panels = HashMap::new();
        for (id, kind) in tabs {
            let mut panel = crate::ws::intents::tabs::blank_panel(id, "left", *kind, "home", "/tmp", None, false);
            if *kind == TabKind::Editor {
                panel.editor = Some(EditorTab::new(1, "/tmp/a.md".into(), origin.map(str::to_string), None));
            }
            panels.insert(id.to_string(), panel);
        }
        let slot = PanelSlot {
            slot_id: "left".into(),
            tab_panel_ids: tabs.iter().map(|(id, _)| id.to_string()).collect(),
            active_panel_id: tabs[0].0.into(),
            version: 0,
        };
        (panels, HashMap::from([("left".to_string(), slot)]))
    }

    #[test]
    fn origin_is_the_requesting_listing() {
        let (panels, slots) = layout(&[("a", TabKind::Listing), ("b", TabKind::Listing)], None);
        assert_eq!(resolve_origin_in(&panels, &slots, "b").as_deref(), Some("b"));
    }

    #[test]
    fn origin_from_an_editor_prefers_its_own_origin_then_the_nearest_listing() {
        let (panels, slots) = layout(
            &[("a", TabKind::Listing), ("b", TabKind::Listing), ("e", TabKind::Editor)],
            Some("a"),
        );
        assert_eq!(resolve_origin_in(&panels, &slots, "e").as_deref(), Some("a"));
        let (panels, slots) = layout(
            &[("a", TabKind::Listing), ("t", TabKind::Terminal), ("e", TabKind::Editor)],
            Some("gone"),
        );
        assert_eq!(resolve_origin_in(&panels, &slots, "e").as_deref(), Some("a"));
        assert_eq!(resolve_origin_in(&panels, &slots, "t").as_deref(), Some("a"));
        let (panels, slots) = layout(&[("t", TabKind::Terminal), ("a", TabKind::Listing)], None);
        assert_eq!(resolve_origin_in(&panels, &slots, "t").as_deref(), Some("a"));
    }

    #[test]
    fn no_listing_tab_means_no_origin() {
        let (panels, slots) = layout(&[("e", TabKind::Editor)], None);
        assert_eq!(resolve_origin_in(&panels, &slots, "e"), None);
    }
}
