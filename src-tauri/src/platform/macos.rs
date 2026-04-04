use super::{append_supplemental_apps, OpenWithAppOption, OpenWithLaunch, Platform, ResolvedOpenWithApp, VolumeInfo};
use libc;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

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
        if ret != 0 {
            return None;
        }
        let block_size = stat.f_frsize as u64;
        let total = stat.f_blocks as u64 * block_size;
        let free = stat.f_bavail as u64 * block_size;
        Some((free, total))
    }

    fn open_terminal(&self, path: &str) {
        let _ = Command::new("open")
            .arg("-a")
            .arg("Terminal")
            .arg(path)
            .spawn();
    }

    fn reveal_in_file_manager(&self, path: &str, _kind: &str) {
        let _ = Command::new("open").arg("-R").arg(path).spawn();
    }

    fn list_open_with_apps(&self, path: &str, kind: &str) -> Result<Vec<ResolvedOpenWithApp>, String> {
        let mut apps = vec![ResolvedOpenWithApp {
            option: OpenWithAppOption {
                id: "mac:default".to_string(),
                label: "Default application".to_string(),
                detail: None,
                is_default: true,
                source: "system".to_string(),
            },
            launch: OpenWithLaunch::Default,
        }];

        let content_type = if kind == "directory" {
            "public.folder".to_string()
        } else {
            mac_content_type(path).unwrap_or_else(|| "public.data".to_string())
        };

        let query = format!("kMDItemContentTypeTree == '{content_type}'c && kMDItemContentType == 'com.apple.application-bundle'");
        if let Ok(output) = Command::new("mdfind").arg(query).output() {
            if output.status.success() {
                for app_path in String::from_utf8_lossy(&output.stdout).lines().map(str::trim).filter(|line| !line.is_empty()) {
                    apps.push(ResolvedOpenWithApp {
                        option: OpenWithAppOption {
                            id: format!("mac:app:{app_path}"),
                            label: app_bundle_name(app_path),
                            detail: Some(app_path.to_string()),
                            is_default: false,
                            source: "system".to_string(),
                        },
                        launch: OpenWithLaunch::MacApplication {
                            application_path: app_path.to_string(),
                        },
                    });
                }
            }
        }

        append_supplemental_apps(&mut apps, mac_supplemental_apps());
        Ok(apps)
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
            .filter(|entry| {
                entry
                    .file_type()
                    .map(|ft| ft.is_dir() || ft.is_symlink())
                    .unwrap_or(false)
            })
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().to_string();
                let root_path = format!("/Volumes/{}", name);
                let id = format!("vol-{}", name);
                Some(VolumeInfo {
                    id,
                    name,
                    root_path,
                    mounted: true,
                    device_path: None,
                })
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

fn mac_content_type(path: &str) -> Option<String> {
    let output = Command::new("mdls")
        .args(["-name", "kMDItemContentType", "-raw", path])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() || value == "(null)" {
        None
    } else {
        Some(value)
    }
}

fn app_bundle_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string())
}

fn mac_supplemental_apps() -> Vec<ResolvedOpenWithApp> {
    let home = env::var("HOME").unwrap_or_default();
    let mut locations = vec![
        "/Applications/Visual Studio Code.app".to_string(),
        "/Applications/VSCodium.app".to_string(),
        "/Applications/Cursor.app".to_string(),
        "/Applications/Zed.app".to_string(),
        "/Applications/Sublime Text.app".to_string(),
        "/Applications/IntelliJ IDEA.app".to_string(),
        "/Applications/PyCharm.app".to_string(),
        "/Applications/WebStorm.app".to_string(),
        "/Applications/CLion.app".to_string(),
        "/Applications/GoLand.app".to_string(),
        "/Applications/RustRover.app".to_string(),
    ];

    if !home.is_empty() {
        locations.extend([
            format!("{home}/Applications/Visual Studio Code.app"),
            format!("{home}/Applications/VSCodium.app"),
            format!("{home}/Applications/Cursor.app"),
            format!("{home}/Applications/Zed.app"),
            format!("{home}/Applications/Sublime Text.app"),
            format!("{home}/Applications/IntelliJ IDEA.app"),
            format!("{home}/Applications/PyCharm.app"),
            format!("{home}/Applications/WebStorm.app"),
            format!("{home}/Applications/CLion.app"),
            format!("{home}/Applications/GoLand.app"),
            format!("{home}/Applications/RustRover.app"),
        ]);
    }

    locations
        .into_iter()
        .filter_map(|app_path| {
            let path = PathBuf::from(&app_path);
            if !path.exists() {
                return None;
            }
            Some(ResolvedOpenWithApp {
                option: OpenWithAppOption {
                    id: format!("supplement:mac:{app_path}"),
                    label: app_bundle_name(&app_path),
                    detail: Some(app_path.clone()),
                    is_default: false,
                    source: "supplement".to_string(),
                },
                launch: OpenWithLaunch::MacApplication {
                    application_path: app_path,
                },
            })
        })
        .collect()
}
