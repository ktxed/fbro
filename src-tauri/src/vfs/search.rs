use std::any::Any;
use std::io::{Read, Write};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::jobs::search::SearchResultStore;
use crate::vfs::{DirChild, VfsDriver, VfsEntry, VfsError, VfsKind};

/// A read-only virtual filesystem driver backed by search results.
/// Presents results as a flat listing at the root path "/".
/// There is no hierarchy — parent_path always returns None.
pub struct SearchResultsDriver {
    store: Arc<Mutex<SearchResultStore>>,
}

impl SearchResultsDriver {
    pub fn new(store: Arc<Mutex<SearchResultStore>>) -> Self {
        Self { store }
    }

    /// Returns `(full_path, source_id)` for the named entry in the results,
    /// or `None` if not found. Used by intent handlers to redirect I/O to
    /// the real underlying source.
    pub fn resolve_real_info(&self, name: &str) -> Option<(String, String)> {
        let store = futures::executor::block_on(self.store.lock());
        store
            .results
            .iter()
            .find(|r| r.name == name)
            .map(|r| (r.full_path.clone(), r.source_id.clone()))
    }

    /// Same as `resolve_real_info` but matches by full path. Preferred when
    /// the caller already knows the exact result entry (avoids ambiguity when
    /// two hits share a filename).
    pub fn resolve_real_info_by_path(&self, full_path: &str) -> Option<(String, String)> {
        let store = futures::executor::block_on(self.store.lock());
        store
            .results
            .iter()
            .find(|r| r.full_path == full_path)
            .map(|r| (r.full_path.clone(), r.source_id.clone()))
    }
}

impl VfsDriver for SearchResultsDriver {
    fn is_local(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn list(
        &self,
        _path: &str,
        _sort: &crate::ws::protocol::SortSpec,
        _filter: Option<&crate::ws::protocol::FilterSpec>,
        _show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        // Block briefly to get the current snapshot.
        let store = futures::executor::block_on(self.store.lock());
        let entries = store
            .results
            .iter()
            .map(|r| {
                let kind = if r.kind == "directory" {
                    VfsKind::Directory
                } else {
                    VfsKind::File
                };
                VfsEntry {
                    name: r.name.clone(),
                    kind,
                    size: r.size,
                    mtime: None,
                    btime: None,
                    permissions: None,
                    icon_name: None,
                    cc: if r.kind == "directory" {
                        Some("cc-dir".to_string())
                    } else {
                        crate::vfs::color_class_for_entry(&r.name, &VfsKind::File)
                    },
                    source_path: Some(r.full_path.clone()),
                }
            })
            .collect();
        Ok(entries)
    }

    fn resolve_child(&self, _base: &str, name: &str) -> String {
        // Return the actual full_path for this result entry so the panel
        // navigates to the real filesystem location.
        let store = futures::executor::block_on(self.store.lock());
        store
            .results
            .iter()
            .find(|r| r.name == name)
            .map(|r| r.full_path.clone())
            .unwrap_or_else(|| name.to_string())
    }

    fn parent_path(&self, _path: &str) -> Option<String> {
        // No parent — navigating "up" from the results root exits the shadow source.
        None
    }

    fn open_read(&self, _path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        Err(VfsError::NotSupported("read not supported on search results".into()))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(VfsError::NotSupported("write not supported on search results".into()))
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported("create_dir not supported on search results".into()))
    }

    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported("remove not supported on search results".into()))
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported("remove not supported on search results".into()))
    }

    fn exists(&self, _path: &str) -> bool {
        false
    }

    fn file_size(&self, _path: &str) -> Option<u64> {
        None
    }

    fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> {
        Err(VfsError::NotSupported("read_dir_entries not supported on search results".into()))
    }

    fn read_chunk(&self, _path: &str, _offset: u64, _size: usize) -> Result<Vec<u8>, VfsError> {
        Err(VfsError::NotSupported("read_chunk not supported on search results".into()))
    }

    fn try_rename(&self, _src: &str, _dst: &str) -> Result<bool, VfsError> {
        Err(VfsError::NotSupported("rename not supported on search results".into()))
    }
}
