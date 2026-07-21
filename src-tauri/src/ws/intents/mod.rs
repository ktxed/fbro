mod app;
mod bookmarks;
pub(crate) mod bro;
mod cache;
mod git;
mod compare;
pub(crate) mod global_selection;
mod history;
pub(crate) mod jobs;
mod navigation;
mod panel;
mod procs;
mod remote;
mod settings;
pub(crate) mod sharing;
mod tabs;
mod terminal;
mod viewer;

use serde_json::Value;

use crate::ws::runtime::IntentContext;

pub async fn dispatch(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    // The journal observes semantic commands, not frontend/backend chatter.
    // Its own source is explicitly excluded to prevent recursive audit events.
    if should_record_command(name)
        && args.get("sourceId").and_then(Value::as_str) != Some(crate::journal::SOURCE_ID)
    {
        crate::journal::record_command(
            name,
            scope,
            args,
            args.get("sourceId").and_then(Value::as_str),
        );
    }
    match name {
        "RefreshPanels"
        | "Refresh"
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
        | "OpenPathInRightPanel"
        | "NavigateUp"
        | "RevealInFileManager"
        | "ListOpenWithApps"
        | "LaunchOpenWithApp"
        | "ProvideInput"
        | "NavigateToHistoryEntry" => navigation::handle(name, scope, args, ctx).await,
        "ListBros"
        | "LaunchBro"
        | "KillProcess"
        | "SaveSyncReport"
        | "SaveFedupReport"
        | "GenerateFedupDeleteDryRun"
        | "GetFedupResultPage" => bro::handle(name, scope, args, ctx).await,
        "AddToGlobalSelection" | "RemoveGlobalSelections" | "RemoveGlobalSelectionItems" => {
            global_selection::handle(name, scope, args, ctx).await
        }
        "StartFileOp"
        | "PasteExternalClipboard"
        | "WriteExternalClipboard"
        | "ProvideJobDecision"
        | "CancelJob"
        | "SkipCurrentFile"
        | "PauseJob"
        | "ResumeJob"
        | "DeleteQueuedJob"
        | "ReorderQueuedJob"
        | "RenameFile"
        | "MakeDir"
        | "TouchFile"
        | "SetPermissions"
        | "StartSearch"
        | "NavigateToSearchResult"
        | "MountSearchResults"
        | "RemoveSearchSource" => jobs::handle(name, scope, args, ctx).await,
        "ViewFile"
        | "EditFile"
        | "CloseViewerWindow"
        | "ViewerNavigate"
        | "ReadFileChunk"
        | "SetEmbeddedViewerActive" => viewer::handle(name, args, ctx).await,
        "CompareFiles" | "GetCompareDiff" | "CloseCompareWindow" => {
            compare::handle(name, scope, args, ctx).await
        }
        "QueryCacheStatus" | "ClearCache" => cache::handle(name, args, ctx).await,
        "AddBookmark" | "RemoveBookmark" | "EnterBookmarkSource" => {
            bookmarks::handle(name, scope, args, ctx).await
        }
        "EnterHistorySource" => history::handle(name, scope, args, ctx).await,
        "GitRefresh" | "GitFetch" | "GitStagePaths" | "GitUnstagePaths" | "GitCommit" | "GitSwitchBranch" | "GitPull" | "GitPush" | "GitSetUpstream" => git::handle(name, args, ctx).await,
        "EnterProcsSource" => procs::handle(name, scope, args, ctx).await,
        "Quit" | "ConfirmQuit" | "SetFontSize" => app::handle(name, args, ctx).await,
        "SetGeneralOption" | "SetModalSizePreference" | "ClearThumbnailCache" => {
            settings::handle(name, args, ctx).await
        }
        "OpenPanelTerminal" | "QuickEditFile" | "ResizeTerminal" | "ClosePanelTerminal" => {
            terminal::handle(name, scope, args, ctx).await
        }
        "panel.tab.new" | "panel.tab.close" | "panel.tab.switch" | "panel.terminal.promote" => {
            tabs::handle(name, scope, args, ctx).await
        }
        "EnableSharing"
        | "SetSharedTargets"
        | "SetSharedSources"
        | "SetSharingIpv6"
        | "StartPairing"
        | "ConfirmPairing"
        | "RevokePairing"
        | "RefreshDiscoveredPeers" => sharing::handle(name, args, ctx).await,
        "ConnectRemotePeer"
        | "DisconnectRemotePeer"
        | "ReconnectRemoteSource"
        | "RetryLastPeer" => remote::handle(name, args, ctx).await,
        unknown => eprintln!("[fbro] Unknown intent: {unknown}"),
    }
}

fn should_record_command(name: &str) -> bool {
    matches!(
        name,
        "StartFileOp"
            | "RenameFile"
            | "MakeDir"
            | "TouchFile"
            | "SetPermissions"
            | "WriteExternalClipboard"
            | "PasteExternalClipboard"
            | "ViewFile"
            | "EditFile"
            | "QuickEditFile"
            | "OpenFile"
            | "KillProcess"
            | "OpenTerminal"
            | "GitFetch" | "GitStagePaths" | "GitUnstagePaths" | "GitCommit" | "GitSwitchBranch" | "GitPull" | "GitPush" | "GitSetUpstream"
    )
}

pub(crate) fn is_archive_source(source_id: &str) -> bool {
    source_id.starts_with("zip:") || source_id.starts_with("7z:")
}

/// Capability flag that marks a source whose `directory` entries are intra-driver
/// tree nodes (zip/7z archives, hierarchical procs) rather than pointers into a
/// real local filesystem. When absent, `OpenEntry` on a directory hands off to
/// the local FS driver. See proto.md for the full list of capabilities.
pub(crate) const CAP_INTERNAL_NAV: &str = "InternalNavigation";

pub(crate) fn is_search_source(source_id: &str) -> bool {
    source_id.starts_with("search:")
}

pub(crate) fn is_bookmarks_source(source_id: &str) -> bool {
    source_id == "bookmarks"
}

pub(crate) fn is_history_source(source_id: &str) -> bool {
    source_id == "history"
}

pub(crate) fn is_procs_source(source_id: &str) -> bool {
    source_id == "procs"
}

pub(crate) fn is_journal_source(source_id: &str) -> bool {
    source_id == crate::journal::SOURCE_ID
}

pub(crate) fn is_global_selection_source(source_id: &str) -> bool {
    source_id == crate::vfs::global_selection::SOURCE_ID
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
