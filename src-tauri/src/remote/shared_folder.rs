//! Validation and driver lifetime for user-entered local folder shares.

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::state::{share_root_id_for_path, SharedRoot, SharedRootOrigin};
use crate::vfs::{local::LocalFsDriver, VfsDriver};

const DRIVER_PREFIX: &str = "share-folder:";

#[derive(Debug)]
pub struct FolderError {
    pub code: &'static str,
    pub message: String,
}

fn io_error(path: &Path, error: io::Error) -> FolderError {
    FolderError {
        code: match error.kind() {
            io::ErrorKind::NotFound => "FOLDER_NOT_FOUND",
            io::ErrorKind::PermissionDenied => "FOLDER_ACCESS_DENIED",
            _ => "FOLDER_UNAVAILABLE",
        },
        message: format!("Cannot share {}: {error}", path.display()),
    }
}

/// Call on a blocking worker. An existing canonical-equivalent share wins.
pub fn validate(
    raw: &str,
    base: &str,
    home: &str,
    existing: &[SharedRoot],
) -> Result<SharedRoot, FolderError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(FolderError {
            code: "FOLDER_PATH_REQUIRED",
            message: "Enter a folder path.".into(),
        });
    }
    let expanded = if raw == "~" {
        PathBuf::from(home)
    } else if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        Path::new(home).join(rest)
    } else {
        PathBuf::from(raw)
    };
    let requested = if expanded.is_absolute() {
        expanded
    } else {
        Path::new(base).join(expanded)
    };
    let canonical = std::fs::canonicalize(&requested).map_err(|e| io_error(&requested, e))?;
    if !canonical.is_dir() {
        return Err(FolderError {
            code: "NOT_A_FOLDER",
            message: format!("{} is not a folder.", requested.display()),
        });
    }
    // Use a real access attempt, rather than guessing from permission bits/ACLs.
    std::fs::read_dir(&canonical).map_err(|e| io_error(&requested, e))?;
    for root in existing {
        if std::fs::canonicalize(&root.path).ok().as_ref() == Some(&canonical) {
            return Ok(root.clone());
        }
    }
    let path = crate::vfs::local::normalize_path(canonical);
    let origin = SharedRootOrigin::Folder;
    let id = share_root_id_for_path(&origin, &path);
    let name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(&path)
        .to_string();
    Ok(SharedRoot {
        source_id: format!("{DRIVER_PREFIX}{id}"),
        id,
        name,
        path,
        origin,
        icon_name: Some("folder".into()),
    })
}

/// Folder drivers are internal entries, independent of the volume inventory.
pub fn sync_drivers(drivers: &mut HashMap<String, Arc<dyn VfsDriver>>, roots: &[SharedRoot]) {
    let wanted: HashSet<&str> = roots
        .iter()
        .filter(|root| matches!(root.origin, SharedRootOrigin::Folder))
        .map(|root| root.source_id.as_str())
        .collect();
    drivers.retain(|id, _| !id.starts_with(DRIVER_PREFIX) || wanted.contains(id.as_str()));
    for id in wanted {
        drivers
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(LocalFsDriver));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("fbro-shared-folder-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> String {
            self.0.to_str().unwrap().to_string()
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn shared_folder_validates_without_registered_sources_and_normalizes_paths() {
        let dir = TempDir::new();
        std::fs::create_dir(dir.0.join("child")).unwrap();
        let root = validate("child/../child", &dir.path(), &dir.path(), &[])
            .ok()
            .unwrap();
        assert_eq!(
            root.path,
            crate::vfs::local::normalize_path(std::fs::canonicalize(dir.0.join("child")).unwrap())
        );
        assert_eq!(root.name, "child");
        assert_eq!(root.origin, SharedRootOrigin::Folder);
        assert!(root.source_id.starts_with(DRIVER_PREFIX));
        assert_eq!(
            validate("~/child", "/unused", &dir.path(), &[])
                .ok()
                .unwrap(),
            root
        );
    }

    #[test]
    fn shared_folder_rejects_empty_missing_and_file_paths() {
        let dir = TempDir::new();
        std::fs::write(dir.0.join("file"), "payload").unwrap();
        for (path, code) in [
            (" ", "FOLDER_PATH_REQUIRED"),
            ("missing", "FOLDER_NOT_FOUND"),
            ("file", "NOT_A_FOLDER"),
        ] {
            assert_eq!(
                validate(path, &dir.path(), &dir.path(), &[])
                    .err()
                    .unwrap()
                    .code,
                code
            );
        }
    }

    #[test]
    fn shared_folder_deduplicates_existing_home_and_folder_shares() {
        let dir = TempDir::new();
        let folder = validate(".", &dir.path(), &dir.path(), &[]).ok().unwrap();
        let mut home = folder.clone();
        home.id = "home-existing".into();
        home.origin = SharedRootOrigin::Home;
        assert_eq!(
            validate("~/.", &dir.path(), &dir.path(), &[home.clone()])
                .ok()
                .unwrap(),
            home
        );
        assert_eq!(
            validate(".", &dir.path(), &dir.path(), &[folder.clone()])
                .ok()
                .unwrap(),
            folder
        );
        #[cfg(unix)]
        {
            let alias = dir.0.join("alias");
            std::os::unix::fs::symlink(&dir.0, &alias).unwrap();
            assert_eq!(
                validate(
                    alias.to_str().unwrap(),
                    &dir.path(),
                    &dir.path(),
                    &[folder.clone()]
                )
                .unwrap(),
                folder
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn shared_folder_denies_inaccessible_directory() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = TempDir::new();
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
            }
        }
        let _restore = Restore(dir.0.clone());
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0)).unwrap();
        assert_eq!(
            validate(".", &dir.path(), &dir.path(), &[])
                .err()
                .unwrap()
                .code,
            "FOLDER_ACCESS_DENIED"
        );
    }

    #[test]
    fn shared_folder_drivers_restore_and_remove_without_touching_other_drivers() {
        let dir = TempDir::new();
        let root = validate(".", &dir.path(), &dir.path(), &[]).ok().unwrap();
        drop(dir);
        assert!(!Path::new(&root.path).exists());
        let restored = crate::state::migrate_saved_shared_roots(
            &HashMap::new(),
            &HashSet::new(),
            vec![root.clone()],
        );
        assert_eq!(restored, vec![root.clone()]);
        let mut drivers: HashMap<String, Arc<dyn VfsDriver>> = HashMap::new();
        drivers.insert("volume".into(), Arc::new(LocalFsDriver));
        sync_drivers(&mut drivers, &restored);
        assert!(drivers.get(&root.source_id).unwrap().is_local());
        sync_drivers(&mut drivers, &[]);
        assert!(!drivers.contains_key(&root.source_id));
        assert!(drivers.contains_key("volume"));
    }
}
