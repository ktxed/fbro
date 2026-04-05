use super::{executable_exists, find_binary_on_path, resolve_existing_executable};

#[test]
fn missing_binary_on_path_returns_none() {
    assert!(find_binary_on_path(&["fbro-definitely-missing-binary"]).is_none());
}

#[test]
fn executable_resolution_is_safe_for_missing_paths() {
    assert!(!executable_exists("/definitely/missing/fbro-binary"));
    assert!(resolve_existing_executable("/definitely/missing/fbro-binary").is_none());
}
