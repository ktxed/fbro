use std::sync::Arc;

use serde_json::Value;

use crate::jobs::viewer_prefetch::PREFETCH_IMAGE_CAP;
use crate::state::settings::GeneralSettings;
use crate::vfs::{search::SearchResultsDriver, VfsDriver};
use crate::ws::intents::{arg_str, arg_u64, is_archive_source, is_procs_source, is_search_source};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{url_encode, IntentContext};

/// Effective image cap (in bytes) for the viewer + prefetch.
/// When ludicrous mode is enabled, scales up to the user-configured `max_image_size_mb`.
/// Otherwise the conservative 10 MiB baseline (matches FE's IMAGE_LIMIT for prefetch).
fn effective_image_cap_bytes(general: &GeneralSettings) -> usize {
    if general.ludicrous_mode {
        (general.max_image_size_mb as usize).saturating_mul(1024 * 1024)
    } else {
        PREFETCH_IMAGE_CAP
    }
}

/// Files larger than this (non-image, non-cached) get a 4 KB binary probe during navigation.
const PREFETCH_OTHER_THRESHOLD: u64 = 1 * 1024 * 1024;

/// Mirrors the FE's `detectBinary()`: a null byte or >30% non-printable chars → binary.
fn detect_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(4096)];
    if sample.contains(&0u8) {
        return true;
    }
    let non_text = sample
        .iter()
        .filter(|&&b| b < 9 || (b > 13 && b < 32) || b == 127)
        .count();
    !sample.is_empty() && non_text * 10 > sample.len() * 3
}

async fn driver_file_size(driver: Option<Arc<dyn VfsDriver>>, path: String) -> Option<u64> {
    let driver = driver?;
    tokio::task::spawn_blocking(move || driver.file_size(&path))
        .await
        .ok()
        .flatten()
}

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "ViewFile" => view_file(args, ctx).await,
        "EditFile" => edit_file(args, ctx).await,
        "CloseViewerWindow" => close_viewer_window(args, ctx).await,
        "ViewerNavigate" => viewer_navigate(args, ctx).await,
        "ReadFileChunk" => read_file_chunk(args, ctx).await,
        "SetEmbeddedViewerActive" => set_embedded_viewer_active(args, ctx).await,
        _ => {}
    }
}

struct ViewFileArgs {
    panel_id: String,
    path: String,
    source_id: String,
    filename: String,
}

impl ViewFileArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            panel_id: arg_str(args, "panelId").unwrap_or("").to_string(),
            path: arg_str(args, "path").unwrap_or("").to_string(),
            source_id: arg_str(args, "sourceId").unwrap_or("").to_string(),
            filename: arg_str(args, "filename").unwrap_or("").to_string(),
        }
    }
}

struct EditFileArgs {
    path: String,
    filename: String,
}

impl EditFileArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            path: arg_str(args, "path").unwrap_or("").to_string(),
            filename: arg_str(args, "filename").unwrap_or("").to_string(),
        }
    }
}

struct LabelArgs {
    label: String,
}

impl LabelArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            label: arg_str(args, "label").unwrap_or("").to_string(),
        }
    }
}

struct ViewerNavigateArgs {
    panel_id: String,
    direction: String,
}

impl ViewerNavigateArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            panel_id: arg_str(args, "panelId").unwrap_or("").to_string(),
            direction: arg_str(args, "direction").unwrap_or("next").to_string(),
        }
    }
}

struct ReadChunkArgs {
    path: String,
    source_id: String,
    offset: u64,
    size: usize,
}

impl ReadChunkArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            path: arg_str(args, "path").unwrap_or("").to_string(),
            source_id: arg_str(args, "sourceId").unwrap_or("").to_string(),
            offset: arg_u64(args, "offset", 0),
            size: arg_u64(args, "size", 4096) as usize,
        }
    }
}

