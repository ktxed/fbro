use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;

use crate::jobs::dirsize::DirSizeQueue;
use crate::jobs::{DecisionResponse, ScanHandle};
use crate::state::{PanelStatus, SharedState};
use crate::watcher::{WatcherAction, WatcherCommand};
use crate::ws::protocol::{now, ClientMessage, ServerMessage};
use crate::ws::runtime::{
    active_job_count, build_state_snapshot, handle_volumes_changed, navigate_up_safe,
    quiet_rescan_panel, rescan_panel, save_session, selected_names_for, update_panel_from_patch,
    IntentContext,
};

const WS_ADDR: &str = "127.0.0.1:7878";
const CHANNEL_CAP: usize = 512;

pub async fn start(state: SharedState, app_handle: tauri::AppHandle, quit_notify: Arc<Notify>) {
    let listener = match TcpListener::bind(WS_ADDR).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("[fbro] Failed to bind WebSocket server on {WS_ADDR}: {error}");
            return;
        }
    };
    eprintln!("[fbro] WebSocket server listening on ws://{WS_ADDR}");

    let dir_size_queue = Arc::new(DirSizeQueue::new());
    let platform = state.lock().await.platform.clone();
    let volume_watcher = crate::platform::create_volume_watcher(platform).await;

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let state = Arc::clone(&state);
                let app_handle = app_handle.clone();
                let quit_notify = Arc::clone(&quit_notify);
                let dir_size_queue = Arc::clone(&dir_size_queue);
                let volume_watcher = Arc::clone(&volume_watcher);
                tokio::spawn(handle_connection(
                    stream,
                    addr,
                    state,
                    app_handle,
                    quit_notify,
                    dir_size_queue,
                    volume_watcher,
                ));
            }
            Err(error) => eprintln!("[fbro] Accept error: {error}"),
        }
    }
}

