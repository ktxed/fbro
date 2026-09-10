use std::collections::{HashMap, HashSet};

use tokio::sync::mpsc;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::Connection;

use super::VolumeInfo;

const UDISKS2_BUS: &str = "org.freedesktop.UDisks2";
const UDISKS2_PATH: &str = "/org/freedesktop/UDisks2";
const BLOCK_PREFIX: &str = "/org/freedesktop/UDisks2/block_devices/";

const BLOCK_IFACE: &str = "org.freedesktop.UDisks2.Block";
const DRIVE_IFACE: &str = "org.freedesktop.UDisks2.Drive";
const FS_IFACE: &str = "org.freedesktop.UDisks2.Filesystem";

pub(super) struct VolumeInventory {
    pub volumes: Vec<VolumeInfo>,
    /// Mounted paths hidden by UDisks must not be reintroduced by /proc/mounts.
    pub ignored_mount_paths: HashSet<String>,
}

/// Filesystem types we care about (same whitelist as /proc/mounts path).
const FS_WHITELIST: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "zfs", "vfat", "exfat", "ntfs", "ntfs3", "f2fs", "jfs",
    "hfsplus", "fuseblk", "nfs", "nfs4", "cifs",
];

pub(super) struct UDisks2 {
    connection: Connection,
}

impl UDisks2 {
    pub(super) async fn new() -> Result<Self, String> {
        let connection = Connection::system()
            .await
            .map_err(|e| format!("D-Bus system bus: {e}"))?;

        // Quick check that udisks2 is reachable
        let proxy = zbus::fdo::DBusProxy::new(&connection)
            .await
            .map_err(|e| format!("DBus proxy: {e}"))?;
        let names = proxy
            .list_names()
            .await
            .map_err(|e| format!("list names: {e}"))?;
        if !names.iter().any(|n| n.as_str() == UDISKS2_BUS) {
            return Err("udisks2 service not found on D-Bus".to_string());
        }

        Ok(Self { connection })
    }

    /// Return mounted filesystems plus unmounted user-removable media. UDisks
    /// knows about every low-level partition, but those are not all sources.
    pub(super) async fn list_block_devices(&self) -> VolumeInventory {
        let objects = match self.get_managed_objects().await {
            Ok(o) => o,
            Err(e) => {
                eprintln!("[fbro] udisks2 GetManagedObjects failed: {e}");
                return VolumeInventory {
                    volumes: vec![],
                    ignored_mount_paths: HashSet::new(),
                };
            }
        };

        let mut volumes = Vec::new();
        let mut ignored_mount_paths = HashSet::new();

        for (path, interfaces) in &objects {
            let path_str = path.as_str();
            if !path_str.starts_with(BLOCK_PREFIX) {
                continue;
            }

            let block = match interfaces.get(BLOCK_IFACE) {
                Some(props) => props,
                None => continue,
            };

            let Some(fs_props) = interfaces.get(FS_IFACE) else {
                continue;
            };
            let mount_points = prop_byte_array_list(fs_props, "MountPoints");
            let mounted = !mount_points.is_empty();

            // Remember ignored mounted paths so the /proc fallback cannot put
            // a device that UDisks explicitly hid back into the source bar.
            if prop_bool(block, "HintIgnore").unwrap_or(false) {
                ignored_mount_paths.extend(mount_points);
                continue;
            }

            let hint_system = prop_bool(block, "HintSystem").unwrap_or(true);
            let drive_removable = drive_is_removable(block, &objects);
            if !is_user_visible_volume(mounted, hint_system, drive_removable) {
                continue;
            }

            // Skip devices with no filesystem type
            let id_type = prop_string(block, "IdType").unwrap_or_default();
            if id_type.is_empty() || !FS_WHITELIST.contains(&id_type.as_str()) {
                continue;
            }

            // Skip zero-size devices
            let size = prop_u64(block, "Size").unwrap_or(0);
            if size == 0 {
                continue;
            }

            // Device name (e.g. "sdb1") from object path
            let dev_name = path_str.strip_prefix(BLOCK_PREFIX).unwrap_or(path_str);

            // Skip loop and dm devices
            if dev_name.starts_with("loop") || dev_name.starts_with("dm-") {
                continue;
            }

            let id_uuid = prop_string(block, "IdUUID").unwrap_or_default();
            let id_label = prop_string(block, "IdLabel").unwrap_or_default();
            let hint_name = prop_string(block, "HintName").unwrap_or_default();
            let mount_point = mount_points.first().cloned().unwrap_or_default();

            // Generate stable ID
            let id = if !id_uuid.is_empty() {
                format!("vol-{id_uuid}")
            } else {
                format!("vol-dev-{dev_name}")
            };

            // Mounted sources historically use their mount-point component in
            // fbro. Keep that stable across the /proc -> UDisks reconciliation;
            // labels remain useful for unmounted media with no mount point.
            let name = if mounted {
                mount_display_name(&mount_point)
            } else if !hint_name.is_empty() {
                hint_name
            } else if !id_label.is_empty() {
                id_label
            } else {
                dev_name.to_string()
            };

            volumes.push(VolumeInfo {
                id,
                name,
                root_path: mount_point,
                mounted,
                device_path: Some(path_str.to_string()),
            });
        }

        // Sort: mounted first, then by name
        volumes.sort_by(|a, b| b.mounted.cmp(&a.mounted).then_with(|| a.name.cmp(&b.name)));

        VolumeInventory {
            volumes,
            ignored_mount_paths,
        }
    }

