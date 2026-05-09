use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Settings {
    pub general: GeneralSettings,
    /// Per-panel view mode ("list" or "grid"), keyed by panel_id.
    #[serde(default)]
    pub panel_view_modes: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GeneralSettings {
    pub show_hidden_default: bool,
    pub remember_window_state: bool,
    #[serde(default = "default_true")]
    pub preserve_attributes: bool,
    #[serde(default)]
    pub ludicrous_mode: bool,
    #[serde(default = "default_max_image_size_mb")]
    pub max_image_size_mb: u64,
}

fn default_true() -> bool {
    true
}

fn default_max_image_size_mb() -> u64 {
    512
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            show_hidden_default: false,
            remember_window_state: true,
            preserve_attributes: true,
            ludicrous_mode: false,
            max_image_size_mb: 512,
        }
    }
}

fn settings_path() -> PathBuf {
    crate::config::config_dir().join("settings.json")
}

pub fn load() -> Settings {
    let path = settings_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Settings::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn save_general(general: &GeneralSettings) -> std::io::Result<()> {
    let mut settings = load();
    settings.general = general.clone();
    save(&settings)
}
