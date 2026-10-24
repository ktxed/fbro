use std::path::{Component, Path};
use std::sync::Arc;

use serde_json::Value;

use crate::state::{FileClipboard, PanelStatus, TabKind};
use crate::vfs::git_blob::{GitBlobDriver, CLIPBOARD_SOURCE_ID};
use crate::ws::protocol::{now, FileOpFileSchema, ServerMessage};
use crate::ws::runtime::{quiet_rescan_panel, rescan_panel, selected_names_for, IntentContext};

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    if name == "InitGitRepository" {
        init_repository(args, ctx).await;
        return;
    }
    if name == "RefreshGitStatus" {
        refresh_git_status(args, ctx).await;
        return;
    }
    if name == "OpenGitReviewRequest" {
        open_review_request(args, ctx).await;
        return;
    }

    let panel_id = args.get("panelId").and_then(Value::as_str).unwrap_or("").to_string();
    let activity_id = args
        .get("activityId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let (source_id, repository) = panel_repo(&panel_id, ctx).await;

    // Read-only log / commit show / raw-file clipboard: no journal, no panel rescan.
    if name == "GitGetLog" {
        let request_id = args
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let Some((root, _)) = repository else {
            send_error(ctx, "GIT_UNAVAILABLE", "No Git repository is available for this panel.").await;
            return;
        };
        let skip = args.get("skip").and_then(Value::as_u64).unwrap_or(0) as usize;
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(100) as usize;
        match crate::git::log(&root, skip, limit) {
            Ok((commits, has_more)) => {
                let _ = ctx
                    .tx
                    .send(ServerMessage::GitLogReady {
                        timestamp: now(),
                        panel_id,
                        request_id,
                        commits,
                        has_more,
                    })
                    .await;
            }
            Err(message) => send_error(ctx, "GIT_LOG_FAILED", &message).await,
        }
        return;
    }
    if name == "GitGetCommitShow" {
        let request_id = args
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let hash = args
            .get("hash")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let Some((root, _)) = repository else {
            send_error(ctx, "GIT_UNAVAILABLE", "No Git repository is available for this panel.").await;
            return;
        };
        match (crate::git::commit_show(&root, &hash), crate::git::commit_message(&root, &hash)) {
            (Ok(changes), Ok(message)) => {
                let _ = ctx
                    .tx
                    .send(ServerMessage::GitCommitShowReady {
                        timestamp: now(),
                        panel_id,
                        request_id,
                        hash,
                        message,
                        changes,
                    })
                    .await;
            }
            (Err(message), _) | (_, Err(message)) => send_error(ctx, "GIT_COMMIT_SHOW_FAILED", &message).await,
        }
        return;
    }
    if name == "GitCopyRawFile" {
        git_copy_raw_file(args, repository, ctx).await;
        return;
    }

    let operation = crate::journal::record_git_intent(
        name,
        args,
        repository.as_ref().map(|(root, _)| root.as_str()),
        source_id.as_deref(),
    );
    let Some((root, clean)) = repository else {
        let message = "No Git repository is available for this panel.";
        if let Some(operation) = operation.as_ref() {
            crate::journal::record_git_result(operation, "failed", Some(message));
        }
        send_action_finished(
            ctx,
            &panel_id,
            name,
            activity_id.as_deref(),
            false,
            Some(message),
            None,
        )
        .await;
        send_error(ctx, "GIT_UNAVAILABLE", message).await;
        return;
    };
    let result: Result<Option<String>, String> = match name {
        "GitStagePaths" => run_paths(&root, "add", args, false).map(|_| None),
        "GitUnstagePaths" => run_paths(&root, "restore", args, true).map(|_| None),
        "GitRollbackPaths" => rollback_paths(&root, args).map(|_| None),
        "GitCommit" => {
            let message = args.get("message").and_then(Value::as_str).unwrap_or("").trim();
            if message.is_empty() { Err("A commit message is required.".to_string()) }
            else { crate::git::run(&root, &["commit".into(), "-m".into(), message.into()]).map(|_| None) }
        }
        "GitCreateBranch" => {
            let branch = super::arg_str(args, "branch").unwrap_or("").to_string();
            let root = root.clone();
            crate::jobs::git::create_branch(root, branch).await
                .map(|_| None)
        },
        "GitSwitchBranch" => {
            let branch = super::arg_str(args, "branch").unwrap_or("").to_string();
            crate::jobs::git::switch_branch(root.clone(), branch).await.map(|_| None)
        },
        "GitCheckoutRemoteBranch" => {
            let remote_ref = super::arg_str(args, "remoteRef").unwrap_or("").to_string();
            let local_branch = super::arg_str(args, "localBranch").map(str::to_string);
            match crate::jobs::git::checkout_remote_branch(root.clone(), remote_ref, local_branch).await {
                Ok(crate::git::RemoteCheckoutOutcome::Switched) => Ok(None),
                Ok(crate::git::RemoteCheckoutOutcome::NameRequired { suggested_branch_name, message }) => {
                    if let Some(operation) = operation.as_ref() {
                        crate::journal::record_git_result(operation, "failed", Some(&message));
                    }
                    let _ = ctx.tx.send(ServerMessage::GitActionFinished {
                        timestamp: now(), panel_id: panel_id.clone(), action: name.into(),
                        activity_id, success: false, message: Some(message), suggested_url: None,
                        code: Some("GIT_LOCAL_BRANCH_NAME_REQUIRED".into()),
                        suggested_branch_name: Some(suggested_branch_name),
                    }).await;
                    return;
                },
                Err(message) => Err(message),
            }
        },
        "GitPull" => guarded(clean, || crate::git::run(&root, &["pull".into()])).map(|_| None),
        "GitPush" => match args.get("forceMode").and_then(Value::as_str).unwrap_or("") {
            "" => crate::git::push(&root, &["push".into()]),
            "lease" => crate::git::push(&root, &["push".into(), "--force-with-lease".into()]),
            "force" => crate::git::push(&root, &["push".into(), "--force".into()]),
            _ => Err("Invalid Git push force mode.".to_string()),
        },
        "GitSetUpstream" => guarded(clean, || {
            let branch = args.get("branch").and_then(Value::as_str).unwrap_or("").trim();
            if branch.is_empty() { Err("A local branch is required to set an upstream.".to_string()) }
            else { crate::git::push(&root, &["push".into(), "--set-upstream".into(), "origin".into(), branch.into()]) }
        }),
        "GitFetch" => crate::git::run(&root, &["fetch".into()]).map(|_| None),
        // Refresh always re-scans local status (below). Also best-effort fetch so
        // ahead/behind stay current; network/remote failures must not block that.
        "GitRefresh" => {
            let _ = crate::git::run(&root, &["fetch".into()]);
            Ok(None)
        }
        _ => Ok(None),
    };
    match result {
        Ok(suggested_url) => {
            if let Some(operation) = operation.as_ref() {
                crate::journal::record_git_result(operation, "completed", None);
            }
            let (cursor_hint, selection_names) = {
                let state = ctx.state.lock().await;
                let cursor_hint = state
                    .panels
                    .get(&panel_id)
                    .and_then(|panel| panel.entries.get(panel.cursor).map(|entry| entry.name.clone()));
                (cursor_hint, selected_names_for(&state, &panel_id))
            };
            rescan_panel(
                &panel_id,
                ctx.state,
                ctx.tx,
                ctx.scan_handles,
                cursor_hint,
                selection_names,
                ctx.dir_size_queue,
                ctx.watcher_cmd_tx,
            )
            .await;
            send_action_finished(ctx, &panel_id, name, activity_id.as_deref(), true, None, suggested_url.as_deref())
                .await;
        }
        Err(message) => {
            if let Some(operation) = operation.as_ref() {
                crate::journal::record_git_result(operation, "failed", Some(&message));
            }
            send_action_finished(
                ctx,
                &panel_id,
                name,
                activity_id.as_deref(),
                false,
                Some(&message),
                None,
            )
            .await;
            send_error(ctx, "GIT_ACTION_FAILED", &message).await
        }
    }
}

