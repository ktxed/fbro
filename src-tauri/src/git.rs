use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use serde::Serialize;

use crate::ws::protocol::EntrySchema;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitChange {
    pub path: String,
    pub index_status: String,
    pub worktree_status: String,
}

#[derive(Clone, Debug, Serialize)]
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

fn git(path: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
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
    let detached = branch.is_empty();
    let branch = if detached {
        String::from_utf8(git(&root, &["rev-parse", "--short", "HEAD"]).ok()?)
            .ok()?
            .trim()
            .to_string()
    } else { branch };
    let status = git(&root, &["status", "--porcelain=v1", "-z"]).ok()?;
    let mut changes = Vec::new();
    let mut records = status.split(|b| *b == 0);
    while let Some(record) = records.next() {
        if record.len() < 4 { continue; }
        let index = record[0] as char;
        let worktree = record[1] as char;
        let path = String::from_utf8_lossy(&record[3..]).to_string();
        // Porcelain v1 rename/copy output has a second NUL-separated source path.
        if index == 'R' || index == 'C' { let _ = records.next(); }
        changes.push(GitChange { path, index_status: index.to_string(), worktree_status: worktree.to_string() });
    }
    let staged_count = changes.iter().filter(|c| c.index_status != " " && c.index_status != "?").count();
    let modified_count = changes.iter().filter(|c| c.worktree_status != " " && c.worktree_status != "?").count();
    let untracked_count = changes.iter().filter(|c| c.index_status == "?" && c.worktree_status == "?").count();
    let conflicted_count = changes.iter().filter(|c| c.index_status == "U" || c.worktree_status == "U" || c.index_status == "A" && c.worktree_status == "A").count();
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
    Some(GitRepoStatus { root, branch, detached, clean: changes.is_empty(), staged_count, modified_count, untracked_count, conflicted_count, upstream, ahead_count, behind_count, branches, changes })
}

pub fn decorate_entries(entries: &mut [EntrySchema], panel_path: &str, status: Option<&GitRepoStatus>) {
    let Some(status) = status else { return };
    let base = Path::new(panel_path).strip_prefix(&status.root).unwrap_or(Path::new(""));
    let prefix = base.to_string_lossy().trim_matches('/').to_string();
    let changes: HashMap<&str, &GitChange> = status.changes.iter().map(|c| (c.path.as_str(), c)).collect();
    for entry in entries {
        let rel = if prefix.is_empty() { entry.name.clone() } else { format!("{prefix}/{}", entry.name) };
        let exact = changes.get(rel.as_str()).copied();
        let nested = entry.kind == "directory" && status.changes.iter().any(|c| c.path.starts_with(&(rel.clone() + "/")));
        entry.git_status = exact.map(status_label).or_else(|| nested.then(|| "changed".to_string()));
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

pub fn run(root: &str, args: &[String]) -> Result<(), String> {
    let output = Command::new("git").arg("-C").arg(root).args(args).output().map_err(|e| e.to_string())?;
    if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).trim().to_string()) }
}
