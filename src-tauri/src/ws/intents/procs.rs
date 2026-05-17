use std::sync::Arc;

use serde_json::Value;

use crate::state::{ShadowSource, Source};
use crate::vfs::VfsDriver;
use crate::watcher::WatcherCommand;
use crate::ws::intents::navigation::teardown_shadow_source;
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{
    build_sources_json, navigate_to, rescan_panel, resolve_panel_id, IntentContext,
};

pub async fn handle(name: &str, scope: &Value, _args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "EnterProcsSource" => enter_procs_source(scope, ctx).await,
        _ => {}
    }
}

async fn enter_procs_source(scope: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let source_id = "procs".to_string();

    {
        let mut state_guard = ctx.state.lock().await;

        let panel = match state_guard.panels.get(&panel_id) {
            Some(p) => p,
            None => return,
        };

        // Already browsing procs — treat as a toggle and exit back to the parent source.
        if panel.source_id == source_id {
            drop(state_guard);
            if let Some((_, parent_path, cursor_hint, cursor_index)) =
                teardown_shadow_source(&panel_id, ctx).await
            {
                navigate_to(
                    &panel_id,
                    &parent_path,
                    cursor_hint,
                    cursor_index,
                    ctx.state,
                    ctx.tx,
                    ctx.scan_handles,
                    ctx.dir_size_queue,
                    ctx.watcher_cmd_tx,
                )
                .await;
            }
            return;
        }

        let parent_source_id = panel.source_id.clone();
        let parent_path = panel.path.clone();
        let prev_shadow = panel.shadow_source.clone();

        let shadow = ShadowSource {
            parent_source_id,
            parent_path,
            archive_path: "Processes".to_string(),
            cursor_hint: None,
            cursor_index: None,
            parent_shadow: prev_shadow.map(Box::new),
        };

        let hierarchical = state_guard.settings.general.proc_tree_mode;
        let driver = Arc::new(crate::vfs::procs::ProcsDriver { hierarchical });
        // Hierarchical mode → directory entries are intra-driver tree nodes;
        // advertise InternalNavigation so open_entry doesn't hand off to local FS.
        let mut supports = vec!["ListEntries".to_string()];
        if hierarchical {
            supports.push(crate::ws::intents::CAP_INTERNAL_NAV.to_string());
        }
        state_guard.sources.insert(
            source_id.clone(),
            Source {
                id: source_id.clone(),
                name: "Processes".to_string(),
                kind: "virtual".to_string(),
                supports,
                root_path: String::new(),
                mounted: true,
                device_path: None,
                shadow: true,
                icon_name: Some("activity".to_string()),
                sort_priority: 0,
            },
        );
        state_guard
            .drivers
            .insert(source_id.clone(), driver as Arc<dyn VfsDriver>);

        if let Some(panel) = state_guard.panels.get_mut(&panel_id) {
            panel.shadow_source = Some(shadow);
            panel.source_id = source_id.clone();
            panel.path = String::new();
            panel.cursor = 0;
            panel.selection.clear();
            panel.filter = None;
            panel.filter_mode_active = false;
        }
    }

    let sources_json = {
        let state_guard = ctx.state.lock().await;
        build_sources_json(&state_guard)
    };
    let _ = ctx
        .tx
        .send(ServerMessage::StatePatch {
            timestamp: now(),
            domain: "sources".to_string(),
            entity_id: None,
            version: 0,
            patch: serde_json::json!({ "sources": sources_json }),
        })
        .await;

    // Stop watching any real-FS path — procs is not a watchable filesystem.
    let _ = ctx
        .watcher_cmd_tx
        .send(WatcherCommand::Unwatch {
            panel_id: panel_id.clone(),
        })
        .await;

    rescan_panel(
        &panel_id,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        None,
        None,
        ctx.dir_size_queue,
    )
    .await;
}
