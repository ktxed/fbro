use std::collections::{HashMap, HashSet};

use tokio::sync::mpsc;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::Connection;

use crate::platform::{EjectAction, EjectInfo, VolumeInfo};

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
                eject: if mounted {
                    eject_info(block, &objects)
                } else {
                    None
                },
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

    pub(super) async fn eject(&self, block_path: &str, action: EjectAction) -> Result<(), String> {
        let objects = self.get_managed_objects().await.map_err(|e| format!("Inspect drive: {e}"))?;
        let block = objects.iter()
            .find(|(path, _)| path.as_str() == block_path)
            .and_then(|(_, interfaces)| interfaces.get(BLOCK_IFACE))
            .ok_or_else(|| "Volume is no longer available".to_string())?;
        let info = eject_info(block, &objects)
            .ok_or_else(|| "Volume is no longer mounted or removable".to_string())?;
        let drive = info.drive_id.as_str();

        let mut mounted_blocks: Vec<String> = objects.iter()
            .filter_map(|(path, interfaces)| {
                let block = interfaces.get(BLOCK_IFACE)?;
                let fs = interfaces.get(FS_IFACE)?;
                (prop_object_path(block, "Drive").as_deref() == Some(drive)
                    && !prop_byte_array_list(fs, "MountPoints").is_empty())
                    .then(|| path.as_str().to_string())
            })
            .collect();
        mounted_blocks.sort();
        if !mounted_blocks.iter().any(|path| path == block_path) {
            return Err("Volume is no longer mounted".into());
        }

        match action {
            EjectAction::UnmountVolume => self.unmount(block_path).await,
            EjectAction::EjectDrive => {
                if !info.can_eject_drive {
                    return Err("This drive does not support safe removal".into());
                }
                for path in mounted_blocks {
                    self.unmount(&path).await?;
                }
                let drive_props = drive_props(drive, &objects)
                    .ok_or_else(|| "Drive is no longer available".to_string())?;
                let method = match drive_removal_method(drive_props) {
                    Some(DriveRemovalMethod::PowerOff) => "PowerOff",
                    Some(DriveRemovalMethod::Eject) => "Eject",
                    Some(DriveRemovalMethod::UnmountOnly) => return Ok(()),
                    None => return Err("This drive does not support safe removal".into()),
                };
                let path = ObjectPath::try_from(drive).map_err(|e| format!("Invalid drive: {e}"))?;
                let proxy = zbus::Proxy::new(&self.connection, UDISKS2_BUS, path, DRIVE_IFACE)
                    .await.map_err(|e| format!("Drive proxy: {e}"))?;
                let options: HashMap<&str, Value<'_>> = HashMap::new();
                proxy.call::<_, _, ()>(method, &(options,)).await
                    .map_err(|e| format!("Eject drive: {e}"))
            }
        }
    }

    async fn unmount(&self, block_path: &str) -> Result<(), String> {
        let path = ObjectPath::try_from(block_path).map_err(|e| format!("Invalid volume: {e}"))?;
        let proxy = zbus::Proxy::new(&self.connection, UDISKS2_BUS, path, FS_IFACE)
            .await.map_err(|e| format!("Filesystem proxy: {e}"))?;
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        proxy.call::<_, _, ()>("Unmount", &(options,)).await
            .map_err(|e| format!("Unmount volume: {e}"))
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

fn drive_props<'a>(drive_path: &str, objects: &'a ManagedObjects) -> Option<&'a HashMap<String, OwnedValue>> {
    objects.iter()
        .find(|(path, _)| path.as_str() == drive_path)
        .and_then(|(_, interfaces)| interfaces.get(DRIVE_IFACE))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DriveRemovalMethod {
    PowerOff,
    Eject,
    UnmountOnly,
}

fn drive_removal_method(drive: &HashMap<String, OwnedValue>) -> Option<DriveRemovalMethod> {
    if prop_bool(drive, "CanPowerOff").unwrap_or(false) {
        return Some(DriveRemovalMethod::PowerOff);
    }
    // Card readers and USB sticks can advertise Ejectable even when their
    // hardware has no eject command. Once all filesystems are unmounted, the
    // flash media is safe to remove without issuing Drive.Eject.
    let media = prop_string(drive, "Media").unwrap_or_default();
    if media.starts_with("flash") || media == "thumb" {
        return Some(DriveRemovalMethod::UnmountOnly);
    }
    prop_bool(drive, "Ejectable").unwrap_or(false).then_some(DriveRemovalMethod::Eject)
}

fn eject_info(block: &HashMap<String, OwnedValue>, objects: &ManagedObjects) -> Option<EjectInfo> {
    let drive_id = prop_object_path(block, "Drive")?;
    let drive = drive_props(&drive_id, objects)?;
    if !prop_bool(drive, "Removable").unwrap_or(false) {
        return None;
    }
    let mounted_volume_count = objects.values().filter(|interfaces| {
        let Some(other_block) = interfaces.get(BLOCK_IFACE) else { return false };
        let Some(fs) = interfaces.get(FS_IFACE) else { return false };
        prop_object_path(other_block, "Drive").as_deref() == Some(drive_id.as_str())
            && !prop_byte_array_list(fs, "MountPoints").is_empty()
    }).count();
    let vendor = prop_string(drive, "Vendor").unwrap_or_default();
    let model = prop_string(drive, "Model").unwrap_or_default();
    let drive_name = format!("{vendor} {model}").trim().to_string();
    Some(EjectInfo {
        drive_id,
        drive_name: if drive_name.is_empty() { "Removable drive".into() } else { drive_name },
        mounted_volume_count,
        can_eject_drive: drive_removal_method(drive).is_some(),
    })
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
    use super::{drive_removal_method, eject_info, is_user_visible_volume, mount_display_name, DriveRemovalMethod, ManagedObjects, DRIVE_IFACE};
    use std::collections::HashMap;
    use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Str};

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

    #[test]
    fn only_removable_drives_advertise_eject() {
        let drive_path = "/org/freedesktop/UDisks2/drives/usb";
        let block = HashMap::from([(
            "Drive".to_string(),
            OwnedValue::from(ObjectPath::try_from(drive_path).unwrap()),
        )]);
        let objects = |removable: bool| -> ManagedObjects {
            let props = HashMap::from([
                ("Removable".to_string(), OwnedValue::from(removable)),
                ("CanPowerOff".to_string(), OwnedValue::from(true)),
            ]);
            HashMap::from([(
                OwnedObjectPath::try_from(drive_path).unwrap(),
                HashMap::from([(DRIVE_IFACE.to_string(), props)]),
            )])
        };
        assert!(eject_info(&block, &objects(false)).is_none());
        let info = eject_info(&block, &objects(true)).unwrap();
        assert_eq!(info.drive_id, drive_path);
        assert!(info.can_eject_drive);
        assert_eq!(info.mounted_volume_count, 0);
    }

    #[test]
    fn flash_card_without_power_off_uses_unmount_only() {
        let props = HashMap::from([
            ("CanPowerOff".to_string(), OwnedValue::from(false)),
            ("Ejectable".to_string(), OwnedValue::from(true)),
            ("Media".to_string(), OwnedValue::from(Str::from("flash_sd"))),
        ]);
        assert_eq!(drive_removal_method(&props), Some(DriveRemovalMethod::UnmountOnly));
        let powered = HashMap::from([
            ("CanPowerOff".to_string(), OwnedValue::from(true)),
            ("Media".to_string(), OwnedValue::from(Str::from("flash_sd"))),
        ]);
        assert_eq!(drive_removal_method(&powered), Some(DriveRemovalMethod::PowerOff));
    }
}
