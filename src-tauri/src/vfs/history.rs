use std::any::Any;
use std::io::{Read, Write};

use crate::vfs::{DirChild, VfsDriver, VfsEntry, VfsError, VfsKind};
use crate::ws::protocol::{FilterSpec, SortSpec};

pub struct HistoryDriver {
    entries: Vec<String>,
}

impl HistoryDriver {
    pub fn new(entries: Vec<String>) -> Self {
        Self { entries }
    }
}

fn last_component(path: &str) -> String {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

impl VfsDriver for HistoryDriver {
    fn is_local(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn list(
        &self,
        _path: &str,
        _sort: &SortSpec,
        filter: Option<&FilterSpec>,
        _show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        let entries: Vec<VfsEntry> = self
            .entries
            .iter()
            .filter(|path| {
                if let Some(f) = filter {
                    let val = f.value.to_lowercase();
                    val.split('|').any(|part| {
                        let part = part.trim();
                        let name = last_component(path).to_lowercase();
                        name.contains(part) || path.to_lowercase().contains(part)
                    })
                } else {
                    true
                }
            })
            .map(|path| VfsEntry {
                name: last_component(path),
                kind: VfsKind::Directory,
                target_kind: None,
                path_hint: None,
                size: None,
                mtime: None,
                btime: None,
                permissions: None,
                icon_name: Some("folder".to_string()),
                cc: Some("cc-dir".to_string()),
                source_path: Some(path.clone()),
            })
            .collect();

        Ok(entries)
    }

    fn resolve_child(&self, _base: &str, name: &str) -> String {
        self.entries
            .iter()
            .find(|p| last_component(p) == name)
            .cloned()
            .unwrap_or_else(|| name.to_string())
    }

    fn parent_path(&self, _path: &str) -> Option<String> {
        None
    }

    fn open_read(&self, _path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        Err(VfsError::NotSupported("history source is read-only".into()))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(VfsError::NotSupported("history source is read-only".into()))
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported("history source is read-only".into()))
    }

    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported("history source is read-only".into()))
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported("history source is read-only".into()))
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
        Err(VfsError::NotSupported("history source is read-only".into()))
    }
}
