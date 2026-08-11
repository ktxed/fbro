use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use serde_json::Value;
use similar::{ChangeTag, TextDiff};

use crate::state::settings::CompareDifferMode;
use crate::state::{source_id_for_local_path, AppState, CompareSession, GitCompareNavigation, Panel, Source};
use crate::vfs::{git_diff::GitDiffSnapshotDriver, search::SearchResultsDriver, VfsDriver};
use crate::ws::intents::{
    arg_str, is_archive_source, is_global_selection_source, is_procs_source, is_search_source,
};
use crate::ws::protocol::{
    now, CompareDiffNavigationSchema, CompareDiffResultSchema, CompareDiffRowSchema,
    CompareTargetSchema, EntrySchema, ServerMessage,
};
use crate::ws::runtime::{resolve_panel_id, url_encode, IntentContext};

const TEXT_READ_CHUNK: usize = 64 * 1024;

#[derive(Clone)]
struct CompareTarget {
    panel_id: String,
    source_id: String,
    path: String,
    name: String,
    size: Option<u64>,
    local_path: Option<String>,
    driver: Arc<dyn VfsDriver>,
}

impl CompareTarget {
    fn schema(&self) -> CompareTargetSchema {
        CompareTargetSchema {
            panel_id: self.panel_id.clone(),
            source_id: self.source_id.clone(),
            path: self.path.clone(),
            name: self.name.clone(),
            size: self.size,
        }
    }
}

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "CompareFiles" => compare_files(scope, args, ctx).await,
        "GitViewDiff" => git_view_diff(args, ctx).await,
        "GetCompareDiff" => get_compare_diff(args, ctx).await,
        "NavigateCompareDiff" => navigate_compare_diff(args, ctx).await,
        "CloseCompareWindow" => close_compare_window(args, ctx).await,
        _ => {}
    }
}

async fn git_view_diff(args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = arg_str(args, "panelId").unwrap_or("");
    let target_path = arg_str(args, "targetPath").unwrap_or("");
    let commit_hash = arg_str(args, "commitHash").map(str::to_string);
    let old_path = arg_str(args, "oldPath").map(str::to_string);
    let repository = if panel_id.is_empty() {
        Err("A changed Git file is required.".to_string())
    } else {
        resolve_git_diff_repo(panel_id, ctx).await
    };
    let operation = crate::journal::record_git_intent(
        "GitViewDiff",
        args,
        repository.as_ref().ok().map(|(root, _)| root.as_str()),
        repository.as_ref().ok().map(|(_, source_id)| source_id.as_str()),
    );
    let result = match repository {
        Ok((root, _)) if !target_path.is_empty() => {
            launch_git_diff(panel_id, target_path, root, commit_hash, old_path, ctx).await
        }
        Ok(_) => Err("A changed Git file is required.".to_string()),
        Err(message) => Err(message),
    };
    match result {
        Ok(()) => {
            if let Some(operation) = operation.as_ref() {
                crate::journal::record_git_result(operation, "completed", None);
            }
        }
        Err(message) => {
            if let Some(operation) = operation.as_ref() {
                crate::journal::record_git_result(operation, "failed", Some(&message));
            }
            send_compare_error(ctx, message, None).await;
        }
    }
}

async fn compare_files(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = match arg_str(args, "panelId") {
        Some(panel_id) => panel_id.to_string(),
        None => resolve_panel_id(scope, ctx.state).await,
    };
    let target_path = arg_str(args, "targetPath").map(str::to_string);
    if let Err(message) = launch_compare_for_panel(&panel_id, target_path.as_deref(), ctx).await {
        send_compare_error(ctx, message, None).await;
    }
}

pub(crate) async fn launch_compare_for_panel(
    panel_id: &str,
    target_path: Option<&str>,
    ctx: &mut IntentContext<'_>,
) -> Result<(), String> {
    let targets = {
        let state_guard = ctx.state.lock().await;
        resolve_compare_targets(&state_guard, panel_id, target_path)?
    };
    launch_compare_targets(targets, true, ctx).await
}

pub(crate) fn can_compare_from_state(
    state: &AppState,
    panel_id: &str,
    target_path: Option<&str>,
) -> bool {
    resolve_compare_targets(state, panel_id, target_path).is_ok()
}

