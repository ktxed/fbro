//! Shared, conservative policy for speculative reads. Ordinary file access never
//! depends on this registry. Discovery only queries OS metadata, never benchmarks disks.
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(test)]
mod tests;
#[cfg(any(target_os = "windows", test))]
mod windows;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StorageClass {
    InternalSsd,
    Rotational,
    RemovableExternal,
    Remote,
    Virtual,
    #[default]
    Unknown,
}

impl StorageClass {
    pub fn eligible(self) -> bool {
        self == Self::InternalSsd
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct MountPolicy {
    pub root: PathBuf,
    /// Includes the mount/device identity, so replacement invalidates old reads.
    pub identity: String,
    pub class: StorageClass,
}

#[derive(Default)]
struct Snapshot {
    mounts: Vec<MountPolicy>,
    generation: u64,
}

#[derive(Default)]
pub struct StorageRegistry {
    snapshot: RwLock<Snapshot>,
}

/// Evidence attached to a speculative operation; invalid after a registry change.
pub struct ReadPermit {
    pub path: PathBuf,
    generation: u64,
}

impl ReadPermit {
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

pub fn registry() -> Arc<StorageRegistry> {
    static REGISTRY: OnceLock<Arc<StorageRegistry>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| Arc::new(StorageRegistry::default()))
        .clone()
}

impl StorageRegistry {
    pub fn generation(&self) -> u64 {
        self.snapshot.read().unwrap().generation
    }

    pub fn class_for_path(&self, path: &Path) -> StorageClass {
        let snapshot = self.snapshot.read().unwrap();
        classify_path(&snapshot.mounts, path)
    }

    pub fn source_class(&self, kind: &str, mounted: bool, root: &str) -> StorageClass {
        match kind {
            "remote" => StorageClass::Remote,
            "filesystem" if mounted => self.class_for_path(Path::new(root)),
            "filesystem" => StorageClass::Unknown,
            _ => StorageClass::Virtual,
        }
    }

    /// Called on blocking workers, never while holding the application-state lock.
    /// Reject known slow mounts before even resolving their paths. Then check the
    /// resolved target as well, so symlinks cannot inherit an SSD parent's policy.
    pub fn authorize(&self, path: &str) -> Option<ReadPermit> {
        let generation = self.generation();
        let class = self.class_for_path(Path::new(path));
        if !class.eligible() {
            eprintln!("[storage prefetch] skip path={path:?} class={class:?}");
            return None;
        }
        let resolved = std::fs::canonicalize(path).ok()?;
        if !self.class_for_path(&resolved).eligible() || generation != self.generation() {
            eprintln!("[storage prefetch] skip resolved target path={path:?} target={resolved:?}");
            return None;
        }
        Some(ReadPermit {
            path: resolved,
            generation,
        })
    }

    pub fn valid(&self, permit: &ReadPermit) -> bool {
        self.generation() == permit.generation
    }

    pub fn refresh(&self) -> bool {
        #[cfg(target_os = "linux")]
        let mounts = linux::discover();
        #[cfg(target_os = "macos")]
        let mounts = macos::discover();
        #[cfg(target_os = "windows")]
        let mounts = windows::discover();
        self.replace(mounts)
    }

    fn replace(&self, mut mounts: Vec<MountPolicy>) -> bool {
        mounts.sort();
        let mut snapshot = self.snapshot.write().unwrap();
        if snapshot.mounts == mounts {
            return false;
        }
        for mount in &mounts {
            if !snapshot.mounts.contains(mount) {
                eprintln!(
                    "[storage] root={:?} class={:?} prefetch={} device={:?}",
                    mount.root,
                    mount.class,
                    mount.class.eligible(),
                    mount.identity
                );
            }
        }
        snapshot.mounts = mounts;
        snapshot.generation += 1;
        true
    }

    #[cfg(test)]
    pub(crate) fn allow_test_root(&self, root: &Path) {
        self.set_test_root(root, StorageClass::InternalSsd);
    }

    #[cfg(test)]
    pub(crate) fn set_test_root(&self, root: &Path, class: StorageClass) {
        self.replace(vec![MountPolicy {
            root: root.to_path_buf(),
            identity: "test".into(),
            class,
        }]);
    }
}

fn classify_path(mounts: &[MountPolicy], path: &Path) -> StorageClass {
    #[cfg(target_os = "windows")]
    let normalized = PathBuf::from(
        path.to_string_lossy()
            .trim_start_matches("\\\\?\\")
            .replace('/', "\\")
            .to_lowercase(),
    );
    #[cfg(target_os = "windows")]
    let path = normalized.as_path();
    if !path.is_absolute()
        || path
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return StorageClass::Unknown;
    }
    // Component-aware matching prevents /media/card from matching /media/card2.
    // Mount enumeration includes filesystems hidden from the source bar.
    mounts
        .iter()
        .filter(|m| path.starts_with(&m.root))
        .max_by_key(|m| m.root.components().count())
        .map_or(StorageClass::Unknown, |m| m.class)
}

/// Every physical member must qualify (encrypted mappings, RAID, APFS containers).
pub(super) fn combine(classes: impl IntoIterator<Item = StorageClass>) -> StorageClass {
    let classes: Vec<_> = classes.into_iter().collect();
    if classes.is_empty() {
        return StorageClass::Unknown;
    }
    for rejected in [
        StorageClass::Remote,
        StorageClass::RemovableExternal,
        StorageClass::Rotational,
        StorageClass::Virtual,
        StorageClass::Unknown,
    ] {
        if classes.contains(&rejected) {
            return rejected;
        }
    }
    StorageClass::InternalSsd
}