fn init_target_in_panel(panel_path: &str, target_path: &str) -> bool {
    let target = Path::new(target_path);
    let panel = Path::new(panel_path);
    !target_path.is_empty()
        && !target.components().any(|c| matches!(c, Component::ParentDir))
        && (target == panel || target.parent() == Some(panel))
}

#[cfg(test)]
mod init_tests {
    use super::*;

    #[test]
    fn remote_checkout_name_request_has_correlated_camel_case_fields() {
        let reply = serde_json::to_value(ServerMessage::GitActionFinished {
            timestamp: 1, panel_id: "left".into(), action: "GitCheckoutRemoteBranch".into(),
            activity_id: Some("checkout-1".into()), success: false,
            message: Some("Choose another name".into()), suggested_url: None,
            code: Some("GIT_LOCAL_BRANCH_NAME_REQUIRED".into()),
            suggested_branch_name: Some("topic".into()),
        }).unwrap();
        assert_eq!(reply["panelId"], "left");
        assert_eq!(reply["activityId"], "checkout-1");
        assert_eq!(reply["code"], "GIT_LOCAL_BRANCH_NAME_REQUIRED");
        assert_eq!(reply["suggestedBranchName"], "topic");
        assert!(reply.get("suggested_branch_name").is_none());
        assert!(reply.get("suggestedUrl").is_none());
    }

