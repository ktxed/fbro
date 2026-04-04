use std::env;
use std::path::Path;

pub(crate) fn find_binary_on_path(candidates: &[&str]) -> Option<String> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var).find_map(|dir| {
        candidates.iter().find_map(|candidate| {
            let joined = dir.join(candidate);
            if joined.is_file() {
                Some(joined.to_string_lossy().to_string())
            } else {
                None
            }
        })
    })
}

pub(crate) fn executable_exists(path: &str) -> bool {
    Path::new(path).is_file()
}

pub(crate) fn resolve_existing_executable(path: &str) -> Option<String> {
    executable_exists(path).then(|| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::{executable_exists, find_binary_on_path, resolve_existing_executable};

    #[test]
    fn missing_binary_on_path_returns_none() {
        assert!(find_binary_on_path(&["fbro-definitely-missing-binary"]).is_none());
    }

    #[test]
    fn executable_resolution_is_safe_for_missing_paths() {
        assert!(!executable_exists("/definitely/missing/fbro-binary"));
        assert!(resolve_existing_executable("/definitely/missing/fbro-binary").is_none());
    }
}
