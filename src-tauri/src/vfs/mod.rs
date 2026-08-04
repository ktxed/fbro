#![allow(dead_code)]

pub mod bookmarks;
pub mod global_selection;
pub mod git_blob;
pub mod git_diff;
pub mod history;
pub mod local;
pub mod procs;
pub mod search;
pub mod seven_zip;
pub mod sftp;
#[cfg(unix)]
pub mod spy;
pub mod zip;

use std::io::{Read, Write};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ws::protocol::{FilterSpec, SortSpec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VfsTransferDigest {
    pub algorithm: String,
    pub size: u64,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VfsManifestEntry {
    pub path: String,
    pub size: u64,
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VfsManifestError {
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VfsManifest {
    pub entries: Vec<VfsManifestEntry>,
    pub errors: Vec<VfsManifestError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VfsManifestProgress {
    pub processed: usize,
    pub current_path: Option<String>,
    pub current_file_bytes_total: u64,
    pub current_file_bytes_hashed: u64,
    pub remote: bool,
}

pub struct VfsManifestOptions<'a> {
    pub cancel: &'a AtomicBool,
    pub progress: Option<&'a (dyn Fn(VfsManifestProgress) + Send + Sync)>,
}

/// BLAKE3 hex digest of a local file using a memory map and multi-core hashing.
///
/// `cancel` is checked between hash windows so a single very large file stays
/// cancellable; `progress` reports bytes hashed so far for the FE progress bar.
/// Chunked `update_rayon` over the mapping is bit-identical to a single
/// streaming `blake3::Hasher`, so the digest stays comparable with the remote
/// side of Sync Bro (which produces the same BLAKE3 hex).
pub fn hash_local_file_blake3(
    path: &std::path::Path,
    cancel: &dyn Fn() -> bool,
    mut progress: impl FnMut(u64),
) -> Result<String, VfsError> {
    // Window large enough to amortize rayon dispatch, small enough to keep
    // cancellation and progress responsive on multi-GB files.
    const HASH_WINDOW: usize = 64 * 1024 * 1024;

    let file = std::fs::File::open(path).map_err(|e| VfsError::IOError(e.to_string()))?;
    let meta = file
        .metadata()
        .map_err(|e| VfsError::IOError(e.to_string()))?;
    if !meta.is_file() {
        return Err(VfsError::NotSupported(format!(
            "not a regular file: {}",
            path.display()
        )));
    }
    let mut hasher = blake3::Hasher::new();
    // Empty files cannot be memory-mapped; hash the empty input directly.
    if meta.len() == 0 {
        progress(0);
        return Ok(hasher.finalize().to_hex().to_string());
    }
    // SAFETY: mmap of a regular file. Concurrent truncation of the mapped file
    // could raise SIGBUS — an accepted risk for a local integrity tool.
    let mmap =
        unsafe { memmap2::Mmap::map(&file) }.map_err(|e| VfsError::IOError(e.to_string()))?;
    let len = mmap.len();
    let mut offset = 0usize;
    while offset < len {
        if cancel() {
            return Err(VfsError::IOError("Cancelled".to_string()));
        }
        progress(offset as u64);
        let end = (offset + HASH_WINDOW).min(len);
        hasher.update_rayon(&mmap[offset..end]);
        offset = end;
    }
    progress(len as u64);
    Ok(hasher.finalize().to_hex().to_string())
}

// ─── VFS trait ──────────────────────────────────────────────────────────────

/// Abstraction over a filesystem-like data source.
/// All file I/O in fbro goes through this trait so that the same job logic
/// works across local, remote, archive, and virtual sources.
pub trait VfsDriver: Send + Sync {
    // ── Driver identity ─────────────────────────────────────────────────────

    /// Returns true if this driver accesses the local native filesystem,
    /// meaning paths it exposes are valid `file://` URLs in the webview.
    /// Remote drivers (SFTP, S3, etc.) must return false.
    fn is_local(&self) -> bool {
        false
    }

    /// Returns true if entries from this driver have real OS file paths
    /// suitable for OS-native thumbnail providers. LocalFs and search-results
    /// (which carry `source_path` per entry) qualify; archives, bookmarks,
    /// and history do not.
    fn supports_thumbnails(&self) -> bool {
        false
    }

    /// Downcast helper for runtime type checking (e.g. detecting ZipFsDriver).
    fn as_any(&self) -> &dyn std::any::Any;

    /// Return the real on-disk path for `path` when this driver maps directly
    /// to the local native filesystem. Enables fast memory-mapped, multi-core
    /// hashing (see [`hash_local_file_blake3`]). Drivers without real OS paths
    /// (archives, remote, virtual) return `None`.
    fn local_path(&self, _path: &str) -> Option<std::path::PathBuf> {
        None
    }

    /// Optional hook for drivers that need to scope backend resources to a
    /// file-operation job. Most VFS drivers do not need this.
    fn begin_file_op(&self, _job_id: &str) {}

    /// Complements `begin_file_op`.
    fn end_file_op(&self, _job_id: &str) {}

    // ── Listing & navigation ────────────────────────────────────────────────

    fn list(
        &self,
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
        show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError>;

    fn resolve_child(&self, base: &str, name: &str) -> String;
    fn parent_path(&self, path: &str) -> Option<String>;

    // ── File I/O ────────────────────────────────────────────────────────────

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError>;
    fn supports_read_at(&self) -> bool {
        false
    }
    fn open_read_at(&self, path: &str, offset: u64) -> Result<Box<dyn Read + Send>, VfsError> {
        if offset == 0 {
            self.open_read(path)
        } else {
            Err(VfsError::NotSupported(
                "offset reads are not supported by this driver".to_string(),
            ))
        }
    }
    fn open_write(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError>;
    fn supports_write_at(&self) -> bool {
        false
    }
    fn open_write_at(&self, path: &str, offset: u64) -> Result<Box<dyn Write + Send>, VfsError> {
        if offset == 0 {
            self.open_write(path)
        } else {
            Err(VfsError::NotSupported(
                "offset writes are not supported by this driver".to_string(),
            ))
        }
    }
    fn truncate_file(&self, _path: &str, _len: u64) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "truncate is not supported by this driver".to_string(),
        ))
    }
    fn create_dir_all(&self, path: &str) -> Result<(), VfsError>;
    fn remove_file(&self, path: &str) -> Result<(), VfsError>;
    fn remove_dir_all(&self, path: &str) -> Result<(), VfsError>;
    fn exists(&self, path: &str) -> bool;
    fn file_size(&self, path: &str) -> Option<u64>;

    /// List direct children (name, is_dir, size). Used by recursive copy/delete.
    fn read_dir_entries(&self, path: &str) -> Result<Vec<DirChild>, VfsError>;

    // ── Same-driver fast path ───────────────────────────────────────────────

    /// Read up to `size` bytes from `path` starting at `offset`.
    /// Returns fewer bytes than requested only at EOF.
    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError>;

    /// Return a transfer-scoped digest after a file-operation read. Drivers
    /// that do not participate in fbro remote-integrity checks return `None`.
    fn transfer_digest(
        &self,
        _job_id: &str,
        _path: &str,
    ) -> Result<Option<VfsTransferDigest>, VfsError> {
        Ok(None)
    }

    fn file_digest(&self, _path: &str) -> Result<Option<VfsTransferDigest>, VfsError> {
        Ok(None)
    }

    fn remote_manifest(
        &self,
        _path: &str,
        _options: &VfsManifestOptions<'_>,
    ) -> Result<Option<VfsManifest>, VfsError> {
        Ok(None)
    }

    /// Try an O(1) filesystem-level rename/move.
    /// Returns `Ok(true)` if the rename succeeded (e.g. same-filesystem move).
    /// Returns `Ok(false)` if this driver cannot do it (caller falls back to
    /// copy + delete). Returns `Err` on an actual I/O error.
    fn try_rename(&self, _src: &str, _dst: &str) -> Result<bool, VfsError> {
        Ok(false) // default: no fast path
    }

    /// Set Unix permissions (mode bits) on the file at `path`.
    /// Returns `NotSupported` by default — only `LocalFsDriver` implements this.
    fn set_permissions(&self, _path: &str, _mode: u32) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "set_permissions is not supported by this driver".to_string(),
        ))
    }
}

