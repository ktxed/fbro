use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, Notify};
use tokio_tungstenite::tungstenite::Message;

use crate::jobs::dirsize::DirSizeQueue;
use crate::jobs::viewer_prefetch::ViewerPrefetchQueue;
use crate::jobs::{DecisionResponse, JobState, ScanHandle};
use crate::state::{CliOpenRequest, PanelStatus, SharedState};
use crate::watcher::{WatcherAction, WatcherCommand};
use crate::ws::protocol::{now, ClientMessage, ServerMessage};
use crate::ws::runtime::{
    active_job_count, build_state_snapshot, handle_volumes_changed, navigate_up_safe,
    quiet_rescan_panel, rescan_panel, save_session, selected_names_for, update_panel_from_patch,
    IntentContext,
};

const DEFAULT_WS_ADDR: &str = "127.0.0.1:7878";
const CHANNEL_CAP: usize = 512;

pub async fn start(
    state: SharedState,
    app_handle: tauri::AppHandle,
    quit_notify: Arc<Notify>,
    viewer_prefetch: Arc<ViewerPrefetchQueue>,
) {
    let ws_addr = std::env::var("FBRO_WS_BIND").unwrap_or_else(|_| DEFAULT_WS_ADDR.to_string());
    let listener = match TcpListener::bind(&ws_addr).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("[fbro] Failed to bind WebSocket server on {ws_addr}: {error}");
            return;
        }
    };
    eprintln!("[fbro] WebSocket server listening on ws://{ws_addr}");

    let dir_size_queue = Arc::new(DirSizeQueue::new());
    let platform = state.lock().await.platform.clone();
    let volume_watcher = crate::platform::create_volume_watcher(platform).await;
    // AppState is constructed synchronously from the platform fallback. Before
    // exposing a snapshot, replace that temporary list with the same canonical
    // inventory used for every later hot-plug update.
    let initial_volumes = volume_watcher.list_volumes().await;
    state
        .lock()
        .await
        .reconcile_initial_volumes(initial_volumes);
    let (cli_open_notify_tx, _) = broadcast::channel::<()>(16);

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let state = Arc::clone(&state);
                let app_handle = app_handle.clone();
                let quit_notify = Arc::clone(&quit_notify);
                let dir_size_queue = Arc::clone(&dir_size_queue);
                let viewer_prefetch = Arc::clone(&viewer_prefetch);
                let volume_watcher = Arc::clone(&volume_watcher);
                let cli_open_notify_tx = cli_open_notify_tx.clone();
                let cli_open_notify_rx = cli_open_notify_tx.subscribe();
                tokio::spawn(handle_connection(
                    stream,
                    addr,
                    state,
                    app_handle,
                    quit_notify,
                    dir_size_queue,
                    viewer_prefetch,
                    volume_watcher,
                    cli_open_notify_tx,
                    cli_open_notify_rx,
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
    viewer_prefetch: Arc<ViewerPrefetchQueue>,
    volume_watcher: Arc<dyn crate::platform::VolumeWatcher>,
    cli_open_notify_tx: broadcast::Sender<()>,
    mut cli_open_notify_rx: broadcast::Receiver<()>,
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
    // Test/automation observers need the same state and intent surface as the
    // GUI, but must not start another watcher for the shared panel state.
    let is_observer = hello_client.kind == "observer";
    let is_state_client = is_gui || is_observer;
    let session_id = uuid::Uuid::new_v4().to_string();

    if is_gui {
        // Embedded terminals survive a frontend reload. Point their reader
        // threads at this connection before the snapshot mounts new xterms;
        // the ensuing resize then redraws full-screen programs such as nano.
        let pty_manager = state.lock().await.pty_manager.clone();
        pty_manager.rebind_output(tx.clone());
    }

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

    // Subscribe before capturing the initial GUI snapshot. The service lock
    // captures the notification history and its delivery cursor atomically.
    let notifications = state.lock().await.notifications.clone();
    let (notification_snapshot, mut notification_rx) = if is_state_client {
        notifications.subscribe()
    } else {
        // Viewer windows have no store or toasts, so they must not accumulate
        // an unread application-lifetime subscriber queue.
        let (_, receiver) = mpsc::unbounded_channel();
        (notifications.snapshot(), receiver)
    };
    let mut notification_delivered_version = notification_snapshot.version;

    let (snapshot, panel_ids, is_reconnect) = if is_gui {
        let (panel_ids, is_reconnect) = {
            let mut state_guard = state.lock().await;
            let reconnect = state_guard.has_gui_connected;
            if !reconnect {
                // First ever connection: clear entries so the client starts
                // from a clean Loading state while the initial scan runs.
                for panel in state_guard.panels.values_mut() {
                    panel.entries.clear();
                    panel.status = PanelStatus::Loading;
                    panel.calculating_size.clear();
                    panel.version += 1;
                }
            }
            let ids = state_guard.panels.keys().cloned().collect::<Vec<_>>();
            (ids, reconnect)
        };
        let dir_size_tasks = dir_size_queue.snapshot().await;
        let snapshot = {
            let state_guard = state.lock().await;
            build_state_snapshot(&state_guard, dir_size_tasks)
        };
        (Some(snapshot), panel_ids, is_reconnect)
    } else if is_observer {
        let dir_size_tasks = dir_size_queue.snapshot().await;
        let snapshot = {
            let state_guard = state.lock().await;
            build_state_snapshot(&state_guard, dir_size_tasks)
        };
        (Some(snapshot), vec![], false)
    } else {
        (None, vec![], false)
    };

    if let Some(mut snapshot) = snapshot {
        if let ServerMessage::StateSnapshot { domains, .. } = &mut snapshot {
            domains.notifications = notification_snapshot;
        }
        let _ = send_msg(&mut ws_tx, &snapshot).await;
    } else {
        // Non-GUI clients (viewers) don't get the full snapshot but still need
        // display settings so they render consistently with the main window.
        let (font_size, color_scheme, enable_heic2any) = {
            let state_guard = state.lock().await;
            (
                state_guard.font_size,
                state_guard.settings.general.color_scheme.clone(),
                state_guard.settings.general.enable_heic2any,
            )
        };
        let _ = send_msg(
            &mut ws_tx,
            &ServerMessage::StatePatch {
                timestamp: now(),
                domain: "settings".to_string(),
                entity_id: None,
                version: 0,
                patch: serde_json::json!({
                    "fontSize": font_size,
                    "general": {
                        "colorScheme": color_scheme,
                        "enableHeic2any": enable_heic2any,
                    },
                }),
            },
        )
        .await;
    }

    let mut scan_handles: HashMap<String, ScanHandle> = HashMap::new();
    let mut decision_senders: HashMap<String, mpsc::Sender<DecisionResponse>> = HashMap::new();
    let (watcher_cmd_tx, watcher_cmd_rx) = mpsc::channel::<WatcherCommand>(64);
    let (watcher_action_tx, mut watcher_action_rx) = mpsc::channel::<WatcherAction>(64);

    if is_reconnect {
        // Reconnection: keep existing entries visible and refresh quietly
        // (atomic replace, no Loading flash).
        for panel_id in &panel_ids {
            quiet_rescan_panel(
                panel_id,
                &state,
                &tx,
                &mut scan_handles,
                None,
                &dir_size_queue,
                &watcher_cmd_tx,
            )
            .await;
        }
    } else {
        // First connection: incremental scan so entries appear progressively.
        for panel_id in &panel_ids {
            let (source_id, panel_epoch, path, sort, filter, show_hidden, version_base, driver) = {
                let mut state_guard = state.lock().await;
                let Some(panel) = state_guard.panels.get(panel_id) else {
                    continue;
                };
                let source_id = panel.source_id.clone();
                let panel_epoch = panel.panel_epoch;
                let path = panel.path.clone();
                let current_sort = panel.sort.clone();
                let filter = panel.filter.clone();
                let show_hidden = panel.show_hidden;
                let version = panel.version;
                let Some(driver) = state_guard.drivers.get(&source_id).cloned() else {
                    // Driver missing (e.g. a search source was ejected while
                    // the client was disconnected) — skip the initial scan for
                    // this panel; it will be rescanned when navigated.
                    continue;
                };
                let sort = state_guard
                    .apply_location_settings(panel_id, driver.is_local())
                    .unwrap_or(current_sort);
                (
                    source_id,
                    panel_epoch,
                    path,
                    sort,
                    filter,
                    show_hidden,
                    version,
                    driver,
                )
            };

            let handle = ScanHandle {
                handle: tokio::spawn(crate::jobs::scan::run_scan(
                    panel_id.clone(),
                    source_id,
                    panel_epoch,
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
                    watcher_cmd_tx.clone(),
                    true,
                )),
            };
            scan_handles.insert(panel_id.clone(), handle);
        }
    }

    let watcher_handle = if is_gui {
        for panel_id in &panel_ids {
            let watch_path = {
                let state_guard = state.lock().await;
                if let Some(panel) = state_guard.panels.get(panel_id) {
                    let is_local = state_guard
                        .drivers
                        .get(&panel.source_id)
                        .map(|d| d.is_local())
                        .unwrap_or(false);
                    if is_local {
                        Some(panel.path.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if let Some(path) = watch_path {
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

    {
        let mut state_guard = state.lock().await;
        if is_state_client {
            state_guard.gui_senders.push(tx.clone());
            if is_gui {
                state_guard.has_gui_connected = true;
            }
        } else {
            state_guard.viewer_senders.push(tx.clone());
        }
    }

    if is_gui {
        drain_cli_open_requests(
            &state,
            &tx,
            &mut scan_handles,
            &mut decision_senders,
            &app_handle,
            &dir_size_queue,
            &viewer_prefetch,
            &watcher_cmd_tx,
            &volume_watcher,
        )
        .await;
    }

    'connection: loop {
        tokio::select! {
            Some(update) = notification_rx.recv(), if is_state_client => {
                notification_delivered_version = update.version;
                if send_msg(&mut ws_tx, &update.into_server_message()).await.is_err() {
                    break;
                }
            }
            Some(msg) = rx.recv() => {
                let Some(mut msg) = prepare_connection_job_message(&state, msg).await else {
                    continue;
                };
                if is_state_client {
                    if let ServerMessage::StateSnapshot { domains, .. } = &mut msg {
                        let updates = notification_updates_before_snapshot(
                            &notifications, &mut domains.notifications, &mut notification_rx,
                            &mut notification_delivered_version,
                        ).await;
                        for update in updates {
                            if send_msg(&mut ws_tx, &update.into_server_message()).await.is_err() {
                                break 'connection;
                            }
                        }
                    }
                }
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
                                &watcher_cmd_tx,
                            ).await;
                        }
                        let _ = crate::ws::intents::jobs::maybe_start_next_queued_file_op(
                            job_id, &state, &tx,
                        )
                        .await;
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
                            &watcher_cmd_tx,
                        ).await;
                    }
                    WatcherAction::DirDeleted { panel_id, path } => {
                        navigate_up_safe(
                            &panel_id,
                            &path,
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
                    save_session(&state, &app_handle).await;
                    let journal = state.lock().await.remote_journal.clone();
                    journal.flush().await;
                    std::process::exit(0);
                } else {
                    let _ = tx.send(ServerMessage::QuitConfirmRequired {
                        timestamp: now(),
                        active_jobs,
                    }).await;
                }
            }

            Ok(_) = cli_open_notify_rx.recv(), if is_gui => {
                drain_cli_open_requests(
                    &state,
                    &tx,
                    &mut scan_handles,
                    &mut decision_senders,
                    &app_handle,
                    &dir_size_queue,
                    &viewer_prefetch,
                    &watcher_cmd_tx,
                    &volume_watcher,
                ).await;
            }

            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::Intent { name, scope, args, .. }) => {
                                if let Some(trace) = intent_trace(&name, &scope, &args) {
                                    eprintln!("{trace}");
                                }
                                if !is_state_client && name == "OpenPathInRightPanel" {
                                    queue_cli_open_request(
                                        &state,
                                        &cli_open_notify_tx,
                                        cli_open_request_from_args(&args),
                                    )
                                    .await;
                                    continue;
                                }
                                let mut ctx = IntentContext {
                                    state: &state,
                                    tx: &tx,
                                    scan_handles: &mut scan_handles,
                                    decision_senders: &mut decision_senders,
                                    app_handle: &app_handle,
                                    dir_size_queue: &dir_size_queue,
                                    viewer_prefetch: &viewer_prefetch,
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
                    Some(Ok(Message::Binary(bytes))) => {
                        // Binary WS frames carry terminal input only.
                        // Layout: [tag:u8 = 0x02][session_id:u32-le][payload...]
                        if bytes.len() >= 5 && bytes[0] == 0x02 {
                            let sid = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                            let payload = &bytes[5..];
                            let pty_manager = {
                                let guard = state.lock().await;
                                guard.pty_manager.clone()
                            };
                            if let Err(e) = pty_manager.write(sid, payload) {
                                eprintln!("[fbro] terminal write sid={sid}: {e}");
                            }
                        }
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

    let mut state_guard = state.lock().await;
    state_guard.gui_senders.retain(|sender| !sender.is_closed());
    state_guard
        .viewer_senders
        .retain(|sender| !sender.is_closed());
}

fn cli_open_request_from_args(args: &serde_json::Value) -> CliOpenRequest {
    CliOpenRequest {
        path: args
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(".")
            .to_string(),
        cwd: args
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        create_tab: args
            .get("createTab")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
    }
}

async fn queue_cli_open_request(
    state: &SharedState,
    notify: &broadcast::Sender<()>,
    request: CliOpenRequest,
) {
    {
        let mut state_guard = state.lock().await;
        state_guard.pending_cli_opens.push(request);
    }
    let _ = notify.send(());
}

async fn drain_cli_open_requests(
    state: &SharedState,
    tx: &mpsc::Sender<ServerMessage>,
    scan_handles: &mut HashMap<String, ScanHandle>,
    decision_senders: &mut HashMap<String, mpsc::Sender<DecisionResponse>>,
    app_handle: &tauri::AppHandle,
    dir_size_queue: &Arc<DirSizeQueue>,
    viewer_prefetch: &Arc<ViewerPrefetchQueue>,
    watcher_cmd_tx: &mpsc::Sender<WatcherCommand>,
    volume_watcher: &Arc<dyn crate::platform::VolumeWatcher>,
) {
    let requests = {
        let mut state_guard = state.lock().await;
        std::mem::take(&mut state_guard.pending_cli_opens)
    };

    for request in requests {
        let mut ctx = IntentContext {
            state,
            tx,
            scan_handles,
            decision_senders,
            app_handle,
            dir_size_queue,
            viewer_prefetch,
            watcher_cmd_tx,
            volume_watcher,
        };
        crate::ws::intents::dispatch(
            "OpenPathInRightPanel",
            &serde_json::Value::Null,
            &serde_json::json!({
                "path": request.path,
                "cwd": request.cwd,
                "createTab": request.create_tab,
            }),
            &mut ctx,
        )
        .await;
    }
}

async fn send_msg<S>(ws_tx: &mut S, msg: &ServerMessage) -> Result<(), ()>
where
    S: SinkExt<Message> + Unpin,
    <S as futures_util::Sink<Message>>::Error: std::fmt::Debug,
{
    // Terminal output is sent as a raw binary WS frame to bypass JSON+base64 overhead.
    // Frame layout: [tag:u8 = 0x01][session_id:u32-le][bytes...]
    if let ServerMessage::TerminalOutput { session_id, data } = msg {
        let mut frame = Vec::with_capacity(5 + data.len());
        frame.push(0x01u8);
        frame.extend_from_slice(&session_id.to_le_bytes());
        frame.extend_from_slice(data);
        return ws_tx
            .send(Message::Binary(frame.into()))
            .await
            .map_err(|error| {
                eprintln!("[fbro] Send error (binary): {error:?}");
            });
    }

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

pub(crate) async fn prepare_connection_job_message(
    state: &SharedState,
    mut message: ServerMessage,
) -> Option<ServerMessage> {
    if let ServerMessage::AppliedWorkerMessage { mut message } = message {
        // Cancellation can occur after producer application but before socket
        // delivery. Preserve its presentation rules without replaying state.
        if let ServerMessage::JobEvent { job_id, event, .. } = message.as_mut() {
            let guard = state.lock().await;
            if let Some(job) = guard.job_store.jobs.get(job_id) {
                if !reconcile_cancelled_job_event(&job.state, event) {
                    return None;
                }
            }
        }
        return Some(*message);
    }
    apply_job_event_to_state(state, &mut message)
        .await
        .then_some(message)
}

fn reconcile_cancelled_job_event(state: &JobState, event: &mut String) -> bool {
    if *state == JobState::Cancelled && event != "Cancelled" {
        if matches!(event.as_str(), "Completed" | "Failed") {
            // Acknowledge drained workers without reviving a cancelled job.
            *event = "Cancelled".to_string();
        } else {
            return false;
        }
    }
    true
}

fn intent_trace(name: &str, scope: &serde_json::Value, args: &serde_json::Value) -> Option<String> {
    if matches!(name, "PublishNotification" | "ClearNotifications") {
        return None;
    }
    Some(format!("[fbro] intent: {name} scope={scope} args={args}"))
}

pub(crate) async fn notification_updates_before_snapshot(
    service: &crate::notifications::NotificationService,
    snapshot: &mut crate::notifications::NotificationSnapshot,
    receiver: &mut mpsc::UnboundedReceiver<crate::notifications::NotificationUpdate>,
    delivered_version: &mut u64,
) -> Vec<crate::notifications::NotificationUpdate> {
    // A resnapshot may have waited in the normal job channel while live
    // notifications advanced. Capture a fresh boundary, then deliver every
    // subscribed update it covers before the frontend installs that version.
    // Initial connection snapshots bypass this path and never replay toasts.
    *snapshot = service.snapshot();
    let mut updates = Vec::new();
    while *delivered_version < snapshot.version {
        let Some(update) = receiver.recv().await else {
            break;
        };
        *delivered_version = update.version;
        updates.push(update);
    }
    updates
}

#[cfg(test)]
mod notification_trace_tests {
    use super::intent_trace;

    #[test]
    fn notifications_housekeeping_payloads_do_not_bypass_log_channel_routing() {
        let scope = serde_json::json!({});
        for channels in [serde_json::json!([]), serde_json::json!(["toast"])] {
            let args = serde_json::json!({"message": "private feedback", "channels": channels});
            assert!(intent_trace("PublishNotification", &scope, &args).is_none());
        }
        assert!(intent_trace("ClearNotifications", &scope, &scope).is_none());
        assert!(intent_trace("RenameFile", &scope, &scope).is_some());
    }
}

pub(crate) async fn apply_job_event_to_state(state: &SharedState, msg: &mut ServerMessage) -> bool {
    let generation_job_id = match msg {
        ServerMessage::HashGenerateBroResult { job_id, .. }
        | ServerMessage::HashGenerateBroFileResult { job_id, .. } => Some(job_id),
        _ => None,
    };
    if let Some(job_id) = generation_job_id {
        let mut state = state.lock().await;
        if let Some(job) = state.job_store.jobs.get_mut(job_id) {
            if let crate::jobs::JobKind::HashGenerateBro(meta) = &mut job.kind {
                match msg {
                    ServerMessage::HashGenerateBroResult { result, .. } => {
                        meta.result = Some(result.clone())
                    }
                    ServerMessage::HashGenerateBroFileResult { item, .. } => {
                        if let Some(row) = meta.result.as_mut().and_then(|result| {
                            result
                                .items
                                .iter_mut()
                                .find(|row| row.hash_path == item.hash_path)
                        }) {
                            *row = item.clone();
                        }
                    }
                    _ => {}
                }
            }
        }
        return true;
    }

    let hash_job_id = match msg {
        ServerMessage::HashCheckBroResult { job_id, .. }
        | ServerMessage::HashCheckBroFileResult { job_id, .. }
        | ServerMessage::HashCheckReportSaved { job_id, .. }
        | ServerMessage::HashCheckReportFailed { job_id, .. } => Some(job_id),
        _ => None,
    };
    if let Some(job_id) = hash_job_id {
        let mut guard = state.lock().await;
        if let Some(job) = guard.job_store.jobs.get_mut(job_id) {
            if let crate::jobs::JobKind::HashCheckBro(meta) = &mut job.kind {
                match msg {
                    ServerMessage::HashCheckBroResult { result, .. } => {
                        meta.result = Some(result.clone())
                    }
                    ServerMessage::HashCheckBroFileResult { item, .. } => {
                        if let Some(row) = meta.result.as_mut().and_then(|result| {
                            result
                                .items
                                .iter_mut()
                                .find(|row| row.hash_path == item.hash_path)
                        }) {
                            *row = item.clone();
                        }
                    }
                    ServerMessage::HashCheckReportSaved { path, .. } => {
                        meta.report_saved_path = Some(path.clone())
                    }
                    ServerMessage::HashCheckReportFailed { message, .. } => {
                        meta.report_error = Some(message.clone())
                    }
                    _ => {}
                }
            }
        }
        return true;
    }
    let ServerMessage::JobEvent {
        job_id,
        event,
        files,
        progress,
        search_progress: _,
        decision,
        error,
        ..
    } = msg
    else {
        return true;
    };

    let mut state_guard = state.lock().await;
    let is_fp_job = state_guard
        .job_store
        .jobs
        .get(job_id)
        .map(|job| matches!(job.kind, crate::jobs::JobKind::FpBro(_)))
        .unwrap_or(false);

    let notifications = state_guard.notifications.clone();
    let mut terminal_feedback = None;
    let mut job_feedback = None;

    let is_search_job = state_guard
        .job_store
        .jobs
        .get(job_id)
        .map(|job| matches!(job.kind, crate::jobs::JobKind::Search(_)))
        .unwrap_or(false);

    if let Some(job) = state_guard.job_store.jobs.get_mut(job_id) {
        if !reconcile_cancelled_job_event(&job.state, event) {
            return false;
        }
        if let crate::jobs::JobKind::HashGenerateBro(meta) = &mut job.kind {
            terminal_feedback = crate::notifications::hash_generation_feedback(meta, event);
        }
        if let Some(files) = files {
            match &mut job.kind {
                crate::jobs::JobKind::FileOp(meta) => {
                    meta.files = files
                        .iter()
                        .map(|file| crate::jobs::FileOpEntry {
                            name: file.name.clone(),
                            dest_name: file.dest_name.clone(),
                            source_path: file.source_path.clone().unwrap_or_default(),
                            size: file.size,
                            kind: file.kind.clone(),
                            mtime: None,
                            permissions: None,
                        })
                        .collect();
                }
                crate::jobs::JobKind::OutgoingTransfer(_) => {}
                _ => {}
            }
        }
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
                        kind: decision.kind.clone(),
                        source: decision.source.clone(),
                        destination: decision.destination.clone(),
                        message: decision.message.clone(),
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
            "FileSkipped" => {
                job.state = JobState::Running;
                job.paused = false;
                job.decision = None;
                job.error = None;
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
        job_feedback = crate::notifications::jobs::terminal_feedback(job, event, error.as_deref());
    }

    crate::journal::jobs::terminal(&mut state_guard, job_id, event, error.as_deref());

    if is_fp_job && matches!(event.as_str(), "Completed" | "Failed" | "Cancelled") {
        if state_guard.job_store.fp_running.as_deref() == Some(job_id.as_str()) {
            state_guard.job_store.fp_running = None;
        }
        state_guard.job_store.remove_from_fp_queue(job_id);
    }

    let search_store = if is_search_job && matches!(event.as_str(), "Completed" | "Failed" | "Cancelled") {
        state_guard.search_stores.get(&format!("search:{job_id}")).cloned()
    } else { None };
    drop(state_guard);
    // Results become authoritative before any terminal summary is delivered.
    if let Some(store) = search_store {
        store.lock().await.done = true;
    }
    if let Some(input) = terminal_feedback {
        if input.level == Some(crate::notifications::NotificationLevel::Error) {
            notifications.publish_os_once(&format!("hash-generation:{job_id}"), input, None);
        } else {
            notifications.publish(input);
        }
    }
    if let Some(feedback) = job_feedback {
        feedback.publish(&notifications);
    }
    true
}
