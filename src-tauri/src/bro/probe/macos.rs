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

#[cfg(test)]
mod tests {
    use super::{macos_app_executable, macos_app_exists};

    #[test]
    fn missing_bundle_is_safe() {
        assert!(!macos_app_exists("/Applications/fbro-missing.app"));
        assert!(macos_app_executable("/Applications/fbro-missing.app", "fbro").is_none());
    }
}
