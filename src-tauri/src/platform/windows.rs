use std::process::Command;
use super::{Platform, VolumeInfo};

pub struct WindowsPlatform;

// ── WinAPI declarations ──────────────────────────────────────────────────────

#[link(name = "kernel32")]
extern "system" {
    fn GetDiskFreeSpaceExW(
        lpDirectoryName: *const u16,
        lpFreeBytesAvailableToCaller: *mut u64,
        lpTotalNumberOfBytes: *mut u64,
        lpTotalNumberOfFreeBytes: *mut u64,
    ) -> i32;

    fn GetLogicalDrives() -> u32;

    fn GetVolumeInformationW(
        lpRootPathName: *const u16,
        lpVolumeNameBuffer: *mut u16,
        nVolumeNameSize: u32,
        lpVolumeSerialNumber: *mut u32,
        lpMaximumComponentLength: *mut u32,
        lpFileSystemFlags: *mut u32,
        lpFileSystemNameBuffer: *mut u16,
        nFileSystemNameSize: u32,
    ) -> i32;
}

/// Encode a Rust string as a null-terminated UTF-16 buffer for WinAPI calls.
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Platform for WindowsPlatform {
    fn home_dir(&self) -> String {
        std::env::var("USERPROFILE")
            .or_else(|_| {
                std::env::var("HOMEDRIVE").and_then(|d| {
                    std::env::var("HOMEPATH").map(|p| format!("{d}{p}"))
                })
            })
            .map(|p| p.replace('\\', "/"))
            .unwrap_or_else(|_| "C:/Users".to_string())
    }

    fn disk_space(&self, path: &str) -> Option<(u64, u64)> {
        let wide = to_wide(path);
        let mut free: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;
        let ret = unsafe {
            GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free)
        };
        if ret != 0 { Some((free, total)) } else { None }
    }

    fn open_file(&self, path: &str) {
        let _ = Command::new("cmd").args(["/c", "start", "", path]).spawn();
    }

    fn open_terminal(&self, path: &str) {
        // Try Windows Terminal first, fall back to cmd
        if Command::new("wt")
            .args(["--startingDirectory", path])
            .spawn()
            .is_err()
        {
            let _ = Command::new("cmd")
                .args(["/k", &format!("cd /d \"{}\"", path)])
                .spawn();
        }
    }

    fn open_in_editor(&self, path: &str) -> Result<String, String> {
        let output = Command::new("cmd")
            .args(["/c", "start", "", path])
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(path.to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(stderr.trim().to_string())
        }
    }

    fn list_volumes(&self) -> Vec<VolumeInfo> {
        let mask = unsafe { GetLogicalDrives() };
        let mut volumes = Vec::new();

        for i in 0u32..26 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = (b'A' + i as u8) as char;
            let root = format!("{}:/", letter);

            // Try to get the volume label
            let wide_root = to_wide(&root);
            let mut name_buf = vec![0u16; 256];
            let label = unsafe {
                let ret = GetVolumeInformationW(
                    wide_root.as_ptr(),
                    name_buf.as_mut_ptr(),
                    name_buf.len() as u32,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    0,
                );
                if ret != 0 {
                    let end = name_buf.iter().position(|&c| c == 0).unwrap_or(0);
                    String::from_utf16_lossy(&name_buf[..end])
                } else {
                    String::new()
                }
            };

            let name = if label.is_empty() {
                format!("{}:", letter)
            } else {
                format!("{} ({}:)", label, letter)
            };

            volumes.push(VolumeInfo {
                id: format!("vol-{}", letter.to_lowercase()),
                name,
                root_path: root,
            });
        }

        volumes
    }
}
