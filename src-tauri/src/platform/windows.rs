use super::{
    ExternalClipboardOp, ExternalClipboardPayload, OpenWithAppOption, OpenWithLaunch, Platform,
    ResolvedOpenWithApp, VolumeInfo,
};
use std::env;
use std::ffi::c_void;
use std::path::PathBuf;
use std::process::Command;
use std::ptr;
use std::slice;

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
    fn OpenClipboard(h_wnd_new_owner: *mut c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(u_format: u32) -> *mut c_void;
    fn SetClipboardData(u_format: u32, h_mem: *mut c_void) -> *mut c_void;
    fn RegisterClipboardFormatW(lp_string: *const u16) -> u32;
    fn DragQueryFileW(h_drop: *mut c_void, i_file: u32, lpsz_file: *mut u16, cch: u32) -> u32;
    fn GlobalAlloc(u_flags: u32, dw_bytes: usize) -> *mut c_void;
    fn GlobalFree(h_mem: *mut c_void) -> *mut c_void;
    fn GlobalLock(h_mem: *mut c_void) -> *mut c_void;
    fn GlobalUnlock(h_mem: *mut c_void) -> i32;
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut c_void,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show_command: i32,
    ) -> isize;
}

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const DROP_EFFECT_COPY: u32 = 1;
const DROP_EFFECT_MOVE: u32 = 2;
const GMEM_MOVEABLE: u32 = 0x0002;

#[repr(C)]
struct DropFiles {
    p_files: u32,
    pt_x: i32,
    pt_y: i32,
    f_nc: i32,
    f_wide: i32,
}

/// Encode a Rust string as a null-terminated UTF-16 buffer for WinAPI calls.
fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Platform for WindowsPlatform {
    fn home_dir(&self) -> String {
        std::env::var("USERPROFILE")
            .or_else(|_| {
                std::env::var("HOMEDRIVE")
                    .and_then(|d| std::env::var("HOMEPATH").map(|p| format!("{d}{p}")))
            })
            .map(|p| p.replace('\\', "/"))
            .unwrap_or_else(|_| "C:/Users".to_string())
    }

    fn disk_space(&self, path: &str) -> Option<(u64, u64)> {
        let wide = to_wide(path);
        let mut free: u64 = 0;
        let mut total: u64 = 0;
        let mut total_free: u64 = 0;
        let ret =
            unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) };
        if ret != 0 {
            Some((free, total))
        } else {
            None
        }
    }

    fn open_file(&self, path: &str) {
        let _ = Command::new("cmd").args(["/c", "start", "", path]).spawn();
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        let operation = to_wide("open");
        let url = to_wide(url);
        let result = unsafe {
            ShellExecuteW(
                ptr::null_mut(),
                operation.as_ptr(),
                url.as_ptr(),
                ptr::null(),
                ptr::null(),
                1,
            )
        };
        if result > 32 {
            Ok(())
        } else {
            Err(format!("Could not open the link (system error {result})."))
        }
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

    fn reveal_in_file_manager(&self, path: &str, _kind: &str) {
        let windows_path = path.replace('/', "\\");
        let _ = Command::new("explorer")
            .arg(format!("/select,{windows_path}"))
            .spawn();
    }

    fn list_open_with_apps(
        &self,
        path: &str,
        kind: &str,
    ) -> Result<Vec<ResolvedOpenWithApp>, String> {
        let ext = file_extension_dot(path);
        let default_label = if kind == "directory" {
            "File Explorer".to_string()
        } else {
            ext.as_deref()
                .and_then(query_friendly_app_name)
                .unwrap_or_else(|| "Default application".to_string())
        };

        let mut apps = vec![ResolvedOpenWithApp {
            option: OpenWithAppOption {
                id: "windows:default".to_string(),
                label: default_label,
                detail: None,
                is_default: true,
                source: "system".to_string(),
            },
            launch: OpenWithLaunch::Default,
        }];

        if let Some(ref ext) = ext {
            apps.extend(windows_enumerate_handlers(ext));
        }

        super::append_supplemental_apps(&mut apps, windows_supplemental_apps(path));
        Ok(apps)
    }

    fn open_in_editor(&self, path: &str) -> Result<String, String> {
        // Try known editors in preference order before falling back to cmd /c start.
        const PREFERRED: &[&[&str]] = &[
            &["code.cmd", "code.exe"],
            &["codium.cmd", "codium.exe"],
            &["cursor.exe"],
            &["zed.exe"],
            &["subl.exe"],
        ];
        for candidates in PREFERRED {
            if let Some(editor) = find_windows_executable(candidates) {
                // .cmd files must be run via cmd.exe; .exe can be run directly.
                let spawned = if editor.to_lowercase().ends_with(".cmd") {
                    Command::new("cmd").args(["/c", &editor, path]).spawn()
                } else {
                    Command::new(&editor).arg(path).spawn()
                };
                if spawned.is_ok() {
                    return Ok(path.to_string());
                }
            }
        }
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
                mounted: true,
                device_path: None,
                eject: None,
            });
        }

        volumes
    }

    fn read_external_file_clipboard(&self) -> Result<Option<ExternalClipboardPayload>, String> {
        read_windows_external_clipboard()
    }

    fn write_external_file_clipboard(
        &self,
        payload: &ExternalClipboardPayload,
    ) -> Result<(), String> {
        write_windows_external_clipboard(payload)
    }

    fn write_text_clipboard(&self, text: &str) -> Result<(), String> {
        write_windows_text_clipboard(text)
    }

    fn get_thumbnail(&self, path: &str, size: u32) -> Option<(Vec<u8>, &'static str)> {
        windows_get_thumbnail(path, size).map(|bytes| (bytes, "image/png"))
    }
}

