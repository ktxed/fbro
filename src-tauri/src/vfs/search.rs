use std::any::Any;
use std::cmp::Ordering;
use std::io::{Read, Write};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::jobs::search::SearchResultStore;
use crate::vfs::{icon_name_for_entry, DirChild, VfsDriver, VfsEntry, VfsError, VfsKind};
use crate::ws::protocol::SortSpec;

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

fn entry_extension(entry: &VfsEntry) -> String {
    entry
        .name
        .rfind('.')
        .map(|i| entry.name[i + 1..].to_lowercase())
        .unwrap_or_default()
}

fn sort_entries(entries: &mut [VfsEntry], sort: &SortSpec) {
    entries.sort_by(|a, b| {
        let a_dir = a.kind == VfsKind::Directory;
        let b_dir = b.kind == VfsKind::Directory;
        if a_dir != b_dir {
            return b_dir.cmp(&a_dir);
        }

        let primary = match sort.by.as_str() {
            "size" => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
            "date" => a.mtime.cmp(&b.mtime),
            "kind" => entry_extension(a).cmp(&entry_extension(b)),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        let primary = if sort.order == "desc" {
            primary.reverse()
        } else {
            primary
        };
        if primary != Ordering::Equal {
            return primary;
        }

        let path_order = a
            .source_path
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .cmp(&b.source_path.as_deref().unwrap_or_default().to_lowercase());
        if path_order != Ordering::Equal {
            return path_order;
        }

        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    });
}

impl VfsDriver for SearchResultsDriver {
    fn is_local(&self) -> bool {
        false
    }

    fn supports_thumbnails(&self) -> bool {
        true
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn list(
        &self,
        _path: &str,
        sort: &SortSpec,
        _filter: Option<&crate::ws::protocol::FilterSpec>,
        _show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        // Block briefly to get the current snapshot.
        let store = futures::executor::block_on(self.store.lock());
        let mut entries: Vec<VfsEntry> = store
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
                    kind: kind.clone(),
                    size: r.size,
                    mtime: r.mtime.clone(),
                    btime: None,
                    permissions: None,
                    icon_name: icon_name_for_entry(&r.name, &kind),
                    cc: if r.kind == "directory" {
                        Some("cc-dir".to_string())
                    } else {
                        crate::vfs::color_class_for_entry(&r.name, &VfsKind::File)
                    },
                    source_path: Some(r.full_path.clone()),
                }
            })
            .collect();
        sort_entries(&mut entries, sort);
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
        Err(VfsError::NotSupported(
            "read not supported on search results".into(),
        ))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(VfsError::NotSupported(
            "write not supported on search results".into(),
        ))
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "create_dir not supported on search results".into(),
        ))
    }

    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "remove not supported on search results".into(),
        ))
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "remove not supported on search results".into(),
        ))
    }

    fn exists(&self, _path: &str) -> bool {
        false
    }

    fn file_size(&self, _path: &str) -> Option<u64> {
        None
    }

    fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> {
        Err(VfsError::NotSupported(
            "read_dir_entries not supported on search results".into(),
        ))
    }

    fn read_chunk(&self, _path: &str, _offset: u64, _size: usize) -> Result<Vec<u8>, VfsError> {
        Err(VfsError::NotSupported(
            "read_chunk not supported on search results".into(),
        ))
    }

    fn try_rename(&self, _src: &str, _dst: &str) -> Result<bool, VfsError> {
        Err(VfsError::NotSupported(
            "rename not supported on search results".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::search::SearchResultEntry;

    fn result(
        name: &str,
        kind: &str,
        size: Option<u64>,
        mtime: Option<&str>,
        parent_path: &str,
    ) -> SearchResultEntry {
        SearchResultEntry {
            name: name.to_string(),
            kind: kind.to_string(),
            size,
            mtime: mtime.map(str::to_string),
            parent_path: parent_path.to_string(),
            full_path: format!("{}/{}", parent_path.trim_end_matches('/'), name),
            source_id: "vol-test".to_string(),
        }
    }

    fn driver(entries: Vec<SearchResultEntry>) -> SearchResultsDriver {
        SearchResultsDriver::new(Arc::new(Mutex::new(SearchResultStore {
            results: entries,
            done: true,
        })))
    }

    fn sort(by: &str, order: &str) -> SortSpec {
        SortSpec {
            by: by.to_string(),
            order: order.to_string(),
        }
    }

    fn names(entries: &[VfsEntry]) -> Vec<String> {
        entries.iter().map(|entry| entry.name.clone()).collect()
    }

    fn source_paths(entries: &[VfsEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| entry.source_path.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn sorts_names_and_uses_full_path_tiebreaker() {
        let driver = driver(vec![
            result("same.txt", "file", Some(1), Some("30"), "/z"),
            result("alpha.txt", "file", Some(1), Some("10"), "/b"),
            result("same.txt", "file", Some(1), Some("20"), "/a"),
        ]);

        let entries = driver.list("/", &sort("name", "asc"), None, false).unwrap();
        assert_eq!(
            source_paths(&entries),
            vec!["/b/alpha.txt", "/a/same.txt", "/z/same.txt"]
        );

        let entries = driver
            .list("/", &sort("name", "desc"), None, false)
            .unwrap();
        assert_eq!(
            source_paths(&entries),
            vec!["/a/same.txt", "/z/same.txt", "/b/alpha.txt"]
        );
    }

    #[test]
    fn sorts_by_size_ascending() {
        let driver = driver(vec![
            result("large.bin", "file", Some(300), None, "/r"),
            result("small.bin", "file", Some(10), None, "/r"),
            result("missing.bin", "file", None, None, "/r"),
        ]);

        let entries = driver.list("/", &sort("size", "asc"), None, false).unwrap();
        assert_eq!(
            names(&entries),
            vec!["missing.bin", "small.bin", "large.bin"]
        );
    }

    #[test]
    fn sorts_by_date_descending() {
        let driver = driver(vec![
            result("old.txt", "file", Some(1), Some("100"), "/r"),
            result("new.txt", "file", Some(1), Some("300"), "/r"),
            result("mid.txt", "file", Some(1), Some("200"), "/r"),
        ]);

        let entries = driver
            .list("/", &sort("date", "desc"), None, false)
            .unwrap();
        assert_eq!(names(&entries), vec!["new.txt", "mid.txt", "old.txt"]);
    }

    #[test]
    fn sorts_by_extension_kind() {
        let driver = driver(vec![
            result("readme.md", "file", Some(1), None, "/r"),
            result("archive.zip", "file", Some(1), None, "/r"),
            result("Makefile", "file", Some(1), None, "/r"),
        ]);

        let entries = driver.list("/", &sort("kind", "asc"), None, false).unwrap();
        assert_eq!(
            names(&entries),
            vec!["Makefile", "readme.md", "archive.zip"]
        );
    }

    #[test]
    fn keeps_directories_before_files_for_all_sorts() {
        let driver = driver(vec![
            result("z-file.txt", "file", Some(1), Some("300"), "/r"),
            result("a-dir", "directory", None, Some("100"), "/r"),
            result("b-file.txt", "file", Some(2), Some("200"), "/r"),
            result("z-dir", "directory", None, Some("400"), "/r"),
        ]);

        let entries = driver
            .list("/", &sort("date", "desc"), None, false)
            .unwrap();
        assert_eq!(
            names(&entries),
            vec!["z-dir", "a-dir", "z-file.txt", "b-file.txt"]
        );
    }
}