// ─── Helper types ───────────────────────────────────────────────────────────

/// Lightweight child entry returned by `read_dir_entries`.
pub struct DirChild {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: Option<u64>,
    pub permissions: Option<String>,
}

// ─── Domain types ───────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct VfsEntry {
    pub name: String,
    pub kind: VfsKind,
    pub size: Option<u64>,
    pub mtime: Option<String>,
    pub btime: Option<String>, // birth time — file creation; not available on all platforms/filesystems
    pub permissions: Option<String>,
    pub icon_name: Option<String>,
    /// Color class hint for the FE — a short CSS class name derived from the
    /// entry kind and file extension (e.g. "cc-dir", "cc-archive").
    pub cc: Option<String>,
    /// Real absolute path when the entry's logical `name` differs from its
    /// filesystem path (e.g. search results). `None` for all normal entries.
    pub source_path: Option<String>,
}

/// Derive a freedesktop icon name from the entry kind and file extension.
/// No content/MIME scanning — purely extension-based.
pub fn icon_name_for_entry(name: &str, kind: &VfsKind) -> Option<String> {
    match kind {
        VfsKind::Directory => return Some("folder".to_string()),
        VfsKind::Symlink => return Some("emblem-symbolic-link".to_string()),
        VfsKind::Other => return Some("application-x-generic".to_string()),
        VfsKind::File => {}
    }
    let ext = name.rsplit('.').next()?.to_lowercase();
    Some(
        match ext.as_str() {
            // Languages
            "rs" => "text-rust",
            "py" | "pyw" => "text-x-python",
            "js" | "mjs" | "cjs" => "application-javascript",
            "ts" | "tsx" | "jsx" => "application-javascript",
            "java" => "text-x-java",
            "c" | "h" => "text-x-csrc",
            "cpp" | "cc" | "cxx" | "hpp" => "text-x-c++src",
            "go" => "text-x-go",
            "rb" => "application-x-ruby",
            "sh" | "bash" | "zsh" | "fish" => "application-x-shellscript",
            "kt" | "kts" => "text-x-kotlin",
            "hs" => "text-x-haskell",
            "r" => "text-x-r",
            // Markup & text
            "html" | "htm" => "fbro-ie-html",
            "css" => "text-css",
            "md" | "markdown" => "text-markdown",
            "txt" | "log" => "text-x-generic",
            "json" | "yaml" | "yml" | "toml" | "xml" | "csv" | "ini" | "cfg" => "text-x-generic",
            // Documents
            "pdf" => "application-pdf",
            "doc" | "docx" => "application-msword",
            "xls" | "xlsx" => "application-vnd.ms-excel",
            "ppt" | "pptx" => "application-vnd.ms-powerpoint",
            // Images
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tiff" | "tif" => {
                "image-x-generic"
            }
            // Audio
            "mp3" | "flac" | "ogg" | "wav" | "aac" | "m4a" | "wma" | "opus" => "audio-x-generic",
            // Video
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" => "video-x-generic",
            // Archives
            "zip" => "application-zip",
            "tar" => "application-x-tar",
            "gz" | "bz2" | "xz" | "zst" => "application-x-tar",
            "7z" => "application-7z",
            "rar" => "application-x-rar",
            "deb" => "application-vnd.debian.binary-package",
            // Executables & objects
            "exe" | "bin" | "appimage" => "application-x-executable",
            "so" | "dylib" | "dll" | "o" => "application-x-object",
            // Fallback
            _ => "text-x-generic",
        }
        .to_string(),
    )
}

