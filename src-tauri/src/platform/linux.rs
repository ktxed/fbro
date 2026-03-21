use super::Platform;

pub struct LinuxPlatform;

impl Platform for LinuxPlatform {
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
