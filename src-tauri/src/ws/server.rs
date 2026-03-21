use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::jobs::file_ops::run_file_op;
use crate::jobs::scan::run_scan;
use crate::jobs::{ConflictPolicy, FileOpEntry, FileOpJob, FileOpType, JobState, ScanHandle};
use crate::state::{PanelStatus, SharedState};
use crate::vfs::local::LocalFsDriver;
use crate::ws::protocol::{
    now, ClientMessage, FileOpFileSchema, JobStartSchema, LayoutDomain, PanelSnapshot,
    ServerMessage, SourceCapabilities, SourceSchema, StateDomains,
};

const WS_ADDR: &str = "127.0.0.1:7878";
const CHANNEL_CAP: usize = 512;

pub async fn start(state: SharedState) {
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
                tokio::spawn(handle_connection(stream, addr, state));
            }
            Err(e) => eprintln!("[fbro] Accept error: {e}"),
        }
    }
}

async fn handle_connection(stream: TcpStream, addr: SocketAddr, state: SharedState) {
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
    let _hello = loop {
        match ws_rx.next().await {
            Some(Ok(Message::Text(text))) => {
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Hello { protocol_version, client }) => {
                        eprintln!("[fbro] Hello from {:?} (protocol v{})", client.name, protocol_version);
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

    let session_id = uuid::Uuid::new_v4().to_string();

    // ── Send Welcome ──────────────────────────────────────────────────────────
    let _ = send_msg(&mut ws_tx, &ServerMessage::Welcome {
        timestamp: now(),
        protocol_version: "0".to_string(),
        session_id: session_id.clone(),
    })
    .await;

    // ── Send initial StateSnapshot ─────────────────────────────────────────────
    let (snapshot, panel_ids) = {
        let s = state.lock().await;
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

        (snapshot, panel_ids)
    };

    let _ = send_msg(&mut ws_tx, &snapshot).await;

    // ── Spawn initial scan jobs ────────────────────────────────────────────────
    let mut scan_handles: HashMap<String, ScanHandle> = HashMap::new();
    let mut decision_senders: HashMap<String, mpsc::Sender<String>> = HashMap::new();

    for panel_id in &panel_ids {
        let (path, sort, filter, version_base) = {
            let s = state.lock().await;
            let p = s.panels.get(panel_id).unwrap();
            (p.path.clone(), p.sort.clone(), p.filter.clone(), p.version)
        };

        let h = spawn_scan(panel_id.clone(), path, sort, filter, version_base, tx.clone());
        scan_handles.insert(panel_id.clone(), h);
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
                    if event == "Completed" || event == "Cancelled" {
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
                            rescan_panel(&pid, &state, &tx, &mut scan_handles).await;
                        }
                    }
                }

                if send_msg(&mut ws_tx, &msg).await.is_err() {
                    break;
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
) {
    match name {
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
                        let full = LocalFsDriver::resolve_child(&panel.path, &entry.name);
                        (kind, full)
                    } else {
                        return;
                    }
                } else {
                    return;
                }
            };

            if entry_kind == "directory" {
                navigate_to(&panel_id, &new_path, state, tx, scan_handles).await;
            }
        }

        "NavigateHome" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let home = state.lock().await.platform.home_dir();
            navigate_to(&panel_id, &home, state, tx, scan_handles).await;
        }

        "NavigateUp" => {
            let panel_id = resolve_panel_id(scope, state).await;
            let current_path = {
                let s = state.lock().await;
                s.panels.get(&panel_id).map(|p| p.path.clone())
            };
            if let Some(path) = current_path {
                if let Some(parent) = LocalFsDriver::parent_path(&path) {
                    if parent != path {
                        navigate_to(&panel_id, &parent, state, tx, scan_handles).await;
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
            let (files, dest_path) = {
                let s = state.lock().await;
                let Some(src_panel) = s.panels.get(&source_panel_id) else { return };

                let indices: Vec<usize> = if src_panel.selection.is_empty() {
                    vec![src_panel.cursor]
                } else {
                    src_panel.selection.clone()
                };

                let files: Vec<FileOpEntry> = indices.iter().filter_map(|&idx| {
                    src_panel.entries.get(idx).map(|e| FileOpEntry {
                        name: e.name.clone(),
                        source_path: LocalFsDriver::resolve_child(&src_panel.path, &e.name),
                        size: e.size,
                        kind: e.kind.clone(),
                    })
                }).collect();

                let dest_path = if op_type != FileOpType::Delete {
                    dest_panel_id.as_ref().and_then(|id| s.panels.get(id)).map(|p| p.path.clone())
                } else {
                    None
                };

                (files, dest_path)
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

            let _ = tx.send(ServerMessage::JobStarted {
                timestamp: now(),
                job: JobStartSchema {
                    job_id: job_id.clone(),
                    op_type: op_type_str.to_string(),
                    files: file_schemas,
                    source_panel: source_panel_id.clone(),
                    dest_panel: dest_panel_id.clone(),
                    dest_path: dest_path.clone(),
                    total_files: files.len(),
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

        unknown => {
            eprintln!("[fbro] Unknown intent: {unknown}");
        }
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

async fn navigate_to(
    panel_id: &str,
    new_path: &str,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
) {
    // Abort any running scan for this panel
    if let Some(h) = scan_handles.remove(panel_id) {
        h.abort();
    }

    let (sort, filter, version) = {
        let mut s = state.lock().await;
        if let Some(panel) = s.panels.get_mut(panel_id) {
            panel.path = new_path.to_string();
            panel.entries.clear();
            panel.cursor = 0;
            panel.selection.clear();
            panel.status = PanelStatus::Loading;
            panel.version += 1;
            (panel.sort.clone(), panel.filter.clone(), panel.version)
        } else {
            return;
        }
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

    let h = spawn_scan(panel_id.to_string(), new_path.to_string(), sort, filter, version, tx.clone());
    scan_handles.insert(panel_id.to_string(), h);
}

async fn rescan_panel(
    panel_id: &str,
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
) {
    if let Some(h) = scan_handles.remove(panel_id) {
        h.abort();
    }
    let (path, sort, filter, version) = {
        let mut s = state.lock().await;
        if let Some(panel) = s.panels.get_mut(panel_id) {
            panel.entries.clear();
            panel.status = PanelStatus::Loading;
            panel.version += 1;
            (panel.path.clone(), panel.sort.clone(), panel.filter.clone(), panel.version)
        } else {
            return;
        }
    };
    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id.to_string()),
            version,
            patch: serde_json::json!({
                "entries": [],
                "status": "loading"
            }),
        })
        .await;
    let h = spawn_scan(panel_id.to_string(), path, sort, filter, version, tx.clone());
    scan_handles.insert(panel_id.to_string(), h);
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
) -> ScanHandle {
    let handle = tokio::spawn(run_scan(panel_id, path, sort, filter, version_base, tx));
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
