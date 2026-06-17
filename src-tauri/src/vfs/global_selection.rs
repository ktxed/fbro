use std::any::Any;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::vfs::{format_mtime, DirChild, VfsDriver, VfsEntry, VfsError, VfsKind};
use crate::ws::protocol::{FilterSpec, SortSpec};

pub const SOURCE_ID: &str = "global-selections";

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSelectionSummary {
    pub id: String,
    pub name: String,
    pub item_count: usize,
    pub created_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalSelectionItem {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub size: Option<u64>,
    pub mtime: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalSelection {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub items: Vec<GlobalSelectionItem>,
}

#[derive(Debug, Default)]
pub struct GlobalSelectionStore {
    selections: Vec<GlobalSelection>,
    next_id: u64,
}

pub type SharedGlobalSelectionStore = Arc<Mutex<GlobalSelectionStore>>;

pub fn normalize_global_selection_path(path: &str) -> String {
    #[cfg(windows)]
    {
        path.replace('\\', "/").to_lowercase()
    }
    #[cfg(not(windows))]
    {
        path.replace('\\', "/")
    }
}

impl GlobalSelectionStore {
    pub fn summaries(&self) -> Vec<GlobalSelectionSummary> {
        self.selections
            .iter()
            .map(|selection| GlobalSelectionSummary {
                id: selection.id.clone(),
                name: selection.name.clone(),
                item_count: selection.items.len(),
                created_at: selection.created_at.clone(),
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
    }

    pub fn next_implicit_name(&self) -> String {
        let existing: HashSet<&str> = self.selections.iter().map(|s| s.name.as_str()).collect();
        let mut n = 1;
        loop {
            let candidate = format!("Selection {n}");
            if !existing.contains(candidate.as_str()) {
                return candidate;
            }
            n += 1;
        }
    }

    pub fn selection_id_by_name(&self, name: &str) -> Option<String> {
        self.selections
            .iter()
            .find(|selection| selection.name == name)
            .map(|selection| selection.id.clone())
    }

    pub fn selection_by_name(&self, name: &str) -> Option<GlobalSelection> {
        self.selections
            .iter()
            .find(|selection| selection.name == name)
            .cloned()
    }

    pub fn selection_by_id(&self, id: &str) -> Option<GlobalSelection> {
        self.selections
            .iter()
            .find(|selection| selection.id == id)
            .cloned()
    }

    pub fn create_selection(&mut self, name: Option<&str>) -> Result<String, String> {
        let name = match name.map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => validate_selection_name(name, self)?.to_string(),
            None => self.next_implicit_name(),
        };
        let id = format!("gs-{}", self.next_id);
        self.next_id += 1;
        let created_at = format_mtime(SystemTime::now()).unwrap_or_else(|| "0".to_string());
        self.selections.push(GlobalSelection {
            id: id.clone(),
            name,
            created_at,
            items: Vec::new(),
        });
        Ok(id)
    }

    pub fn ensure_selection(
        &mut self,
        id: Option<&str>,
        name: Option<&str>,
    ) -> Result<String, String> {
        if let Some(id) = id.filter(|id| !id.trim().is_empty()) {
            if self.selections.iter().any(|selection| selection.id == id) {
                return Ok(id.to_string());
            }
            return Err("Global selection not found.".to_string());
        }
        self.create_selection(name)
    }

    pub fn add_items(
        &mut self,
        selection_id: &str,
        items: Vec<GlobalSelectionItem>,
    ) -> Result<usize, String> {
        let selection = self
            .selections
            .iter_mut()
            .find(|selection| selection.id == selection_id)
            .ok_or_else(|| "Global selection not found.".to_string())?;
        let mut existing: HashSet<String> = selection
            .items
            .iter()
            .map(|item| normalize_global_selection_path(&item.path))
            .collect();
        let before = selection.items.len();
        for item in items {
            let key = normalize_global_selection_path(&item.path);
            if existing.insert(key) {
                selection.items.push(item);
            }
        }
        Ok(selection.items.len() - before)
    }

    pub fn remove_items_by_paths(
        &mut self,
        selection_name: &str,
        paths: &[String],
    ) -> Result<usize, String> {
        let selection = self
            .selections
            .iter_mut()
            .find(|selection| selection.name == selection_name)
            .ok_or_else(|| "Global selection not found.".to_string())?;
        let keys: HashSet<String> = paths
            .iter()
            .map(|path| normalize_global_selection_path(path))
            .collect();
        let before = selection.items.len();
        selection
            .items
            .retain(|item| !keys.contains(&normalize_global_selection_path(&item.path)));
        Ok(before - selection.items.len())
    }

    pub fn rename_selection(&mut self, old_name: &str, new_name: &str) -> Result<(), String> {
        validate_selection_name(new_name, self)?;
        let selection = self
            .selections
            .iter_mut()
            .find(|selection| selection.name == old_name)
            .ok_or_else(|| "Global selection not found.".to_string())?;
        selection.name = new_name.to_string();
        Ok(())
    }
}

fn validate_selection_name<'a>(
    name: &'a str,
    store: &GlobalSelectionStore,
) -> Result<&'a str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Selection name is required.".to_string());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("Selection name cannot contain path separators.".to_string());
    }
    if store
        .selections
        .iter()
        .any(|selection| selection.name == name)
    {
        return Err("A global selection with this name already exists.".to_string());
    }
    Ok(name)
}

