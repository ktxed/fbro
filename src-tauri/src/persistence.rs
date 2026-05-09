use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ─── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
pub struct SessionState {
    #[serde(default)]
    pub panels: HashMap<String, PanelSession>,
    #[serde(default)]
    pub font_size: Option<u8>,
    #[serde(default)]
    pub window: Option<WindowSession>,
}

#[derive(Serialize, Deserialize)]
pub struct PanelSession {
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct WindowSession {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

// ─── Path ─────────────────────────────────────────────────────────────────────

pub fn session_path() -> PathBuf {
    crate::config::config_dir().join("session.toml")
}

fn config_path() -> PathBuf {
    session_path()
}

// ─── I/O ──────────────────────────────────────────────────────────────────────

pub fn load() -> SessionState {
    let path = config_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        return SessionState::default();
    };
    toml::from_str(&content).unwrap_or_default()
}

pub fn save(session: &SessionState) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(content) = toml::to_string(session) {
        let _ = std::fs::write(path, content);
    }
}
