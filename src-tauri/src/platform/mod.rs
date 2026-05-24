#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(unix)]
mod shell_command;
#[cfg(target_os = "windows")]
pub mod windows;

use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Information about a volume / filesystem root (mounted or unmounted).
pub struct VolumeInfo {
    pub id: String,                  // stable ID, e.g. "vol-root", "vol-home"
    pub name: String,                // display name, e.g. "/", "/home", "USB"
    pub root_path: String,           // mount point path (empty if unmounted)
    pub mounted: bool,               // whether the volume is currently mounted
    pub device_path: Option<String>, // udisks2 object path for mount operations
}

#[derive(Clone, Debug)]
pub struct OpenWithAppOption {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
    pub is_default: bool,
    pub source: String,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum OpenWithLaunch {
    Default,
    Command {
        program: String,
        args: Vec<String>,
    },
    LinuxApp {
        content_type: String,
        app_key: String,
    },
    MacApplication {
        application_path: String,
    },
}

#[derive(Clone, Debug)]
pub struct ResolvedOpenWithApp {
    pub option: OpenWithAppOption,
    pub launch: OpenWithLaunch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalClipboardOp {
    Copy,
    Move,
}

#[derive(Clone, Debug)]
pub struct ExternalClipboardPayload {
    pub op: ExternalClipboardOp,
    pub source_paths: Vec<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCommandInstallResult {
    pub path: String,
    pub command: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCommandStatus {
    pub command: String,
    pub path: Option<String>,
    pub installed: bool,
    pub managed: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellCommandRemoveResult {
    pub path: String,
    pub command: String,
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

    /// Generate a thumbnail for a real OS file path at the requested size (px,
    /// longer edge). Returns (PNG bytes, content_type) on success. Default impl
    /// returns None — platforms override to call native thumbnail providers
    /// (Windows IShellItemImageFactory, macOS QLThumbnailGenerator).
    /// MUST be safe to call from any worker thread.
    fn get_thumbnail(&self, _path: &str, _size: u32) -> Option<(Vec<u8>, &'static str)> {
        None
    }

    /// Opens a file in the system's default editor.
    /// Returns the full path on success, or an error message if no editor is available.
    fn open_in_editor(&self, path: &str) -> Result<String, String>;

    /// Opens a file with the OS default application for its type. Fire-and-forget.
    fn open_file(&self, _path: &str) {}

    /// Opens a terminal emulator at the given directory. Fire-and-forget.
    fn open_terminal(&self, _path: &str) {}

    /// Reveals a path in the platform file manager.
    fn reveal_in_file_manager(&self, _path: &str, _kind: &str) {}

    /// Lists applications that can open the given path.
    fn list_open_with_apps(
        &self,
        _path: &str,
        _kind: &str,
    ) -> Result<Vec<ResolvedOpenWithApp>, String> {
        Ok(vec![])
    }

    /// Launches the given path using one of the applications returned by `list_open_with_apps`.
    fn launch_open_with(&self, path: &str, app: &ResolvedOpenWithApp) -> Result<(), String> {
        match &app.launch {
            OpenWithLaunch::Default => {
                self.open_file(path);
                Ok(())
            }
            OpenWithLaunch::Command { program, args } => {
                let mut cmd = std::process::Command::new(program);
                cmd.args(args).arg(path);
                cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
            }
            OpenWithLaunch::MacApplication { application_path } => {
                std::process::Command::new("open")
                    .arg("-a")
                    .arg(application_path)
                    .arg(path)
                    .spawn()
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
            OpenWithLaunch::LinuxApp { .. } => {
                Err("Open With is not supported for this app on this platform".to_string())
            }
        }
    }

    /// Returns (free_bytes, total_bytes) for the filesystem containing `path`.
    fn disk_space(&self, _path: &str) -> Option<(u64, u64)> {
        None
    }

    /// Returns all available volumes / mount points on this platform.
    fn list_volumes(&self) -> Vec<VolumeInfo> {
        vec![]
    }

    /// Reads an external file-manager clipboard payload, if available.
    fn read_external_file_clipboard(&self) -> Result<Option<ExternalClipboardPayload>, String> {
        Ok(None)
    }

    /// Writes a host file-manager clipboard payload.
    fn write_external_file_clipboard(
        &self,
        _payload: &ExternalClipboardPayload,
    ) -> Result<(), String> {
        Err("Writing to the host clipboard is not supported on this platform.".to_string())
    }

    /// Writes plain text to the system clipboard.
    fn write_text_clipboard(&self, _text: &str) -> Result<(), String> {
        Err("Writing text to clipboard is not supported on this platform.".to_string())
    }

    /// Probes whether the terminal command used to launch fbro is available.
    fn probe_fbro_command(&self) -> Result<ShellCommandStatus, String> {
        Ok(ShellCommandStatus {
            command: "fbro".to_string(),
            path: None,
            installed: false,
            managed: false,
        })
    }

    /// Installs the terminal command used to launch fbro.
    fn install_fbro_command(&self, _exe: &Path) -> Result<ShellCommandInstallResult, String> {
        Err("Installing the fbro command is currently supported on Unix-like systems.".to_string())
    }

    /// Removes the fbro-managed terminal command.
    fn remove_fbro_command(&self) -> Result<ShellCommandRemoveResult, String> {
        Err("Removing the fbro command is currently supported on Unix-like systems.".to_string())
    }
}

// ── Embedded fallback icons ─────────────────────────────────────────────────

const FALLBACK_FOLDER_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="#8da5b4"><path d="M10 4H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-8l-2-2z"/></svg>"##;

const FALLBACK_FILE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="#8da5b4"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8l-6-6zm4 18H6V4h7v5h5v11z"/></svg>"##;

const IE_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128"><path fill="#1EBBEE" d="M126.11 66.834c0-9.82-2.539-19.034-6.991-27.051C138.097-3.167 98.785 3.13 96.596 3.558c-8.328 1.629-16.034 4.249-23.142 7.566a56.276 56.276 0 00-3.166-.09c-26.525 0-48.729 18.501-54.411 43.303 13.98-15.684 23.765-22.012 29.624-24.544-.936.835-1.851 1.681-2.751 2.532l-.885.855c-.594.571-1.186 1.143-1.765 1.72-.344.343-.68.689-1.019 1.035-.514.522-1.026 1.044-1.527 1.569l-1.052 1.122c-.468.501-.934 1.003-1.39 1.506l-1.053 1.175a143.278 143.278 0 00-3.516 4.099c-.347.419-.692.838-1.031 1.256-.367.453-.726.905-1.083 1.356-.348.438-.695.875-1.034 1.312l-.953 1.246c-.356.469-.715.938-1.063 1.404l-.731.998a173.801 173.801 0 00-6.041 8.796l-.013.021c-.479.747-.941 1.482-1.395 2.211l-.074.12c-.454.732-.892 1.451-1.319 2.164l-.045.075a181.942 181.942 0 00-3.172 5.501C7.562 80.977 5.114 87.343 5.008 87.72c-15.876 56.759 33.674 32.791 40.588 29.213 7.445 3.678 15.827 5.973 24.692 5.973C94.563 122.905 115.213 107 122.9 86H93.569c-4.34 7-12.694 12.049-22.293 12.049C57.211 98.049 45.81 87 45.81 74h79.842c.303-2 .458-4.73.458-7.166zM13.526 116.817c-3.916-4.015-4.608-13.795 4.032-31.616 4.36 12.537 13.061 23.041 24.307 29.708-5.593 3.079-20.441 10.009-28.339 1.908zM45.687 59c.446-13 11.564-23.005 25.226-23.005C84.574 35.995 95.694 46 96.14 59H45.687zm44.776-44.204c5.166-2.495 17.958-7.771 25.22-2.871 4.807 3.245 8.663 8.34 2.042 25.499-6.351-10.213-15.905-18.224-27.262-22.628z"/></svg>"##;

/// Resolve an icon name using the platform, falling back to generic embedded SVGs.
pub fn resolve_icon(platform: &dyn Platform, icon_name: &str) -> (Vec<u8>, &'static str) {
    if icon_name == "fbro-ie-html" {
        return (IE_SVG.to_vec(), "image/svg+xml");
    }
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

pub fn append_supplemental_apps(
    apps: &mut Vec<ResolvedOpenWithApp>,
    supplements: Vec<ResolvedOpenWithApp>,
) {
    let mut seen: std::collections::HashSet<String> = apps
        .iter()
        .map(|app| normalize_open_with_key(&app.option.label, app.option.detail.as_deref()))
        .collect();

    for app in supplements {
        let key = normalize_open_with_key(&app.option.label, app.option.detail.as_deref());
        if seen.insert(key) {
            apps.push(app);
        }
    }
}

fn normalize_open_with_key(label: &str, detail: Option<&str>) -> String {
    let label = label.trim().to_lowercase();
    let detail = detail.unwrap_or("").trim().to_lowercase();
    format!("{label}::{detail}")
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
#[allow(dead_code)]
pub struct FallbackVolumeWatcher {
    platform: Arc<dyn Platform>,
}

#[allow(dead_code)]
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