fn read_windows_external_clipboard() -> Result<Option<ExternalClipboardPayload>, String> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("Failed to open the system clipboard.".to_string());
        }
    }

    let result = (|| {
        let hdrop = unsafe { GetClipboardData(CF_HDROP) };
        if hdrop.is_null() {
            return Ok(None);
        }

        let file_count = unsafe { DragQueryFileW(hdrop, u32::MAX, std::ptr::null_mut(), 0) };
        if file_count == 0 {
            return Ok(None);
        }

        let mut source_paths = Vec::with_capacity(file_count as usize);
        for index in 0..file_count {
            let len = unsafe { DragQueryFileW(hdrop, index, std::ptr::null_mut(), 0) };
            if len == 0 {
                continue;
            }
            let mut buffer = vec![0u16; len as usize + 1];
            let written =
                unsafe { DragQueryFileW(hdrop, index, buffer.as_mut_ptr(), buffer.len() as u32) };
            if written == 0 {
                continue;
            }
            let path = String::from_utf16_lossy(&buffer[..written as usize]).replace('\\', "/");
            source_paths.push(path);
        }

        if source_paths.is_empty() {
            return Ok(None);
        }

        let preferred_drop_effect = unsafe {
            RegisterClipboardFormatW(
                "Preferred DropEffect"
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect::<Vec<u16>>()
                    .as_ptr(),
            )
        };
        let op = if preferred_drop_effect != 0 {
            let handle = unsafe { GetClipboardData(preferred_drop_effect) };
            if !handle.is_null() {
                let ptr = unsafe { GlobalLock(handle) };
                if !ptr.is_null() {
                    let effect = unsafe { *(ptr as *const u32) };
                    unsafe {
                        GlobalUnlock(handle);
                    }
                    if effect & DROP_EFFECT_MOVE != 0 {
                        ExternalClipboardOp::Move
                    } else {
                        ExternalClipboardOp::Copy
                    }
                } else {
                    ExternalClipboardOp::Copy
                }
            } else {
                ExternalClipboardOp::Copy
            }
        } else {
            ExternalClipboardOp::Copy
        };

        Ok(Some(ExternalClipboardPayload { op, source_paths }))
    })();

    unsafe {
        CloseClipboard();
    }

    result
}

fn write_windows_external_clipboard(payload: &ExternalClipboardPayload) -> Result<(), String> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("Failed to open the system clipboard.".to_string());
        }
    }

    let result = (|| {
        if unsafe { EmptyClipboard() } == 0 {
            return Err("Failed to clear the system clipboard.".to_string());
        }

        let hdrop_handle = build_windows_hdrop(payload)?;
        if unsafe { SetClipboardData(CF_HDROP, hdrop_handle) }.is_null() {
            unsafe {
                GlobalFree(hdrop_handle);
            }
            return Err("Failed to publish file paths to the system clipboard.".to_string());
        }

        let drop_effect_format = unsafe {
            RegisterClipboardFormatW(
                "Preferred DropEffect"
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect::<Vec<u16>>()
                    .as_ptr(),
            )
        };

        if drop_effect_format != 0 {
            let effect = match payload.op {
                ExternalClipboardOp::Move => DROP_EFFECT_MOVE,
                ExternalClipboardOp::Copy => DROP_EFFECT_COPY,
            };
            let effect_handle = alloc_global_bytes(&effect.to_le_bytes())?;
            if unsafe { SetClipboardData(drop_effect_format, effect_handle) }.is_null() {
                unsafe {
                    GlobalFree(effect_handle);
                }
                return Err("Failed to publish clipboard operation metadata.".to_string());
            }
        }

        Ok(())
    })();

    unsafe {
        CloseClipboard();
    }

    result
}