    #[test]
    fn initialization_target_is_the_current_folder_or_one_direct_child() {
        assert!(init_target_in_panel("/work/project/", "/work/project"));
        assert!(init_target_in_panel("/work/project", "/work/project/child"));
        assert!(!init_target_in_panel("/work/project", ""));
        assert!(!init_target_in_panel("/work/project", "/work/other"));
        assert!(!init_target_in_panel("/work/project", "/work/project/../other"));
        assert!(!init_target_in_panel("/work/project", "/work/project/child/nested"));
    }

    #[test]
    fn initialization_replies_use_correlated_camel_case_fields() {
        let ok = serde_json::to_value(ServerMessage::InitGitRepositoryOk {
            timestamp: 1, panel_id: "left".into(), request_id: "init-1".into(),
            path: "/work/project".into(), branch: "main".into(),
        }).unwrap();
        assert_eq!(ok["type"], "InitGitRepositoryOk");
        assert_eq!(ok["panelId"], "left");
        assert_eq!(ok["requestId"], "init-1");
        let failed = serde_json::to_value(ServerMessage::InitGitRepositoryFailed {
            timestamp: 1, panel_id: "left".into(), request_id: "init-2".into(),
            code: "GIT_INIT_FAILED".into(), message: "Cannot initialize".into(),
        }).unwrap();
        assert_eq!(failed["type"], "InitGitRepositoryFailed");
        assert_eq!(failed["panelId"], "left");
        assert_eq!(failed["requestId"], "init-2");
        assert_eq!(failed["code"], "GIT_INIT_FAILED");
    }
}

async fn init_repository(args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = super::arg_str(args, "panelId").unwrap_or("").to_string();
    let source_id = super::arg_str(args, "sourceId").unwrap_or("").to_string();
    let target = super::arg_str(args, "targetPath").unwrap_or("").to_string();
    let branch = super::arg_str(args, "branch").unwrap_or("").trim().to_string();
    let request_id = super::arg_str(args, "requestId").unwrap_or("").to_string();
    let operation = crate::journal::record_git_intent(
        "InitGitRepository", args, Some(&target), Some(&source_id),
    );
    let driver = {
        let state = ctx.state.lock().await;
        (|| -> Result<_, String> {
            if request_id.is_empty() {
                return Err("A request id is required.".into());
            }
            let panel = state.panels.get(&panel_id).ok_or("The target panel is no longer available.")?;
            let source = state.sources.get(&source_id).ok_or("The target source is no longer available.")?;
            if panel.source_id != source_id || !init_target_in_panel(&panel.path, &target) {
                return Err("The panel location changed. Reopen Git initialization for the target folder.".into());
            }
            if source.kind != "filesystem" || !source.mounted || source.shadow || source.read_only
                || panel.shadow_source.is_some() || panel.tab_kind != TabKind::Listing
            {
                return Err("Git initialization is available only on writable local filesystem folders.".into());
            }
            state.drivers.get(&source_id).cloned().ok_or("The target filesystem is no longer available.".into())
        })()
    };
    let result = match driver {
        Ok(driver) => crate::jobs::git::init_repository(driver, target.clone(), branch.clone()).await,
        Err(message) => Err(message),
    };
    match result {
        Ok(path) => {
            if let Some(operation) = operation.as_ref() {
                crate::journal::record_git_result(operation, "completed", None);
            }
            let panels = {
                let state = ctx.state.lock().await;
                state.panels.iter().filter(|(_, panel)| {
                    panel.tab_kind == TabKind::Listing
                        && state.drivers.get(&panel.source_id).is_some_and(|driver| driver.is_local())
                        && (Path::new(&panel.path).starts_with(&target)
                            || Path::new(&target).starts_with(&panel.path)
                            || panel.git.as_ref().is_some_and(|repo| Path::new(&target).starts_with(&repo.root)))
                }).map(|(id, panel)| {
                    let cursor = panel.entries.get(panel.cursor).map(|entry| entry.name.clone());
                    (id.clone(), cursor, selected_names_for(&state, id))
                }).collect::<Vec<_>>()
            };
            for (id, cursor, selection) in panels {
                rescan_panel(&id, ctx.state, ctx.tx, ctx.scan_handles, cursor, selection,
                    ctx.dir_size_queue, ctx.watcher_cmd_tx).await;
            }
            let _ = ctx.send_feedback(ServerMessage::InitGitRepositoryOk {
                timestamp: now(), panel_id, request_id, path, branch,
            }).await;
        }
        Err(message) => {
            if let Some(operation) = operation.as_ref() {
                crate::journal::record_git_result(operation, "failed", Some(&message));
            }
            let _ = ctx.tx.send(ServerMessage::InitGitRepositoryFailed {
                timestamp: now(), panel_id, request_id, code: "GIT_INIT_FAILED".into(), message,
            }).await;
        }
    }
}

