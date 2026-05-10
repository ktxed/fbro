use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ─── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
pub struct SessionState {
    /// Legacy single-panel-per-slot persistence. Kept for back-compat with
    /// session files written before tabs landed; ignored when `slots` is
    /// non-empty.
    #[serde(default)]
    pub panels: HashMap<String, PanelSession>,
    /// New slot-aware persistence. One entry per layout slot, in order.
    #[serde(default)]
    pub slots: Vec<SlotSession>,
    #[serde(default)]
    pub font_size: Option<u8>,
    #[serde(default)]
    pub window: Option<WindowSession>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PanelSession {
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct SlotSession {
    pub slot_id: String,
    #[serde(default)]
    pub active_index: usize,
    pub tabs: Vec<TabSession>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TabSession {
    Listing {
        #[serde(default)]
        source_id: Option<String>,
        path: String,
    },
    /// Terminal tabs aren't restored as a live PTY — on restore we open a
    /// plain listing at the recorded cwd. The user can re-promote a terminal
    /// later if desired.
    Terminal {
        cwd: String,
    },
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
