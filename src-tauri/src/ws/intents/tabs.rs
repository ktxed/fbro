use std::collections::HashSet;
use std::sync::Arc;

use serde_json::Value;

use crate::jobs::scan::run_scan;
use crate::state::{Panel, PanelStatus, PanelViewMode, TabKind};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::IntentContext;

pub(crate) async fn handle(name: &str, _scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "panel.tab.new" => new_tab(args, ctx).await,
        "panel.tab.close" => close_tab(args, ctx).await,
        "panel.tab.switch" => switch_tab(args, ctx).await,
        "panel.terminal.promote" => promote_terminal(args, ctx).await,
        _ => {}
    }
}

/// Allocate a new panel id within a slot. Suffixed with an epoch counter so
/// rapid creates don't collide. Slot id stays as the panel id "prefix".
fn alloc_panel_id(slot_id: &str, existing: &HashSet<String>) -> String {
    let mut n = 1u64;
    loop {
        let candidate = format!("{slot_id}-tab-{n}");
        if !existing.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// Emit the targeted patches needed when a slot's membership (and possibly the
/// set of known panels) changes. Avoids re-serializing every panel's entries
/// like a full snapshot would.
async fn broadcast_slot_change(
    ctx: &IntentContext<'_>,
    slot_id: &str,
    new_panel_id: Option<&str>,
    focus_to: Option<&str>,
) {
    let (panel_upsert, slot_patch, layout_patch, focus_patch, gui_senders) = {
        let state_guard = ctx.state.lock().await;
        let slot = state_guard.slots.get(slot_id).cloned();
        let gui = state_guard.gui_senders.clone();

        // If a brand-new panel was added, send a "create" upsert so the FE
        // builds its PanelState entry before slot/layout patches reference it.
        let panel_upsert = new_panel_id.and_then(|pid| {
            let p = state_guard.panels.get(pid)?;
            let supports_thumbs = state_guard
                .drivers
                .get(&p.source_id)
                .map(|d| d.supports_thumbnails())
                .unwrap_or(false);
            Some(ServerMessage::StatePatch {
                timestamp: now(),
                domain: "panels".to_string(),
                entity_id: Some(pid.to_string()),
                version: p.version,
                patch: serde_json::json!({
                    "create": true,
                    "slotId": p.slot_id,
                    "tabKind": p.tab_kind.as_str(),
                    "tabTitle": p.tab_title_override,
                    "path": p.path,
                    "sourceId": p.source_id,
                    "status": p.status.as_str(),
                    "sort": p.sort,
                    "showHidden": p.show_hidden,
                    "viewMode": p.view_mode.as_str(),
                    "supportsThumbnails": supports_thumbs,
                    "terminalSessionId": p.terminal_session,
                }),
            })
        });

        let slot_patch = slot.map(|s| ServerMessage::StatePatch {
            timestamp: now(),
            domain: "slots".to_string(),
            entity_id: Some(s.slot_id.clone()),
            version: 0,
            patch: serde_json::json!({
                "tabPanelIds": s.tab_panel_ids,
                "activePanelId": s.active_panel_id,
            }),
        });

        let active_per_slot: Vec<String> = state_guard
            .layout_slots
            .iter()
            .filter_map(|sid| {
                state_guard
                    .slots
                    .get(sid)
                    .map(|s| s.active_panel_id.clone())
            })
            .collect();
        let layout_patch = ServerMessage::StatePatch {
            timestamp: now(),
            domain: "layout".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({
                "panels": active_per_slot,
                "slots": state_guard.layout_slots,
            }),
        };

        let focus_patch = focus_to.map(|fid| ServerMessage::StatePatch {
            timestamp: now(),
            domain: "focus".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "focus": fid }),
        });

        (panel_upsert, slot_patch, layout_patch, focus_patch, gui)
    };

    // Send panel-upsert FIRST so slot/layout patches reference a known id.
    if let Some(p) = &panel_upsert {
        let _ = ctx.tx.send(p.clone()).await;
        for s in &gui_senders {
            let _ = s.send(p.clone()).await;
        }
    }
    if let Some(p) = &slot_patch {
        let _ = ctx.tx.send(p.clone()).await;
        for s in &gui_senders {
            let _ = s.send(p.clone()).await;
        }
    }
    let _ = ctx.tx.send(layout_patch.clone()).await;
    for s in &gui_senders {
        let _ = s.send(layout_patch.clone()).await;
    }
    if let Some(p) = &focus_patch {
        let _ = ctx.tx.send(p.clone()).await;
        for s in &gui_senders {
            let _ = s.send(p.clone()).await;
        }
    }
}

