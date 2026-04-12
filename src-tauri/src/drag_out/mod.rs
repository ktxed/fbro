#[cfg(target_os = "linux")]
mod linux;

#[cfg(not(target_os = "linux"))]
mod other;

// ── Linux ────────────────────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
pub use linux::{init_drag_out, set_drag_out_handle};

/// Queue a drag-out operation on Linux (dispatched on the next GTK motion event).
#[cfg(target_os = "linux")]
pub fn start_drag_out(paths: Vec<String>) -> Result<(), String> {
    linux::start_drag_out(paths)
}

// ── macOS / Windows ──────────────────────────────────────────────────────────

/// Store the AppHandle for later use by the Tauri command.
#[cfg(not(target_os = "linux"))]
pub fn init_drag_out(app_handle: tauri::AppHandle) {
    other::init_drag_out(app_handle);
}

/// Start a drag-out operation (spawns a thread; returns immediately).
#[cfg(not(target_os = "linux"))]
pub fn start_drag_out(paths: Vec<String>, window: tauri::WebviewWindow) -> Result<(), String> {
    other::start_drag_out(paths, window)
}
