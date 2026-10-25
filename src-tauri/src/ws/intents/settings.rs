use std::sync::Arc;

use serde_json::Value;

use super::arg_str;

use crate::state::settings::ModalSizePreference;
use crate::vfs::VfsDriver;
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{rescan_panel, selected_names_for, IntentContext};

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "SetGeneralOption" => set_general_option(args, ctx).await,
        "SetModalSizePreference" => set_modal_size_preference(args, ctx).await,
        "ClearIconCache" => clear_icon_cache(args, ctx).await,
        "OpenOsNotificationSettings" => open_os_notification_settings(args, ctx).await,
        "ClearThumbnailCache" => clear_thumbnail_cache(ctx).await,
        "ResetLocationSortSettings" => reset_location_sort_settings(ctx).await,
        _ => {}
    }
}

async fn open_os_notification_settings(args: &Value, ctx: &mut IntentContext<'_>) {
    let platform = ctx.state.lock().await.platform.clone();
    let reply = open_notification_settings_reply(args, move || async move {
        tokio::task::spawn_blocking(move || platform.open_os_notification_settings())
            .await
            .map_err(|error| format!("Open OS notification settings worker: {error}"))?
    }).await;
    let _ = ctx.tx.send(reply).await;
}

async fn open_notification_settings_reply<F, Fut>(args: &Value, open: F) -> ServerMessage
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let request_id = arg_str(args, "requestId").unwrap_or_default().to_string();
    let result = if request_id.trim().is_empty() {
        Err("A requestId is required".into())
    } else {
        open().await
    };
    match result {
        Ok(()) => ServerMessage::OpenOsNotificationSettingsOk { timestamp: now(), request_id },
        Err(message) => ServerMessage::OpenOsNotificationSettingsFailed {
            timestamp: now(), request_id, code: "OPEN_OS_NOTIFICATION_SETTINGS_FAILED".into(), message,
        },
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

#[cfg(test)]
mod notification_settings_tests {
    use super::*;

    #[tokio::test]
    async fn notification_settings_open_rejects_invalid_requests_without_native_work() {
        for args in [serde_json::json!({}), serde_json::json!({"requestId": "  "}), serde_json::json!({"requestId": 4})] {
            let reply = open_notification_settings_reply(&args, || async {
                panic!("Invalid requests must not open native settings");
            }).await;
            assert!(matches!(reply, ServerMessage::OpenOsNotificationSettingsFailed { .. }));
        }
    }

    #[tokio::test]
    async fn notification_settings_open_replies_correlate_success_and_host_failure() {
        let args = serde_json::json!({"requestId": "settings-1"});
        let ok = serde_json::to_value(open_notification_settings_reply(&args, || async { Ok(()) }).await).unwrap();
        assert_eq!(ok["type"], "OpenOsNotificationSettingsOk");
        assert_eq!(ok["requestId"], "settings-1");
        let failed = serde_json::to_value(open_notification_settings_reply(&args, || async { Err("Host refused to open settings".into()) }).await).unwrap();
        assert_eq!(failed["type"], "OpenOsNotificationSettingsFailed");
        assert_eq!(failed["requestId"], "settings-1");
        assert_eq!(failed["code"], "OPEN_OS_NOTIFICATION_SETTINGS_FAILED");
        assert_eq!(failed["message"], "Host refused to open settings");
    }
}

async fn set_modal_size_preference(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(modal_id) = args.get("modalId").and_then(Value::as_str) else {
        bad_setting(ctx.tx, "modalId must be a string").await;
        return;
    };
    if modal_id.is_empty()
        || modal_id.len() > 80
        || !modal_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
    {
        bad_setting(ctx.tx, "modalId contains unsupported characters").await;
        return;
    }

    let reset = args.get("reset").and_then(Value::as_bool).unwrap_or(false);
    let (modal_sizes, gui_senders) = {
        let mut state_guard = ctx.state.lock().await;
        if reset {
            state_guard.settings.modal_sizes.remove(modal_id);
        } else {
            let Some(width) = args.get("width").and_then(Value::as_u64) else {
                drop(state_guard);
                bad_setting(ctx.tx, "width must be a positive integer").await;
                return;
            };
            let Some(height) = args.get("height").and_then(Value::as_u64) else {
                drop(state_guard);
                bad_setting(ctx.tx, "height must be a positive integer").await;
                return;
            };
            state_guard.settings.modal_sizes.insert(
                modal_id.to_string(),
                ModalSizePreference {
                    width: width.clamp(240, 2400) as u32,
                    height: height.clamp(160, 1600) as u32,
                },
            );
        }

        if let Err(e) = crate::state::settings::save(&state_guard.settings) {
            eprintln!("[fbro] Failed to save modal size preference: {e}");
        }
        (
            state_guard.settings.modal_sizes.clone(),
            state_guard.gui_senders.clone(),
        )
    };

    let patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "settings".to_string(),
        entity_id: None,
        version: 0,
        patch: serde_json::json!({ "modalSizes": modal_sizes }),
    };
    for sender in &gui_senders {
        let _ = sender.send(patch.clone()).await;
    }
    let _ = ctx.tx.send(patch).await;
}

