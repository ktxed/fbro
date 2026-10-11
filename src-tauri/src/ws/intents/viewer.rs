use std::io::Read;
use std::sync::Arc;

use comrak::nodes::NodeValue;
use comrak::{format_html, parse_document, Arena, Options};
use serde_json::Value;

use crate::jobs::JobKind;
use crate::jobs::viewer_prefetch::{is_image_ext, PREFETCH_IMAGE_CAP};
use crate::state::settings::GeneralSettings;
use crate::vfs::{search::SearchResultsDriver, VfsDriver};
use crate::ws::intents::{
    arg_str, arg_u64, is_archive_source, is_journal_source, is_procs_source, is_search_source,
};
use crate::ws::protocol::{now, MarkdownLinkSchema, ServerMessage, ViewerFileSchema, ViewerTargetSchema, ViewerFindMatchSchema};
use crate::ws::runtime::{url_encode, IntentContext};
use super::navigation::source_for_local_path;

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
const VIEWER_FIND_BATCH_SIZE: usize = 256;
const VIEWER_FIND_MAX_MATCHES: usize = 10_000;

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
        "ResolveViewerFile" => resolve_panel_viewer_file(args, ctx).await,
        "EditFile" => edit_file(args, ctx).await,
        "CloseViewerWindow" => close_viewer_window(args, ctx).await,
        "ViewerNavigate" => viewer_navigate(args, ctx).await,
        "ViewerOpenFile" => viewer_open_file(args, ctx).await,
        "ReadFileChunk" => read_file_chunk(args, ctx).await,
        "FindInViewerFile" => find_in_viewer_file(args, ctx).await,
        "SetEmbeddedViewerActive" => set_embedded_viewer_active(args, ctx).await,
        _ => {}
    }
}

fn find_literal_matches<R: Read>(
    mut reader: R,
    query: &[u8],
    max_matches: usize,
) -> std::io::Result<(Vec<ViewerFindMatchSchema>, bool)> {
    let needle: Vec<u8> = query.iter().map(u8::to_ascii_lowercase).collect();
    if needle.is_empty() {
        return Ok((Vec::new(), false));
    }

    // KMP keeps matching state across read boundaries without retaining the
    // file. Matches intentionally don't overlap, matching the frontend's old
    // `pos += query.length` behavior.
    let mut prefix = vec![0usize; needle.len()];
    for i in 1..needle.len() {
        let mut j = prefix[i - 1];
        while j > 0 && needle[i] != needle[j] { j = prefix[j - 1]; }
        if needle[i] == needle[j] { j += 1; }
        prefix[i] = j;
    }

    let mut buffer = [0u8; 64 * 1024];
    let mut matched = 0usize;
    let mut offset = 0u64;
    let mut line_number = 1u64;
    let mut recent_lines = std::collections::VecDeque::with_capacity(needle.len());
    let mut results = Vec::new();

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 { break; }
        for &raw in &buffer[..read] {
            recent_lines.push_back(line_number);
            if recent_lines.len() > needle.len() { recent_lines.pop_front(); }

            let byte = raw.to_ascii_lowercase();
            while matched > 0 && byte != needle[matched] { matched = prefix[matched - 1]; }
            if byte == needle[matched] { matched += 1; }
            if matched == needle.len() {
                if results.len() == max_matches {
                    return Ok((results, true));
                }
                results.push(ViewerFindMatchSchema {
                    byte_offset: offset + 1 - needle.len() as u64,
                    byte_length: needle.len() as u64,
                    line_number: *recent_lines.front().unwrap_or(&line_number),
                });
                matched = 0;
            }
            if raw == b'\n' { line_number += 1; }
            offset += 1;
        }
    }
    Ok((results, false))
}

#[derive(Clone)]
struct ViewFileArgs {
    panel_id: String,
    path: String,
    source_id: String,
    filename: String,
    file_path: Option<String>,
    initial_find_query: Option<String>,
}

impl ViewFileArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            panel_id: arg_str(args, "panelId").unwrap_or("").to_string(),
            path: arg_str(args, "path").unwrap_or("").to_string(),
            source_id: arg_str(args, "sourceId").unwrap_or("").to_string(),
            filename: arg_str(args, "filename").unwrap_or("").to_string(),
            file_path: arg_str(args, "filePath").map(String::from),
            initial_find_query: arg_str(args, "initialFindQuery").map(String::from),
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

struct ViewerOpenFileArgs {
    panel_id: String,
    source_id: String,
    path: String,
}

impl ViewerOpenFileArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            panel_id: arg_str(args, "panelId").unwrap_or("").to_string(),
            source_id: arg_str(args, "sourceId").unwrap_or("").to_string(),
            path: arg_str(args, "path").unwrap_or("").to_string(),
        }
    }
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

// Search rows are identified by their real path, since filenames can repeat.
fn search_viewer_target(
    driver: &SearchResultsDriver,
    source_path: Option<&str>,
) -> Option<(String, String)> {
    driver.resolve_real_info_by_path(source_path?)
}

async fn send_search_viewer_error(ctx: &IntentContext<'_>) {
    let _ = ctx.send_feedback(ServerMessage::Error {
            timestamp: now(),
            code: "NOT_A_FILE".into(),
            message: "Search result is no longer available".into(),
            recoverable: true,
            extra: None,
        })
        .await;
}

