use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;

use crate::jobs::file_ops::run_file_op;
use crate::jobs::scan::run_scan;
use crate::jobs::{ConflictPolicy, FileOpEntry, FileOpJob, FileOpType, JobState, ScanHandle};
use crate::persistence::{PanelSession, SessionState};
use crate::state::{PanelStatus, SharedState};
use crate::vfs::VfsDriver;
use crate::ws::protocol::{
    now, ClientMessage, FileOpFileSchema, JobStartSchema, LayoutDomain, PanelSnapshot,
    ServerMessage, SourceCapabilities, SourceSchema, StateDomains,
};

const WS_ADDR: &str = "127.0.0.1:7878";
const CHANNEL_CAP: usize = 512;

pub async fn start(state: SharedState, app_handle: tauri::AppHandle, quit_notify: Arc<Notify>) {
    let listener = match TcpListener::bind(WS_ADDR).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[fbro] Failed to bind WebSocket server on {WS_ADDR}: {e}");
            return;
        }
    };
    eprintln!("[fbro] WebSocket server listening on ws://{WS_ADDR}");

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let state = Arc::clone(&state);
                let app_handle = app_handle.clone();
                let quit_notify = Arc::clone(&quit_notify);
                tokio::spawn(handle_connection(stream, addr, state, app_handle, quit_notify));
            }
            Err(e) => eprintln!("[fbro] Accept error: {e}"),
        }
    }
}

