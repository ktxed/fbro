use serde_json::Value;

use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::IntentContext;

pub(crate) async fn handle(
    name: &str,
    scope: &Value,
    args: &Value,
    ctx: &mut IntentContext<'_>,
) {
    let Some(panel_id) = scope.as_str() else {
        eprintln!("[fbro] terminal intent {name} missing panel_id scope");
        return;
    };

    match name {
        "OpenPanelTerminal" => open(panel_id, args, ctx).await,
        "ResizeTerminal" => resize(panel_id, args, ctx).await,
        "ClosePanelTerminal" => close(panel_id, ctx).await,
        _ => {}
    }
}

async fn open(panel_id: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    let cols = args.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = args.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;

    // Pull the panel cwd. Refuse if shadow source (no real local cwd).
    let (cwd, version, gui_senders, pty_manager, already_open) = {
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
        )
    };

    if already_open.is_some() {
        // Already has a terminal — be idempotent.
        return;
    }

    let session_id = match pty_manager.open(panel_id.to_string(), &cwd, cols, rows, ctx.tx.clone(), ctx.state.clone(), pty_manager.clone())
    {
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
