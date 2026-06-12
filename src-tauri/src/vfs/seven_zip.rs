use std::collections::HashMap;
use std::io::{Read, Seek, Write};
use std::sync::{Arc, Mutex};

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

    /// All file (non-directory) descendants of `dir_path`, returned as
    /// `(inner_path, size)` pairs. Used to expand a directory selection into
    /// the concrete entry list for batch extraction.
    fn flatten_files(&self, dir_path: &str) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut stack = vec![dir_path.to_string()];
        while let Some(current) = stack.pop() {
            let Some(entries) = self.children.get(&current) else {
                continue;
            };
            for entry in entries {
                let child_path = if current.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{}/{}", current, entry.name)
                };
                if entry.is_dir {
                    stack.push(child_path);
                } else {
                    out.push((child_path, entry.size));
                }
            }
        }
        out
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

    /// Look up the uncompressed size of a single file entry. Returns `None`
    /// for directories or missing paths.
    pub fn entry_size(&self, inner_path: &str) -> Option<u64> {
        self.tree.find_entry(inner_path).and_then(|meta| {
            if meta.is_dir {
                None
            } else {
                Some(meta.size)
            }
        })
    }

    /// Expand a directory entry into a flat `(inner_path, size)` list of every
    /// file beneath it. For file entries pass them in directly — callers use
    /// this only when they have a directory.
    pub fn flatten_files(&self, dir_path: &str) -> Vec<(String, u64)> {
        self.tree.flatten_files(dir_path)
    }

    /// Stream-extract many entries from the archive.
    ///
    /// 7z files are by default a solid LZMA2 stream, meaning decoding entry N
    /// requires decoding entries 0..N first within its block. We can't speed
    /// up an individual block's decode (LZMA2 dependent chunks are sequential
    /// by design), but max-compression 7z splits large archives into many
    /// independent solid blocks. We fan out one worker per block and have
    /// them decode in parallel — that's how `7z.exe` gets its speed.
    ///
    /// Within each block we force `thread_count = 1`: sevenz-rust2's MT
    /// LZMA2 reader buffers the entire compressed stream into one work-unit
    /// when all chunks are dependent, which kills both progress and startup
    /// latency. Lazy single-threaded decode streams bytes from the first
    /// chunk onward.
    ///
    /// `targets` is `(inner_path, dest_path)`. Parent directories on disk
    /// are created as needed; existing destination files are overwritten.
    pub fn extract_many(
        &self,
        targets: &[(String, std::path::PathBuf)],
        reporter: &crate::jobs::progress::ProgressReporter,
    ) -> Result<(), VfsError> {
        use std::collections::HashMap;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Mutex;

        let want: Arc<HashMap<String, std::path::PathBuf>> = Arc::new(
            targets
                .iter()
                .map(|(inner, dest)| {
                    (
                        inner.replace('\\', "/").trim_end_matches('/').to_string(),
                        dest.clone(),
                    )
                })
                .collect(),
        );

        let password = self
            .get_password()
            .map(|p| Password::from(p.as_str()))
            .unwrap_or_else(Password::empty);

        // Parse the archive header once on the calling thread. Each worker
        // then opens its own File handle and shares the parsed structure.
        let archive = {
            let mut file = std::fs::File::open(&self.archive_path)
                .map_err(|e| VfsError::IOError(format!("Cannot open archive: {e}")))?;
            sevenz_rust2::Archive::read(&mut file, &password).map_err(|e| match &e {
                sevenz_rust2::Error::PasswordRequired => {
                    VfsError::PermissionDenied("encrypted".to_string())
                }
                sevenz_rust2::Error::MaybeBadPassword(_) => {
                    VfsError::PermissionDenied("encrypted:bad_password".to_string())
                }
                _ => VfsError::IOError(format!("Cannot read archive: {e}")),
            })?
        };

        let block_count = archive.blocks.len();
        if block_count == 0 {
            return Ok(());
        }
        let n_cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        let max_threads = n_cores.min(block_count);
        eprintln!(
            "[fbro] 7z extract_many: blocks={block_count}, workers={max_threads}, targets={}",
            want.len()
        );

        let archive = Arc::new(archive);
        let password = Arc::new(password);
        let archive_path = self.archive_path.clone();
        let next_block = Arc::new(AtomicUsize::new(0));
        let first_err: Arc<Mutex<Option<VfsError>>> = Arc::new(Mutex::new(None));

        let mut handles = Vec::with_capacity(max_threads);
        for _ in 0..max_threads {
            let archive = Arc::clone(&archive);
            let password = Arc::clone(&password);
            let want = Arc::clone(&want);
            let reporter = reporter.clone();
            let next_block = Arc::clone(&next_block);
            let first_err = Arc::clone(&first_err);
            let archive_path = archive_path.clone();
            let total_blocks = block_count;

            let handle = std::thread::spawn(move || {
                let record_err = |err: VfsError| {
                    let mut slot = first_err.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(err);
                    }
                };

                loop {
                    if reporter.is_cancelled() {
                        return;
                    }
                    if first_err.lock().unwrap().is_some() {
                        return;
                    }

                    let idx = next_block.fetch_add(1, Ordering::SeqCst);
                    if idx >= total_blocks {
                        return;
                    }

                    let mut source = match std::fs::File::open(&archive_path) {
                        Ok(f) => f,
                        Err(e) => {
                            record_err(VfsError::IOError(format!(
                                "Cannot open archive: {e}"
                            )));
                            return;
                        }
                    };

                    let block_decoder = sevenz_rust2::BlockDecoder::new(
                        1, // sequential LZMA2 within the block
                        idx, &archive, &password, &mut source,
                    );

                    let res = block_decoder.for_each_entries(&mut |entry, entry_reader| {
                        if reporter.is_cancelled() {
                            return Err(sevenz_rust2::Error::from(std::io::Error::new(
                                std::io::ErrorKind::Other,
                                "Cancelled",
                            )));
                        }
                        if entry.is_directory() {
                            return Ok(true);
                        }
                        let entry_name = entry
                            .name()
                            .replace('\\', "/")
                            .trim_end_matches('/')
                            .to_string();

                        // The 7z solid stream demands every entry's bytes be
                        // consumed in order — the next file's `BoundedReader`
                        // reads from the shared decoder at its current
                        // position. If we skip an entry without draining its
                        // bytes the stream gets misaligned and subsequent
                        // files come back as garbage.
                        let Some(dest_path) = want.get(&entry_name) else {
                            std::io::copy(entry_reader, &mut std::io::sink())
                                .map_err(sevenz_rust2::Error::from)?;
                            return Ok(true);
                        };

                        if let Some(parent) = dest_path.parent() {
                            if !parent.as_os_str().is_empty() {
                                std::fs::create_dir_all(parent)
                                    .map_err(sevenz_rust2::Error::from)?;
                            }
                        }

                        let dest_file = std::fs::File::create(dest_path)
                            .map_err(sevenz_rust2::Error::from)?;
                        let mut progress = crate::jobs::progress::ProgressReader::new(
                            entry_reader,
                            reporter.clone(),
                            entry_name.clone(),
                            entry.size(),
                            "Extracting",
                        );
                        let mut sink =
                            std::io::BufWriter::with_capacity(256 * 1024, dest_file);
                        std::io::copy(&mut progress, &mut sink)
                            .map_err(sevenz_rust2::Error::from)?;
                        Ok(true)
                    });

                    if let Err(e) = res {
                        let mapped = if reporter.is_cancelled() {
                            VfsError::IOError("Cancelled".to_string())
                        } else {
                            match e {
                                sevenz_rust2::Error::PasswordRequired => {
                                    VfsError::PermissionDenied("encrypted".to_string())
                                }
                                sevenz_rust2::Error::MaybeBadPassword(_)
                                | sevenz_rust2::Error::ChecksumVerificationFailed => {
                                    VfsError::PermissionDenied(
                                        "encrypted:bad_password".to_string(),
                                    )
                                }
                                other => {
                                    VfsError::IOError(format!("Extraction failed: {other}"))
                                }
                            }
                        };
                        record_err(mapped);
                        return;
                    }
                }
            });
            handles.push(handle);
        }

        for h in handles {
            // join failures only happen on panic — we have no panic-prone code
            // in the worker, but if one ever escapes we want to surface it.
            if let Err(_) = h.join() {
                let mut slot = first_err.lock().unwrap();
                if slot.is_none() {
                    *slot = Some(VfsError::IOError(
                        "Extraction worker panicked".to_string(),
                    ));
                }
            }
        }

        if let Some(err) = first_err.lock().unwrap().take() {
            return Err(err);
        }
        if reporter.is_cancelled() {
            return Err(VfsError::IOError("Cancelled".to_string()));
        }
        Ok(())
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