async fn handle_connection(stream: TcpStream, addr: SocketAddr, state: SharedState, app_handle: tauri::AppHandle, quit_notify: Arc<Notify>) {
    eprintln!("[fbro] Client connected: {addr}");

    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("[fbro] WS handshake failed: {e}");
            return;
        }
    };

    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel::<ServerMessage>(CHANNEL_CAP);

    // ── Wait for Hello ────────────────────────────────────────────────────────
    let hello_client = loop {
        match ws_rx.next().await {
            Some(Ok(Message::Text(text))) => {
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Hello { protocol_version, client }) => {
                        eprintln!("[fbro] Hello from {:?} kind={:?} (protocol v{})", client.name, client.kind, protocol_version);
                        break client;
                    }
                    Ok(_) => eprintln!("[fbro] Expected Hello, got other intent"),
                    Err(e) => eprintln!("[fbro] Parse error: {e}"),
                }
            }
            Some(Ok(Message::Close(_))) | None => return,
            _ => {}
        }
    };

    // Viewer connections only need file chunk reads — skip state snapshot and
    // panel scans, which would corrupt the shared cursor/entry state.
    let is_gui = hello_client.kind == "gui";

    let session_id = uuid::Uuid::new_v4().to_string();

    // ── Send Welcome ──────────────────────────────────────────────────────────
    let _ = send_msg(&mut ws_tx, &ServerMessage::Welcome {
        timestamp: now(),
        protocol_version: "0".to_string(),
        session_id: session_id.clone(),
    })
    .await;

    // ── Send initial StateSnapshot (gui clients only) ──────────────────────────
    let (snapshot, panel_ids) = if is_gui { {
        let mut s = state.lock().await;
        // Clear stale entries so the snapshot + scan don't double on reconnect
        for panel in s.panels.values_mut() {
            panel.entries.clear();
            panel.status = PanelStatus::Loading;
            panel.version += 1;
        }
        let panel_ids: Vec<String> = s.panels.keys().cloned().collect::<Vec<_>>();

        let panels: std::collections::HashMap<String, PanelSnapshot> = s
            .panels
            .iter()
            .map(|(id, p)| {
                (
                    id.clone(),
                    PanelSnapshot {
                        panel_id: p.panel_id.clone(),
                        path: p.path.clone(),
                        status: p.status.as_str().to_string(),
                        source_id: p.source_id.clone(),
                        sort: p.sort.clone(),
                        filter: p.filter.clone(),
                        cursor: p.cursor,
                        selection: p.selection.clone(),
                        entries: p.entries.clone(),
                        version: p.version,
                    },
                )
            })
            .collect();

        let sources: Vec<SourceSchema> = s
            .sources
            .values()
            .map(|src| SourceSchema {
                id: src.id.clone(),
                name: src.name.clone(),
                kind: src.kind.clone(),
                supports: src.supports.clone(),
                capabilities: SourceCapabilities {
                    read_only: false,
                    supports_streaming: true,
                    supports_jobs: true,
                },
                root_path: src.root_path.clone(),
            })
            .collect();

        // Sort panel IDs for stable ordering: left then right
        let mut sorted_panel_ids = panel_ids.clone();
        sorted_panel_ids.sort();

        let snapshot = ServerMessage::StateSnapshot {
            timestamp: now(),
            domains: StateDomains {
                layout: LayoutDomain { panels: sorted_panel_ids },
                panels,
                sources,
                focus: s.focus.clone(),
                jobs: vec![],
            },
        };

        (Some(snapshot), panel_ids)
    } } else {
        (None, vec![])
    };

    if let Some(snapshot) = snapshot {
        let _ = send_msg(&mut ws_tx, &snapshot).await;
    }

    // ── Spawn initial scan jobs (gui clients only) ─────────────────────────────
    let mut scan_handles: HashMap<String, ScanHandle> = HashMap::new();
    let mut decision_senders: HashMap<String, mpsc::Sender<String>> = HashMap::new();

    for panel_id in &panel_ids {
        let (path, sort, filter, version_base, driver) = {
            let s = state.lock().await;
            let p = s.panels.get(panel_id).unwrap();
            let driver = s.drivers.get(&p.source_id).cloned().unwrap();
            (p.path.clone(), p.sort.clone(), p.filter.clone(), p.version, driver)
        };

        let h = spawn_scan(panel_id.clone(), path, sort, filter, version_base, tx.clone(), driver, None);
        scan_handles.insert(panel_id.clone(), h);
    }

    // Register GUI client's sender for cross-connection broadcasts
    if is_gui {
        let mut s = state.lock().await;
        s.gui_senders.push(tx.clone());
    }

    // ── Main loop: fanout outgoing messages / dispatch incoming intents ────────
    loop {
        tokio::select! {
            // Outgoing: messages queued by scan jobs or intent handlers
            Some(msg) = rx.recv() => {
                // Update our state mirror for patches
                if let ServerMessage::StatePatch { ref domain, ref entity_id, ref patch, version, .. } = msg {
                    if domain == "panels" {
                        if let Some(panel_id) = entity_id {
                            update_panel_from_patch(&state, panel_id, patch, version).await;
                        }
                    }
                }

                // When a job completes, rescan affected panels
                if let ServerMessage::JobEvent { ref job_id, ref event, .. } = msg {
                    if event == "Completed" || event == "Cancelled" || event == "Failed" {
                        let panels_to_rescan = {
                            let s = state.lock().await;
                            if let Some(job) = s.job_store.jobs.get(job_id) {
                                let mut panels = vec![job.source_panel.clone()];
                                if let Some(ref dp) = job.dest_panel {
                                    panels.push(dp.clone());
                                }
                                panels
                            } else {
                                vec![]
                            }
                        };
                        for pid in panels_to_rescan {
                            rescan_panel(&pid, &state, &tx, &mut scan_handles, None).await;
                        }
                    }
                }

                if send_msg(&mut ws_tx, &msg).await.is_err() {
                    break;
                }
            }

            // OS window close button: treat like "Quit" intent
            _ = quit_notify.notified(), if is_gui => {
                let active_jobs = {
                    let s = state.lock().await;
                    s.job_store.jobs.values().filter(|j| {
                        j.state == JobState::Running || j.state == JobState::AwaitingDecision
                    }).count()
                };
                if active_jobs == 0 {
                    save_session(&state).await;
                    std::process::exit(0);
                } else {
                    let _ = tx.send(ServerMessage::QuitConfirmRequired {
                        timestamp: now(),
                        active_jobs,
                    }).await;
                }
            }

            // Incoming: intents from the FE
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::Intent { name, scope, args, .. }) => {
                                eprintln!("[fbro] intent: {name} scope={scope} args={args}");
                                handle_intent(
                                    &name,
                                    &scope,
                                    &args,
                                    &state,
                                    &tx,
                                    &mut scan_handles,
                                    &mut decision_senders,
                                    &app_handle,
                                )
                                .await;
                            }
                            Ok(ClientMessage::Hello { .. }) => {
                                // ignore duplicate Hello
                            }
                            Err(e) => eprintln!("[fbro] Intent parse error: {e} — raw: {text}"),
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = ws_tx.send(Message::Pong(p)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    eprintln!("[fbro] Client disconnected: {addr}");
    for h in scan_handles.values() {
        h.abort();
    }

    // Unregister GUI sender
    if is_gui {
        let mut s = state.lock().await;
        s.gui_senders.retain(|s| !s.is_closed());
    }
}

// ─── Intent dispatch ─────────────────────────────────────────────────────────

async fn handle_intent(
    name: &str,
    scope: &serde_json::Value,
    args: &serde_json::Value,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
    decision_senders: &mut HashMap<String, mpsc::Sender<String>>,
    app_handle: &tauri::AppHandle,
) {
    match name {
        "RefreshPanels" => {
            let panel_ids: Vec<String> = {
                let s = state.lock().await;
                s.panels.keys().cloned().collect()
            };
            for panel_id in panel_ids {
                rescan_panel(&panel_id, state, &tx, scan_handles, None).await;
            }
        }

        "SetSort" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let by    = args.get("by").and_then(|v| v.as_str()).unwrap_or("name").to_string();
            let order = args.get("order").and_then(|v| v.as_str()).unwrap_or("asc").to_string();
            {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    panel.sort = crate::ws::protocol::SortSpec { by, order };
                }
            }
            rescan_panel(&panel_id, state, &tx, scan_handles, None).await;
        }

        "MoveCursor" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let direction = args.get("direction").and_then(|v| v.as_str()).unwrap_or("down");
            let (new_cursor, version) = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    let len = panel.entries.len();
                    if len == 0 {
                        return;
                    }
                    let cursor = match direction {
                        "up" => panel.cursor.saturating_sub(1),
                        _ => (panel.cursor + 1).min(len - 1),
                    };
                    panel.cursor = cursor;
                    panel.version += 1;
                    (cursor, panel.version)
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "cursor": new_cursor }),
                })
                .await;
        }

        "SetSelection" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let indices: Vec<usize> = args
                .get("indices")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_u64().map(|n| n as usize)).collect())
                .unwrap_or_default();
            let version = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    panel.selection = indices.clone();
                    panel.version += 1;
                    panel.version
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "selection": indices }),
                })
                .await;
        }

        "SelectAtIndex" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let index = args.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let (new_selection, version) = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    if index >= panel.entries.len() || panel.selection.contains(&index) {
                        return;
                    }
                    panel.selection.push(index);
                    panel.version += 1;
                    (panel.selection.clone(), panel.version)
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "selection": new_selection }),
                })
                .await;
        }

        "DeselectAtIndex" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let index = args.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let (new_selection, version) = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    let before = panel.selection.len();
                    panel.selection.retain(|&i| i != index);
                    if panel.selection.len() == before {
                        return; // wasn't selected, nothing changed
                    }
                    panel.version += 1;
                    (panel.selection.clone(), panel.version)
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "selection": new_selection }),
                })
                .await;
        }

        "ToggleSelectionAtIndex" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let index = args.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let (new_selection, version) = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    if index >= panel.entries.len() {
                        return;
                    }
                    if let Some(pos) = panel.selection.iter().position(|&i| i == index) {
                        panel.selection.remove(pos);
                    } else {
                        panel.selection.push(index);
                    }
                    panel.version += 1;
                    (panel.selection.clone(), panel.version)
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "selection": new_selection }),
                })
                .await;
        }

        "ToggleSelection" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let (new_selection, version) = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    let cursor = panel.cursor;
                    if let Some(pos) = panel.selection.iter().position(|&i| i == cursor) {
                        panel.selection.remove(pos);
                    } else {
                        panel.selection.push(cursor);
                    }
                    panel.version += 1;
                    (panel.selection.clone(), panel.version)
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "selection": new_selection }),
                })
                .await;
        }

        "SelectAll" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let (indices, version) = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    let indices: Vec<usize> = (0..panel.entries.len()).collect();
                    panel.selection = indices.clone();
                    panel.version += 1;
                    (indices, panel.version)
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "selection": indices }),
                })
                .await;
        }

        "MoveCursorWithSelect" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let direction = args.get("direction").and_then(|v| v.as_str()).unwrap_or("down");
            let (new_cursor, new_selection, version) = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    let len = panel.entries.len();
                    if len == 0 {
                        return;
                    }
                    let cursor = match direction {
                        "up" => panel.cursor.saturating_sub(1),
                        _ => (panel.cursor + 1).min(len - 1),
                    };
                    let prev = panel.cursor;
                    panel.cursor = cursor;
                    if panel.selection.contains(&cursor) {
                        // Shrinking: remove both destination and the entry we left
                        panel.selection.retain(|&i| i != cursor && i != prev);
                    } else {
                        // Extending: add destination + starting point if needed
                        if !panel.selection.contains(&prev) {
                            panel.selection.push(prev);
                        }
                        panel.selection.push(cursor);
                    }
                    panel.version += 1;
                    (cursor, panel.selection.clone(), panel.version)
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "cursor": new_cursor, "selection": new_selection }),
                })
                .await;
        }

        "MoveCursorBy" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let delta = args.get("delta").and_then(|v| v.as_i64()).unwrap_or(0);
            let (new_cursor, version) = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    let len = panel.entries.len();
                    if len == 0 { return; }
                    let cursor = (panel.cursor as i64 + delta).clamp(0, (len - 1) as i64) as usize;
                    if cursor == panel.cursor { return; }
                    panel.cursor = cursor;
                    panel.version += 1;
                    (cursor, panel.version)
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "cursor": new_cursor }),
                })
                .await;
        }

        "MoveCursorToEdge" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let edge = args.get("edge").and_then(|v| v.as_str()).unwrap_or("first");
            let (new_cursor, version) = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    let len = panel.entries.len();
                    if len == 0 {
                        return;
                    }
                    let cursor = if edge == "last" { len - 1 } else { 0 };
                    panel.cursor = cursor;
                    panel.version += 1;
                    (cursor, panel.version)
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "cursor": new_cursor }),
                })
                .await;
        }

        "SetCursor" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let index = args.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let (new_cursor, version) = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    let cursor = index.min(panel.entries.len().saturating_sub(1));
                    panel.cursor = cursor;
                    panel.version += 1;
                    (cursor, panel.version)
                } else {
                    return;
                }
            };
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version,
                    patch: serde_json::json!({ "cursor": new_cursor }),
                })
                .await;
        }

        "OpenEntry" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let (entry_kind, new_path) = {
                let s = state.lock().await;
                if let Some(panel) = s.panels.get(&panel_id) {
                    if let Some(entry) = panel.entries.get(panel.cursor) {
                        let kind = entry.kind.clone();
                        let driver = s.drivers.get(&panel.source_id).unwrap();
                        let full = driver.resolve_child(&panel.path, &entry.name);
                        (kind, full)
                    } else {
                        return;
                    }
                } else {
                    return;
                }
            };

            if entry_kind == "directory" {
                navigate_to(&panel_id, &new_path, None, state, tx, scan_handles).await;
            }
        }

        "NavigateHome" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let home = state.lock().await.platform.home_dir();
            navigate_to(&panel_id, &home, None, state, tx, scan_handles).await;
        }

        "OpenFile" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let (path, entry, platform) = {
                let s = state.lock().await;
                let panel = match s.panels.get(&panel_id) {
                    Some(p) => p,
                    None => return,
                };
                let entry = panel.entries.get(panel.cursor).cloned();
                (panel.path.clone(), entry, Arc::clone(&s.platform))
            };
            if let Some(entry) = entry {
                if entry.kind != "directory" {
                    let full_path = format!("{}/{}", path.trim_end_matches('/'), entry.name);
                    platform.open_file(&full_path);
                }
            }
        }

        "NavigateToSource" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let source_id = args.get("sourceId").and_then(|v| v.as_str()).unwrap_or("").to_string();

            let root_path = {
                let s = state.lock().await;
                s.sources.get(&source_id).map(|src| src.root_path.clone())
            };

            if let Some(root_path) = root_path {
                {
                    let mut s = state.lock().await;
                    if let Some(panel) = s.panels.get_mut(&panel_id) {
                        panel.source_id = source_id;
                        panel.path = root_path;
                        panel.cursor = 0;
                        panel.selection.clear();
                    }
                }
                rescan_panel(&panel_id, state, tx, scan_handles, None).await;
            }
        }

        "NavigateUp" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let (current_path, parent_path) = {
                let s = state.lock().await;
                if let Some(panel) = s.panels.get(&panel_id) {
                    let driver = s.drivers.get(&panel.source_id).unwrap();
                    (Some(panel.path.clone()), driver.parent_path(&panel.path))
                } else {
                    (None, None)
                }
            };
            if let Some(path) = current_path {
                if let Some(parent) = parent_path {
                    if parent != path {
                        let dir_name = path.trim_end_matches('/').rsplit('/').next().map(str::to_string);
                        navigate_to(&panel_id, &parent, dir_name, state, tx, scan_handles).await;
                    }
                }
            }
        }

        "SwitchFocus" => {
            let to = args.get("to").and_then(|v| v.as_str()).map(String::from);
            if let Some(target) = to {
                let version = {
                    let mut s = state.lock().await;
                    s.focus = target.clone();
                    1u64
                };
                let _ = tx
                    .send(ServerMessage::StatePatch {
                        timestamp: now(),
                        domain: "focus".to_string(),
                        entity_id: None,
                        version,
                        patch: serde_json::json!({ "focus": target }),
                    })
                    .await;
            }
        }

        "RequestStateSnapshot" => {
            let s = state.lock().await;
            let panels = s
                .panels
                .iter()
                .map(|(id, p)| {
                    (
                        id.clone(),
                        crate::ws::protocol::PanelSnapshot {
                            panel_id: p.panel_id.clone(),
                            path: p.path.clone(),
                            status: p.status.as_str().to_string(),
                            source_id: p.source_id.clone(),
                            sort: p.sort.clone(),
                            filter: p.filter.clone(),
                            cursor: p.cursor,
                            selection: p.selection.clone(),
                            entries: p.entries.clone(),
                            version: p.version,
                        },
                    )
                })
                .collect();

            let sources: Vec<SourceSchema> = s
                .sources
                .values()
                .map(|src| SourceSchema {
                    id: src.id.clone(),
                    name: src.name.clone(),
                    kind: src.kind.clone(),
                    supports: src.supports.clone(),
                    capabilities: SourceCapabilities {
                        read_only: false,
                        supports_streaming: true,
                        supports_jobs: true,
                    },
                    root_path: src.root_path.clone(),
                })
                .collect();

            let mut sorted_panel_ids: Vec<String> = s.panels.keys().cloned().collect();
            sorted_panel_ids.sort();

            let _ = tx
                .send(ServerMessage::StateSnapshot {
                    timestamp: now(),
                    domains: StateDomains {
                        layout: LayoutDomain { panels: sorted_panel_ids },
                        panels,
                        sources,
                        focus: s.focus.clone(),
                        jobs: vec![],
                    },
                })
                .await;
        }

        "StartFileOp" => {
            let source_panel_id = args.get("sourcePanel").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let dest_panel_id = args.get("destPanel").and_then(|v| v.as_str()).map(String::from);
            let op_type_str = args.get("opType").and_then(|v| v.as_str()).unwrap_or("copy");
            let conflict_str = args.get("conflictPolicy").and_then(|v| v.as_str()).unwrap_or("ask");
            let background = args.get("background").and_then(|v| v.as_bool()).unwrap_or(false);

            let op_type = match op_type_str {
                "move" => FileOpType::Move,
                "delete" => FileOpType::Delete,
                _ => FileOpType::Copy,
            };
            let conflict_policy = match conflict_str {
                "skip" => ConflictPolicy::Skip,
                "overwrite" => ConflictPolicy::Overwrite,
                _ => ConflictPolicy::Ask,
            };

            // Gather files from the source panel's selection (or cursor)
            let (files, source_path, dest_path, src_driver, dst_driver) = {
                let s = state.lock().await;
                let Some(src_panel) = s.panels.get(&source_panel_id) else { return };
                let src_driver = s.drivers.get(&src_panel.source_id).cloned().unwrap();

                let indices: Vec<usize> = if src_panel.selection.is_empty() {
                    vec![src_panel.cursor]
                } else {
                    src_panel.selection.clone()
                };

                let files: Vec<FileOpEntry> = indices.iter().filter_map(|&idx| {
                    src_panel.entries.get(idx).map(|e| FileOpEntry {
                        name: e.name.clone(),
                        source_path: src_driver.resolve_child(&src_panel.path, &e.name),
                        size: e.size,
                        kind: e.kind.clone(),
                    })
                }).collect();

                let (dest_path, dst_driver) = if op_type != FileOpType::Delete {
                    if let Some(dp) = dest_panel_id.as_ref().and_then(|id| s.panels.get(id)) {
                        let drv = s.drivers.get(&dp.source_id).cloned().unwrap();
                        (Some(dp.path.clone()), Some(drv))
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                };

                let source_path = src_panel.path.clone();
                (files, source_path, dest_path, src_driver, dst_driver)
            };

            if files.is_empty() { return; }

            // Create the job
            let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
            let (dec_tx, dec_rx) = mpsc::channel::<String>(4);

            let job_id = {
                let mut s = state.lock().await;
                s.job_store.next_job_id()
            };

            // Send JobStarted to FE
            let file_schemas: Vec<FileOpFileSchema> = files.iter().map(|f| FileOpFileSchema {
                name: f.name.clone(),
                size: f.size,
                kind: f.kind.clone(),
            }).collect();

            let total_bytes_total: u64 = files.iter().map(|f| f.size.unwrap_or(0)).sum();
            let _ = tx.send(ServerMessage::JobStarted {
                timestamp: now(),
                job: JobStartSchema {
                    job_id: job_id.clone(),
                    op_type: op_type_str.to_string(),
                    files: file_schemas,
                    source_panel: source_panel_id.clone(),
                    source_path,
                    dest_panel: dest_panel_id.clone(),
                    dest_path: dest_path.clone(),
                    total_files: files.len(),
                    total_bytes_total,
                    background,
                },
            }).await;

            // Store decision sender
            decision_senders.insert(job_id.clone(), dec_tx);

            // Spawn the job
            let handle = tokio::spawn(run_file_op(
                job_id.clone(),
                op_type.clone(),
                files.clone(),
                dest_path.clone(),
                conflict_policy.clone(),
                tx.clone(),
                cancel_rx,
                dec_rx,
                src_driver,
                dst_driver,
            ));

            // Store job in state
            {
                let mut s = state.lock().await;
                s.job_store.jobs.insert(job_id.clone(), FileOpJob {
                    job_id: job_id.clone(),
                    op_type,
                    source_panel: source_panel_id,
                    dest_panel: dest_panel_id,
                    dest_path,
                    files,
                    conflict_policy,
                    state: JobState::Running,
                    progress: Default::default(),
                    decision: None,
                    error: None,
                    background,
                    handle,
                    cancel_tx,
                });
            }
        }

        "ProvideJobDecision" => {
            let job_id = args.get("jobId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let decision = args.get("decision").and_then(|v| v.as_str()).unwrap_or("skip").to_string();

            if let Some(sender) = decision_senders.get(&job_id) {
                let _ = sender.send(decision).await;
            }
        }

        "CancelJob" => {
            let job_id = args.get("jobId").and_then(|v| v.as_str()).unwrap_or("").to_string();

            let mut s = state.lock().await;
            if let Some(job) = s.job_store.jobs.get_mut(&job_id) {
                let _ = job.cancel_tx.send(true);
                job.state = JobState::Cancelled;
            }
            drop(s);
            decision_senders.remove(&job_id);
        }

        "ViewFile" => {
            let panel_id  = args.get("panelId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let dir_path  = args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let source_id = args.get("sourceId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let filename  = args.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();

            let full_path = format!("{}/{}", dir_path.trim_end_matches('/'), filename);
            let label = format!("viewer-{}", uuid::Uuid::new_v4());
            let qs = format!(
                "?viewer=1&path={}&source={}&name={}&label={}&panel={}",
                url_encode(&full_path),
                url_encode(&source_id),
                url_encode(&filename),
                url_encode(&label),
                url_encode(&panel_id),
            );

            let _ = tauri::WebviewWindowBuilder::new(
                app_handle,
                label,
                tauri::WebviewUrl::App(qs.into()),
            )
            .title(format!("{filename} — fbro viewer"))
            .inner_size(900.0, 700.0)
            .resizable(true)
            .build();
        }

        "EditFile" => {
            let dir_path  = args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let filename  = args.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let full_path = format!("{}/{}", dir_path.trim_end_matches('/'), filename);

            let result = {
                let s = state.lock().await;
                s.platform.open_in_editor(&full_path)
            };

            match result {
                Ok(_) => {}
                Err(e) => {
                    let _ = tx.send(ServerMessage::Error {
                        timestamp: now(),
                        code: "NO_EDITOR".to_string(),
                        message: e,
                        recoverable: true,
                        extra: Some(serde_json::json!({ "path": full_path })),
                    }).await;
                }
            }
        }

        "RenameFile" => {
            let source_panel = args.get("sourcePanel").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let source_path  = args.get("sourcePath").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let dest_name    = args.get("destName").and_then(|v| v.as_str()).unwrap_or("").to_string();

            // Rename only — no path separators allowed.
            if dest_name.contains('/') || dest_name.contains('\\') {
                let _ = tx.send(ServerMessage::Error {
                    timestamp: now(),
                    code: "RENAME_FAILED".to_string(),
                    message: "Rename cannot change directories — use Move (F6) for that.".to_string(),
                    recoverable: true,
                    extra: None,
                }).await;
                return;
            }
            let source_dir = std::path::Path::new(&source_path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            let dest_path = format!("{}/{}", source_dir.trim_end_matches('/'), dest_name);

            let driver = {
                let s = state.lock().await;
                s.panels.get(&source_panel)
                    .and_then(|p| s.drivers.get(&p.source_id).cloned())
            };

            if let Some(driver) = driver {
                let src = source_path.clone();
                let dst = dest_path.clone();
                let result = tokio::task::spawn_blocking(move || driver.try_rename(&src, &dst)).await;
                match result {
                    Ok(Ok(true)) => {
                        let _ = tx.send(ServerMessage::RenameOk {
                            timestamp: now(),
                            panel_id: source_panel.clone(),
                        }).await;
                        rescan_panel(&source_panel, state, &tx, scan_handles, Some(dest_name.clone())).await;
                    }
                    Ok(Ok(false)) => {
                        let _ = tx.send(ServerMessage::Error {
                            timestamp: now(),
                            code: "RENAME_FAILED".to_string(),
                            message: "Cannot rename across filesystems — use Move (F6) instead.".to_string(),
                            recoverable: true,
                            extra: None,
                        }).await;
                    }
                    Ok(Err(e)) => {
                        let _ = tx.send(ServerMessage::Error {
                            timestamp: now(),
                            code: "RENAME_FAILED".to_string(),
                            message: e.to_string(),
                            recoverable: true,
                            extra: None,
                        }).await;
                    }
                    Err(_) => {}
                }
            }
        }

        "MakeDir" => {
            let source_panel = args.get("sourcePanel").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let dir_path     = args.get("dirPath").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let name         = args.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();

            let full_path = format!("{}/{}", dir_path.trim_end_matches('/'), name);

            let driver = {
                let s = state.lock().await;
                s.panels.get(&source_panel)
                    .and_then(|p| s.drivers.get(&p.source_id).cloned())
            };

            if let Some(driver) = driver {
                let p = full_path.clone();
                let result = tokio::task::spawn_blocking(move || driver.create_dir_all(&p)).await;
                match result {
                    Ok(Ok(())) => {
                        let _ = tx.send(ServerMessage::MkdirOk {
                            timestamp: now(),
                            panel_id: source_panel.clone(),
                        }).await;
                        rescan_panel(&source_panel, state, &tx, scan_handles, Some(name)).await;
                    }
                    Ok(Err(e)) => {
                        let _ = tx.send(ServerMessage::Error {
                            timestamp: now(),
                            code: "MKDIR_FAILED".to_string(),
                            message: e.to_string(),
                            recoverable: true,
                            extra: None,
                        }).await;
                    }
                    Err(_) => {}
                }
            }
        }

        "CloseViewerWindow" => {
            let label = args.get("label").and_then(|v| v.as_str()).unwrap_or("").to_string();
            use tauri::Manager;
            if let Some(win) = app_handle.get_webview_window(&label) {
                let _ = win.close();
            }
        }

        "ViewerNavigate" => {
            let panel_id  = args.get("panelId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let direction = args.get("direction").and_then(|v| v.as_str()).unwrap_or("next").to_string();

            let nav_result = {
                let mut s = state.lock().await;
                if let Some(panel) = s.panels.get_mut(&panel_id) {
                    let new_cursor = if direction == "prev" {
                        if panel.cursor == 0 { 0 } else { panel.cursor - 1 }
                    } else {
                        (panel.cursor + 1).min(panel.entries.len().saturating_sub(1))
                    };
                    panel.cursor = new_cursor;
                    let source_id = panel.source_id.clone();
                    let dir_path = panel.path.clone();
                    let entry_info = panel.entries.get(new_cursor).map(|e| {
                        (e.name.clone(), e.kind.clone())
                    });
                    Some((new_cursor, source_id, dir_path, entry_info))
                } else {
                    None
                }
            };

            if let Some((new_cursor, source_id, dir_path, entry_info)) = nav_result {
                if let Some((name, kind)) = entry_info {
                    if kind == "file" {
                        let full_path = format!("{}/{}", dir_path.trim_end_matches('/'), name);
                        let _ = tx.send(ServerMessage::ViewerFileChanged {
                            timestamp: now(),
                            path: full_path,
                            filename: name,
                            source_id,
                            panel_id: panel_id.clone(),
                        }).await;
                    } else {
                        let _ = tx.send(ServerMessage::Error {
                            timestamp: now(),
                            code: "NOT_A_FILE".to_string(),
                            message: format!("{name} is a {kind}, not a file"),
                            recoverable: true,
                            extra: None,
                        }).await;
                    }
                }
                // Broadcast cursor update to all GUI clients
                let cursor_patch = ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id),
                    version: 0,
                    patch: serde_json::json!({ "cursor": new_cursor }),
                };
                let s = state.lock().await;
                for gui_tx in &s.gui_senders {
                    let _ = gui_tx.send(cursor_patch.clone()).await;
                }
            }
        }

        "ReadFileChunk" => {
            let path      = args.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let source_id = args.get("sourceId").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let offset    = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
            let size      = args.get("size").and_then(|v| v.as_u64()).unwrap_or(4096) as usize;

            let (driver, file_size) = {
                let s = state.lock().await;
                let drv = s.drivers.get(&source_id).cloned();
                let sz  = drv.as_ref().and_then(|d| d.file_size(&path));
                (drv, sz)
            };

            if let Some(driver) = driver {
                let path_clone = path.clone();
                let result = tokio::task::spawn_blocking(move || {
                    driver.read_chunk(&path_clone, offset, size)
                }).await;

                match result {
                    Ok(Ok(bytes)) => {
                        let eof = bytes.len() < size
                            || file_size.map_or(false, |fs| offset + bytes.len() as u64 >= fs);
                        let data: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                        let _ = tx.send(ServerMessage::FileChunk {
                            timestamp: now(),
                            path,
                            offset,
                            data,
                            eof,
                            file_size,
                        }).await;
                    }
                    Ok(Err(e)) => {
                        let _ = tx.send(ServerMessage::Error {
                            timestamp: now(),
                            code: "READ_CHUNK_FAILED".to_string(),
                            message: e.to_string(),
                            recoverable: true,
                            extra: None,
                        }).await;
                    }
                    Err(_) => {}
                }
            }
        }

        "Quit" => {
            let active_jobs = {
                let s = state.lock().await;
                s.job_store.jobs.values().filter(|j| {
                    j.state == JobState::Running || j.state == JobState::AwaitingDecision
                }).count()
            };
            if active_jobs == 0 {
                save_session(state).await;
                std::process::exit(0);
            } else {
                let _ = tx.send(ServerMessage::QuitConfirmRequired {
                    timestamp: now(),
                    active_jobs,
                }).await;
            }
        }

        "ConfirmQuit" => {
            save_session(state).await;
            std::process::exit(0);
        }

        unknown => {
            eprintln!("[fbro] Unknown intent: {unknown}");
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn navigate_to(
    panel_id: &str,
    new_path: &str,
    cursor_hint: Option<String>,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
) {
    // Abort any running scan for this panel
    if let Some(h) = scan_handles.remove(panel_id) {
        h.abort();
    }

    let (sort, filter, version, driver) = {
        let mut s = state.lock().await;
        let (source_id, sort, filter, version) = if let Some(panel) = s.panels.get_mut(panel_id) {
            let sid = panel.source_id.clone();
            panel.path = new_path.to_string();
            panel.entries.clear();
            panel.cursor = 0;
            panel.selection.clear();
            panel.status = PanelStatus::Loading;
            panel.version += 1;
            (sid, panel.sort.clone(), panel.filter.clone(), panel.version)
        } else {
            return;
        };
        let driver = s.drivers.get(&source_id).cloned().unwrap();
        (sort, filter, version, driver)
    };

    // Inform FE of the path change + cleared entries
    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id.to_string()),
            version,
            patch: serde_json::json!({
                "path": new_path,
                "entries": [],
                "cursor": 0,
                "selection": [],
                "status": "loading"
            }),
        })
        .await;

    let h = spawn_scan(panel_id.to_string(), new_path.to_string(), sort, filter, version, tx.clone(), driver, cursor_hint);
    scan_handles.insert(panel_id.to_string(), h);
}

async fn rescan_panel(
    panel_id: &str,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
    cursor_hint: Option<String>,
) {
    if let Some(h) = scan_handles.remove(panel_id) {
        h.abort();
    }
    let (path, sort, filter, version, driver, source_id) = {
        let mut s = state.lock().await;
        let (source_id, path, sort, filter, version) = if let Some(panel) = s.panels.get_mut(panel_id) {
            let sid = panel.source_id.clone();
            panel.entries.clear();
            panel.status = PanelStatus::Loading;
            panel.version += 1;
            (sid, panel.path.clone(), panel.sort.clone(), panel.filter.clone(), panel.version)
        } else {
            return;
        };
        let driver = s.drivers.get(&source_id).cloned().unwrap();
        (path, sort, filter, version, driver, source_id)
    };
    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id.to_string()),
            version,
            patch: serde_json::json!({
                "path": path,
                "sourceId": source_id,
                "entries": [],
                "cursor": 0,
                "status": "loading",
                "sort": { "by": sort.by, "order": sort.order }
            }),
        })
        .await;
    let h = spawn_scan(panel_id.to_string(), path, sort, filter, version, tx.clone(), driver, cursor_hint);
    scan_handles.insert(panel_id.to_string(), h);
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'/' | b'~' | b':' => out.push(byte as char),
            b' ' => out.push_str("%20"),
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

