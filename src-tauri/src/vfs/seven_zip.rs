use std::collections::HashMap;
use std::io::{Read, Seek, Write};
use std::sync::Mutex;

use sevenz_rust2::{ArchiveReader, NtTime, Password};

use super::{
    color_class_for_entry, icon_name_for_entry, DirChild, VfsDriver, VfsEntry, VfsError, VfsKind,
};
use crate::ws::protocol::{FilterSpec, SortSpec};

// ─── In-memory directory tree ───────────────────────────────────────────────

#[derive(Clone, Debug)]
struct SevenZipMeta {
    name: String,
    is_dir: bool,
    size: u64,
    mtime: Option<String>,
}

struct DirTree {
    children: HashMap<String, Vec<SevenZipMeta>>,
}

impl DirTree {
    fn build(archive_path: &str, password: Password) -> Result<Self, VfsError> {
        let reader = ArchiveReader::open(archive_path, password).map_err(|e| match e {
            sevenz_rust2::Error::PasswordRequired => {
                VfsError::PermissionDenied("encrypted".to_string())
            }
            sevenz_rust2::Error::MaybeBadPassword(_) => {
                VfsError::PermissionDenied("encrypted:bad_password".to_string())
            }
            other => VfsError::IOError(format!("Cannot open archive: {other}")),
        })?;

        let mut children: HashMap<String, Vec<SevenZipMeta>> = HashMap::new();
        let mut known_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();

        children.entry(String::new()).or_default();

        for entry in &reader.archive().files.clone() {
            let raw_name = entry.name().replace('\\', "/");
            let clean_name = raw_name.trim_end_matches('/').to_string();
            if clean_name.is_empty() {
                continue;
            }

            let is_dir = entry.is_directory();
            let size = if is_dir { 0 } else { entry.size() };
            let mtime = if entry.has_last_modified_date {
                nt_time_to_epoch(entry.last_modified_date())
            } else {
                None
            };

            let (parent, entry_name) = match clean_name.rfind('/') {
                Some(pos) => (
                    clean_name[..pos].to_string(),
                    clean_name[pos + 1..].to_string(),
                ),
                None => (String::new(), clean_name.clone()),
            };

            ensure_ancestors(&mut children, &mut known_dirs, &parent);

            let meta = SevenZipMeta {
                name: entry_name.clone(),
                is_dir,
                size,
                mtime: mtime.clone(),
            };

            children.entry(parent.clone()).or_default();
            let parent_entries = children.get_mut(&parent).unwrap();
            if let Some(existing) = parent_entries.iter_mut().find(|e| e.name == entry_name) {
                if mtime.is_some() && existing.mtime.is_none() {
                    existing.is_dir = is_dir;
                    existing.size = size;
                    existing.mtime = mtime;
                }
            } else {
                parent_entries.push(meta);
            }

            if is_dir {
                known_dirs.insert(clean_name.clone());
                children.entry(clean_name).or_default();
            }
        }

        drop(reader);
        Ok(DirTree { children })
    }

    fn list_dir(&self, path: &str) -> Option<&Vec<SevenZipMeta>> {
        self.children.get(path)
    }

    fn exists(&self, path: &str) -> bool {
        if path.is_empty() {
            return true;
        }
        if self.children.contains_key(path) {
            return true;
        }
        let (parent, name) = match path.rfind('/') {
            Some(pos) => (&path[..pos], &path[pos + 1..]),
            None => ("", path),
        };
        self.children
            .get(parent)
            .map(|entries| entries.iter().any(|e| e.name == name))
            .unwrap_or(false)
    }

    fn find_entry(&self, path: &str) -> Option<&SevenZipMeta> {
        if path.is_empty() {
            return None;
        }
        let (parent, name) = match path.rfind('/') {
            Some(pos) => (&path[..pos], &path[pos + 1..]),
            None => ("", path),
        };
        self.children
            .get(parent)
            .and_then(|entries| entries.iter().find(|e| e.name == name))
    }

