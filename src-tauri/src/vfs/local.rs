use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use super::{
    color_class_for_entry, format_mtime, icon_name_for_entry, DirChild, VfsDriver, VfsEntry,
    VfsError, VfsKind,
};
use crate::ws::protocol::{FilterSpec, SortSpec};

pub struct LocalFsDriver;

fn normalize_path(path: std::path::PathBuf) -> String {
    #[cfg(windows)]
    return path.to_string_lossy().replace('\\', "/");
    #[cfg(not(windows))]
    path.to_string_lossy().to_string()
}

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
    Some(
        if meta.permissions().readonly() {
            "r-"
        } else {
            "rw"
        }
        .to_string(),
    )
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
    fn is_local(&self) -> bool {
        true
    }

    fn supports_thumbnails(&self) -> bool {
        true
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    // ── Listing & navigation ────────────────────────────────────────────────

    fn list(
        &self,
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
        show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        let dir = fs::read_dir(path).map_err(|e| map_io_err(path, e))?;

        // Filter out hidden files unless show_hidden is true
        let hide_hidden = !show_hidden;

        // Build the filter regex once before iterating entries.
        // Pattern syntax: glob-like with * as wildcard, | as OR separator.
        // Parts with * are anchored full-match; parts without * are substring searches.
        // Matching is case-insensitive.
        let filter_re: Option<regex::Regex> = filter.and_then(|f| {
            if f.mode != "glob" || f.value.trim().is_empty() {
                return None;
            }
            let regex_str = f
                .value
                .split('|')
                .map(|part| {
                    let part = part.trim();
                    if part.contains('*') {
                        // Glob: anchor and convert * to .*
                        let escaped: String = part
                            .chars()
                            .map(|ch| match ch {
                                '*' => ".*".to_string(),
                                '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$'
                                | '\\' => format!("\\{ch}"),
                                _ => ch.to_string(),
                            })
                            .collect();
                        format!("^{escaped}$")
                    } else {
                        // Substring: escape only, match anywhere
                        part.chars()
                            .map(|ch| match ch {
                                '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$'
                                | '|' | '\\' | '*' => format!("\\{ch}"),
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
        });

        let mut entries: Vec<VfsEntry> = Vec::new();

        for entry_result in dir {
            let entry = match entry_result {
                Ok(e) => e,
                Err(_) => continue,
            };

            let name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files unless show_hidden is enabled
            if hide_hidden && name.starts_with('.') && name != ".." {
                continue;
            }

            if let Some(re) = &filter_re {
                if !re.is_match(&name) {
                    continue;
                }
            }

            let meta = entry.metadata().ok();
            let (kind, size, mtime, btime, permissions) = if let Some(m) = meta {
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
                let btime = m.created().ok().and_then(format_mtime);
                let permissions = format_permissions(&m);
                (kind, size, mtime, btime, permissions)
            } else {
                (VfsKind::Other, None, None, None, None)
            };

            let icon_name = icon_name_for_entry(&name, &kind);
            let cc = color_class_for_entry(&name, &kind);
            entries.push(VfsEntry {
                name,
                kind,
                size,
                mtime,
                btime,
                permissions,
                icon_name,
                cc,
                source_path: None,
            });
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

        Ok(entries)
    }

    fn resolve_child(&self, base: &str, name: &str) -> String {
        normalize_path(Path::new(base).join(name))
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        Path::new(path)
            .parent()
            .map(|p| normalize_path(p.to_path_buf()))
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
        file.seek(SeekFrom::Start(offset))
            .map_err(|e| map_io_err(path, e))?;
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
            let mtime = meta.as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            let permissions = meta.as_ref().and_then(format_permissions);
            children.push(DirChild { name, is_dir, size, mtime, permissions });
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
                // Cross-device move — not an error, caller falls back to copy + delete
                let is_cross_device = {
                    #[cfg(windows)]
                    {
                        e.raw_os_error() == Some(17)
                    } // ERROR_NOT_SAME_DEVICE
                    #[cfg(not(windows))]
                    {
                        e.raw_os_error() == Some(18)
                    } // EXDEV
                };
                if is_cross_device {
                    Ok(false)
                } else {
                    Err(map_io_err(src, e))
                }
            }
        }
    }

    // ── Permissions ─────────────────────────────────────────────────────────

    fn set_permissions(&self, path: &str, mode: u32) -> Result<(), VfsError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .map_err(|e| map_io_err(path, e))
        }
        #[cfg(not(unix))]
        {
            let _ = mode;
            Err(VfsError::NotSupported(
                "set_permissions is only supported on Unix".to_string(),
            ))
        }
    }
}