async fn launch_compare_targets(
    targets: [CompareTarget; 2],
    allow_external: bool,
    ctx: &mut IntentContext<'_>,
) -> Result<(), String> {
    let (mode, external_command, max_file_size_mb, color_scheme, viewer_bg) = {
        let state_guard = ctx.state.lock().await;
        let color_scheme = state_guard.settings.general.color_scheme.clone();
        let viewer_bg = match color_scheme.as_str() {
            "daylight" => tauri::webview::Color(244, 246, 248, 255),
            _ => tauri::webview::Color(13, 13, 13, 255),
        };
        (
            state_guard.settings.general.compare_differ_mode,
            state_guard
                .settings
                .general
                .compare_external_command
                .clone(),
            state_guard.settings.general.compare_max_file_size_mb,
            color_scheme,
            viewer_bg,
        )
    };

    if allow_external && mode != CompareDifferMode::Internal {
        match launch_external_differ(&targets, &external_command).await {
            Ok(()) => return Ok(()),
            Err(error) if mode == CompareDifferMode::External => return Err(error),
            Err(error) => {
                eprintln!("[compare] external differ unavailable, falling back internal: {error}");
            }
        }
    }

    let max_bytes = max_file_size_mb.saturating_mul(1024 * 1024);
    let result = build_diff(&targets[0], &targets[1], max_bytes).await?;
    open_internal_diff(result, color_scheme, viewer_bg, None, ctx).await
}

async fn launch_git_diff(
    panel_id: &str,
    target_path: &str,
    root: String,
    commit_hash: Option<String>,
    old_path: Option<String>,
    ctx: &mut IntentContext<'_>,
) -> Result<(), String> {
    let (max_bytes, color_scheme, viewer_bg) = {
        let state_guard = ctx.state.lock().await;
        let color_scheme = state_guard.settings.general.color_scheme.clone();
        let viewer_bg = match color_scheme.as_str() {
            "daylight" => tauri::webview::Color(244, 246, 248, 255),
            _ => tauri::webview::Color(13, 13, 13, 255),
        };
        (
            state_guard.settings.general.compare_max_file_size_mb.saturating_mul(1024 * 1024),
            color_scheme,
            viewer_bg,
        )
    };
    let changes = if let Some(hash) = commit_hash.as_deref() {
        crate::git::commit_show(&root, hash).unwrap_or_default()
    } else {
        crate::git::inspect(&root).map(|status| status.changes).unwrap_or_default()
    };
    let current_index = changes.iter().position(|change| {
        change.path == target_path || Path::new(&root).join(&change.path) == Path::new(target_path)
    });
    let mut result = build_git_diff_result(
        panel_id,
        &root,
        target_path,
        commit_hash.as_deref(),
        old_path.as_deref(),
        max_bytes,
    ).await?;
    let navigation = current_index.map(|current_index| GitCompareNavigation {
        panel_id: panel_id.to_string(),
        root,
        commit_hash,
        changes,
        current_index,
    });
    if let Some(navigation) = navigation.as_ref() {
        set_navigation_metadata(&mut result, navigation);
    }
    open_internal_diff(result, color_scheme, viewer_bg, navigation, ctx).await
}

async fn build_git_diff_result(
    panel_id: &str,
    root: &str,
    target_path: &str,
    commit_hash: Option<&str>,
    old_path: Option<&str>,
    max_bytes: u64,
) -> Result<CompareDiffResultSchema, String> {
    let root = root.to_string();
    let target_path = target_path.to_string();
    let commit_hash = commit_hash.map(str::to_string);
    let old_path = old_path.map(str::to_string);
    let content = tokio::task::spawn_blocking(move || {
        crate::git::read_diff_content(
            &root,
            &target_path,
            max_bytes,
            commit_hash.as_deref(),
            old_path.as_deref(),
        )
    })
    .await
    .map_err(|error| format!("Git diff task failed: {error}"))??;
    let driver = Arc::new(GitDiffSnapshotDriver::new(
        content.head_path,
        content.head,
        content.worktree_path,
        content.worktree,
    ));
    let source_id = format!("git-diff:{}", uuid::Uuid::new_v4());
    let left_path = driver.head_path().to_string();
    let right_path = driver.worktree_path().to_string();
    let targets = [
        CompareTarget {
            panel_id: panel_id.to_string(),
            source_id: source_id.clone(),
            name: left_path.clone(),
            size: driver.file_size(&left_path),
            path: left_path,
            local_path: None,
            driver: driver.clone(),
        },
        CompareTarget {
            panel_id: panel_id.to_string(),
            source_id,
            name: right_path.clone(),
            size: driver.file_size(&right_path),
            path: right_path,
            local_path: None,
            driver,
        },
    ];
    build_diff(&targets[0], &targets[1], max_bytes).await
}