async fn set_general_option(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(key) = args.get("key").and_then(Value::as_str) else {
        return;
    };
    let Some(value) = args.get("value") else {
        return;
    };

    if key == "rememberSortPerLocation" {
        let Some(enable) = value.as_bool() else {
            bad_setting(ctx.tx, "rememberSortPerLocation must be a boolean").await;
            return;
        };
        if enable {
            if let Err(error) = expire_location_sorts_if_needed(ctx).await {
                bad_setting(ctx.tx, &error).await;
                return;
            }
        }
    }

    // Track whether show_hidden changed so we can propagate to panels.
    let mut show_hidden_changed: Option<bool> = None;
    // Track whether proc_tree_mode changed so we can reload the procs driver.
    let mut proc_tree_mode_changed: Option<bool> = None;
    let mut prepare_notifications = None;

    let (gui_senders, viewer_senders) = {
        let mut state_guard = ctx.state.lock().await;

        match key {
            "osNotificationsEnabled" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "osNotificationsEnabled must be a boolean").await;
                    return;
                };
                state_guard.settings.general.os_notifications_enabled = v;
                state_guard.notifications.set_os_enabled(v);
                if v {
                    prepare_notifications = Some(state_guard.notifications.clone());
                }
            }
            "showButtonBar" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "showButtonBar must be a boolean").await;
                    return;
                };
                state_guard.settings.general.show_button_bar = v;
            }
            "rememberWindowState" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "rememberWindowState must be a boolean").await;
                    return;
                };
                state_guard.settings.general.remember_window_state = v;
            }
            "showHiddenDefault" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "showHiddenDefault must be a boolean").await;
                    return;
                };
                state_guard.settings.general.show_hidden_default = v;
                show_hidden_changed = Some(v);
            }
            "preserveAttributes" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "preserveAttributes must be a boolean").await;
                    return;
                };
                state_guard.settings.general.preserve_attributes = v;
            }
            "autoResumeCopies" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "autoResumeCopies must be a boolean").await;
                    return;
                };
                state_guard.settings.general.auto_resume_copies = v;
            }
            "enableHeic2any" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "enableHeic2any must be a boolean").await;
                    return;
                };
                state_guard.settings.general.enable_heic2any = v;
            }
            "ludicrousMode" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "ludicrousMode must be a boolean").await;
                    return;
                };
                state_guard.settings.general.ludicrous_mode = v;
            }
            "maxImageSizeMb" => {
                let Some(v) = value.as_u64() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "maxImageSizeMb must be a positive integer").await;
                    return;
                };
                state_guard.settings.general.max_image_size_mb = v.clamp(1, 2048);
            }
            "enableVideoThumbnails" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "enableVideoThumbnails must be a boolean").await;
                    return;
                };
                state_guard.settings.general.enable_video_thumbnails = v;
            }
            "thumbnailMaxImageSizeMb" => {
                let Some(v) = value.as_u64() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "thumbnailMaxImageSizeMb must be a positive integer").await;
                    return;
                };
                state_guard.settings.general.thumbnail_max_image_size_mb = v.clamp(1, 4096);
            }
            "thumbnailVideoFrameSkip" => {
                let Some(v) = value.as_u64() else {
                    drop(state_guard);
                    bad_setting(
                        ctx.tx,
                        "thumbnailVideoFrameSkip must be a non-negative integer",
                    )
                    .await;
                    return;
                };
                state_guard.settings.general.thumbnail_video_frame_skip = (v.min(60)) as u32;
            }
            "streamThumbnails" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "streamThumbnails must be a boolean").await;
                    return;
                };
                state_guard.settings.general.stream_thumbnails = v;
            }
            "persistPanelTabs" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "persistPanelTabs must be a boolean").await;
                    return;
                };
                state_guard.settings.general.persist_panel_tabs = v;
            }
            "procTreeMode" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "procTreeMode must be a boolean").await;
                    return;
                };
                state_guard.settings.general.proc_tree_mode = v;
                // Replace the procs driver and toggle the InternalNavigation
                // capability so any active procs panel switches mode on the
                // next rescan.
                if state_guard.drivers.contains_key("procs") {
                    state_guard.drivers.insert(
                        "procs".to_string(),
                        Arc::new(crate::vfs::procs::ProcsDriver { hierarchical: v })
                            as Arc<dyn VfsDriver>,
                    );
                    if let Some(src) = state_guard.sources.get_mut("procs") {
                        let cap = crate::ws::intents::CAP_INTERNAL_NAV.to_string();
                        src.supports.retain(|c| c != &cap);
                        if v {
                            src.supports.push(cap);
                        }
                    }
                }
                proc_tree_mode_changed = Some(v);
            }
            "colorScheme" => {
                let Some(v) = value.as_str() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "colorScheme must be a string").await;
                    return;
                };
                match v {
                    "fbro-dark" | "daylight" => {
                        state_guard.settings.general.color_scheme = v.to_string();
                    }
                    _ => {
                        let msg = format!("Unknown color scheme: {v}");
                        drop(state_guard);
                        bad_setting(ctx.tx, &msg).await;
                        return;
                    }
                }
            }
            "savePanelViewMode" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "savePanelViewMode must be a boolean").await;
                    return;
                };
                state_guard.settings.general.save_panel_view_mode = v;
                if !v {
                    // Clear the persisted view-mode map so disabling the
                    // option doesn't leave stale entries that would re-apply
                    // if the user re-enables it later.
                    state_guard.settings.panel_view_modes.clear();
                }
            }
            "rememberSortPerLocation" => {
                let v = value.as_bool().expect("validated above");
                let was_enabled = state_guard.settings.general.remember_sort_per_location;
                state_guard.settings.general.remember_sort_per_location = v;
                if was_enabled && !v {
                    state_guard.settings.location_sort_disabled_at_ms =
                        (!state_guard.location_settings.is_empty()).then_some(now());
                } else if v {
                    state_guard.settings.location_sort_disabled_at_ms = None;
                }
            }
            "sevenzipExtractor" => {
                let Some(v) = value.as_str() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "sevenzipExtractor must be a string").await;
                    return;
                };
                match crate::state::settings::SevenzipExtractorMode::from_str(v) {
                    Some(mode) => {
                        state_guard.settings.general.sevenzip_extractor = mode;
                    }
                    None => {
                        let msg = format!("Unknown sevenzipExtractor: {v}");
                        drop(state_guard);
                        bad_setting(ctx.tx, &msg).await;
                        return;
                    }
                }
            }
            "sevenzipCustomPath" => {
                let Some(v) = value.as_str() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "sevenzipCustomPath must be a string").await;
                    return;
                };
                state_guard.settings.general.sevenzip_custom_path = v.trim().to_string();
            }
            "fedupDefaultIgnorePatterns" => {
                let Some(v) = value.as_str() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "fedupDefaultIgnorePatterns must be a string").await;
                    return;
                };
                state_guard.settings.general.fedup_default_ignore_patterns =
                    normalize_fedup_ignore_patterns(v);
            }
            "compareMaxFileSizeMb" => {
                let Some(v) = value.as_u64() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "compareMaxFileSizeMb must be a positive integer").await;
                    return;
                };
                state_guard.settings.general.compare_max_file_size_mb = v.clamp(1, 1024);
            }
            "compareDifferMode" => {
                let Some(v) = value.as_str() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "compareDifferMode must be a string").await;
                    return;
                };
                match crate::state::settings::CompareDifferMode::from_str(v) {
                    Some(mode) => {
                        state_guard.settings.general.compare_differ_mode = mode;
                    }
                    None => {
                        let msg = format!("Unknown compareDifferMode: {v}");
                        drop(state_guard);
                        bad_setting(ctx.tx, &msg).await;
                        return;
                    }
                }
            }
            "compareExternalCommand" => {
                let Some(v) = value.as_str() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "compareExternalCommand must be a string").await;
                    return;
                };
                state_guard.settings.general.compare_external_command = v.trim().to_string();
            }
            "quickTerminalEditor" => {
                let Some(v) = value.as_str() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "quickTerminalEditor must be a string").await;
                    return;
                };
                match crate::state::settings::QuickTerminalEditor::from_str(v) {
                    Some(editor) => state_guard.settings.general.quick_terminal_editor = editor,
                    None => {
                        let msg = format!("Unknown quickTerminalEditor: {v}");
                        drop(state_guard);
                        bad_setting(ctx.tx, &msg).await;
                        return;
                    }
                }
            }
            "zshEmacsKeymap" => {
                let Some(v) = value.as_bool() else {
                    drop(state_guard);
                    bad_setting(ctx.tx, "zshEmacsKeymap must be a boolean").await;
                    return;
                };
                state_guard.settings.general.zsh_emacs_keymap = v;
            }
            _ => {
                let msg = format!("Unknown setting key: {key}");
                drop(state_guard);
                bad_setting(ctx.tx, &msg).await;
                return;
            }
        }

        // Save the full Settings struct so the panel_view_modes clear above
        // (when savePanelViewMode is disabled) actually lands on disk.
        if let Err(e) = crate::state::settings::save(&state_guard.settings) {
            eprintln!("[fbro] Failed to save settings: {e}");
        }

        // Push the latest thumbnail knobs into the live atomics so the worker
        // pool sees changes without a restart.
        #[cfg(target_os = "linux")]
        crate::platform::linux::thumbnail::apply_settings(
            state_guard.settings.general.enable_video_thumbnails,
            state_guard.settings.general.thumbnail_max_image_size_mb,
            state_guard.settings.general.thumbnail_video_frame_skip,
        );

        (
            state_guard.gui_senders.clone(),
            state_guard.viewer_senders.clone(),
        )
    };

    if let Some(notifications) = prepare_notifications {
        if let Err(error) = ctx.app_handle.run_on_main_thread(move || {
            notifications.prepare_os_notifications();
        }) {
            eprintln!("[notifications] Cannot prepare OS authorization: {error}");
        }
    }

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
                let version = state_guard
                    .panels
                    .get(panel_id)
                    .map(|p| p.version)
                    .unwrap_or(0);
                let names = selected_names_for(&state_guard, panel_id);
                (version, names)
            };

            // Notify FE of the show_hidden change before the rescan patch arrives.
            let _ = ctx
                .tx
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
                ctx.watcher_cmd_tx,
            )
            .await;
        }
    }

    // Rescan any panels currently showing the procs source.
    if proc_tree_mode_changed.is_some() {
        let procs_panel_ids: Vec<String> = {
            let state_guard = ctx.state.lock().await;
            state_guard
                .panels
                .iter()
                .filter(|(_, p)| p.source_id == "procs")
                .map(|(id, _)| id.clone())
                .collect()
        };
        for panel_id in &procs_panel_ids {
            rescan_panel(
                panel_id,
                ctx.state,
                ctx.tx,
                ctx.scan_handles,
                None,
                None,
                ctx.dir_size_queue,
                ctx.watcher_cmd_tx,
            )
            .await;
        }
    }

    let (general, location_settings_count) = {
        let state_guard = ctx.state.lock().await;
        (
            state_guard.settings.general.clone(),
            state_guard.location_settings.len(),
        )
    };

    let sevenzip_resolved = crate::jobs::extract_archive::resolve_sevenzip_binary(
        general.sevenzip_extractor,
        &general.sevenzip_custom_path,
    )
    .map(|p| p.to_string_lossy().to_string())
    .unwrap_or_default();

    let patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "settings".to_string(),
        entity_id: None,
        version: 0,
        patch: serde_json::json!({
            "general": {
                "showHiddenDefault": general.show_hidden_default,
                "showButtonBar": general.show_button_bar,
                "osNotificationsEnabled": general.os_notifications_enabled,
                "rememberWindowState": general.remember_window_state,
                "preserveAttributes": general.preserve_attributes,
                "autoResumeCopies": general.auto_resume_copies,
                "ludicrousMode": general.ludicrous_mode,
                "enableHeic2any": general.enable_heic2any,
                "maxImageSizeMb": general.max_image_size_mb,
                "savePanelViewMode": general.save_panel_view_mode,
                "rememberSortPerLocation": general.remember_sort_per_location,
                "locationSettingsCount": location_settings_count,
                "enableVideoThumbnails": general.enable_video_thumbnails,
                "thumbnailMaxImageSizeMb": general.thumbnail_max_image_size_mb,
                "thumbnailVideoFrameSkip": general.thumbnail_video_frame_skip,
                "persistPanelTabs": general.persist_panel_tabs,
                "procTreeMode": general.proc_tree_mode,
                "colorScheme": general.color_scheme,
                "sevenzipExtractor": general.sevenzip_extractor.as_str(),
                "sevenzipCustomPath": general.sevenzip_custom_path,
                "sevenzipResolvedPath": sevenzip_resolved,
                "fedupDefaultIgnorePatterns": general.fedup_default_ignore_patterns,
                "compareMaxFileSizeMb": general.compare_max_file_size_mb,
                "compareDifferMode": general.compare_differ_mode.as_str(),
                "compareExternalCommand": general.compare_external_command,
                "quickTerminalEditor": general.quick_terminal_editor.program(),
                "zshEmacsKeymap": general.zsh_emacs_keymap,
            }
        }),
    };
    for sender in &gui_senders {
        let _ = sender.send(patch.clone()).await;
    }
    for sender in &viewer_senders {
        let _ = sender.send(patch.clone()).await;
    }
    let _ = ctx.tx.send(patch).await;
}

