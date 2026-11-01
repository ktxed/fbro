//! Custom color schemes: TOML files in `<config_dir>/themes/`.
//!
//! One file per scheme, id `custom:<file stem>`. The backend owns validation so
//! the frontend can safely render the values inside a generated `<style>`:
//! only hex and `rgb()`/`rgba()` colours and `[a-z][a-z0-9-]*` token names are
//! accepted. See `spex/settings.md` ("Custom color schemes").

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const CUSTOM_PREFIX: &str = "custom:";
pub const FORMAT_VERSION: u32 = 1;
const MAX_NAME: usize = 60;
const MAX_DESCRIPTION: usize = 200;
const MAX_COLORS: usize = 512;

pub const BASE_DARK: &str = "fbro-dark";
pub const BASE_LIGHT: &str = "daylight";

pub fn themes_dir() -> PathBuf {
    crate::config::config_dir().join("themes")
}

pub fn is_builtin(id: &str) -> bool {
    id == BASE_DARK || id == BASE_LIGHT
}

/// Built-in terminal palette, used for terminal keys a theme leaves out.
fn builtin_terminal(base: &str) -> [&'static str; 4] {
    if base == BASE_LIGHT {
        ["#ffffff", "#1e2933", "#2f6fb3", "#cfe3ff"]
    } else {
        ["#1e1e1e", "#dddddd", "#d6e8ff", "#2d228a"]
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct TerminalColors {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_background: Option<String>,
}

/// On-disk representation.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Theme {
    #[serde(default = "default_version")]
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_base")]
    pub base: String,
    #[serde(default)]
    pub colors: BTreeMap<String, String>,
    #[serde(default)]
    pub terminal: TerminalColors,
}

fn default_version() -> u32 {
    FORMAT_VERSION
}
fn default_mode() -> String {
    "dark".to_string()
}
fn default_base() -> String {
    BASE_DARK.to_string()
}

/// A loaded, validated theme plus its identity.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeEntry {
    pub slug: String,
    pub theme: Theme,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeError {
    pub file: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemeCatalog {
    pub items: Vec<ThemeEntry>,
    pub errors: Vec<ThemeError>,
}

impl ThemeCatalog {
    pub fn get(&self, id: &str) -> Option<&ThemeEntry> {
        let slug = id.strip_prefix(CUSTOM_PREFIX)?;
        self.items.iter().find(|entry| entry.slug == slug)
    }

    /// Wire projection (`general.colorSchemes`). Terminal colours are resolved
    /// against the base so clients never merge.
    pub fn to_wire(&self, dir: &Path) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|entry| {
                let t = &entry.theme;
                let defaults = builtin_terminal(&t.base);
                json!({
                    "id": format!("{CUSTOM_PREFIX}{}", entry.slug),
                    "name": t.name,
                    "description": t.description,
                    "mode": t.mode,
                    "base": t.base,
                    "colors": t.colors,
                    "terminal": {
                        "background": t.terminal.background.as_deref().unwrap_or(defaults[0]),
                        "foreground": t.terminal.foreground.as_deref().unwrap_or(defaults[1]),
                        "cursor": t.terminal.cursor.as_deref().unwrap_or(defaults[2]),
                        "selectionBackground": t.terminal.selection_background.as_deref().unwrap_or(defaults[3]),
                    },
                })
            })
            .collect();
        let errors: Vec<Value> = self
            .errors
            .iter()
            .map(|e| json!({ "file": e.file, "message": e.message }))
            .collect();
        json!({ "dir": dir.to_string_lossy(), "items": items, "errors": errors })
    }
}

fn color_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(concat!(
            r"^(?:#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})",
            r"|rgba?\(\s*\d{1,3}(?:\.\d+)?\s*,\s*\d{1,3}(?:\.\d+)?\s*,\s*\d{1,3}(?:\.\d+)?\s*(?:,\s*(?:\d+(?:\.\d+)?|\.\d+)\s*)?\))$"
        ))
        .expect("color regex")
    })
}

fn key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9-]{0,63}$").expect("key regex"))
}

fn slug_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$").expect("slug regex"))
}

pub fn is_valid_slug(slug: &str) -> bool {
    slug_re().is_match(slug)
}

/// Slug of a `custom:<slug>` id, if well formed.
pub fn slug_of(id: &str) -> Option<&str> {
    id.strip_prefix(CUSTOM_PREFIX).filter(|s| is_valid_slug(s))
}

