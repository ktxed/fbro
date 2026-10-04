use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::ws::protocol::EntrySchema;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitChange {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub index_status: String,
    pub worktree_status: String,
}

pub struct GitDiffContent {
    pub head_path: String,
    pub worktree_path: String,
    pub head: Vec<u8>,
    pub worktree: Vec<u8>,
}

/// A small, scan-time summary attached to a listed entry. It deliberately
/// contains only display-ready data so the frontend never needs to aggregate
/// the full repository status while rendering a panel.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitEntryDetails {
    pub changed_count: usize,
    pub sample_paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepoStatus {
    pub root: String,
    pub branch: String,
    pub detached: bool,
    pub clean: bool,
    pub staged_count: usize,
    pub modified_count: usize,
    pub untracked_count: usize,
    pub conflicted_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind_count: Option<usize>,
    pub branches: Vec<String>,
    pub changes: Vec<GitChange>,
}

/// A ref decoration attached to a log entry (`git log` `%D`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GitLogRef {
    pub name: String,
    /// `head` | `branch` | `remote` | `tag`
    pub kind: String,
}

/// One row of `git log` for the repository modal Log tab.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitLogEntry {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
    pub author_name: String,
    pub author_email: String,
    /// ISO-8601 author date from `%aI`.
    pub author_date: String,
    /// Branch / tag / remote decorations pointing at this commit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<GitLogRef>,
}

/// `git` child that does not allocate a console.
///
/// Release builds are a GUI-subsystem process, so a console child such as
/// `git.exe` gets its own window. Those windows steal focus, and the
/// frontend refreshes Git status on focus, which spawns `git` again.
pub(crate) fn git_command() -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW — do not allocate a console for this child.
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        let mut command = Command::new("git");
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        Command::new("git")
    }
}

