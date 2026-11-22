//! macOS side of the single-instance guard (see `crate::single_instance`):
//! the handover plugin and the native "already running" alert.

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAlertStyle, NSApplication, NSApplicationActivationPolicy,
    NSModalPanelWindowLevel,
};
use objc2_foundation::NSString;
use tauri::{AppHandle, Manager, Runtime};

use crate::platform::AlreadyRunningChoice;

/// `tauri-plugin-single-instance`: a later launch connects to a Unix socket
/// keyed by the bundle identifier, the running instance brings its window to
/// the front, and the later launch exits during plugin setup.
pub(super) fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_single_instance::init(|app, argv, _cwd| {
        eprintln!(
            "[fbro] another launch ({}) handed over to this instance",
            argv.first().map(String::as_str).unwrap_or("?")
        );
        focus_main_window(app);
    })
}

fn focus_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// App-modal `NSAlert`. Runs before Tauri starts, so it bootstraps
/// `NSApplication` itself. Must be called on the main thread.
pub(super) fn ask_already_running() -> AlreadyRunningChoice {
    let Some(mtm) = MainThreadMarker::new() else {
        return AlreadyRunningChoice::ShowRunning;
    };
    let app = NSApplication::sharedApplication(mtm);
    // A dev binary launched from a terminal has no bundle; without a regular
    // activation policy its alert would open behind the terminal.
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    let alert = NSAlert::new(mtm);
    alert.setAlertStyle(NSAlertStyle::Warning);
    alert.setMessageText(&NSString::from_str("fbro is already running"));
    alert.setInformativeText(&NSString::from_str(
        "Another fbro (the installed app or a development build) is already running. \
         Only one can run at a time because they share settings, the session and \
         sharing.",
    ));
    alert.addButtonWithTitle(&NSString::from_str("Show Running fbro"));
    // Return picks the first button; NSAlert only maps Escape to a button
    // titled "Cancel", so bind it to Quit explicitly.
    alert
        .addButtonWithTitle(&NSString::from_str("Quit"))
        .setKeyEquivalent(&NSString::from_str("\u{1b}"));
    // Activation is cooperative on recent macOS, so a process started from a
    // terminal may not come to the front. Float the alert above other apps'
    // windows so it is visible either way.
    alert.layout();
    let window = alert.window();
    window.setLevel(NSModalPanelWindowLevel);
    window.orderFrontRegardless();

    if alert.runModal() == NSAlertFirstButtonReturn {
        AlreadyRunningChoice::ShowRunning
    } else {
        AlreadyRunningChoice::Quit
    }
}