    fn recursive_size(&self, dir_path: &str) -> u64 {
        let mut total: u64 = 0;
        let mut stack = vec![dir_path.to_string()];
        while let Some(current) = stack.pop() {
            if let Some(entries) = self.children.get(&current) {
                for entry in entries {
                    total += entry.size;
                    if entry.is_dir {
                        let child_path = if current.is_empty() {
                            entry.name.clone()
                        } else {
                            format!("{}/{}", current, entry.name)
                        };
                        stack.push(child_path);
                    }
                }
            }
        }
        total
    }
}

fn ensure_ancestors(
    children: &mut HashMap<String, Vec<SevenZipMeta>>,
    known_dirs: &mut std::collections::HashSet<String>,
    dir_path: &str,
) {
    if dir_path.is_empty() || known_dirs.contains(dir_path) {
        return;
    }
    let parts: Vec<&str> = dir_path.split('/').collect();
    let mut current = String::new();
    for (i, part) in parts.iter().enumerate() {
        let parent = current.clone();
        if i == 0 {
            current = part.to_string();
        } else {
            current = format!("{}/{}", current, part);
        }
        if known_dirs.insert(current.clone()) {
            children.entry(current.clone()).or_default();
            let parent_entries = children.entry(parent).or_default();
            if !parent_entries.iter().any(|e| e.name == *part) {
                parent_entries.push(SevenZipMeta {
                    name: part.to_string(),
                    is_dir: true,
                    size: 0,
                    mtime: None,
                });
            }
        }
    }
}

/// Convert a 7z entry's NtTime (Windows FILETIME) to a Unix epoch seconds string.
fn nt_time_to_epoch(nt: NtTime) -> Option<String> {
    let system_time: std::time::SystemTime = nt.into();
    super::format_mtime(system_time)
}

