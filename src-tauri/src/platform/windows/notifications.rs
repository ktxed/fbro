//! Windows toast delivery through notify-rust. Shared policy stays in notifications::desktop.
//!
//! Toasts are attributed to the installed AppUserModelID `dev.fbro.app`. Tauri's NSIS and
//! WiX installers stamp that id on the Start Menu shortcut; the shortcut supplies the
//! display name and icon. notify-rust's unset-id fallback is PowerShell, which this
//! adapter never uses.

use notify_rust::{Notification, Timeout, Urgency};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use windows::core::{Interface, PCWSTR, GUID};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, PROPERTYKEY, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc, PROPVARIANT};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::System::Variant::{VT_BSTR, VT_EMPTY, VT_LPWSTR};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

pub const APP_ID: &str = "dev.fbro.app";
pub const SETTINGS_URI: &str = "ms-settings:notifications";
const SERVICE_TIMEOUT: Duration = Duration::from_secs(3);
const APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToastPayload {
    app_id: &'static str,
    title: &'static str,
    body: String,
    silent: bool,
    normal_urgency: bool,
    default_timeout: bool,
    actions: Vec<String>,
}

pub fn open_settings() -> Result<(), String> {
    super::launch_with_shell(SETTINGS_URI)
        .map_err(|error| format!("Cannot open Windows notification settings: {error}"))
}

pub fn supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| match std::env::current_exe() {
        Ok(exe) => installed_identity(&exe),
        Err(error) => {
            eprintln!("[notifications] Cannot resolve the application executable: {error}");
            false
        }
    })
}