async fn handle_connection(
    stream: TcpStream,
    addr: SocketAddr,
    state: SharedState,
    app_handle: tauri::AppHandle,
    quit_notify: Arc<Notify>,
    dir_size_queue: Arc<DirSizeQueue>,
    volume_watcher: Arc<dyn crate::platform::VolumeWatcher>,
) {
    eprintln!("[fbro] Client connected: {addr}");

    let ws_stream = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws_stream) => ws_stream,
        Err(error) => {
            eprintln!("[fbro] WS handshake failed: {error}");
            return;
        }
    };

    let (mut ws_tx, mut ws_rx) = ws_stream.split();
    let (tx, mut rx) = mpsc::channel::<ServerMessage>(CHANNEL_CAP);

    let hello_client = loop {
        match ws_rx.next().await {
            Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientMessage>(&text) {
                Ok(ClientMessage::Hello {
                    protocol_version,
                    client,
                }) => {
                    eprintln!(
                        "[fbro] Hello from {:?} kind={:?} (protocol v{})",
                        client.name, client.kind, protocol_version
                    );
                    break client;
                }
                Ok(_) => eprintln!("[fbro] Expected Hello, got other intent"),
                Err(error) => eprintln!("[fbro] Parse error: {error}"),
            },
            Some(Ok(Message::Close(_))) | None => return,
            _ => {}
        }
    };

    let is_gui = hello_client.kind == "gui";
    let session_id = uuid::Uuid::new_v4().to_string();

    let file_server_port = state.lock().await.file_server_port;
    let _ = send_msg(
        &mut ws_tx,
        &ServerMessage::Welcome {
            timestamp: now(),
            protocol_version: "0".to_string(),
            session_id: session_id.clone(),
            file_server_port,
        },
    )
    .await;

    let (snapshot, panel_ids) = if is_gui {
        let panel_ids = {
            let mut state_guard = state.lock().await;
            for panel in state_guard.panels.values_mut() {
                panel.entries.clear();
                panel.status = PanelStatus::Loading;
                panel.calculating_size.clear();
                panel.version += 1;
            }
            state_guard.panels.keys().cloned().collect::<Vec<_>>()
        };
        let dir_size_tasks = dir_size_queue.snapshot().await;
        let snapshot = {
            let state_guard = state.lock().await;
            build_state_snapshot(&state_guard, dir_size_tasks)
        };
        (Some(snapshot), panel_ids)
    } else {
        (None, vec![])
    };

    if let Some(snapshot) = snapshot {
        let _ = send_msg(&mut ws_tx, &snapshot).await;
    }

    let mut scan_handles: HashMap<String, ScanHandle> = HashMap::new();
    let mut decision_senders: HashMap<String, mpsc::Sender<DecisionResponse>> = HashMap::new();

    for panel_id in &panel_ids {
        let (path, sort, filter, version_base, driver) = {
            let state_guard = state.lock().await;
            let panel = state_guard.panels.get(panel_id).unwrap();
            let driver = state_guard.drivers.get(&panel.source_id).cloned().unwrap();
            (
                panel.path.clone(),
                panel.sort.clone(),
                panel.filter.clone(),
                panel.version,
                driver,
            )
        };

        let handle = ScanHandle {
            handle: tokio::spawn(crate::jobs::scan::run_scan(
                panel_id.clone(),
                path,
                sort,
                filter,
                version_base,
                tx.clone(),
                driver,
                None,
                None,
                Arc::clone(&state),
                Arc::clone(&dir_size_queue),
                false,
            )),
        };
        scan_handles.insert(panel_id.clone(), handle);
    }

    let (watcher_cmd_tx, watcher_cmd_rx) = mpsc::channel::<WatcherCommand>(64);
    let (watcher_action_tx, mut watcher_action_rx) = mpsc::channel::<WatcherAction>(64);

    let watcher_handle = if is_gui {
        for panel_id in &panel_ids {
            let path = state
                .lock()
                .await
                .panels
                .get(panel_id)
                .map(|panel| panel.path.clone());
            if let Some(path) = path {
                let _ = watcher_cmd_tx
                    .send(WatcherCommand::Watch {
                        panel_id: panel_id.clone(),
                        path: PathBuf::from(&path),
                    })
                    .await;
            }
        }
        Some(crate::watcher::spawn_watcher(
            Arc::clone(&volume_watcher),
            watcher_cmd_rx,
            watcher_action_tx,
        ))
    } else {
        None
    };

    if is_gui {
        let mut state_guard = state.lock().await;
        state_guard.gui_senders.push(tx.clone());
    }

    loop {
        tokio::select! {
            Some(msg) = rx.recv() => {
                if let ServerMessage::StatePatch { ref domain, ref entity_id, ref patch, version, .. } = msg {
                    if domain == "panels" {
                        if let Some(panel_id) = entity_id {
                            update_panel_from_patch(&state, panel_id, patch, version).await;
                        }
                    }
                }

                if let ServerMessage::JobEvent { ref job_id, ref event, .. } = msg {
                    if event == "Completed" || event == "Cancelled" || event == "Failed" {
                        let panels_to_rescan = {
                            let state_guard = state.lock().await;
                            if let Some(job) = state_guard.job_store.jobs.get(job_id) {
                                let mut panels = vec![job.source_panel.clone()];
                                if let Some(dest_panel) = &job.dest_panel {
                                    panels.push(dest_panel.clone());
                                }
                                panels
                            } else {
                                vec![]
                            }
                        };
                        for panel_id in panels_to_rescan {
                            rescan_panel(
                                &panel_id,
                                &state,
                                &tx,
                                &mut scan_handles,
                                None,
                                None,
                                &dir_size_queue,
                            ).await;
                        }
                    }
                }

                if send_msg(&mut ws_tx, &msg).await.is_err() {
                    break;
                }
            }

            Some(action) = watcher_action_rx.recv() => {
                match action {
                    WatcherAction::DirChanged { panel_id } => {
                        let names = {
                            let state_guard = state.lock().await;
                            selected_names_for(&state_guard, &panel_id)
                        };
                        quiet_rescan_panel(
                            &panel_id,
                            &state,
                            &tx,
                            &mut scan_handles,
                            names,
                            &dir_size_queue,
                        ).await;
                    }
                    WatcherAction::DirDeleted { panel_id } => {
                        navigate_up_safe(
                            &panel_id,
                            &state,
                            &tx,
                            &mut scan_handles,
                            &watcher_cmd_tx,
                            &dir_size_queue,
                        ).await;
                    }
                    WatcherAction::VolumesChanged { volumes } => {
                        handle_volumes_changed(
                            volumes,
                            &state,
                            &tx,
                            &mut scan_handles,
                            &watcher_cmd_tx,
                            &dir_size_queue,
                        ).await;
                    }
                }
            }

            _ = quit_notify.notified(), if is_gui => {
                let active_jobs = active_job_count(&state).await;
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

            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::Intent { name, scope, args, .. }) => {
                                eprintln!("[fbro] intent: {name} scope={scope} args={args}");
                                let mut ctx = IntentContext {
                                    state: &state,
                                    tx: &tx,
                                    scan_handles: &mut scan_handles,
                                    decision_senders: &mut decision_senders,
                                    app_handle: &app_handle,
                                    dir_size_queue: &dir_size_queue,
                                    watcher_cmd_tx: &watcher_cmd_tx,
                                    volume_watcher: &volume_watcher,
                                };
                                crate::ws::intents::dispatch(&name, &scope, &args, &mut ctx).await;
                            }
                            Ok(ClientMessage::Hello { .. }) => {}
                            Err(error) => eprintln!("[fbro] Intent parse error: {error} — raw: {text}"),
                        }
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        let _ = ws_tx.send(Message::Pong(payload)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }

    eprintln!("[fbro] Client disconnected: {addr}");
    let _ = watcher_cmd_tx.send(WatcherCommand::Shutdown).await;
    if let Some(handle) = watcher_handle {
        handle.abort();
    }
    for handle in scan_handles.values() {
        handle.abort();
    }

    if is_gui {
        let mut state_guard = state.lock().await;
        state_guard.gui_senders.retain(|sender| !sender.is_closed());
    }
}

async fn send_msg<S>(ws_tx: &mut S, msg: &ServerMessage) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Debug,
{
    match serde_json::to_string(msg) {
        Ok(text) => ws_tx
            .send(Message::Text(text.into()))
            .await
            .map_err(|error| {
                eprintln!("[fbro] Send error: {error:?}");
            }),
        Err(error) => {
            eprintln!("[fbro] Serialization error: {error}");
            Err(())
        }
    }
}