async fn view_file(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = ViewFileArgs::from_args(args);

    // When viewing from a search results source, resolve the real file path
    // and real source_id so the viewer uses the local filesystem driver.
    let (effective_source_id, full_path) = if is_search_source(&parsed.source_id) {
        let state_guard = ctx.state.lock().await;
        let resolved = state_guard.drivers.get(&parsed.source_id).and_then(|drv| {
            drv.as_any()
                .downcast_ref::<SearchResultsDriver>()
                .and_then(|sd| sd.resolve_real_info(&parsed.filename))
        });
        if let Some((real_full_path, real_source_id)) = resolved {
            (real_source_id, real_full_path)
        } else {
            let best_source = state_guard
                .sources
                .iter()
                .filter(|(_, s)| !s.shadow && s.mounted)
                .max_by_key(|(_, s)| {
                    let candidate =
                        format!("{}/{}", parsed.path.trim_end_matches('/'), parsed.filename);
                    if candidate.starts_with(&s.root_path) {
                        s.root_path.len()
                    } else {
                        0
                    }
                })
                .map(|(id, _)| id.clone())
                .unwrap_or_else(|| parsed.source_id.clone());
            (
                best_source,
                format!("{}/{}", parsed.path.trim_end_matches('/'), parsed.filename),
            )
        }
    } else if is_archive_source(&parsed.source_id) || is_procs_source(&parsed.source_id) {
        let inner_path = {
            let p = parsed.path.trim_end_matches('/');
            if p.is_empty() {
                parsed.filename.clone()
            } else {
                format!("{}/{}", p, parsed.filename)
            }
        };
        (parsed.source_id.clone(), inner_path)
    } else {
        (
            parsed.source_id.clone(),
            format!("{}/{}", parsed.path.trim_end_matches('/'), parsed.filename),
        )
    };

    let (is_local, driver, file_server_port, image_cap_bytes, viewer_bg) = {
        let state_guard = ctx.state.lock().await;
        let driver = state_guard.drivers.get(&effective_source_id).cloned();
        let viewer_bg = match state_guard.settings.general.color_scheme.as_str() {
            "daylight" => tauri::webview::Color(244, 246, 248, 255),
            _ => tauri::webview::Color(13, 13, 13, 255),
        };
        (
            driver
                .as_ref()
                .map(|driver| driver.is_local())
                .unwrap_or(false),
            driver,
            state_guard.file_server_port,
            effective_image_cap_bytes(&state_guard.settings.general),
            viewer_bg,
        )
    };
    let file_size = driver_file_size(driver.clone(), full_path.clone()).await;

    let label = format!("viewer-{}", uuid::Uuid::new_v4());
    let mut query_string = format!(
        "?viewer=1&path={}&source={}&name={}&label={}&panel={}",
        url_encode(&full_path),
        url_encode(&effective_source_id),
        url_encode(&parsed.filename),
        url_encode(&label),
        url_encode(&parsed.panel_id),
    );
    if is_local {
        query_string.push_str("&local=1");
    }
    if let Some(file_size) = file_size {
        query_string.push_str(&format!("&size={file_size}"));
    }
    if file_server_port > 0 {
        query_string.push_str(&format!("&fsport={file_server_port}"));
    }
    query_string.push_str(&format!("&imgcap={image_cap_bytes}"));

    // Use spawn_blocking so the macOS dispatch_sync to the Cocoa main thread
    // runs on the blocking thread pool rather than a Tokio async worker thread.
    // If an async worker is blocked, the Tokio runtime can't schedule the
    // viewer's WS handshake task, leaving it in CONNECTING state until the
    // 2-second connect timeout fires.
    let app_handle = ctx.app_handle.clone();
    let window_title = format!("{} — fbro viewer", parsed.filename);
    tokio::task::spawn_blocking(move || {
        let _ = tauri::WebviewWindowBuilder::new(
            &app_handle,
            label,
            tauri::WebviewUrl::App(query_string.into()),
        )
        .title(window_title)
        .inner_size(900.0, 700.0)
        .resizable(true)
        .devtools(true)
        .background_color(viewer_bg)
        .build();
    });

    // Mark a viewer as active so panel cursor moves start warming the prefetch
    // cache. Decremented in `close_viewer_window`.
    {
        let mut state_guard = ctx.state.lock().await;
        state_guard.active_viewers += 1;
        sync_thumb_concurrency(&state_guard);
    }

    // Prime the prefetch cache for files neighboring the one being opened.
    ctx.viewer_prefetch.clear();
    let (neighbors, driver, image_cap) = {
        let state_guard = ctx.state.lock().await;
        let panel = state_guard.panels.get(&parsed.panel_id);
        let mut neighbors = match (panel, driver.as_ref()) {
            (Some(panel), Some(drv)) if drv.is_local() => {
                collect_neighbors(&panel.entries, panel.cursor, &panel.path, &panel.source_id)
            }
            _ => vec![],
        };
        // Include the target file itself so it is warm in the cache when the
        // viewer window finishes loading and sends ReadFileChunk. Without this
        // the first open always misses the cache and reads from disk, which is
        // noticeable on macOS where WebKit takes longer to initialise.
        if driver.as_ref().map_or(false, |d| d.is_local()) {
            neighbors.push((full_path.clone(), file_size));
        }
        (
            neighbors,
            driver,
            effective_image_cap_bytes(&state_guard.settings.general),
        )
    };
    if let Some(driver) = driver {
        ctx.viewer_prefetch
            .schedule_neighbors(neighbors, effective_source_id, driver, image_cap);
    }
}

