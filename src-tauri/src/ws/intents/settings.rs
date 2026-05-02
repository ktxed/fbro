use serde_json::Value;

use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{IntentContext, rescan_panel, selected_names_for};

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "SetGeneralOption" => set_general_option(args, ctx).await,
        _ => {}
    }
}

async fn bad_setting(tx: &tokio::sync::mpsc::Sender<ServerMessage>, msg: &str) {
    let _ = tx
        .send(ServerMessage::Error {
            timestamp: now(),
            code: "BAD_SETTING".to_string(),
            message: msg.to_string(),
            recoverable: true,
            extra: None,
        })
        .await;
}

async fn set_general_option(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(key) = args.get("key").and_then(Value::as_str) else {
        return;
    };
    let Some(value) = args.get("value") else {
        return;
    };

    // Track whether show_hidden changed so we can propagate to panels.
    let mut show_hidden_changed: Option<bool> = None;

    let gui_senders = {
        let mut state_guard = ctx.state.lock().await;
        let general = &mut state_guard.settings.general;

        match key {
            "rememberWindowState" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "rememberWindowState must be a boolean").await;
                    return;
                };
                general.remember_window_state = v;
            }
            "showHiddenDefault" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "showHiddenDefault must be a boolean").await;
                    return;
                };
                general.show_hidden_default = v;
                show_hidden_changed = Some(v);
            }
            "preserveAttributes" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "preserveAttributes must be a boolean").await;
                    return;
                };
                general.preserve_attributes = v;
            }
            "ludicrousMode" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "ludicrousMode must be a boolean").await;
                    return;
                };
                general.ludicrous_mode = v;
            }
            "maxImageSizeMb" => {
                let Some(v) = value.as_u64() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "maxImageSizeMb must be a positive integer").await;
                    return;
                };
                general.max_image_size_mb = v.clamp(1, 2048);
            }
            _ => {
                let msg = format!("Unknown setting key: {key}");
                drop(state_guard);
                bad_setting(ctx.tx, &msg).await;
                return;
            }
        }

        if let Err(e) = crate::state::settings::save_general(general) {
            eprintln!("[fbro] Failed to save general settings: {e}");
        }

        state_guard.gui_senders.clone()
    };

    // Apply show_hidden_default to all panels immediately.
    if let Some(new_show_hidden) = show_hidden_changed {
        let panel_ids: Vec<String> = {
            let mut state_guard = ctx.state.lock().await;
            state_guard
                .panels
                .iter_mut()
                .filter_map(|(id, panel)| {
                    if panel.show_hidden != new_show_hidden {
                        panel.show_hidden = new_show_hidden;
                        panel.version += 1;
                        Some(id.clone())
                    } else {
                        None
                    }
                })
                .collect()
        };

        for panel_id in &panel_ids {
            let (version, selection_names) = {
                let state_guard = ctx.state.lock().await;
                let version = state_guard.panels.get(panel_id).map(|p| p.version).unwrap_or(0);
                let names = selected_names_for(&state_guard, panel_id);
                (version, names)
            };

            // Notify FE of the show_hidden change before the rescan patch arrives.
            let _ = ctx.tx
                .send(ServerMessage::StatePatch {
                    timestamp: now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id.clone()),
                    version,
                    patch: serde_json::json!({ "showHidden": new_show_hidden }),
                })
                .await;
            for sender in &gui_senders {
                let _ = sender
                    .send(ServerMessage::StatePatch {
                        timestamp: now(),
                        domain: "panels".to_string(),
                        entity_id: Some(panel_id.clone()),
                        version,
                        patch: serde_json::json!({ "showHidden": new_show_hidden }),
                    })
                    .await;
            }

            rescan_panel(
                panel_id,
                ctx.state,
                ctx.tx,
                ctx.scan_handles,
                None,
                selection_names,
                ctx.dir_size_queue,
            )
            .await;
        }
    }

    let general = {
        let state_guard = ctx.state.lock().await;
        state_guard.settings.general.clone()
    };

    let patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "settings".to_string(),
        entity_id: None,
        version: 0,
        patch: serde_json::json!({
            "general": {
                "showHiddenDefault": general.show_hidden_default,
                "rememberWindowState": general.remember_window_state,
                "preserveAttributes": general.preserve_attributes,
                "ludicrousMode": general.ludicrous_mode,
                "maxImageSizeMb": general.max_image_size_mb,
            }
        }),
    };
    for sender in &gui_senders {
        let _ = sender.send(patch.clone()).await;
    }
    let _ = ctx.tx.send(patch).await;
}