pub fn authorized() -> Result<bool, String> {
    let global = query_dword(
        r"Software\Microsoft\Windows\CurrentVersion\PushNotifications",
        "ToastEnabled",
    )?;
    let per_app = query_dword(
        &format!(r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\{APP_ID}"),
        "Enabled",
    )?;
    allows_delivery(per_app, global)
}

fn allows_delivery(per_app: Option<u32>, global: Option<u32>) -> Result<bool, String> {
    match global {
        None | Some(1) => {}
        Some(0) => return Ok(false),
        Some(value) => return Err(format!("Unexpected ToastEnabled value {value}")),
    }
    match per_app {
        None | Some(1) => Ok(true),
        Some(0) => Ok(false),
        Some(value) => Err(format!("Unexpected notification Enabled value {value}")),
    }
}

fn payload(message: &str) -> ToastPayload {
    ToastPayload {
        app_id: APP_ID,
        title: "fbro",
        // WinRT escapes XML. Pre-escaping would show the entities to the user.
        body: message.to_string(),
        silent: true,
        normal_urgency: true,
        default_timeout: true,
        actions: Vec::new(),
    }
}

pub fn send(message: &str) -> Result<(), String> {
    let payload = payload(message);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("os-notification-submit".into())
        .spawn(move || {
            let _ = tx.send(submit(&payload));
        })
        .map_err(|error| format!("Cannot submit desktop notification: {error}"))?;
    match rx.recv_timeout(SERVICE_TIMEOUT) {
        Ok(result) => result,
        Err(error) => Err(format!("Desktop notification service timed out: {error}")),
    }
}

fn submit(payload: &ToastPayload) -> Result<(), String> {
    if !payload.silent || !payload.normal_urgency || !payload.default_timeout || !payload.actions.is_empty() {
        return Err("Windows notification payload is not informational".into());
    }
    let mut notification = Notification::new();
    notification
        .app_id(payload.app_id)
        .summary(payload.title)
        .body(&payload.body)
        .urgency(Urgency::Normal)
        .timeout(Timeout::Default);
    // Drop the handle immediately. Waiting would block until the user dismisses
    // the toast. Action Center owns the displayed lifetime.
    notification
        .show()
        .map(|_| ())
        .map_err(|error| format!("Cannot send desktop notification: {error}"))
}

pub fn in_build_tree(exe: &Path) -> bool {
    let mut components = exe.components();
    while let Some(component) = components.next() {
        if component.as_os_str() == "target" {
            if let Some(next) = components.next() {
                let name = next.as_os_str();
                if name == "debug" || name == "release" {
                    return true;
                }
            }
        }
    }
    false
}

pub fn identity_matches(exe: &Path, link_target: &Path, app_id: &str) -> bool {
    app_id == APP_ID && !in_build_tree(exe) && paths_equal(exe, link_target)
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

fn path_key(path: &Path) -> Vec<String> {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(text.as_ref());
    Path::new(text)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect()
}

fn installed_identity(exe: &Path) -> bool {
    // Cargo test and `tauri dev` live under target/{debug,release}. Never touch
    // COM or WinRT for those executables.
    if in_build_tree(exe) {
        return false;
    }
    let com = match ComInit::new() {
        Ok(com) => com,
        Err(error) => {
            eprintln!("[notifications] Cannot initialize COM for notification identity: {error}");
            return false;
        }
    };
    let mut matched = false;
    for candidate in shortcut_candidates() {
        if !candidate.is_file() {
            continue;
        }
        match read_shortcut(&candidate) {
            Ok((app_id, target)) if identity_matches(exe, Path::new(&target), &app_id) => {
                matched = true;
                break;
            }
            Ok(_) => {}
            Err(error) => {
                eprintln!(
                    "[notifications] Cannot read Start Menu shortcut {}: {error}",
                    candidate.display()
                );
            }
        }
    }
    drop(com);
    matched
}

fn shortcut_candidates() -> Vec<PathBuf> {
    let mut programs = Vec::new();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        programs.push(PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs"));
    }
    let common = std::env::var_os("ProgramData")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    programs.push(common.join(r"Microsoft\Windows\Start Menu\Programs"));
    shortcut_paths(programs)
}

fn shortcut_paths(programs_dirs: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    programs_dirs
        .into_iter()
        .flat_map(|root| [root.join(r"fbro\fbro.lnk"), root.join("fbro.lnk")])
        .collect()
}

fn read_shortcut(path: &Path) -> Result<(String, String), String> {
    let wide = wide_path(path)?;
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| format!("Cannot open the shell link: {error}"))?;
        let file: IPersistFile = link
            .cast()
            .map_err(|error| format!("Cannot persist the shell link: {error}"))?;
        file.Load(PCWSTR(wide.as_ptr()), STGM_READ)
            .map_err(|error| format!("Cannot load the shell link: {error}"))?;
        let mut buffer = vec![0u16; 32_768];
        link.GetPath(&mut buffer, std::ptr::null_mut(), 0)
            .map_err(|error| format!("Cannot read the shortcut target: {error}"))?;
        let target = utf16_lossy(&buffer);
        let store: IPropertyStore = link
            .cast()
            .map_err(|error| format!("Cannot read shortcut properties: {error}"))?;
        let variant = store
            .GetValue(&APP_USER_MODEL_ID)
            .map_err(|error| format!("Cannot read the shortcut application id: {error}"))?;
        let app_id = prop_string(variant)?;
        Ok((app_id, target))
    }
}

fn prop_string(mut variant: PROPVARIANT) -> Result<String, String> {
    let result = (|| {
        let vt = unsafe { variant.Anonymous.Anonymous.vt };
        if vt == VT_EMPTY {
            return Ok(String::new());
        }
        if vt != VT_LPWSTR && vt != VT_BSTR {
            return Err(format!("Shortcut application id has unexpected type {}", vt.0));
        }
        let owned = unsafe { PropVariantToStringAlloc(&variant) }
            .map_err(|error| format!("Cannot read the shortcut application id: {error}"))?;
        let text = utf16_lossy_ptr(owned.0);
        unsafe { CoTaskMemFree(Some(owned.0.cast())) };
        Ok(text)
    })();
    unsafe {
        let _ = PropVariantClear(&mut variant);
    }
    result
}

fn query_dword(subkey: &str, value: &str) -> Result<Option<u32>, String> {
    let subkey = wide(subkey);
    let value = wide(value);
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if status == ERROR_SUCCESS {
        Ok(Some(data))
    } else if status == ERROR_FILE_NOT_FOUND {
        Ok(None)
    } else {
        Err(format!("Cannot read Windows notification setting ({})", status.0))
    }
}

struct ComInit {
    uninit: bool,
}

impl ComInit {
    fn new() -> Result<Self, String> {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr == RPC_E_CHANGED_MODE {
            return Ok(Self { uninit: false });
        }
        if hr.is_err() {
            return Err(hr.to_string());
        }
        Ok(Self { uninit: true })
    }
}