async fn edit_file(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = EditFileArgs::from_args(args);
    let full_path = format!("{}/{}", parsed.path.trim_end_matches('/'), parsed.filename);

    let result = {
        let state_guard = ctx.state.lock().await;
        state_guard.platform.open_in_editor(&full_path)
    };

    if let Err(error) = result {
        let _ = ctx
            .tx
            .send(ServerMessage::Error {
                timestamp: now(),
                code: "NO_EDITOR".to_string(),
                message: error,
                recoverable: true,
                extra: Some(serde_json::json!({ "path": full_path })),
            })
            .await;
    }
}

async fn close_viewer_window(args: &Value, ctx: &IntentContext<'_>) {
    let parsed = LabelArgs::from_args(args);
    use tauri::Manager;
    let existed = ctx.app_handle.get_webview_window(&parsed.label).is_some();
    if let Some(window) = ctx.app_handle.get_webview_window(&parsed.label) {
        let _ = window.close();
    }
    if existed {
        let mut state_guard = ctx.state.lock().await;
        state_guard.active_viewers = state_guard.active_viewers.saturating_sub(1);
        if state_guard.active_viewers == 0 {
            ctx.viewer_prefetch.clear();
        }
        sync_thumb_concurrency(&state_guard);
    }
}

/// Set thumbnail-pool concurrency based on whether any viewers are open.
/// Caller holds the AppState lock.
fn sync_thumb_concurrency(state: &crate::state::AppState) {
    // Active viewer → throttle thumbs to 2 workers so they don't fight the
    // viewer for CPU/disk while it's loading a full-resolution image.
    // Idle → restore to 8.
    let cap = if state.active_viewers > 0 { 2 } else { 8 };
    state.thumb_pool.set_max_concurrent(cap);
}

/// Notifies BE that an embedded viewer panel was opened (active=true) or
/// closed (active=false). Used to gate background prefetch so the cache is
/// only warmed while a viewer is actually consuming files.
async fn set_embedded_viewer_active(args: &Value, ctx: &IntentContext<'_>) {
    let active = arg_str(args, "active")
        .map(|s| s == "true")
        .or_else(|| args.get("active").and_then(|v| v.as_bool()))
        .unwrap_or(false);
    let mut state_guard = ctx.state.lock().await;
    if active {
        state_guard.active_viewers += 1;
    } else {
        state_guard.active_viewers = state_guard.active_viewers.saturating_sub(1);
        if state_guard.active_viewers == 0 {
            ctx.viewer_prefetch.clear();
        }
    }
    sync_thumb_concurrency(&state_guard);
}

