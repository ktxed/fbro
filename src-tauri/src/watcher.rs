use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::event::{EventKind, ModifyKind};
use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

use crate::platform::{VolumeInfo, VolumeWatcher};

/// Commands sent from the main loop to the watcher task.
#[allow(dead_code)]
pub enum WatcherCommand {
    /// Start watching a directory for a panel. Replaces any existing watch.
    Watch { panel_id: String, path: PathBuf },
    /// Stop watching for a panel.
    Unwatch { panel_id: String },
    /// Shut down the watcher task.
    Shutdown,
}

/// Actions the watcher task sends back to the main loop.
pub enum WatcherAction {
    /// Directory contents changed — rescan the panel.
    DirChanged { panel_id: String },
    /// The watched directory was deleted — navigate panel to safe location.
    DirDeleted { panel_id: String },
    /// Volume list changed.
    VolumesChanged { volumes: Vec<VolumeInfo> },
}

const DEBOUNCE_MS: u64 = 200;
const VOLUME_POLL_SECS: u64 = 2;

pub fn spawn_watcher(
    volume_watcher: Arc<dyn VolumeWatcher>,
    cmd_rx: mpsc::Receiver<WatcherCommand>,
    action_tx: mpsc::Sender<WatcherAction>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(watcher_task(volume_watcher, cmd_rx, action_tx))
}

// ── Main watcher task ───────────────────────────────────────────────────────

