use super::{
    append_supplemental_apps, ExternalClipboardOp, ExternalClipboardPayload, OpenWithAppOption,
    OpenWithLaunch, Platform, ResolvedOpenWithApp, VolumeInfo,
};
use libc;
use std::env;
use std::ffi::{c_char, c_void, CStr, CString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct MacOSPlatform;

type CFAllocatorRef = *const c_void;
type CFArrayRef = *const c_void;
type CFURLRef = *const c_void;
type CFTypeRef = *const c_void;
type CFIndex = isize;
type Boolean = u8;
type LSRolesMask = u32;
type ObjcId = *mut c_void;
type ObjcClass = *mut c_void;
type ObjcSel = *const c_void;

const K_LS_ROLES_ALL: LSRolesMask = u32::MAX;
const NSFILENAMES_PBOARD_TYPE: &str = "NSFilenamesPboardType";

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

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> ObjcClass;
    fn sel_registerName(name: *const c_char) -> ObjcSel;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
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

    fn list_open_with_apps(
        &self,
        path: &str,
        kind: &str,
    ) -> Result<Vec<ResolvedOpenWithApp>, String> {
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
        // Try known editors in preference order before falling back to open.
        let home = env::var("HOME").unwrap_or_default();
        let preferred = [
            "Visual Studio Code.app",
            "VSCodium.app",
            "Cursor.app",
            "Zed.app",
            "Sublime Text.app",
        ];
        for app_name in preferred {
            let candidates = if home.is_empty() {
                vec![format!("/Applications/{app_name}")]
            } else {
                vec![
                    format!("/Applications/{app_name}"),
                    format!("{home}/Applications/{app_name}"),
                ]
            };
            for app_path in &candidates {
                if std::path::Path::new(app_path).exists() {
                    return Command::new("open")
                        .args(["-a", app_path, path])
                        .spawn()
                        .map(|_| path.to_string())
                        .map_err(|e| e.to_string());
                }
            }
        }
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
        let ignored_mounts = disk_image_mount_points();
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
                if ignored_mounts.contains(&root_path) {
                    return None;
                }
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

    fn read_external_file_clipboard(&self) -> Result<Option<ExternalClipboardPayload>, String> {
        read_external_file_clipboard_native()
    }

    fn write_external_file_clipboard(
        &self,
        payload: &ExternalClipboardPayload,
    ) -> Result<(), String> {
        write_external_file_clipboard_native(payload)
    }
}

fn disk_image_mount_points() -> std::collections::HashSet<String> {
    let output = match Command::new("hdiutil").args(["info", "-plist"]).output() {
        Ok(output) if output.status.success() => output,
        _ => return std::collections::HashSet::new(),
    };

    let mut child = match Command::new("plutil")
        .args(["-convert", "json", "-o", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return std::collections::HashSet::new(),
    };

    if let Some(stdin) = child.stdin.as_mut() {
        if stdin.write_all(&output.stdout).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return std::collections::HashSet::new();
        }
    }

    let converted = match child.wait_with_output() {
        Ok(output) if output.status.success() => output,
        _ => return std::collections::HashSet::new(),
    };

    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&converted.stdout) else {
        return std::collections::HashSet::new();
    };

    value
        .get("images")
        .and_then(|images| images.as_array())
        .into_iter()
        .flatten()
        .filter_map(|image| {
            image
                .get("system-entities")
                .and_then(|entities| entities.as_array())
        })
        .flatten()
        .filter_map(|entity| entity.get("mount-point").and_then(|mount| mount.as_str()))
        .map(str::to_string)
        .collect()
}

fn read_external_file_clipboard_native() -> Result<Option<ExternalClipboardPayload>, String> {
    let pool = unsafe { objc_autoreleasePoolPush() };
    let result = unsafe {
        let pasteboard_class = get_objc_class("NSPasteboard")?;
        let pasteboard = msg_send0::<ObjcId>(pasteboard_class, "generalPasteboard");
        if pasteboard.is_null() {
            return Ok(None);
        }

        if let Some(paths) = read_nsfilenames_payload(pasteboard)? {
            return Ok(Some(ExternalClipboardPayload {
                op: ExternalClipboardOp::Copy,
                source_paths: paths,
            }));
        }

        let array_class = get_objc_class("NSArray")?;
        let url_class = get_objc_class("NSURL")?;
        let classes = msg_send1::<ObjcId, ObjcId>(array_class, "arrayWithObject:", url_class);
        if classes.is_null() {
            return Ok(None);
        }

        let urls = msg_send2::<ObjcId, ObjcId, ObjcId>(
            pasteboard,
            "readObjectsForClasses:options:",
            classes,
            std::ptr::null_mut(),
        );
        if urls.is_null() {
            return Ok(None);
        }

        let source_paths = collect_nsurl_paths(urls);
        if source_paths.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ExternalClipboardPayload {
                op: ExternalClipboardOp::Copy,
                source_paths,
            }))
        }
    };
    unsafe { objc_autoreleasePoolPop(pool) };
    result
}

