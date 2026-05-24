use std::any::Any;
use std::io::{Read, Write};

use crate::bookmarks::BookmarkEntry;
use crate::vfs::{DirChild, VfsDriver, VfsEntry, VfsError, VfsKind};
use crate::ws::protocol::{FilterSpec, SortSpec};

pub struct BookmarksDriver {
    entries: Vec<BookmarkEntry>,
}

impl BookmarksDriver {
    pub fn new(entries: Vec<BookmarkEntry>) -> Self {
        Self { entries }
    }
}

impl VfsDriver for BookmarksDriver {
    fn is_local(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn list(
        &self,
        _path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
        _show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        let mut entries: Vec<VfsEntry> = self
            .entries
            .iter()
            .filter(|bm| {
                if let Some(f) = filter {
                    let val = f.value.to_lowercase();
                    // OR-split on `|`, substring match against name or path
                    val.split('|').any(|part| {
                        let part = part.trim();
                        bm.name.to_lowercase().contains(part)
                            || bm.path.to_lowercase().contains(part)
                    })
                } else {
                    true
                }
            })
            .map(|bm| VfsEntry {
                name: bm.name.clone(),
                kind: VfsKind::Directory,
                size: None,
                mtime: None,
                btime: None,
                permissions: None,
                icon_name: Some("folder".to_string()),
                cc: Some("cc-dir".to_string()),
                source_path: Some(bm.path.clone()),
            })
            .collect();

        // Sort by name
        match (sort.by.as_str(), sort.order.as_str()) {
            ("name", "desc") => {
                entries.sort_by(|a, b| b.name.to_lowercase().cmp(&a.name.to_lowercase()))
            }
            _ => entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        }

        Ok(entries)
    }

    fn resolve_child(&self, _base: &str, name: &str) -> String {
        // Return the real path for the named bookmark so that open_entry
        // gets the correct navigation target.
        self.entries
            .iter()
            .find(|bm| bm.name == name)
            .map(|bm| bm.path.clone())
            .unwrap_or_else(|| name.to_string())
    }

    fn parent_path(&self, _path: &str) -> Option<String> {
        // No parent — navigating up exits the bookmarks shadow source.
        None
    }

    fn open_read(&self, _path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        Err(VfsError::NotSupported(
            "bookmarks source is read-only".into(),
        ))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(VfsError::NotSupported(
            "bookmarks source is read-only".into(),
        ))
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "bookmarks source is read-only".into(),
        ))
    }

    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "bookmarks source is read-only".into(),
        ))
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "bookmarks source is read-only".into(),
        ))
    }

    fn exists(&self, _path: &str) -> bool {
        false
    }

    fn file_size(&self, _path: &str) -> Option<u64> {
        None
    }

    fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> {
        Ok(vec![])
    }

    fn read_chunk(&self, _path: &str, _offset: u64, _size: usize) -> Result<Vec<u8>, VfsError> {
        Err(VfsError::NotSupported(
            "bookmarks source is read-only".into(),
        ))
    }
}