async fn new_tab(args: &Value, ctx: &mut IntentContext<'_>) {
    let slot_id = match args.get("slotId").and_then(Value::as_str) {
        Some(s) => s.to_string(),
        None => return,
    };
    // Optional: caller can supply a specific panel to clone from (e.g. the
    // leftmost non-terminal tab when double-clicking empty tab-bar space).
    let base_panel_id = args
        .get("basePanelId")
        .and_then(Value::as_str)
        .map(str::to_string);

    // Clone the requested (or active) tab's source/path/view defaults to seed the new panel.
    let (new_panel, driver, supports_thumbs, sort, filter, show_hidden, version) = {
        let mut state_guard = ctx.state.lock().await;

        let Some(slot) = state_guard.slots.get(&slot_id).cloned() else {
            return;
        };
        let source_id = base_panel_id.unwrap_or_else(|| slot.active_panel_id.clone());
        let Some(source) = state_guard.panels.get(&source_id).cloned() else {
            return;
        };

        let panel_ids: HashSet<String> = state_guard.panels.keys().cloned().collect();
        let new_panel_id = alloc_panel_id(&slot_id, &panel_ids);

        let new_panel = Panel {
            panel_id: new_panel_id.clone(),
            slot_id: slot_id.clone(),
            tab_kind: TabKind::Listing,
            tab_title_override: None,
            source_id: source.source_id.clone(),
            path: source.path.clone(),
            status: PanelStatus::Loading,
            sort: source.sort.clone(),
            filter: None,
            filter_mode_active: false,
            show_hidden: source.show_hidden,
            view_mode: source.view_mode,
            cursor: 0,
            selection: vec![],
            entries: vec![],
            version: 0,
            calculating_size: Default::default(),
            windowed: false,
            window_start: 0,
            window_count: 0,
            window_version: 0,
            source_paths: source.source_paths.clone(),
            shadow_source: None,
            terminal_session: None,
        };

        let driver = state_guard.drivers.get(&source.source_id).cloned();
        let supports_thumbs = driver
            .as_ref()
            .map(|d| d.supports_thumbnails())
            .unwrap_or(false);

        // Adjust view if the cloned source doesn't support thumbnails.
        let mut new_panel = new_panel;
        if !supports_thumbs {
            new_panel.view_mode = PanelViewMode::List;
        }

        let sort = new_panel.sort.clone();
        let filter = new_panel.filter.clone();
        let show_hidden = new_panel.show_hidden;
        let path = new_panel.path.clone();
        let version = new_panel.version;

        // Insert panel + update slot.
        state_guard
            .panels
            .insert(new_panel_id.clone(), new_panel.clone());
        if let Some(slot) = state_guard.slots.get_mut(&slot_id) {
            let active_idx = slot
                .tab_panel_ids
                .iter()
                .position(|id| id == &slot.active_panel_id)
                .unwrap_or(slot.tab_panel_ids.len().saturating_sub(1));
            slot.tab_panel_ids
                .insert(active_idx + 1, new_panel_id.clone());
            slot.active_panel_id = new_panel_id.clone();
        }
        // Move focus to the newly created tab.
        state_guard.focus = new_panel_id.clone();

        let _ = path; // path captured inside `new_panel.path` already
        (
            new_panel,
            driver,
            supports_thumbs,
            sort,
            filter,
            show_hidden,
            version,
        )
    };

    broadcast_slot_change(
        ctx,
        &slot_id,
        Some(&new_panel.panel_id),
        Some(&new_panel.panel_id),
    )
    .await;

    // Kick off the scan for the new tab so its listing populates.
    if let Some(driver) = driver {
        let _ = supports_thumbs;
        let handle = tokio::spawn(run_scan(
            new_panel.panel_id.clone(),
            new_panel.path.clone(),
            sort,
            filter,
            version,
            ctx.tx.clone(),
            driver,
            None,
            None,
            None,
            Arc::clone(ctx.state),
            Arc::clone(ctx.dir_size_queue),
            false,
            show_hidden,
        ));
        ctx.scan_handles.insert(
            new_panel.panel_id.clone(),
            crate::jobs::ScanHandle { handle },
        );
    }
}