async fn resolve_git_diff_repo(
    panel_id: &str,
    ctx: &IntentContext<'_>,
) -> Result<(String, String), String> {
    let (panel_path, source_id, local) = {
        let state_guard = ctx.state.lock().await;
        let panel = state_guard
            .panels
            .get(panel_id)
            .ok_or_else(|| "Git panel is unavailable.".to_string())?;
        let local = state_guard
            .drivers
            .get(&panel.source_id)
            .map(|driver| driver.is_local())
            .unwrap_or(false);
        (panel.path.clone(), panel.source_id.clone(), local)
    };
    if !local {
        return Err("Git diffs are only available for local filesystem panels.".to_string());
    }
    let root = crate::git::inspect(&panel_path)
        .map(|status| status.root)
        .ok_or_else(|| "No Git repository is available for this panel.".to_string())?;
    Ok((root, source_id))
}

async fn open_internal_diff(
    result: CompareDiffResultSchema,
    color_scheme: String,
    viewer_bg: tauri::webview::Color,
    git_navigation: Option<GitCompareNavigation>,
    ctx: &mut IntentContext<'_>,
) -> Result<(), String> {
    let request_id = uuid::Uuid::new_v4().to_string();
    {
        let mut state_guard = ctx.state.lock().await;
        state_guard.compare_sessions.insert(
            request_id.clone(),
            CompareSession {
                result: Ok(result.clone()),
                git_navigation,
            },
        );
        while state_guard.compare_sessions.len() > 16 {
            if let Some(oldest) = state_guard.compare_sessions.keys().next().cloned() {
                state_guard.compare_sessions.remove(&oldest);
            } else {
                break;
            }
        }
    }

    let label = format!("diff-{request_id}");
    let title = format!(
        "{} ↔ {} — fbro compare",
        result.left.name, result.right.name
    );
    let query = format!(
        "?diff=1&request={}&label={}&colorScheme={}",
        url_encode(&request_id),
        url_encode(&label),
        url_encode(&color_scheme),
    );
    let app_handle = ctx.app_handle.clone();
    let window_result = tokio::task::spawn_blocking(move || {
        tauri::WebviewWindowBuilder::new(&app_handle, label, tauri::WebviewUrl::App(query.into()))
            .title(title)
            .inner_size(1200.0, 760.0)
            .resizable(true)
            .devtools(true)
            .background_color(viewer_bg)
            .build()
            .map(|_| ())
    })
    .await
    .map_err(|error| format!("Diff window task failed: {error}"))?;

    if let Err(error) = window_result {
        ctx.state.lock().await.compare_sessions.remove(&request_id);
        return Err(format!("Could not open diff window: {error}"));
    }

    Ok(())
}

async fn launch_external_differ(
    targets: &[CompareTarget; 2],
    external_command: &str,
) -> Result<(), String> {
    let command = external_command.trim();
    if command.is_empty() {
        return Err("External differ command is empty.".to_string());
    }
    let left = targets[0]
        .local_path
        .as_ref()
        .ok_or_else(|| {
            "External differ requires both files to be local filesystem paths.".to_string()
        })?
        .clone();
    let right = targets[1]
        .local_path
        .as_ref()
        .ok_or_else(|| {
            "External differ requires both files to be local filesystem paths.".to_string()
        })?
        .clone();
    let command = command.to_string();
    tokio::task::spawn_blocking(move || {
        Command::new(&command)
            .arg(left)
            .arg(right)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Could not launch external differ {command}: {error}"))
    })
    .await
    .map_err(|error| format!("External differ launch task failed: {error}"))?
}