pub fn validate_color(value: &str) -> Result<(), String> {
    if color_re().is_match(value.trim()) {
        Ok(())
    } else {
        Err(format!("Unsupported color value: {value:?} (use #rrggbb[aa] or rgb()/rgba())"))
    }
}

pub fn validate_theme(theme: &Theme) -> Result<(), String> {
    let name = theme.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(format!("Name must be 1–{MAX_NAME} characters"));
    }
    if theme.description.chars().count() > MAX_DESCRIPTION {
        return Err(format!("Description is longer than {MAX_DESCRIPTION} characters"));
    }
    if theme.mode != "dark" && theme.mode != "light" {
        return Err("mode must be \"dark\" or \"light\"".to_string());
    }
    if !is_builtin(&theme.base) {
        return Err(format!("base must be \"{BASE_DARK}\" or \"{BASE_LIGHT}\""));
    }
    if theme.colors.len() > MAX_COLORS {
        return Err("Too many colors".to_string());
    }
    for (key, value) in &theme.colors {
        if !key_re().is_match(key) {
            return Err(format!("Invalid color name: {key:?}"));
        }
        validate_color(value).map_err(|e| format!("{key}: {e}"))?;
    }
    let t = &theme.terminal;
    for (key, value) in [
        ("terminal.background", &t.background),
        ("terminal.foreground", &t.foreground),
        ("terminal.cursor", &t.cursor),
        ("terminal.selection_background", &t.selection_background),
    ] {
        if let Some(value) = value {
            validate_color(value).map_err(|e| format!("{key}: {e}"))?;
        }
    }
    Ok(())
}

/// Parse `#rgb[a]`, `#rrggbb[aa]`, `rgb()` or `rgba()` into RGBA bytes.
pub fn parse_color(value: &str) -> Option<(u8, u8, u8, u8)> {
    let value = value.trim();
    if !color_re().is_match(value) {
        return None;
    }
    if let Some(hex) = value.strip_prefix('#') {
        let digit = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|d| d * 17);
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        return match hex.len() {
            3 => Some((digit(0)?, digit(1)?, digit(2)?, 255)),
            4 => Some((digit(0)?, digit(1)?, digit(2)?, digit(3)?)),
            6 => Some((byte(0)?, byte(2)?, byte(4)?, 255)),
            8 => Some((byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        };
    }
    let inner = value.split_once('(')?.1.strip_suffix(')')?;
    let parts: Vec<f32> = inner.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    let channel = |v: f32| v.round().clamp(0.0, 255.0) as u8;
    match parts.as_slice() {
        [r, g, b] => Some((channel(*r), channel(*g), channel(*b), 255)),
        [r, g, b, a] => Some((channel(*r), channel(*g), channel(*b), channel(a.clamp(0.0, 1.0) * 255.0))),
        _ => None,
    }
}

/// Slug for a new theme: lowercase alphanumerics and dashes, unique in `taken`.
pub fn unique_slug(name: &str, taken: &[String]) -> String {
    let mut slug = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let mut slug = slug.trim_matches('-').chars().take(40).collect::<String>();
    if slug.is_empty() {
        slug = "scheme".to_string();
    }
    if !taken.contains(&slug) {
        return slug;
    }
    (2..)
        .map(|n| format!("{slug}-{n}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("unbounded range")
}

pub fn read_theme(path: &Path) -> Result<Theme, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("Cannot read file: {e}"))?;
    let theme: Theme = toml::from_str(&content).map_err(|e| format!("Invalid TOML: {}", e.message()))?;
    validate_theme(&theme)?;
    Ok(theme)
}

/// Load every `*.toml` in `dir`. Broken files become `errors`, never entries.
pub fn load_all_from(dir: &Path) -> ThemeCatalog {
    let mut catalog = ThemeCatalog::default();
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return catalog;
    };
    let mut files: Vec<PathBuf> = read_dir
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();
    for path in files {
        let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let stem = path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if !is_valid_slug(&stem) {
            catalog.errors.push(ThemeError {
                file,
                message: "File name must use lowercase letters, digits, - or _ (rename it)".to_string(),
            });
            continue;
        }
        match read_theme(&path) {
            Ok(theme) => catalog.items.push(ThemeEntry { slug: stem, theme }),
            Err(message) => catalog.errors.push(ThemeError { file, message }),
        }
    }
    catalog.items.sort_by(|a, b| a.theme.name.to_lowercase().cmp(&b.theme.name.to_lowercase()));
    catalog
}

