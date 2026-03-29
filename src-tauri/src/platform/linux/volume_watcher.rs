use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::udisks2::UDisks2;
use crate::platform::{Platform, VolumeInfo, VolumeWatcher};

// ── UDisks2 volume watcher ─────────────────────────────────────────────────

/// Volume watcher backed by udisks2 D-Bus. Supports unmounted volume enumeration
/// and mounting. Signals arrive via D-Bus InterfacesAdded/Removed and
/// PropertiesChanged.
pub struct UDisks2Watcher {
    udisks2: UDisks2,
    platform: Arc<dyn Platform>,
}

#[async_trait::async_trait]
impl VolumeWatcher for UDisks2Watcher {
    async fn list_volumes(&self) -> Vec<VolumeInfo> {
        let mut vols = self.udisks2.list_block_devices().await;
        // Merge with /proc/mounts-based volumes: udisks2 may miss some (e.g. root, network mounts)
        let ids: HashSet<String> = vols.iter().map(|v| v.id.clone()).collect();
        for pv in self.platform.list_volumes() {
            if !ids.contains(&pv.id) {
                vols.push(pv);
            }
        }
        vols
    }

    async fn mount(&self, device_path: &str) -> Result<String, String> {
        self.udisks2.mount(device_path).await
    }

    async fn subscribe(&self) -> mpsc::Receiver<()> {
        self.udisks2.subscribe_signals().await
    }
}

/// Try to create a UDisks2-backed watcher. Returns None if udisks2 is not available.
pub async fn try_udisks2(platform: Arc<dyn Platform>) -> Option<UDisks2Watcher> {
    match UDisks2::new().await {
        Ok(ud) => Some(UDisks2Watcher { udisks2: ud, platform }),
        Err(_) => None,
    }
}

// ── /proc/mounts volume watcher ────────────────────────────────────────────

/// Linux fallback when udisks2 is not available. Uses Platform::list_volumes()
/// (reads /proc/mounts) and detects changes via poll() on /proc/mounts.
pub struct ProcMountsWatcher {
    platform: Arc<dyn Platform>,
}

impl ProcMountsWatcher {
    pub fn new(platform: Arc<dyn Platform>) -> Self {
        Self { platform }
    }
}

#[async_trait::async_trait]
impl VolumeWatcher for ProcMountsWatcher {
    async fn list_volumes(&self) -> Vec<VolumeInfo> {
        self.platform.list_volumes()
    }

    async fn mount(&self, _device_path: &str) -> Result<String, String> {
        Err("udisks2 is not available on this system".to_string())
    }

    async fn subscribe(&self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel(4);
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
                let ret = unsafe { libc::poll(&mut pfd as *mut _, 1, -1) };
                if ret < 0 {
                    break;
                }
                if pfd.revents & (libc::POLLPRI | libc::POLLERR) != 0 {
                    if tx.blocking_send(()).is_err() {
                        break;
                    }
                }
            }
        });
        rx
    }
}