async fn viewer_navigate(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = ViewerNavigateArgs::from_args(args);
    let nav_result = {
        let mut state_guard = ctx.state.lock().await;
        if let Some(panel) = state_guard.panels.get_mut(&parsed.panel_id) {
            let new_cursor = if parsed.direction == "prev" {
                if panel.cursor == 0 {
                    0
                } else {
                    panel.cursor - 1
                }
            } else {
                (panel.cursor + 1).min(panel.entries.len().saturating_sub(1))
            };
            panel.cursor = new_cursor;
            let source_id = panel.source_id.clone();
            let dir_path = panel.path.clone();
            let entry_info = panel
                .entries
                .get(new_cursor)
                .map(|entry| (entry.name.clone(), entry.kind.clone()));
            let prefetch_neighbors =
                collect_neighbors(&panel.entries, new_cursor, &dir_path, &source_id);
            Some((
                new_cursor,
                source_id,
                dir_path,
                entry_info,
                prefetch_neighbors,
            ))
        } else {
            None
        }
    };

    if let Some((new_cursor, source_id, dir_path, entry_info, prefetch_neighbors)) = nav_result {
        if let Some((name, kind)) = entry_info {
            if kind == "file" {
                // When the panel is on a search results source, resolve the
                // real full_path and source_id so the viewer reads the actual
                // file rather than hitting the stub search driver.
                let (effective_path, effective_source_id) = {
                    let state_guard = ctx.state.lock().await;
                    if is_search_source(&source_id) {
                        state_guard
                            .drivers
                            .get(&source_id)
                            .and_then(|drv| drv.as_any().downcast_ref::<SearchResultsDriver>())
                            .and_then(|sd| sd.resolve_real_info(&name))
                            .unwrap_or_else(|| {
                                (
                                    format!("{}/{}", dir_path.trim_end_matches('/'), name),
                                    source_id.clone(),
                                )
                            })
                    } else if is_archive_source(&source_id) || is_procs_source(&source_id) {
                        let inner_path = {
                            let p = dir_path.trim_end_matches('/');
                            if p.is_empty() {
                                name.clone()
                            } else {
                                format!("{}/{}", p, name)
                            }
                        };
                        (inner_path, source_id.clone())
                    } else {
                        (
                            format!("{}/{}", dir_path.trim_end_matches('/'), name),
                            source_id.clone(),
                        )
                    }
                };
                let driver = ctx
                    .state
                    .lock()
                    .await
                    .drivers
                    .get(&effective_source_id)
                    .cloned();
                let is_local = driver.as_ref().map(|drv| drv.is_local()).unwrap_or(false);
                let file_size = driver_file_size(driver.clone(), effective_path.clone()).await;

                // Determine binary hint so the FE can skip directly to hex mode.
                // Try prefetch cache first (no I/O), then probe 4 KB for large files.
                let is_binary: Option<bool> = {
                    if let Some(entry) = ctx.viewer_prefetch.get_cached_by_path(&effective_path) {
                        Some(detect_binary(&entry.content))
                    } else if file_size
                        .map(|sz| sz > PREFETCH_OTHER_THRESHOLD)
                        .unwrap_or(false)
                    {
                        if let Some(drv) = driver.clone() {
                            let path = effective_path.clone();
                            tokio::task::spawn_blocking(move || {
                                drv.read_chunk(&path, 0, 4096)
                                    .ok()
                                    .map(|b| detect_binary(&b))
                            })
                            .await
                            .ok()
                            .flatten()
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };

                let _ = ctx
                    .tx
                    .send(ServerMessage::ViewerFileChanged {
                        timestamp: now(),
                        path: effective_path,
                        filename: name,
                        source_id: effective_source_id.clone(),
                        panel_id: parsed.panel_id.clone(),
                        file_size,
                        is_local,
                        is_binary,
                    })
                    .await;

                // Schedule BE cache prefetch for neighboring files.
                if let Some(ref drv) = driver {
                    if drv.is_local() {
                        let image_cap = {
                            let state_guard = ctx.state.lock().await;
                            effective_image_cap_bytes(&state_guard.settings.general)
                        };
                        ctx.viewer_prefetch.schedule_neighbors(
                            prefetch_neighbors,
                            effective_source_id,
                            Arc::clone(drv),
                            image_cap,
                        );
                    }
                }
            } else {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        timestamp: now(),
                        code: "NOT_A_FILE".to_string(),
                        message: format!("{name} is a {kind}, not a file"),
                        recoverable: true,
                        extra: None,
                    })
                    .await;
            }
        }

        let cursor_patch = ServerMessage::StatePatch {
            timestamp: now(),
            domain: "panels".to_string(),
            entity_id: Some(parsed.panel_id),
            version: 0,
            patch: serde_json::json!({ "cursor": new_cursor }),
        };
        let state_guard = ctx.state.lock().await;
        for gui_tx in &state_guard.gui_senders {
            let _ = gui_tx.send(cursor_patch.clone()).await;
        }
    }
}