fn write_windows_text_clipboard(text: &str) -> Result<(), String> {
    let wide = encode_clipboard_utf16(text);
    let bytes = unsafe {
        slice::from_raw_parts(
            wide.as_ptr().cast::<u8>(),
            wide.len() * std::mem::size_of::<u16>(),
        )
    };

    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return Err("Failed to open the system clipboard.".to_string());
        }
    }

    let result = (|| {
        if unsafe { EmptyClipboard() } == 0 {
            return Err("Failed to clear the system clipboard.".to_string());
        }

        let handle = alloc_global_bytes(bytes)?;
        if unsafe { SetClipboardData(CF_UNICODETEXT, handle) }.is_null() {
            unsafe {
                GlobalFree(handle);
            }
            return Err("Failed to publish text to the system clipboard.".to_string());
        }

        Ok(())
    })();

    unsafe {
        CloseClipboard();
    }

    result
}

/// CF_UNICODETEXT is UTF-16LE with a trailing NUL. Lone `\n` becomes `\r\n`
/// so Notepad, cmd, and PowerShell paste the same line breaks Explorer would.
fn encode_clipboard_utf16(text: &str) -> Vec<u16> {
    text.replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

fn build_windows_hdrop(payload: &ExternalClipboardPayload) -> Result<*mut c_void, String> {
    if payload.source_paths.is_empty() {
        return Err("No files are available to export to the host clipboard.".to_string());
    }

    let mut encoded_paths = Vec::new();
    for source_path in &payload.source_paths {
        let wide_path: Vec<u16> = source_path
            .replace('/', "\\")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        encoded_paths.extend(wide_path);
    }
    encoded_paths.push(0);

    let header = DropFiles {
        p_files: std::mem::size_of::<DropFiles>() as u32,
        pt_x: 0,
        pt_y: 0,
        f_nc: 0,
        f_wide: 1,
    };

    let header_bytes = unsafe {
        slice::from_raw_parts(
            (&header as *const DropFiles).cast::<u8>(),
            std::mem::size_of::<DropFiles>(),
        )
    };
    let path_bytes = unsafe {
        slice::from_raw_parts(
            encoded_paths.as_ptr().cast::<u8>(),
            encoded_paths.len() * std::mem::size_of::<u16>(),
        )
    };

    let mut bytes = Vec::with_capacity(header_bytes.len() + path_bytes.len());
    bytes.extend_from_slice(header_bytes);
    bytes.extend_from_slice(path_bytes);
    alloc_global_bytes(&bytes)
}

fn alloc_global_bytes(bytes: &[u8]) -> Result<*mut c_void, String> {
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) };
    if handle.is_null() {
        return Err("Failed to allocate clipboard memory.".to_string());
    }

    let ptr = unsafe { GlobalLock(handle) };
    if ptr.is_null() {
        unsafe {
            GlobalFree(handle);
        }
        return Err("Failed to access clipboard memory.".to_string());
    }

    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len());
        GlobalUnlock(handle);
    }

    Ok(handle)
}