async fn save_session(state: &SharedState) {
    let s = state.lock().await;
    let panels = s.panels.iter()
        .map(|(id, panel)| (id.clone(), PanelSession { path: panel.path.clone() }))
        .collect();
    crate::persistence::save(&SessionState { panels });
}

async fn resolve_panel_id(scope: &serde_json::Value, state: &SharedState) -> String {
    if scope == "focused-panel" || scope.as_str() == Some("focused-panel") {
        state.lock().await.focus.clone()
    } else if let Some(id) = scope.as_str() {
        id.to_string()
    } else {
        state.lock().await.focus.clone()
    }
}

fn spawn_scan(
    panel_id: String,
    path: String,
    sort: crate::ws::protocol::SortSpec,
    filter: Option<crate::ws::protocol::FilterSpec>,
    version_base: u64,
    tx: mpsc::Sender<ServerMessage>,
    driver: Arc<dyn VfsDriver>,
    cursor_hint: Option<String>,
) -> ScanHandle {
    let handle = tokio::spawn(run_scan(panel_id, path, sort, filter, version_base, tx, driver, cursor_hint));
    ScanHandle { handle }
}

async fn update_panel_from_patch(
    state: &SharedState,
    panel_id: &str,
    patch: &serde_json::Value,
    version: u64,
) {
    let mut s = state.lock().await;
    if let Some(panel) = s.panels.get_mut(panel_id) {
        if let Some(entries) = patch.get("appendEntries").and_then(|v| v.as_array()) {
            for e in entries {
                panel.entries.push(crate::ws::protocol::EntrySchema {
                    name: e["name"].as_str().unwrap_or("").to_string(),
                    kind: e["kind"].as_str().unwrap_or("file").to_string(),
                    size: e["size"].as_u64(),
                    mtime: e["mtime"].as_str().map(String::from),
                    permissions: e["permissions"].as_str().map(String::from),
                    icon_name: e["iconName"].as_str().map(String::from),
                });
            }
        }
        if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
            panel.status = match status {
                "ready" => PanelStatus::Ready,
                "error" => PanelStatus::Error(String::new()),
                _ => PanelStatus::Loading,
            };
        }
        if let Some(cursor) = patch.get("cursor").and_then(|v| v.as_u64()) {
            panel.cursor = cursor as usize;
        }
        if let Some(sel) = patch.get("selection").and_then(|v| v.as_array()) {
            panel.selection = sel.iter()
                .filter_map(|v| v.as_u64().map(|n| n as usize))
                .collect();
        }
        panel.version = version;
    }
}

async fn send_msg<S>(ws_tx: &mut S, msg: &ServerMessage) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Debug,
{
    match serde_json::to_string(msg) {
        Ok(text) => ws_tx.send(Message::Text(text.into())).await.map_err(|e| {
            eprintln!("[fbro] Send error: {e:?}");
        }),
        Err(e) => {
            eprintln!("[fbro] Serialization error: {e}");
            Err(())
        }
    }
}
