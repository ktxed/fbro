use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Settings {
    pub general: GeneralSettings,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GeneralSettings {
    pub show_hidden_default: bool,
    pub remember_window_state: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            show_hidden_default: false,
            remember_window_state: true,
        }
    }
}

fn settings_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        format!("{home}/.config")
    });
    PathBuf::from(base).join("fbro").join("settings.json")
}

pub fn load() -> Settings {
    let path = settings_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Settings::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

pub fn save_general(general: &GeneralSettings) -> std::io::Result<()> {
    let settings = Settings {
        general: general.clone(),
    };
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}
