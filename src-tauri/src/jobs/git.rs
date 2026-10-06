//! Short, atomic Git mutations use the Git modal's correlated activity lifecycle.
//! Closing the modal backgrounds the activity; a started switch is never aborted.
use std::sync::Arc;

use crate::vfs::VfsDriver;

pub(crate) async fn init_repository(
    driver: Arc<dyn VfsDriver>,
    path: String,
    branch: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        if !driver.is_local() {
            return Err("Git initialization is available only on local filesystem folders.".into());
        }
        let native_path = driver.local_path(&path)
            .ok_or("This source does not expose a local filesystem folder.")?;
        crate::git::init_repository(&native_path, &branch)
    })
    .await
    .map_err(|e| format!("Git initialization job failed: {e}"))?
}

pub(crate) async fn create_branch(root: String, branch: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || crate::git::create_branch(&root, &branch))
        .await
        .map_err(|e| format!("Branch creation job failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::git_blob::GitBlobDriver;

    #[tokio::test]
    async fn init_repository_rejects_virtual_sources_before_io() {
        let driver = Arc::new(GitBlobDriver::single("missing", "HEAD", "file.txt"));
        let error = init_repository(driver, "missing".into(), "main".into()).await.unwrap_err();
        assert!(error.contains("only on local filesystem"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn init_repository_supports_spy_local_paths() {
        use crate::vfs::{local::LocalFsDriver, spy::SpyFsDriver};
        let path = std::env::temp_dir().join(format!("fbro-git-spy-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
        }
        let _cleanup = Cleanup(path.clone());
        let driver = Arc::new(SpyFsDriver::new(Arc::new(LocalFsDriver), "missing-spy-socket".into()));
        let initialized = init_repository(driver, path.to_string_lossy().into_owned(), "main".into()).await.unwrap();
        assert_eq!(crate::git::inspect(&initialized).unwrap().branch, "main");
    }
}
