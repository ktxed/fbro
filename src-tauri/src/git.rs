use std::collections::{HashMap, HashSet};
use std::path::{Component, Path};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::ws::protocol::EntrySchema;

#[derive(Clone, Debug, Serialize, Deserialize)]
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

pub fn run(root: &str, args: &[String]) -> Result<(), String> {
    let output = Command::new("git").arg("-C").arg(root).args(args).output().map_err(|e| e.to_string())?;
    if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).trim().to_string()) }
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

pub fn read_diff_content(root: &str, target_path: &str, max_bytes: u64) -> Result<GitDiffContent, String> {
    let repo_path = normalize_repo_path(root, target_path)?;
    let status = inspect(root).ok_or_else(|| "No Git repository is available for this panel.".to_string())?;
    let change = status
        .changes
        .iter()
        .find(|change| change.path == repo_path)
        .ok_or_else(|| "The requested path has no Git changes.".to_string())?;
    let head_path = change.old_path.as_deref().unwrap_or(&change.path).to_string();
    let head = read_head_file(root, &head_path, max_bytes)?;
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

fn read_head_file(root: &str, path: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
    let has_head = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "--quiet", "HEAD"])
        .output()
        .map_err(|error| format!("Git is unavailable: {error}"))?
        .status
        .success();
    if !has_head {
        return Ok(Vec::new());
    }
    let object = format!("HEAD:{path}");
    let size = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["cat-file", "-s", &object])
        .output()
        .map_err(|error| format!("Git is unavailable: {error}"))?;
    if !size.status.success() {
        return Ok(Vec::new());
    }
    let size = String::from_utf8_lossy(&size.stdout).trim().parse::<u64>()
        .map_err(|error| format!("Could not read Git object size: {error}"))?;
    if size > max_bytes {
        return Err(format!("{path} exceeds the compare size limit ({} MiB).", max_bytes / 1024 / 1024));
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("show")
        .arg(object)
        .output()
        .map_err(|error| format!("Git is unavailable: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        // A path absent from HEAD is the expected baseline for new files.
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{has_no_staged_changes, read_diff_content, rollback_paths, status_counts, GitChange};

    fn change(index_status: &str, worktree_status: &str) -> GitChange {
        GitChange { path: "file".into(), old_path: None, index_status: index_status.into(), worktree_status: worktree_status.into() }
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
    fn git_diff_reads_head_and_worktree_content() {
        with_test_repo(|repo| {
            fs::write(repo.join("tracked.txt"), "working\n").unwrap();

            let diff = read_diff_content(repo.to_str().unwrap(), &repo.join("tracked.txt").to_string_lossy(), 1024).unwrap();

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

            let added = read_diff_content(repo.to_str().unwrap(), "new.txt", 1024).unwrap();
            assert!(added.head.is_empty());
            assert_eq!(added.worktree, b"new\n");

            let deleted = read_diff_content(repo.to_str().unwrap(), "removed.txt", 1024).unwrap();
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

        let diff = read_diff_content(repo.to_str().unwrap(), "new.txt", 1024).unwrap();

        assert!(diff.head.is_empty());
        assert_eq!(diff.worktree, b"new\n");
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn git_diff_uses_rename_source_from_head() {
        with_test_repo(|repo| {
            fs::rename(repo.join("old.txt"), repo.join("renamed.txt")).unwrap();
            git(repo, &["add", "-A"]);

            let diff = read_diff_content(repo.to_str().unwrap(), "renamed.txt", 1024).unwrap();

            assert_eq!(diff.head_path, "old.txt");
            assert_eq!(diff.worktree_path, "renamed.txt");
            assert_eq!(diff.head, b"old\n");
            assert_eq!(diff.worktree, b"old\n");
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

    fn with_test_repo(test: impl FnOnce(&Path)) {
        let repo = temp_repo_path();
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "fbro-tests@example.invalid"]);
        git(&repo, &["config", "user.name", "fbro tests"]);
        fs::write(repo.join("tracked.txt"), "base\n").unwrap();
        fs::write(repo.join("removed.txt"), "remove\n").unwrap();
        fs::write(repo.join("old.txt"), "old\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "initial"]);
        test(&repo);
        fs::remove_dir_all(repo).unwrap();
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
