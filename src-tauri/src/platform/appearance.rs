//! System light/dark appearance: shared types and the pure value mappings
//! used by the per-OS watchers. Kept free of `#[cfg]` so the mappings are
//! unit-tested on every host.

/// The OS-level light/dark preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemColorScheme {
    Dark,
    Light,
}

impl SystemColorScheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// Latest known OS preference; `None` while detection is unavailable.
pub type SystemColorSchemeRx = tokio::sync::watch::Receiver<Option<SystemColorScheme>>;

/// freedesktop `org.freedesktop.appearance` `color-scheme`: 1 prefers dark,
/// 2 prefers light, 0 is "no preference" — which is what GNOME's default
/// (light) style reports, so it maps to light like libadwaita does.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn portal_color_scheme(value: u32) -> Option<SystemColorScheme> {
    match value {
        1 => Some(SystemColorScheme::Dark),
        0 | 2 => Some(SystemColorScheme::Light),
        _ => None,
    }
}

/// Windows `AppsUseLightTheme`: 0 is dark. A missing value predates the app
/// dark mode (Windows 10 1809) and is therefore light.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn windows_apps_use_light_theme(value: Option<u32>) -> SystemColorScheme {
    match value {
        Some(0) => SystemColorScheme::Dark,
        _ => SystemColorScheme::Light,
    }
}

/// macOS `AppleInterfaceStyle` user default: "Dark" when dark (including the
/// dark phase of Auto); absent in light mode.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn macos_interface_style(value: Option<&str>) -> SystemColorScheme {
    match value {
        Some(style) if style.eq_ignore_ascii_case("dark") => SystemColorScheme::Dark,
        _ => SystemColorScheme::Light,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_values_map_to_schemes() {
        assert_eq!(portal_color_scheme(0), Some(SystemColorScheme::Light));
        assert_eq!(portal_color_scheme(1), Some(SystemColorScheme::Dark));
        assert_eq!(portal_color_scheme(2), Some(SystemColorScheme::Light));
        assert_eq!(portal_color_scheme(3), None);
    }

    #[test]
    fn windows_registry_values_map_to_schemes() {
        assert_eq!(windows_apps_use_light_theme(Some(0)), SystemColorScheme::Dark);
        assert_eq!(windows_apps_use_light_theme(Some(1)), SystemColorScheme::Light);
        assert_eq!(windows_apps_use_light_theme(None), SystemColorScheme::Light);
    }

    #[test]
    fn macos_interface_style_maps_to_schemes() {
        assert_eq!(macos_interface_style(Some("Dark")), SystemColorScheme::Dark);
        assert_eq!(macos_interface_style(None), SystemColorScheme::Light);
        assert_eq!(macos_interface_style(Some("Light")), SystemColorScheme::Light);
    }

    #[test]
    fn scheme_wire_names() {
        assert_eq!(SystemColorScheme::Dark.as_str(), "dark");
        assert_eq!(SystemColorScheme::Light.as_str(), "light");
    }
}