pub fn load_all() -> ThemeCatalog {
    load_all_from(&themes_dir())
}

/// Existing file stems (valid or not), so new slugs never overwrite anything.
pub fn taken_slugs(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Atomic write (tmp + rename), like `bookmarks::save`.
pub fn save_to(dir: &Path, slug: &str, theme: &Theme) -> io::Result<()> {
    if !is_valid_slug(slug) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid theme file name"));
    }
    std::fs::create_dir_all(dir)?;
    let content = toml::to_string(theme).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    let path = dir.join(format!("{slug}.toml"));
    let tmp = dir.join(format!("{slug}.toml.tmp"));
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn delete_from(dir: &Path, slug: &str) -> io::Result<()> {
    if !is_valid_slug(slug) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid theme file name"));
    }
    std::fs::remove_file(dir.join(format!("{slug}.toml")))
}

/// Native window background for a scheme id: the theme's `bg` (else its base's).
pub fn window_background(id: &str, catalog: &ThemeCatalog) -> (u8, u8, u8, u8) {
    let builtin = |base: &str| {
        if base == BASE_LIGHT {
            (244, 246, 248, 255)
        } else {
            (13, 13, 13, 255)
        }
    };
    match catalog.get(id) {
        Some(entry) => entry
            .theme
            .colors
            .get("bg")
            .and_then(|v| parse_color(v))
            .map(|(r, g, b, _)| (r, g, b, 255))
            .unwrap_or_else(|| builtin(&entry.theme.base)),
        None => builtin(id),
    }
}

