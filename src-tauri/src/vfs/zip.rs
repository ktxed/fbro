use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::sync::Mutex;

use super::{
    color_class_for_entry, icon_name_for_entry, DirChild, VfsDriver, VfsEntry, VfsError, VfsKind,
};
use crate::ws::protocol::{FilterSpec, SortSpec};

// ─── In-memory directory tree ───────────────────────────────────────────────

/// Metadata for a single entry inside the zip archive.
#[derive(Clone, Debug)]
struct ZipMeta {
    name: String,
    is_dir: bool,
    size: u64, // uncompressed
    mtime: Option<String>,
}

/// The pre-parsed directory tree built from the zip central directory.
/// Maps a virtual directory path (e.g. "" for root, "subdir", "subdir/nested")
/// to the list of its direct children.
struct DirTree {
    children: HashMap<String, Vec<ZipMeta>>,
}

impl DirTree {
    fn build(archive_path: &str) -> Result<Self, VfsError> {
        let file = std::fs::File::open(archive_path)
            .map_err(|e| VfsError::IOError(format!("Cannot open archive: {e}")))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| VfsError::IOError(format!("Cannot read zip: {e}")))?;

        let mut children: HashMap<String, Vec<ZipMeta>> = HashMap::new();
        // Track names we've already added so implicit directories aren't duplicated.
        let mut known_dirs: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Ensure root always exists.
        children.entry(String::new()).or_default();