async fn close_tab(args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = match args.get("panelId").and_then(Value::as_str) {
        Some(s) => s.to_string(),
        None => return,
    };

    // Resolve the slot and decide who the new active will be. Abort if this
    // is the slot's only tab.
    let (slot_id, new_active, terminal_session) = {
        let state_guard = ctx.state.lock().await;
        let Some(panel) = state_guard.panels.get(&panel_id) else {
            return;
        };
        let slot_id = panel.slot_id.clone();
        let Some(slot) = state_guard.slots.get(&slot_id) else {
            return;
        };
        if slot.tab_panel_ids.len() <= 1 {
            return; // last tab — no-op
        }
        let idx = match slot.tab_panel_ids.iter().position(|id| id == &panel_id) {
            Some(i) => i,
            None => return,
        };
        // Activate the left neighbor when possible; else the right one.
        let new_active = if idx > 0 {
            slot.tab_panel_ids[idx - 1].clone()
        } else {
            slot.tab_panel_ids[idx + 1].clone()
        };
        (slot_id, new_active, panel.terminal_session)
    };

    // If the tab has a PTY, kill it.
    if let Some(sid) = terminal_session {
        let pty_manager = ctx.state.lock().await.pty_manager.clone();
        pty_manager.close(sid);
    }

    // Cancel any in-flight scan for the closing tab.
    if let Some(handle) = ctx.scan_handles.remove(&panel_id) {
        handle.abort();
    }

    let focus_followed = {
        let mut state_guard = ctx.state.lock().await;
        let prev_focus = state_guard.focus.clone();
        if let Some(slot) = state_guard.slots.get_mut(&slot_id) {
            slot.tab_panel_ids.retain(|id| id != &panel_id);
            slot.active_panel_id = new_active.clone();
        }
        state_guard.panels.remove(&panel_id);
        if prev_focus == panel_id {
            state_guard.focus = new_active.clone();
            true
        } else {
            false
        }
    };

    broadcast_slot_change(
        ctx,
        &slot_id,
        None,
        if focus_followed {
            Some(&new_active)
        } else {
            None
        },
    )
    .await;
}

async fn promote_terminal(args: &Value, ctx: &mut IntentContext<'_>) {
    let source_panel_id = match args.get("panelId").and_then(Value::as_str) {
        Some(s) => s.to_string(),
        None => return,
    };

    // Snapshot what we need from the source panel, decide on the new tab id,
    // and mutate the AppState atomically.
    let pty_rebind: Option<(u32, String)> = {
        let mut state_guard = ctx.state.lock().await;
        let Some(source) = state_guard.panels.get(&source_panel_id).cloned() else {
            return;
        };
        let Some(sid) = source.terminal_session else {
            return;
        };
        let slot_id = source.slot_id.clone();
        let Some(slot_snapshot) = state_guard.slots.get(&slot_id).cloned() else {
            return;
        };

        let panel_ids: HashSet<String> = state_guard.panels.keys().cloned().collect();
        let new_panel_id = alloc_panel_id(&slot_id, &panel_ids);

        // Build a terminal-kind tab that carries the PTY session. Listing fields
        // stay empty — the FE doesn't render the listing for terminal tabs.
        let new_panel = Panel {
            panel_id: new_panel_id.clone(),
            slot_id: slot_id.clone(),
            tab_kind: TabKind::Terminal,
            tab_title_override: None,
            source_id: source.source_id.clone(),
            path: source.path.clone(),
            status: PanelStatus::Ready,
            sort: source.sort.clone(),
            filter: None,
            filter_mode_active: false,
            show_hidden: source.show_hidden,
            view_mode: PanelViewMode::List,
            cursor: 0,
            selection: vec![],
            entries: vec![],
            version: 0,
            calculating_size: Default::default(),
            windowed: false,
            window_start: 0,
            window_count: 0,
            window_version: 0,
            source_paths: source.source_paths.clone(),
            shadow_source: None,
            terminal_session: Some(sid),
        };

        // Detach the PTY from the source panel.
        if let Some(src_mut) = state_guard.panels.get_mut(&source_panel_id) {
            src_mut.terminal_session = None;
            src_mut.version += 1;
        }

        // Insert the new terminal tab into the slot after the source tab.
        state_guard.panels.insert(new_panel_id.clone(), new_panel);
        if let Some(slot) = state_guard.slots.get_mut(&slot_id) {
            let idx = slot
                .tab_panel_ids
                .iter()
                .position(|id| id == &source_panel_id)
                .unwrap_or(slot_snapshot.tab_panel_ids.len().saturating_sub(1));
            slot.tab_panel_ids.insert(idx + 1, new_panel_id.clone());
            slot.active_panel_id = new_panel_id.clone();
        }
        state_guard.focus = new_panel_id.clone();

        Some((sid, new_panel_id))
    };

    if let Some((sid, new_panel_id)) = &pty_rebind {
        let pty_manager = ctx.state.lock().await.pty_manager.clone();
        pty_manager.rebind_panel(*sid, new_panel_id.clone());
    }

    let Some((_, new_panel_id)) = pty_rebind else {
        return;
    };

    // Tell the FE the source panel no longer hosts an embedded terminal.
    let (src_version, gui_senders, slot_id) = {
        let state_guard = ctx.state.lock().await;
        let version = state_guard
            .panels
            .get(&source_panel_id)
            .map(|p| p.version)
            .unwrap_or(0);
        let slot_id = state_guard
            .panels
            .get(&new_panel_id)
            .map(|p| p.slot_id.clone())
            .unwrap_or_default();
        (version, state_guard.gui_senders.clone(), slot_id)
    };
    let src_patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "panels".to_string(),
        entity_id: Some(source_panel_id.clone()),
        version: src_version,
        patch: serde_json::json!({ "terminalSessionId": null }),
    };
    let _ = ctx.tx.send(src_patch.clone()).await;
    for s in &gui_senders {
        let _ = s.send(src_patch.clone()).await;
    }

    broadcast_slot_change(ctx, &slot_id, Some(&new_panel_id), Some(&new_panel_id)).await;
}

