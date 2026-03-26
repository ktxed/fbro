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

use state::AppState;
use tokio::sync::Mutex;

fn main() {
    let platform = platform::create_platform();
    let icon_cache: Arc<StdMutex<HashMap<String, (Vec<u8>, &'static str)>>> =
        Arc::new(StdMutex::new(HashMap::new()));

    tauri::Builder::default()
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
        .setup({
            let platform = platform.clone();
            move |app| {
                let app_handle = app.handle().clone();
                let session = persistence::load();
                let initial_paths = session.panels.into_iter()
                    .map(|(id, p)| (id, p.path))
                    .collect();
                let shared_state = Arc::new(Mutex::new(AppState::new_default(platform, initial_paths)));
                let quit_notify = Arc::new(tokio::sync::Notify::new());
                use tauri::Manager;
                app.manage(Arc::clone(&quit_notify));
                tauri::async_runtime::spawn(ws::server::start(shared_state, app_handle, quit_notify));
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
