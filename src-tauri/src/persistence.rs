use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ─── Types ────────────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
pub struct SessionState {
    #[serde(default)]
    pub panels: HashMap<String, PanelSession>,
}

#[derive(Serialize, Deserialize)]
pub struct PanelSession {
    pub path: String,
}

// ─── Path ─────────────────────────────────────────────────────────────────────

fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        format!("{home}/.config")
    });
    PathBuf::from(base).join("fbro").join("session.toml")
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
