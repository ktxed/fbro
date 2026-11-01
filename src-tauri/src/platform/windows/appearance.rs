//! Windows system appearance: the per-user "app mode"
//! (`HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`,
//! `AppsUseLightTheme`), watched with `RegNotifyChangeKeyValue`.

use super::registry::{query_hkcu_dword, wide};
use crate::platform::appearance::{windows_apps_use_light_theme, SystemColorScheme};
use tokio::sync::watch;
use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY,
    KEY_QUERY_VALUE, REG_NOTIFY_CHANGE_LAST_SET,
};

const PERSONALIZE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const VALUE: &str = "AppsUseLightTheme";

fn read() -> Option<SystemColorScheme> {
    match query_hkcu_dword(PERSONALIZE_KEY, VALUE) {
        Ok(value) => Some(windows_apps_use_light_theme(value)),
        Err(code) => {
            eprintln!("[appearance] cannot read AppsUseLightTheme ({code})");
            None
        }
    }
}

pub async fn start(tx: watch::Sender<Option<SystemColorScheme>>) {
    let initial = tokio::task::spawn_blocking(read).await.unwrap_or(None);
    tx.send_replace(initial);
    let spawned = std::thread::Builder::new()
        .name("fbro-appearance".into())
        .spawn(move || watch_key(tx));
    if let Err(error) = spawned {
        eprintln!("[appearance] cannot start registry watcher: {error}");
    }
}

/// Blocks on registry change notifications for the life of the app. The key
/// also changes for unrelated values (transparency, accent), so only real
/// scheme changes are published.
fn watch_key(tx: watch::Sender<Option<SystemColorScheme>>) {
    let subkey = wide(PERSONALIZE_KEY);
    let mut hkey = HKEY(std::ptr::null_mut());
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            KEY_NOTIFY | KEY_QUERY_VALUE,
            &mut hkey,
        )
    };
    if status != ERROR_SUCCESS {
        eprintln!("[appearance] cannot open Personalize key ({})", status.0);
        return;
    }
    loop {
        let status = unsafe {
            RegNotifyChangeKeyValue(hkey, false, REG_NOTIFY_CHANGE_LAST_SET, None, false)
        };
        if status != ERROR_SUCCESS {
            eprintln!("[appearance] registry change notification failed ({})", status.0);
            break;
        }
        if tx.is_closed() {
            break;
        }
        let next = read();
        tx.send_if_modified(|current| {
            if *current == next {
                return false;
            }
            *current = next;
            true
        });
    }
    unsafe {
        let _ = RegCloseKey(hkey);
    }
}
