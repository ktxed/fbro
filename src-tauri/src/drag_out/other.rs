use std::path::PathBuf;
use std::sync::OnceLock;

use tauri::Emitter;
use tauri::Manager;

#[derive(Clone, Debug, serde::Serialize)]
struct DragOutResult {
    success: bool,
}

// The AppHandle is stored once during setup so the Tauri command (which
// runs on a thread-pool thread) can reach it.
static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

pub fn init_drag_out(app_handle: tauri::AppHandle) {
    let _ = APP_HANDLE.set(app_handle);
}

pub fn start_drag_out(paths: Vec<String>) -> Result<(), String> {
    let app_handle = APP_HANDLE
        .get()
        .ok_or_else(|| "Drag-out not initialized".to_string())?
        .clone();

    // Resolve absolute PathBufs; `drag` requires canonical absolute paths.
    let path_bufs: Vec<PathBuf> = paths
        .iter()
        .map(|p| {
            let pb = PathBuf::from(p);
            if pb.is_absolute() {
                pb
            } else {
                std::fs::canonicalize(&pb).unwrap_or(pb)
            }
        })
        .collect();

    let icon_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("icons")
        .join("32x32.png");

    let app_handle_cb = app_handle.clone();
    let app_handle_main = app_handle.clone();

    app_handle_main
        .run_on_main_thread(move || {
            let window = match app_handle.get_webview_window("main") {
                Some(w) => w,
                None => {
                    eprintln!("[fbro] drag-out: could not get main window");
                    let _ = app_handle.emit("drag-out-complete", DragOutResult { success: false });
                    return;
                }
            };

            let image = drag::Image::File(icon_path);
            let item = drag::DragItem::Files(path_bufs);

            let result = drag::start_drag(
                &window,
                item,
                image,
                move |drag_result, _cursor_pos| {
                    let success = matches!(drag_result, drag::DragResult::Dropped);
                    let _ = app_handle_cb.emit("drag-out-complete", DragOutResult { success });
                },
                drag::Options::default(),
            );

            if let Err(e) = result {
                eprintln!("[fbro] drag-out failed: {e}");
                let _ = app_handle.emit("drag-out-complete", DragOutResult { success: false });
            }
        })
        .map_err(|e| format!("run_on_main_thread failed: {e}"))
}