async fn expire_location_sorts_if_needed(ctx: &mut IntentContext<'_>) -> Result<(), String> {
    let (disabled_at_ms, cached_count) = {
        let state_guard = ctx.state.lock().await;
        (
            state_guard.settings.location_sort_disabled_at_ms,
            state_guard.location_settings.len(),
        )
    };
    let Some(disabled_at_ms) = disabled_at_ms else {
        return Ok(());
    };
    if !crate::state::location_settings::retention_expired(disabled_at_ms, now()) {
        return Ok(());
    }

    let result = match tokio::task::spawn_blocking(crate::state::location_settings::clear_sorts).await
    {
        Ok(result) => result,
        Err(error) => {
            let error = error.to_string();
            crate::journal::record_location_sort_expiry("failed", 0, Some(&error));
            return Err(format!(
                "Could not expire {cached_count} remembered location settings: {error}"
            ));
        }
    };
    match result {
        Ok(removed_count) => {
            let mut state_guard = ctx.state.lock().await;
            state_guard.location_settings.clear();
            state_guard.settings.location_sort_disabled_at_ms = None;
            if let Err(error) = crate::state::settings::save(&state_guard.settings) {
                eprintln!("[fbro] Failed to save expired location settings state: {error}");
            }
            crate::journal::record_location_sort_expiry("completed", removed_count, None);
            Ok(())
        }
        Err(error) => {
            crate::journal::record_location_sort_expiry("failed", 0, Some(&error));
            Err(format!(
                "Could not expire {cached_count} remembered location settings: {error}"
            ))
        }
    }
}

