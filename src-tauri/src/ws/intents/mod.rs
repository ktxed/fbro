mod app;
mod bookmarks;
pub(crate) mod bro;
mod cache;
mod git;
mod compare;
mod eject;
pub(crate) mod global_selection;
mod history;
pub(crate) mod jobs;
mod navigation;
mod notifications;
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

/// Resolve against the driver captured at launch, never a panel's later location.
pub(crate) fn job_file_location(
    state: &crate::state::AppState,
    driver: &std::sync::Arc<dyn crate::vfs::VfsDriver>,
    path: &str,
) -> Option<crate::ws::protocol::JobFileLocationSchema> {
    let (source_id, path) = if let Some(local_path) = driver.local_path(path) {
        // LocalFsDriver accepts relative strings too; they are not host locations.
        if !local_path.is_absolute() {
            return None;
        }
        let path = crate::vfs::local::normalize_path(local_path);
        (navigation::source_for_local_path(state, &path)?, path)
    } else {
        // A local driver without an explicit host-path mapping is not enough
        // evidence to expose its synthetic or relative path as a host path.
        if driver.is_local() {
            return None;
        }
        let source_id = state
            .sources
            .values()
            .filter(|source| source.mounted)
            .filter(|source| {
                state
                    .drivers
                    .get(&source.id)
                    .is_some_and(|registered| std::sync::Arc::ptr_eq(registered, driver))
            })
            .map(|source| source.id.clone())
            .min()?;
        (source_id, path.to_string())
    };
    let parent_path = state.drivers.get(&source_id)?.parent_path(&path)?;
    let cursor_hint = path.trim_end_matches('/').rsplit('/').next()?;
    if cursor_hint.is_empty() {
        return None;
    }
    Some(crate::ws::protocol::JobFileLocationSchema {
        source_id,
        parent_path,
        cursor_hint: cursor_hint.to_string(),
        path,
    })
}

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
        "PublishNotification" | "ClearNotifications" => notifications::handle(name, args, ctx).await,
        "EjectSource" => eject::handle(args, ctx).await,
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
        | "SelectByMask"
        | "DeselectByMask"
        | "ExtendSelectionTo"
        | "MoveCursorWithSelect"
        | "MoveCursorBy"
        | "MoveCursorToEdge"
        | "SetCursor"
        | "CalculateSize"
        | "ToggleDirectorySizes"
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
        | "ListBroCatalog"
        | "LaunchBro"
        | "KillProcess"
        | "SaveSyncReport"
        | "SaveFedupReport"
        | "GenerateFedupDeleteDryRun"
        | "GetFedupResultPage" => bro::handle(name, scope, args, ctx).await,
        "AddToGlobalSelection" | "RemoveGlobalSelections" | "RemoveGlobalSelectionItems" => {
            global_selection::handle(name, scope, args, ctx).await
        }
        "QueryThumbnailQueue"
        | "StartFileOp"
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
        | "ViewerOpenFile"
        | "ResolveViewerFile"
        | "ReadFileChunk"
        | "FindInViewerFile"
        | "SetEmbeddedViewerActive" => viewer::handle(name, args, ctx).await,
        "CompareFiles"
        | "GitViewDiff"
        | "GetCompareDiff"
        | "NavigateCompareDiff"
        | "CloseCompareWindow" => compare::handle(name, scope, args, ctx).await,
        "QueryCacheStatus" | "ClearCache" => cache::handle(name, args, ctx).await,
        "AddBookmark" | "RemoveBookmark" | "EnterBookmarkSource" => {
            bookmarks::handle(name, scope, args, ctx).await
        }
        "EnterHistorySource" => history::handle(name, scope, args, ctx).await,
        "InitGitRepository" | "RefreshGitStatus" | "GitRefresh" | "GitFetch" | "GitStagePaths" | "GitUnstagePaths" | "GitRollbackPaths" | "GitCommit" | "GitCreateBranch" | "GitCheckoutRemoteBranch" | "GitSwitchBranch" | "GitPull" | "GitPush" | "GitSetUpstream" | "GitGetLog" | "GitGetCommitShow" | "GitCopyRawFile" | "OpenGitReviewRequest" => git::handle(name, args, ctx).await,
        "EnterProcsSource" => procs::handle(name, scope, args, ctx).await,
        "Quit" | "ConfirmQuit" | "SetFontSize" => app::handle(name, args, ctx).await,
        "SetGeneralOption"
        | "SetModalSizePreference"
        | "ClearIconCache"
        | "OpenOsNotificationSettings"
        | "ClearThumbnailCache"
        | "ResetLocationSortSettings" => {
            settings::handle(name, args, ctx).await
        }
        "OpenPanelTerminal" | "QuickEditFile" | "ResizeTerminal" | "ClosePanelTerminal" => {
            terminal::handle(name, scope, args, ctx).await
        }
        "panel.tab.new" | "panel.tab.close" | "panel.tab.switch" | "panel.terminal.promote" | "ReorderPanelTab" => {
            tabs::handle(name, scope, args, ctx).await
        }
        "EnableSharing"
        | "AddSharedFolder"
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
    let state = ctx.state.lock().await;
    crate::ws::runtime::refresh_thumb_visibility(&state);
}

fn should_record_command(name: &str) -> bool {
    // Notification delivery/history housekeeping is ephemeral UI chatter.
    // Journaling it would persist message payloads and ignore their channels.
    matches!(
        name,
        "ClearIconCache"
            | "ReorderPanelTab"
            | "GitCreateBranch"
            | "GitCheckoutRemoteBranch"
            | "AddSharedFolder"
            | "InitGitRepository"
            | "StartFileOp"
            | "RenameFile"
            | "MakeDir"
            | "TouchFile"
            | "SetPermissions"
            | "WriteExternalClipboard"
            | "PasteExternalClipboard"
            | "ViewFile"
            | "EditFile"
            | "OpenFile"
            | "KillProcess"
            | "OpenTerminal"
            | "EjectSource"
    )
}

#[cfg(test)]
mod notification_journal_tests {
    use super::should_record_command;

    #[test]
    fn notifications_housekeeping_does_not_persist_notification_payloads_in_the_journal() {
        assert!(!should_record_command("PublishNotification"));
        assert!(!should_record_command("ClearNotifications"));
        assert!(should_record_command("WriteExternalClipboard"));
        assert!(should_record_command("ClearIconCache"));
    }
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