async fn build_diff(
    left: &CompareTarget,
    right: &CompareTarget,
    max_bytes: u64,
) -> Result<CompareDiffResultSchema, String> {
    let left_text = read_text_file(left, max_bytes).await?;
    let right_text = read_text_file(right, max_bytes).await?;
    Ok(build_diff_from_text(
        left.schema(),
        right.schema(),
        left_text,
        right_text,
    ))
}

fn build_diff_from_text(
    left: CompareTargetSchema,
    right: CompareTargetSchema,
    left_text: String,
    right_text: String,
) -> CompareDiffResultSchema {
    let diff = TextDiff::from_lines(&left_text, &right_text);
    let mut rows = Vec::new();
    let mut unified = String::new();
    let mut identical = true;

    for change in diff.iter_all_changes() {
        let text = trim_diff_line(change.value());
        match change.tag() {
            ChangeTag::Equal => {
                rows.push(CompareDiffRowSchema {
                    kind: "equal".to_string(),
                    left_line: change.old_index().map(|n| (n + 1) as u32),
                    right_line: change.new_index().map(|n| (n + 1) as u32),
                    left_text: Some(text.clone()),
                    right_text: Some(text.clone()),
                });
                unified.push(' ');
            }
            ChangeTag::Delete => {
                identical = false;
                rows.push(CompareDiffRowSchema {
                    kind: "delete".to_string(),
                    left_line: change.old_index().map(|n| (n + 1) as u32),
                    right_line: None,
                    left_text: Some(text.clone()),
                    right_text: None,
                });
                unified.push('-');
            }
            ChangeTag::Insert => {
                identical = false;
                rows.push(CompareDiffRowSchema {
                    kind: "insert".to_string(),
                    left_line: None,
                    right_line: change.new_index().map(|n| (n + 1) as u32),
                    left_text: None,
                    right_text: Some(text.clone()),
                });
                unified.push('+');
            }
        }
        unified.push_str(&text);
        unified.push('\n');
    }

    CompareDiffResultSchema {
        left,
        right,
        rows,
        unified,
        identical,
        navigation: None,
    }
}

fn set_navigation_metadata(result: &mut CompareDiffResultSchema, navigation: &GitCompareNavigation) {
    result.navigation = Some(CompareDiffNavigationSchema {
        current_index: navigation.current_index,
        total: navigation.changes.len(),
    });
}

fn trim_diff_line(value: &str) -> String {
    value.trim_end_matches(['\r', '\n']).to_string()
}

async fn read_text_file(target: &CompareTarget, max_bytes: u64) -> Result<String, String> {
    if let Some(size) = target.size {
        if size > max_bytes {
            return Err(format!(
                "{} is larger than the compare limit ({} MiB).",
                target.name,
                max_bytes / 1024 / 1024
            ));
        }
    }

    let driver = target.driver.clone();
    let path = target.path.clone();
    let cap = max_bytes as usize;
    let bytes = tokio::task::spawn_blocking(move || read_all_capped(driver, path, cap))
        .await
        .map_err(|error| format!("Compare read task failed: {error}"))??;
    validate_text_bytes(&target.name, bytes, max_bytes)
}

fn read_all_capped(
    driver: Arc<dyn VfsDriver>,
    path: String,
    cap: usize,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut offset = 0u64;
    loop {
        let remaining = cap.saturating_add(1).saturating_sub(out.len());
        if remaining == 0 {
            return Err("File exceeds compare size limit.".to_string());
        }
        let to_read = remaining.min(TEXT_READ_CHUNK);
        let chunk = driver
            .read_chunk(&path, offset, to_read)
            .map_err(|error| format!("Could not read {path}: {error}"))?;
        if chunk.is_empty() {
            break;
        }
        offset += chunk.len() as u64;
        out.extend_from_slice(&chunk);
        if out.len() > cap {
            return Err("File exceeds compare size limit.".to_string());
        }
        if chunk.len() < to_read {
            break;
        }
    }
    Ok(out)
}