async fn switch_tab(args: &Value, ctx: &mut IntentContext<'_>) {
    let slot_id = match args.get("slotId").and_then(Value::as_str) {
        Some(s) => s.to_string(),
        None => return,
    };
    let panel_id = match args.get("panelId").and_then(Value::as_str) {
        Some(s) => s.to_string(),
        None => return,
    };

    // Mutate slot + focus, then emit small targeted patches. Avoid a full
    // snapshot — those re-serialize every panel's entries and stall when any
    // tab is sitting on a big directory.
    let (focus_follow, slot_snapshot, layout_panels, layout_slots, gui_senders) = {
        let mut state_guard = ctx.state.lock().await;
        let Some(slot) = state_guard.slots.get_mut(&slot_id) else {
            return;
        };
        if !slot.tab_panel_ids.contains(&panel_id) {
            return;
        }
        let prev_active = slot.active_panel_id.clone();
        slot.active_panel_id = panel_id.clone();
        let snapshot = slot.clone();
        let focus_follow = state_guard.focus == prev_active;
        if focus_follow {
            state_guard.focus = panel_id.clone();
        }
        let panels_active: Vec<String> = state_guard
            .layout_slots
            .iter()
            .filter_map(|sid| {
                state_guard
                    .slots
                    .get(sid)
                    .map(|s| s.active_panel_id.clone())
            })
            .collect();
        (
            focus_follow,
            snapshot,
            panels_active,
            state_guard.layout_slots.clone(),
            state_guard.gui_senders.clone(),
        )
    };

    // slot.activate patch
    let slot_patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "slots".to_string(),
        entity_id: Some(slot_snapshot.slot_id.clone()),
        version: 0,
        patch: serde_json::json!({
            "tabPanelIds": slot_snapshot.tab_panel_ids,
            "activePanelId": slot_snapshot.active_panel_id,
        }),
    };
    let _ = ctx.tx.send(slot_patch.clone()).await;
    for sender in &gui_senders {
        let _ = sender.send(slot_patch.clone()).await;
    }

    // layout.panels patch — keeps Layout.tsx rendering the correct active tab.
    let layout_patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "layout".to_string(),
        entity_id: None,
        version: 0,
        patch: serde_json::json!({
            "panels": layout_panels,
            "slots": layout_slots,
        }),
    };
    let _ = ctx.tx.send(layout_patch.clone()).await;
    for sender in &gui_senders {
        let _ = sender.send(layout_patch.clone()).await;
    }

    if focus_follow {
        let focus_patch = ServerMessage::StatePatch {
            timestamp: now(),
            domain: "focus".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "focus": panel_id }),
        };
        let _ = ctx.tx.send(focus_patch.clone()).await;
        for sender in &gui_senders {
            let _ = sender.send(focus_patch.clone()).await;
        }
    }
}
