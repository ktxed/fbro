use super::{combine, MountPolicy, StorageClass};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) fn discover() -> Vec<MountPolicy> {
    let Ok(contents) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return vec![];
    };
    parse_mounts(&contents, Path::new("/sys"))
}

fn parse_mounts(contents: &str, sys: &Path) -> Vec<MountPolicy> {
    let mut devices = HashMap::new();
    let mut mounts = HashMap::new();
    for line in contents.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<_> = left.split_whitespace().collect();
        let fs: Vec<_> = right.split_whitespace().collect();
        if fields.len() < 6 || fs.len() < 2 {
            continue;
        }
        let root = PathBuf::from(unescape(fields[4]));
        let (class, device_identity) = devices.entry(fields[2].to_string()).or_insert_with(|| {
            let device = std::fs::canonicalize(sys.join("dev/block").join(fields[2]));
            match device {
                Ok(device) => {
                    let class = classify_device(&device, &mut HashSet::new());
                    let identity = backing_identity(&device, &mut HashSet::new());
                    (class, identity)
                }
                Err(_) => (StorageClass::Unknown, String::new()),
            }
        });
        let class = match fs[0] {
            "nfs" | "nfs4" | "cifs" | "smb3" | "9p" | "afs" | "ceph" | "fuse.sshfs"
            | "fuse.rclone" => StorageClass::Remote,
            "tmpfs" | "ramfs" | "overlay" | "proc" | "sysfs" | "devtmpfs" | "squashfs" => {
                StorageClass::Virtual
            }
            // A single mount device does not identify every member of these pools.
            "btrfs" | "zfs" => StorageClass::Unknown,
            _ => *class,
        };
        // Last mount at a path masks earlier mounts, matching mountinfo ordering.
        mounts.insert(
            root.clone(),
            MountPolicy {
                root,
                class,
                identity: format!("{}:{}:{}:{device_identity}", fields[0], fields[2], fs[1]),
            },
        );
    }
    mounts.into_values().collect()
}