    /// Mount a block device via udisks2. Returns the mount point on success.
    pub(super) async fn mount(&self, object_path: &str) -> Result<String, String> {
        let path =
            ObjectPath::try_from(object_path).map_err(|e| format!("invalid object path: {e}"))?;

        let proxy = zbus::Proxy::new(&self.connection, UDISKS2_BUS, path, FS_IFACE)
            .await
            .map_err(|e| format!("proxy creation failed: {e}"))?;

        let options: HashMap<&str, Value<'_>> = HashMap::new();
        let reply: String = proxy.call("Mount", &(options,)).await.map_err(|e| {
            // Extract the human-readable D-Bus error message
            format!("{e}")
        })?;

        Ok(reply)
    }

    /// Subscribe to udisks2 D-Bus signals. Returns a channel that fires ()
    /// whenever a device or mount state changes.
    pub(super) async fn subscribe_signals(&self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::channel(8);

        // InterfacesAdded / InterfacesRemoved on ObjectManager
        let conn1 = self.connection.clone();
        let tx1 = tx.clone();
        tokio::spawn(async move {
            use futures_util::StreamExt;
            let proxy = match zbus::fdo::ObjectManagerProxy::builder(&conn1)
                .destination(UDISKS2_BUS)
                .unwrap()
                .path(UDISKS2_PATH)
                .unwrap()
                .build()
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[fbro] udisks2 ObjectManager proxy failed: {e}");
                    return;
                }
            };

            let mut added = proxy.receive_interfaces_added().await.unwrap();
            let mut removed = proxy.receive_interfaces_removed().await.unwrap();

            loop {
                tokio::select! {
                    Some(_) = added.next() => { let _ = tx1.send(()).await; }
                    Some(_) = removed.next() => { let _ = tx1.send(()).await; }
                    else => break,
                }
            }
        });

        // PropertiesChanged on Filesystem interface (mount/unmount)
        let conn2 = self.connection.clone();
        let tx2 = tx;
        tokio::spawn(async move {
            use futures_util::StreamExt;
            // Match rule: any PropertiesChanged signal from udisks2 on Filesystem interface
            let rule = zbus::MatchRule::builder()
                .msg_type(zbus::message::Type::Signal)
                .sender(UDISKS2_BUS)
                .unwrap()
                .interface("org.freedesktop.DBus.Properties")
                .unwrap()
                .member("PropertiesChanged")
                .unwrap()
                .build();

            let mut stream = match zbus::MessageStream::for_match_rule(rule, &conn2, None).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[fbro] udisks2 PropertiesChanged match failed: {e}");
                    return;
                }
            };

            while let Some(Ok(msg)) = stream.next().await {
                // Check if it's about the Filesystem interface
                if let Ok(body) = msg
                    .body()
                    .deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>()
                {
                    if body.0 == FS_IFACE {
                        let _ = tx2.send(()).await;
                    }
                }
            }
        });

        rx
    }

    async fn get_managed_objects(
        &self,
    ) -> Result<HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>, zbus::Error>
    {
        let proxy = zbus::fdo::ObjectManagerProxy::builder(&self.connection)
            .destination(UDISKS2_BUS)?
            .path(UDISKS2_PATH)?
            .build()
            .await?;

        let objects = proxy.get_managed_objects().await?;

        // Convert the zbus types to our simpler HashMap
        Ok(objects
            .into_iter()
            .map(|(path, ifaces)| {
                let ifaces_map: HashMap<String, HashMap<String, OwnedValue>> = ifaces
                    .into_iter()
                    .map(|(iface, props)| (iface.to_string(), props))
                    .collect();
                (path, ifaces_map)
            })
            .collect())
    }
}

