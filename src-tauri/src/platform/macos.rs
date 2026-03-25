use super::Platform;
use std::process::Command;

pub struct MacOSPlatform;

impl Platform for MacOSPlatform {
    fn home_dir(&self) -> String {
        std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
    }

    fn open_in_editor(&self, path: &str) -> Result<String, String> {
        let output = Command::new("open")
            .arg(path)
            .output()
            .map_err(|e| e.to_string())?;

        if output.status.success() {
            Ok(path.to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(stderr.trim().to_string())
        }
    }

    // find_icon: uses default (None) — falls back to embedded generic icons.
    // TODO: use NSWorkspace iconForFileType: for extension-based system icons.
}