fn unescape(s: &str) -> String {
    s.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn text(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

fn whole_device(path: &Path) -> &Path {
    if path.join("partition").exists() {
        path.parent().unwrap_or(path)
    } else {
        path
    }
}

fn members(path: &Path) -> Option<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(path.join("slaves")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(vec![]),
        Err(_) => return None,
    };
    entries
        .map(|entry| std::fs::canonicalize(entry.ok()?.path()).ok())
        .collect()
}

fn classify_device(path: &Path, visited: &mut HashSet<PathBuf>) -> StorageClass {
    let path = whole_device(path);
    if !visited.insert(path.to_path_buf()) {
        return StorageClass::Unknown;
    }
    let Some(slaves) = members(path) else {
        return StorageClass::Unknown;
    };
    if !slaves.is_empty() {
        return combine(
            slaves
                .iter()
                .map(|p| classify_device(p, &mut visited.clone())),
        );
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let ancestry = path.to_string_lossy();
    // USB readers frequently report nonremovable/nonrotating; bus and MMC
    // ancestry take precedence. Do not infer SSD speed from filesystem type.
    if path.ancestors().any(|p| {
        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        n.starts_with("usb")
            || n.starts_with("mmc")
            || n.starts_with("thunderbolt")
            || matches!(
                text(p.join("removable")).as_deref(),
                Some("1" | "removable")
            )
            || text(p.join("external")).as_deref() == Some("1")
            || text(p.join("external_facing")).as_deref() == Some("1")
    }) {
        return StorageClass::RemovableExternal;
    }
    if name.starts_with("loop") || name.starts_with("ram") || ancestry.contains("/virtual/") {
        return StorageClass::Virtual;
    }
    // NVMe over Fabrics is remote despite exposing a normal block device.
    if name.starts_with("nvme") {
        let transport = path.ancestors().find_map(|p| text(p.join("transport")));
        if matches!(transport.as_deref(), Some("tcp" | "rdma" | "fc")) {
            return StorageClass::Remote;
        }
        if transport.as_deref() != Some("pcie") {
            return StorageClass::Unknown;
        }
    }
    if text(path.join("removable")).as_deref() != Some("0") {
        return StorageClass::Unknown;
    }
    if path
        .ancestors()
        .any(|p| text(p.join("removable")).as_deref() == Some("unknown"))
    {
        return StorageClass::Unknown;
    }
    let internal_bus = (name.starts_with("nvme") && ancestry.contains("/pci"))
        || (name.starts_with("sd") && ancestry.contains("/ata"));
    match text(path.join("queue/rotational")).as_deref() {
        Some("1") => StorageClass::Rotational,
        Some("0") if internal_bus => StorageClass::InternalSsd,
        _ => StorageClass::Unknown,
    }
}

fn backing_identity(path: &Path, seen: &mut HashSet<PathBuf>) -> String {
    let path = whole_device(path);
    if !seen.insert(path.to_path_buf()) {
        return String::new();
    }
    let mut parts = vec![format!(
        "{}:{}",
        path.display(),
        text(path.join("diskseq")).unwrap_or_default()
    )];
    for member in members(path).unwrap_or_default() {
        parts.push(backing_identity(&member, seen));
    }
    parts.sort();
    parts.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("fbro-sysfs-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(root.join("dev/block")).unwrap();
            Self(root)
        }
        fn write(&self, path: &str, value: &str) {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, value).unwrap();
        }
        fn disk(&self, relative: &str, dev: &str, rotational: &str) -> PathBuf {
            self.write(&format!("{relative}/removable"), "0");
            self.write(&format!("{relative}/queue/rotational"), rotational);
            let path = self.0.join(relative);
            symlink(&path, self.0.join("dev/block").join(dev)).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn class(path: &Path) -> StorageClass {
        classify_device(path, &mut HashSet::new())
    }

    #[test]
    fn nvme_sata_card_readers_and_unknown_devices() {
        let f = Fixture::new();
        f.write("devices/pci0/nvme/nvme0/transport", "pcie");
        let nvme = f.disk("devices/pci0/nvme/nvme0/nvme0n1", "259:0", "0");
        assert_eq!(class(&nvme), StorageClass::InternalSsd);
        let sata = f.disk("devices/pci0/ata1/block/sda", "8:0", "0");
        assert_eq!(class(&sata), StorageClass::InternalSsd);
        f.write("devices/pci0/ata1/block/sda/queue/rotational", "1");
        assert_eq!(class(&sata), StorageClass::Rotational);
        let usb = f.disk("devices/pci0/usb1/host0/block/sdb", "8:16", "0");
        assert_eq!(class(&usb), StorageClass::RemovableExternal);
        let mmc = f.disk("devices/pci0/mmc_host/mmc0/block/mmcblk0", "179:0", "0");
        assert_eq!(class(&mmc), StorageClass::RemovableExternal);
        let unknown = f.disk("devices/host0/block/sdc", "8:32", "0");
        assert_eq!(class(&unknown), StorageClass::Unknown);
        f.write("devices/pci0/removable", "removable");
        assert_eq!(class(&nvme), StorageClass::RemovableExternal);
        f.write("devices/pci0/removable", "unknown");
        assert_eq!(class(&nvme), StorageClass::Unknown);
        f.write("devices/pci0/removable", "fixed");
        f.write("devices/pci0/nvme/nvme0/transport", "tcp");
        assert_eq!(class(&nvme), StorageClass::Remote);
    }

    #[test]
    fn mapped_devices_require_every_member_and_follow_partitions() {
        let f = Fixture::new();
        f.write("devices/pci0/nvme/nvme0/transport", "pcie");
        let nvme = f.disk("devices/pci0/nvme/nvme0/nvme0n1", "259:0", "0");
        let partition = nvme.join("nvme0n1p1");
        std::fs::create_dir(&partition).unwrap();
        std::fs::write(partition.join("partition"), "1").unwrap();
        assert_eq!(class(&partition), StorageClass::InternalSsd);
        let dm = f.0.join("devices/virtual/block/dm-0");
        std::fs::create_dir_all(dm.join("slaves")).unwrap();
        symlink(&partition, dm.join("slaves/nvme0n1p1")).unwrap();
        assert_eq!(class(&dm), StorageClass::InternalSsd);
        let usb = f.disk("devices/pci0/usb1/block/sdb", "8:16", "0");
        symlink(usb, dm.join("slaves/sdb")).unwrap();
        assert_eq!(class(&dm), StorageClass::RemovableExternal);
        // A missing member is not silently dropped from a mixed pool.
        symlink(f.0.join("missing"), dm.join("slaves/missing")).unwrap();
        assert_eq!(class(&dm), StorageClass::Unknown);
    }

    #[test]
    fn full_mountinfo_preserves_nested_mounts_and_escaped_paths() {
        let f = Fixture::new();
        f.write("devices/pci0/nvme/nvme0/transport", "pcie");
        f.disk("devices/pci0/nvme/nvme0/nvme0n1", "259:0", "0");
        f.disk("devices/pci0/usb1/block/sdb", "8:16", "0");
        let input = "1 0 259:0 / / rw - ext4 /dev/nvme0n1 rw\n\
            2 1 8:16 / /hidden/My\\040Card rw - exfat /dev/sdb rw\n\
            3 1 0:42 / /hidden/network rw - nfs server:/photos rw\n\
            4 1 0:43 / /container rw - overlay overlay rw\n\
            5 1 259:0 / /pool rw - btrfs /dev/nvme0n1 rw\n";
        let mounts = parse_mounts(input, &f.0);
        for (root, expected) in [
            ("/", StorageClass::InternalSsd),
            ("/hidden/My Card", StorageClass::RemovableExternal),
            ("/hidden/network", StorageClass::Remote),
            ("/container", StorageClass::Virtual),
            ("/pool", StorageClass::Unknown),
        ] {
            assert_eq!(
                mounts
                    .iter()
                    .find(|m| m.root == Path::new(root))
                    .unwrap()
                    .class,
                expected
            );
        }
        let old = parse_mounts(input, &f.0);
        f.write("devices/pci0/nvme/nvme0/nvme0n1/diskseq", "99");
        let new = parse_mounts(input, &f.0);
        assert_ne!(
            old.iter()
                .find(|m| m.root == Path::new("/"))
                .unwrap()
                .identity,
            new.iter()
                .find(|m| m.root == Path::new("/"))
                .unwrap()
                .identity
        );
    }
}
