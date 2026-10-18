use std::path::Path;

use serde_json::Value;

use crate::state::{Panel, TabKind};
use crate::terminal::{file_fingerprint, fingerprint_changed, QuickEditCompletion};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::IntentContext;

use super::arg_str;

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(panel_id) = scope.as_str() else {
        eprintln!("[fbro] terminal intent {name} missing panel_id scope");
        return;
    };

    match name {
        "OpenPanelTerminal" => open(panel_id, args, ctx).await,
        "QuickEditFile" => quick_edit(panel_id, args, ctx).await,
        "ResizeTerminal" => resize(panel_id, args, ctx).await,
        "ClosePanelTerminal" => close(panel_id, ctx).await,
        _ => {}
    }
}

fn quick_edit_target(panel: &Panel, args: &Value) -> Result<(String, String), String> {
    let path = arg_str(args, "path")
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "Quick edit requires a directory path.".to_string())?;
    let filename = arg_str(args, "filename")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "Quick edit requires a filename.".to_string())?;
    if Path::new(path) != Path::new(&panel.path) {
        return Err("Quick edit panel location has changed.".into());
    }
    // The request identifies a file, independently of cursor or render-window changes.
    let entry = panel.entries.iter()
        .find(|entry| entry.name == filename)
        .ok_or_else(|| "Quick edit target is no longer in the listing.".to_string())?;
    if entry.kind != "file" || entry.name == ".." {
        return Err("Quick edit is available only for regular files.".into());
    }
    Ok((
        path.to_string(),
        Path::new(path).join(filename).to_string_lossy().to_string(),
    ))
}