/// Watch `themes/` and call `on_change` (debounced) when files change, so
/// copying a `.toml` into the folder imports it. Returns the watcher; dropping
/// it stops watching.
pub fn watch(
    dir: PathBuf,
    on_change: tokio::sync::mpsc::UnboundedSender<()>,
) -> Result<notify::RecommendedWatcher, String> {
    use notify::{RecursiveMode, Watcher};
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok() {
            let _ = on_change.send(());
        }
    })
    .map_err(|e| format!("themes watcher: {e}"))?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|e| format!("watch {}: {e}", dir.display()))?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("fbro-themes-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample() -> Theme {
        Theme {
            version: 1,
            name: "Midnight".into(),
            description: "Based on fbro Dark".into(),
            mode: "dark".into(),
            base: BASE_DARK.into(),
            colors: BTreeMap::from([
                ("bg".to_string(), "#0d0d0d".to_string()),
                ("bg-selected".to_string(), "rgba(220, 170, 20, 0.30)".to_string()),
            ]),
            terminal: TerminalColors { cursor: Some("#fff".into()), ..Default::default() },
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new("roundtrip");
        save_to(&dir.0, "midnight", &sample()).unwrap();
        let catalog = load_all_from(&dir.0);
        assert!(catalog.errors.is_empty(), "{:?}", catalog.errors);
        assert_eq!(catalog.items.len(), 1);
        assert_eq!(catalog.items[0].theme, sample());
        assert!(catalog.get("custom:midnight").is_some());
        assert!(catalog.get("midnight").is_none());
        assert!(!dir.0.join("midnight.toml.tmp").exists());
    }

    #[test]
    fn color_validation_rejects_css_injection() {
        for ok in ["#fff", "#ffff", "#ffffff", "#ffffff80", "rgb(1, 2, 3)", "rgba(1,2,3,.5)", "rgba(1, 2, 3, 0.5)"] {
            assert!(validate_color(ok).is_ok(), "{ok}");
        }
        for bad in ["red", "red;}", "url(http://x)", "#ggg", "#12345", "rgb(1,2)", "rgba(1,2,3,4,5)", "#fff;}", "var(--x)", "", "rgb(1,2,3) !important"] {
            assert!(validate_color(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn theme_validation_checks_keys_mode_and_base() {
        let mut t = sample();
        t.colors.insert("Bad Key".into(), "#fff".into());
        assert!(validate_theme(&t).is_err());
        let mut t = sample();
        t.colors.insert("bg".into(), "red;}".into());
        assert!(validate_theme(&t).is_err());
        let mut t = sample();
        t.mode = "sepia".into();
        assert!(validate_theme(&t).is_err());
        let mut t = sample();
        t.base = "custom:other".into();
        assert!(validate_theme(&t).is_err());
        let mut t = sample();
        t.terminal.cursor = Some("expression(1)".into());
        assert!(validate_theme(&t).is_err());
        let mut t = sample();
        t.name = "  ".into();
        assert!(validate_theme(&t).is_err());
    }

    #[test]
    fn partial_hand_written_file_inherits_from_base() {
        let dir = TempDir::new("partial");
        std::fs::write(dir.0.join("mine.toml"), "name = \"Mine\"\nmode = \"light\"\nbase = \"daylight\"\n[colors]\nbg = \"#fafafa\"\n").unwrap();
        let catalog = load_all_from(&dir.0);
        assert_eq!(catalog.items.len(), 1);
        let wire = catalog.to_wire(&dir.0);
        let item = &wire["items"][0];
        assert_eq!(item["id"], "custom:mine");
        assert_eq!(item["colors"]["bg"], "#fafafa");
        assert!(item["colors"].get("bg-panel").is_none());
        // Missing terminal colours resolve to the light base.
        assert_eq!(item["terminal"]["background"], "#ffffff");
    }

    #[test]
    fn broken_files_are_reported_not_loaded_or_rewritten() {
        let dir = TempDir::new("broken");
        std::fs::write(dir.0.join("garbage.toml"), "this is = = not toml").unwrap();
        std::fs::write(dir.0.join("evil.toml"), "name = \"E\"\n[colors]\nbg = \"red;}\"\n").unwrap();
        std::fs::write(dir.0.join("Bad Name.toml"), "name = \"B\"\n").unwrap();
        std::fs::write(dir.0.join("notes.txt"), "ignored").unwrap();
        let before = std::fs::read_to_string(dir.0.join("garbage.toml")).unwrap();
        let catalog = load_all_from(&dir.0);
        assert!(catalog.items.is_empty());
        let files: Vec<_> = catalog.errors.iter().map(|e| e.file.as_str()).collect();
        assert_eq!(files, ["Bad Name.toml", "evil.toml", "garbage.toml"]);
        assert_eq!(std::fs::read_to_string(dir.0.join("garbage.toml")).unwrap(), before);
    }

    #[test]
    fn slugs_are_sanitized_and_unique() {
        assert_eq!(unique_slug("Midnight Blue!", &[]), "midnight-blue");
        assert_eq!(unique_slug("  ", &[]), "scheme");
        assert_eq!(unique_slug("../../etc", &[]), "etc");
        let taken = vec!["midnight".to_string(), "midnight-2".to_string()];
        assert_eq!(unique_slug("Midnight", &taken), "midnight-3");
        assert!(is_valid_slug(&unique_slug("Ünïcode ☃", &[])));
    }

    #[test]
    fn save_and_delete_reject_path_traversal() {
        let dir = TempDir::new("traversal");
        assert!(save_to(&dir.0, "../escape", &sample()).is_err());
        assert!(delete_from(&dir.0, "../escape").is_err());
        assert_eq!(slug_of("custom:../x"), None);
        assert_eq!(slug_of("custom:ok-1"), Some("ok-1"));
        assert_eq!(slug_of("fbro-dark"), None);
    }

    #[test]
    fn parse_color_handles_all_forms() {
        assert_eq!(parse_color("#0d0d0d"), Some((13, 13, 13, 255)));
        assert_eq!(parse_color("#fff"), Some((255, 255, 255, 255)));
        assert_eq!(parse_color("#ff000080"), Some((255, 0, 0, 128)));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some((1, 2, 3, 255)));
        assert_eq!(parse_color("rgba(0,0,0,0.5)"), Some((0, 0, 0, 128)));
        assert_eq!(parse_color("nope"), None);
    }

    #[test]
    fn window_background_uses_theme_bg_then_base() {
        let mut catalog = ThemeCatalog::default();
        catalog.items.push(ThemeEntry { slug: "a".into(), theme: sample() });
        let mut light = sample();
        light.base = BASE_LIGHT.into();
        light.colors.clear();
        catalog.items.push(ThemeEntry { slug: "b".into(), theme: light });
        assert_eq!(window_background("custom:a", &catalog), (13, 13, 13, 255));
        assert_eq!(window_background("custom:b", &catalog), (244, 246, 248, 255));
        assert_eq!(window_background("custom:missing", &catalog), (13, 13, 13, 255));
        assert_eq!(window_background("daylight", &catalog), (244, 246, 248, 255));
    }
}
