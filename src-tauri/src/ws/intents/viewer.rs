use serde_json::Value;

use crate::ws::intents::{arg_str, arg_u64};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{url_encode, IntentContext};

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "ViewFile" => view_file(args, ctx).await,
        "EditFile" => edit_file(args, ctx).await,
        "CloseViewerWindow" => close_viewer_window(args, ctx),
        "ViewerNavigate" => viewer_navigate(args, ctx).await,
        "ReadFileChunk" => read_file_chunk(args, ctx).await,
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
    let full_path = format!("{}/{}", parsed.path.trim_end_matches('/'), parsed.filename);
    let (is_local, file_size, file_server_port) = {
        let state_guard = ctx.state.lock().await;
        let driver = state_guard.drivers.get(&parsed.source_id);
        (
            driver.map(|driver| driver.is_local()).unwrap_or(false),
            driver.and_then(|driver| driver.file_size(&full_path)),
            state_guard.file_server_port,
        )
    };

    let label = format!("viewer-{}", uuid::Uuid::new_v4());
    let mut query_string = format!(
        "?viewer=1&path={}&source={}&name={}&label={}&panel={}",
        url_encode(&full_path),
        url_encode(&parsed.source_id),
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

    let _ = tauri::WebviewWindowBuilder::new(
        ctx.app_handle,
        label,
        tauri::WebviewUrl::App(query_string.into()),
    )
    .title(format!("{} — fbro viewer", parsed.filename))
    .inner_size(900.0, 700.0)
    .resizable(true)
    .devtools(true)
    .background_color(tauri::webview::Color(13, 13, 13, 255))
    .build();
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

fn close_viewer_window(args: &Value, ctx: &IntentContext<'_>) {
    let parsed = LabelArgs::from_args(args);
    use tauri::Manager;
    if let Some(window) = ctx.app_handle.get_webview_window(&parsed.label) {
        let _ = window.close();
    }
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
            Some((new_cursor, source_id, dir_path, entry_info))
        } else {
            None
        }
    };

    if let Some((new_cursor, source_id, dir_path, entry_info)) = nav_result {
        if let Some((name, kind)) = entry_info {
            if kind == "file" {
                let full_path = format!("{}/{}", dir_path.trim_end_matches('/'), name);
                let driver = ctx.state.lock().await.drivers.get(&source_id).cloned();
                let (file_size, is_local) = driver
                    .as_ref()
                    .map(|driver| (driver.file_size(&full_path), driver.is_local()))
                    .unwrap_or((None, false));
                let _ = ctx
                    .tx
                    .send(ServerMessage::ViewerFileChanged {
                        timestamp: now(),
                        path: full_path,
                        filename: name,
                        source_id,
                        panel_id: parsed.panel_id.clone(),
                        file_size,
                        is_local,
                    })
                    .await;
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
    let (driver, file_size) = {
        let state_guard = ctx.state.lock().await;
        let driver = state_guard.drivers.get(&parsed.source_id).cloned();
        let file_size = driver
            .as_ref()
            .and_then(|driver| driver.file_size(&parsed.path));
        (driver, file_size)
    };

    if let Some(driver) = driver {
        let path_clone = parsed.path.clone();
        let result = tokio::task::spawn_blocking(move || {
            driver.read_chunk(&path_clone, parsed.offset, parsed.size)
        })
        .await;
        match result {
            Ok(Ok(bytes)) => {
                let eof = bytes.len() < parsed.size
                    || file_size.map_or(false, |size| parsed.offset + bytes.len() as u64 >= size);
                let data: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
                let _ = ctx
                    .tx
                    .send(ServerMessage::FileChunk {
                        timestamp: now(),
                        path: parsed.path,
                        offset: parsed.offset,
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
