use super::{executable_exists, find_binary_on_path, resolve_existing_executable};

#[test]
fn missing_binary_on_path_returns_none() {
    assert!(find_binary_on_path(&["fbro-definitely-missing-binary"]).is_none());
}

#[cfg(windows)]
#[test]
fn finds_windows_system_binary_from_bare_name() {
    let found = find_binary_on_path(&["cmd"]).expect("cmd.exe should be on PATH");
    assert!(
        found.to_ascii_lowercase().ends_with("cmd.exe"),
        "unexpected cmd path: {found}"
    );
}

#[cfg(windows)]
#[test]
fn finds_tar_exe_from_bare_name() {
    let Some(found) = find_binary_on_path(&["tar"]) else {
        return;
    };
    assert!(
        found.to_ascii_lowercase().ends_with("tar.exe"),
        "unexpected tar path: {found}"
    );
}

#[test]
fn executable_resolution_is_safe_for_missing_paths() {
    assert!(!executable_exists("/definitely/missing/fbro-binary"));
    assert!(resolve_existing_executable("/definitely/missing/fbro-binary").is_none());
}
