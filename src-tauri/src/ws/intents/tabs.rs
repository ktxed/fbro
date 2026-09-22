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

fn can_clone_active_tab(
    tab_kind: TabKind,
    source_kind: Option<&str>,
    source_mounted: bool,
    source_shadow: bool,
    has_shadow_context: bool,
    driver_is_local: bool,
) -> bool {
    tab_kind == TabKind::Listing
        && source_kind == Some("filesystem")
        && source_mounted
        && !source_shadow
        && !has_shadow_context
        && driver_is_local
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
    // Clone local filesystem listings. Virtual, remote, and terminal tabs do
    // not carry a safe filesystem context to duplicate, so they seed at Home.
    let (new_panel, driver, sort, filter, show_hidden, version) = {
        let mut state_guard = ctx.state.lock().await;

        let Some(slot) = state_guard.slots.get(&slot_id).cloned() else {
            return;
        };
        let Some(source) = state_guard.panels.get(&slot.active_panel_id).cloned() else {
            return;
        };

        let active_source = state_guard.sources.get(&source.source_id);
        let active_driver = state_guard.drivers.get(&source.source_id);
        let clone_active = can_clone_active_tab(
            source.tab_kind,
            active_source.map(|item| item.kind.as_str()),
            active_source.map(|item| item.mounted).unwrap_or(false),
            active_source.map(|item| item.shadow).unwrap_or(true),
            source.shadow_source.is_some(),
            active_driver.map(|item| item.is_local()).unwrap_or(false),
        );

        let mut source_paths = source.source_paths.clone();
        let (new_source_id, new_path, git) = if clone_active {
            (
                source.source_id.clone(),
                source.path.clone(),
                source.git.clone(),
            )
        } else {
            let home = state_guard.platform.home_dir();
            let Some(home_source_id) = state_guard.fallback_source_id_for_home() else {
                return;
            };
            source_paths.insert(home_source_id.clone(), home.clone());
            (home_source_id, home, None)
        };

        let Some(driver) = state_guard.drivers.get(&new_source_id).cloned() else {
            return;
        };

        let panel_ids: HashSet<String> = state_guard.panels.keys().cloned().collect();
        let new_panel_id = alloc_panel_id(&slot_id, &panel_ids);

        let new_panel = Panel {
            panel_id: new_panel_id.clone(),
            slot_id: slot_id.clone(),
            tab_kind: TabKind::Listing,
            tab_title_override: None,
            source_id: new_source_id,
            path: new_path,
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
            panel_epoch: 0,
            calculating_size: Default::default(),
            windowed: false,
            window_start: 0,
            window_count: 0,
            window_version: 0,
            source_paths,
            shadow_source: None,
            terminal_session: None,
            git,
        };

        let supports_thumbs = driver.supports_thumbnails();

        // Adjust view if the cloned source doesn't support thumbnails.
        let mut new_panel = new_panel;
        if !supports_thumbs {
            new_panel.view_mode = PanelViewMode::List;
        }

        let sort = new_panel.sort.clone();
        let filter = new_panel.filter.clone();
        let show_hidden = new_panel.show_hidden;
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

        (new_panel, driver, sort, filter, show_hidden, version)
    };

    broadcast_slot_change(
        ctx,
        &slot_id,
        Some(&new_panel.panel_id),
        Some(&new_panel.panel_id),
    )
    .await;

    // Kick off the scan for the new tab so its listing populates.
    let handle = tokio::spawn(run_scan(
        new_panel.panel_id.clone(),
        new_panel.source_id.clone(),
        new_panel.panel_epoch,
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
        ctx.watcher_cmd_tx.clone(),
        true,
    ));
    ctx.scan_handles.insert(
        new_panel.panel_id.clone(),
        crate::jobs::ScanHandle { handle },
    );
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
            eprintln!("[fbro] promote_terminal: source panel {source_panel_id} not found");
            return;
        };
        let Some(sid) = source.terminal_session else {
            eprintln!("[fbro] promote_terminal: source panel {source_panel_id} has no terminal session");
            return;
        };
        let slot_id = source.slot_id.clone();
        let Some(slot_snapshot) = state_guard.slots.get(&slot_id).cloned() else {
            eprintln!("[fbro] promote_terminal: slot {slot_id} not found");
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
            panel_epoch: 0,
            calculating_size: Default::default(),
            windowed: false,
            window_start: 0,
            window_count: 0,
            window_version: 0,
            source_paths: source.source_paths.clone(),
            shadow_source: None,
            terminal_session: Some(sid),
            git: source.git.clone(),
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

        eprintln!("[fbro] promote_terminal: promoted {source_panel_id} → {new_panel_id} with session {sid}");
        Some((sid, new_panel_id))
    };

    if let Some((sid, new_panel_id)) = &pty_rebind {
        let pty_manager = ctx.state.lock().await.pty_manager.clone();
        pty_manager.rebind_panel(*sid, new_panel_id.clone());
    }

    let Some((_, new_panel_id)) = pty_rebind else {
        return;
    };

    // Broadcast: source panel cleared + new panel created + slot updated. Send these patches
    // atomically to avoid frontend race conditions where both terminals briefly appear.
    let slot_id = {
        let state_guard = ctx.state.lock().await;
        state_guard
            .panels
            .get(&new_panel_id)
            .map(|p| p.slot_id.clone())
            .unwrap_or_default()
    };

    let (panel_upsert, src_patch, slot_patch, layout_patch, focus_patch, gui_senders) = {
        let state_guard = ctx.state.lock().await;
        let slot = state_guard.slots.get(&slot_id).cloned();
        let gui = state_guard.gui_senders.clone();

        // Source panel patch: clear terminal
        let src_version = state_guard
            .panels
            .get(&source_panel_id)
            .map(|p| p.version)
            .unwrap_or(0);
        let src_patch = ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(source_panel_id.clone()),
            version: src_version,
            patch: serde_json::json!({ "terminalSessionId": null }),
        };

        // New panel upsert
        let panel_upsert = state_guard.panels.get(&new_panel_id).map(|p| {
            let supports_thumbs = state_guard
                .drivers
                .get(&p.source_id)
                .map(|d| d.supports_thumbnails())
                .unwrap_or(false);
            ServerMessage::StatePatch {
                timestamp: now(),
                domain: "panels".to_string(),
                entity_id: Some(new_panel_id.clone()),
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
            }
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
                "slots": state_guard.layout_slots.clone(),
            }),
        };

        let focus_patch = ServerMessage::StatePatch {
            timestamp: now(),
            domain: "focus".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "focus": new_panel_id }),
        };

        (panel_upsert, src_patch, slot_patch, layout_patch, focus_patch, gui)
    };

    eprintln!("[fbro] promote_terminal: sending atomic patches for {source_panel_id} → {new_panel_id}");

    // Send in order: source cleared, new created, slot updated, layout updated, focus
    let _ = ctx.tx.send(src_patch.clone()).await;
    for s in &gui_senders {
        let _ = s.send(src_patch.clone()).await;
    }

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

    let _ = ctx.tx.send(focus_patch.clone()).await;
    for s in &gui_senders {
        let _ = s.send(focus_patch.clone()).await;
    }
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

    crate::ws::intents::git::refresh_panel_git_status(&panel_id, ctx).await;
}

