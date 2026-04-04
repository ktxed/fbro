use crate::bro::probe::resolve_existing_executable;
use std::path::PathBuf;

pub(crate) fn macos_app_exists(bundle_path: &str) -> bool {
    PathBuf::from(bundle_path).is_dir()
}

pub(crate) fn macos_app_executable(bundle_path: &str, executable_name: &str) -> Option<String> {
    if !macos_app_exists(bundle_path) {
        return None;
    }
    let executable = PathBuf::from(bundle_path)
        .join("Contents")
        .join("MacOS")
        .join(executable_name);
    resolve_existing_executable(executable.to_string_lossy().as_ref())
}

pub(crate) fn macos_common_cli_candidates(binary_names: &[&str]) -> Vec<String> {
    let mut candidates = Vec::new();
    for root in ["/opt/homebrew/bin", "/usr/local/bin"] {
        for binary in binary_names {
            candidates.push(format!("{root}/{binary}"));
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            for binary in binary_names {
                candidates.push(format!("{home}/homebrew/bin/{binary}"));
            }
        }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::{macos_app_executable, macos_app_exists, macos_common_cli_candidates};

    #[test]
    fn missing_bundle_is_safe() {
        assert!(!macos_app_exists("/Applications/fbro-missing.app"));
        assert!(macos_app_executable("/Applications/fbro-missing.app", "fbro").is_none());
    }

    #[test]
    fn includes_common_homebrew_cli_candidates() {
        let candidates = macos_common_cli_candidates(&["7z", "7zz"]);
        assert!(candidates.iter().any(|path| path == "/opt/homebrew/bin/7zz"));
        assert!(candidates.iter().any(|path| path == "/usr/local/bin/7z"));
    }
}
