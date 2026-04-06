use super::{macos_app_executable, macos_app_exists, macos_common_cli_candidates};

#[test]
fn missing_bundle_is_safe() {
    assert!(!macos_app_exists("/Applications/fbro-missing.app"));
    assert!(macos_app_executable("/Applications/fbro-missing.app", "fbro").is_none());
}

#[test]
fn includes_common_homebrew_cli_candidates() {
    let candidates = macos_common_cli_candidates(&["7z", "7zz"]);
    assert!(candidates
        .iter()
        .any(|path| path == "/opt/homebrew/bin/7zz"));
    assert!(candidates.iter().any(|path| path == "/usr/local/bin/7z"));
}
