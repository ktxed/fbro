use super::{macos_mpv_bundle_candidates, probe_mpv_on_path};

#[test]
fn includes_global_applications_bundle_path() {
    let candidates = macos_mpv_bundle_candidates();
    assert!(candidates
        .iter()
        .any(|path| path == "/Applications/mpv.app"));
}

#[test]
fn path_probe_is_safe_when_mpv_is_missing() {
    let _ = probe_mpv_on_path();
}