impl Drop for ComInit {
    fn drop(&mut self) {
        if self.uninit {
            unsafe { CoUninitialize() };
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(path: &Path) -> Result<Vec<u16>, String> {
    let text = path.to_str().ok_or_else(|| format!("Shortcut path is not Unicode: {}", path.display()))?;
    Ok(wide(text))
}

fn utf16_lossy(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

fn utf16_lossy_ptr(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    unsafe {
        while len < 32_768 && *ptr.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POWERSHELL_APP_ID: &str =
        "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe";

    #[test]
    fn windows_notifications_development_keeps_settings_available_without_native_delivery() {
        use crate::platform::Platform;
        let exe = std::env::current_exe().unwrap();
        assert!(in_build_tree(&exe), "the test executable must stay inside Cargo's build tree");
        let platform = super::super::WindowsPlatform;
        assert_eq!(platform.os_notification_platform(), "windows");
        assert!(platform.supports_os_notification_settings());
        assert!(!platform.supports_os_notifications(), "Cargo's test executable must not initialize WinRT toasts");
    }

    #[test]
    fn windows_notifications_require_installed_identity() {
        let installed = Path::new(r"C:\Program Files\fbro\fbro.exe");
        assert!(identity_matches(installed, installed, APP_ID));
        assert!(identity_matches(
            Path::new(r"\\?\C:\Program Files\fbro\fbro.exe"),
            Path::new(r"C:/Program Files/fbro/fbro.exe"),
            APP_ID
        ));
        for exe in [
            r"C:\src\fbro\src-tauri\target\debug\fbro.exe",
            r"C:\src\fbro\src-tauri\target\release\fbro.exe",
            r"C:\src\fbro\src-tauri\target\debug\deps\fbro-abc123.exe",
        ] {
            let exe = Path::new(exe);
            assert!(in_build_tree(exe));
            assert!(!identity_matches(exe, exe, APP_ID));
        }
        assert!(!identity_matches(installed, Path::new(r"C:\Program Files\other\fbro.exe"), APP_ID));
        assert!(!identity_matches(installed, installed, "com.example.other"));
        assert!(!identity_matches(installed, installed, POWERSHELL_APP_ID));
        assert_ne!(APP_ID, POWERSHELL_APP_ID);
        let config: serde_json::Value = serde_json::from_str(include_str!("../../../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], APP_ID);
    }

    #[test]
    fn windows_notifications_payload_is_plain_silent_and_informational() {
        let message = "peer <雪> & file\nsecond line";
        let toast = payload(message);
        assert_eq!(toast.app_id, APP_ID);
        assert_eq!(toast.title, "fbro");
        assert_eq!(toast.body, message);
        assert!(toast.silent);
        assert!(toast.normal_urgency);
        assert!(toast.default_timeout);
        assert!(toast.actions.is_empty());
        assert_ne!(toast.app_id, POWERSHELL_APP_ID);
    }

    #[test]
    fn windows_notifications_authorization_respects_explicit_disables() {
        assert_eq!(allows_delivery(None, None).unwrap(), true);
        assert_eq!(allows_delivery(Some(1), Some(1)).unwrap(), true);
        assert_eq!(allows_delivery(Some(0), None).unwrap(), false);
        assert_eq!(allows_delivery(None, Some(0)).unwrap(), false);
        assert_eq!(allows_delivery(Some(1), Some(0)).unwrap(), false);
        assert!(allows_delivery(Some(2), None).is_err());
        assert!(allows_delivery(None, Some(7)).is_err());
    }

    #[test]
    fn windows_notifications_settings_link_targets_the_system_page() {
        assert_eq!(SETTINGS_URI, "ms-settings:notifications");
    }

    #[test]
    fn windows_notifications_shortcuts_use_the_tauri_start_menu_layout() {
        let user = PathBuf::from(r"C:\Users\me\AppData\Roaming\Microsoft\Windows\Start Menu\Programs");
        let common = PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs");
        let paths = shortcut_paths([user.clone(), common.clone()]);
        assert_eq!(paths, vec![
            user.join(r"fbro\fbro.lnk"),
            user.join("fbro.lnk"),
            common.join(r"fbro\fbro.lnk"),
            common.join("fbro.lnk"),
        ]);
    }
}