async fn quick_edit(panel_id: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    let (cwd, file_path, source_id, version, gui_senders, pty_manager, editor) = {
        let state_guard = ctx.state.lock().await;
        let Some(panel) = state_guard.panels.get(panel_id) else {
            record_quick_edit_failure(panel_id, None, None, None, &[], "Quick Edit panel is unavailable.", None);
            return;
        };
        if panel.tab_kind != TabKind::Listing || panel.shadow_source.is_some() || panel.terminal_session.is_some() {
            record_quick_edit_failure(panel_id, Some(&panel.source_id), None, Some(state_guard.settings.general.quick_terminal_editor.program()), &[], "Quick Edit is unavailable for this panel state.", None);
            return;
        }
        let Some(driver) = state_guard.drivers.get(&panel.source_id) else {
            let source_id = panel.source_id.clone();
            let requested_editor = state_guard.settings.general.quick_terminal_editor.program();
            drop(state_guard);
            let message = "Quick edit requires a local filesystem panel.";
            record_quick_edit_failure(panel_id, Some(&source_id), None, Some(requested_editor), &[], message, None);
            send_quick_edit_error(ctx, message).await;
            return;
        };
        if !driver.is_local() {
            let source_id = panel.source_id.clone();
            let requested_editor = state_guard.settings.general.quick_terminal_editor.program();
            drop(state_guard);
            let message = "Quick edit requires a local filesystem panel.";
            record_quick_edit_failure(panel_id, Some(&source_id), None, Some(requested_editor), &[], message, None);
            send_quick_edit_error(ctx, message).await;
            return;
        }
        let (cwd, file_path) = match quick_edit_target(panel, args) {
            Ok(target) => target,
            Err(message) => {
                let source_id = panel.source_id.clone();
                let requested_editor = state_guard.settings.general.quick_terminal_editor.program();
                drop(state_guard);
                record_quick_edit_failure(panel_id, Some(&source_id), None, Some(requested_editor), &[], &message, None);
                send_quick_edit_error(ctx, &message).await;
                return;
            }
        };
        (
            cwd,
            file_path,
            panel.source_id.clone(),
            panel.version,
            state_guard.gui_senders.clone(),
            state_guard.pty_manager.clone(),
            state_guard.settings.general.quick_terminal_editor,
        )
    };

    let editor_names = crate::terminal::search_names(editor);
    let operation = crate::journal::new_operation("QuickEditFile", Some(&source_id)).expect("QuickEditFile is a journal operation");
    crate::journal::record_quick_edit_intent(
        &operation,
        panel_id,
        Some(&file_path),
        Some(editor.program()),
        &editor_names,
    );
    let before = file_fingerprint(&file_path);
    let mut failures = Vec::new();
    let mut session_id = None;
    for name in &editor_names {
        let Some(program) = crate::terminal::locate_editor(name) else {
            continue;
        };
        match pty_manager.open_command(
            panel_id.to_string(),
            &cwd,
            80,
            24,
            &program,
            &[file_path.clone()],
            ctx.tx.clone(),
            ctx.state.clone(),
            pty_manager.clone(),
            Some(QuickEditCompletion::new(operation.clone(), file_path.clone(), name.to_string(), before.clone())),
        ) {
            Ok(id) => {
                session_id = Some(id);
                break;
            }
            Err(error) => failures.push(format!("{name}: {error}")),
        }
    }
    let Some(session_id) = session_id else {
        let message = if failures.is_empty() {
            "Could not find nano, vim, micro, or edit on PATH (also checks Git for Windows, Scoop, Chocolatey, and WinGet).".to_string()
        } else {
            format!("Could not start a terminal editor. {}", failures.join("; "))
        };
        let changed = fingerprint_changed(&before, &file_fingerprint(&file_path));
        crate::journal::record_quick_edit_result(&operation, Some(&file_path), None, None, "failed", Some(&message), changed, None, None);
        send_quick_edit_error(ctx, &message).await;
        return;
    };

    {
        let mut state_guard = ctx.state.lock().await;
        let Some(panel) = state_guard.panels.get_mut(panel_id) else {
            pty_manager.close(session_id);
            return;
        };
        if panel.terminal_session.is_some() {
            pty_manager.close(session_id);
            return;
        }
        panel.terminal_session = Some(session_id);
        panel.version += 1;
    }
    let new_version = version + 1;
    let patch = ServerMessage::StatePatch {
        timestamp: now(), domain: "panels".to_string(), entity_id: Some(panel_id.to_string()), version: new_version,
        patch: serde_json::json!({ "terminalSessionId": session_id }),
    };
    let _ = ctx.tx.send(patch.clone()).await;
    for sender in &gui_senders { let _ = sender.send(patch.clone()).await; }
    let opened = ServerMessage::TerminalSessionOpened { timestamp: now(), panel_id: panel_id.to_string(), session_id };
    let _ = ctx.tx.send(opened.clone()).await;
    for sender in &gui_senders { let _ = sender.send(opened.clone()).await; }
}

fn record_quick_edit_failure(
    panel_id: &str,
    source_id: Option<&str>,
    path: Option<&str>,
    requested_editor: Option<&str>,
    editor_candidates: &[&str],
    message: &str,
    changed: Option<bool>,
) {
    let operation = crate::journal::new_operation("QuickEditFile", source_id).expect("QuickEditFile is a journal operation");
    crate::journal::record_quick_edit_intent(&operation, panel_id, path, requested_editor, editor_candidates);
    crate::journal::record_quick_edit_result(&operation, path, None, None, "failed", Some(message), changed, None, None);
}

async fn send_quick_edit_error(ctx: &IntentContext<'_>, message: &str) {
    let _ = ctx.tx.send(ServerMessage::Error {
        timestamp: now(), code: "QUICK_EDITOR_UNAVAILABLE".to_string(), message: message.to_string(), recoverable: true, extra: None,
    }).await;
}