async fn reset_location_sort_settings(ctx: &mut IntentContext<'_>) {
    let cached_count = {
        let state_guard = ctx.state.lock().await;
        state_guard.location_settings.len()
    };
    let operation = crate::journal::record_location_sort_reset_intent(cached_count);
    let result = tokio::task::spawn_blocking(crate::state::location_settings::clear_sorts).await;
    let removed_count = match result {
        Ok(Ok(removed_count)) => removed_count,
        Ok(Err(error)) => {
            crate::journal::record_location_sort_reset_result(
                &operation,
                "failed",
                0,
                Some(&error),
            );
            bad_setting(
                ctx.tx,
                &format!("Could not reset remembered location settings: {error}"),
            )
            .await;
            return;
        }
        Err(error) => {
            let error = error.to_string();
            crate::journal::record_location_sort_reset_result(
                &operation,
                "failed",
                0,
                Some(&error),
            );
            bad_setting(
                ctx.tx,
                &format!("Could not reset remembered location settings: {error}"),
            )
            .await;
            return;
        }
    };

    let gui_senders = {
        let mut state_guard = ctx.state.lock().await;
        state_guard.location_settings.clear();
        state_guard.settings.location_sort_disabled_at_ms = None;
        if let Err(error) = crate::state::settings::save(&state_guard.settings) {
            eprintln!("[fbro] Failed to save reset location settings state: {error}");
        }
        state_guard.gui_senders.clone()
    };
    crate::journal::record_location_sort_reset_result(
        &operation,
        "completed",
        removed_count,
        None,
    );
    let patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "settings".to_string(),
        entity_id: None,
        version: 0,
        patch: serde_json::json!({ "general": { "locationSettingsCount": 0 } }),
    };
    for sender in &gui_senders {
        let _ = sender.send(patch.clone()).await;
    }
    let _ = ctx.tx.send(patch).await;
}

