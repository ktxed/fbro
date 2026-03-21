use std::fs;
use std::path::Path;

use super::{format_mtime, VfsEntry, VfsError, VfsKind};
use crate::ws::protocol::{FilterSpec, SortSpec};

pub struct LocalFsDriver;

impl LocalFsDriver {
    pub fn list(
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        let dir = fs::read_dir(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                VfsError::PermissionDenied(e.to_string())
            } else if e.kind() == std::io::ErrorKind::NotFound {
                VfsError::NotFound(path.to_string())
            } else {
                VfsError::IOError(e.to_string())
            }
        })?;

        let mut entries: Vec<VfsEntry> = Vec::new();

        for entry_result in dir {
            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.file_name().to_string_lossy().to_string();

            // Apply filter
            if let Some(f) = filter {
                match f.mode.as_str() {
                    "prefix" => {
                        if !name.starts_with(&f.value) {
                            continue;
                        }
                    }
                    _ => {}
                }
            }

            let meta = entry.metadata().ok();
            let (kind, size, mtime) = if let Some(m) = meta {
                let kind = if m.is_symlink() {
                    VfsKind::Symlink
                } else if m.is_dir() {
                    VfsKind::Directory
                } else if m.is_file() {
                    VfsKind::File
                } else {
                    VfsKind::Other
                };
                let size = if m.is_file() { Some(m.len()) } else { None };
                let mtime = m.modified().ok().and_then(format_mtime);
                (kind, size, mtime)
            } else {
                (VfsKind::Other, None, None)
            };

            entries.push(VfsEntry { name, kind, size, mtime });
        }

        // Sort: directories first, then by the sort spec
        entries.sort_by(|a, b| {
            // Directories always come first
            let a_dir = a.kind == VfsKind::Directory;
            let b_dir = b.kind == VfsKind::Directory;
            if a_dir != b_dir {
                return b_dir.cmp(&a_dir);
            }

            let ord = match sort.by.as_str() {
                "size" => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
                "mtime" => a.mtime.cmp(&b.mtime),
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };

            if sort.order == "desc" { ord.reverse() } else { ord }
        });

        Ok(entries)
    }

    pub fn parent_path(path: &str) -> Option<String> {
        Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
    }

    pub fn resolve_child(base: &str, name: &str) -> String {
        Path::new(base).join(name).to_string_lossy().to_string()
    }
}
