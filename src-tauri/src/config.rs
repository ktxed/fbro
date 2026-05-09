use std::path::PathBuf;

/// Returns the fbro config directory for the current platform.
///
/// - Windows: `%USERPROFILE%\.config\fbro`  (XDG-style in home, consistent with dotfile tools)
/// - Unix:    `$XDG_CONFIG_HOME/fbro`  or  `$HOME/.config/fbro`
pub fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        let home = std::env::var("USERPROFILE").unwrap_or_else(|_| {
            std::env::var("HOMEDRIVE")
                .and_then(|d| std::env::var("HOMEPATH").map(|p| format!("{d}{p}")))
                .unwrap_or_else(|_| "C:\\Users\\Default".to_string())
        });
        PathBuf::from(home).join(".config").join("fbro")
    }
    #[cfg(not(windows))]
    {
        let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
            format!("{home}/.config")
        });
        PathBuf::from(base).join("fbro")
    }
}