fn validate_text_bytes(name: &str, mut bytes: Vec<u8>, max_bytes: u64) -> Result<String, String> {
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "{name} exceeds the compare size limit ({} MiB).",
            max_bytes / 1024 / 1024
        ));
    }
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes.drain(..3);
    }
    if bytes.contains(&0) {
        return Err(format!("{name} is not a text file."));
    }
    let non_text = bytes
        .iter()
        .filter(|&&b| b < 9 || (b > 13 && b < 32) || b == 127)
        .count();
    if !bytes.is_empty() && non_text * 10 > bytes.len() * 3 {
        return Err(format!("{name} is not a text file."));
    }
    String::from_utf8(bytes).map_err(|_| format!("{name} is not valid UTF-8 text."))
}

fn resolve_compare_targets(
    state: &AppState,
    panel_id: &str,
    target_path: Option<&str>,
) -> Result<[CompareTarget; 2], String> {
    let panel = state
        .panels
        .get(panel_id)
        .ok_or_else(|| "Compare panel is unavailable.".to_string())?;

    if let Ok(targets) = resolve_same_panel_targets(state, panel, target_path) {
        return Ok(targets);
    }

    let other_panel_id = active_other_panel_id(state, panel_id).ok_or_else(|| {
        "Compare requires another visible panel or two files in one panel.".to_string()
    })?;
    let other_panel = state
        .panels
        .get(&other_panel_id)
        .ok_or_else(|| "Other compare panel is unavailable.".to_string())?;
    let left = resolve_single_panel_target(state, panel, target_path)?;
    let right = resolve_single_panel_target(state, other_panel, None)?;
    ensure_distinct(&left, &right)?;
    Ok([left, right])
}

fn resolve_same_panel_targets(
    state: &AppState,
    panel: &Panel,
    target_path: Option<&str>,
) -> Result<[CompareTarget; 2], String> {
    let driver = state
        .drivers
        .get(&panel.source_id)
        .ok_or_else(|| "Compare source driver is unavailable.".to_string())?;
    let selected = selected_file_entries(panel);
    let mut entries: Vec<&EntrySchema> = Vec::new();
    if selected.len() == 2
        && target_path
            .map(|path| {
                selected
                    .iter()
                    .any(|entry| driver.resolve_child(&panel.path, &entry.name) == path)
            })
            .unwrap_or(true)
    {
        entries.extend(selected);
    } else if selected.len() == 1 {
        entries.push(selected[0]);
        let cursor_entry = target_path
            .and_then(|path| entry_for_path(panel, driver.as_ref(), path))
            .or_else(|| entry_at_cursor(panel));
        if let Some(cursor_entry) = cursor_entry {
            if cursor_entry.name != entries[0].name || target_path.is_some() {
                entries.push(cursor_entry);
            }
        }
    } else if selected.is_empty() && target_path.is_some() {
        if let Some(target_entry) =
            target_path.and_then(|path| entry_for_path(panel, driver.as_ref(), path))
        {
            entries.push(target_entry);
        }
        if let Some(cursor_entry) = entry_at_cursor(panel) {
            if entries
                .first()
                .map(|entry| entry.name != cursor_entry.name)
                .unwrap_or(true)
            {
                entries.push(cursor_entry);
            }
        }
    }

    if entries.len() != 2 {
        return Err("Choose exactly two files to compare.".to_string());
    }
    let left = target_for_entry(state, panel, entries[0])?;
    let right = target_for_entry(state, panel, entries[1])?;
    ensure_distinct(&left, &right)?;
    Ok([left, right])
}

fn resolve_single_panel_target(
    state: &AppState,
    panel: &Panel,
    target_path: Option<&str>,
) -> Result<CompareTarget, String> {
    let driver = state
        .drivers
        .get(&panel.source_id)
        .ok_or_else(|| "Compare source driver is unavailable.".to_string())?;
    if let Some(path) = target_path {
        let entry = entry_for_path(panel, driver.as_ref(), path)
            .ok_or_else(|| "The requested compare file is no longer available.".to_string())?;
        return target_for_entry(state, panel, entry);
    }

    let selected = selected_file_entries(panel);
    if selected.len() > 1 {
        return Err("Choose only one file per panel for cross-panel compare.".to_string());
    }
    let entry = selected
        .first()
        .copied()
        .or_else(|| entry_at_cursor(panel))
        .ok_or_else(|| "Choose one file in each panel to compare.".to_string())?;
    target_for_entry(state, panel, entry)
}