fn windows_supplemental_apps(_path: &str) -> Vec<ResolvedOpenWithApp> {
    [
        (
            "supplement:code",
            "Visual Studio Code",
            &["code.cmd", "code.exe"][..],
        ),
        (
            "supplement:codium",
            "VSCodium",
            &["codium.cmd", "codium.exe"][..],
        ),
        ("supplement:cursor", "Cursor", &["cursor.exe"][..]),
        ("supplement:zed", "Zed", &["zed.exe"][..]),
        ("supplement:sublime", "Sublime Text", &["subl.exe"][..]),
        (
            "supplement:idea",
            "IntelliJ IDEA",
            &["idea64.exe", "idea.exe"][..],
        ),
        (
            "supplement:pycharm",
            "PyCharm",
            &["pycharm64.exe", "pycharm.exe"][..],
        ),
        (
            "supplement:webstorm",
            "WebStorm",
            &["webstorm64.exe", "webstorm.exe"][..],
        ),
        (
            "supplement:clion",
            "CLion",
            &["clion64.exe", "clion.exe"][..],
        ),
        (
            "supplement:goland",
            "GoLand",
            &["goland64.exe", "goland.exe"][..],
        ),
        (
            "supplement:rustrover",
            "RustRover",
            &["rustrover64.exe", "rustrover.exe"][..],
        ),
    ]
    .iter()
    .filter_map(|(id, label, executables)| {
        find_windows_executable(executables).map(|program| ResolvedOpenWithApp {
            option: OpenWithAppOption {
                id: (*id).to_string(),
                label: (*label).to_string(),
                detail: Some(program.clone()),
                is_default: false,
                source: "supplement".to_string(),
            },
            launch: OpenWithLaunch::Command {
                program,
                args: vec![],
            },
        })
    })
    .collect()
}

fn find_windows_executable(candidates: &[&str]) -> Option<String> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .find_map(|dir| {
            candidates.iter().find_map(|candidate| {
                let joined = dir.join(candidate);
                if joined.is_file() {
                    Some(joined.to_string_lossy().to_string())
                } else {
                    None
                }
            })
        })
        .or_else(|| {
            let local_app_data = env::var_os("LOCALAPPDATA")?;
            let candidate_dirs = [
                PathBuf::from(&local_app_data).join("Programs"),
                PathBuf::from(r"C:\Program Files"),
                PathBuf::from(r"C:\Program Files (x86)"),
            ];
            candidate_dirs.iter().find_map(|dir| {
                candidates.iter().find_map(|candidate| {
                    let joined = dir.join(candidate);
                    if joined.is_file() {
                        Some(joined.to_string_lossy().to_string())
                    } else {
                        None
                    }
                })
            })
        })
}

// ── Thumbnail extraction via IShellItemImageFactory ──────────────────────────

#[repr(C)]
#[derive(Clone, Copy)]
struct GUID {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

// IID_IShellItemImageFactory: bcc18b79-ba16-442f-80c4-8a59c30c463b
const IID_ISHELL_ITEM_IMAGE_FACTORY: GUID = GUID {
    data1: 0xbcc18b79,
    data2: 0xba16,
    data3: 0x442f,
    data4: [0x80, 0xc4, 0x8a, 0x59, 0xc3, 0x0c, 0x46, 0x3b],
};

#[repr(C)]
struct SIZE {
    cx: i32,
    cy: i32,
}

const SIIGBF_BIGGERSIZEOK: u32 = 0x01;
const SIIGBF_THUMBNAILONLY: u32 = 0x08;
const COINIT_APARTMENTTHREADED: u32 = 0x2;
const COINIT_DISABLE_OLE1DDE: u32 = 0x4;

const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;

#[repr(C)]
struct BITMAP {
    bm_type: i32,
    bm_width: i32,
    bm_height: i32,
    bm_width_bytes: i32,
    bm_planes: u16,
    bm_bits_pixel: u16,
    bm_bits: *mut c_void,
}

#[repr(C)]
struct BITMAPINFOHEADER {
    bi_size: u32,
    bi_width: i32,
    bi_height: i32,
    bi_planes: u16,
    bi_bit_count: u16,
    bi_compression: u32,
    bi_size_image: u32,
    bi_x_pels_per_meter: i32,
    bi_y_pels_per_meter: i32,
    bi_clr_used: u32,
    bi_clr_important: u32,
}

#[repr(C)]
struct BITMAPINFO {
    header: BITMAPINFOHEADER,
    colors: [u32; 1],
}

#[link(name = "ole32")]
extern "system" {
    fn CoInitializeEx(reserved: *mut c_void, co_init: u32) -> i32;
    fn CoUninitialize();
    fn CoTaskMemFree(pv: *mut c_void);
}

#[link(name = "shlwapi")]
extern "system" {
    fn AssocQueryStringW(
        flags: u32,
        str_kind: u32,
        psz_assoc: *const u16,
        psz_extra: *const u16,
        psz_out: *mut u16,
        pcch_out: *mut u32,
    ) -> i32;
}

const ASSOCSTR_FRIENDLYAPPNAME: u32 = 4;
const ASSOC_FILTER_RECOMMENDED: u32 = 1;

// IID_IEnumAssocHandlers: 973810ae-9599-4b88-9e4d-6ee98c9552da
const IID_IENUM_ASSOC_HANDLERS: GUID = GUID {
    data1: 0x973810ae,
    data2: 0x9599,
    data3: 0x4b88,
    data4: [0x9e, 0x4d, 0x6e, 0xe9, 0x8c, 0x95, 0x52, 0xda],
};

#[repr(C)]
struct IEnumAssocHandlersVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    next: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void, *mut u32) -> i32,
}