type ManagedObjects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

fn is_user_visible_volume(mounted: bool, hint_system: bool, drive_removable: bool) -> bool {
    mounted || (!hint_system && drive_removable)
}

fn mount_display_name(mount_point: &str) -> String {
    if mount_point == "/" {
        return "/".into();
    }
    mount_point
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(mount_point)
        .to_string()
}

fn drive_is_removable(block: &HashMap<String, OwnedValue>, objects: &ManagedObjects) -> bool {
    let Some(drive_path) = prop_object_path(block, "Drive") else {
        return false;
    };
    let Some(drive) = objects
        .iter()
        .find(|(path, _)| path.as_str() == drive_path)
        .and_then(|(_, interfaces)| interfaces.get(DRIVE_IFACE))
    else {
        return false;
    };
    prop_bool(drive, "Removable").unwrap_or(false)
}

// ── Property extraction helpers ─────────────────────────────────────────────

fn prop_string(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|v| <&str>::try_from(v).ok().map(|s| s.to_string()))
}

fn prop_bool(props: &HashMap<String, OwnedValue>, key: &str) -> Option<bool> {
    props.get(key).and_then(|v| bool::try_from(v).ok())
}

fn prop_u64(props: &HashMap<String, OwnedValue>, key: &str) -> Option<u64> {
    props.get(key).and_then(|v| u64::try_from(v).ok())
}

fn prop_object_path(props: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    props
        .get(key)
        .and_then(|value| <&ObjectPath<'_>>::try_from(value).ok())
        .map(|path| path.as_str().to_string())
}

/// Extract MountPoints property: Array of Array<byte> (null-terminated paths).
fn prop_byte_array_list(props: &HashMap<String, OwnedValue>, key: &str) -> Vec<String> {
    let val = match props.get(key) {
        Some(v) => v,
        None => return vec![],
    };

    // MountPoints is typed as aay (array of byte arrays) in D-Bus.
    // Each byte array is a null-terminated filesystem path.
    if let Ok(array) = <&zbus::zvariant::Array>::try_from(val) {
        array
            .iter()
            .filter_map(|item| {
                if let Ok(inner) = <&zbus::zvariant::Array>::try_from(item) {
                    let bytes: Vec<u8> =
                        inner.iter().filter_map(|b| u8::try_from(b).ok()).collect();
                    // Strip trailing null byte
                    let trimmed = if bytes.last() == Some(&0) {
                        &bytes[..bytes.len() - 1]
                    } else {
                        &bytes
                    };
                    String::from_utf8(trimmed.to_vec()).ok()
                } else {
                    None
                }
            })
            .collect()
    } else {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::{is_user_visible_volume, mount_display_name};

    #[test]
    fn visibility_keeps_mounts_and_only_user_removable_unmounted_media() {
        assert!(is_user_visible_volume(true, true, false));
        assert!(is_user_visible_volume(true, false, false));
        assert!(is_user_visible_volume(false, false, true));
        assert!(!is_user_visible_volume(false, true, true));
        assert!(!is_user_visible_volume(false, false, false));
        assert!(!is_user_visible_volume(false, true, false));
    }

    #[test]
    fn unlabeled_mounted_devices_use_friendly_mount_names() {
        assert_eq!(mount_display_name("/"), "/");
        assert_eq!(mount_display_name("/home"), "home");
        assert_eq!(mount_display_name("/mnt/photo archive/"), "photo archive");
    }
}
