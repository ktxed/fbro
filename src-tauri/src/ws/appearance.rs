//! Color-scheme projection shared by settings patches, snapshots, viewer
//! windows and the live OS appearance watcher.

use crate::platform::{SystemColorScheme, SystemColorSchemeRx};
use crate::state::settings::COLOR_SCHEME_LIGHT;
use crate::state::{AppState, SharedState};
use crate::ws::protocol::{now, ServerMessage};

/// Native background for new viewer/diff windows, so they open without a
/// flash of the wrong scheme before the webview paints.
pub(crate) fn window_background(scheme: &str) -> tauri::webview::Color {
    if scheme == COLOR_SCHEME_LIGHT {
        tauri::webview::Color(244, 246, 248, 255)
    } else {
        tauri::webview::Color(13, 13, 13, 255)
    }
}

pub(crate) struct AppearanceFields {
    pub follow_system_color_scheme: bool,
    pub active_color_scheme: &'static str,
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
        assert_eq!(window_background("daylight"), tauri::webview::Color(244, 246, 248, 255));
        assert_eq!(window_background("fbro-dark"), tauri::webview::Color(13, 13, 13, 255));
        assert_eq!(window_background("unknown"), tauri::webview::Color(13, 13, 13, 255));
    }
}
