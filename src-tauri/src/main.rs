// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod jobs;
mod persistence;
mod platform;
mod state;
mod vfs;
mod ws;

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use platform::linux::LinuxPlatform;
use state::AppState;
use tokio::sync::Mutex;

// ── Icon protocol ────────────────────────────────────────────────────────────

/// Search the system icon theme for `icon_name` (e.g. "text-rust", "folder").
/// Returns (bytes, content_type) on success.
fn find_icon(icon_name: &str) -> Option<(Vec<u8>, &'static str)> {
    const THEMES: &[&str] = &["Pop", "Adwaita", "hicolor"];
    const SIZES: &[&str] = &["48x48", "scalable", "32x32", "24x24"];
    const CATS: &[&str] = &["mimetypes", "places", "emblems"];

    for theme in THEMES {
        for size in SIZES {
            for cat in CATS {
                let svg = format!("/usr/share/icons/{theme}/{size}/{cat}/{icon_name}.svg");
                if let Ok(data) = std::fs::read(&svg) {
                    return Some((data, "image/svg+xml"));
                }
                let png = format!("/usr/share/icons/{theme}/{size}/{cat}/{icon_name}.png");
                if let Ok(data) = std::fs::read(&png) {
                    return Some((data, "image/png"));
                }
            }
        }
    }
    None
}

fn main() {
    let icon_cache: Arc<StdMutex<HashMap<String, Option<(Vec<u8>, &'static str)>>>> =
        Arc::new(StdMutex::new(HashMap::new()));

    tauri::Builder::default()
        .register_uri_scheme_protocol("icon", {
            let cache = icon_cache;
            move |_ctx, request| {
                let name = request.uri().path().trim_start_matches('/').to_string();
                let entry = {
                    let mut c = cache.lock().unwrap();
                    c.entry(name.clone()).or_insert_with(|| find_icon(&name)).clone()
                };
                match entry {
                    Some((data, ct)) => http::Response::builder()
                        .header("Content-Type", ct)
                        .header("Cache-Control", "public, max-age=31536000, immutable")
                        .body(data)
                        .unwrap(),
                    None => http::Response::builder()
                        .status(http::StatusCode::NOT_FOUND)
                        .body(Vec::new())
                        .unwrap(),
                }
            }
        })
        .setup(|app| {
            let app_handle = app.handle().clone();
            let platform = Arc::new(LinuxPlatform);
            let session = persistence::load();
            let initial_paths = session.panels.into_iter()
                .map(|(id, p)| (id, p.path))
                .collect();
            let shared_state = Arc::new(Mutex::new(AppState::new_default(platform, initial_paths)));
            tauri::async_runtime::spawn(ws::server::start(shared_state, app_handle));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
