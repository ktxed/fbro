mod app;
mod bookmarks;
pub(crate) mod bro;
mod cache;
mod history;
mod jobs;
mod navigation;
mod panel;
mod settings;
mod terminal;
mod viewer;

use serde_json::Value;

use crate::ws::runtime::IntentContext;

pub async fn dispatch(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "RefreshPanels"
        | "SetSort"
        | "SetFilter"
        | "SetFilterMode"
        | "ToggleHidden"
        | "MoveCursor"
        | "SetSelection"
        | "SelectAtIndex"
        | "DeselectAtIndex"
        | "ToggleSelectionAtIndex"
        | "ToggleSelection"
        | "SelectAll"
        | "ClearSelection"
        | "InvertSelection"
        | "MoveCursorWithSelect"
        | "MoveCursorBy"
        | "MoveCursorToEdge"
        | "SetCursor"
        | "CalculateSize"
        | "CancelSizeCalculation"
        | "RequestWindow"
        | "SwitchFocus"
        | "SetPanelViewMode"
        | "RequestStateSnapshot" => panel::handle(name, scope, args, ctx).await,
        "OpenEntry"
        | "NavigateHome"
        | "NavigateSourceRoot"
        | "NavigatePanelTo"
        | "OpenFile"
        | "OpenTerminal"
        | "NavigateToSource"
        | "MountSource"
        | "NavigateUp"
        | "RevealInFileManager"
        | "ListOpenWithApps"
        | "LaunchOpenWithApp"
        | "ProvideInput"
        | "NavigateToHistoryEntry" => navigation::handle(name, scope, args, ctx).await,
        "ListBros" | "LaunchBro" | "KillProcess" => bro::handle(name, scope, args, ctx).await,
        "StartFileOp" | "PasteExternalClipboard" | "WriteExternalClipboard"
        | "ProvideJobDecision" | "CancelJob"
        | "PauseJob" | "ResumeJob" | "RenameFile" | "MakeDir" | "TouchFile"
        | "StartSearch" | "NavigateToSearchResult" | "MountSearchResults"
        | "RemoveSearchSource" => {
            jobs::handle(name, scope, args, ctx).await
        }
        "ViewFile" | "EditFile" | "CloseViewerWindow" | "ViewerNavigate" | "ReadFileChunk"
        | "SetEmbeddedViewerActive" => {
            viewer::handle(name, args, ctx).await
        }
        "QueryCacheStatus" | "ClearCache" => cache::handle(name, args, ctx).await,
        "AddBookmark" | "RemoveBookmark" | "EnterBookmarkSource" => bookmarks::handle(name, scope, args, ctx).await,
        "EnterHistorySource" => history::handle(name, scope, args, ctx).await,
        "Quit" | "ConfirmQuit" | "SetFontSize" => app::handle(name, args, ctx).await,
        "SetGeneralOption" | "ClearThumbnailCache" => settings::handle(name, args, ctx).await,
        "OpenPanelTerminal" | "ResizeTerminal" | "ClosePanelTerminal" => {
            terminal::handle(name, scope, args, ctx).await
        }
        unknown => eprintln!("[fbro] Unknown intent: {unknown}"),
    }
}

pub(crate) fn is_archive_source(source_id: &str) -> bool {
    source_id.starts_with("zip:") || source_id.starts_with("7z:")
}

pub(crate) fn is_search_source(source_id: &str) -> bool {
    source_id.starts_with("search:")
}

pub(crate) fn is_bookmarks_source(source_id: &str) -> bool {
    source_id == "bookmarks"
}

pub(crate) fn is_history_source(source_id: &str) -> bool {
    source_id == "history"
}

/// Strips the archive prefix ("zip:" or "7z:") and returns the filesystem path.
pub(crate) fn archive_file_path(source_id: &str) -> &str {
    source_id
        .strip_prefix("zip:")
        .or_else(|| source_id.strip_prefix("7z:"))
        .unwrap_or(source_id)
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn arg_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn arg_u64(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(Value::as_u64).unwrap_or(default)
}

fn arg_i64(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn arg_usize_vec(args: &Value, key: &str) -> Vec<usize> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_u64().map(|index| index as usize))
                .collect()
        })
        .unwrap_or_default()
}