#[repr(C)]
struct IEnumAssocHandlers {
    vtbl: *const IEnumAssocHandlersVtbl,
}

#[repr(C)]
struct IAssocHandlerVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_name: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32,
    get_ui_name: unsafe extern "system" fn(*mut c_void, *mut *mut u16) -> i32,
    get_icon_location: unsafe extern "system" fn(*mut c_void, *mut *mut u16, *mut i32) -> i32,
    is_recommended: unsafe extern "system" fn(*mut c_void) -> i32,
    make_default: unsafe extern "system" fn(*mut c_void, *const u16) -> i32,
    invoke: unsafe extern "system" fn(*mut c_void, *mut c_void) -> i32,
}

#[repr(C)]
struct IAssocHandler {
    vtbl: *const IAssocHandlerVtbl,
}

#[link(name = "shell32")]
extern "system" {
    fn SHAssocEnumHandlers(
        psz_extra: *const u16,
        af_filter: u32,
        pp_enum_handler: *mut *mut c_void,
    ) -> i32;
}

/// Extract the dotted file extension (e.g. ".png") in lowercase, or None.
fn file_extension_dot(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let basename = trimmed
        .rsplit_once(['/', '\\'])
        .map(|(_, b)| b)
        .unwrap_or(trimmed);
    let dot = basename.rfind('.')?;
    if dot == 0 {
        return None; // hidden file with no extension (.gitignore)
    }
    let ext = &basename[dot..];
    if ext.len() < 2 || ext.len() > 32 {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

fn query_friendly_app_name(ext: &str) -> Option<String> {
    let wext = to_wide(ext);
    let mut len: u32 = 0;
    let hr = unsafe {
        AssocQueryStringW(
            0,
            ASSOCSTR_FRIENDLYAPPNAME,
            wext.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            &mut len,
        )
    };
    // E_POINTER (0x80004003) is expected when probing for size.
    if len == 0 {
        return None;
    }
    let _ = hr;
    let mut buf = vec![0u16; len as usize];
    let hr = unsafe {
        AssocQueryStringW(
            0,
            ASSOCSTR_FRIENDLYAPPNAME,
            wext.as_ptr(),
            ptr::null(),
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    if hr < 0 {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let name = String::from_utf16_lossy(&buf[..end]);
    if name.trim().is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Read a CoTaskMem-allocated UTF-16 string and free it.
unsafe fn cotask_string(p: *mut u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
    }
    let slice = std::slice::from_raw_parts(p, len);
    let s = String::from_utf16_lossy(slice);
    CoTaskMemFree(p as *mut c_void);
    Some(s)
}

fn windows_enumerate_handlers(ext: &str) -> Vec<ResolvedOpenWithApp> {
    let wext = to_wide(ext);

    unsafe {
        let hr = CoInitializeEx(
            ptr::null_mut(),
            COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE,
        );
        if hr < 0 {
            return Vec::new();
        }

        let mut enumerator: *mut c_void = ptr::null_mut();
        let hr = SHAssocEnumHandlers(wext.as_ptr(), ASSOC_FILTER_RECOMMENDED, &mut enumerator);
        if hr < 0 || enumerator.is_null() {
            return Vec::new();
        }

        let enum_ptr = enumerator as *mut IEnumAssocHandlers;
        let enum_vtbl = (*enum_ptr).vtbl;

        let mut out = Vec::new();
        let mut seen_programs: std::collections::HashSet<String> = std::collections::HashSet::new();

        loop {
            let mut handler: *mut c_void = ptr::null_mut();
            let mut fetched: u32 = 0;
            let hr = ((*enum_vtbl).next)(enumerator, 1, &mut handler, &mut fetched);
            if hr != 0 || fetched == 0 || handler.is_null() {
                break;
            }

            let handler_ptr = handler as *mut IAssocHandler;
            let handler_vtbl = (*handler_ptr).vtbl;

            let mut name_ptr: *mut u16 = ptr::null_mut();
            let mut ui_name_ptr: *mut u16 = ptr::null_mut();
            let name_hr = ((*handler_vtbl).get_name)(handler, &mut name_ptr);
            let ui_hr = ((*handler_vtbl).get_ui_name)(handler, &mut ui_name_ptr);

            let exe = if name_hr >= 0 {
                cotask_string(name_ptr)
            } else {
                None
            };
            let ui_name = if ui_hr >= 0 {
                cotask_string(ui_name_ptr)
            } else {
                None
            };

            ((*handler_vtbl).release)(handler);

            // Keep only handlers backed by a real .exe path; skip UWP / protocol handlers.
            if let (Some(exe), Some(label)) = (exe, ui_name) {
                let exe_lower = exe.to_ascii_lowercase();
                if !exe_lower.ends_with(".exe") {
                    continue;
                }
                if !std::path::Path::new(&exe).is_file() {
                    continue;
                }
                if !seen_programs.insert(exe_lower) {
                    continue;
                }
                out.push(ResolvedOpenWithApp {
                    option: OpenWithAppOption {
                        id: format!("windows:handler:{}", out.len()),
                        label,
                        detail: Some(exe.clone()),
                        is_default: false,
                        source: "system".to_string(),
                    },
                    launch: OpenWithLaunch::WindowsHandlerInvoke {
                        ext: ext.to_string(),
                        exe_path: exe,
                    },
                });
            }
        }

        ((*enum_vtbl).release)(enumerator);
        out
    }
}

#[link(name = "shell32")]
extern "system" {
    fn SHCreateItemFromParsingName(
        psz_path: *const u16,
        pbc: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn GetObjectW(h: *mut c_void, c: i32, pv: *mut c_void) -> i32;
    fn DeleteObject(h: *mut c_void) -> i32;
    fn CreateCompatibleDC(hdc: *mut c_void) -> *mut c_void;
    fn DeleteDC(hdc: *mut c_void) -> i32;
    fn GetDIBits(
        hdc: *mut c_void,
        h_bmp: *mut c_void,
        u_start_scan: u32,
        c_scan_lines: u32,
        lp_v_bits: *mut c_void,
        lpbmi: *mut BITMAPINFO,
        usage: u32,
    ) -> i32;
}

// IShellItemImageFactory vtable layout: IUnknown (3) + GetImage
#[repr(C)]
struct IShellItemImageFactoryVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    get_image: unsafe extern "system" fn(*mut c_void, SIZE, u32, *mut *mut c_void) -> i32,
}

#[repr(C)]
struct IShellItemImageFactory {
    vtbl: *const IShellItemImageFactoryVtbl,
}

fn windows_get_thumbnail(path: &str, size: u32) -> Option<Vec<u8>> {
    // Convert forward slashes to backslashes for the shell.
    let win_path = path.replace('/', "\\");
    let wide = to_wide(&win_path);

    unsafe {
        let hr = CoInitializeEx(
            ptr::null_mut(),
            COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE,
        );
        // S_OK == 0, S_FALSE == 1 (already initialized) — both fine.
        if hr < 0 {
            return None;
        }

        let result = (|| -> Option<Vec<u8>> {
            let mut factory: *mut c_void = ptr::null_mut();
            let hr = SHCreateItemFromParsingName(
                wide.as_ptr(),
                ptr::null_mut(),
                &IID_ISHELL_ITEM_IMAGE_FACTORY,
                &mut factory,
            );
            if hr < 0 || factory.is_null() {
                return None;
            }

            let factory_ptr = factory as *mut IShellItemImageFactory;
            let vtbl = (*factory_ptr).vtbl;

            let mut hbitmap: *mut c_void = ptr::null_mut();
            let hr = ((*vtbl).get_image)(
                factory,
                SIZE {
                    cx: size as i32,
                    cy: size as i32,
                },
                SIIGBF_BIGGERSIZEOK,
                &mut hbitmap,
            );
            ((*vtbl).release)(factory);

            if hr < 0 || hbitmap.is_null() {
                return None;
            }

            let png = hbitmap_to_png(hbitmap);
            DeleteObject(hbitmap);
            png
        })();

        // Don't CoUninitialize — leave COM initialized for the worker thread's lifetime.
        // Calling CoUninitialize after a successful CoInitializeEx that returned S_FALSE
        // (already-initialized) would balance someone else's init.
        let _ = hr;
        result
    }
}

unsafe fn hbitmap_to_png(hbitmap: *mut c_void) -> Option<Vec<u8>> {
    let mut bm: BITMAP = std::mem::zeroed();
    let n = GetObjectW(
        hbitmap,
        std::mem::size_of::<BITMAP>() as i32,
        &mut bm as *mut _ as *mut c_void,
    );
    if n == 0 || bm.bm_width <= 0 || bm.bm_height <= 0 {
        return None;
    }
    let width = bm.bm_width as u32;
    let height = bm.bm_height as u32;

    let hdc = CreateCompatibleDC(ptr::null_mut());
    if hdc.is_null() {
        return None;
    }

    let mut bmi = BITMAPINFO {
        header: BITMAPINFOHEADER {
            bi_size: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            bi_width: bm.bm_width,
            // Negative height: top-down rows.
            bi_height: -bm.bm_height,
            bi_planes: 1,
            bi_bit_count: 32,
            bi_compression: BI_RGB,
            bi_size_image: 0,
            bi_x_pels_per_meter: 0,
            bi_y_pels_per_meter: 0,
            bi_clr_used: 0,
            bi_clr_important: 0,
        },
        colors: [0],
    };

    let stride = (width as usize) * 4;
    let mut pixels = vec![0u8; stride * (height as usize)];

    let lines = GetDIBits(
        hdc,
        hbitmap,
        0,
        height,
        pixels.as_mut_ptr() as *mut c_void,
        &mut bmi,
        DIB_RGB_COLORS,
    );
    DeleteDC(hdc);
    if lines == 0 {
        return None;
    }

    // Pixels arrive as BGRA. Swap to RGBA for PNG.
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }

    let mut png_bytes: Vec<u8> = Vec::with_capacity(pixels.len() / 4);
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&pixels).ok()?;
    }
    Some(png_bytes)
}

// ── IAssocHandler::Invoke launch path ────────────────────────────────────────
//
// Spawning the registered exe directly (`Command::new(exe).arg(path)`) drops
// the verb-specific command template stored in the registry. For apps like
// VLC that template is `"vlc.exe" --started-from-file "%1"`, so plain
// `vlc.exe <path>` launches the player without opening the file.
//
// `IAssocHandler::Invoke(IDataObject*)` is the shell's official entry point;
// it honors the registered template and works for UWP handlers too.

// IID_IShellItem: 43826d1e-e718-42ee-bc55-a1e261c37bfe
const IID_ISHELL_ITEM: GUID = GUID {
    data1: 0x43826d1e,
    data2: 0xe718,
    data3: 0x42ee,
    data4: [0xbc, 0x55, 0xa1, 0xe2, 0x61, 0xc3, 0x7b, 0xfe],
};

// IID_IDataObject: 0000010e-0000-0000-c000-000000000046
const IID_IDATA_OBJECT: GUID = GUID {
    data1: 0x0000010e,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xc0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

// BHID_DataObject: b8c0bd9f-ed24-455c-83e6-d5390c4fe8c4
const BHID_DATA_OBJECT: GUID = GUID {
    data1: 0xb8c0bd9f,
    data2: 0xed24,
    data3: 0x455c,
    data4: [0x83, 0xe6, 0xd5, 0x39, 0x0c, 0x4f, 0xe8, 0xc4],
};

#[repr(C)]
struct IShellItemVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    bind_to_handler: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const GUID,
        *const GUID,
        *mut *mut c_void,
    ) -> i32,
    // Remaining IShellItem methods (GetParent, GetDisplayName, GetAttributes, Compare) unused.
}

#[repr(C)]
struct IShellItem {
    vtbl: *const IShellItemVtbl,
}

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

#[repr(C)]
struct IUnknown {
    vtbl: *const IUnknownVtbl,
}

/// Release any COM object via its IUnknown vtable slot (Release at offset 2).
unsafe fn com_release(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    let unk = p as *mut IUnknown;
    let vtbl = (*unk).vtbl;
    ((*vtbl).release)(p);
}

pub fn invoke_handler(ext: &str, exe_path: &str, file_path: &str) -> Result<(), String> {
    let wext = to_wide(ext);
    let win_file_path = file_path.replace('/', "\\");
    let wfile = to_wide(&win_file_path);
    let exe_lower = exe_path.to_ascii_lowercase();

    unsafe {
        let hr = CoInitializeEx(
            ptr::null_mut(),
            COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE,
        );
        if hr < 0 {
            return Err(format!("CoInitializeEx failed: 0x{:08x}", hr));
        }

        // Locate the handler whose GetName() matches exe_path.
        let mut enumerator: *mut c_void = ptr::null_mut();
        let hr = SHAssocEnumHandlers(wext.as_ptr(), ASSOC_FILTER_RECOMMENDED, &mut enumerator);
        if hr < 0 || enumerator.is_null() {
            return Err(format!("SHAssocEnumHandlers failed: 0x{:08x}", hr));
        }
        let enum_vtbl = (*(enumerator as *mut IEnumAssocHandlers)).vtbl;

        let mut chosen: *mut c_void = ptr::null_mut();
        loop {
            let mut handler: *mut c_void = ptr::null_mut();
            let mut fetched: u32 = 0;
            let hr = ((*enum_vtbl).next)(enumerator, 1, &mut handler, &mut fetched);
            if hr != 0 || fetched == 0 || handler.is_null() {
                break;
            }
            let handler_vtbl = (*(handler as *mut IAssocHandler)).vtbl;
            let mut name_ptr: *mut u16 = ptr::null_mut();
            let matched = if ((*handler_vtbl).get_name)(handler, &mut name_ptr) >= 0 {
                cotask_string(name_ptr)
                    .map(|n| n.to_ascii_lowercase() == exe_lower)
                    .unwrap_or(false)
            } else {
                false
            };
            if matched {
                chosen = handler;
                break;
            }
            ((*handler_vtbl).release)(handler);
        }
        com_release(enumerator);

        if chosen.is_null() {
            return Err(format!(
                "No registered handler matched '{}' for extension '{}'",
                exe_path, ext
            ));
        }

        // Build an IDataObject for the file via IShellItem::BindToHandler(BHID_DataObject).
        let mut shell_item: *mut c_void = ptr::null_mut();
        let hr = SHCreateItemFromParsingName(
            wfile.as_ptr(),
            ptr::null_mut(),
            &IID_ISHELL_ITEM,
            &mut shell_item,
        );
        if hr < 0 || shell_item.is_null() {
            com_release(chosen);
            return Err(format!("SHCreateItemFromParsingName failed: 0x{:08x}", hr));
        }
        let item_vtbl = (*(shell_item as *mut IShellItem)).vtbl;

        let mut data_obj: *mut c_void = ptr::null_mut();
        let hr = ((*item_vtbl).bind_to_handler)(
            shell_item,
            ptr::null_mut(),
            &BHID_DATA_OBJECT,
            &IID_IDATA_OBJECT,
            &mut data_obj,
        );
        com_release(shell_item);
        if hr < 0 || data_obj.is_null() {
            com_release(chosen);
            return Err(format!("BindToHandler(BHID_DataObject) failed: 0x{:08x}", hr));
        }

        let handler_vtbl = (*(chosen as *mut IAssocHandler)).vtbl;
        let hr = ((*handler_vtbl).invoke)(chosen, data_obj);
        com_release(data_obj);
        com_release(chosen);

        if hr < 0 {
            Err(format!("IAssocHandler::Invoke failed: 0x{:08x}", hr))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::encode_clipboard_utf16;

    fn decode_clipboard_utf16(wide: &[u16]) -> String {
        assert_eq!(
            wide.last().copied(),
            Some(0),
            "clipboard payload must be NUL-terminated"
        );
        String::from_utf16(&wide[..wide.len() - 1]).expect("valid UTF-16")
    }

    #[test]
    fn encode_clipboard_utf16_terminates_empty_text() {
        assert_eq!(encode_clipboard_utf16(""), vec![0]);
    }

    #[test]
    fn encode_clipboard_utf16_keeps_plain_text() {
        assert_eq!(
            decode_clipboard_utf16(&encode_clipboard_utf16("hello")),
            "hello"
        );
    }

    #[test]
    fn encode_clipboard_utf16_converts_lone_newlines() {
        assert_eq!(
            decode_clipboard_utf16(&encode_clipboard_utf16("C:/a\nC:/b")),
            "C:/a\r\nC:/b"
        );
    }

    #[test]
    fn encode_clipboard_utf16_does_not_double_existing_crlf() {
        assert_eq!(
            decode_clipboard_utf16(&encode_clipboard_utf16("C:/a\r\nC:/b")),
            "C:/a\r\nC:/b"
        );
    }

    #[test]
    fn encode_clipboard_utf16_preserves_non_ascii_paths() {
        let path = "C:/Users/ktx/файл.txt";
        assert_eq!(decode_clipboard_utf16(&encode_clipboard_utf16(path)), path);
    }
}
