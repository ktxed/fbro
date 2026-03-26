use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::{format_mtime, icon_name_for_entry, DirChild, VfsDriver, VfsEntry, VfsError, VfsKind};
use crate::ws::protocol::{FilterSpec, SortSpec};

pub struct LocalFsDriver;

#[cfg(unix)]
fn format_permissions(meta: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode();
    let bits = |shift: u32| -> [char; 3] {
        let b = (mode >> shift) & 7;
        [
            if b & 4 != 0 { 'r' } else { '-' },
            if b & 2 != 0 { 'w' } else { '-' },
            if b & 1 != 0 { 'x' } else { '-' },
        ]
    };
    let [ur, uw, ux] = bits(6);
    let [gr, gw, gx] = bits(3);
    let [or, ow, ox] = bits(0);
    Some(format!("{ur}{uw}{ux}{gr}{gw}{gx}{or}{ow}{ox}"))
}

#[cfg(not(unix))]
fn format_permissions(meta: &fs::Metadata) -> Option<String> {
    Some(if meta.permissions().readonly() { "r-" } else { "rw" }.to_string())
}

fn map_io_err(path: &str, e: std::io::Error) -> VfsError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => VfsError::PermissionDenied(e.to_string()),
        std::io::ErrorKind::NotFound => VfsError::NotFound(path.to_string()),
        std::io::ErrorKind::AlreadyExists => VfsError::AlreadyExists(path.to_string()),
        _ => VfsError::IOError(e.to_string()),
    }
}

impl VfsDriver for LocalFsDriver {
    // ── Listing & navigation ────────────────────────────────────────────────

    fn list(
        &self,
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        let dir = fs::read_dir(path).map_err(|e| map_io_err(path, e))?;

        let mut entries: Vec<VfsEntry> = Vec::new();

        for entry_result in dir {
            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.file_name().to_string_lossy().to_string();

            if let Some(f) = filter {
                match f.mode.as_str() {
                    "prefix" => {
                        if !name.starts_with(&f.value) {
                            continue;
                        }
                    }
                    _ => {}
                }
            }

            let meta = entry.metadata().ok();
            let (kind, size, mtime, permissions) = if let Some(m) = meta {
                let kind = if m.is_symlink() {
                    VfsKind::Symlink
                } else if m.is_dir() {
                    VfsKind::Directory
                } else if m.is_file() {
                    VfsKind::File
                } else {
                    VfsKind::Other
                };
                let size = if m.is_file() { Some(m.len()) } else { None };
                let mtime = m.modified().ok().and_then(format_mtime);
                let permissions = format_permissions(&m);
                (kind, size, mtime, permissions)
            } else {
                (VfsKind::Other, None, None, None)
            };

            let icon_name = icon_name_for_entry(&name, &kind);
            entries.push(VfsEntry { name, kind, size, mtime, permissions, icon_name });
        }

        entries.sort_by(|a, b| {
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
                        e.name.rfind('.').map(|i| e.name[i+1..].to_lowercase()).unwrap_or_default()
                    };
                    ext(a).cmp(&ext(b))
                }
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            if sort.order == "desc" { ord.reverse() } else { ord }
        });

        Ok(entries)
    }

    fn resolve_child(&self, base: &str, name: &str) -> String {
        Path::new(base).join(name).to_string_lossy().to_string()
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
    }

    // ── File I/O ────────────────────────────────────────────────────────────

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        let f = fs::File::open(path).map_err(|e| map_io_err(path, e))?;
        Ok(Box::new(f))
    }

    fn open_write(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        let f = fs::File::create(path).map_err(|e| map_io_err(path, e))?;
        Ok(Box::new(f))
    }

    fn create_dir_all(&self, path: &str) -> Result<(), VfsError> {
        fs::create_dir_all(path).map_err(|e| map_io_err(path, e))
    }

    fn remove_file(&self, path: &str) -> Result<(), VfsError> {
        fs::remove_file(path).map_err(|e| map_io_err(path, e))
    }

    fn remove_dir_all(&self, path: &str) -> Result<(), VfsError> {
        fs::remove_dir_all(path).map_err(|e| map_io_err(path, e))
    }

    fn exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn file_size(&self, path: &str) -> Option<u64> {
        fs::metadata(path).ok().map(|m| m.len())
    }

    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
        let mut file = fs::File::open(path).map_err(|e| map_io_err(path, e))?;
        file.seek(SeekFrom::Start(offset)).map_err(|e| map_io_err(path, e))?;
        let mut buf = vec![0u8; size];
        let n = file.read(&mut buf).map_err(|e| map_io_err(path, e))?;
        buf.truncate(n);
        Ok(buf)
    }

    fn read_dir_entries(&self, path: &str) -> Result<Vec<DirChild>, VfsError> {
        let dir = fs::read_dir(path).map_err(|e| map_io_err(path, e))?;
        let mut children = Vec::new();
        for entry_result in dir {
            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => continue,
            };
            let name = entry.file_name().to_string_lossy().to_string();
            let meta = entry.metadata().ok();
            let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
            children.push(DirChild { name, is_dir, size });
        }
        Ok(children)
    }

    // ── Same-driver fast path ───────────────────────────────────────────────

    fn try_rename(&self, src: &str, dst: &str) -> Result<bool, VfsError> {
        // std::fs::rename succeeds instantly on same filesystem,
        // returns EXDEV (CrossesDevices / not supported) across filesystems.
        match fs::rename(src, dst) {
            Ok(()) => Ok(true),
            Err(e) => {
                // EXDEV / cross-device link — not an error, just means
                // the caller should fall back to copy + delete
                if e.raw_os_error() == Some(18) {
                    // 18 = EXDEV on Linux
                    Ok(false)
                } else {
                    Err(map_io_err(src, e))
                }
            }
        }
    }
}