async fn resolve_viewer_file(
    parsed: &ViewFileArgs,
    ctx: &IntentContext<'_>,
) -> Result<ViewerFileSchema, String> {
    // When viewing from a search results source, resolve the real file path
    // and real source_id so the viewer uses the local filesystem driver.
    let (mut effective_source_id, mut full_path, initial_find_query) =
        if let Some(path) = &parsed.file_path {
            (
                parsed.source_id.clone(),
                path.clone(),
                parsed.initial_find_query.clone(),
            )
        } else if is_search_source(&parsed.source_id) {
            let (driver, source_path, query) = {
                let state = ctx.state.lock().await;
                let query = parsed
                    .source_id
                    .strip_prefix("search:")
                    .and_then(|job_id| state.job_store.jobs.get(job_id))
                    .and_then(|job| match &job.kind {
                        JobKind::Search(meta) => meta.params.content_query.clone(),
                        _ => None,
                    })
                    .filter(|query| !query.is_empty());
                let source_path = state
                    .panels
                    .get(&parsed.panel_id)
                    .filter(|panel| panel.source_id == parsed.source_id)
                    .and_then(|panel| panel.entries.get(panel.cursor))
                    .filter(|entry| entry.name == parsed.filename)
                    .and_then(|entry| entry.source_path.clone());
                (
                    state.drivers.get(&parsed.source_id).cloned(),
                    source_path,
                    query,
                )
            };
            let resolved = tokio::task::spawn_blocking(move || {
                driver.and_then(|driver| {
                    driver
                        .as_any()
                        .downcast_ref::<SearchResultsDriver>()
                        .and_then(|search| search_viewer_target(search, source_path.as_deref()))
                })
            })
            .await
            .map_err(|error| format!("Could not resolve search viewer target: {error}"))?;
            let (path, source_id) =
                resolved.ok_or_else(|| "Search result is no longer available".to_string())?;
            (source_id, path, query)
        } else {
            let driver = {
                let state = ctx.state.lock().await;
                state.drivers.get(&parsed.source_id).cloned()
            }
            .ok_or_else(|| "Viewer source is no longer available".to_string())?;
            let base = if is_journal_source(&parsed.source_id) {
                String::new()
            } else {
                parsed.path.clone()
            };
            let filename = parsed.filename.clone();
            let path = tokio::task::spawn_blocking(move || driver.resolve_child(&base, &filename))
                .await
                .map_err(|error| format!("Could not resolve viewer path: {error}"))?;
            (parsed.source_id.clone(), path, None)
        };

    let local_driver = {
        let state = ctx.state.lock().await;
        state
            .drivers
            .get(&effective_source_id)
            .is_some_and(|driver| driver.is_local())
    };
    let mut viewer_filename = parsed.filename.clone();
    if local_driver {
        let candidate = full_path.clone();
        let resolved =
            tokio::task::spawn_blocking(move || resolve_local_viewer_path(&candidate)).await;
        match resolved {
            Ok(Ok(Some(target))) => {
                viewer_filename = std::path::Path::new(&target)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| parsed.filename.clone());
                full_path = target;
                let state = ctx.state.lock().await;
                if let Some(source_id) = source_for_local_path(&state, &full_path) {
                    effective_source_id = source_id;
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(message)) => return Err(message),
            Err(error) => return Err(format!("Could not resolve viewer file: {error}")),
        }
    }

    if full_path.contains('\0') {
        return Err("Invalid viewer path".into());
    }
    let driver = {
        let state = ctx.state.lock().await;
        state.drivers.get(&effective_source_id).cloned()
    }
    .ok_or_else(|| "Viewer source is no longer available".to_string())?;
    let file_size = driver_file_size(Some(Arc::clone(&driver)), full_path.clone()).await;
    Ok(ViewerFileSchema {
        panel_id: parsed.panel_id.clone(),
        path: full_path,
        source_id: effective_source_id,
        filename: viewer_filename,
        file_size,
        is_local: driver.is_local(),
        initial_find_query,
    })
}

fn resolve_local_viewer_path(path: &str) -> Result<Option<String>, String> {
    let link = std::fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect viewer file: {error}"))?;
    if !link.is_symlink() {
        if !link.is_file() {
            return Err("Viewer target is not a file".into());
        }
        return Ok(None);
    }
    let target = std::fs::canonicalize(path)
        .map_err(|error| format!("Could not resolve symbolic link: {error}"))?;
    if !target.is_file() {
        return Err("Symbolic link target is not a file".into());
    }
    Ok(Some(crate::vfs::local::normalize_path(target)))
}

async fn resolve_panel_viewer_file(args: &Value, ctx: &IntentContext<'_>) {
    let panel_id = arg_str(args, "panelId").unwrap_or("");
    let request_id = arg_str(args, "requestId").unwrap_or("").to_string();
    // Clone the authoritative cursor entry before any blocking driver call.
    let entry = {
        let state = ctx.state.lock().await;
        state.panels.get(panel_id).map(|panel| {
            (
                panel.source_id.clone(),
                panel.path.clone(),
                panel.entries.get(panel.cursor).cloned(),
            )
        })
    };
    let target = match entry {
        Some((source_id, path, Some(entry))) => {
            if entry.kind == "directory"
                || (entry.kind == "symlink" && entry.target_kind.as_deref() == Some("directory"))
            {
                ViewerTargetSchema::Directory { name: entry.name }
            } else {
                let parsed = ViewFileArgs {
                    panel_id: panel_id.into(),
                    source_id,
                    path,
                    filename: entry.name,
                    file_path: None,
                    initial_find_query: None,
                };
                match resolve_viewer_file(&parsed, ctx).await {
                    Ok(file) => ViewerTargetSchema::File { file },
                    Err(message) => ViewerTargetSchema::Error { message },
                }
            }
        }
        _ => ViewerTargetSchema::Empty,
    };
    let _ = ctx
        .tx
        .send(ServerMessage::ViewerFileReady {
            timestamp: now(),
            request_id,
            target,
        })
        .await;
}

