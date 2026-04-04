use super::{append_supplemental_apps, OpenWithAppOption, OpenWithLaunch, Platform, ResolvedOpenWithApp, VolumeInfo};
use libc;
use std::env;
use std::ffi::{c_void, CString};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct MacOSPlatform;

type CFAllocatorRef = *const c_void;
type CFArrayRef = *const c_void;
type CFURLRef = *const c_void;
type CFTypeRef = *const c_void;
type CFIndex = isize;
type Boolean = u8;
type LSRolesMask = u32;

const K_LS_ROLES_ALL: LSRolesMask = u32::MAX;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFArrayGetCount(the_array: CFArrayRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(the_array: CFArrayRef, idx: CFIndex) -> *const c_void;
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: CFAllocatorRef,
        buffer: *const u8,
        buf_len: CFIndex,
        is_directory: Boolean,
    ) -> CFURLRef;
    fn CFURLGetFileSystemRepresentation(
        url: CFURLRef,
        resolve_against_base: Boolean,
        buffer: *mut u8,
        max_buf_len: CFIndex,
    ) -> Boolean;
}

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn LSCopyDefaultApplicationURLForURL(
        in_url: CFURLRef,
        in_role_mask: LSRolesMask,
        out_error: *mut CFTypeRef,
    ) -> CFURLRef;
    fn LSCopyApplicationURLsForURL(in_url: CFURLRef, in_role_mask: LSRolesMask) -> CFArrayRef;
}

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
        let file_url = create_file_url(path, kind == "directory")
            .ok_or_else(|| "Failed to resolve the target URL".to_string())?;

        let default_path = unsafe {
            let mut error: CFTypeRef = std::ptr::null();
            let url = LSCopyDefaultApplicationURLForURL(file_url, K_LS_ROLES_ALL, &mut error);
            if !error.is_null() {
                CFRelease(error);
            }
            cf_url_to_path(url)
        };

        let mut apps = Vec::new();
        let mut seen_paths = std::collections::HashSet::new();

        if let Some(default_path) = default_path {
            seen_paths.insert(default_path.clone());
            apps.push(ResolvedOpenWithApp {
                option: OpenWithAppOption {
                    id: format!("mac:app:{default_path}"),
                    label: app_bundle_name(&default_path),
                    detail: Some(default_path.clone()),
                    is_default: true,
                    source: "system".to_string(),
                },
                launch: OpenWithLaunch::MacApplication {
                    application_path: default_path,
                },
            });
        }

        unsafe {
            let app_urls = LSCopyApplicationURLsForURL(file_url, K_LS_ROLES_ALL);
            if !app_urls.is_null() {
                let count = CFArrayGetCount(app_urls);
                for index in 0..count {
                    let value = CFArrayGetValueAtIndex(app_urls, index);
                    let app_url = value as CFURLRef;
                    if let Some(app_path) = cf_url_to_path_no_release(app_url) {
                        if !seen_paths.insert(app_path.clone()) {
                            continue;
                        }
                        apps.push(ResolvedOpenWithApp {
                            option: OpenWithAppOption {
                                id: format!("mac:app:{app_path}"),
                                label: app_bundle_name(&app_path),
                                detail: Some(app_path.clone()),
                                is_default: false,
                                source: "system".to_string(),
                            },
                            launch: OpenWithLaunch::MacApplication {
                                application_path: app_path,
                            },
                        });
                    }
                }
                CFRelease(app_urls);
            }
            CFRelease(file_url);
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

fn create_file_url(path: &str, is_directory: bool) -> Option<CFURLRef> {
    let path = CString::new(path).ok()?;
    let url = unsafe {
        CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            path.as_ptr() as *const u8,
            path.as_bytes().len() as CFIndex,
            if is_directory { 1 } else { 0 },
        )
    };
    if url.is_null() {
        None
    } else {
        Some(url)
    }
}

fn cf_url_to_path(url: CFURLRef) -> Option<String> {
    let path = unsafe { cf_url_to_path_no_release(url) };
    if !url.is_null() {
        unsafe { CFRelease(url) };
    }
    path
}

unsafe fn cf_url_to_path_no_release(url: CFURLRef) -> Option<String> {
    if url.is_null() {
        return None;
    }

    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    let ok = CFURLGetFileSystemRepresentation(
        url,
        1,
        buffer.as_mut_ptr(),
        buffer.len() as CFIndex,
    );
    if ok == 0 {
        return None;
    }

    let nul = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
    String::from_utf8(buffer[..nul].to_vec()).ok()
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
