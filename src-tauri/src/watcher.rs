use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::time::{Duration, Instant};

use crate::platform::{Platform, VolumeInfo};
#[cfg(target_os = "linux")]
use crate::platform::linux::udisks2::UDisks2;

/// Commands sent from the main loop to the watcher task.
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
    platform: Arc<dyn Platform>,
    #[cfg(target_os = "linux")] udisks2: Option<Arc<UDisks2>>,
    cmd_rx: mpsc::Receiver<WatcherCommand>,
    action_tx: mpsc::Sender<WatcherAction>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(watcher_task(
        platform,
        #[cfg(target_os = "linux")]
        udisks2,
        cmd_rx,
        action_tx,
    ))
}

// ── Linux: instant mount-change notification via /proc/mounts poll() ────────

#[cfg(target_os = "linux")]
fn spawn_mount_watcher(mount_tx: mpsc::Sender<()>) -> tokio::task::JoinHandle<()> {
    tokio::task::spawn_blocking(move || {
        use std::os::unix::io::AsRawFd;
        let file = match std::fs::File::open("/proc/mounts") {
            Ok(f) => f,
            Err(e) => {
                eprintln!("[fbro] Cannot open /proc/mounts: {e}");
                return;
            }
        };
        let fd = file.as_raw_fd();
        loop {
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLPRI | libc::POLLERR,
                revents: 0,
            };
            // Block until mount table changes (timeout -1 = infinite)
            let ret = unsafe { libc::poll(&mut pfd as *mut _, 1, -1) };
            if ret < 0 {
                break; // error — stop watching
            }
            if pfd.revents & (libc::POLLPRI | libc::POLLERR) != 0 {
                if mount_tx.blocking_send(()).is_err() {
                    break; // receiver dropped
                }
            }
        }
    })
}

#[cfg(not(target_os = "linux"))]
fn spawn_mount_watcher(_mount_tx: mpsc::Sender<()>) -> tokio::task::JoinHandle<()> {
    // No-op on non-Linux — volume detection falls back to polling.
    tokio::spawn(std::future::pending())
}

// ── Main watcher task ───────────────────────────────────────────────────────

async fn watcher_task(
    platform: Arc<dyn Platform>,
    #[cfg(target_os = "linux")] udisks2: Option<Arc<UDisks2>>,
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
    let mut watcher = match notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
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

    // Mount change detection: udisks2 signals (Linux) > /proc/mounts poll > periodic fallback
    let (mount_tx, mut mount_rx) = mpsc::channel::<()>(4);
    let mut udisks2_rx: Option<mpsc::Receiver<()>> = None;

    #[cfg(target_os = "linux")]
    if let Some(ref ud) = udisks2 {
        udisks2_rx = Some(ud.subscribe_signals().await);
    }

    let udisks2_available = udisks2_rx.is_some();

    let mount_handle = if !udisks2_available {
        Some(spawn_mount_watcher(mount_tx))
    } else {
        None
    };

    // Poll fallback for non-Linux (also serves as a secondary check on Linux without udisks2)
    let mut volume_interval = tokio::time::interval(Duration::from_secs(VOLUME_POLL_SECS));
    volume_interval.tick().await; // consume the immediate first tick
    let mut prev_volumes: Vec<(String, bool, String)> = platform
        .list_volumes()
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

            // ── udisks2 D-Bus signal (Linux with udisks2) ────────────────
            Some(()) = async {
                match udisks2_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                #[cfg(target_os = "linux")]
                check_volumes_udisks2(&udisks2, &platform, &mut prev_volumes, &action_tx).await;
                #[cfg(not(target_os = "linux"))]
                { let _ = (&platform, &mut prev_volumes, &action_tx); }
            }

            // ── Instant mount change (Linux /proc/mounts, non-udisks2) ──
            Some(()) = mount_rx.recv() => {
                check_volumes(&platform, &mut prev_volumes, &action_tx).await;
            }

            // ── Volume poll fallback (all platforms) ────────────────────
            _ = volume_interval.tick(), if !udisks2_available => {
                check_volumes(&platform, &mut prev_volumes, &action_tx).await;
            }
        }
    }

    if let Some(h) = mount_handle {
        h.abort();
    }
    bridge_handle.abort();
}

async fn check_volumes(
    platform: &Arc<dyn Platform>,
    prev_volumes: &mut Vec<(String, bool, String)>,
    action_tx: &mpsc::Sender<WatcherAction>,
) {
    let vols = platform.list_volumes();
    let mut current: Vec<(String, bool, String)> = vols
        .iter()
        .map(|v| (v.id.clone(), v.mounted, v.root_path.clone()))
        .collect();
    current.sort();
    if current != *prev_volumes {
        *prev_volumes = current;
        let _ = action_tx.send(WatcherAction::VolumesChanged { volumes: vols }).await;
    }
}

#[cfg(target_os = "linux")]
async fn check_volumes_udisks2(
    udisks2: &Option<Arc<UDisks2>>,
    platform: &Arc<dyn Platform>,
    prev_volumes: &mut Vec<(String, bool, String)>,
    action_tx: &mpsc::Sender<WatcherAction>,
) {
    let vols = if let Some(ref ud) = udisks2 {
        let mut udisks_vols = ud.list_block_devices().await;
        // Merge with /proc/mounts-based volumes: udisks2 may miss some (e.g. network mounts)
        let udisks_ids: std::collections::HashSet<String> = udisks_vols.iter().map(|v| v.id.clone()).collect();
        let proc_vols = platform.list_volumes();
        for pv in proc_vols {
            // Add /proc/mounts volumes not covered by udisks2 (like root "/")
            if !udisks_ids.contains(&pv.id) {
                udisks_vols.push(pv);
            }
        }
        udisks_vols
    } else {
        platform.list_volumes()
    };

    let mut current: Vec<(String, bool, String)> = vols
        .iter()
        .map(|v| (v.id.clone(), v.mounted, v.root_path.clone()))
        .collect();
    current.sort();
    if current != *prev_volumes {
        *prev_volumes = current;
        let _ = action_tx.send(WatcherAction::VolumesChanged { volumes: vols }).await;
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