fn git(path: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = git_command()
        // These commands only inspect repository state. Prevent Git from
        // refreshing the index as an optional optimization: the Git watcher
        // would observe that write and schedule another inspection forever.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|e| format!("Git is unavailable: {e}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

pub fn inspect(path: &str) -> Option<GitRepoStatus> {
    let root = String::from_utf8(git(path, &["rev-parse", "--show-toplevel"]).ok()?)
        .ok()?
        .trim()
        .to_string();
    let branch = String::from_utf8(git(&root, &["branch", "--show-current"]).ok()?)
        .ok()?
        .trim()
        .to_string();
    let has_head = git(&root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
    let detached = branch.is_empty() && has_head;
    let branch = if detached {
        String::from_utf8(git(&root, &["rev-parse", "--short", "HEAD"]).ok()?)
            .ok()?
            .trim()
            .to_string()
    } else if branch.is_empty() {
        String::from_utf8(git(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or_default())
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        branch
    };
    let status = git(&root, &["status", "--porcelain=v1", "-z"]).ok()?;
    let mut changes = Vec::new();
    let mut records = status.split(|b| *b == 0);
    while let Some(record) = records.next() {
        if record.len() < 4 { continue; }
        let index = record[0] as char;
        let worktree = record[1] as char;
        let path = String::from_utf8_lossy(&record[3..]).to_string();
        // Porcelain v1 rename/copy output has a second NUL-separated source path.
        let old_path = if matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C') {
            records.next().map(|value| String::from_utf8_lossy(value).to_string())
        } else {
            None
        };
        changes.push(GitChange { path, old_path, index_status: index.to_string(), worktree_status: worktree.to_string() });
    }
    let (staged_count, modified_count, untracked_count, conflicted_count) = status_counts(&changes);
    let upstream = git(&root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"])
        .ok()
        .and_then(|value| String::from_utf8(value).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let (ahead_count, behind_count) = upstream.as_ref().and_then(|_| {
        let counts = git(&root, &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"]).ok()?;
        let counts = String::from_utf8(counts).ok()?;
        let mut values = counts.split_whitespace().filter_map(|value| value.parse::<usize>().ok());
        Some((values.next()?, values.next()?))
    }).map(|(behind, ahead)| (Some(ahead), Some(behind))).unwrap_or((None, None));
    let branches = String::from_utf8(git(&root, &["for-each-ref", "--format=%(refname:short)", "refs/heads"]).unwrap_or_default())
        .unwrap_or_default().lines().map(str::to_string).collect();
    // fbro considers a work area dirty only when it contains staged changes.
    // Untracked and unstaged changes remain visible, but do not gate Git actions.
    Some(GitRepoStatus { root, branch, detached, clean: has_no_staged_changes(staged_count), staged_count, modified_count, untracked_count, conflicted_count, upstream, ahead_count, behind_count, branches, changes })
}

fn has_no_staged_changes(staged_count: usize) -> bool {
    staged_count == 0
}

fn status_counts(changes: &[GitChange]) -> (usize, usize, usize, usize) {
    let staged_count = changes.iter().filter(|c| c.index_status != " " && c.index_status != "?").count();
    let modified_count = changes.iter().filter(|c| c.worktree_status != " " && c.worktree_status != "?").count();
    let untracked_count = changes.iter().filter(|c| c.index_status == "?" && c.worktree_status == "?").count();
    let conflicted_count = changes.iter().filter(|c| c.index_status == "U" || c.worktree_status == "U" || c.index_status == "A" && c.worktree_status == "A").count();
    (staged_count, modified_count, untracked_count, conflicted_count)
}

pub fn decorate_entries(entries: &mut [EntrySchema], panel_path: &str, status: Option<&GitRepoStatus>) {
    let Some(status) = status else { return };
    let base = Path::new(panel_path).strip_prefix(&status.root).unwrap_or(Path::new(""));
    let prefix = base.to_string_lossy().trim_matches('/').to_string();
    let changes: HashMap<&str, &GitChange> = status.changes.iter().map(|c| (c.path.as_str(), c)).collect();
    for entry in entries {
        let rel = if prefix.is_empty() { entry.name.clone() } else { format!("{prefix}/{}", entry.name) };
        let exact = changes.get(rel.as_str()).copied();
        let nested_prefix = format!("{rel}/");
        let matching: Vec<&GitChange> = status.changes.iter().filter(|change| {
            change.path == rel || (entry.kind == "directory" && change.path.starts_with(&nested_prefix))
        }).collect();
        let nested = matching.iter().any(|change| change.path.starts_with(&nested_prefix));
        entry.git_status = exact.map(status_label).or_else(|| nested.then(|| "changed".to_string()));
        entry.git_details = (!matching.is_empty()).then(|| GitEntryDetails {
            changed_count: matching.len(),
            // For a directory, show paths relative to that directory so the
            // inline sample remains useful even deep in a repository.
            sample_paths: matching.iter().take(10).map(|change| {
                change.path.strip_prefix(&nested_prefix).unwrap_or(&change.path).to_string()
            }).collect(),
        });
    }
}

fn status_label(change: &GitChange) -> String {
    if change.index_status == "?" { return "untracked".into(); }
    if change.index_status == "U" || change.worktree_status == "U" { return "conflicted".into(); }
    if change.index_status == "A" { return "staged-added".into(); }
    if change.index_status == "D" || change.worktree_status == "D" { return "deleted".into(); }
    if change.index_status == "R" { return "renamed".into(); }
    if change.index_status != " " { return "staged".into(); }
    "modified".into()
}

/// Create from current HEAD without overwriting refs or discarding work.
pub fn create_branch(root: &str, branch: &str) -> Result<(), String> {
    let branch = branch.trim();
    if branch.is_empty() || branch.starts_with('-') || branch.contains("@{") {
        return Err("A valid literal branch name is required.".into());
    }
    git(root, &["check-ref-format", "--branch", branch])
        .map_err(|_| format!("Invalid branch name: {branch}"))?;
    run(root, &["switch".into(), "--create".into(), branch.into()])
}

pub fn run(root: &str, args: &[String]) -> Result<(), String> {
    run_with_output(root, args).map(|_| ())
}

/// Run a push while retaining the remote's successful output. Hosting services
/// commonly print a newly-created branch's pull/merge-request URL to stderr.
pub fn push(root: &str, args: &[String]) -> Result<Option<String>, String> {
    let output = run_with_output(root, args)?;
    Ok(find_review_request_url(&output))
}

fn run_with_output(root: &str, args: &[String]) -> Result<std::process::Output, String> {
    let output = git_command()
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn find_review_request_url(output: &std::process::Output) -> Option<String> {
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    find_review_request_url_in_text(&text)
}

fn find_review_request_url_in_text(text: &str) -> Option<String> {
    let url_re = regex::Regex::new(r#"https?://[^\s<>\"'\x00-\x1f\x7f]+"#)
        .expect("valid URL regex");
    let lines = text.lines().collect::<Vec<_>>();

    // The explanatory line and URL are separate in GitHub/GitLab output. Keep
    // the scan close to explicit PR/MR wording so an arbitrary remote message
    // does not turn every URL into an open-browser suggestion.
    for (index, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        let mentions_request = lower.contains("pull request") || lower.contains("merge request");
        if !mentions_request {
            continue;
        }
        for candidate_line in &lines[index..lines.len().min(index + 4)] {
            if let Some(url) = url_re.find(candidate_line) {
                return Some(trim_url_punctuation(url.as_str()).to_string());
            }
        }
    }

    // Some servers print only a recognizable creation URL without prose.
    let recognizable_url = url_re.find_iter(text).find_map(|candidate| {
        let url = trim_url_punctuation(candidate.as_str());
        let lower = url.to_ascii_lowercase();
        (lower.contains("/pull/new/")
            || lower.contains("/pull-requests/new")
            || lower.contains("/merge_requests/new")
            || lower.contains("/pullrequestcreate"))
        .then(|| url.to_string())
    });
    recognizable_url
}

fn trim_url_punctuation(url: &str) -> &str {
    url.trim_end_matches([',', '.', ';', ':', '!', '?', ')', ']', '}'])
}

/// Read a page of commit history for `root`.
///
/// Requests `limit + 1` rows so the caller can set `has_more` without a second
/// round-trip. Unborn repositories (no HEAD) return an empty page.
pub fn log(root: &str, skip: usize, limit: usize) -> Result<(Vec<GitLogEntry>, bool), String> {
    let limit = limit.max(1);
    let has_head = git_command()
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()
        .map_err(|e| format!("Git is unavailable: {e}"))?
        .status
        .success();
    if !has_head {
        return Ok((Vec::new(), false));
    }

    // Fields: hash, short, subject, author name, author email, author date (ISO), decorations.
    // Subject uses %s (single line). Records are newline-separated; fields NUL-separated.
    // %D is comma-separated ref names (no wrapping parens), e.g. "HEAD -> main, origin/main, tag: v1".
    let fetch = limit + 1;
    let skip_arg = format!("--skip={skip}");
    let n_arg = format!("-n{fetch}");
    let format_arg = "--format=%H%x00%h%x00%s%x00%an%x00%ae%x00%aI%x00%D".to_string();
    let output = git(root, &["log", &skip_arg, &n_arg, &format_arg])?;
    let text = String::from_utf8_lossy(&output);
    let mut entries = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\0');
        let Some(hash) = parts.next().filter(|s| !s.is_empty()) else { continue };
        let short_hash = parts.next().unwrap_or("").to_string();
        let subject = parts.next().unwrap_or("").to_string();
        let author_name = parts.next().unwrap_or("").to_string();
        let author_email = parts.next().unwrap_or("").to_string();
        let author_date = parts.next().unwrap_or("").to_string();
        let refs = parse_log_decorations(parts.next().unwrap_or(""));
        entries.push(GitLogEntry {
            hash: hash.to_string(),
            short_hash,
            subject,
            author_name,
            author_email,
            author_date,
            refs,
        });
    }
    let has_more = entries.len() > limit;
    if has_more {
        entries.truncate(limit);
    }
    Ok((entries, has_more))
}

/// Parse `git log` `%D` decoration list into display refs.
fn parse_log_decorations(raw: &str) -> Vec<GitLogRef> {
    raw.split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            if part == "HEAD" {
                return Some(GitLogRef {
                    name: "HEAD".into(),
                    kind: "head".into(),
                });
            }
            if let Some(branch) = part.strip_prefix("HEAD -> ") {
                let name = branch.trim();
                if name.is_empty() {
                    return None;
                }
                return Some(GitLogRef {
                    name: name.into(),
                    kind: "head".into(),
                });
            }
            if let Some(tag) = part.strip_prefix("tag: ") {
                let name = tag.trim();
                if name.is_empty() {
                    return None;
                }
                return Some(GitLogRef {
                    name: name.into(),
                    kind: "tag".into(),
                });
            }
            // remotes look like origin/main; local branches usually have no slash
            // (except nested names). Prefer remote when it looks like remote/…
            let kind = if part.contains('/') { "remote" } else { "branch" };
            Some(GitLogRef {
                name: part.to_string(),
                kind: kind.into(),
            })
        })
        .collect()
}

/// File list for a single commit (name-status vs first parent).
///
/// Reuses [`GitChange`] so the Log expand UI can share row layout with the
/// Changes tab. `index_status` holds the name-status letter (`M`/`A`/`D`/`R`/…);
/// `worktree_status` is always `" "`.
pub fn commit_show(root: &str, hash: &str) -> Result<Vec<GitChange>, String> {
    let hash = hash.trim();
    if hash.is_empty() {
        return Err("A commit hash is required.".into());
    }
    // Resolve to a full object so short hashes and tags still work.
    let resolved = String::from_utf8(git(root, &["rev-parse", "--verify", &format!("{hash}^{{commit}}")])?)
        .map_err(|e| format!("Invalid commit hash encoding: {e}"))?
        .trim()
        .to_string();
    if resolved.is_empty() {
        return Err("Could not resolve the commit hash.".into());
    }

    // --first-parent: merge commits list the first-parent diff only.
    // -z: NUL-separated records (status, path[, path2 for renames/copies]).
    let output = git(
        root,
        &[
            "show",
            "--pretty=format:",
            "--name-status",
            "--first-parent",
            "-z",
            &resolved,
        ],
    )?;
    Ok(parse_name_status_z(&output))
}

pub fn commit_message(root: &str, hash: &str) -> Result<String, String> {
    let hash = hash.trim();
    if hash.is_empty() {
        return Err("A commit hash is required.".into());
    }
    // Resolve to a full object so short hashes and tags still work.
    let resolved = String::from_utf8(git(root, &["rev-parse", "--verify", &format!("{hash}^{{commit}}")])?)
        .map_err(|e| format!("Invalid commit hash encoding: {e}"))?
        .trim()
        .to_string();
    if resolved.is_empty() {
        return Err("Could not resolve the commit hash.".into());
    }

    // %B returns full message (subject + body), %b returns just body; we want body only
    let output = git(root, &["log", "-1", "--pretty=format:%b", &resolved])?;
    String::from_utf8(output)
        .map_err(|e| format!("Invalid commit message encoding: {e}"))
}

/// Parse `git … -z --name-status` output into [`GitChange`] rows.
fn parse_name_status_z(raw: &[u8]) -> Vec<GitChange> {
    let text = String::from_utf8_lossy(raw);
    let mut parts = text.split('\0').filter(|p| !p.is_empty()).peekable();
    let mut changes = Vec::new();
    while let Some(status_field) = parts.next() {
        // Status is like "M", "A", "D", "T", "R100", "C080".
        let letter = status_field.chars().next().unwrap_or('M');
        let Some(path1) = parts.next() else { break };
        let (path, old_path) = if matches!(letter, 'R' | 'C') {
            let path2 = parts.next().unwrap_or("");
            (path2.to_string(), Some(path1.to_string()))
        } else {
            (path1.to_string(), None)
        };
        if path.is_empty() {
            continue;
        }
        changes.push(GitChange {
            path,
            old_path,
            index_status: letter.to_string(),
            worktree_status: " ".into(),
        });
    }
    changes
}

/// Discard the current local state for the requested status entries. Tracked
/// paths return to HEAD; additions and untracked paths are removed instead.
/// Rename source paths are included so rolling back a rename is one action.
pub fn rollback_paths(root: &str, paths: &[String]) -> Result<(), String> {
    let status = inspect(root).ok_or_else(|| "No Git repository is available for this panel.".to_string())?;
    let requested: HashSet<String> = paths
        .iter()
        .map(|path| normalize_repo_path(root, path))
        .collect::<Result<_, _>>()?;
    if requested.is_empty() { return Err("No paths were provided.".into()); }

    let changes: HashMap<&str, &GitChange> = status.changes.iter().map(|change| (change.path.as_str(), change)).collect();
    let mut tracked = HashSet::new();
    let mut added_or_untracked = HashSet::new();
    for path in requested {
        let change = changes.get(path.as_str()).ok_or_else(|| format!("The requested Git path has no changes: {path}"))?;
        if change.index_status == "?" || change.index_status == "A" {
            added_or_untracked.insert(change.path.clone());
        } else {
            tracked.insert(change.path.clone());
            if let Some(old_path) = &change.old_path { tracked.insert(old_path.clone()); }
        }
    }

    if !tracked.is_empty() {
        let mut command = vec!["restore".into(), "--source=HEAD".into(), "--staged".into(), "--worktree".into(), "--".into()];
        command.extend(tracked);
        run(root, &command)?;
    }
    if !added_or_untracked.is_empty() {
        let paths: Vec<String> = added_or_untracked.into_iter().collect();
        let mut unstage = vec!["rm".into(), "--cached".into(), "--ignore-unmatch".into(), "--".into()];
        unstage.extend(paths.iter().cloned());
        run(root, &unstage)?;
        let mut clean = vec!["clean".into(), "-fd".into(), "--".into()];
        clean.extend(paths);
        run(root, &clean)?;
    }
    Ok(())
}

/// Diff content for a path.
///
/// - `commit` is `None`: worktree vs HEAD for a porcelain change (Changes tab).
/// - `commit` is `Some(hash)`: parent vs that commit (`old_path` is the rename
///   source when present). Used by the Log expand “View diff” action.
pub fn read_diff_content(
    root: &str,
    target_path: &str,
    max_bytes: u64,
    commit: Option<&str>,
    old_path: Option<&str>,
) -> Result<GitDiffContent, String> {
    if let Some(commit) = commit {
        return read_commit_diff_content(root, target_path, commit, max_bytes, old_path);
    }

    let repo_path = normalize_repo_path(root, target_path)?;
    let status = inspect(root).ok_or_else(|| "No Git repository is available for this panel.".to_string())?;
    let change = status
        .changes
        .iter()
        .find(|change| change.path == repo_path)
        .ok_or_else(|| "The requested path has no Git changes.".to_string())?;
    let head_path = change.old_path.as_deref().unwrap_or(&change.path).to_string();
    let head = read_git_blob(root, &format!("HEAD:{head_path}"), max_bytes)?;
    let worktree_path = Path::new(root).join(&change.path);
    let worktree = match std::fs::metadata(&worktree_path) {
        Ok(metadata) if metadata.len() > max_bytes => {
            return Err(format!("{} exceeds the compare size limit ({} MiB).", change.path, max_bytes / 1024 / 1024));
        }
        Ok(_) => std::fs::read(&worktree_path).map_err(|error| format!("Could not read {}: {error}", change.path))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(format!("Could not inspect {}: {error}", change.path)),
    };
    Ok(GitDiffContent {
        head_path,
        worktree_path: change.path.clone(),
        head,
        worktree,
    })
}

fn read_commit_diff_content(
    root: &str,
    target_path: &str,
    commit: &str,
    max_bytes: u64,
    old_path: Option<&str>,
) -> Result<GitDiffContent, String> {
    let commit = commit.trim();
    if commit.is_empty() {
        return Err("A commit hash is required.".into());
    }
    let repo_path = normalize_repo_path(root, target_path)?;
    let left_path = old_path
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| normalize_repo_path(root, p))
        .transpose()?
        .unwrap_or_else(|| repo_path.clone());
    // Parent side (empty for root commits / pure adds).
    let head = read_git_blob(root, &format!("{commit}^:{left_path}"), max_bytes)?;
    let worktree = read_git_blob(root, &format!("{commit}:{repo_path}"), max_bytes)?;
    Ok(GitDiffContent {
        head_path: left_path,
        worktree_path: repo_path,
        head,
        worktree,
    })
}

fn normalize_repo_path(root: &str, target_path: &str) -> Result<String, String> {
    let root = Path::new(root);
    let target = Path::new(target_path);
    let relative = if target.is_absolute() {
        target
            .strip_prefix(root)
            .map_err(|_| "The requested path is outside this Git repository.".to_string())?
    } else {
        target
    };
    if relative.as_os_str().is_empty()
        || relative.components().any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return Err("The requested Git path is invalid.".to_string());
    }
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

/// Read a blob via `git show <object>` where `object` is e.g. `HEAD:path` or
/// `abc123^:path`. Missing objects (new file / root parent) return empty bytes.
fn read_git_blob(root: &str, object: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    let size = git_command()
        .arg("-C")
        .arg(root)
        .args(["cat-file", "-s", object])
        .output()
        .map_err(|error| format!("Git is unavailable: {error}"))?;
    if !size.status.success() {
        // Absent blob (new file, root commit parent, deleted path on a side).
        return Ok(Vec::new());
    }
    let size = String::from_utf8_lossy(&size.stdout).trim().parse::<u64>()
        .map_err(|error| format!("Could not read Git object size: {error}"))?;
    if size > max_bytes {
        let label = object.rsplit_once(':').map(|(_, p)| p).unwrap_or(object);
        return Err(format!("{label} exceeds the compare size limit ({} MiB).", max_bytes / 1024 / 1024));
    }
    let output = git_command()
        .arg("-C")
        .arg(root)
        .arg("show")
        .arg(object)
        .output()
        .map_err(|error| format!("Git is unavailable: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{commit_show, find_review_request_url_in_text, has_no_staged_changes, read_diff_content, rollback_paths, status_counts, GitChange};

    fn change(index_status: &str, worktree_status: &str) -> GitChange {
        GitChange { path: "file".into(), old_path: None, index_status: index_status.into(), worktree_status: worktree_status.into() }
    }

    #[test]
    fn finds_github_pull_request_url_on_the_following_line() {
        let output = "remote: Create a pull request for 'topic' on GitHub by visiting:\nremote:   https://github.com/acme/repo/pull/new/topic\n";
        assert_eq!(
            find_review_request_url_in_text(output).as_deref(),
            Some("https://github.com/acme/repo/pull/new/topic")
        );
    }

    #[test]
    fn finds_gitlab_merge_request_url_and_trims_sentence_punctuation() {
        let output = "remote: To create a merge request for topic, visit:\nremote: https://gitlab.example/acme/repo/-/merge_requests/new?merge_request%5Bsource_branch%5D=topic.\x1b[0m\n";
        assert_eq!(
            find_review_request_url_in_text(output).as_deref(),
            Some("https://gitlab.example/acme/repo/-/merge_requests/new?merge_request%5Bsource_branch%5D=topic")
        );
    }

    #[test]
    fn ignores_unrelated_urls_in_push_output() {
        assert_eq!(
            find_review_request_url_in_text("remote: Learn more at https://example.test/docs\n"),
            None
        );
    }

    #[test]
    fn git_log_returns_commits_newest_first_with_has_more() {
        with_test_repo(|repo| {
            fs::write(repo.join("tracked.txt"), "second\n").unwrap();
            git(repo, &["add", "tracked.txt"]);
            git(repo, &["commit", "-qm", "second commit"]);

            let root = repo.to_str().unwrap();
            let (page, has_more) = super::log(root, 0, 1).unwrap();
            assert_eq!(page.len(), 1);
            assert!(has_more);
            assert_eq!(page[0].subject, "second commit");
            assert!(!page[0].hash.is_empty());
            assert!(!page[0].short_hash.is_empty());
            assert_eq!(page[0].author_name, "fbro tests");

            let (all, has_more) = super::log(root, 0, 10).unwrap();
            assert_eq!(all.len(), 2);
            assert!(!has_more);
            assert_eq!(all[0].subject, "second commit");
            assert_eq!(all[1].subject, "initial");

            let (skipped, has_more) = super::log(root, 1, 10).unwrap();
            assert_eq!(skipped.len(), 1);
            assert!(!has_more);
            assert_eq!(skipped[0].subject, "initial");
        });
    }

    #[test]
    fn git_log_includes_branch_and_tag_decorations() {
        with_test_repo(|repo| {
            git(repo, &["branch", "feature"]);
            git(repo, &["tag", "v1.0"]);
            let root = repo.to_str().unwrap();
            let (entries, _) = super::log(root, 0, 10).unwrap();
            let tip = &entries[0];
            let names: Vec<&str> = tip.refs.iter().map(|r| r.name.as_str()).collect();
            assert!(
                names.iter().any(|n| *n == "feature" || n.ends_with("feature")),
                "expected feature branch decoration, got {names:?}"
            );
            assert!(
                names.iter().any(|n| *n == "v1.0"),
                "expected tag decoration, got {names:?}"
            );
            assert!(
                tip.refs.iter().any(|r| r.kind == "tag" && r.name == "v1.0"),
                "tag should be kind=tag: {:?}",
                tip.refs
            );
        });
    }

    #[test]
    fn parse_log_decorations_splits_kinds() {
        let refs = super::parse_log_decorations("HEAD -> main, origin/main, tag: v1.0, feature");
        assert_eq!(
            refs,
            vec![
                super::GitLogRef { name: "main".into(), kind: "head".into() },
                super::GitLogRef { name: "origin/main".into(), kind: "remote".into() },
                super::GitLogRef { name: "v1.0".into(), kind: "tag".into() },
                super::GitLogRef { name: "feature".into(), kind: "branch".into() },
            ]
        );
    }

    #[test]
    fn git_log_empty_for_unborn_repository() {
        let repo = temp_repo_path();
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        let (entries, has_more) = super::log(repo.to_str().unwrap(), 0, 50).unwrap();
        assert!(entries.is_empty());
        assert!(!has_more);
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn only_staged_entries_make_a_work_area_dirty() {
        let untracked = vec![change("?", "?")];
        let unstaged = vec![change(" ", "M")];
        let staged = vec![change("M", " ")];
        let mixed = vec![change("M", " "), change("?", "?")];
        let conflicted = vec![change("U", "U")];

        assert!(has_no_staged_changes(status_counts(&untracked).0));
        assert!(has_no_staged_changes(status_counts(&unstaged).0));
        assert!(!has_no_staged_changes(status_counts(&staged).0));
        assert!(!has_no_staged_changes(status_counts(&mixed).0));
        assert!(!has_no_staged_changes(status_counts(&conflicted).0));
    }

    #[test]
    fn inspect_does_not_write_the_git_index() {
        with_test_repo(|repo| {
            let index = repo.join(".git/index");
            let before = fs::metadata(&index).unwrap().modified().unwrap();

            // Make the cached stat data stale without changing file content.
            // A normal `git status` refreshes and rewrites the index here.
            std::thread::sleep(std::time::Duration::from_millis(25));
            fs::write(repo.join("tracked.txt"), "base\n").unwrap();

            assert!(super::inspect(repo.to_str().unwrap()).is_some());
            let after = fs::metadata(index).unwrap().modified().unwrap();
            assert_eq!(after, before);
        });
    }

    #[test]
    fn inspect_snapshots_change_only_with_semantic_git_state() {
        with_test_repo(|repo| {
            let first = super::inspect(repo.to_str().unwrap()).unwrap();
            let unchanged = super::inspect(repo.to_str().unwrap()).unwrap();
            assert_eq!(unchanged, first);

            fs::write(repo.join("tracked.txt"), "changed\n").unwrap();
            let changed = super::inspect(repo.to_str().unwrap()).unwrap();
            assert_ne!(changed, first);
        });
    }

    #[test]
    fn git_diff_reads_head_and_worktree_content() {
        with_test_repo(|repo| {
            fs::write(repo.join("tracked.txt"), "working\n").unwrap();

            let diff = read_diff_content(
                repo.to_str().unwrap(),
                &repo.join("tracked.txt").to_string_lossy(),
                1024,
                None,
                None,
            )
            .unwrap();

            assert_eq!(diff.head_path, "tracked.txt");
            assert_eq!(diff.worktree_path, "tracked.txt");
            assert_eq!(diff.head, b"base\n");
            assert_eq!(diff.worktree, b"working\n");
        });
    }

    #[test]
    fn git_diff_uses_empty_content_for_added_and_deleted_paths() {
        with_test_repo(|repo| {
            fs::write(repo.join("new.txt"), "new\n").unwrap();
            fs::remove_file(repo.join("removed.txt")).unwrap();

            let added = read_diff_content(repo.to_str().unwrap(), "new.txt", 1024, None, None).unwrap();
            assert!(added.head.is_empty());
            assert_eq!(added.worktree, b"new\n");

            let deleted = read_diff_content(repo.to_str().unwrap(), "removed.txt", 1024, None, None).unwrap();
            assert_eq!(deleted.head, b"remove\n");
            assert!(deleted.worktree.is_empty());
        });
    }

    #[test]
    fn git_diff_supports_an_unborn_repository() {
        let repo = temp_repo_path();
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        fs::write(repo.join("new.txt"), "new\n").unwrap();

        let diff = read_diff_content(repo.to_str().unwrap(), "new.txt", 1024, None, None).unwrap();

        assert!(diff.head.is_empty());
        assert_eq!(diff.worktree, b"new\n");
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn git_diff_uses_rename_source_from_head() {
        with_test_repo(|repo| {
            fs::rename(repo.join("old.txt"), repo.join("renamed.txt")).unwrap();
            git(repo, &["add", "-A"]);

            let diff = read_diff_content(repo.to_str().unwrap(), "renamed.txt", 1024, None, None).unwrap();

            assert_eq!(diff.head_path, "old.txt");
            assert_eq!(diff.worktree_path, "renamed.txt");
            assert_eq!(diff.head, b"old\n");
            assert_eq!(diff.worktree, b"old\n");
        });
    }

    #[test]
    fn commit_show_lists_modified_added_and_deleted_paths() {
        with_test_repo(|repo| {
            fs::write(repo.join("tracked.txt"), "changed\n").unwrap();
            fs::write(repo.join("brand-new.txt"), "hello\n").unwrap();
            fs::remove_file(repo.join("removed.txt")).unwrap();
            git(repo, &["add", "-A"]);
            git(repo, &["commit", "-qm", "mixed changes"]);

            let root = repo.to_str().unwrap();
            let (log, _) = super::log(root, 0, 5).unwrap();
            let tip = &log[0];
            let changes = commit_show(root, &tip.hash).unwrap();
            let by_path: std::collections::HashMap<_, _> =
                changes.iter().map(|c| (c.path.as_str(), c)).collect();

            assert_eq!(by_path["tracked.txt"].index_status, "M");
            assert_eq!(by_path["brand-new.txt"].index_status, "A");
            assert_eq!(by_path["removed.txt"].index_status, "D");
        });
    }

    #[test]
    fn commit_show_includes_rename_with_old_path() {
        with_test_repo(|repo| {
            fs::rename(repo.join("old.txt"), repo.join("renamed.txt")).unwrap();
            git(repo, &["add", "-A"]);
            git(repo, &["commit", "-qm", "rename old"]);

            let root = repo.to_str().unwrap();
            let (log, _) = super::log(root, 0, 5).unwrap();
            let changes = commit_show(root, &log[0].hash).unwrap();
            let renamed = changes.iter().find(|c| c.path == "renamed.txt").expect("rename");
            assert_eq!(renamed.index_status, "R");
            assert_eq!(renamed.old_path.as_deref(), Some("old.txt"));
        });
    }

    #[test]
    fn commit_show_for_root_commit_lists_initial_files() {
        with_test_repo(|repo| {
            let root = repo.to_str().unwrap();
            let (log, _) = super::log(root, 0, 10).unwrap();
            let initial = log.last().unwrap();
            let changes = commit_show(root, &initial.hash).unwrap();
            assert!(changes.iter().any(|c| c.path == "tracked.txt" && c.index_status == "A"));
            assert!(changes.iter().any(|c| c.path == "old.txt" && c.index_status == "A"));
        });
    }

    #[test]
    fn commit_diff_reads_parent_and_commit_blobs() {
        with_test_repo(|repo| {
            fs::write(repo.join("tracked.txt"), "second\n").unwrap();
            git(repo, &["add", "tracked.txt"]);
            git(repo, &["commit", "-qm", "second"]);

            let root = repo.to_str().unwrap();
            let (log, _) = super::log(root, 0, 5).unwrap();
            let tip = &log[0];
            let diff = read_diff_content(root, "tracked.txt", 1024, Some(&tip.hash), None).unwrap();
            assert_eq!(diff.head, b"base\n");
            assert_eq!(diff.worktree, b"second\n");
        });
    }

    #[test]
    fn commit_diff_uses_empty_parent_for_added_file() {
        with_test_repo(|repo| {
            fs::write(repo.join("brand-new.txt"), "hello\n").unwrap();
            git(repo, &["add", "brand-new.txt"]);
            git(repo, &["commit", "-qm", "add file"]);

            let root = repo.to_str().unwrap();
            let (log, _) = super::log(root, 0, 5).unwrap();
            let diff = read_diff_content(root, "brand-new.txt", 1024, Some(&log[0].hash), None).unwrap();
            assert!(diff.head.is_empty());
            assert_eq!(diff.worktree, b"hello\n");
        });
    }

    #[test]
    fn rollback_restores_tracked_and_deleted_paths_and_removes_additions() {
        with_test_repo(|repo| {
            fs::write(repo.join("tracked.txt"), "changed\n").unwrap();
            fs::remove_file(repo.join("removed.txt")).unwrap();
            fs::write(repo.join("new.txt"), "new\n").unwrap();
            fs::write(repo.join("untracked.txt"), "untracked\n").unwrap();
            git(repo, &["add", "tracked.txt", "removed.txt", "new.txt"]);

            rollback_paths(repo.to_str().unwrap(), &[
                "tracked.txt".into(), "removed.txt".into(), "new.txt".into(), "untracked.txt".into(),
            ]).unwrap();

            assert_eq!(fs::read_to_string(repo.join("tracked.txt")).unwrap(), "base\n");
            assert_eq!(fs::read_to_string(repo.join("removed.txt")).unwrap(), "remove\n");
            assert!(!repo.join("new.txt").exists());
            assert!(!repo.join("untracked.txt").exists());
            assert!(super::inspect(repo.to_str().unwrap()).unwrap().changes.is_empty());
        });
    }

    #[test]
    fn rollback_restores_both_sides_of_a_rename() {
        with_test_repo(|repo| {
            fs::rename(repo.join("old.txt"), repo.join("renamed.txt")).unwrap();
            git(repo, &["add", "-A"]);

            rollback_paths(repo.to_str().unwrap(), &["renamed.txt".into()]).unwrap();

            assert_eq!(fs::read_to_string(repo.join("old.txt")).unwrap(), "old\n");
            assert!(!repo.join("renamed.txt").exists());
            assert!(super::inspect(repo.to_str().unwrap()).unwrap().changes.is_empty());
        });
    }

    #[test]
    fn rollback_removes_staged_additions_in_an_unborn_repository() {
        let repo = temp_repo_path();
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        fs::write(repo.join("new.txt"), "new\n").unwrap();
        git(&repo, &["add", "new.txt"]);

        rollback_paths(repo.to_str().unwrap(), &["new.txt".into()]).unwrap();

        assert!(!repo.join("new.txt").exists());
        assert!(super::inspect(repo.to_str().unwrap()).unwrap().changes.is_empty());
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn create_branch_preserves_work_and_switches() {
        with_test_repo(|repo| {
            fs::write(repo.join("tracked.txt"), "staged\n").unwrap();
            git(repo, &["add", "tracked.txt"]);
            fs::write(repo.join("tracked.txt"), "unstaged\n").unwrap();
            fs::write(repo.join("new.txt"), "untracked\n").unwrap();
            let root = repo.to_str().unwrap();
            let before = super::git(root, &["status", "--porcelain"]).unwrap();
            let head = super::git(root, &["rev-parse", "HEAD"]).unwrap();
            super::create_branch(root, "feature/new-work").unwrap();
            assert_eq!(super::inspect(root).unwrap().branch, "feature/new-work");
            assert_eq!(super::git(root, &["rev-parse", "HEAD"]).unwrap(), head);
            assert_eq!(super::git(root, &["status", "--porcelain"]).unwrap(), before);
            assert_eq!(fs::read_to_string(repo.join("tracked.txt")).unwrap(), "unstaged\n");
            assert_eq!(fs::read_to_string(repo.join("new.txt")).unwrap(), "untracked\n");
        });
    }

    #[test]
    fn create_branch_rejects_invalid_and_existing_names_without_mutation() {
        with_test_repo(|repo| {
            git(repo, &["branch", "existing"]);
            let root = repo.to_str().unwrap();
            let before = super::git(root, &["show-ref"]).unwrap();
            let head = super::git(root, &["symbolic-ref", "HEAD"]).unwrap();
            for name in ["", "-oops", "@{-1}", "bad name", "bad..name", "HEAD", "existing"] {
                assert!(super::create_branch(root, name).is_err(), "accepted {name}");
                assert_eq!(super::git(root, &["show-ref"]).unwrap(), before);
                assert_eq!(super::git(root, &["symbolic-ref", "HEAD"]).unwrap(), head);
            }
        });
    }

    #[test]
    fn create_branch_git_failure_leaves_head_and_refs_unchanged() {
        with_test_repo(|repo| {
            let root = repo.to_str().unwrap();
            let head = super::git(root, &["symbolic-ref", "HEAD"]).unwrap();
            let before = super::git(root, &["show-ref"]).unwrap();
            let lock = repo.join(".git/refs/heads/blocked.lock");
            fs::write(&lock, "locked").unwrap();
            assert!(super::create_branch(root, "blocked").is_err());
            assert_eq!(super::git(root, &["show-ref"]).unwrap(), before);
            assert_eq!(super::git(root, &["symbolic-ref", "HEAD"]).unwrap(), head);
        });
    }

    #[test]
    fn create_branch_from_detached_head() {
        with_test_repo(|repo| {
            git(repo, &["switch", "--detach"]);
            let root = repo.to_str().unwrap();
            let head = super::git(root, &["rev-parse", "HEAD"]).unwrap();
            super::create_branch(root, "saved-detached-work").unwrap();
            assert_eq!(super::inspect(root).unwrap().branch, "saved-detached-work");
            assert_eq!(super::git(root, &["rev-parse", "HEAD"]).unwrap(), head);
        });
    }

    fn with_test_repo(test: impl FnOnce(&Path)) {
        let repo = temp_repo_path();
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        // Keep fixture bytes identical on hosts with core.autocrlf enabled.
        git(&repo, &["config", "core.autocrlf", "false"]);
        git(&repo, &["config", "user.email", "fbro-tests@example.invalid"]);
        git(&repo, &["config", "user.name", "fbro tests"]);
        fs::write(repo.join("tracked.txt"), "base\n").unwrap();
        fs::write(repo.join("removed.txt"), "remove\n").unwrap();
        fs::write(repo.join("old.txt"), "old\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "initial"]);
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
        }
        let _cleanup = Cleanup(repo.clone());
        test(&repo);
    }

    fn temp_repo_path() -> PathBuf {
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("fbro-git-test-{}-{stamp}", std::process::id()))
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git").arg("-C").arg(repo).args(args).output().unwrap();
        assert!(output.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&output.stderr));
    }
}