async fn watcher_task(
    volume_watcher: Arc<dyn VolumeWatcher>,
    mut cmd_rx: mpsc::Receiver<WatcherCommand>,
    action_tx: mpsc::Sender<WatcherAction>,
) {
    // Sync→async bridge: notify's callback is sync, so we use std::sync::mpsc
    // and a bridging task to forward events into a tokio channel.
    let (sync_tx, sync_rx) = std::sync::mpsc::channel::<notify::Event>();
    let (notify_tx, mut notify_rx) = mpsc::channel::<notify::Event>(256);

    let bridge_notify_tx = notify_tx.clone();
    let bridge_handle = tokio::task::spawn_blocking(move || {
        while let Ok(event) = sync_rx.recv() {
            if bridge_notify_tx.blocking_send(event).is_err() {
                break;
            }
        }
    });

    // Create the filesystem watcher.
    let mut watcher =
        match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = sync_tx.send(event);
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[fbro] Failed to create filesystem watcher: {e}");
                return;
            }
        };

    // panel_id → watched path
    let mut watched: HashMap<String, PathBuf> = HashMap::new();
    // path → list of panel_ids watching it
    let mut path_panels: HashMap<PathBuf, Vec<String>> = HashMap::new();
    // Debounce: path → latest event time
    let mut pending: HashMap<PathBuf, Instant> = HashMap::new();

    // Volume change signals from the platform-specific VolumeWatcher
    let mut volume_signal_rx = volume_watcher.subscribe().await;

    // Poll fallback: covers platforms with no signal source
    let mut volume_interval = tokio::time::interval(Duration::from_secs(VOLUME_POLL_SECS));
    volume_interval.tick().await; // consume the immediate first tick
    let mut prev_volumes: Vec<(String, bool, String)> = volume_watcher
        .list_volumes()
        .await
        .iter()
        .map(|v| (v.id.clone(), v.mounted, v.root_path.clone()))
        .collect();
    prev_volumes.sort();

    loop {
        let next_debounce = pending
            .values()
            .map(|t| *t + Duration::from_millis(DEBOUNCE_MS))
            .min();

        tokio::select! {
            // ── Commands from the main loop ─────────────────────────────
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(WatcherCommand::Watch { panel_id, path }) => {
                        if let Some(old_path) = watched.remove(&panel_id) {
                            if old_path != path {
                                remove_panel_from_path(&mut path_panels, &old_path, &panel_id);
                                if !path_panels.contains_key(&old_path) {
                                    let _ = watcher.unwatch(&old_path);
                                }
                            }
                        }
                        let needs_watch = !path_panels.contains_key(&path);
                        path_panels.entry(path.clone()).or_default().push(panel_id.clone());
                        watched.insert(panel_id, path.clone());
                        if needs_watch {
                            if let Err(e) = watcher.watch(&path, RecursiveMode::NonRecursive) {
                                eprintln!("[fbro] watch error for {}: {e}", path.display());
                            }
                        }
                    }
                    Some(WatcherCommand::Unwatch { panel_id }) => {
                        if let Some(old_path) = watched.remove(&panel_id) {
                            remove_panel_from_path(&mut path_panels, &old_path, &panel_id);
                            if !path_panels.contains_key(&old_path) {
                                let _ = watcher.unwatch(&old_path);
                            }
                        }
                    }
                    Some(WatcherCommand::Shutdown) | None => break,
                }
            }

            // ── Raw notify events ───────────────────────────────────────
            Some(event) = notify_rx.recv() => {
                // Filter out events that don't affect directory listing.
                // Access events (atime) and metadata-only Modify events (atime/mtime
                // updates from stat-walking the directory) would otherwise cause
                // phantom rescans every time the panel itself reads the directory.
                let affects_listing = match event.kind {
                    EventKind::Access(_) => false,
                    EventKind::Modify(ModifyKind::Metadata(_)) => false,
                    EventKind::Modify(ModifyKind::Other) => false,
                    EventKind::Other => false,
                    _ => true,
                };
                if !affects_listing {
                    continue;
                }
                for event_path in &event.paths {
                    let dir = if path_panels.contains_key(event_path) {
                        event_path.clone()
                    } else if let Some(parent) = event_path.parent() {
                        parent.to_path_buf()
                    } else {
                        continue;
                    };
                    if path_panels.contains_key(&dir) {
                        pending.insert(dir, Instant::now());
                    }
                }
            }

            // ── Debounce timer fires ────────────────────────────────────
            _ = async {
                match next_debounce {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let now = Instant::now();
                let threshold = now - Duration::from_millis(DEBOUNCE_MS);
                let ready: Vec<PathBuf> = pending
                    .iter()
                    .filter(|(_, t)| **t <= threshold)
                    .map(|(p, _)| p.clone())
                    .collect();

                for path in ready {
                    pending.remove(&path);
                    if let Some(panels) = path_panels.get(&path) {
                        let dir_exists = Path::new(&path).is_dir();
                        for pid in panels {
                            let action = if dir_exists {
                                WatcherAction::DirChanged { panel_id: pid.clone() }
                            } else {
                                WatcherAction::DirDeleted { panel_id: pid.clone() }
                            };
                            let _ = action_tx.send(action).await;
                        }
                    }
                }
            }

            // ── Volume change signal from VolumeWatcher ─────────────────
            Some(()) = volume_signal_rx.recv() => {
                check_volumes(&volume_watcher, &mut prev_volumes, &action_tx).await;
            }

            // ── Volume poll fallback ────────────────────────────────────
            _ = volume_interval.tick() => {
                check_volumes(&volume_watcher, &mut prev_volumes, &action_tx).await;
            }
        }
    }

    bridge_handle.abort();
}

async fn check_volumes(
    volume_watcher: &Arc<dyn VolumeWatcher>,
    prev_volumes: &mut Vec<(String, bool, String)>,
    action_tx: &mpsc::Sender<WatcherAction>,
) {
    let vols = volume_watcher.list_volumes().await;
    let mut current: Vec<(String, bool, String)> = vols
        .iter()
        .map(|v| (v.id.clone(), v.mounted, v.root_path.clone()))
        .collect();
    current.sort();
    if current != *prev_volumes {
        *prev_volumes = current;
        let _ = action_tx
            .send(WatcherAction::VolumesChanged { volumes: vols })
            .await;
    }
}

fn remove_panel_from_path(
    path_panels: &mut HashMap<PathBuf, Vec<String>>,
    path: &Path,
    panel_id: &str,
) {
    if let Some(panels) = path_panels.get_mut(path) {
        panels.retain(|p| p != panel_id);
        if panels.is_empty() {
            path_panels.remove(path);
        }
    }
}
