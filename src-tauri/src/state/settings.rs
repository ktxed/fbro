use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Settings {
    pub general: GeneralSettings,
    /// Missing command IDs use defaults; empty lists explicitly unassign.
    #[serde(default)]
    pub command_bindings: HashMap<String, Vec<crate::ws::protocol::KeyBindingSchema>>,
    /// Per-panel view mode ("list" or "grid"), keyed by panel_id. Only
    /// honoured when `general.save_panel_view_mode` is true.
    #[serde(default)]
    pub panel_view_modes: HashMap<String, String>,
    /// Per-modal preferred dimensions, keyed by stable frontend modal id.
    #[serde(default)]
    pub modal_sizes: HashMap<String, ModalSizePreference>,
    /// Start of the grace period during which location sorts are retained
    /// after the feature is disabled. Internal persistence metadata only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_sort_disabled_at_ms: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ModalSizePreference {
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct GeneralSettings {
    pub show_hidden_default: bool,
    /// Show the classic bottom command buttons without affecting shortcuts.
    #[serde(default = "default_true")]
    pub show_button_bar: bool,
    /// Deliver selected operation feedback through the OS while fbro is inactive.
    #[serde(default = "default_true")]
    pub os_notifications_enabled: bool,
    pub remember_window_state: bool,
    #[serde(default = "default_true")]
    pub preserve_attributes: bool,
    #[serde(default = "default_true")]
    pub auto_resume_copies: bool,
    #[serde(default)]
    pub ludicrous_mode: bool,
    /// Opt-in browser HEIC/HEIF preview conversion on Windows and Linux.
    #[serde(default)]
    pub enable_heic2any: bool,
    #[serde(default = "default_max_image_size_mb")]
    pub max_image_size_mb: u64,
    /// When true, each panel's grid/list mode is persisted across sessions.
    /// When false (default), every session starts with both panels in list.
    #[serde(default)]
    pub save_panel_view_mode: bool,
    /// Remember sorting independently for each local filesystem location and Journal.
    #[serde(default)]
    pub remember_sort_per_location: bool,
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
    /// Default wildcard name patterns Fedup Bro ignores when requested.
    /// Newline or comma separated. Matched case-insensitively against file and
    /// directory names.
    #[serde(default = "default_fedup_ignore_patterns")]
    pub fedup_default_ignore_patterns: String,
    /// Maximum size per text file for the built-in differ.
    #[serde(default = "default_compare_max_file_size_mb")]
    pub compare_max_file_size_mb: u64,
    /// Which differ to use for Compare files.
    #[serde(default)]
    pub compare_differ_mode: CompareDifferMode,
    /// External differ executable. fbro passes the two local file paths as args.
    #[serde(default)]
    pub compare_external_command: String,
    /// Terminal editor used by Quick Edit (nano/vim preference; Windows also probes Git, Scoop, Chocolatey, WinGet).
    #[serde(default)]
    pub quick_terminal_editor: QuickTerminalEditor,
    /// Start interactive zsh terminals with Emacs line-editing bindings.
    /// Other shells and Quick Edit commands keep their own keymaps.
    #[serde(default)]
    pub zsh_emacs_keymap: bool,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QuickTerminalEditor {
    #[default]
    Nano,
    Vim,
}

impl QuickTerminalEditor {
    pub fn program(self) -> &'static str {
        match self {
            Self::Nano => "nano",
            Self::Vim => "vim",
        }
    }

    pub fn with_fallback(self) -> [Self; 2] {
        match self {
            Self::Nano => [Self::Nano, Self::Vim],
            Self::Vim => [Self::Vim, Self::Nano],
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "nano" => Some(Self::Nano),
            "vim" => Some(Self::Vim),
            _ => None,
        }
    }
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

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CompareDifferMode {
    #[default]
    Internal,
    External,
    Auto,
}

impl CompareDifferMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::External => "external",
            Self::Auto => "auto",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "internal" => Some(Self::Internal),
            "external" => Some(Self::External),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
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

fn default_compare_max_file_size_mb() -> u64 {
    10
}

pub fn default_fedup_ignore_patterns() -> String {
    [
        ".git",
        ".idea",
        ".vscode",
        ".venv",
        "node_modules",
        "__MACOSX",
        "._*",
        ".DS_Store",
        "artifacts/e2e",
        "dist/assets",
        "target/debug",
        "target/release",
        "target/bundle",
        ".m2",
    ]
    .join("\n")
}

fn legacy_fedup_ignore_patterns() -> String {
    [".git", "node_modules", "__MACOSX", "._*", ".DS_Store"].join("\n")
}

fn normalize_fedup_ignore_patterns(value: &str) -> String {
    value
        .split(|c| c == '\n' || c == '\r' || c == ',')
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn migrate_settings(settings: &mut Settings) {
    if normalize_fedup_ignore_patterns(&settings.general.fedup_default_ignore_patterns)
        == legacy_fedup_ignore_patterns()
    {
        settings.general.fedup_default_ignore_patterns = default_fedup_ignore_patterns();
    }
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            show_hidden_default: false,
            show_button_bar: true,
            os_notifications_enabled: true,
            remember_window_state: true,
            preserve_attributes: true,
            auto_resume_copies: true,
            ludicrous_mode: false,
            enable_heic2any: false,
            max_image_size_mb: 512,
            save_panel_view_mode: false,
            remember_sort_per_location: false,
            enable_video_thumbnails: true,
            thumbnail_max_image_size_mb: 100,
            thumbnail_video_frame_skip: 5,
            stream_thumbnails: true,
            persist_panel_tabs: true,
            proc_tree_mode: false,
            color_scheme: default_color_scheme(),
            sevenzip_extractor: SevenzipExtractorMode::default(),
            sevenzip_custom_path: String::new(),
            fedup_default_ignore_patterns: default_fedup_ignore_patterns(),
            compare_max_file_size_mb: default_compare_max_file_size_mb(),
            compare_differ_mode: CompareDifferMode::default(),
            compare_external_command: String::new(),
            quick_terminal_editor: QuickTerminalEditor::default(),
            zsh_emacs_keymap: false,
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
    let mut settings: Settings = serde_json::from_str(&content).unwrap_or_default();
    migrate_settings(&mut settings);
    settings
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    save_to_path(settings, &settings_path())
}

pub(crate) fn save_to_path(settings: &Settings, path: &std::path::Path) -> std::io::Result<()> {
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
    fn button_bar_is_visible_for_existing_settings_files() {
        let mut encoded = serde_json::to_value(Settings::default()).unwrap();
        encoded["general"]
            .as_object_mut()
            .unwrap()
            .remove("show_button_bar");
        let decoded: Settings = serde_json::from_value(encoded).unwrap();

        assert_eq!(
            serde_json::to_value(decoded).unwrap()["general"]["show_button_bar"],
            true
        );
    }

    #[test]
    fn button_bar_visibility_survives_settings_roundtrip() {
        for visible in [false, true] {
            let mut encoded = serde_json::to_value(Settings::default()).unwrap();
            encoded["general"]["show_button_bar"] = serde_json::json!(visible);
            let decoded: Settings = serde_json::from_value(encoded).unwrap();
            let saved = serde_json::to_string(&decoded).unwrap();
            let reloaded: Settings = serde_json::from_str(&saved).unwrap();

            assert_eq!(
                serde_json::to_value(reloaded).unwrap()["general"]["show_button_bar"],
                visible
            );
        }
    }

    #[test]
    fn modal_sizes_default_when_missing_from_settings_file() {
        let settings = Settings {
            general: GeneralSettings::default(),
            panel_view_modes: HashMap::new(),
            modal_sizes: HashMap::new(),
            location_sort_disabled_at_ms: None,
            command_bindings: HashMap::new(),
        };
        let mut value = serde_json::to_value(settings).unwrap();
        value.as_object_mut().unwrap().remove("modal_sizes");

        let decoded: Settings = serde_json::from_value(value).unwrap();

        assert!(decoded.modal_sizes.is_empty());
    }

    #[test]
    fn location_sort_memory_defaults_off_for_existing_settings() {
        let settings = Settings::default();
        let mut value = serde_json::to_value(settings).unwrap();
        value["general"]
            .as_object_mut()
            .unwrap()
            .remove("remember_sort_per_location");
        value
            .as_object_mut()
            .unwrap()
            .remove("location_sort_disabled_at_ms");

        let decoded: Settings = serde_json::from_value(value).unwrap();

        assert!(!decoded.general.remember_sort_per_location);
        assert_eq!(decoded.location_sort_disabled_at_ms, None);
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
            location_sort_disabled_at_ms: None,
            command_bindings: HashMap::new(),
        };

        let encoded = serde_json::to_string(&settings).unwrap();
        let decoded: Settings = serde_json::from_str(&encoded).unwrap();

        let size = decoded.modal_sizes.get("jobs").unwrap();
        assert_eq!(size.width, 720);
        assert_eq!(size.height, 520);
    }

    #[test]
    fn old_fedup_default_ignore_patterns_migrate_to_current_default() {
        let mut settings = Settings {
            general: GeneralSettings {
                fedup_default_ignore_patterns: legacy_fedup_ignore_patterns(),
                ..GeneralSettings::default()
            },
            panel_view_modes: HashMap::new(),
            modal_sizes: HashMap::new(),
            location_sort_disabled_at_ms: None,
            command_bindings: HashMap::new(),
        };

        migrate_settings(&mut settings);

        assert_eq!(
            settings.general.fedup_default_ignore_patterns,
            default_fedup_ignore_patterns()
        );
    }

    #[test]
    fn customized_fedup_ignore_patterns_do_not_migrate() {
        let custom = format!("{}\ncustom-cache", legacy_fedup_ignore_patterns());
        let mut settings = Settings {
            general: GeneralSettings {
                fedup_default_ignore_patterns: custom.clone(),
                ..GeneralSettings::default()
            },
            panel_view_modes: HashMap::new(),
            modal_sizes: HashMap::new(),
            location_sort_disabled_at_ms: None,
            command_bindings: HashMap::new(),
        };

        migrate_settings(&mut settings);

        assert_eq!(settings.general.fedup_default_ignore_patterns, custom);
    }

    #[test]
    fn quick_terminal_editor_uses_the_other_supported_editor_as_fallback() {
        assert_eq!(
            QuickTerminalEditor::Nano.with_fallback(),
            [QuickTerminalEditor::Nano, QuickTerminalEditor::Vim]
        );
        assert_eq!(
            QuickTerminalEditor::Vim.with_fallback(),
            [QuickTerminalEditor::Vim, QuickTerminalEditor::Nano]
        );
        assert_eq!(QuickTerminalEditor::from_str("nano"), Some(QuickTerminalEditor::Nano));
        assert_eq!(QuickTerminalEditor::from_str("vim"), Some(QuickTerminalEditor::Vim));
        assert_eq!(QuickTerminalEditor::from_str("emacs"), None);
    }

    #[test]
    fn os_notifications_default_enabled_for_legacy_settings_and_preserve_opt_out() {
        let defaults = serde_json::to_value(GeneralSettings::default()).unwrap();
        assert_eq!(defaults["os_notifications_enabled"], true);
        let mut legacy = defaults.clone();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("os_notifications_enabled");
        let restored: GeneralSettings = serde_json::from_value(legacy).unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap()["os_notifications_enabled"],
            true
        );
        let mut disabled = defaults.clone();
        disabled["os_notifications_enabled"] = serde_json::json!(false);
        let restored: GeneralSettings = serde_json::from_value(disabled).unwrap();
        assert_eq!(
            serde_json::to_value(restored).unwrap()["os_notifications_enabled"],
            false
        );
        let mut invalid = defaults;
        invalid["os_notifications_enabled"] = serde_json::json!("false");
        assert!(serde_json::from_value::<GeneralSettings>(invalid).is_err());
    }

    #[test]
    fn heic_preview_is_opt_in_and_survives_settings_roundtrip() {
        let mut encoded = serde_json::to_value(GeneralSettings::default()).unwrap();
        encoded.as_object_mut().unwrap().remove("enable_heic2any");
        let legacy: GeneralSettings = serde_json::from_value(encoded).unwrap();
        assert!(!legacy.enable_heic2any);
        let enabled = GeneralSettings { enable_heic2any: true, ..legacy };
        let decoded: GeneralSettings = serde_json::from_str(
            &serde_json::to_string(&enabled).unwrap()).unwrap();
        assert!(decoded.enable_heic2any);
    }
}