async fn open(panel_id: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    let cols = args.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = args.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;

    // Pull the panel cwd. Refuse if shadow source (no real local cwd).
    let (cwd, version, gui_senders, pty_manager, already_open, zsh_emacs_keymap) = {
        let state_guard = ctx.state.lock().await;
        let Some(panel) = state_guard.panels.get(panel_id) else {
            return;
        };
        if panel.shadow_source.is_some() {
            eprintln!("[fbro] terminal: refused — panel {panel_id} is in shadow source");
            return;
        }
        (
            panel.path.clone(),
            panel.version,
            state_guard.gui_senders.clone(),
            state_guard.pty_manager.clone(),
            panel.terminal_session,
            state_guard.settings.general.zsh_emacs_keymap
                && state_guard.platform.supports_zsh_emacs_keymap(),
        )
    };

    if already_open.is_some() {
        // Already has a terminal — be idempotent.
        return;
    }

    let session_id = match pty_manager.open(
        panel_id.to_string(),
        &cwd,
        cols,
        rows,
        zsh_emacs_keymap,
        ctx.tx.clone(),
        ctx.state.clone(),
        pty_manager.clone(),
    ) {
        Ok(sid) => sid,
        Err(e) => {
            eprintln!("[fbro] terminal open failed: {e}");
            return;
        }
    };

    // Update panel state.
    {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(panel_id) {
            panel.terminal_session = Some(session_id);
            panel.version += 1;
        }
    }

    let new_version = version + 1;
    let patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "panels".to_string(),
        entity_id: Some(panel_id.to_string()),
        version: new_version,
        patch: serde_json::json!({ "terminalSessionId": session_id }),
    };
    let _ = ctx.tx.send(patch.clone()).await;
    for sender in &gui_senders {
        let _ = sender.send(patch.clone()).await;
    }

    let opened = ServerMessage::TerminalSessionOpened {
        timestamp: now(),
        panel_id: panel_id.to_string(),
        session_id,
    };
    let _ = ctx.tx.send(opened.clone()).await;
    for sender in &gui_senders {
        let _ = sender.send(opened.clone()).await;
    }
}

async fn resize(panel_id: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    let cols = args.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = args.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;

    let (sid, pty_manager) = {
        let state_guard = ctx.state.lock().await;
        let Some(panel) = state_guard.panels.get(panel_id) else {
            return;
        };
        let Some(sid) = panel.terminal_session else {
            return;
        };
        (sid, state_guard.pty_manager.clone())
    };

    if let Err(e) = pty_manager.resize(sid, cols, rows) {
        eprintln!("[fbro] terminal resize sid={sid}: {e}");
    }
}

