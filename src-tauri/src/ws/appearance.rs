//! Color-scheme projection shared by settings patches, snapshots, viewer
//! windows and the live OS appearance watcher.

use crate::platform::{SystemColorScheme, SystemColorSchemeRx};
use crate::state::{AppState, SharedState};
use crate::ws::protocol::{now, ServerMessage};

/// Native background for new viewer/diff windows, so they open without a
/// flash of the wrong scheme before the webview paints.
pub(crate) fn window_background(
    scheme: &str,
    catalog: &crate::themes::ThemeCatalog,
) -> tauri::webview::Color {
    let (r, g, b, a) = crate::themes::window_background(scheme, catalog);
    tauri::webview::Color(r, g, b, a)
}

pub(crate) struct AppearanceFields {
    pub follow_system_color_scheme: bool,
    pub active_color_scheme: String,
    pub system_color_scheme: Option<&'static str>,
}

pub(crate) fn appearance_patch_fields(state: &AppState) -> AppearanceFields {
    AppearanceFields {
        follow_system_color_scheme: state.settings.general.follow_system_color_scheme,
        active_color_scheme: state.active_color_scheme(),
        system_color_scheme: state.system_color_scheme.map(SystemColorScheme::as_str),
    }
}

/// Record a new OS preference and broadcast the resulting projection to every
/// GUI and viewer client. Unchanged values publish nothing.
pub(crate) async fn apply_system_color_scheme(
    state: &SharedState,
    value: Option<SystemColorScheme>,
) {
    // Share the settings gate so this patch cannot interleave with an
    // in-flight SetGeneralOption patch and leave clients with a stale scheme.
    let mutation = { state.lock().await.settings_mutation.clone() };
    let _mutation = mutation.lock().await;
    let (patch, senders) = {
        let mut state_guard = state.lock().await;
        if state_guard.system_color_scheme == value {
            return;
        }
        state_guard.system_color_scheme = value;
        let fields = appearance_patch_fields(&state_guard);
        let patch = ServerMessage::StatePatch {
            timestamp: now(),
            domain: "settings".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({
                "general": {
                    "systemColorScheme": fields.system_color_scheme,
                    "activeColorScheme": fields.active_color_scheme,
                }
            }),
        };
        let senders: Vec<_> = state_guard
            .gui_senders
            .iter()
            .chain(&state_guard.viewer_senders)
            .cloned()
            .collect();
        (patch, senders)
    };
    for sender in senders {
        let _ = sender.send(patch.clone()).await;
    }
}

/// Replace the loaded catalog with `catalog` and broadcast it (plus the
/// possibly changed active scheme) to every GUI and viewer client. Unchanged
/// catalogs publish nothing. Callers hold the `settings_mutation` gate.
pub(crate) async fn publish_color_schemes(state: &SharedState, catalog: crate::themes::ThemeCatalog) {
    let (patch, senders) = {
        let mut state_guard = state.lock().await;
        if state_guard.color_schemes == catalog {
            return;
        }
        state_guard.color_schemes = catalog;
        let patch = ServerMessage::StatePatch {
            timestamp: now(),
            domain: "settings".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({
                "general": {
                    "colorSchemes": state_guard.color_schemes.to_wire(&crate::themes::themes_dir()),
                    "colorScheme": state_guard.settings.general.color_scheme.clone(),
                    "activeColorScheme": state_guard.active_color_scheme(),
                }
            }),
        };
        let senders: Vec<_> = state_guard
            .gui_senders
            .iter()
            .chain(&state_guard.viewer_senders)
            .cloned()
            .collect();
        (patch, senders)
    };
    for sender in senders {
        let _ = sender.send(patch.clone()).await;
    }
}

/// Reload `themes/` from disk and publish the result.
pub(crate) async fn reload_color_schemes(state: &SharedState) {
    let catalog = tokio::task::spawn_blocking(crate::themes::load_all)
        .await
        .unwrap_or_default();
    let mutation = { state.lock().await.settings_mutation.clone() };
    let _mutation = mutation.lock().await;
    publish_color_schemes(state, catalog).await;
}

/// Watch the themes folder so copying a `.toml` in imports it. Changes are
/// debounced; our own writes reload to an identical catalog and publish nothing.
pub(crate) async fn run_theme_watcher(state: SharedState) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let _watcher = match crate::themes::watch(crate::themes::themes_dir(), tx) {
        Ok(watcher) => watcher,
        Err(message) => {
            eprintln!("[themes] {message}");
            return;
        }
    };
    while rx.recv().await.is_some() {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        while rx.try_recv().is_ok() {}
        reload_color_schemes(&state).await;
    }
}

/// Forward live OS appearance changes until the watcher ends.
pub(crate) async fn run_appearance_updates(state: SharedState, mut rx: SystemColorSchemeRx) {
    while rx.changed().await.is_ok() {
        let value = *rx.borrow_and_update();
        apply_system_color_scheme(&state, value).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_background_matches_scheme() {
        let catalog = crate::themes::ThemeCatalog::default();
        assert_eq!(window_background("daylight", &catalog), tauri::webview::Color(244, 246, 248, 255));
        assert_eq!(window_background("fbro-dark", &catalog), tauri::webview::Color(13, 13, 13, 255));
        assert_eq!(window_background("unknown", &catalog), tauri::webview::Color(13, 13, 13, 255));
    }
}