pub struct GlobalSelectionDriver {
    store: SharedGlobalSelectionStore,
}

impl GlobalSelectionDriver {
    pub fn new(store: SharedGlobalSelectionStore) -> Self {
        Self { store }
    }
}

impl VfsDriver for GlobalSelectionDriver {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn list(
        &self,
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
        _show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        let store = self
            .store
            .lock()
            .map_err(|_| VfsError::IOError("global selection store is poisoned".to_string()))?;
        let mut entries = if path.is_empty() {
            store
                .selections
                .iter()
                .map(|selection| VfsEntry {
                    name: selection.name.clone(),
                    kind: VfsKind::Directory,
                    size: Some(selection.items.len() as u64),
                    mtime: Some(selection.created_at.clone()),
                    btime: Some(selection.created_at.clone()),
                    permissions: None,
                    icon_name: Some("folder".to_string()),
                    cc: Some("cc-dir".to_string()),
                    source_path: None,
                })
                .collect::<Vec<_>>()
        } else {
            let selection = store
                .selection_by_name(path)
                .ok_or_else(|| VfsError::NotFound(format!("global selection not found: {path}")))?;
            selection
                .items
                .iter()
                .map(|item| VfsEntry {
                    name: item.path.clone(),
                    kind: kind_from_str(&item.kind),
                    size: item.size,
                    mtime: item.mtime.clone(),
                    btime: None,
                    permissions: None,
                    icon_name: None,
                    cc: None,
                    source_path: Some(item.path.clone()),
                })
                .collect::<Vec<_>>()
        };

        if let Some(filter) = filter {
            let needle = filter.value.to_lowercase();
            entries.retain(|entry| entry.name.to_lowercase().contains(&needle));
        }

        match (sort.by.as_str(), sort.order.as_str()) {
            ("size", "desc") => entries.sort_by(|a, b| b.size.cmp(&a.size)),
            ("size", _) => entries.sort_by(|a, b| a.size.cmp(&b.size)),
            ("mtime", "desc") => entries.sort_by(|a, b| b.mtime.cmp(&a.mtime)),
            ("mtime", _) => entries.sort_by(|a, b| a.mtime.cmp(&b.mtime)),
            ("kind", "desc") => entries.sort_by(|a, b| {
                b.kind
                    .as_str()
                    .cmp(a.kind.as_str())
                    .then_with(|| b.name.cmp(&a.name))
            }),
            ("kind", _) => entries.sort_by(|a, b| {
                a.kind
                    .as_str()
                    .cmp(b.kind.as_str())
                    .then_with(|| a.name.cmp(&b.name))
            }),
            ("name", "desc") => {
                entries.sort_by(|a, b| b.name.to_lowercase().cmp(&a.name.to_lowercase()))
            }
            _ => entries.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        }

        Ok(entries)
    }