/// Derive a color class from the entry kind and file extension.
/// Returns a short CSS class name like "cc-dir", "cc-archive", etc.
pub fn color_class_for_entry(name: &str, kind: &VfsKind) -> Option<String> {
    // Dot files and dot folders get a dedicated dim style.
    if name.starts_with('.') && name != ".." {
        return Some("cc-dot".to_string());
    }
    match kind {
        VfsKind::Directory => return Some("cc-dir".to_string()),
        VfsKind::Symlink => return Some("cc-link".to_string()),
        VfsKind::Other => return None,
        VfsKind::File => {}
    }
    let ext = name.rsplit('.').next()?.to_lowercase();
    match ext.as_str() {
        // Archives
        "zip" | "tar" | "gz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "deb" | "rpm" | "tgz"
        | "tbz2" | "txz" | "lz" | "lzma" | "cab" | "iso" | "dmg" | "apk" | "jar" | "war"
        | "ear" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" | "xpi" | "crx" => {
            Some("cc-archive".to_string())
        }
        // Executables / scripts
        "sh" | "bash" | "zsh" | "fish" | "exe" | "bin" | "appimage" | "cmd" | "bat" | "ps1" => {
            Some("cc-exec".to_string())
        }
        // Images, audio, video
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tiff" | "tif"
        | "mp3" | "flac" | "ogg" | "wav" | "aac" | "m4a" | "wma" | "opus" | "mp4" | "mkv"
        | "avi" | "mov" | "wmv" | "flv" | "webm" => Some("cc-media".to_string()),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum VfsKind {
    File,
    Directory,
    Symlink,
    Other,
}

impl VfsKind {
    pub fn as_str(&self) -> &str {
        match self {
            VfsKind::File => "file",
            VfsKind::Directory => "directory",
            VfsKind::Symlink => "symlink",
            VfsKind::Other => "other",
        }
    }
}

#[derive(Debug)]
pub enum VfsError {
    NotFound(String),
    PermissionDenied(String),
    AlreadyExists(String),
    NotSupported(String),
    IOError(String),
}

impl std::fmt::Display for VfsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VfsError::NotFound(s) => write!(f, "NotFound: {s}"),
            VfsError::PermissionDenied(s) => write!(f, "PermissionDenied: {s}"),
            VfsError::AlreadyExists(s) => write!(f, "AlreadyExists: {s}"),
            VfsError::NotSupported(s) => write!(f, "NotSupported: {s}"),
            VfsError::IOError(s) => write!(f, "IOError: {s}"),
        }
    }
}

