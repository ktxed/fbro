use std::process::Command;

pub(crate) fn flatpak_app_installed(app_id: &str) -> bool {
    Command::new("flatpak")
        .arg("info")
        .arg("--show-ref")
        .arg(app_id)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