fn write_external_file_clipboard_native(payload: &ExternalClipboardPayload) -> Result<(), String> {
    if payload.op == ExternalClipboardOp::Move {
        return Err("Host cut export is not available on macOS yet.".to_string());
    }

    if payload.source_paths.is_empty() {
        return Err("No files are available to export to the host clipboard.".to_string());
    }

    let pool = unsafe { objc_autoreleasePoolPush() };
    let result = unsafe {
        let pasteboard_class = get_objc_class("NSPasteboard")?;
        let pasteboard = msg_send0::<ObjcId>(pasteboard_class, "generalPasteboard");
        if pasteboard.is_null() {
            return Err("Failed to access the system pasteboard.".to_string());
        }

        let _: isize = msg_send0(pasteboard, "clearContents");

        let array_class = get_objc_class("NSMutableArray")?;
        let urls = msg_send0::<ObjcId>(array_class, "array");
        if urls.is_null() {
            return Err("Failed to prepare pasteboard URLs.".to_string());
        }

        let url_class = get_objc_class("NSURL")?;
        for source_path in &payload.source_paths {
            let ns_path = make_nsstring(source_path)?;
            let is_directory = std::fs::symlink_metadata(source_path)
                .map(|metadata| metadata.file_type().is_dir())
                .unwrap_or(false);
            let url = msg_send2::<ObjcId, Boolean, ObjcId>(
                url_class,
                "fileURLWithPath:isDirectory:",
                ns_path,
                if is_directory { 1 } else { 0 },
            );
            if url.is_null() {
                return Err("Failed to build a file URL for the host clipboard.".to_string());
            }
            let _: () = msg_send1(urls, "addObject:", url);
        }

        let success = msg_send1::<ObjcId, Boolean>(pasteboard, "writeObjects:", urls);
        if success == 0 {
            Err("Failed to publish files to the system pasteboard.".to_string())
        } else {
            Ok(())
        }
    };
    unsafe { objc_autoreleasePoolPop(pool) };
    result
}

unsafe fn read_nsfilenames_payload(pasteboard: ObjcId) -> Result<Option<Vec<String>>, String> {
    let ns_type = make_nsstring(NSFILENAMES_PBOARD_TYPE)?;
    let payload = msg_send1::<ObjcId, ObjcId>(pasteboard, "propertyListForType:", ns_type);
    if payload.is_null() {
        return Ok(None);
    }

    let source_paths = collect_nsstring_array(payload);
    if source_paths.is_empty() {
        Ok(None)
    } else {
        Ok(Some(source_paths))
    }
}

unsafe fn collect_nsurl_paths(urls: ObjcId) -> Vec<String> {
    let count = msg_send0::<usize>(urls, "count");
    let mut source_paths = Vec::with_capacity(count);
    for index in 0..count {
        let url = msg_send1::<usize, ObjcId>(urls, "objectAtIndex:", index);
        if url.is_null() {
            continue;
        }
        let path = msg_send0::<ObjcId>(url, "path");
        if let Some(path) = nsstring_to_string(path) {
            if !path.is_empty() {
                source_paths.push(path);
            }
        }
    }
    source_paths
}

unsafe fn collect_nsstring_array(values: ObjcId) -> Vec<String> {
    let count = msg_send0::<usize>(values, "count");
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let value = msg_send1::<usize, ObjcId>(values, "objectAtIndex:", index);
        if let Some(value) = nsstring_to_string(value) {
            if !value.is_empty() {
                result.push(value);
            }
        }
    }
    result
}

unsafe fn nsstring_to_string(value: ObjcId) -> Option<String> {
    if value.is_null() {
        return None;
    }

    let utf8 = msg_send0::<*const c_char>(value, "UTF8String");
    if utf8.is_null() {
        None
    } else {
        Some(CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}

unsafe fn get_objc_class(name: &str) -> Result<ObjcClass, String> {
    let name = CString::new(name).map_err(|error| error.to_string())?;
    let class = objc_getClass(name.as_ptr());
    if class.is_null() {
        Err(format!("Missing Objective-C class: {name:?}"))
    } else {
        Ok(class)
    }
}

unsafe fn make_nsstring(value: &str) -> Result<ObjcId, String> {
    let string_class = get_objc_class("NSString")?;
    let value = CString::new(value).map_err(|error| error.to_string())?;
    let string =
        msg_send1::<*const c_char, ObjcId>(string_class, "stringWithUTF8String:", value.as_ptr());
    if string.is_null() {
        Err("Failed to construct NSString.".to_string())
    } else {
        Ok(string)
    }
}

unsafe fn selector(name: &str) -> ObjcSel {
    let name = CString::new(name).expect("selector name contains no nul bytes");
    sel_registerName(name.as_ptr())
}

unsafe fn msg_send0<R>(target: ObjcId, name: &str) -> R {
    let send: unsafe extern "C" fn(ObjcId, ObjcSel) -> R =
        std::mem::transmute(objc_msgSend as *const ());
    send(target, selector(name))
}

unsafe fn msg_send1<A, R>(target: ObjcId, name: &str, arg: A) -> R {
    let send: unsafe extern "C" fn(ObjcId, ObjcSel, A) -> R =
        std::mem::transmute(objc_msgSend as *const ());
    send(target, selector(name), arg)
}

unsafe fn msg_send2<A, B, R>(target: ObjcId, name: &str, arg1: A, arg2: B) -> R {
    let send: unsafe extern "C" fn(ObjcId, ObjcSel, A, B) -> R =
        std::mem::transmute(objc_msgSend as *const ());
    send(target, selector(name), arg1, arg2)
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
    let ok = CFURLGetFileSystemRepresentation(url, 1, buffer.as_mut_ptr(), buffer.len() as CFIndex);
    if ok == 0 {
        return None;
    }

    let nul = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
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