/// Simple FNV hash for temp directory naming.
fn fnv_hash(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

// ─── Filter/sort helper ──────────────────────────────────────────────────────

fn build_filter_regex(filter: Option<&FilterSpec>) -> Option<regex::Regex> {
    let f = filter?;
    if f.mode != "glob" || f.value.trim().is_empty() {
        return None;
    }
    let regex_str = f
        .value
        .split('|')
        .map(|part| {
            let part = part.trim();
            if part.contains('*') {
                let escaped: String =
                    part.chars()
                        .map(|ch| match ch {
                            '*' => ".*".to_string(),
                            '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$'
                            | '\\' => format!("\\{ch}"),
                            _ => ch.to_string(),
                        })
                        .collect();
                format!("^{escaped}$")
            } else {
                part.chars()
                    .map(|ch| match ch {
                        '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|'
                        | '\\' | '*' => format!("\\{ch}"),
                        _ => ch.to_string(),
                    })
                    .collect()
            }
        })
        .collect::<Vec<_>>()
        .join("|");
    regex::RegexBuilder::new(&regex_str)
        .case_insensitive(true)
        .build()
        .ok()
}

// ─── SevenZipFsDriver ────────────────────────────────────────────────────────

pub struct SevenZipFsDriver {
    archive_path: String,
    tree: DirTree,
    password: Mutex<Option<String>>,
    temp_dir: Mutex<Option<std::path::PathBuf>>,
}

impl SevenZipFsDriver {
    pub fn new(archive_path: &str) -> Result<Self, VfsError> {
        let tree = DirTree::build(archive_path, Password::empty())?;
        Ok(SevenZipFsDriver {
            archive_path: archive_path.to_string(),
            tree,
            password: Mutex::new(None),
            temp_dir: Mutex::new(None),
        })
    }

    pub fn set_password(&self, pwd: &str) {
        let mut lock = self.password.lock().unwrap();
        *lock = Some(pwd.to_string());
    }

    pub fn get_password(&self) -> Option<String> {
        self.password.lock().unwrap().clone()
    }

    pub fn dir_size(&self, path: &str) -> u64 {
        self.tree.recursive_size(path)
    }

    fn ensure_temp_dir(&self) -> Result<std::path::PathBuf, VfsError> {
        let mut lock = self.temp_dir.lock().unwrap();
        if let Some(ref path) = *lock {
            return Ok(path.clone());
        }
        let dir = std::env::temp_dir()
            .join("fbro-7z-cache")
            .join(format!("{:x}", fnv_hash(&self.archive_path)));
        std::fs::create_dir_all(&dir)
            .map_err(|e| VfsError::IOError(format!("Cannot create temp dir: {e}")))?;
        *lock = Some(dir.clone());
        Ok(dir)
    }

    /// Extract a single file entry to a temp location and return the path.
    /// Result is cached — subsequent calls for the same entry return immediately.
    pub fn extract_to_temp(&self, inner_path: &str) -> Result<std::path::PathBuf, VfsError> {
        let temp_dir = self.ensure_temp_dir()?;
        let safe_name = inner_path.replace('/', "__");
        let temp_path = temp_dir.join(&safe_name);

        if temp_path.exists() {
            return Ok(temp_path);
        }

        let password = self
            .get_password()
            .map(|p| Password::from(p.as_str()))
            .unwrap_or_else(Password::empty);

        let mut reader =
            ArchiveReader::open(&self.archive_path, password).map_err(|e| match &e {
                sevenz_rust2::Error::PasswordRequired => {
                    VfsError::PermissionDenied("encrypted".to_string())
                }
                sevenz_rust2::Error::MaybeBadPassword(_) => {
                    VfsError::PermissionDenied("encrypted:bad_password".to_string())
                }
                _ => VfsError::IOError(format!("Cannot open archive: {e}")),
            })?;

        let normalized = inner_path
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string();
        let temp_path_ref = temp_path.clone();
        let mut found = false;

        reader
            .for_each_entries(|entry, reader| {
                if entry.is_directory() {
                    return Ok(true);
                }
                let entry_name = entry
                    .name()
                    .replace('\\', "/")
                    .trim_end_matches('/')
                    .to_string();
                if entry_name != normalized {
                    return Ok(true);
                }
                found = true;
                let mut file =
                    std::fs::File::create(&temp_path_ref).map_err(sevenz_rust2::Error::from)?;
                std::io::copy(reader, &mut file).map_err(sevenz_rust2::Error::from)?;
                Ok(false)
            })
            .map_err(|e| match e {
                sevenz_rust2::Error::PasswordRequired => {
                    VfsError::PermissionDenied("encrypted".to_string())
                }
                sevenz_rust2::Error::MaybeBadPassword(_)
                | sevenz_rust2::Error::ChecksumVerificationFailed => {
                    VfsError::PermissionDenied("encrypted:bad_password".to_string())
                }
                other => VfsError::IOError(format!("Extraction failed: {other}")),
            })?;

        if !found {
            return Err(VfsError::NotFound(inner_path.to_string()));
        }

        Ok(temp_path)
    }

    pub fn cleanup(&self) {
        let lock = self.temp_dir.lock().unwrap();
        if let Some(ref path) = *lock {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

// ─── VfsDriver implementation ────────────────────────────────────────────────

impl VfsDriver for SevenZipFsDriver {
    fn is_local(&self) -> bool {
        false
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn list(
        &self,
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
        show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        let entries = self
            .tree
            .list_dir(path)
            .ok_or_else(|| VfsError::NotFound(format!("Directory not found in archive: {path}")))?;

        let filter_re = build_filter_regex(filter);
        let hide_hidden = !show_hidden;

        let mut seen: std::collections::HashMap<String, &SevenZipMeta> =
            std::collections::HashMap::new();
        for e in entries.iter() {
            if hide_hidden && e.name.starts_with('.') {
                continue;
            }
            if let Some(ref re) = filter_re {
                if !re.is_match(&e.name) {
                    continue;
                }
            }
            let replace = match seen.get(&e.name) {
                None => true,
                Some(existing) => e.mtime.is_some() && existing.mtime.is_none(),
            };
            if replace {
                seen.insert(e.name.clone(), e);
            }
        }

        let mut result: Vec<VfsEntry> = seen
            .into_values()
            .map(|e| {
                let kind = if e.is_dir {
                    VfsKind::Directory
                } else {
                    VfsKind::File
                };
                let icon_name = icon_name_for_entry(&e.name, &kind);
                let cc = color_class_for_entry(&e.name, &kind);
                VfsEntry {
                    name: e.name.clone(),
                    kind,
                    size: if e.is_dir { None } else { Some(e.size) },
                    mtime: e.mtime.clone(),
                    btime: None,
                    permissions: None,
                    icon_name,
                    cc,
                    source_path: None,
                }
            })
            .collect();

        result.sort_by(|a, b| {
            let a_dir = a.kind == VfsKind::Directory;
            let b_dir = b.kind == VfsKind::Directory;
            if a_dir != b_dir {
                return b_dir.cmp(&a_dir);
            }
            let ord = match sort.by.as_str() {
                "size" => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)),
                "date" => a.mtime.cmp(&b.mtime),
                "kind" => {
                    let ext = |e: &VfsEntry| -> String {
                        e.name
                            .rfind('.')
                            .map(|i| e.name[i + 1..].to_lowercase())
                            .unwrap_or_default()
                    };
                    ext(a).cmp(&ext(b))
                }
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            if sort.order == "desc" {
                ord.reverse()
            } else {
                ord
            }
        });

        Ok(result)
    }

    fn resolve_child(&self, base: &str, name: &str) -> String {
        if base.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", base, name)
        }
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        if path.is_empty() {
            return None;
        }
        match path.rfind('/') {
            Some(pos) => Some(path[..pos].to_string()),
            None => Some(String::new()),
        }
    }

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        // 7z has no random access — always extract to temp, then return a file reader.
        let temp_path = self.extract_to_temp(path)?;
        let file = std::fs::File::open(&temp_path)
            .map_err(|e| VfsError::IOError(format!("Cannot open temp file: {e}")))?;
        Ok(Box::new(file))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(VfsError::NotSupported(
            "Cannot write into a 7z archive".to_string(),
        ))
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "Cannot create directories in a 7z archive".to_string(),
        ))
    }

    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "Cannot delete from a 7z archive".to_string(),
        ))
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "Cannot delete from a 7z archive".to_string(),
        ))
    }

    fn exists(&self, path: &str) -> bool {
        self.tree.exists(path)
    }

    fn file_size(&self, path: &str) -> Option<u64> {
        self.tree.find_entry(path).map(|e| e.size)
    }

    fn read_dir_entries(&self, path: &str) -> Result<Vec<DirChild>, VfsError> {
        let entries = self
            .tree
            .list_dir(path)
            .ok_or_else(|| VfsError::NotFound(format!("Directory not found in archive: {path}")))?;

        Ok(entries
            .iter()
            .map(|e| DirChild {
                name: e.name.clone(),
                is_dir: e.is_dir,
                size: e.size,
                mtime: e.mtime.as_ref().and_then(|s| s.parse::<u64>().ok()),
                permissions: None,
            })
            .collect())
    }

    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
        let temp_path = self.extract_to_temp(path)?;
        let mut file = std::fs::File::open(&temp_path)
            .map_err(|e| VfsError::IOError(format!("Cannot open temp file: {e}")))?;
        file.seek(std::io::SeekFrom::Start(offset))
            .map_err(|e| VfsError::IOError(format!("Seek failed: {e}")))?;
        let mut buf = vec![0u8; size];
        let n = file
            .read(&mut buf)
            .map_err(|e| VfsError::IOError(format!("Read failed: {e}")))?;
        buf.truncate(n);
        Ok(buf)
    }

    fn try_rename(&self, _src: &str, _dst: &str) -> Result<bool, VfsError> {
        Err(VfsError::NotSupported(
            "Cannot rename inside a 7z archive".to_string(),
        ))
    }
}

impl Drop for SevenZipFsDriver {
    fn drop(&mut self) {
        self.cleanup();
    }
}
