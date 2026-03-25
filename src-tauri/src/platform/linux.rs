use super::Platform;
use std::process::Command;

pub struct LinuxPlatform;

impl Platform for LinuxPlatform {
    fn open_in_editor(&self, path: &str) -> Result<String, String> {
        let output = Command::new("xdg-open")
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

    fn find_icon(&self, icon_name: &str) -> Option<(Vec<u8>, &'static str)> {
        const THEMES: &[&str] = &["Pop", "Adwaita", "hicolor"];
        const SIZES: &[&str] = &["48x48", "scalable", "32x32", "24x24"];
        const CATS: &[&str] = &["mimetypes", "places", "emblems"];

        for theme in THEMES {
            for size in SIZES {
                for cat in CATS {
                    let svg = format!("/usr/share/icons/{theme}/{size}/{cat}/{icon_name}.svg");
                    if let Ok(data) = std::fs::read(&svg) {
                        return Some((data, "image/svg+xml"));
                    }
                    let png = format!("/usr/share/icons/{theme}/{size}/{cat}/{icon_name}.png");
                    if let Ok(data) = std::fs::read(&png) {
                        return Some((data, "image/png"));
                    }
                }
            }
        }
        None
    }

    fn home_dir(&self) -> String {
        // Prefer $HOME; fall back to /etc/passwd entry for current uid
        if let Ok(h) = std::env::var("HOME") {
            if !h.is_empty() {
                return h;
            }
        }

        // Fallback: parse /etc/passwd for current uid
        if let Ok(uid) = std::env::var("UID")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or(())
            .or_else(|_| -> Result<u32, ()> {
                // SAFETY: getuid() is always safe to call
                #[cfg(unix)]
                {
                    Ok(libc_uid())
                }
                #[cfg(not(unix))]
                Err(())
            })
        {
            if let Ok(contents) = std::fs::read_to_string("/etc/passwd") {
                for line in contents.lines() {
                    let fields: Vec<&str> = line.split(':').collect();
                    if fields.len() >= 6 {
                        if fields[2].parse::<u32>().ok() == Some(uid) {
                            return fields[5].to_string();
                        }
                    }
                }
            }
        }

        "/".to_string()
    }
}

#[cfg(unix)]
fn libc_uid() -> u32 {
    // Use std::os::unix instead of pulling in libc crate
    // nix syscall via inline assembly is overkill; use /proc/self/status
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                if let Some(uid_str) = rest.split_whitespace().next() {
                    if let Ok(uid) = uid_str.parse::<u32>() {
                        return uid;
                    }
                }
            }
        }
    }
    u32::MAX
}
