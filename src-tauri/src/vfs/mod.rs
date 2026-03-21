#![allow(dead_code)]

pub mod local;

use std::io::{Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ws::protocol::{FilterSpec, SortSpec};

// ─── VFS trait ──────────────────────────────────────────────────────────────

/// Abstraction over a filesystem-like data source.
/// All file I/O in fbro goes through this trait so that the same job logic
/// works across local, remote, archive, and virtual sources.
pub trait VfsDriver: Send + Sync {
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
