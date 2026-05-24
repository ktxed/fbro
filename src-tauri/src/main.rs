// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bookmarks;
mod bro;
mod config;
mod drag_out;
mod fileserver;
mod jobs;
mod persistence;
mod platform;
mod remote;
mod state;
mod terminal;
mod thumb_pool;
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
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use futures_util::SinkExt;
use jobs::viewer_prefetch::ViewerPrefetchQueue;
use state::{AppState, CliOpenRequest};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

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

fn parse_cli_open_request() -> Option<CliOpenRequest> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--fbro-open" {
            let path = args.next().unwrap_or_else(|| ".".to_string());
            let cwd = std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .to_string_lossy()
                .to_string();
            return Some(CliOpenRequest {
                path,
                cwd,
                create_tab: true,
            });
        }
    }
    None
}

fn try_forward_cli_open(request: &CliOpenRequest) -> bool {
    let request = request.clone();
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return false,
    };

    runtime.block_on(async move {
        let connect = tokio::time::timeout(
            std::time::Duration::from_millis(350),
            tokio_tungstenite::connect_async("ws://127.0.0.1:7878"),
        )
        .await;
        let Ok(Ok((mut stream, _))) = connect else {
            return false;
        };

        let hello = serde_json::json!({
            "type": "Hello",
            "timestamp": crate::ws::protocol::now(),
            "protocolVersion": "0",
            "client": { "kind": "cli", "name": "fbro-cli", "supports": [] },
        });
        let intent = serde_json::json!({
            "type": "Intent",
            "timestamp": crate::ws::protocol::now(),
            "name": "OpenPathInRightPanel",
            "scope": null,
            "args": {
                "path": request.path,
                "cwd": request.cwd,
                "createTab": true,
            },
            "options": {},
        });

        if stream
            .send(Message::Text(hello.to_string().into()))
            .await
            .is_err()
        {
            return false;
        }
        if stream
            .send(Message::Text(intent.to_string().into()))
            .await
            .is_err()
        {
            return false;
        }
        let _ = stream.close(None).await;
        true
    })
}

#[tauri::command]
fn probe_fbro_command() -> Result<platform::ShellCommandStatus, String> {
    platform::create_platform().probe_fbro_command()
}

#[tauri::command]
fn install_fbro_command() -> Result<platform::ShellCommandInstallResult, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot locate fbro: {e}"))?;
    platform::create_platform().install_fbro_command(&exe)
}

#[tauri::command]
fn remove_fbro_command() -> Result<platform::ShellCommandRemoveResult, String> {
    platform::create_platform().remove_fbro_command()
}

#[derive(serde::Serialize)]
struct FrontendConfig {
    #[serde(rename = "wsUrl")]
    ws_url: Option<String>,
}

#[tauri::command]
fn frontend_config() -> FrontendConfig {
    FrontendConfig {
        ws_url: std::env::var("FBRO_WS_URL").ok().filter(|v| !v.is_empty()),
    }
}

#[derive(serde::Serialize)]
struct PathInfo {
    kind: String,
    size: Option<u64>,
}

#[tauri::command]
fn get_path_info(path: &str) -> Result<PathInfo, String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|e| format!("Cannot stat {}: {e}", path))?;
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
    Ok(PathInfo {
        kind: kind.to_string(),
        size,
    })
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

fn frontend_only() -> bool {
    matches!(
        std::env::var("FBRO_FRONTEND_ONLY").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

fn main() {
    let cli_open_request = parse_cli_open_request();
    if let Some(request) = &cli_open_request {
        if try_forward_cli_open(request) {
            return;
        }
    }

    #[cfg(target_os = "linux")]
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        std::env::set_var("GDK_BACKEND", "wayland");
    }

    #[cfg(target_os = "linux")]
    let _ = gtk::init();

    let is_frontend_only = frontend_only();
    let platform = platform::create_platform();
    let viewer_prefetch = Arc::new(ViewerPrefetchQueue::new());
    // 1024 entries × ~20 KB/PNG ≈ 20 MB resident cap. Comfortable for browsing
    // image-heavy folders (thousands of files) without re-extracting thumbnails
    // on every scroll-back.
    let thumb_cache = Arc::new(fileserver::ThumbCache::new(1024));
    // Worker threads for thumbnail extraction. 8 lets the OS thumbnail
    // provider keep multiple cores busy when the user lands on a fresh
    // region of a large image folder. Going higher mostly contends with
    // the shell's own background thumbnailer.
    let thumb_pool = thumb_pool::ThumbPool::start(platform.clone(), 8);
    let file_server_port = if is_frontend_only {
        0
    } else {
        fileserver::start(
            platform.clone(),
            Arc::clone(&viewer_prefetch),
            Arc::clone(&thumb_cache),
            Arc::clone(&thumb_pool),
        )
    };
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
        .invoke_handler(tauri::generate_handler![
            get_path_info,
            frontend_config,
            probe_fbro_command,
            install_fbro_command,
            remove_fbro_command,
            crate::drag_out_cmd::start_drag_out
        ])
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
                if is_frontend_only {
                    eprintln!("[fbro] frontend-only mode enabled; embedded backend is disabled");
                    return Ok(());
                }
                let session = persistence::load();
                let settings = crate::state::settings::load();
                #[cfg(target_os = "linux")]
                crate::platform::linux::thumbnail::apply_settings(
                    settings.general.enable_video_thumbnails,
                    settings.general.thumbnail_max_image_size_mb,
                    settings.general.thumbnail_video_frame_skip,
                );
                if settings.general.remember_window_state {
                    if let Some(ws) = &session.window {
                        if let Some(window) = app.get_webview_window("main") {
                            if ws.maximized {
                                let _ = window.maximize();
                            } else {
                                let _ =
                                    window.set_size(tauri::PhysicalSize::new(ws.width, ws.height));
                                let _ =
                                    window.set_position(tauri::PhysicalPosition::new(ws.x, ws.y));
                            }
                        }
                    }
                }
                let initial_paths = session
                    .panels
                    .iter()
                    .map(|(id, p)| (id.clone(), p.path.clone()))
                    .collect();
                let mut app_state = AppState::new_default(
                    platform,
                    initial_paths,
                    file_server_port,
                    Arc::clone(&thumb_pool),
                    Arc::clone(&thumb_cache),
                );
                // Restore extra tabs (beyond the seed panel) from the saved
                // session when the new slot-aware schema is present.
                crate::state::restore_tabs_from_session(&mut app_state, &session.slots);
                if let Some(mut request) = cli_open_request.clone() {
                    request.create_tab = false;
                    app_state.pending_cli_opens.push(request);
                }
                let shared_state = Arc::new(Mutex::new(app_state));

                // If the user previously enabled sharing, restart the russh +
                // pair listeners, start discovery, and re-advertise. This is
                // what makes the "pair once, restart fbro, still works"
                // workflow viable without browsing the LAN while remote
                // access is disabled.
                {
                    let st = Arc::clone(&shared_state);
                    let was_enabled = tauri::async_runtime::block_on(async {
                        st.lock().await.share_config.enabled
                    });
                    if was_enabled {
                        tauri::async_runtime::spawn(async move {
                            if let Err(e) =
                                ws::intents::sharing::start_sharing_server_for(&st).await
                            {
                                eprintln!("[fbro] auto-restart sharing failed: {e}");
                            } else {
                                eprintln!("[fbro] sharing auto-restored from share.json");
                            }
                        });
                    }
                }

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
            if !frontend_only() && window.label() == "main" {
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