/// Reconcile cached Git decorations after a meaningful user action. Unlike a
/// filesystem watcher, this checks Git's semantic state and only rescans when
/// the result actually changed.
pub(crate) async fn refresh_git_status(args: &Value, ctx: &mut IntentContext<'_>) {
    let requested_panel = args.get("panelId").and_then(Value::as_str).map(str::to_string);
    let panel_ids = if let Some(panel_id) = requested_panel {
        vec![panel_id]
    } else {
        let state = ctx.state.lock().await;
        state
            .layout_slots
            .iter()
            .filter_map(|slot_id| state.slots.get(slot_id).map(|slot| slot.active_panel_id.clone()))
            .collect()
    };

    for panel_id in panel_ids {
        refresh_panel_git_status(&panel_id, ctx).await;
    }
}

pub(crate) async fn refresh_panel_git_status(panel_id: &str, ctx: &mut IntentContext<'_>) {
    let Some((path, source_id, cached_git)) = ({
        let state = ctx.state.lock().await;
        let panel = state.panels.get(panel_id);
        panel.and_then(|panel| {
            let is_local = state
                .drivers
                .get(&panel.source_id)
                .map(|driver| driver.is_local())
                .unwrap_or(false);
            (is_local
                && panel.tab_kind == TabKind::Listing
                && !matches!(panel.status, PanelStatus::Loading))
            .then(|| (panel.path.clone(), panel.source_id.clone(), panel.git.clone()))
        })
    }) else {
        return;
    };

    let inspect_path = path.clone();
    let current_git = tokio::task::spawn_blocking(move || crate::git::inspect(&inspect_path))
        .await
        .ok()
        .flatten();

    let selection_names = {
        let state = ctx.state.lock().await;
        let Some(panel) = state.panels.get(panel_id) else {
            return;
        };
        if panel.path != path || panel.source_id != source_id || panel.git != cached_git {
            return;
        }
        if panel.git == current_git {
            return;
        }
        selected_names_for(&state, panel_id)
    };

    quiet_rescan_panel(
        panel_id,
        ctx.state,
        ctx.tx,
        ctx.scan_handles,
        selection_names,
        ctx.dir_size_queue,
        ctx.watcher_cmd_tx,
    )
    .await;
}

async fn send_action_finished(
    ctx: &mut IntentContext<'_>,
    panel_id: &str,
    action: &str,
    activity_id: Option<&str>,
    success: bool,
    message: Option<&str>,
    suggested_url: Option<&str>,
) {
    let _ = ctx
        .tx
        .send(ServerMessage::GitActionFinished {
            timestamp: now(),
            panel_id: panel_id.to_string(),
            action: action.to_string(),
            activity_id: activity_id.map(str::to_string),
            success,
            message: message.map(str::to_string),
            suggested_url: suggested_url.map(str::to_string),
            code: None,
            suggested_branch_name: None,
        })
        .await;
}

fn rollback_paths(root: &str, args: &Value) -> Result<(), String> {
    let paths = args.get("paths").and_then(Value::as_array).ok_or("No paths were provided.")?;
    let paths = paths.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
    crate::git::rollback_paths(root, &paths)
}

fn guarded<T>(clean: bool, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    if clean { action() } else { Err("This action requires no staged changes. Commit or unstage them before continuing.".to_string()) }
}

async fn open_review_request(args: &Value, ctx: &IntentContext<'_>) {
    let url = args.get("url").and_then(Value::as_str).unwrap_or("").trim();
    let parsed = tauri::Url::parse(url).ok();
    let valid = url.len() <= 4096
        && parsed.as_ref().is_some_and(|url| {
            matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
        });
    if !valid {
        send_error(ctx, "GIT_REVIEW_URL_INVALID", "The suggested review link is invalid.").await;
        return;
    }
    let platform = {
        let state = ctx.state.lock().await;
        Arc::clone(&state.platform)
    };
    if let Err(message) = platform.open_url(url) {
        send_error(ctx, "GIT_REVIEW_OPEN_FAILED", &message).await;
    }
}

