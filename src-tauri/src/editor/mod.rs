//! Built-in editor tabs: the file-level core.
//!
//! An editor tab is a panel (`TabKind::Editor`) whose `editor` field carries an
//! `EditorTab`. This module owns everything about the edited file on disk:
//! loading, change detection, and saving. It knows nothing about notes; the
//! notes layer (`crate::notes`) only hands it a local file to open.
//!
//! Saves are small single-file writes, so they run on a blocking thread rather
//! than as an `AppJob` (no progress, cancel, pause, or job-bar entry).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::terminal::{file_fingerprint, FileFingerprint};
use crate::vfs::VfsDriver;

/// Files larger than this are refused; the editor is for notes and small text.
pub(crate) const MAX_EDITOR_BYTES: u64 = 2 * 1024 * 1024;

const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Eol {
    Lf,
    Crlf,
}

impl Eol {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Eol::Lf => "lf",
            Eol::Crlf => "crlf",
        }
    }
}

/// What the editor last loaded from, or saved to, disk.
#[derive(Clone, Debug)]
pub(crate) struct DiskState {
    /// blake3 of the bytes; the only change test (see `disk_change`).
    pub hash: [u8; 32],
    pub eol: Eol,
    pub bom: bool,
}

/// A note created by the notes layer for this tab. Used only to discard the
/// note again when its tab closes without ever having been written to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CreatedNote {
    pub store_id: String,
    pub note_id: String,
}

/// Editor state carried by a `TabKind::Editor` panel.
#[derive(Clone, Debug)]
pub(crate) struct EditorTab {
    /// Frontend cache key. New for every tab, including restored ones.
    pub session_id: u64,
    /// Canonical local path (symlinks resolved when the tab was opened).
    pub file_path: String,
    /// Listing tab Escape returns to. `None` when there was no listing tab.
    pub origin_panel_id: Option<String>,
    pub created_note: Option<CreatedNote>,
    /// Bumped on every save, reload, and Keep both; quick preview refreshes on it.
    pub saved_version: u64,
    /// True once any save succeeded in this session.
    pub ever_saved: bool,
    /// `None` until the first `LoadEditorSession`.
    pub disk: Option<DiskState>,
    /// Serializes saves for this tab.
    pub io_lock: Arc<tokio::sync::Mutex<()>>,
}

impl EditorTab {
    pub(crate) fn new(
        session_id: u64,
        file_path: String,
        origin_panel_id: Option<String>,
        created_note: Option<CreatedNote>,
    ) -> Self {
        Self {
            session_id,
            file_path,
            origin_panel_id,
            created_note,
            saved_version: 0,
            ever_saved: false,
            disk: None,
            io_lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub(crate) fn file_name(&self) -> String {
        Path::new(&self.file_path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.file_path.clone())
    }

    /// The panel patch / snapshot payload for this tab.
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "sessionId": self.session_id,
            "filePath": self.file_path,
            "fileName": self.file_name(),
            "originPanelId": self.origin_panel_id,
            "savedVersion": self.saved_version,
            "isNote": self.created_note.is_some(),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EditorError {
    NotLocal,
    NotRegularFile,
    TooLarge,
    NotUtf8,
    Stale,
    Missing,
    Io(String),
}

impl EditorError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            EditorError::NotLocal => "NOT_LOCAL",
            EditorError::NotRegularFile => "NOT_REGULAR_FILE",
            EditorError::TooLarge => "TOO_LARGE",
            EditorError::NotUtf8 => "NOT_UTF8",
            EditorError::Stale => "STALE",
            EditorError::Missing => "MISSING",
            EditorError::Io(_) => "IO",
        }
    }

    pub(crate) fn message(&self) -> String {
        match self {
            EditorError::NotLocal => "The built-in editor only opens files on a local disk.".into(),
            EditorError::NotRegularFile => "The built-in editor only opens regular files.".into(),
            EditorError::TooLarge => format!(
                "The file is larger than {} MB, the built-in editor's limit.",
                MAX_EDITOR_BYTES / (1024 * 1024)
            ),
            EditorError::NotUtf8 => "The file is not UTF-8 text.".into(),
            EditorError::Stale => "The file changed on disk since it was loaded.".into(),
            EditorError::Missing => "The file no longer exists.".into(),
            EditorError::Io(message) => message.clone(),
        }
    }
}

pub(crate) struct LoadedFile {
    /// Text with `\n` line endings and no BOM.
    pub text: String,
    pub disk: DiskState,
}

/// Resolve symlinks so saves write the target and keep the link.
pub(crate) fn canonical_path(path: &str) -> Result<String, EditorError> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(crate::vfs::local::normalize_path(path)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(EditorError::Missing),
        Err(error) => Err(EditorError::Io(format!("Cannot resolve {path}: {error}"))),
    }
}

