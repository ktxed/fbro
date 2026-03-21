#![allow(dead_code)]

pub mod local;

use std::time::{SystemTime, UNIX_EPOCH};

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
        // Minimal ISO-ish timestamp: seconds since epoch as a unix timestamp string
        // A real app would use chrono; this is sufficient for display
        format!("{secs}")
    })
}
