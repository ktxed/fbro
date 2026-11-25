//! One default fbro per user, on platforms that opt in (macOS and Linux).
//!
//! The bundled `.app` and `npm run tauri dev` share the bundle identifier and
//! `~/.config/fbro`. Run together, the second copy cannot bind the WebSocket
//! port, so its window silently drives the first copy's backend, and both
//! write the same session, settings and `share.json`. Instead, the second copy
//! asks the user (`Platform::ask_already_running`) and, unless they quit,
//! hands over to the first through `Platform::single_instance_plugin`.
//!
//! A running copy that predates the plugin never answers its handover, so the
//! default WebSocket port doubles as the "already running" signal: if it is
//! taken and the plugin did not exit us, we exit ourselves.
//!
//! Isolated instances (smoke tests, docs screenshots, the showcase) set
//! `FBRO_WS_BIND` and their own `XDG_CONFIG_HOME`, so they share nothing with
//! the user's fbro and are not guarded. Frontend-only windows connect to an
//! existing backend on purpose and are not guarded either.

use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::platform::{AlreadyRunningChoice, Platform};
use crate::ws::server::DEFAULT_WS_ADDR;

pub enum Startup {
    /// No other instance (or not guarded): start normally, registering the
    /// plugin if there is one so later launches hand over to us.
    Proceed { plugin: Option<tauri::plugin::TauriPlugin<tauri::Wry>> },
    /// Another instance runs and the user chose to show it. Register the
    /// plugin (it hands over and exits); if we get to app setup anyway, the
    /// running copy predates the plugin, so exit there.
    HandOver { plugin: tauri::plugin::TauriPlugin<tauri::Wry> },
    /// Another instance runs and the user chose Quit.
    Quit,
}

pub fn check(platform: &dyn Platform, frontend_only: bool) -> Startup {
    let custom_ws_bind = std::env::var_os("FBRO_WS_BIND").is_some();
    if !guarded(frontend_only, custom_ws_bind) {
        return Startup::Proceed { plugin: None };
    }
    let Some(plugin) = platform.single_instance_plugin() else {
        return Startup::Proceed { plugin: None };
    };
    if !default_ws_port_in_use() {
        return Startup::Proceed { plugin: Some(plugin) };
    }
    eprintln!("[fbro] fbro is already running (.app or dev build)");
    match platform.ask_already_running() {
        AlreadyRunningChoice::ShowRunning => Startup::HandOver { plugin },
        AlreadyRunningChoice::Quit => Startup::Quit,
    }
}

fn guarded(frontend_only: bool, custom_ws_bind: bool) -> bool {
    !frontend_only && !custom_ws_bind
}

/// True when something already listens on the default WebSocket address —
/// in practice, another fbro.
fn default_ws_port_in_use() -> bool {
    let addr: SocketAddr = DEFAULT_WS_ADDR.parse().expect("valid default ws addr");
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::guarded;

    #[test]
    fn only_the_default_instance_is_guarded() {
        assert!(guarded(false, false));
        assert!(!guarded(true, false), "frontend-only window");
        assert!(!guarded(false, true), "isolated FBRO_WS_BIND instance");
    }
}