#[cfg(test)]
mod tests {
    use super::can_clone_active_tab;
    use crate::state::TabKind;

    #[test]
    fn local_filesystem_listing_is_cloneable() {
        assert!(can_clone_active_tab(
            TabKind::Listing,
            Some("filesystem"),
            true,
            false,
            false,
            true,
        ));
    }

    #[test]
    fn non_physical_tabs_fall_back_to_home() {
        for (tab_kind, source_kind, source_shadow, has_shadow_context, driver_is_local) in [
            (TabKind::Listing, Some("virtual"), true, true, false),
            (TabKind::Listing, Some("remote"), false, false, false),
            (TabKind::Terminal, Some("filesystem"), false, false, true),
            (TabKind::Listing, Some("filesystem"), false, true, true),
        ] {
            assert!(!can_clone_active_tab(
                tab_kind,
                source_kind,
                true,
                source_shadow,
                has_shadow_context,
                driver_is_local,
            ));
        }
    }

    #[test]
    fn unavailable_or_non_local_filesystem_falls_back_to_home() {
        assert!(!can_clone_active_tab(
            TabKind::Listing,
            Some("filesystem"),
            false,
            false,
            false,
            true,
        ));
        assert!(!can_clone_active_tab(
            TabKind::Listing,
            Some("filesystem"),
            true,
            false,
            false,
            false,
        ));
        assert!(!can_clone_active_tab(
            TabKind::Listing,
            None,
            true,
            false,
            false,
            true,
        ));
    }
}
