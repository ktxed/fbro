use std::env;
use std::path::PathBuf;

use crate::bro::find_binary_on_path;
use crate::state::settings::QuickTerminalEditor;

/// Names to try for Quick Edit, preferred first.
pub(crate) fn search_names(preferred: QuickTerminalEditor) -> Vec<&'static str> {
    #[cfg(not(windows))]
    {
        preferred
            .with_fallback()
            .into_iter()
            .map(QuickTerminalEditor::program)
            .collect()
    }
    #[cfg(windows)]
    {
        match preferred {
            QuickTerminalEditor::Nano => {
                vec!["nano", "vim", "nvim", "micro", "edit", "msedit"]
            }
            QuickTerminalEditor::Vim => {
                vec!["vim", "nvim", "nano", "micro", "edit", "msedit"]
            }
        }
    }
}

pub(crate) fn locate_editor(name: &str) -> Option<String> {
    if let Some(path) = find_binary_on_path(&[name]) {
        return Some(path);
    }
    extra_editor_paths(name)
        .into_iter()
        .find(|path| path.is_file())
        .map(|path| path.to_string_lossy().to_string())
}

fn extra_editor_paths(name: &str) -> Vec<PathBuf> {
    #[cfg(not(windows))]
    {
        let _ = name;
        Vec::new()
    }
    #[cfg(windows)]
    {
        let exe = format!("{name}.exe");
        let mut paths = Vec::new();
        let mut push_dir = |dir: PathBuf| paths.push(dir.join(&exe));

        if let Ok(pf) = env::var("ProgramFiles") {
            push_dir(PathBuf::from(pf).join("Git").join("usr").join("bin"));
        }
        if let Ok(pf) = env::var("ProgramFiles(x86)") {
            push_dir(PathBuf::from(pf).join("Git").join("usr").join("bin"));
        }
        if let Ok(local) = env::var("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            push_dir(
                local
                    .join("Programs")
                    .join("Git")
                    .join("usr")
                    .join("bin"),
            );
            push_dir(local.join("Microsoft").join("WinGet").join("Links"));
        }
        if let Ok(pd) = env::var("ProgramData") {
            push_dir(PathBuf::from(pd).join("chocolatey").join("bin"));
        }
        if let Ok(home) = env::var("USERPROFILE") {
            let home = PathBuf::from(home);
            push_dir(home.join("scoop").join("shims"));
            push_dir(home.join("scoop").join("apps").join(name).join("current"));
        }
        paths
    }
}

#[cfg(test)]
mod tests {
    use super::{locate_editor, search_names};
    use crate::state::settings::QuickTerminalEditor;

    #[test]
    fn nano_preference_tries_nano_before_vim() {
        let names = search_names(QuickTerminalEditor::Nano);
        assert_eq!(names[0], "nano");
        assert!(names.contains(&"vim"));
    }

    #[cfg(windows)]
    #[test]
    fn locates_git_for_windows_nano_when_installed() {
        let Some(path) = locate_editor("nano") else {
            return;
        };
        assert!(
            path.to_ascii_lowercase().ends_with("nano.exe"),
            "unexpected nano path: {path}"
        );
    }
}
