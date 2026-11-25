//! Linux side of the single-instance guard (see `crate::single_instance`):
//! the handover plugin and the GTK "already running" dialog.

use gtk::prelude::*;
use tauri::{AppHandle, Manager, Runtime};

use crate::platform::AlreadyRunningChoice;

/// `tauri-plugin-single-instance`: a later launch talks to the running
/// instance over D-Bus (name keyed by the bundle identifier), the running
/// instance brings its window to the front, and the later launch exits during
/// plugin setup.
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

/// Modal GTK dialog. Runs before Tauri starts, on the thread that called
/// `gtk::init()`. Enter picks "Show Running fbro", Escape picks "Quit".
pub(super) fn ask_already_running() -> AlreadyRunningChoice {
    if !gtk::is_initialized_main_thread() {
        return AlreadyRunningChoice::ShowRunning;
    }
    let dialog = gtk::MessageDialog::new(
        None::<&gtk::Window>,
        gtk::DialogFlags::MODAL,
        gtk::MessageType::Warning,
        gtk::ButtonsType::None,
        "fbro is already running",
    );
    dialog.set_secondary_text(Some(
        "Another fbro (the installed app or a development build) is already running. \
         Only one can run at a time because they share settings, the session and \
         sharing.",
    ));
    dialog.set_title("fbro");
    dialog.set_keep_above(true);
    dialog.add_button("Show Running fbro", gtk::ResponseType::Accept);
    dialog.add_button("Quit", gtk::ResponseType::Cancel);
    dialog.set_default_response(gtk::ResponseType::Accept);
    let response = dialog.run();
    dialog.close();
    // Let GTK process the close so the dialog disappears before we exit.
    while gtk::events_pending() {
        gtk::main_iteration();
    }
    if response == gtk::ResponseType::Accept {
        AlreadyRunningChoice::ShowRunning
    } else {
        AlreadyRunningChoice::Quit
    }
}