fn normalize_fedup_ignore_patterns(value: &str) -> String {
    crate::jobs::fedup::parse_fedup_patterns(value).join("\n")
}

async fn clear_thumbnail_cache(ctx: &mut IntentContext<'_>) {
    // Drop the in-memory L1 cache regardless of platform.
    let (cache, gui_senders) = {
        let state_guard = ctx.state.lock().await;
        (
            state_guard.thumb_cache.clone(),
            state_guard.gui_senders.clone(),
        )
    };
    cache.clear();

    // Wipe the L2 disk cache on Linux (the only platform that owns one).
    #[cfg(target_os = "linux")]
    {
        let (_removed, errors) = crate::platform::linux::thumbnail::clear_disk_cache();
        if !errors.is_empty() {
            let msg = format!(
                "Some thumbnail cache files could not be removed: {}",
                errors.join("; ")
            );
            bad_setting(ctx.tx, &msg).await;
            return;
        }
    }

    // Broadcast refreshed stats so the UI reflects the empty cache.
    let patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "settings".to_string(),
        entity_id: None,
        version: 0,
        patch: serde_json::json!({
            "general": {
                "thumbnailCacheItemCount": 0u64,
                "thumbnailCacheSizeBytes": 0u64,
            }
        }),
    };
    for sender in &gui_senders {
        let _ = sender.send(patch.clone()).await;
    }
    let _ = ctx.tx.send(patch).await;
}

