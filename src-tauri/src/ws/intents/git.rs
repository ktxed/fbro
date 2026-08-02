use serde_json::Value;

use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{rescan_panel, selected_names_for, IntentContext};

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = args.get("panelId").and_then(Value::as_str).unwrap_or("").to_string();
    let (source_id, repository) = panel_repo(&panel_id, ctx).await;

    // Read-only log / commit show: no journal, no panel rescan.
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
        match crate::git::commit_show(&root, &hash) {
            Ok(changes) => {
                let _ = ctx
                    .tx
                    .send(ServerMessage::GitCommitShowReady {
                        timestamp: now(),
                        panel_id,
                        request_id,
                        hash,
                        changes,
                    })
                    .await;
            }
            Err(message) => send_error(ctx, "GIT_COMMIT_SHOW_FAILED", &message).await,
        }
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
        send_error(ctx, "GIT_UNAVAILABLE", message).await;
        return;
    };
    let result = match name {
        "GitStagePaths" => run_paths(&root, "add", args, false),
        "GitUnstagePaths" => run_paths(&root, "restore", args, true),
        "GitRollbackPaths" => rollback_paths(&root, args),
        "GitCommit" => {
            let message = args.get("message").and_then(Value::as_str).unwrap_or("").trim();
            if message.is_empty() { Err("A commit message is required.".to_string()) }
            else { crate::git::run(&root, &["commit".into(), "-m".into(), message.into()]) }
        }
        "GitSwitchBranch" => guarded(clean, || {
            let branch = args.get("branch").and_then(Value::as_str).unwrap_or("").trim();
            if branch.is_empty() { Err("A branch is required.".to_string()) }
            else { crate::git::run(&root, &["switch".into(), branch.into()]) }
        }),
        "GitPull" => guarded(clean, || crate::git::run(&root, &["pull".into()])),
        "GitPush" => crate::git::run(&root, &["push".into()]),
        "GitSetUpstream" => guarded(clean, || {
            let branch = args.get("branch").and_then(Value::as_str).unwrap_or("").trim();
            if branch.is_empty() { Err("A local branch is required to set an upstream.".to_string()) }
            else { crate::git::run(&root, &["push".into(), "--set-upstream".into(), "origin".into(), branch.into()]) }
        }),
        "GitFetch" => crate::git::run(&root, &["fetch".into()]),
        "GitRefresh" => Ok(()),
        _ => Ok(()),
    };
    match result {
        Ok(()) => {
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
            ).await
        }
        Err(message) => {
            if let Some(operation) = operation.as_ref() {
                crate::journal::record_git_result(operation, "failed", Some(&message));
            }
            send_error(ctx, "GIT_ACTION_FAILED", &message).await
        }
    }
}

fn rollback_paths(root: &str, args: &Value) -> Result<(), String> {
    let paths = args.get("paths").and_then(Value::as_array).ok_or("No paths were provided.")?;
    let paths = paths.iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
    crate::git::rollback_paths(root, &paths)
}

fn guarded(clean: bool, action: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    if clean { action() } else { Err("This action requires no staged changes. Commit or unstage them before continuing.".to_string()) }
}

fn run_paths(root: &str, action: &str, args: &Value, staged: bool) -> Result<(), String> {
    let paths = args.get("paths").and_then(Value::as_array).ok_or("No paths were provided.")?;
    let mut command = if staged { vec![action.into(), "--staged".into(), "--".into()] } else { vec![action.into(), "--".into()] };
    command.extend(paths.iter().filter_map(Value::as_str).map(str::to_string));
    if command.last().is_some_and(|s| s == "--") { return Err("No paths were provided.".into()); }
    crate::git::run(root, &command)
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
    let _ = ctx.tx.send(ServerMessage::Error { timestamp: now(), code: code.into(), message: message.into(), recoverable: true, extra: None }).await;
}
