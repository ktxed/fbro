use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::mpsc;

use super::udisks2::UDisks2;
use crate::platform::{EjectAction, Platform, VolumeInfo, VolumeWatcher};

fn mount_path_key(path: &str) -> String {
    if path == "/" {
        path.to_string()
    } else {
        path.trim_end_matches('/').to_string()
    }
}

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
        let inventory = self.udisks2.list_block_devices().await;
        merge_with_platform_volumes(
            inventory.volumes,
            inventory.ignored_mount_paths,
            self.platform.list_volumes(),
        )
    }

    async fn mount(&self, device_path: &str) -> Result<String, String> {
        self.udisks2.mount(device_path).await
    }

    async fn eject(&self, device_path: &str, action: EjectAction) -> Result<(), String> {
        self.udisks2.eject(device_path, action).await
    }

    async fn subscribe(&self) -> mpsc::Receiver<()> {
        self.udisks2.subscribe_signals().await
    }
}

fn merge_with_platform_volumes(
    mut vols: Vec<VolumeInfo>,
    ignored_mount_paths: HashSet<String>,
    platform_volumes: Vec<VolumeInfo>,
) -> Vec<VolumeInfo> {
    // Merge with /proc/mounts-based volumes: udisks2 may miss some (e.g. root, network mounts)
    let ids: HashSet<String> = vols.iter().map(|v| v.id.clone()).collect();
    // Also deduplicate by mount point: two volumes cannot share the same
    // mounted path. This prevents duplicates when udisks2 and /proc/mounts
    // both report the same physical volume with different ID formats.
    let mounted_paths: HashSet<String> = vols
        .iter()
        .filter(|v| v.mounted)
        .map(|v| mount_path_key(&v.root_path))
        .collect();
    for pv in platform_volumes {
        let path = mount_path_key(&pv.root_path);
        if !ids.contains(&pv.id)
            && !mounted_paths.contains(&path)
            && !ignored_mount_paths.contains(&path)
        {
            vols.push(pv);
        }
    }
    vols
}

#[cfg(test)]
mod tests {
    use super::{merge_with_platform_volumes, mount_path_key};
    use crate::platform::VolumeInfo;
    use std::collections::HashSet;

    fn volume(id: &str, path: &str) -> VolumeInfo {
        VolumeInfo {
            id: id.into(),
            name: id.into(),
            root_path: path.into(),
            mounted: true,
            device_path: None,
            eject: None,
        }
    }

    #[test]
    fn normalizes_mount_path_keys_for_deduplication() {
        assert_eq!(mount_path_key("/"), "/");
        assert_eq!(
            mount_path_key("/run/media/me/Extreme SSD/"),
            "/run/media/me/Extreme SSD"
        );
        assert_eq!(
            mount_path_key("/run/media/me/Extreme SSD"),
            "/run/media/me/Extreme SSD"
        );
    }

    #[test]
    fn merge_adds_missing_mounts_without_readding_ignored_or_duplicate_paths() {
        let merged = merge_with_platform_volumes(
            vec![volume("udisks-root", "/")],
            HashSet::from(["/hidden".to_string()]),
            vec![
                volume("proc-root", "/"),
                volume("proc-hidden", "/hidden"),
                volume("network", "/mnt/network"),
            ],
        );
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|volume| volume.id == "udisks-root"));
        assert!(merged.iter().any(|volume| volume.id == "network"));
    }
}

/// Try to create a UDisks2-backed watcher. Returns None if udisks2 is not available.
pub async fn try_udisks2(platform: Arc<dyn Platform>) -> Option<UDisks2Watcher> {
    match UDisks2::new().await {
        Ok(ud) => Some(UDisks2Watcher {
            udisks2: ud,
            platform,
        }),
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

    async fn eject(&self, _device_path: &str, _action: EjectAction) -> Result<(), String> {
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