    fn resolve_child(&self, base: &str, name: &str) -> String {
        if base.is_empty() {
            name.to_string()
        } else {
            name.to_string()
        }
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        if path.is_empty() {
            None
        } else {
            Some(String::new())
        }
    }

    fn open_read(&self, _path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        Err(VfsError::NotSupported(
            "global selection entries are references".to_string(),
        ))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(VfsError::NotSupported(
            "global selections are virtual".to_string(),
        ))
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "global selections are virtual".to_string(),
        ))
    }

    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "use RemoveGlobalSelectionItems".to_string(),
        ))
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "global selections are virtual".to_string(),
        ))
    }

    fn exists(&self, path: &str) -> bool {
        let Ok(store) = self.store.lock() else {
            return false;
        };
        if path.is_empty() {
            return true;
        }
        store.selection_by_name(path).is_some() || Path::new(path).exists()
    }

    fn file_size(&self, path: &str) -> Option<u64> {
        let store = self.store.lock().ok()?;
        store
            .selections
            .iter()
            .flat_map(|selection| selection.items.iter())
            .find(|item| item.path == path)
            .and_then(|item| item.size)
    }

    fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> {
        Ok(vec![])
    }

    fn read_chunk(&self, _path: &str, _offset: u64, _size: usize) -> Result<Vec<u8>, VfsError> {
        Err(VfsError::NotSupported(
            "global selection entries are references".to_string(),
        ))
    }

    fn try_rename(&self, _src: &str, _dst: &str) -> Result<bool, VfsError> {
        Err(VfsError::NotSupported(
            "use RenameFile at the global selection root".to_string(),
        ))
    }
}

fn kind_from_str(kind: &str) -> VfsKind {
    match kind {
        "directory" => VfsKind::Directory,
        "symlink" => VfsKind::Symlink,
        "other" => VfsKind::Other,
        _ => VfsKind::File,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(path: &str) -> GlobalSelectionItem {
        GlobalSelectionItem {
            path: path.to_string(),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            kind: "file".to_string(),
            size: Some(1),
            mtime: None,
        }
    }

    #[test]
    fn deduplicates_by_normalized_path() {
        let mut store = GlobalSelectionStore::default();
        let id = store.create_selection(None).unwrap();
        assert!(store.selection_by_id(&id).unwrap().created_at.parse::<u64>().is_ok());
        assert_eq!(
            store
                .add_items(&id, vec![item("/tmp/a"), item("/tmp/a")])
                .unwrap(),
            1
        );
        assert_eq!(store.selection_by_id(&id).unwrap().items.len(), 1);
    }

    #[test]
    fn rejects_duplicate_names() {
        let mut store = GlobalSelectionStore::default();
        store.create_selection(Some("A")).unwrap();
        assert!(store.create_selection(Some("A")).is_err());
    }

    #[test]
    fn driver_limits_navigation_to_one_level() {
        let store = Arc::new(Mutex::new(GlobalSelectionStore::default()));
        {
            let mut guard = store.lock().unwrap();
            let id = guard.create_selection(Some("A")).unwrap();
            guard.add_items(&id, vec![item("/tmp/a")]).unwrap();
        }
        let driver = GlobalSelectionDriver::new(store);
        let sort = SortSpec {
            by: "name".into(),
            order: "asc".into(),
        };
        assert_eq!(driver.list("", &sort, None, false).unwrap().len(), 1);
        assert_eq!(driver.list("A", &sort, None, false).unwrap().len(), 1);
        assert_eq!(driver.parent_path("A"), Some(String::new()));
        assert!(driver.parent_path("").is_none());
    }
}
