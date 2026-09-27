use std::path::Path;

use serde_json::Value;

use crate::state::TabKind;
use crate::terminal::{file_fingerprint, fingerprint_changed, QuickEditCompletion};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::IntentContext;

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(panel_id) = scope.as_str() else {
        eprintln!("[fbro] terminal intent {name} missing panel_id scope");
        return;
    };

    match name {
        "OpenPanelTerminal" => open(panel_id, args, ctx).await,
        "QuickEditFile" => quick_edit(panel_id, ctx).await,
        "ResizeTerminal" => resize(panel_id, args, ctx).await,
        "ClosePanelTerminal" => close(panel_id, ctx).await,
        _ => {}
    }
}

async fn quick_edit(panel_id: &str, ctx: &mut IntentContext<'_>) {
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
        let local_index = panel.cursor.saturating_sub(panel.window_start);
        let Some(entry) = panel.entries.get(local_index) else {
            let source_id = panel.source_id.clone();
            let requested_editor = state_guard.settings.general.quick_terminal_editor.program();
            drop(state_guard);
            let message = "Quick edit requires a file under the cursor.";
            record_quick_edit_failure(panel_id, Some(&source_id), None, Some(requested_editor), &[], message, None);
            send_quick_edit_error(ctx, message).await;
            return;
        };
        if entry.kind != "file" || entry.name == ".." {
            let source_id = panel.source_id.clone();
            let requested_editor = state_guard.settings.general.quick_terminal_editor.program();
            drop(state_guard);
            let message = "Quick edit is available only for regular files.";
            record_quick_edit_failure(panel_id, Some(&source_id), None, Some(requested_editor), &[], message, None);
            send_quick_edit_error(ctx, message).await;
            return;
        }
        let file_path = Path::new(&panel.path).join(&entry.name).to_string_lossy().to_string();
        (
            panel.path.clone(),
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
