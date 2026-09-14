use std::env;
use std::path::Path;

pub(crate) fn find_binary_on_path(candidates: &[&str]) -> Option<String> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var).find_map(|dir| {
        candidates
            .iter()
            .find_map(|candidate| probe_dir_for_candidate(&dir, candidate))
    })
}

fn probe_dir_for_candidate(dir: &Path, candidate: &str) -> Option<String> {
    for name in candidate_file_names(candidate) {
        let joined = dir.join(name);
        if joined.is_file() {
            return Some(joined.to_string_lossy().to_string());
        }
    }
    None
}

fn candidate_file_names(candidate: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        if Path::new(candidate)
            .extension()
            .is_some_and(|ext| !ext.is_empty())
        {
            return vec![candidate.to_string()];
        }
        vec![
            candidate.to_string(),
            format!("{candidate}.exe"),
            format!("{candidate}.cmd"),
            format!("{candidate}.bat"),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![candidate.to_string()]
    }
}

#[allow(dead_code)]
pub(crate) fn executable_exists(path: &str) -> bool {
    Path::new(path).is_file()
}

#[allow(dead_code)]
pub(crate) fn resolve_existing_executable(path: &str) -> Option<String> {
    executable_exists(path).then(|| path.to_string())
}

#[cfg(test)]
mod tests;