fn run_paths(root: &str, action: &str, args: &Value, staged: bool) -> Result<(), String> {
    let paths = args.get("paths").and_then(Value::as_array).ok_or("No paths were provided.")?;
    let mut command = if staged { vec![action.into(), "--staged".into(), "--".into()] } else { vec![action.into(), "--".into()] };
    command.extend(paths.iter().filter_map(Value::as_str).map(str::to_string));
    if command.last().is_some_and(|s| s == "--") { return Err("No paths were provided.".into()); }
    crate::git::run(root, &command)
}

async fn git_copy_raw_file(
    args: &Value,
    repository: Option<(String, bool)>,
    ctx: &mut IntentContext<'_>,
) {
    let Some((root, _)) = repository else {
        send_error(ctx, "GIT_UNAVAILABLE", "No Git repository is available for this panel.").await;
        return;
    };
    let commit = args
        .get("commitHash")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let path_arg = args
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if commit.is_empty() || path_arg.is_empty() {
        send_error(ctx, "GIT_COPY_RAW_FAILED", "A commit hash and path are required.").await;
        return;
    }
    let repo_path = match normalize_repo_rel_path(&path_arg) {
        Ok(p) => p,
        Err(message) => {
            send_error(ctx, "GIT_COPY_RAW_FAILED", &message).await;
            return;
        }
    };
    if !GitBlobDriver::blob_exists(&root, &commit, &repo_path) {
        send_error(
            ctx,
            "GIT_COPY_RAW_FAILED",
            &format!("No file content at {commit:.7}:{repo_path} (deleted or missing)."),
        )
        .await;
        return;
    }
    let size = match GitBlobDriver::blob_size(&root, &commit, &repo_path) {
        Ok(s) => s,
        Err(message) => {
            send_error(ctx, "GIT_COPY_RAW_FAILED", &message).await;
            return;
        }
    };
    let driver = GitBlobDriver::single(&root, &commit, &repo_path);
    let name = driver
        .blob_names()
        .next()
        .unwrap_or(repo_path.as_str())
        .to_string();
    let short = if commit.len() >= 7 {
        commit[..7].to_string()
    } else {
        commit.clone()
    };

    {
        let mut state = ctx.state.lock().await;
        state.drivers.insert(
            CLIPBOARD_SOURCE_ID.to_string(),
            Arc::new(driver) as Arc<dyn crate::vfs::VfsDriver>,
        );
        state.file_clipboard = Some(FileClipboard {
            source_id: CLIPBOARD_SOURCE_ID.to_string(),
            files: vec![FileOpFileSchema {
                name: name.clone(),
                size: Some(size),
                kind: "file".into(),
                source_path: Some(name.clone()),
                dest_name: None,
                location: None,
            }],
            label: Some(short.clone()),
        });
    }

    let _ = ctx.send_feedback(ServerMessage::FileClipboardWritten {
            timestamp: now(),
            item_count: 1,
            name,
            label: Some(short),
        })
        .await;
}

fn normalize_repo_rel_path(path: &str) -> Result<String, String> {
    let path = path.trim().trim_start_matches("./");
    if path.is_empty() {
        return Err("The requested Git path is invalid.".into());
    }
    let p = Path::new(path);
    if p.is_absolute()
        || p.components()
            .any(|c| matches!(c, Component::ParentDir | Component::RootDir))
    {
        return Err("The requested Git path is invalid.".into());
    }
    Ok(path.replace('\\', "/"))
}

async fn panel_repo(panel_id: &str, ctx: &IntentContext<'_>) -> (Option<String>, Option<(String, bool)>) {
    let Some((path, source_id, local)) = ({
        let state = ctx.state.lock().await;
        let panel = state.panels.get(panel_id);
        panel.map(|panel| {
            let local = state.drivers.get(&panel.source_id).map(|driver| driver.is_local()).unwrap_or(false);
            (panel.path.clone(), panel.source_id.clone(), local)
        })
    }) else {
        return (None, None);
    };
    let repository = local.then(|| crate::git::inspect(&path)).flatten().map(|status| (status.root, status.clean));
    (Some(source_id), repository)
}

async fn send_error(ctx: &IntentContext<'_>, code: &str, message: &str) {
    let _ = ctx.send_feedback(ServerMessage::Error { timestamp: now(), code: code.into(), message: message.into(), recoverable: true, extra: None }).await;
}