async fn read_file_chunk(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = ReadChunkArgs::from_args(args);

    let (effective_source_id, effective_path) = if is_search_source(&parsed.source_id) {
        let state_guard = ctx.state.lock().await;
        // Prefer matching the request path: callers (e.g. the embedded viewer)
        // send the real absolute path, which is unambiguous when several search
        // hits share a filename. Fall back to filename-only lookup for legacy
        // callers.
        let filename = parsed
            .path
            .rsplit_once('/')
            .map(|(_, name)| name)
            .unwrap_or(&parsed.path);
        let resolved = state_guard
            .drivers
            .get(&parsed.source_id)
            .and_then(|drv| drv.as_any().downcast_ref::<SearchResultsDriver>())
            .and_then(|sd| {
                sd.resolve_real_info_by_path(&parsed.path)
                    .or_else(|| sd.resolve_real_info(filename))
            });
        if let Some((real_path, real_source)) = resolved {
            (real_source, real_path)
        } else {
            let best_source = state_guard
                .sources
                .iter()
                .filter(|(_, s)| !s.shadow && s.mounted)
                .max_by_key(|(_, s)| {
                    if parsed.path.starts_with(&s.root_path) {
                        s.root_path.len()
                    } else {
                        0
                    }
                })
                .map(|(id, _)| id.clone())
                .unwrap_or_else(|| parsed.source_id.clone());
            (best_source, parsed.path.clone())
        }
    } else {
        (parsed.source_id.clone(), parsed.path.clone())
    };

    let driver = {
        let state_guard = ctx.state.lock().await;
        state_guard.drivers.get(&effective_source_id).cloned()
    };
    let file_size = driver_file_size(driver.clone(), effective_path.clone()).await;

    // Serve from prefetch cache when this is the initial (offset=0) request.
    if parsed.offset == 0 {
        if let Some(cached) = ctx
            .viewer_prefetch
            .get_cached(&effective_source_id, &effective_path)
        {
            let bytes = cached.content.clone();
            let cached_file_size = cached.file_size.or(file_size);
            let size = parsed.size;
            let eof =
                bytes.len() < size || cached_file_size.map_or(false, |fs| bytes.len() as u64 >= fs);
            let bytes = if !eof && !bytes.is_empty() && bytes[bytes.len() - 1] != b'\n' {
                let mut b = bytes;
                if let Some(pos) = b.iter().rposition(|&c| c == b'\n') {
                    b.truncate(pos + 1);
                }
                b
            } else {
                bytes
            };
            let data: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let _ = ctx
                .tx
                .send(ServerMessage::FileChunk {
                    timestamp: now(),
                    path: effective_path,
                    offset: 0,
                    data,
                    eof,
                    file_size: cached_file_size,
                })
                .await;
            return;
        }
    }

    if let Some(driver) = driver {
        let path_clone = effective_path.clone();
        let offset = parsed.offset;
        let size = parsed.size;
        let result =
            tokio::task::spawn_blocking(move || driver.read_chunk(&path_clone, offset, size)).await;
        match result {
            Ok(Ok(mut bytes)) => {
                let eof = bytes.len() < size
                    || file_size.map_or(false, |fs| offset + bytes.len() as u64 >= fs);

                if !eof && !bytes.is_empty() && bytes[bytes.len() - 1] != b'\n' {
                    if let Some(pos) = bytes.iter().rposition(|&b| b == b'\n') {
                        bytes.truncate(pos + 1);
                    }
                }

                let data: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                let _ = ctx
                    .tx
                    .send(ServerMessage::FileChunk {
                        timestamp: now(),
                        path: effective_path,
                        offset,
                        data,
                        eof,
                        file_size,
                    })
                    .await;
            }
            Ok(Err(error)) => {
                let _ = ctx
                    .tx
                    .send(ServerMessage::Error {
                        timestamp: now(),
                        code: "READ_CHUNK_FAILED".to_string(),
                        message: error.to_string(),
                        recoverable: true,
                        extra: None,
                    })
                    .await;
            }
            Err(_) => {}
        }
    }
}

fn collect_neighbors(
    entries: &[crate::ws::protocol::EntrySchema],
    cursor: usize,
    dir_path: &str,
    source_id: &str,
) -> Vec<(String, Option<u64>)> {
    use crate::jobs::viewer_prefetch::PREFETCH_N;
    if is_archive_source(source_id) || is_search_source(source_id) {
        return vec![];
    }
    let mut neighbors = Vec::new();
    let base = dir_path.trim_end_matches('/');
    let mut count = 0usize;
    let mut i = cursor;
    while count < PREFETCH_N && i > 0 {
        i -= 1;
        if entries[i].kind == "file" {
            neighbors.push((format!("{}/{}", base, entries[i].name), entries[i].size));
            count += 1;
        }
    }
    count = 0;
    i = cursor;
    while count < PREFETCH_N && i + 1 < entries.len() {
        i += 1;
        if entries[i].kind == "file" {
            neighbors.push((format!("{}/{}", base, entries[i].name), entries[i].size));
            count += 1;
        }
    }
    neighbors
}
