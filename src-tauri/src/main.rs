// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bro;
mod drag_out;
mod fileserver;
mod jobs;
mod persistence;
mod platform;
mod state;
mod vfs;
mod watcher;
mod ws;

mod drag_out_cmd {
    #[tauri::command]
    pub fn start_drag_out(paths: Vec<String>) -> Result<(), String> {
        super::drag_out::start_drag_out(paths)
    }
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use jobs::viewer_prefetch::ViewerPrefetchQueue;
use state::AppState;
use tokio::sync::Mutex;

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let s = header.strip_prefix("bytes=")?;
    let (start_str, end_str) = s.split_once('-')?;
    let start: u64 = start_str.parse().ok()?;
    let end: u64 = if end_str.is_empty() {
        total.saturating_sub(1)
    } else {
        end_str.parse::<u64>().ok()?.min(total.saturating_sub(1))
    };
    if start > end {
        return None;
    }
    Some((start, end))
}

fn read_file_range(path: &str, start: u64, length: usize) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut buf = vec![0u8; length];
    let n = file.read(&mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

#[derive(serde::Serialize)]
struct PathInfo {
    kind: String,
    size: Option<u64>,
}

#[tauri::command]
fn get_path_info(path: &str) -> Result<PathInfo, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| format!("Cannot stat {}: {e}", path))?;
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        "symlink"
    } else if file_type.is_dir() {
        "directory"
    } else if file_type.is_file() {
        "file"
    } else {
        "other"
    };
    let size = if file_type.is_file() {
        Some(metadata.len())
    } else {
        None
    };
    Ok(PathInfo { kind: kind.to_string(), size })
}

fn mime_for_path(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn main() {
    #[cfg(target_os = "linux")]
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        std::env::set_var("GDK_BACKEND", "wayland");
    }

    #[cfg(target_os = "linux")]
    let _ = gtk::init();

    let platform = platform::create_platform();
    let viewer_prefetch = Arc::new(ViewerPrefetchQueue::new());
    let file_server_port = fileserver::start(platform.clone(), Arc::clone(&viewer_prefetch));
    let icon_cache: Arc<StdMutex<HashMap<String, (Vec<u8>, &'static str)>>> =
        Arc::new(StdMutex::new(HashMap::new()));

    tauri::Builder::default()
        .register_uri_scheme_protocol("localfile", |_ctx, request| {
            // Serve local filesystem files for the viewer.
            // WebKit blocks file:// from a non-file:// origin, so we proxy via
            // this custom scheme: localfile://localhost/abs/path/to/file
            //
            // Range request support is required for video playback: browsers send
            // "Range: bytes=N-M" to buffer/seek; we respond with 206 Partial Content.
            let raw_path = request.uri().path().to_string();
            let decoded = percent_decode(&raw_path);
            // On Windows the URI path has a leading / before the drive letter: /C:/... → C:/...
            #[cfg(windows)]
            let path = decoded.trim_start_matches('/').to_string();
            #[cfg(not(windows))]
            let path = decoded;
            let mime = mime_for_path(&path);

            let total = match std::fs::metadata(&path).map(|m| m.len()) {
                Ok(n) => n,
                Err(_) => return http::Response::builder().status(404).body(vec![]).unwrap(),
            };

            // Check for Range header
            let range = request
                .headers()
                .get("Range")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| parse_range(s, total));

            if let Some((start, end)) = range {
                let length = (end - start + 1) as usize;
                let data = read_file_range(&path, start, length).unwrap_or_default();
                http::Response::builder()
                    .status(206)
                    .header("Content-Type", mime)
                    .header("Content-Range", format!("bytes {start}-{end}/{total}"))
                    .header("Content-Length", data.len().to_string())
                    .header("Accept-Ranges", "bytes")
                    .body(data)
                    .unwrap()
            } else {
                match std::fs::read(&path) {
                    Ok(data) => http::Response::builder()
                        .status(200)
                        .header("Content-Type", mime)
                        .header("Accept-Ranges", "bytes")
                        .header("Content-Length", total.to_string())
                        .body(data)
                        .unwrap(),
                    Err(_) => http::Response::builder().status(404).body(vec![]).unwrap(),
                }
            }
        })
        .register_uri_scheme_protocol("icon", {
            let cache = icon_cache;
            let platform = platform.clone();
            move |_ctx, request| {
                let name = request.uri().path().trim_start_matches('/').to_string();
                let (data, ct) = {
                    let mut c = cache.lock().unwrap();
                    c.entry(name.clone())
                        .or_insert_with(|| platform::resolve_icon(&*platform, &name))
                        .clone()
                };
                http::Response::builder()
                    .header("Content-Type", ct)
                    .header("Cache-Control", "public, max-age=31536000, immutable")
                    .body(data)
                    .unwrap()
            }
        })
        .invoke_handler(tauri::generate_handler![get_path_info, crate::drag_out_cmd::start_drag_out])
        .setup({
            let platform = platform.clone();
            move |app| {
                let app_handle = app.handle().clone();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.set_icon(tauri::include_image!("icons/128x128.png"));
                }

                #[cfg(target_os = "linux")]
                {
                    if let Some(window) = app.get_webview_window("main") {
                        if let Ok(gtk_window) = window.gtk_window() {
                            let handle = drag_out::init_drag_out(&gtk_window, app.handle().clone());
                            drag_out::set_drag_out_handle(handle);
                        }
                    }
                }

                #[cfg(not(target_os = "linux"))]
                {
                    drag_out::init_drag_out(app.handle().clone());
                }
                let session = persistence::load();
                let initial_paths = session
                    .panels
                    .into_iter()
                    .map(|(id, p)| (id, p.path))
                    .collect();
                let shared_state = Arc::new(Mutex::new(AppState::new_default(
                    platform,
                    initial_paths,
                    file_server_port,
                )));
                let quit_notify = Arc::new(tokio::sync::Notify::new());
                use tauri::Manager;
                app.manage(Arc::clone(&quit_notify));
                tauri::async_runtime::spawn(ws::server::start(
                    shared_state,
                    app_handle,
                    quit_notify,
                    viewer_prefetch,
                ));
                Ok(())
            }
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    use tauri::Manager;
                    let notify = window.app_handle().state::<Arc<tokio::sync::Notify>>();
                    notify.notify_one();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