pub fn format_mtime(t: SystemTime) -> Option<String> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| {
        let secs = d.as_secs();
        format!("{secs}")
    })
}

#[cfg(test)]
mod hash_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TempFile(std::path::PathBuf);
    impl TempFile {
        fn with_bytes(bytes: &[u8]) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path =
                std::env::temp_dir().join(format!("fbro-hash-test-{}-{n}", std::process::id()));
            std::fs::write(&path, bytes).expect("write temp file");
            TempFile(path)
        }
    }
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn no_cancel() -> bool {
        false
    }

    #[test]
    fn matches_canonical_blake3_for_nonempty_file() {
        let bytes = b"the quick brown fox jumps over the lazy dog".repeat(100);
        let file = TempFile::with_bytes(&bytes);
        let expected = blake3::hash(&bytes).to_hex().to_string();

        let mut last_progress = 0u64;
        let got =
            hash_local_file_blake3(&file.0, &no_cancel, |hashed| last_progress = hashed).unwrap();

        assert_eq!(got, expected);
        assert_eq!(last_progress, bytes.len() as u64);
    }

    #[test]
    fn empty_file_hashes_empty_input() {
        let file = TempFile::with_bytes(b"");
        let expected = blake3::hash(b"").to_hex().to_string();

        let got = hash_local_file_blake3(&file.0, &no_cancel, |_| {}).unwrap();

        assert_eq!(got, expected);
    }

    #[test]
    fn non_regular_file_is_not_supported() {
        // A directory is not a regular file.
        let dir = std::env::temp_dir();
        let err = hash_local_file_blake3(&dir, &no_cancel, |_| {}).unwrap_err();
        assert!(matches!(err, VfsError::NotSupported(_)));
    }

    #[test]
    fn cancellation_mid_hash_returns_cancelled() {
        let bytes = b"some bytes to hash".repeat(50);
        let file = TempFile::with_bytes(&bytes);

        let err = hash_local_file_blake3(&file.0, &|| true, |_| {}).unwrap_err();
        assert!(err.to_string().contains("Cancelled"));
    }
}
