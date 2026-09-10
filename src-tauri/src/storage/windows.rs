//! Metadata-only Win32 storage queries. Handles use zero desired access; failure
//! (including insufficient permissions) means unknown, never permission elevation.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]
use super::{combine, MountPolicy, StorageClass};
use std::hash::{Hash, Hasher};
use std::{ffi::c_void, path::PathBuf, ptr};

type Handle = *mut c_void;
#[cfg_attr(target_os = "windows", link(name = "kernel32"))]
extern "system" {
    fn FindFirstVolumeW(name: *mut u16, len: u32) -> Handle;
    fn FindNextVolumeW(handle: Handle, name: *mut u16, len: u32) -> i32;
    fn FindVolumeClose(handle: Handle) -> i32;
    fn GetLastError() -> u32;
    fn GetVolumePathNamesForVolumeNameW(
        name: *const u16,
        paths: *mut u16,
        len: u32,
        required: *mut u32,
    ) -> i32;
    fn GetLogicalDrives() -> u32;
    fn GetDriveTypeW(root: *const u16) -> u32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        sharing: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn DeviceIoControl(
        handle: Handle,
        code: u32,
        input: *const c_void,
        input_size: u32,
        output: *mut c_void,
        output_size: u32,
        returned: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
}

struct Device(Handle);
impl Drop for Device {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn open(name: &str) -> Option<Device> {
    let h = unsafe {
        CreateFileW(
            wide(name).as_ptr(),
            0,
            3,
            ptr::null_mut(),
            3,
            0,
            ptr::null_mut(),
        )
    };
    if h as isize == -1 {
        None
    } else {
        Some(Device(h))
    }
}
fn query(device: &Device, code: u32, input: &[u8]) -> Option<Vec<u8>> {
    let mut output = vec![0u8; 65536];
    let mut returned = 0;
    let ok = unsafe {
        DeviceIoControl(
            device.0,
            code,
            input.as_ptr().cast(),
            input.len() as u32,
            output.as_mut_ptr().cast(),
            output.len() as u32,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 || returned as usize > output.len() {
        return None;
    }
    output.truncate(returned as usize);
    Some(output)
}
fn property(device: &Device, id: u32) -> Option<Vec<u8>> {
    let mut input = [0u8; 12]; // STORAGE_PROPERTY_QUERY, PropertyStandardQuery
    input[..4].copy_from_slice(&id.to_le_bytes());
    query(device, 0x002d1400, &input) // IOCTL_STORAGE_QUERY_PROPERTY
}
fn number(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn physical_class(number: u32) -> (StorageClass, u64) {
    let Some(device) = open(&format!("\\\\.\\PhysicalDrive{number}")) else {
        return (StorageClass::Unknown, 0);
    };
    let (Some(descriptor), Some(hotplug), Some(seek)) = (
        property(&device, 0),
        query(&device, 0x002d0c14, &[]),
        property(&device, 7),
    ) else {
        return (StorageClass::Unknown, 0);
    };
    // Includes the device serial returned in STORAGE_DEVICE_DESCRIPTOR.
    let mut identity = std::collections::hash_map::DefaultHasher::new();
    number.hash(&mut identity);
    descriptor.hash(&mut identity);
    (
        classify_properties(&descriptor, &hotplug, &seek),
        identity.finish(),
    )
}

fn classify_properties(descriptor: &[u8], hotplug: &[u8], seek: &[u8]) -> StorageClass {
    if descriptor.len() < 36 || hotplug.len() < 8 || seek.len() < 9 {
        return StorageClass::Unknown;
    }
    for (bytes, minimum) in [(descriptor, 36), (seek, 9)] {
        let size = number(bytes, 4).unwrap_or(0) as usize;
        if size < minimum || size > bytes.len() {
            return StorageClass::Unknown;
        }
    }
    if number(hotplug, 0) != Some(8) {
        return StorageClass::Unknown;
    }
    let bus = number(descriptor, 28).unwrap_or(0);
    if bus == 9 {
        return StorageClass::Remote;
    } // iSCSI
    if descriptor[10] != 0
        || hotplug[4..7].iter().any(|b| *b != 0)
        || matches!(bus, 4 | 7 | 12 | 13)
    {
        return StorageClass::RemovableExternal;
    }
    if matches!(bus, 14 | 15 | 16) {
        return StorageClass::Virtual;
    }
    if seek[8] != 0 {
        return StorageClass::Rotational;
    }
    if matches!(bus, 1 | 3 | 10 | 11 | 17) {
        StorageClass::InternalSsd
    } else {
        StorageClass::Unknown
    }
}

fn volume_class(name: &str) -> (StorageClass, String) {
    let Some(volume) = open(name.trim_end_matches('\\')) else {
        return (StorageClass::Unknown, name.into());
    };
    let Some(extents) = query(&volume, 0x00560000, &[]) else {
        return (StorageClass::Unknown, name.into());
    };
    let count = number(&extents, 0).unwrap_or(0) as usize;
    if count == 0 || count > 1024 || extents.len() < 8 + count * 24 {
        return (StorageClass::Unknown, name.into());
    }
    let members: Vec<_> = (0..count)
        .map(|i| physical_class(number(&extents, 8 + i * 24).unwrap()))
        .collect();
    let mut identity = std::collections::hash_map::DefaultHasher::new();
    // Do not hash ABI padding, which need not be stable between queries.
    for i in 0..count {
        extents[8 + i * 24..12 + i * 24].hash(&mut identity);
        extents[16 + i * 24..32 + i * 24].hash(&mut identity);
    }
    for (_, member) in &members {
        member.hash(&mut identity);
    }
    (
        combine(members.iter().map(|(class, _)| *class)),
        format!("{name}:{:x}", identity.finish()),
    )
}

pub(super) fn discover() -> Vec<MountPolicy> {
    let mut mounts = Vec::new();
    let mut complete = true;
    let mut name_buf = [0u16; 1024];
    unsafe {
        let handle = FindFirstVolumeW(name_buf.as_mut_ptr(), name_buf.len() as u32);
        if handle as isize != -1 {
            loop {
                let end = name_buf
                    .iter()
                    .position(|x| *x == 0)
                    .unwrap_or(name_buf.len());
                let name = String::from_utf16_lossy(&name_buf[..end]);
                let (class, identity) = volume_class(&name);
                let mut paths = vec![0u16; 32768];
                let mut required = 0;
                if GetVolumePathNamesForVolumeNameW(
                    name_buf.as_ptr(),
                    paths.as_mut_ptr(),
                    paths.len() as u32,
                    &mut required,
                ) != 0
                {
                    for path in paths.split(|x| *x == 0).take_while(|p| !p.is_empty()) {
                        mounts.push(MountPolicy {
                            root: PathBuf::from(String::from_utf16_lossy(path).to_lowercase()),
                            identity: identity.clone(),
                            class,
                        });
                    }
                } else {
                    complete = false;
                }
                if FindNextVolumeW(handle, name_buf.as_mut_ptr(), name_buf.len() as u32) == 0 {
                    complete &= GetLastError() == 18; // ERROR_NO_MORE_FILES
                    break;
                }
            }
            FindVolumeClose(handle);
        } else {
            complete = false;
        }
        let mask = GetLogicalDrives();
        for i in 0..26 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let root = format!("{}:\\", (b'a' + i) as char);
            if mounts.iter().any(|m| m.root == PathBuf::from(&root)) {
                continue;
            }
            let class = match GetDriveTypeW(wide(&root).as_ptr()) {
                2 | 5 => StorageClass::RemovableExternal,
                4 => StorageClass::Remote,
                6 => StorageClass::Virtual,
                _ => StorageClass::Unknown,
            };
            mounts.push(MountPolicy {
                root: PathBuf::from(&root),
                identity: root,
                class,
            });
        }
    }
    if !complete {
        // A missing nested mount must not inherit an eligible parent volume.
        for mount in &mut mounts {
            if mount.class.eligible() {
                mount.class = StorageClass::Unknown;
            }
        }
    }
    mounts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn properties(bus: u32, rotating: bool, removable: bool) -> StorageClass {
        let mut descriptor = [0u8; 40];
        descriptor[4..8].copy_from_slice(&40u32.to_le_bytes());
        descriptor[28..32].copy_from_slice(&bus.to_le_bytes());
        descriptor[10] = u8::from(removable);
        let mut hotplug = [0u8; 8];
        hotplug[..4].copy_from_slice(&8u32.to_le_bytes());
        let mut seek = [0u8; 12];
        seek[4..8].copy_from_slice(&12u32.to_le_bytes());
        seek[8] = u8::from(rotating);
        classify_properties(&descriptor, &hotplug, &seek)
    }

    #[test]
    fn storage_descriptor_fixtures_require_fast_fixed_local_devices() {
        assert_eq!(properties(17, false, false), StorageClass::InternalSsd);
        assert_eq!(properties(11, false, false), StorageClass::InternalSsd);
        assert_eq!(properties(11, true, false), StorageClass::Rotational);
        for bus in [4, 7, 12, 13] {
            assert_eq!(
                properties(bus, false, false),
                StorageClass::RemovableExternal
            );
        }
        assert_eq!(properties(17, false, true), StorageClass::RemovableExternal);
        assert_eq!(properties(9, false, false), StorageClass::Remote);
        assert_eq!(properties(15, false, false), StorageClass::Virtual);
        assert_eq!(properties(0, false, false), StorageClass::Unknown);
        assert_eq!(classify_properties(&[], &[], &[]), StorageClass::Unknown);
        assert_eq!(
            classify_properties(&[0; 40], &[0; 8], &[0; 12]),
            StorageClass::Unknown
        );
    }
}