fn selected_file_entries(panel: &Panel) -> Vec<&EntrySchema> {
    panel
        .selection
        .iter()
        .filter_map(|index| panel.entries.get(*index))
        .filter(|entry| entry.name != ".." && entry.kind == "file")
        .collect()
}

fn entry_at_cursor(panel: &Panel) -> Option<&EntrySchema> {
    panel
        .entries
        .get(panel.cursor)
        .filter(|entry| entry.name != ".." && entry.kind == "file")
}

fn entry_for_path<'a>(
    panel: &'a Panel,
    driver: &dyn VfsDriver,
    path: &str,
) -> Option<&'a EntrySchema> {
    panel.entries.iter().find(|entry| {
        entry.name != ".."
            && entry.kind == "file"
            && (entry.source_path.as_deref() == Some(path)
                || driver.resolve_child(&panel.path, &entry.name) == path)
    })
}

fn target_for_entry(
    state: &AppState,
    panel: &Panel,
    entry: &EntrySchema,
) -> Result<CompareTarget, String> {
    let base_driver = state
        .drivers
        .get(&panel.source_id)
        .cloned()
        .ok_or_else(|| "Compare source driver is unavailable.".to_string())?;
    let (source_id, path) = effective_source_and_path(state, panel, entry, base_driver.as_ref())?;
    let driver = state
        .drivers
        .get(&source_id)
        .cloned()
        .ok_or_else(|| "Compare target driver is unavailable.".to_string())?;
    let size = entry.size.or_else(|| driver.file_size(&path));
    let local_path = driver
        .local_path(&path)
        .map(|path| path.to_string_lossy().to_string());
    Ok(CompareTarget {
        panel_id: panel.panel_id.clone(),
        source_id,
        path,
        name: entry.name.clone(),
        size,
        local_path,
        driver,
    })
}

fn effective_source_and_path(
    state: &AppState,
    panel: &Panel,
    entry: &EntrySchema,
    base_driver: &dyn VfsDriver,
) -> Result<(String, String), String> {
    if is_global_selection_source(&panel.source_id) {
        return global_selection_reference_source(&state.sources, entry);
    }

    if is_search_source(&panel.source_id) {
        let resolved = state
            .drivers
            .get(&panel.source_id)
            .and_then(|drv| drv.as_any().downcast_ref::<SearchResultsDriver>())
            .and_then(|sd| {
                entry
                    .source_path
                    .as_deref()
                    .and_then(|path| sd.resolve_real_info_by_path(path))
                    .or_else(|| sd.resolve_real_info(&entry.name))
            });
        if let Some((real_path, real_source)) = resolved {
            return Ok((real_source, real_path));
        }
    }

    if is_archive_source(&panel.source_id) || is_procs_source(&panel.source_id) {
        let p = panel.path.trim_end_matches('/');
        let path = if p.is_empty() {
            entry.name.clone()
        } else {
            format!("{p}/{}", entry.name)
        };
        return Ok((panel.source_id.clone(), path));
    }

    Ok((
        panel.source_id.clone(),
        base_driver.resolve_child(&panel.path, &entry.name),
    ))
}

fn global_selection_reference_source(
    sources: &HashMap<String, Source>,
    entry: &EntrySchema,
) -> Result<(String, String), String> {
    let path = entry
        .source_path
        .as_deref()
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "Global selection reference has no filesystem path.".to_string())?;
    let source_id = source_id_for_local_path(sources, path).ok_or_else(|| {
        "Global selection reference is not available on a mounted local source.".to_string()
    })?;
    Ok((source_id, path.to_string()))
}

fn active_other_panel_id(state: &AppState, panel_id: &str) -> Option<String> {
    state
        .layout_slots
        .iter()
        .filter_map(|slot_id| state.slots.get(slot_id))
        .map(|slot| slot.active_panel_id.as_str())
        .find(|id| *id != panel_id)
        .map(str::to_string)
}