async fn close(panel_id: &str, ctx: &mut IntentContext<'_>) {
    let (maybe_sid, version, gui_senders, pty_manager) = {
        let state_guard = ctx.state.lock().await;
        let Some(panel) = state_guard.panels.get(panel_id) else {
            return;
        };
        (
            panel.terminal_session,
            panel.version,
            state_guard.gui_senders.clone(),
            state_guard.pty_manager.clone(),
        )
    };

    let Some(sid) = maybe_sid else { return };

    pty_manager.close(sid);

    {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(panel_id) {
            panel.terminal_session = None;
            panel.version += 1;
        }
    }

    let new_version = version + 1;
    let patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "panels".to_string(),
        entity_id: Some(panel_id.to_string()),
        version: new_version,
        patch: serde_json::json!({ "terminalSessionId": null }),
    };
    let _ = ctx.tx.send(patch.clone()).await;
    for sender in &gui_senders {
        let _ = sender.send(patch.clone()).await;
    }

    let closed = ServerMessage::TerminalClosed {
        timestamp: now(),
        session_id: sid,
        panel_id: panel_id.to_string(),
    };
    let _ = ctx.tx.send(closed.clone()).await;
    for sender in &gui_senders {
        let _ = sender.send(closed.clone()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::quick_edit_target;
    use crate::state::{Panel, PanelStatus, PanelViewMode, TabKind};
    use crate::ws::protocol::{EntrySchema, SortSpec};
    use serde_json::json;
    use std::path::Path;

    fn entry(name: &str, kind: &str) -> EntrySchema {
        EntrySchema {
            name: name.into(), kind: kind.into(), target_kind: None,
            browsable_archive: false, size: None, mtime: None, btime: None,
            permissions: None, icon_name: None, cc: None, source_path: None,
            path_hint: None, git_status: None, git_details: None,
        }
    }

    fn panel() -> Panel {
        Panel {
            panel_id: "left".into(), slot_id: "left".into(), tab_kind: TabKind::Listing,
            tab_title_override: None, source_id: "local".into(), path: "/photos".into(),
            status: PanelStatus::Ready,
            sort: SortSpec { by: "name".into(), order: "asc".into() },
            filter: None, filter_mode_active: false, show_hidden: false,
            view_mode: PanelViewMode::List, cursor: 0, selection: vec![0],
            entries: vec![entry("a.jpg", "file"), entry("relinker.py", "file"),
                entry("folder", "directory"), entry("link.py", "symlink"), entry("..", "directory")],
            version: 1, panel_epoch: 1, calculating_size: Default::default(),
            windowed: false, window_start: 0, window_count: 0,
            thumbnail_viewport: None, window_version: 0, source_paths: Default::default(),
            navigation_context: Default::default(), shadow_source: None,
            terminal_session: None, git: None,
        }
    }

    #[test]
    fn explicit_quick_edit_target_ignores_cursor_selection_and_render_window() {
        let mut panel = panel();
        let args = json!({ "path": panel.path, "filename": "relinker.py" });
        for (cursor, windowed, window_start) in [(0, false, 0), (0, true, 1), (2, true, 100), (usize::MAX, true, 100)] {
            panel.cursor = cursor;
            panel.windowed = windowed;
            panel.window_start = window_start;
            panel.window_count = 1;
            let (cwd, target) = quick_edit_target(&panel, &args).unwrap();
            assert_eq!(cwd, panel.path);
            assert_eq!(Path::new(&target), Path::new(&panel.path).join("relinker.py"));
        }
    }

    #[test]
    fn quick_edit_requires_explicit_string_arguments_without_cursor_fallback() {
        let panel = panel();
        for args in [json!({}), json!({ "filename": "a.jpg" }),
            json!({ "path": "", "filename": "a.jpg" }),
            json!({ "path": 7, "filename": "a.jpg" }),
            json!({ "path": panel.path }), json!({ "path": panel.path, "filename": "" }),
            json!({ "path": panel.path, "filename": 7 })] {
            assert!(quick_edit_target(&panel, &args).is_err(), "accepted {args}");
        }
    }

    #[test]
    fn quick_edit_rejects_changed_locations_and_non_file_targets() {
        let panel = panel();
        assert_eq!(quick_edit_target(&panel, &json!({ "path": "/old-photos", "filename": "a.jpg" })),
            Err("Quick edit panel location has changed.".into()));
        for filename in ["missing.py", "folder", "link.py", "..", "../a.jpg", "/a.jpg"] {
            assert!(quick_edit_target(&panel, &json!({ "path": panel.path, "filename": filename })).is_err(),
                "accepted {filename}");
        }
        let mut empty = panel.clone();
        empty.entries.clear();
        assert!(quick_edit_target(&empty, &json!({ "path": panel.path, "filename": "a.jpg" })).is_err());
    }

    #[test]
    fn quick_edit_preserves_spaces_and_unicode_in_explicit_targets() {
        let mut panel = panel();
        panel.path = "/my photos/été".into();
        panel.entries.push(entry(" script é.py", "file"));
        let (cwd, target) = quick_edit_target(&panel,
            &json!({ "path": panel.path, "filename": " script é.py" })).unwrap();
        assert_eq!(cwd, panel.path);
        assert_eq!(Path::new(&target), Path::new(&panel.path).join(" script é.py"));
    }
}
