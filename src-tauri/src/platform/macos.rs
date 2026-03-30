use super::{Platform, VolumeInfo};
use std::process::Command;
use libc;

pub struct MacOSPlatform;

impl Platform for MacOSPlatform {
    fn home_dir(&self) -> String {
        std::env::var("HOME").unwrap_or_else(|_| "/".to_string())
    }

    fn open_file(&self, path: &str) {
        let _ = Command::new("open").arg(path).spawn();
    }

    fn disk_space(&self, path: &str) -> Option<(u64, u64)> {
        use std::ffi::CString;
        let cpath = CString::new(path).ok()?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::statvfs(cpath.as_ptr(), &mut stat) };
        if ret != 0 { return None; }
        let block_size = stat.f_frsize as u64;
        let total = stat.f_blocks as u64 * block_size;
        let free  = stat.f_bavail as u64 * block_size;
        Some((free, total))
    }

    fn open_terminal(&self, path: &str) {
        let _ = Command::new("open").arg("-a").arg("Terminal").arg(path).spawn();
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

    fn list_volumes(&self) -> Vec<VolumeInfo> {
        let dir = match std::fs::read_dir("/Volumes") {
            Ok(d) => d,
            Err(_) => {
                return vec![VolumeInfo {
                    id: "vol-root".to_string(),
                    name: "/".to_string(),
                    root_path: "/".to_string(),
                    mounted: true,
                    device_path: None,
                }];
            }
        };

        let mut volumes: Vec<VolumeInfo> = dir
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|ft| ft.is_dir() || ft.is_symlink()).unwrap_or(false))
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                let root_path = format!("/Volumes/{}", name);
                let id = format!("vol-{}", name);
                Some(VolumeInfo { id, name, root_path, mounted: true, device_path: None })
            })
            .collect();

        if volumes.is_empty() {
            return vec![VolumeInfo {
                id: "vol-root".to_string(),
                name: "/".to_string(),
                root_path: "/".to_string(),
                mounted: true,
                device_path: None,
            }];
        }

        volumes.sort_by(|a, b| a.name.cmp(&b.name));
        volumes
    }
}
