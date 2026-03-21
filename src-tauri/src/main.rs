// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod jobs;
mod state;
mod vfs;
mod ws;

use std::sync::Arc;
use tokio::sync::Mutex;

use state::AppState;

fn main() {
    tauri::Builder::default()
        .setup(|_app| {
            let shared_state = Arc::new(Mutex::new(AppState::new_default()));
            tauri::async_runtime::spawn(ws::server::start(shared_state));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