fn ensure_distinct(left: &CompareTarget, right: &CompareTarget) -> Result<(), String> {
    let mut seen = HashSet::new();
    seen.insert((left.source_id.as_str(), left.path.as_str()));
    if !seen.insert((right.source_id.as_str(), right.path.as_str())) {
        return Err("Choose two different files to compare.".to_string());
    }
    Ok(())
}

async fn navigate_compare_diff(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = arg_str(args, "requestId").unwrap_or("").to_string();
    let direction = arg_str(args, "direction").unwrap_or("");
    if request_id.is_empty() || !matches!(direction, "prev" | "next") {
        return;
    }
    let (mut navigation, max_bytes, current_result) = {
        let state_guard = ctx.state.lock().await;
        let Some(session) = state_guard.compare_sessions.get(&request_id) else {
            return;
        };
        let Some(navigation) = session.git_navigation.clone() else {
            return;
        };
        (
            navigation,
            state_guard.settings.general.compare_max_file_size_mb.saturating_mul(1024 * 1024),
            session.result.clone(),
        )
    };
    let next_index = adjacent_git_change_index(
        navigation.current_index,
        navigation.changes.len(),
        direction,
    );
    if next_index == navigation.current_index {
        if let Ok(result) = current_result {
            let _ = ctx.tx.send(ServerMessage::CompareDiffReady {
                timestamp: now(),
                request_id,
                result,
            }).await;
        }
        return;
    }
    let mut candidate_index = next_index;
    loop {
        let change = navigation.changes[candidate_index].clone();
        match build_git_diff_result(
            &navigation.panel_id,
            &navigation.root,
            &change.path,
            navigation.commit_hash.as_deref(),
            change.old_path.as_deref(),
            max_bytes,
        ).await {
            Ok(mut result) => {
                navigation.current_index = candidate_index;
                set_navigation_metadata(&mut result, &navigation);
                {
                    let mut state_guard = ctx.state.lock().await;
                    let Some(session) = state_guard.compare_sessions.get_mut(&request_id) else {
                        return;
                    };
                    session.result = Ok(result.clone());
                    session.git_navigation = Some(navigation);
                }
                let _ = ctx.tx.send(ServerMessage::CompareDiffReady {
                    timestamp: now(),
                    request_id,
                    result,
                }).await;
                return;
            }
            Err(message) => {
                let following_index = adjacent_git_change_index(
                    candidate_index,
                    navigation.changes.len(),
                    direction,
                );
                if following_index == candidate_index {
                    let _ = ctx.tx.send(ServerMessage::CompareDiffFailed {
                        timestamp: now(),
                        request_id,
                        message,
                    }).await;
                    return;
                }
                candidate_index = following_index;
            }
        }
    }
}

fn adjacent_git_change_index(current: usize, total: usize, direction: &str) -> usize {
    if total == 0 {
        return 0;
    }
    if direction == "prev" {
        current.saturating_sub(1)
    } else {
        current.saturating_add(1).min(total - 1)
    }
}

async fn get_compare_diff(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = arg_str(args, "requestId").unwrap_or("").to_string();
    if request_id.is_empty() {
        return;
    }
    let result = {
        let state_guard = ctx.state.lock().await;
        state_guard
            .compare_sessions
            .get(&request_id)
            .map(|session| session.result.clone())
    };
    match result {
        Some(Ok(result)) => {
            let _ = ctx
                .tx
                .send(ServerMessage::CompareDiffReady {
                    timestamp: now(),
                    request_id,
                    result,
                })
                .await;
        }
        Some(Err(message)) => {
            let _ = ctx
                .tx
                .send(ServerMessage::CompareDiffFailed {
                    timestamp: now(),
                    request_id,
                    message,
                })
                .await;
        }
        None => {
            let _ = ctx
                .tx
                .send(ServerMessage::CompareDiffFailed {
                    timestamp: now(),
                    request_id,
                    message: "Compare session is no longer available.".to_string(),
                })
                .await;
        }
    }
}