async fn view_file(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = ViewFileArgs::from_args(args);
    let file = match resolve_viewer_file(&parsed, ctx).await {
        Ok(file) => file,
        Err(message) => {
            let _ = ctx.send_feedback(ServerMessage::Error {
                    timestamp: now(),
                    code: "NOT_A_FILE".into(),
                    message,
                    recoverable: true,
                    extra: None,
                })
                .await;
            return;
        }
    };
    let ViewerFileSchema {
        path: full_path,
        source_id: effective_source_id,
        filename: viewer_filename,
        file_size,
        initial_find_query,
        ..
    } = file;

    let (is_local, driver, file_server_port, image_cap_bytes, color_scheme, viewer_bg) = {
        let state_guard = ctx.state.lock().await;
        let driver = state_guard.drivers.get(&effective_source_id).cloned();
        let color_scheme = state_guard.settings.general.color_scheme.clone();
        let viewer_bg = match color_scheme.as_str() {
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
            color_scheme,
            viewer_bg,
        )
    };
    let label = format!("viewer-{}", uuid::Uuid::new_v4());
    let mut query_string = format!(
        "?viewer=1&path={}&source={}&name={}&label={}&panel={}",
        url_encode(&full_path),
        url_encode(&effective_source_id),
        url_encode(&viewer_filename),
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
    query_string.push_str(&format!("&colorScheme={}", url_encode(&color_scheme)));
    if let Some(find_query) = initial_find_query {
        query_string.push_str(&format!("&find={}", url_encode(&find_query)));
    }

    // Use spawn_blocking so the macOS dispatch_sync to the Cocoa main thread
    // runs on the blocking thread pool rather than a Tokio async worker thread.
    // If an async worker is blocked, the Tokio runtime can't schedule the
    // viewer's WS handshake task, leaving it in CONNECTING state until the
    // 2-second connect timeout fires.
    let app_handle = ctx.app_handle.clone();
    let window_title = format!("{} — fbro viewer", viewer_filename);
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
    let (neighbors, driver, image_cap) = {
        let state_guard = ctx.state.lock().await;
        let panel = state_guard.panels.get(&parsed.panel_id);
        let mut neighbors = match (panel, driver.as_ref()) {
            (Some(panel), Some(drv)) if drv.is_local() => collect_neighbors(
                &panel.entries,
                panel.cursor,
                &panel.path,
                &panel.source_id,
                "next",
            ),
            _ => vec![],
        };
        // Include the target file itself so it is warm in the cache when the
        // viewer window finishes loading and sends ReadFileChunk. Without this
        // the first open always misses the cache and reads from disk, which is
        // noticeable on macOS where WebKit takes longer to initialise.
        if driver.as_ref().map_or(false, |d| d.is_local()) {
            neighbors.retain(|(path, _)| path != &full_path);
            neighbors.insert(0, (full_path.clone(), file_size));
        }
        (
            neighbors,
            driver,
            effective_image_cap_bytes(&state_guard.settings.general),
        )
    };
    if let Some(driver) = driver {
        ctx.viewer_prefetch.schedule_neighbors(
            parsed.panel_id,
            neighbors,
            effective_source_id,
            driver,
            image_cap,
        );
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
    // macOS does not reliably make another application window key when the
    // active viewer closes. Explicitly reactivate the main window so its
    // focus handler can return DOM focus to the active panel immediately.
    if let Some(main_window) = ctx.app_handle.get_webview_window("main") {
        let _ = main_window.unminimize();
        let _ = main_window.show();
        let _ = main_window.set_focus();
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
    let started = std::time::Instant::now();
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
                .map(|entry| {
                    (
                        entry.name.clone(),
                        entry.kind.clone(),
                        entry.target_kind.clone(),
                        entry.source_path.clone(),
                    )
                });
            let prefetch_neighbors = collect_neighbors(
                &panel.entries,
                new_cursor,
                &dir_path,
                &source_id,
                &parsed.direction,
            );
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

    if nav_result
        .as_ref()
        .and_then(|result| result.3.as_ref())
        .is_none()
    {
        ctx.viewer_prefetch.cancel_panel(&parsed.panel_id);
        let _ = ctx.send_feedback(ServerMessage::Error {
                timestamp: now(),
                code: "NOT_A_FILE".into(),
                message: "No file available in the source panel".into(),
                recoverable: true,
                extra: None,
            })
            .await;
        return;
    }
    if let Some((new_cursor, source_id, dir_path, entry_info, prefetch_neighbors)) = nav_result {
        if let Some((name, kind, target_kind, source_path)) = entry_info {
            if kind == "file" || (kind == "symlink" && target_kind.as_deref() == Some("file")) {
                // When the panel is on a search results source, resolve the
                // real full_path and source_id so the viewer reads the actual
                // file rather than hitting the stub search driver.
                let resolved_target = {
                    let state_guard = ctx.state.lock().await;
                    if is_search_source(&source_id) {
                        state_guard
                            .drivers
                            .get(&source_id)
                            .and_then(|drv| drv.as_any().downcast_ref::<SearchResultsDriver>())
                            .and_then(|sd| search_viewer_target(sd, source_path.as_deref()))
                    } else if is_journal_source(&source_id) {
                        let inner_path = state_guard
                            .drivers
                            .get(&source_id)
                            .map(|driver| driver.resolve_child("", &name))
                            .unwrap_or(name.clone());
                        Some((inner_path, source_id.clone()))
                    } else if is_archive_source(&source_id) || is_procs_source(&source_id) {
                        let inner_path = {
                            let p = dir_path.trim_end_matches('/');
                            if p.is_empty() {
                                name.clone()
                            } else {
                                format!("{}/{}", p, name)
                            }
                        };
                        Some((inner_path, source_id.clone()))
                    } else {
                        Some((
                            format!("{}/{}", dir_path.trim_end_matches('/'), name),
                            source_id.clone(),
                        ))
                    }
                };
                let Some((mut effective_path, mut effective_source_id)) = resolved_target else {
                    ctx.viewer_prefetch.cancel_panel(&parsed.panel_id);
                    send_search_viewer_error(ctx).await;
                    return;
                };
                let mut display_name = name.clone();
                if kind == "symlink" {
                    let link_path = effective_path.clone();
                    let resolved = tokio::task::spawn_blocking(move || {
                        let target = std::fs::canonicalize(&link_path)
                            .map_err(|error| format!("Cannot follow symbolic link: {error}"))?;
                        if !target.is_file() {
                            return Err("Symbolic link target is not a file.".to_string());
                        }
                        Ok(target)
                    }).await;
                    match resolved {
                        Ok(Ok(target)) => {
                            effective_path = crate::vfs::local::normalize_path(target.clone());
                            display_name = target.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or(name);
                            let state = ctx.state.lock().await;
                            if let Some(source_id) = source_for_local_path(&state, &effective_path) {
                                effective_source_id = source_id;
                            }
                        }
                        error => {
                            let message = match error {
                                Ok(Err(message)) => message,
                                Err(error) => format!("Cannot follow symbolic link: {error}"),
                                Ok(Ok(_)) => unreachable!(),
                            };
                            let _ = ctx.send_feedback(ServerMessage::Error {
                                timestamp: now(), code: "NOT_A_FILE".into(), message,
                                recoverable: true, extra: None,
                            }).await;
                            let cursor_patch = ServerMessage::StatePatch {
                                timestamp: now(), domain: "panels".into(),
                                entity_id: Some(parsed.panel_id.clone()), version: 0,
                                patch: serde_json::json!({ "cursor": new_cursor }),
                            };
                            let state = ctx.state.lock().await;
                            for gui_tx in &state.gui_senders {
                                let _ = gui_tx.send(cursor_patch.clone()).await;
                            }
                            return;
                        }
                    }
                }
                let driver = ctx
                    .state
                    .lock()
                    .await
                    .drivers
                    .get(&effective_source_id)
                    .cloned();
                let is_local = driver.as_ref().map(|drv| drv.is_local()).unwrap_or(false);
                if !is_local {
                    ctx.viewer_prefetch.cancel_panel(&parsed.panel_id);
                }
                let file_size = driver_file_size(driver.clone(), effective_path.clone()).await;

                // Determine binary hint so the FE can skip directly to hex mode.
                // Try prefetch cache first (no I/O), then probe 4 KB for large files.
                let image_cap = {
                    let state = ctx.state.lock().await;
                    effective_image_cap_bytes(&state.settings.general)
                };
                let direct_image = is_local
                    && is_image_ext(&effective_path)
                    && file_size.is_some_and(|size| size < image_cap as u64);
                let is_binary: Option<bool> = {
                    if direct_image {
                        None
                    } else if let Some(entry) = ctx
                        .viewer_prefetch
                        .get_cached(&effective_source_id, &effective_path)
                    {
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

                eprintln!("[viewer navigation] panel={} direction={} path={effective_path:?} elapsed_ms={} direct_image={direct_image}",
                    parsed.panel_id, parsed.direction, started.elapsed().as_millis());
                let _ = ctx
                    .tx
                    .send(ServerMessage::ViewerFileChanged {
                        timestamp: now(),
                        path: effective_path,
                        filename: display_name,
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
                        ctx.viewer_prefetch.schedule_neighbors(
                            parsed.panel_id.clone(),
                            prefetch_neighbors,
                            effective_source_id,
                            Arc::clone(drv),
                            image_cap,
                        );
                    }
                }
            } else {
                ctx.viewer_prefetch.cancel_panel(&parsed.panel_id);
                let _ = ctx.send_feedback(ServerMessage::Error {
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

fn is_markdown_filename(filename: &str) -> bool {
    matches!(
        filename
            .rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("md" | "markdown" | "mdown")
    )
}

/// Resolve a deliberately small URL subset: a percent-decoded relative path
/// with no scheme, authority, query, fragment, backslash, or control bytes.
fn safe_relative_path(url: &str) -> Option<String> {
    let mut bytes = Vec::with_capacity(url.len());
    let raw = url.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            let hex = raw.get(index + 1..index + 3)?;
            let text = std::str::from_utf8(hex).ok()?;
            bytes.push(u8::from_str_radix(text, 16).ok()?);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(bytes).ok()?;
    if decoded.is_empty()
        || decoded.starts_with('/')
        || decoded.starts_with('\\')
        || decoded.contains(['\\', ':', '?', '#', '\0'])
        || decoded.chars().any(char::is_control)
    {
        return None;
    }
    Some(decoded)
}

fn resolve_viewer_path(driver: &dyn VfsDriver, current_path: &str, target: &str) -> Option<String> {
    let mut path = driver.parent_path(current_path)?;
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." => path = driver.parent_path(&path)?,
            name => path = driver.resolve_child(&path, name),
        }
    }
    Some(path)
}

async fn viewer_open_file(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = ViewerOpenFileArgs::from_args(args);
    if !is_markdown_filename(&parsed.path) || parsed.path.contains('\0') {
        return;
    }

    let driver = {
        let state_guard = ctx.state.lock().await;
        state_guard.drivers.get(&parsed.source_id).cloned()
    };
    let Some(driver) = driver else { return };
    if !driver.is_local() { return; }

    // Recheck the canonical path received from the validated preview. This is
    // both a TOCTOU check and a trust-boundary check for forged intents.
    let validation_driver = Arc::clone(&driver);
    let validation_path = parsed.path.clone();
    let valid_file =
        tokio::task::spawn_blocking(move || validation_driver.open_read(&validation_path).is_ok())
            .await
            .unwrap_or(false);
    if !valid_file {
        return;
    }

    let filename = parsed
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&parsed.path)
        .to_string();
    let file_size = driver_file_size(Some(Arc::clone(&driver)), parsed.path.clone()).await;
    let _ = ctx
        .tx
        .send(ServerMessage::ViewerFileChanged {
            timestamp: now(),
            path: parsed.path,
            filename,
            source_id: parsed.source_id,
            panel_id: parsed.panel_id,
            file_size,
            is_local: driver.is_local(),
            is_binary: Some(false),
        })
        .await;
}

fn raster_signature_matches(path: &str, bytes: &[u8]) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "webp" => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        "bmp" => bytes.starts_with(b"BM"),
        "ico" => bytes.starts_with(&[0, 0, 1, 0]),
        "avif" => {
            bytes.get(4..8) == Some(b"ftyp")
                && bytes.get(8..32).is_some_and(|brands| {
                    brands.windows(4).any(|brand| brand == b"avif" || brand == b"avis")
                })
        }
        _ => false,
    }
}

fn valid_local_image(driver: &dyn VfsDriver, path: &str, cap: usize) -> bool {
    let Some(size) = driver.file_size(path) else { return false };
    if size > cap as u64 {
        return false;
    }
    let Ok(reader) = driver.open_read(path) else { return false };
    let mut bytes = Vec::new();
    if reader.take(64).read_to_end(&mut bytes).is_err() {
        return false;
    }
    raster_signature_matches(path, &bytes)
}

/// The single policy boundary for every URL-like value coming out of a
/// Markdown AST. Nothing in the renderer or frontend resolves destinations.
struct MarkdownUrlPolicy<'a> {
    driver: &'a dyn VfsDriver,
    current_path: &'a str,
    file_server_port: u16,
    image_cap: usize,
}

impl MarkdownUrlPolicy<'_> {
    fn link_target(&self, url: &str) -> Option<String> {
        safe_relative_path(url)
            .filter(|target| is_markdown_filename(target))
            .and_then(|target| resolve_viewer_path(self.driver, self.current_path, &target))
            .filter(|path| self.driver.open_read(path).is_ok())
    }

    fn image_source(&self, url: &str) -> Option<String> {
        let path = safe_relative_path(url)
            .and_then(|target| resolve_viewer_path(self.driver, self.current_path, &target))
            .filter(|path| valid_local_image(self.driver, path, self.image_cap))?;
        Some(markdown_image_url(self.file_server_port, &path))
    }
}

fn markdown_image_url(port: u16, path: &str) -> String {
    // Windows drive paths need the same leading URL slash as POSIX paths.
    let path = url_encode(path);
    let separator = if path.starts_with('/') { "" } else { "/" };
    format!("http://127.0.0.1:{port}{separator}{path}")
}

fn escape_markdown_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        .replace('"', "&quot;").replace('\'', "&#39;")
}

/// Parse HTML with the browser-grade parser, then rebuild only image markup.
/// No original attribute or markup is passed through to the webview.
fn render_markdown_image_html(source: &str, policy: &MarkdownUrlPolicy<'_>) -> String {
    let fragment = scraper::Html::parse_fragment(source);
    let mut output = String::new();
    let mut stack = vec![(fragment.tree.root(), false)];
    while let Some((node, closing)) = stack.pop() {
        match node.value() {
            scraper::Node::Element(element) => {
                let name = element.name();
                if closing {
                    output.push_str(&format!("</{name}>"));
                    continue;
                }
                match name {
                    "img" => {
                        let alt = escape_markdown_html(element.attr("alt").unwrap_or(""));
                        let Some(source) = element.attr("src").and_then(|src| policy.image_source(src)) else {
                            output.push_str(&alt);
                            continue;
                        };
                        output.push_str(&format!("<img src=\"{}\" alt=\"{alt}\"", escape_markdown_html(&source)));
                        for attribute in ["width", "height"] {
                            if let Some(value) = element.attr(attribute).and_then(|value| value.parse::<u32>().ok()).filter(|value| *value > 0) {
                                output.push_str(&format!(" {attribute}=\"{value}\""));
                            }
                        }
                        if let Some(title) = element.attr("title") {
                            output.push_str(&format!(" title=\"{}\"", escape_markdown_html(title)));
                        }
                        output.push('>');
                        continue;
                    }
                    "p" | "div" => {
                        output.push_str(&format!("<{name}"));
                        if let Some(align) = element.attr("align").map(str::to_ascii_lowercase)
                            .filter(|align| matches!(align.as_str(), "left" | "center" | "right")) {
                            output.push_str(&format!(" class=\"viewer__markdown-align-{align}\""));
                        }
                        output.push('>');
                        stack.push((node, true));
                    }
                    // The fragment parser inserts its own root element.
                    "html" if node.parent().is_some_and(|parent| parent.value().is_fragment()) => {}
                    _ => continue,
                }
            }
            scraper::Node::Text(text) => { output.push_str(&escape_markdown_html(text)); continue; }
            scraper::Node::Fragment => {}
            _ => continue,
        }
        let children: Vec<_> = node.children().collect();
        for child in children.into_iter().rev() { stack.push((child, false)); }
    }
    output
}

fn sanitize_mermaid(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let folded = line.trim().to_ascii_lowercase();
            let unsafe_line = folded.starts_with("click ")
                || folded.starts_with("%%{")
                || folded == "---"
                || folded.starts_with("config:")
                || folded.contains(" href")
                || folded.contains("url(")
                || folded.contains("://")
                || folded.contains("javascript:")
                || folded.contains("vbscript:")
                || folded.contains("data:")
                || folded.contains("file:")
                || folded.contains("image:")
                || folded.contains("img:");
            if unsafe_line {
                "%% unsafe Mermaid URL or interaction removed %%".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct RenderedMarkdown {
    html: String,
    links: Vec<MarkdownLinkSchema>,
}

fn render_markdown(
    driver: &dyn VfsDriver,
    current_path: &str,
    markdown: &str,
    file_server_port: u16,
    image_cap: usize,
) -> RenderedMarkdown {
    let arena = Arena::new();
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;
    options.extension.tagfilter = true;
    options.render.r#unsafe = false;
    let root = parse_document(&arena, markdown, &options);
    let nodes: Vec<_> = root.descendants().collect();
    let mut links = Vec::new();
    let url_policy = MarkdownUrlPolicy {
        driver,
        current_path,
        file_server_port,
        image_cap,
    };

    for node in nodes {
        enum Work { Link(String), Image(String), Html(String), Mermaid, None }
        let work = match &node.data.borrow().value {
            NodeValue::Link(link) => Work::Link(link.url.clone()),
            NodeValue::Image(link) => Work::Image(link.url.clone()),
            NodeValue::HtmlBlock(block) => Work::Html(block.literal.clone()),
            NodeValue::HtmlInline(html) => Work::Html(html.clone()),
            NodeValue::CodeBlock(block)
                if block.info.split_whitespace().next().is_some_and(|lang| lang.eq_ignore_ascii_case("mermaid")) => Work::Mermaid,
            _ => Work::None,
        };

        match work {
            Work::Html(source) => {
                // Only our rebuilt allowlisted HTML bypasses Comrak's raw-HTML filter.
                node.data.borrow_mut().value = NodeValue::Raw(render_markdown_image_html(&source, &url_policy));
            }
            Work::Link(url) => {
                let target = url_policy.link_target(&url);
                if let Some(path) = target {
                    let id = format!("fbro-md-{}", links.len());
                    if let NodeValue::Link(link) = &mut node.data.borrow_mut().value {
                        link.url = format!("#{id}");
                    }
                    links.push(MarkdownLinkSchema { id, path });
                } else {
                    let children: Vec<_> = node.children().collect();
                    for child in children { node.insert_before(child); }
                    node.detach();
                }
            }
            Work::Image(url) => {
                if let Some(source) = url_policy.image_source(&url) {
                    if let NodeValue::Image(link) = &mut node.data.borrow_mut().value {
                        link.url = source;
                    }
                } else {
                    let children: Vec<_> = node.children().collect();
                    for child in children { node.insert_before(child); }
                    node.detach();
                }
            }
            Work::Mermaid => {
                if let NodeValue::CodeBlock(block) = &mut node.data.borrow_mut().value {
                    block.literal = sanitize_mermaid(&block.literal);
                }
            }
            Work::None => {}
        }
    }

    let mut html = String::new();
    format_html(root, &options, &mut html).expect("rendering Markdown to a String cannot fail");
    RenderedMarkdown { html, links }
}

async fn send_markdown_preview(
    ctx: &IntentContext<'_>,
    driver: Arc<dyn VfsDriver>,
    path: String,
    bytes: Vec<u8>,
    file_server_port: u16,
    image_cap: usize,
) {
    if !driver.is_local() || !is_markdown_filename(&path) { return; }
    let Ok(markdown) = String::from_utf8(bytes) else { return };
    let render_path = path.clone();
    let rendered = tokio::task::spawn_blocking(move || {
        render_markdown(driver.as_ref(), &render_path, &markdown, file_server_port, image_cap)
    }).await;
    if let Ok(rendered) = rendered {
        let _ = ctx.tx.send(ServerMessage::MarkdownPreview {
            timestamp: now(), path, html: rendered.html, links: rendered.links,
        }).await;
    }
}

async fn find_in_viewer_file(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = arg_str(args, "requestId").unwrap_or("").to_string();
    let source_id = arg_str(args, "sourceId").unwrap_or("").to_string();
    let path = arg_str(args, "path").unwrap_or("").to_string();
    let query = arg_str(args, "query").unwrap_or("").as_bytes().to_vec();
    if request_id.is_empty() || path.is_empty() || query.is_empty() { return; }

    // Search-result sources are references to files owned by another driver.
    // Resolve them exactly as ReadFileChunk does so find works from those views.
    let (effective_source_id, effective_path) = if is_search_source(&source_id) {
        let state = ctx.state.lock().await;
        let filename = path.rsplit_once('/').map(|(_, name)| name).unwrap_or(&path);
        let resolved = state.drivers.get(&source_id)
            .and_then(|drv| drv.as_any().downcast_ref::<SearchResultsDriver>())
            .and_then(|sd| sd.resolve_real_info_by_path(&path).or_else(|| sd.resolve_real_info(filename)));
        if let Some((real_path, real_source)) = resolved {
            (real_source, real_path)
        } else {
            let best_source = state.sources.iter()
                .filter(|(_, source)| !source.shadow && source.mounted)
                .max_by_key(|(_, source)| if path.starts_with(&source.root_path) { source.root_path.len() } else { 0 })
                .map(|(id, _)| id.clone())
                .unwrap_or(source_id);
            (best_source, path.clone())
        }
    } else {
        (source_id, path.clone())
    };

    let driver = {
        let state = ctx.state.lock().await;
        state.drivers.get(&effective_source_id).cloned()
    };
    let tx = ctx.tx.clone();
    tokio::spawn(async move {
        let result = match driver {
            Some(driver) => tokio::task::spawn_blocking(move || {
                let reader = driver.open_read(&effective_path)
                    .map_err(|error| error.to_string())?;
                find_literal_matches(reader, &query, VIEWER_FIND_MAX_MATCHES)
                    .map_err(|error| error.to_string())
            }).await.map_err(|error| error.to_string()).and_then(|result| result),
            None => Err("Viewer source is no longer available".to_string()),
        };

        match result {
            Ok((matches, truncated)) => {
                if matches.is_empty() {
                    let _ = tx.send(ServerMessage::ViewerFindResults {
                        timestamp: now(), request_id, path, matches, done: true,
                        truncated, error: None,
                    }).await;
                    return;
                }
                let batch_count = matches.len().div_ceil(VIEWER_FIND_BATCH_SIZE);
                for (index, batch) in matches.chunks(VIEWER_FIND_BATCH_SIZE).enumerate() {
                    let _ = tx.send(ServerMessage::ViewerFindResults {
                        timestamp: now(), request_id: request_id.clone(), path: path.clone(),
                        matches: batch.to_vec(), done: index + 1 == batch_count,
                        truncated, error: None,
                    }).await;
                }
            }
            Err(error) => {
                let _ = tx.send(ServerMessage::ViewerFindResults {
                    timestamp: now(), request_id, path, matches: Vec::new(), done: true,
                    truncated: false, error: Some(error),
                }).await;
            }
        }
    });
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

    let (driver, file_server_port, image_cap) = {
        let state_guard = ctx.state.lock().await;
        (
            state_guard.drivers.get(&effective_source_id).cloned(),
            state_guard.file_server_port,
            effective_image_cap_bytes(&state_guard.settings.general),
        )
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
            let preview_bytes = (eof && driver.as_ref().is_some_and(|d| d.is_local()))
                .then(|| bytes.clone());
            let data: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
            let _ = ctx
                .tx
                .send(ServerMessage::FileChunk {
                    timestamp: now(),
                    path: effective_path.clone(),
                    offset: 0,
                    data,
                    eof,
                    file_size: cached_file_size,
                })
                .await;
            if let (Some(driver), Some(preview_bytes)) = (driver.clone(), preview_bytes) {
                send_markdown_preview(
                    ctx, driver, effective_path, preview_bytes, file_server_port, image_cap,
                ).await;
            }
            return;
        }
    }

    if let Some(driver) = driver {
        let read_driver = Arc::clone(&driver);
        let path_clone = effective_path.clone();
        let offset = parsed.offset;
        let size = parsed.size;
        let result =
            tokio::task::spawn_blocking(move || read_driver.read_chunk(&path_clone, offset, size)).await;
        match result {
            Ok(Ok(mut bytes)) => {
                let eof = bytes.len() < size
                    || file_size.map_or(false, |fs| offset + bytes.len() as u64 >= fs);

                if !eof && !bytes.is_empty() && bytes[bytes.len() - 1] != b'\n' {
                    if let Some(pos) = bytes.iter().rposition(|&b| b == b'\n') {
                        bytes.truncate(pos + 1);
                    }
                }

                let preview_bytes = (offset == 0 && eof && driver.is_local()).then(|| bytes.clone());
                let data: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                let _ = ctx
                    .tx
                    .send(ServerMessage::FileChunk {
                        timestamp: now(),
                        path: effective_path.clone(),
                        offset,
                        data,
                        eof,
                        file_size,
                    })
                    .await;
                if let Some(preview_bytes) = preview_bytes {
                    send_markdown_preview(
                        ctx, driver, effective_path, preview_bytes, file_server_port, image_cap,
                    ).await;
                }
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

pub(super) fn collect_neighbors(
    entries: &[crate::ws::protocol::EntrySchema],
    cursor: usize,
    dir_path: &str,
    source_id: &str,
    direction: &str,
) -> Vec<(String, Option<u64>)> {
    use crate::jobs::viewer_prefetch::PREFETCH_N;
    if is_archive_source(source_id) || is_search_source(source_id) {
        return vec![];
    }
    let mut previous = Vec::new();
    let mut following = Vec::new();
    let base = dir_path.trim_end_matches('/');
    let mut count = 0usize;
    let mut i = cursor;
    while count < PREFETCH_N && i > 0 {
        i -= 1;
        if entries[i].kind == "file" || (entries[i].kind == "symlink" && entries[i].target_kind.as_deref() == Some("file")) {
            previous.push((format!("{}/{}", base, entries[i].name), entries[i].size));
            count += 1;
        }
    }
    count = 0;
    i = cursor;
    while count < PREFETCH_N && i + 1 < entries.len() {
        i += 1;
        if entries[i].kind == "file" || (entries[i].kind == "symlink" && entries[i].target_kind.as_deref() == Some("file")) {
            following.push((format!("{}/{}", base, entries[i].name), entries[i].size));
            count += 1;
        }
    }
    let mut neighbors = Vec::new();
    if let Some(entry) = entries.get(cursor).filter(|e| e.kind == "file" || (e.kind == "symlink" && e.target_kind.as_deref() == Some("file"))) {
        neighbors.push((format!("{base}/{}", entry.name), entry.size));
    }
    let (first, second) = if direction == "prev" {
        (previous, following)
    } else {
        (following, previous)
    };
    for distance in 0..PREFETCH_N {
        if let Some(entry) = first.get(distance) {
            neighbors.push(entry.clone());
        }
        if let Some(entry) = second.get(distance) {
            neighbors.push(entry.clone());
        }
    }
    neighbors
}

#[cfg(test)]
mod tests {
    use super::{
        find_literal_matches, is_markdown_filename, markdown_image_url, render_markdown, safe_relative_path,
        sanitize_mermaid,
    };
    use crate::vfs::local::LocalFsDriver;
    use std::fs;
    use std::io::Cursor;

    struct ViewerTestDir(std::path::PathBuf);
    impl ViewerTestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("fbro-viewer-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for ViewerTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn viewer_target_rejects_missing_files_and_directories() {
        let temp = ViewerTestDir::new();
        let file = temp.0.join("clip.mp4");
        fs::write(&file, b"video").unwrap();
        assert_eq!(
            super::resolve_local_viewer_path(file.to_str().unwrap()).unwrap(),
            None
        );
        assert!(super::resolve_local_viewer_path(temp.0.to_str().unwrap()).is_err());
        assert!(super::resolve_local_viewer_path(temp.0.join("gone").to_str().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn viewer_target_resolves_symlinks_and_rejects_broken_or_directory_links() {
        use std::os::unix::fs::symlink;
        let temp = ViewerTestDir::new();
        let file = temp.0.join("clip.mp4");
        fs::write(&file, b"video").unwrap();
        let link = temp.0.join("clip-link");
        symlink(&file, &link).unwrap();
        assert_eq!(
            super::resolve_local_viewer_path(link.to_str().unwrap()).unwrap(),
            Some(crate::vfs::local::normalize_path(
                fs::canonicalize(&file).unwrap()
            ))
        );
        let dir_link = temp.0.join("dir-link");
        symlink(&temp.0, &dir_link).unwrap();
        assert!(super::resolve_local_viewer_path(dir_link.to_str().unwrap()).is_err());
        fs::remove_file(&file).unwrap();
        assert!(super::resolve_local_viewer_path(link.to_str().unwrap()).is_err());
    }

    #[test]
    fn viewer_target_reply_has_correlated_camel_case_file_descriptor() {
        let message = crate::ws::protocol::ServerMessage::ViewerFileReady {
            timestamp: 1,
            request_id: "preview-request".into(),
            target: crate::ws::protocol::ViewerTargetSchema::File {
                file: crate::ws::protocol::ViewerFileSchema {
                    panel_id: "left".into(),
                    path: "/clips/video.mp4".into(),
                    source_id: "volume".into(),
                    filename: "video.mp4".into(),
                    file_size: Some(42),
                    is_local: true,
                    initial_find_query: None,
                },
            },
        };
        let wire = serde_json::to_value(message).unwrap();
        assert_eq!(wire["type"], "ViewerFileReady");
        assert_eq!(wire["requestId"], "preview-request");
        assert_eq!(wire["target"]["file"]["panelId"], "left");
        assert_eq!(wire["target"]["file"]["sourceId"], "volume");
        assert_eq!(wire["target"]["file"]["fileSize"], 42);
        assert_eq!(wire["target"]["file"]["isLocal"], true);
        let explicit = super::ViewFileArgs::from_args(&serde_json::json!({
            "panelId": "left", "sourceId": "volume", "filename": "video.mp4",
            "filePath": "/clips/video.mp4",
        }));
        assert_eq!(explicit.file_path.as_deref(), Some("/clips/video.mp4"));
    }

    #[test]
    fn search_viewer_resolves_duplicate_names_in_panel_order() {
        use crate::jobs::search::{SearchResultEntry, SearchResultStore};
        use crate::vfs::{search::SearchResultsDriver, VfsDriver};
        use crate::ws::protocol::SortSpec;

        // Store arrival order differs from the sorted panel, and the files
        // belong to different sources. Neither the name nor index is an ID.
        let results = ["/repo/README.md", "/deps/README.md"]
            .into_iter()
            .enumerate()
            .map(|(index, path)| SearchResultEntry {
                name: "README.md".into(),
                kind: "file".into(),
                size: Some(10),
                mtime: None,
                parent_path: path.rsplit_once('/').unwrap().0.into(),
                full_path: path.into(),
                source_id: format!("vol-{index}"),
            })
            .collect();
        let driver = SearchResultsDriver::new(std::sync::Arc::new(tokio::sync::Mutex::new(
            SearchResultStore {
                results,
                done: true,
            },
        )));
        let entries = driver
            .list(
                "/",
                &SortSpec {
                    by: "name".into(),
                    order: "asc".into(),
                },
                None,
                false,
            )
            .unwrap();

        for (cursor, path, source) in [
            (0, "/deps/README.md", "vol-1"),
            (1, "/repo/README.md", "vol-0"),
            (0, "/deps/README.md", "vol-1"),
        ] {
            assert_eq!(
                super::search_viewer_target(&driver, entries[cursor].source_path.as_deref()),
                Some((path.into(), source.into()))
            );
        }
        assert_eq!(super::search_viewer_target(&driver, None), None);
        assert_eq!(
            super::search_viewer_target(&driver, Some("/gone/README.md")),
            None
        );
    }

    #[test]
    fn viewer_find_reports_offsets_and_lines_across_read_boundaries() {
        let mut content = vec![b'x'; 65_534];
        content.extend_from_slice(b"NeEdLe\nneedle");
        let (matches, truncated) = find_literal_matches(Cursor::new(content), b"needle", 10).unwrap();
        assert!(!truncated);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].byte_offset, 65_534);
        assert_eq!(matches[0].byte_length, 6);
        assert_eq!(matches[0].line_number, 1);
        assert_eq!(matches[1].byte_offset, 65_541);
        assert_eq!(matches[1].line_number, 2);
    }

    #[test]
    fn viewer_find_caps_pathological_match_counts() {
        let (matches, truncated) = find_literal_matches(Cursor::new(b"aaaaaa"), b"a", 3).unwrap();
        assert!(truncated);
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[2].byte_offset, 2);
    }

    #[test]
    fn markdown_link_extensions_are_restricted() {
        assert!(is_markdown_filename("guide.md"));
        assert!(is_markdown_filename("docs/Guide.MARKDOWN"));
        assert!(is_markdown_filename("notes.mdown"));
        assert!(!is_markdown_filename("https://example.test/page.md?download=1"));
        assert!(!is_markdown_filename("notes.txt"));
        assert!(!is_markdown_filename("README"));
    }

    #[test]
    fn url_path_subset_rejects_encoded_and_explicit_url_syntax() {
        assert_eq!(safe_relative_path("docs/next%2Emd").as_deref(), Some("docs/next.md"));
        for value in [
            "https://example.test/x.md", "//example.test/x.md", "/etc/passwd",
            "%2fetc/passwd", "foo%3abar.md", "foo%5cbar.md", "foo.md%3Fx=1",
            "foo.md%23fragment", "data:text/html,x",
        ] {
            assert_eq!(safe_relative_path(value), None, "{value}");
        }
    }

    #[test]
    fn backend_rendering_emits_only_verified_local_targets() {
        let dir = std::env::temp_dir().join(format!(
            "fbro-markdown-test-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        fs::create_dir_all(&dir).unwrap();
        let current = dir.join("README.md");
        fs::write(dir.join("next.md"), "# next").unwrap();
        fs::write(dir.join("pixel.png"), b"\x89PNG\r\n\x1a\n").unwrap();
        fs::write(dir.join("bad.svg"), b"<svg onload='alert(1)'/>").unwrap();
        let markdown = "[good](next.md) [missing](no.md) [web](https://example.test)\n\n\
            ![pixel](pixel.png) ![vector](bad.svg)\n\n<img src=x onerror=alert(1)>";
        let rendered = render_markdown(
            &LocalFsDriver, &current.to_string_lossy(), markdown, 43210, 1024,
        );

        assert_eq!(rendered.links.len(), 1);
        assert_eq!(rendered.links[0].path, dir.join("next.md").to_string_lossy().replace('\\', "/"));
        assert!(rendered.html.contains("href=\"#fbro-md-0\""));
        assert!(rendered.html.contains("good"));
        assert!(rendered.html.contains("missing"));
        assert!(rendered.html.contains("web"));
        assert!(!rendered.html.contains("https://example.test"));
        assert!(rendered.html.contains(&format!(
            "src=\"{}\"", markdown_image_url(43210, &dir.join("pixel.png").to_string_lossy().replace('\\', "/")),
        )));
        assert!(!rendered.html.contains("bad.svg"));
        assert!(!rendered.html.contains("onerror"));
        assert!(!rendered.html.contains("<img src=x"));

        let html_images = render_markdown(
            &LocalFsDriver, &current.to_string_lossy(),
            r#"<p align="CENTER" style="background:url(https://example.test)">
              <img src="pixel.png" alt="logo &quot; &amp; &lt;" width="96" height="96" title="Logo" onerror="alert(1)" srcset="https://example.test/x.png 2x">
              <img src="https://example.test/x.png" alt="remote">
              <img src="bad.svg" alt="vector">
              <script>alert(1)</script><iframe src="pixel.png"></iframe>
            </p>"#, 43210, 1024,
        );
        assert!(html_images.html.contains("<p class=\"viewer__markdown-align-center\">"));
        assert!(html_images.html.contains("width=\"96\" height=\"96\""));
        assert!(html_images.html.contains("alt=\"logo &quot; &amp; &lt;\""));
        assert!(html_images.html.contains("title=\"Logo\""));
        assert_eq!(html_images.html.matches("<img ").count(), 1);
        for forbidden in ["onerror", "srcset", "style=", "https://", "<script", "<iframe", "bad.svg"] {
            assert!(!html_images.html.contains(forbidden), "{forbidden}: {}", html_images.html);
        }
        let inline = render_markdown(
            &LocalFsDriver, &current.to_string_lossy(),
            "Inline <img src='pixel.png' width='invalid' height='0'> image.", 43210, 1024,
        );
        assert!(inline.html.contains("<img src="));
        assert!(!inline.html.contains("width="));
        assert!(!inline.html.contains("height="));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn markdown_image_urls_have_a_path_separator_on_all_platforms() {
        assert_eq!(markdown_image_url(43210, "C:/Users/me/logo 256.png"),
            "http://127.0.0.1:43210/C:/Users/me/logo%20256.png");
        assert_eq!(markdown_image_url(43210, "/home/me/logo 256.png"),
            "http://127.0.0.1:43210/home/me/logo%20256.png");
    }

    #[test]
    fn mermaid_interactions_and_urls_are_corrupted() {
        let source = "graph TD\n A-->B\n click A href \"https://example.test\"\n B[ok]";
        let safe = sanitize_mermaid(source);
        assert!(safe.contains("A-->B"));
        assert!(safe.contains("B[ok]"));
        assert!(!safe.contains("click A"));
        assert!(!safe.contains("https://"));
    }
}
