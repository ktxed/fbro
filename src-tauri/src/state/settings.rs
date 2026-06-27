use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Settings {
    pub general: GeneralSettings,
    /// Per-panel view mode ("list" or "grid"), keyed by panel_id. Only
    /// honoured when `general.save_panel_view_mode` is true.
    #[serde(default)]
    pub panel_view_modes: HashMap<String, String>,
    /// Per-modal preferred dimensions, keyed by stable frontend modal id.
    #[serde(default)]
    pub modal_sizes: HashMap<String, ModalSizePreference>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ModalSizePreference {
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GeneralSettings {
    pub show_hidden_default: bool,
    pub remember_window_state: bool,
    #[serde(default = "default_true")]
    pub preserve_attributes: bool,
    #[serde(default = "default_true")]
    pub auto_resume_copies: bool,
    #[serde(default)]
    pub ludicrous_mode: bool,
    #[serde(default = "default_max_image_size_mb")]
    pub max_image_size_mb: u64,
    /// When true, each panel's grid/list mode is persisted across sessions.
    /// When false (default), every session starts with both panels in list.
    #[serde(default)]
    pub save_panel_view_mode: bool,
    /// Linux fbro-managed thumbnailer: when false, skip video thumbnail
    /// generation entirely (returns no thumb so the FE falls back to icon).
    #[serde(default = "default_true")]
    pub enable_video_thumbnails: bool,
    /// Linux fbro-managed thumbnailer: skip image files larger than this many
    /// MiB rather than decoding them — protects against memory blowup on
    /// raw / multi-gigapixel images.
    #[serde(default = "default_thumb_max_image_size_mb")]
    pub thumbnail_max_image_size_mb: u64,
    /// Linux fbro-managed thumbnailer: number of decoded frames to discard at
    /// the start of a video before picking the thumbnail frame. Many videos
    /// open on black or studio cards.
    #[serde(default = "default_thumb_video_frame_skip")]
    pub thumbnail_video_frame_skip: u32,
    /// When true, this fbro will generate thumbnails for paired remote fbro
    /// clients browsing shared folders in grid mode.
    #[serde(default = "default_true")]
    pub stream_thumbnails: bool,
    /// When true, every tab in every slot is saved at shutdown and restored on
    /// startup. When false, only the active tab per slot is persisted (the
    /// pre-tabs behavior).
    #[serde(default = "default_true")]
    pub persist_panel_tabs: bool,
    /// When true, the process VFS lists processes as a navigable tree (enter a
    /// process to see its direct children). When false (default), all processes
    /// are shown in a single flat list with the full ancestor chain in the name.
    #[serde(default)]
    pub proc_tree_mode: bool,
    /// Selected frontend color scheme id. Unknown values are rejected by the
    /// settings intent handler; the default preserves the pre-existing look.
    #[serde(default = "default_color_scheme")]
    pub color_scheme: String,
    /// How to extract .7z archives. `Auto` looks for `7z` / `7zz` / `7za` on
    /// PATH and falls back to the in-process Rust extractor if none is
    /// installed. `Custom` invokes whatever the user set in
    /// `sevenzip_custom_path`. `Internal` always uses the in-process path
    /// (slower for max-compression solid 7z, no native dependency).
    #[serde(default)]
    pub sevenzip_extractor: SevenzipExtractorMode,
    /// Absolute path to a 7-Zip binary, used only when `sevenzip_extractor`
    /// is `Custom`. Empty by default.
    #[serde(default)]
    pub sevenzip_custom_path: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SevenzipExtractorMode {
    /// Find `7z` / `7zz` / `7za` on $PATH; fall back to internal if none
    /// found. Default — matches what most users want.
    #[default]
    Auto,
    /// Use the explicit path in `sevenzip_custom_path`.
    Custom,
    /// Always use the in-process Rust extractor (sevenz-rust2).
    Internal,
}

impl SevenzipExtractorMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Custom => "custom",
            Self::Internal => "internal",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "custom" => Some(Self::Custom),
            "internal" => Some(Self::Internal),
            _ => None,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_max_image_size_mb() -> u64 {
    512
}

fn default_thumb_max_image_size_mb() -> u64 {
    100
}

fn default_thumb_video_frame_skip() -> u32 {
    5
}

fn default_color_scheme() -> String {
    "fbro-dark".to_string()
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            show_hidden_default: false,
            remember_window_state: true,
            preserve_attributes: true,
            auto_resume_copies: true,
            ludicrous_mode: false,
            max_image_size_mb: 512,
            save_panel_view_mode: false,
            enable_video_thumbnails: true,
            thumbnail_max_image_size_mb: 100,
            thumbnail_video_frame_skip: 5,
            stream_thumbnails: true,
            persist_panel_tabs: true,
            proc_tree_mode: false,
            color_scheme: default_color_scheme(),
            sevenzip_extractor: SevenzipExtractorMode::default(),
            sevenzip_custom_path: String::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modal_sizes_default_when_missing_from_settings_file() {
        let settings = Settings {
            general: GeneralSettings::default(),
            panel_view_modes: HashMap::new(),
            modal_sizes: HashMap::new(),
        };
        let mut value = serde_json::to_value(settings).unwrap();
        value.as_object_mut().unwrap().remove("modal_sizes");

        let decoded: Settings = serde_json::from_value(value).unwrap();

        assert!(decoded.modal_sizes.is_empty());
    }

    #[test]
    fn modal_sizes_round_trip_through_settings_file() {
        let mut modal_sizes = HashMap::new();
        modal_sizes.insert(
            "jobs".to_string(),
            ModalSizePreference {
                width: 720,
                height: 520,
            },
        );
        let settings = Settings {
            general: GeneralSettings::default(),
            panel_view_modes: HashMap::new(),
            modal_sizes,
        };

        let encoded = serde_json::to_string(&settings).unwrap();
        let decoded: Settings = serde_json::from_str(&encoded).unwrap();

        let size = decoded.modal_sizes.get("jobs").unwrap();
        assert_eq!(size.width, 720);
        assert_eq!(size.height, 520);
    }
}
