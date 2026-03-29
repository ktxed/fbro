#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

use std::sync::Arc;
use tokio::sync::mpsc;

/// Information about a volume / filesystem root (mounted or unmounted).
pub struct VolumeInfo {
    pub id: String,                    // stable ID, e.g. "vol-root", "vol-home"
    pub name: String,                  // display name, e.g. "/", "/home", "USB"
    pub root_path: String,            // mount point path (empty if unmounted)
    pub mounted: bool,                 // whether the volume is currently mounted
    pub device_path: Option<String>,   // udisks2 object path for mount operations
}

/// Abstraction over platform-specific environment queries and services.
pub trait Platform: Send + Sync {
    /// Returns the current user's home directory.
    fn home_dir(&self) -> String;

    /// Look up a system icon by freedesktop-style name (e.g. "text-rust", "folder").
    /// Returns (bytes, content_type) on success.
    fn find_icon(&self, _icon_name: &str) -> Option<(Vec<u8>, &'static str)> {
        None
    }

    /// Opens a file in the system's default editor.
    /// Returns the full path on success, or an error message if no editor is available.
    fn open_in_editor(&self, path: &str) -> Result<String, String>;

    /// Opens a file with the OS default application for its type. Fire-and-forget.
    fn open_file(&self, _path: &str) {}

    /// Opens a terminal emulator at the given directory. Fire-and-forget.
    fn open_terminal(&self, _path: &str) {}

    /// Returns (free_bytes, total_bytes) for the filesystem containing `path`.
    fn disk_space(&self, _path: &str) -> Option<(u64, u64)> {
        None
    }

    /// Returns all available volumes / mount points on this platform.
    fn list_volumes(&self) -> Vec<VolumeInfo> {
        vec![]
    }
}

// ── Embedded fallback icons ─────────────────────────────────────────────────

const FALLBACK_FOLDER_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="#8da5b4"><path d="M10 4H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-8l-2-2z"/></svg>"##;

const FALLBACK_FILE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="#8da5b4"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8l-6-6zm4 18H6V4h7v5h5v11z"/></svg>"##;

/// Resolve an icon name using the platform, falling back to generic embedded SVGs.
pub fn resolve_icon(platform: &dyn Platform, icon_name: &str) -> (Vec<u8>, &'static str) {
    if let Some(result) = platform.find_icon(icon_name) {
        return result;
    }
    // Fallback: generic folder or file icon
    if icon_name == "folder" {
        (FALLBACK_FOLDER_SVG.to_vec(), "image/svg+xml")
    } else {
        (FALLBACK_FILE_SVG.to_vec(), "image/svg+xml")
    }
}

/// Create the platform implementation for the current OS.
pub fn create_platform() -> std::sync::Arc<dyn Platform> {
    #[cfg(target_os = "linux")]
    {
        std::sync::Arc::new(linux::LinuxPlatform)
    }
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(macos::MacOSPlatform)
    }
    #[cfg(target_os = "windows")]
    {
        std::sync::Arc::new(windows::WindowsPlatform)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    compile_error!("unsupported platform")
}

// ── Volume watcher abstraction ──────────────────────────────────────────────

/// Async interface for volume enumeration, mounting, and change notification.
/// Hides platform-specific details (udisks2, /proc/mounts, etc.) behind a
/// uniform API so that `watcher.rs` and `server.rs` contain no `#[cfg]`.
#[async_trait::async_trait]
pub trait VolumeWatcher: Send + Sync {
    /// List all volumes (mounted + unmounted).
    async fn list_volumes(&self) -> Vec<VolumeInfo>;
    /// Mount an unmounted volume by its device path. Returns the mount point.
    async fn mount(&self, device_path: &str) -> Result<String, String>;
    /// Subscribe to volume change signals. Returns a channel that fires `()`
    /// whenever any volume state changes (plug/unplug, mount/unmount).
    async fn subscribe(&self) -> mpsc::Receiver<()>;
}

/// Fallback volume watcher: uses Platform::list_volumes() (sync) and does not
/// support mounting. Used on non-Linux platforms or when udisks2 is unavailable.
pub struct FallbackVolumeWatcher {
    platform: Arc<dyn Platform>,
}

impl FallbackVolumeWatcher {
    pub fn new(platform: Arc<dyn Platform>) -> Self {
        Self { platform }
    }
}

#[async_trait::async_trait]
impl VolumeWatcher for FallbackVolumeWatcher {
    async fn list_volumes(&self) -> Vec<VolumeInfo> {
        self.platform.list_volumes()
    }

    async fn mount(&self, _device_path: &str) -> Result<String, String> {
        Err("Mounting is not supported on this platform".to_string())
    }

    async fn subscribe(&self) -> mpsc::Receiver<()> {
        // No signal source — return a channel that never fires.
        // The watcher falls back to periodic polling.
        let (_tx, rx) = mpsc::channel(1);
        rx
    }
}

/// Create the appropriate VolumeWatcher for the current platform.
pub async fn create_volume_watcher(platform: Arc<dyn Platform>) -> Arc<dyn VolumeWatcher> {
    #[cfg(target_os = "linux")]
    {
        match linux::volume_watcher::try_udisks2(platform.clone()).await {
            Some(watcher) => {
                eprintln!("[fbro] udisks2 available — unmounted volumes will be shown");
                return Arc::new(watcher);
            }
            None => {
                eprintln!("[fbro] udisks2 not available — falling back to /proc/mounts");
                return Arc::new(linux::volume_watcher::ProcMountsWatcher::new(platform));
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    Arc::new(FallbackVolumeWatcher::new(platform))
}
