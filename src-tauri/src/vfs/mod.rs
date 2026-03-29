#![allow(dead_code)]

pub mod local;
#[cfg(unix)]
pub mod spy;

use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ws::protocol::{FilterSpec, SortSpec};

// ─── VFS trait ──────────────────────────────────────────────────────────────

/// Abstraction over a filesystem-like data source.
/// All file I/O in fbro goes through this trait so that the same job logic
/// works across local, remote, archive, and virtual sources.
pub trait VfsDriver: Send + Sync {
    // ── Driver identity ─────────────────────────────────────────────────────

    /// Returns true if this driver accesses the local native filesystem,
    /// meaning paths it exposes are valid `file://` URLs in the webview.
    /// Remote drivers (SFTP, S3, etc.) must return false.
    fn is_local(&self) -> bool { false }

    // ── Listing & navigation ────────────────────────────────────────────────

    fn list(
        &self,
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
    ) -> Result<Vec<VfsEntry>, VfsError>;

    fn resolve_child(&self, base: &str, name: &str) -> String;
    fn parent_path(&self, path: &str) -> Option<String>;

    // ── File I/O ────────────────────────────────────────────────────────────

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError>;
    fn open_write(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError>;
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

    /// Try an O(1) filesystem-level rename/move.
    /// Returns `Ok(true)` if the rename succeeded (e.g. same-filesystem move).
    /// Returns `Ok(false)` if this driver cannot do it (caller falls back to
    /// copy + delete). Returns `Err` on an actual I/O error.
    fn try_rename(&self, _src: &str, _dst: &str) -> Result<bool, VfsError> {
        Ok(false) // default: no fast path
    }
}

// ─── Helper types ───────────────────────────────────────────────────────────

/// Lightweight child entry returned by `read_dir_entries`.
pub struct DirChild {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
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
    Some(match ext.as_str() {
        // Languages
        "rs"                            => "text-rust",
        "py" | "pyw"                    => "text-x-python",
        "js" | "mjs" | "cjs"           => "application-javascript",
        "ts" | "tsx" | "jsx"            => "application-javascript",
        "java"                          => "text-x-java",
        "c" | "h"                       => "text-x-csrc",
        "cpp" | "cc" | "cxx" | "hpp"   => "text-x-c++src",
        "go"                            => "text-x-go",
        "rb"                            => "application-x-ruby",
        "sh" | "bash" | "zsh" | "fish" => "application-x-shellscript",
        "kt" | "kts"                    => "text-x-kotlin",
        "hs"                            => "text-x-haskell",
        "r"                             => "text-x-r",
        // Markup & text
        "html" | "htm"                  => "text-html",
        "css"                           => "text-css",
        "md" | "markdown"               => "text-markdown",
        "txt" | "log"                   => "text-x-generic",
        "json" | "yaml" | "yml" | "toml" | "xml" | "csv" | "ini" | "cfg" => "text-x-generic",
        // Documents
        "pdf"                           => "application-pdf",
        "doc" | "docx"                  => "application-msword",
        "xls" | "xlsx"                  => "application-vnd.ms-excel",
        "ppt" | "pptx"                  => "application-vnd.ms-powerpoint",
        // Images
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tiff" | "tif"
                                        => "image-x-generic",
        // Audio
        "mp3" | "flac" | "ogg" | "wav" | "aac" | "m4a" | "wma" | "opus"
                                        => "audio-x-generic",
        // Video
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm"
                                        => "video-x-generic",
        // Archives
        "zip"                           => "application-zip",
        "tar"                           => "application-x-tar",
        "gz" | "bz2" | "xz" | "zst"   => "application-x-tar",
        "7z"                            => "application-7z",
        "rar"                           => "application-x-rar",
        "deb"                           => "application-vnd.debian.binary-package",
        // Executables & objects
        "exe" | "bin" | "appimage"      => "application-x-executable",
        "so" | "dylib" | "dll" | "o"   => "application-x-object",
        // Fallback
        _                               => "text-x-generic",
    }.to_string())
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
