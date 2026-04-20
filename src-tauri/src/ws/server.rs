use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;

use crate::jobs::dirsize::DirSizeQueue;
use crate::jobs::{DecisionResponse, JobState, ScanHandle};
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
        let (path, sort, filter, show_hidden, version_base, driver) = {
            let state_guard = state.lock().await;
            let Some(panel) = state_guard.panels.get(panel_id) else {
                continue;
            };
            let Some(driver) = state_guard.drivers.get(&panel.source_id).cloned() else {
                // Driver missing (e.g. a search source was ejected while
                // the client was disconnected) — skip the initial scan for
                // this panel; it will be rescanned when navigated.
                continue;
            };
            (
                panel.path.clone(),
                panel.sort.clone(),
                panel.filter.clone(),
                panel.show_hidden,
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
                None,
                Arc::clone(&state),
                Arc::clone(&dir_size_queue),
                false,
                show_hidden,
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
                    apply_job_event_to_state(&state, &msg).await;
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
                        let _ = crate::ws::intents::bro::maybe_start_next_fp_job(&state, &tx).await;
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

async fn apply_job_event_to_state(state: &SharedState, msg: &ServerMessage) {
    let ServerMessage::JobEvent {
        job_id,
        event,
        progress,
        search_progress: _,
        decision,
        error,
        ..
    } = msg
    else {
        return;
    };

    let mut state_guard = state.lock().await;
    let is_fp_job = state_guard
        .job_store
        .jobs
        .get(job_id)
        .map(|job| matches!(job.kind, crate::jobs::JobKind::FpBro(_)))
        .unwrap_or(false);

    let is_search_job = state_guard
        .job_store
        .jobs
        .get(job_id)
        .map(|job| matches!(job.kind, crate::jobs::JobKind::Search(_)))
        .unwrap_or(false);

    if let Some(job) = state_guard.job_store.jobs.get_mut(job_id) {
        match event.as_str() {
            "Queued" => {
                job.state = JobState::Queued;
                job.paused = false;
                job.error = None;
                job.decision = None;
            }
            "Progress" => {
                job.state = JobState::Running;
                job.paused = false;
                if let Some(progress) = progress {
                    job.progress.total_files = progress.total_files;
                    job.progress.completed_files = progress.completed_files;
                    job.progress.current_file = progress.current_file.clone();
                    job.progress.status_text = progress.status_text.clone();
                    job.progress.current_file_bytes_total = progress.current_file_bytes_total;
                    job.progress.current_file_bytes_copied = progress.current_file_bytes_copied;
                    job.progress.total_bytes_total = progress.total_bytes_total;
                    job.progress.total_bytes_copied = progress.total_bytes_copied;
                    job.progress.current_file_percent = progress.current_file_percent;
                    job.progress.overall_percent = progress.overall_percent;
                }
                job.decision = None;
                job.error = None;
            }
            "DecisionRequired" => {
                job.state = JobState::AwaitingDecision;
                job.decision = decision
                    .as_ref()
                    .map(|decision| crate::jobs::ConflictDecision {
                        source: decision.source.clone(),
                        destination: decision.destination.clone(),
                        options: decision.options.clone(),
                        supports_apply_to_all: decision.supports_apply_to_all,
                    });
            }
            "Paused" => {
                job.paused = true;
            }
            "Resumed" => {
                job.state = JobState::Running;
                job.paused = false;
            }
            "Completed" => {
                job.state = JobState::Completed;
                job.paused = false;
                job.decision = None;
                job.error = None;
                job.handle = None;
            }
            "Failed" => {
                job.state = JobState::Failed;
                job.paused = false;
                job.decision = None;
                job.error = error.clone();
                job.handle = None;
            }
            "Cancelled" => {
                job.state = JobState::Cancelled;
                job.paused = false;
                job.decision = None;
                job.handle = None;
            }
            _ => {}
        }
    }

    if is_fp_job && matches!(event.as_str(), "Completed" | "Failed" | "Cancelled") {
        if state_guard.job_store.fp_running.as_deref() == Some(job_id.as_str()) {
            state_guard.job_store.fp_running = None;
        }
        state_guard.job_store.remove_from_fp_queue(job_id);
    }

    // For search jobs: mark the store as done on terminal events
    if is_search_job && matches!(event.as_str(), "Completed" | "Failed" | "Cancelled") {
        let source_id = format!("search:{job_id}");
        if let Some(store) = state_guard.search_stores.get(&source_id) {
            let store = store.clone();
            drop(state_guard);
            store.lock().await.done = true;
            return;
        }
    }
}