async fn close_compare_window(args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = arg_str(args, "requestId").unwrap_or("").to_string();
    let label = arg_str(args, "label").unwrap_or("").to_string();
    if !request_id.is_empty() {
        let mut state_guard = ctx.state.lock().await;
        state_guard.compare_sessions.remove(&request_id);
    }
    if !label.is_empty() {
        use tauri::Manager;
        if let Some(window) = ctx.app_handle.get_webview_window(&label) {
            let _ = window.close();
        }
    }
}

async fn send_compare_error(
    ctx: &mut IntentContext<'_>,
    message: String,
    extra: Option<serde_json::Value>,
) {
    let _ = ctx
        .tx
        .send(ServerMessage::Error {
            timestamp: now(),
            code: "COMPARE_FAILED".to_string(),
            message,
            recoverable: true,
            extra,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{adjacent_git_change_index, global_selection_reference_source, validate_text_bytes};
    use crate::state::Source;
    use crate::ws::protocol::EntrySchema;

    fn local_source(id: &str, root_path: &str, mounted: bool) -> Source {
        Source {
            id: id.to_string(),
            name: id.to_string(),
            kind: "filesystem".to_string(),
            supports: Vec::new(),
            root_path: root_path.to_string(),
            mounted,
            device_path: None,
            shadow: false,
            icon_name: None,
            sort_priority: 100,
            read_only: false,
            peer_id: None,
        }
    }

    fn global_selection_entry(source_path: Option<&str>) -> EntrySchema {
        EntrySchema {
            name: source_path.unwrap_or("missing.txt").to_string(),
            kind: "file".to_string(),
            size: Some(1),
            mtime: None,
            btime: None,
            permissions: None,
            icon_name: None,
            cc: None,
            source_path: source_path.map(str::to_string),
            git_status: None,
            git_details: None,
        }
    }

    #[test]
    fn strict_text_accepts_utf8_bom() {
        let text = validate_text_bytes("a.txt", vec![0xEF, 0xBB, 0xBF, b'a'], 1024).unwrap();
        assert_eq!(text, "a");
    }

    #[test]
    fn strict_text_rejects_nul_bytes() {
        assert!(validate_text_bytes("a.bin", vec![b'a', 0, b'b'], 1024).is_err());
    }

    #[test]
    fn strict_text_rejects_invalid_utf8() {
        assert!(validate_text_bytes("a.bin", vec![0xff], 1024).is_err());
    }

    #[test]
    fn git_diff_navigation_stops_at_changed_file_boundaries() {
        assert_eq!(adjacent_git_change_index(0, 3, "prev"), 0);
        assert_eq!(adjacent_git_change_index(0, 3, "next"), 1);
        assert_eq!(adjacent_git_change_index(1, 3, "prev"), 0);
        assert_eq!(adjacent_git_change_index(2, 3, "next"), 2);
        assert_eq!(adjacent_git_change_index(0, 0, "next"), 0);
    }

    #[test]
    fn global_selection_reference_uses_owning_local_source() {
        let mut sources = HashMap::new();
        sources.insert("root".to_string(), local_source("root", "/", true));
        sources.insert(
            "workspace".to_string(),
            local_source("workspace", "/workspace", true),
        );

        let resolved = global_selection_reference_source(
            &sources,
            &global_selection_entry(Some("/workspace/left.txt")),
        )
        .unwrap();

        assert_eq!(
            resolved,
            ("workspace".to_string(), "/workspace/left.txt".to_string())
        );
    }

    #[test]
    fn global_selection_reference_requires_an_available_local_source() {
        let mut sources = HashMap::new();
        sources.insert(
            "workspace".to_string(),
            local_source("workspace", "/workspace", false),
        );

        let error = global_selection_reference_source(
            &sources,
            &global_selection_entry(Some("/workspace/left.txt")),
        )
        .unwrap_err();

        assert_eq!(
            error,
            "Global selection reference is not available on a mounted local source."
        );
    }

    #[test]
    fn global_selection_reference_requires_a_filesystem_path() {
        let error =
            global_selection_reference_source(&HashMap::new(), &global_selection_entry(None))
                .unwrap_err();

        assert_eq!(error, "Global selection reference has no filesystem path.");
    }
}