/// Check that `path` is something the editor can open, without reading it.
pub(crate) fn check_openable(path: &str) -> Result<(), EditorError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(EditorError::Missing),
        Err(error) => return Err(EditorError::Io(format!("Cannot read {path}: {error}"))),
    };
    if !metadata.is_file() {
        return Err(EditorError::NotRegularFile);
    }
    if metadata.len() > MAX_EDITOR_BYTES {
        return Err(EditorError::TooLarge);
    }
    Ok(())
}

fn read_bytes(path: &str) -> Result<Vec<u8>, EditorError> {
    check_openable(path)?;
    let bytes = std::fs::read(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => EditorError::Missing,
        _ => EditorError::Io(format!("Cannot read {path}: {error}")),
    })?;
    if bytes.len() as u64 > MAX_EDITOR_BYTES {
        return Err(EditorError::TooLarge);
    }
    Ok(bytes)
}

fn hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Split raw bytes into editor text plus the encoding details needed to write
/// them back unchanged.
pub(crate) fn decode(bytes: &[u8]) -> Result<(String, Eol, bool), EditorError> {
    let (bom, body) = match bytes.strip_prefix(UTF8_BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    let text = std::str::from_utf8(body).map_err(|_| EditorError::NotUtf8)?;
    let eol = if text.contains("\r\n") { Eol::Crlf } else { Eol::Lf };
    let text = if eol == Eol::Crlf { text.replace("\r\n", "\n") } else { text.to_string() };
    Ok((text, eol, bom))
}

/// The inverse of `decode`.
pub(crate) fn encode(text: &str, eol: Eol, bom: bool) -> Vec<u8> {
    let body = match eol {
        Eol::Lf => text.to_string(),
        Eol::Crlf => text.replace("\r\n", "\n").replace('\n', "\r\n"),
    };
    let mut bytes = Vec::with_capacity(body.len() + UTF8_BOM.len());
    if bom {
        bytes.extend_from_slice(UTF8_BOM);
    }
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

pub(crate) fn load(path: &str) -> Result<LoadedFile, EditorError> {
    let bytes = read_bytes(path)?;
    let (text, eol, bom) = decode(&bytes)?;
    Ok(LoadedFile {
        text,
        disk: DiskState { hash: hash(&bytes), eol, bom },
    })
}

/// External change status relative to what the editor last loaded or saved.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DiskChange {
    Unchanged,
    Modified,
    Missing,
}

/// Compare the file with `disk` by content. Size and mtime cannot decide it: a
/// same-second, same-size edit keeps them, and a bare `touch` changes them.
pub(crate) fn disk_change(path: &str, disk: &DiskState) -> Result<DiskChange, EditorError> {
    if file_fingerprint(path) == FileFingerprint::Missing {
        return Ok(DiskChange::Missing);
    }
    match read_bytes(path) {
        Ok(bytes) if hash(&bytes) == disk.hash => Ok(DiskChange::Unchanged),
        Ok(_) => Ok(DiskChange::Modified),
        Err(EditorError::Missing) => Ok(DiskChange::Missing),
        // A file that grew past the limit or stopped being text has changed.
        Err(EditorError::TooLarge | EditorError::NotUtf8 | EditorError::NotRegularFile) => {
            Ok(DiskChange::Modified)
        }
        Err(error) => Err(error),
    }
}

/// Whether replacing the file by rename would change its identity, so it must
/// be rewritten in place instead (the rule Vim's `backupcopy=auto` uses).
fn must_write_in_place(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid/getegid have no preconditions and cannot fail.
        let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
        metadata.nlink() > 1 || metadata.uid() != uid || metadata.gid() != gid
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn sync_file(path: &str) -> Result<(), EditorError> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| EditorError::Io(format!("Cannot flush {path}: {error}")))
}

fn write_all(writer: &mut dyn Write, bytes: &[u8], path: &str) -> Result<(), EditorError> {
    writer
        .write_all(bytes)
        .and_then(|()| writer.flush())
        .map_err(|error| EditorError::Io(format!("Cannot write {path}: {error}")))
}

fn temp_path_for(path: &str) -> Result<String, EditorError> {
    let target = Path::new(path);
    let parent = target
        .parent()
        .ok_or_else(|| EditorError::Io(format!("{path} has no parent folder.")))?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let suffix: u64 = rand::random();
    let temp: PathBuf = parent.join(format!(".{name}.fbro-tmp-{suffix:016x}"));
    Ok(crate::vfs::local::normalize_path(temp))
}

fn write_in_place(driver: &dyn VfsDriver, path: &str, bytes: &[u8]) -> Result<(), EditorError> {
    let mut writer = driver
        .open_write(path)
        .map_err(|error| EditorError::Io(format!("Cannot write {path}: {error}")))?;
    write_all(writer.as_mut(), bytes, path)?;
    drop(writer);
    sync_file(path)
}