        for i in 0..archive.len() {
            let entry = match archive.by_index_raw(i) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let raw_name = entry.name().to_string();
            let is_dir = entry.is_dir();
            let size = entry.size(); // uncompressed
            let mtime = zip_mtime_to_epoch(&entry);

            // Normalize: strip trailing slash for directories.
            let clean_name = raw_name.trim_end_matches('/').to_string();
            if clean_name.is_empty() {
                continue;
            }

            // Determine parent directory and entry name.
            let (parent, entry_name) = match clean_name.rfind('/') {
                Some(pos) => (
                    clean_name[..pos].to_string(),
                    clean_name[pos + 1..].to_string(),
                ),
                None => (String::new(), clean_name.clone()),
            };

            // Ensure all ancestor directories exist (zip files sometimes omit explicit dir entries).
            ensure_ancestors(&mut children, &mut known_dirs, &parent);

            let meta = ZipMeta {
                name: entry_name.clone(),
                is_dir,
                size,
                mtime: mtime.clone(),
            };

            children.entry(parent.clone()).or_default();
            let parent_entries = children.get_mut(&parent).unwrap();
            if let Some(existing) = parent_entries.iter_mut().find(|e| e.name == entry_name) {
                // Entry already exists — prefer the one with mtime (explicit zip entry
                // over an implicit directory created by ensure_ancestors).
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

        Ok(DirTree { children })
    }

    fn list_dir(&self, path: &str) -> Option<&Vec<ZipMeta>> {
        self.children.get(path)
    }

    fn exists(&self, path: &str) -> bool {
        if path.is_empty() {
            return true; // root
        }
        // Check if it's a directory.
        if self.children.contains_key(path) {
            return true;
        }
        // Check if it's a file entry in some parent.
        let (parent, name) = match path.rfind('/') {
            Some(pos) => (&path[..pos], &path[pos + 1..]),
            None => ("", path),
        };
        self.children
            .get(parent)
            .map(|entries| entries.iter().any(|e| e.name == name))
            .unwrap_or(false)
    }

    fn find_entry(&self, path: &str) -> Option<&ZipMeta> {
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

    /// Recursively sum the uncompressed sizes of all file entries under `dir_path`.
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

/// Ensure all ancestor paths are registered as directories in the tree.
fn ensure_ancestors(
    children: &mut HashMap<String, Vec<ZipMeta>>,
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
            if let Some(_existing) = parent_entries
                .iter_mut()
                .find(|e| e.name == *part && e.is_dir)
            {
                // Implicit entry already exists, keep it as-is.
            } else if !parent_entries.iter().any(|e| e.name == *part) {
                parent_entries.push(ZipMeta {
                    name: part.to_string(),
                    is_dir: true,
                    size: 0,
                    mtime: None,
                });
            }
        }
    }
}

/// Convert a zip entry's last-modified datetime to a Unix epoch seconds string.
fn zip_mtime_to_epoch<R: std::io::Read>(entry: &zip::read::ZipFile<'_, R>) -> Option<String> {
    // Prefer the Unix extended timestamp (0x5455 extra field) — accurate epoch seconds,
    // always present in zip files created on Linux/macOS.
    use zip::extra_fields::ExtraField;
    for field in entry.extra_data_fields() {
        if let ExtraField::ExtendedTimestamp(ts) = field {
            if let Some(secs) = ts.mod_time() {
                return Some((secs as u64).to_string());
            }
        }
    }

    // Fall back to DOS timestamp.
    let dt = entry.last_modified()?;
    // zip::DateTime stores year/month/day/hour/minute/second.
    // Convert to a rough epoch timestamp (no timezone, assume UTC).
    let year = dt.year() as i64;
    let month = dt.month() as i64;
    let day = dt.day() as i64;
    let hour = dt.hour() as i64;
    let minute = dt.minute() as i64;
    let second = dt.second() as i64;

    // Simplified days-since-epoch calculation.
    let mut days: i64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    let month_days = [
        31,
        28 + if is_leap(year) { 1 } else { 0 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    for m in 0..(month - 1) as usize {
        days += month_days[m] as i64;
    }
    days += day - 1;
    let secs = days * 86400 + hour * 3600 + minute * 60 + second;
    Some(secs.to_string())
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

// ─── ZipFsDriver ────────────────────────────────────────────────────────────

pub struct ZipFsDriver {
    /// Absolute path to the .zip file on the host filesystem.
    archive_path: String,
    /// Pre-parsed directory tree from the central directory.
    tree: DirTree,
    /// Session password for encrypted entries (set lazily).
    password: Mutex<Option<String>>,
    /// Temp directory for extracted files (created on demand).
    temp_dir: Mutex<Option<std::path::PathBuf>>,
}

impl ZipFsDriver {
    pub fn new(archive_path: &str) -> Result<Self, VfsError> {
        let tree = DirTree::build(archive_path)?;
        Ok(ZipFsDriver {
            archive_path: archive_path.to_string(),
            tree,
            password: Mutex::new(None),
            temp_dir: Mutex::new(None),
        })
    }

    /// The absolute path to the .zip file on disk.
    pub fn archive_path(&self) -> &str {
        &self.archive_path
    }

    /// Set the password used for decrypting entries.
    pub fn set_password(&self, pwd: &str) {
        let mut lock = self.password.lock().unwrap();
        *lock = Some(pwd.to_string());
    }

    /// Get the current password (if any).
    pub fn get_password(&self) -> Option<String> {
        self.password.lock().unwrap().clone()
    }

    /// Get or create the temp directory for extracted files.
    fn ensure_temp_dir(&self) -> Result<std::path::PathBuf, VfsError> {
        let mut lock = self.temp_dir.lock().unwrap();
        if let Some(ref path) = *lock {
            return Ok(path.clone());
        }
        let dir = std::env::temp_dir()
            .join("fbro-zip-cache")
            .join(format!("{:x}", md5_hash(&self.archive_path)));
        std::fs::create_dir_all(&dir)
            .map_err(|e| VfsError::IOError(format!("Cannot create temp dir: {e}")))?;
        *lock = Some(dir.clone());
        Ok(dir)
    }

    /// Extract a file entry to a temp location and return the temp file path.
    pub fn extract_to_temp(&self, inner_path: &str) -> Result<std::path::PathBuf, VfsError> {
        let temp_dir = self.ensure_temp_dir()?;
        // Build a safe temp file path that mirrors the archive structure.
        let safe_name = inner_path.replace('/', "__");
        let temp_path = temp_dir.join(&safe_name);

        // If already extracted, return immediately.
        if temp_path.exists() {
            return Ok(temp_path);
        }

        // Extract the entry.
        let mut data = Vec::new();
        let mut reader = self.open_read(inner_path)?;
        reader
            .read_to_end(&mut data)
            .map_err(|e| VfsError::IOError(format!("Extract failed: {e}")))?;

        let mut file = std::fs::File::create(&temp_path)
            .map_err(|e| VfsError::IOError(format!("Cannot create temp file: {e}")))?;
        file.write_all(&data)
            .map_err(|e| VfsError::IOError(format!("Cannot write temp file: {e}")))?;

        Ok(temp_path)
    }

    /// Clean up temp files when the driver is being torn down.
    pub fn cleanup(&self) {
        let lock = self.temp_dir.lock().unwrap();
        if let Some(ref path) = *lock {
            let _ = std::fs::remove_dir_all(path);
        }
    }

    /// Compute the recursive size of a virtual directory from the pre-parsed tree.
    pub fn dir_size(&self, path: &str) -> u64 {
        self.tree.recursive_size(path)
    }

    /// Open a ZipArchive handle — helper used by open_read and read_chunk.
    fn open_archive(&self) -> Result<zip::ZipArchive<std::fs::File>, VfsError> {
        let file = std::fs::File::open(&self.archive_path)
            .map_err(|e| VfsError::IOError(format!("Cannot open archive: {e}")))?;
        zip::ZipArchive::new(file).map_err(|e| VfsError::IOError(format!("Cannot read zip: {e}")))
    }

    /// The full zip entry name (with trailing slash for dirs) for a given virtual path.
    fn zip_entry_name(&self, path: &str) -> String {
        // Try exact name first, then with trailing slash for directories.
        path.to_string()
    }
}

/// Simple hash for temp directory naming — not cryptographic, just collision-avoidant.
fn md5_hash(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for byte in input.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    hash
}

// ─── Filter/sort helpers (shared with local driver logic) ───────────────────

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

// ─── VfsDriver implementation ───────────────────────────────────────────────

impl VfsDriver for ZipFsDriver {
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

        // Deduplicate by name, keeping the entry with mtime (explicit) over
        // the one without (implicit).
        let mut seen: std::collections::HashMap<String, &ZipMeta> =
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

        // Sort: directories first, then by the requested sort criteria.
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
            return None; // At archive root — caller handles exit
        }
        match path.rfind('/') {
            Some(pos) => Some(path[..pos].to_string()),
            None => Some(String::new()), // One level deep → go to root
        }
    }

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        let mut archive = self.open_archive()?;
        let password = self.get_password();

        // Try to find the entry by exact path.
        let entry_result = if let Some(ref pwd) = password {
            archive.by_name_decrypt(path, pwd.as_bytes())
        } else {
            archive.by_name(path)
        };

        match entry_result {
            Ok(mut entry) => {
                let mut buf = Vec::with_capacity(entry.size() as usize);
                entry.read_to_end(&mut buf).map_err(|e| {
                    // A bad password typically surfaces as an I/O error during decompression.
                    let msg = e.to_string();
                    if msg.contains("invalid password")
                        || msg.contains("checksum")
                        || msg.contains("Hmac")
                    {
                        VfsError::PermissionDenied("encrypted:bad_password".to_string())
                    } else {
                        VfsError::IOError(format!("Read error: {e}"))
                    }
                })?;
                Ok(Box::new(Cursor::new(buf)))
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("not found") || msg.contains("name not found") {
                    Err(VfsError::NotFound(path.to_string()))
                } else if msg.contains("password") || msg.contains("encrypted") {
                    Err(VfsError::PermissionDenied("encrypted".to_string()))
                } else {
                    Err(VfsError::IOError(format!("Zip error: {e}")))
                }
            }
        }
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn std::io::Write + Send>, VfsError> {
        Err(VfsError::NotSupported(
            "Cannot write into a zip archive".to_string(),
        ))
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "Cannot create directories in a zip archive".to_string(),
        ))
    }

    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "Cannot delete from a zip archive".to_string(),
        ))
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "Cannot delete from a zip archive".to_string(),
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
        // Extract to temp file (cached), then serve chunks from it.
        let temp_path = self.extract_to_temp(path)?;
        let mut file = std::fs::File::open(&temp_path)
            .map_err(|e| VfsError::IOError(format!("Cannot open temp file: {e}")))?;
        use std::io::Seek;
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
            "Cannot rename inside a zip archive".to_string(),
        ))
    }
}

impl Drop for ZipFsDriver {
    fn drop(&mut self) {
        self.cleanup();
    }
}
