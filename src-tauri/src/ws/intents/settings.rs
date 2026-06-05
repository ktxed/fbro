use std::sync::Arc;

use serde_json::Value;

use crate::vfs::VfsDriver;
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{rescan_panel, selected_names_for, IntentContext};

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "SetGeneralOption" => set_general_option(args, ctx).await,
        "ClearThumbnailCache" => clear_thumbnail_cache(ctx).await,
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
    // Track whether proc_tree_mode changed so we can reload the procs driver.
    let mut proc_tree_mode_changed: Option<bool> = None;

    let (gui_senders, viewer_senders) = {
        let mut state_guard = ctx.state.lock().await;

        match key {
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
                "autoResumeCopies": general.auto_resume_copies,
                "ludicrousMode": general.ludicrous_mode,
                "maxImageSizeMb": general.max_image_size_mb,
                "savePanelViewMode": general.save_panel_view_mode,
                "enableVideoThumbnails": general.enable_video_thumbnails,
                "thumbnailMaxImageSizeMb": general.thumbnail_max_image_size_mb,
                "thumbnailVideoFrameSkip": general.thumbnail_video_frame_skip,
                "persistPanelTabs": general.persist_panel_tabs,
                "procTreeMode": general.proc_tree_mode,
                "colorScheme": general.color_scheme,
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
