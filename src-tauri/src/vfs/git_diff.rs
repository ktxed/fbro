use std::any::Any;
use std::io::{Cursor, Read, Write};

use crate::vfs::{DirChild, VfsDriver, VfsEntry, VfsError};
use crate::ws::protocol::{FilterSpec, SortSpec};

/// Ephemeral read-only storage for the two sides of a Git comparison.
///
/// It is deliberately not registered as an application source: the compare
/// request owns the driver until it has produced the viewer result.
pub struct GitDiffSnapshotDriver {
    head_path: String,
    head: Vec<u8>,
    worktree_path: String,
    worktree: Vec<u8>,
}

impl GitDiffSnapshotDriver {
    pub fn new(head_path: String, head: Vec<u8>, worktree_path: String, worktree: Vec<u8>) -> Self {
        Self {
            head_path: format!("HEAD:{head_path}"),
            head,
            worktree_path: format!("Working tree:{worktree_path}"),
            worktree,
        }
    }

    pub fn head_path(&self) -> &str {
        &self.head_path
    }

    pub fn worktree_path(&self) -> &str {
        &self.worktree_path
    }

    fn content(&self, path: &str) -> Result<&[u8], VfsError> {
        if path == self.head_path {
            Ok(&self.head)
        } else if path == self.worktree_path {
            Ok(&self.worktree)
        } else {
            Err(VfsError::NotFound(path.to_string()))
        }
    }

    fn read_only() -> VfsError {
        VfsError::NotSupported("Git diff snapshots are read-only".to_string())
    }
}

impl VfsDriver for GitDiffSnapshotDriver {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn list(
        &self,
        _path: &str,
        _sort: &SortSpec,
        _filter: Option<&FilterSpec>,
        _show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        Err(Self::read_only())
    }

    fn resolve_child(&self, _base: &str, name: &str) -> String {
        name.to_string()
    }

    fn parent_path(&self, _path: &str) -> Option<String> {
        None
    }

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        Ok(Box::new(Cursor::new(self.content(path)?.to_vec())))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(Self::read_only())
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(Self::read_only())
    }

    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(Self::read_only())
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(Self::read_only())
    }

    fn exists(&self, path: &str) -> bool {
        self.content(path).is_ok()
    }

    fn file_size(&self, path: &str) -> Option<u64> {
        self.content(path).ok().map(|content| content.len() as u64)
    }

    fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> {
        Err(Self::read_only())
    }

    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
        let content = self.content(path)?;
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        if start >= content.len() {
            return Ok(Vec::new());
        }
        let end = start.saturating_add(size).min(content.len());
        Ok(content[start..end].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::GitDiffSnapshotDriver;
    use crate::vfs::VfsDriver;

    #[test]
    fn exposes_two_read_only_snapshots() {
        let driver = GitDiffSnapshotDriver::new(
            "old.txt".to_string(),
            b"before".to_vec(),
            "new.txt".to_string(),
            b"after".to_vec(),
        );

        assert_eq!(driver.head_path(), "HEAD:old.txt");
        assert_eq!(driver.worktree_path(), "Working tree:new.txt");
        assert_eq!(driver.read_chunk(driver.head_path(), 1, 3).unwrap(), b"efo");
        assert_eq!(
            driver.read_chunk(driver.worktree_path(), 0, 16).unwrap(),
            b"after"
        );
        assert_eq!(driver.file_size(driver.head_path()), Some(6));
        assert!(!driver.exists("missing"));
        assert!(driver.open_write(driver.head_path()).is_err());
    }
}