async fn clear_icon_cache(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = super::arg_str(args, "requestId").unwrap_or_default().to_string();
    let (platform, senders, notifications) = {
        let state = ctx.state.lock().await;
        (state.platform.clone(), state.gui_senders.clone(), state.notifications.clone())
    };
    let result = if request_id.is_empty() {
        Err("A requestId is required".to_string())
    } else {
        tokio::task::spawn_blocking(move || platform.clear_icon_cache()).await
            .map_err(|error| format!("Clear file icon cache worker: {error}"))
            .and_then(|result| result)
    };
    let reply = match result {
        Ok(revision) => {
            let patch = ServerMessage::StatePatch {
                timestamp: now(), domain: "settings".into(), entity_id: None, version: 0,
                patch: serde_json::json!({"general": {"iconCacheRevision": revision}}),
            };
            for sender in senders { let _ = sender.send(patch.clone()).await; }
            notifications.publish(crate::notifications::NotificationInput::new("Cleared file icon cache"));
            ServerMessage::ClearIconCacheOk { timestamp: now(), request_id }
        }
        Err(message) => ServerMessage::ClearIconCacheFailed {
            timestamp: now(), request_id, code: "CLEAR_ICON_CACHE_FAILED".into(), message,
        },
    };
    let _ = ctx.tx.send(reply).await;
}