fn rename_with_retry(driver: &dyn VfsDriver, temp: &str, path: &str) -> Result<bool, EditorError> {
    // Another program holding the file open makes the rename fail on Windows;
    // a short retry covers editors and indexers that briefly lock it.
    let attempts = if cfg!(windows) { 4 } else { 1 };
    let mut last_error = String::new();
    for attempt in 0..attempts {
        match driver.try_rename(temp, path) {
            Ok(renamed) => return Ok(renamed),
            Err(error) => last_error = error.to_string(),
        }
        if attempt + 1 < attempts {
            std::thread::sleep(std::time::Duration::from_millis(50 * (attempt as u64 + 1)));
        }
    }
    Err(EditorError::Io(format!("Cannot replace {path}: {last_error}")))
}

fn write_atomically(
    driver: &dyn VfsDriver,
    path: &str,
    bytes: &[u8],
    permissions: Option<std::fs::Permissions>,
) -> Result<(), EditorError> {
    let temp = temp_path_for(path)?;
    let result = (|| {
        let mut writer = driver
            .open_write_new(&temp)
            .map_err(|error| EditorError::Io(format!("Cannot create {temp}: {error}")))?;
        write_all(writer.as_mut(), bytes, &temp)?;
        drop(writer);
        if let Some(permissions) = permissions {
            std::fs::set_permissions(&temp, permissions)
                .map_err(|error| EditorError::Io(format!("Cannot set permissions on {temp}: {error}")))?;
        }
        sync_file(&temp)?;
        rename_with_retry(driver, &temp, path)
    })();
    match result {
        Ok(true) => Ok(()),
        Ok(false) => {
            // The driver cannot rename here; fall back to rewriting in place.
            let _ = driver.remove_file(&temp);
            write_in_place(driver, path, bytes)
        }
        Err(error) => {
            let _ = driver.remove_file(&temp);
            Err(error)
        }
    }
}

/// Save `text` to `path`. Unless `force`, refuses when the file no longer
/// matches `disk`. Returns the new disk state.
pub(crate) fn save(
    driver: &dyn VfsDriver,
    path: &str,
    text: &str,
    disk: &DiskState,
    force: bool,
) -> Result<DiskState, EditorError> {
    if !force {
        match disk_change(path, disk)? {
            DiskChange::Unchanged => {}
            DiskChange::Modified => return Err(EditorError::Stale),
            DiskChange::Missing => return Err(EditorError::Missing),
        }
    }
    let bytes = encode(text, disk.eol, disk.bom);
    match std::fs::metadata(path) {
        Ok(metadata) if !metadata.is_file() => return Err(EditorError::NotRegularFile),
        Ok(metadata) if must_write_in_place(&metadata) => write_in_place(driver, path, &bytes)?,
        Ok(metadata) => write_atomically(driver, path, &bytes, Some(metadata.permissions()))?,
        // Only reachable with `force`: write the file back.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_atomically(driver, path, &bytes, None)?
        }
        Err(error) => return Err(EditorError::Io(format!("Cannot read {path}: {error}"))),
    }
    Ok(DiskState {
        hash: hash(&bytes),
        eol: disk.eol,
        bom: disk.bom,
    })
}

/// Write `text` as a new file next to `path`, named
/// `<stem> (conflict <stamp>).<ext>`, never overwriting. Returns its path.
pub(crate) fn write_conflict_copy(
    driver: &dyn VfsDriver,
    path: &str,
    text: &str,
    disk: &DiskState,
    stamp: &str,
) -> Result<String, EditorError> {
    let target = Path::new(path);
    let parent = target
        .parent()
        .ok_or_else(|| EditorError::Io(format!("{path} has no parent folder.")))?;
    let stem = target.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = target.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let bytes = encode(text, disk.eol, disk.bom);
    for attempt in 1..=100 {
        let suffix = if attempt == 1 { String::new() } else { format!(" {attempt}") };
        let candidate = crate::vfs::local::normalize_path(
            parent.join(format!("{stem} (conflict {stamp}{suffix}){ext}")),
        );
        match driver.open_write_new(&candidate) {
            Ok(mut writer) => {
                write_all(writer.as_mut(), &bytes, &candidate)?;
                return Ok(candidate);
            }
            Err(crate::vfs::VfsError::AlreadyExists(_)) => continue,
            Err(error) => return Err(EditorError::Io(format!("Cannot create {candidate}: {error}"))),
        }
    }
    Err(EditorError::Io("Too many conflict copies already exist.".into()))
}

/// True when the file is still the empty file a new note started as.
pub(crate) fn is_untouched_empty_note(path: &str, disk: Option<&DiskState>) -> bool {
    let empty = std::fs::metadata(path).map(|m| m.is_file() && m.len() == 0).unwrap_or(false);
    match disk {
        Some(disk) => empty && disk.hash == hash(&[]),
        None => empty,
    }
}

/// Local timestamp used in generated file names.
pub(crate) fn file_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%d %H%M%S").to_string()
}

#[cfg(test)]
mod tests;
