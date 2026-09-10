//! DiskArbitration identifies the mounted media; IOKit supplies physical device
//! characteristics. Walk every parent branch, including APFS backing stores.
use super::{combine, MountPolicy, StorageClass};
use std::{
    collections::HashSet,
    ffi::{c_char, c_void, CStr, CString},
    path::PathBuf,
    ptr,
};

type Cf = *const c_void;
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: Cf);
    fn CFGetTypeID(value: Cf) -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(value: Cf) -> u8;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dict: Cf, key: Cf) -> Cf;
    fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithCString(allocator: Cf, text: *const c_char, encoding: u32) -> Cf;
    fn CFStringGetCString(value: Cf, buffer: *mut c_char, capacity: isize, encoding: u32) -> u8;
}
#[link(name = "DiskArbitration", kind = "framework")]
extern "C" {
    fn DASessionCreate(allocator: Cf) -> Cf;
    fn DADiskCreateFromBSDName(allocator: Cf, session: Cf, name: *const c_char) -> Cf;
    fn DADiskCopyDescription(disk: Cf) -> Cf;
    fn DADiskCopyIOMedia(disk: Cf) -> u32;
    static kDADiskDescriptionDeviceInternalKey: Cf;
    static kDADiskDescriptionMediaRemovableKey: Cf;
    static kDADiskDescriptionMediaEjectableKey: Cf;
}
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOObjectRelease(object: u32) -> i32;
    fn IOObjectConformsTo(object: u32, class: *const c_char) -> i32;
    fn IORegistryEntryGetRegistryEntryID(object: u32, id: *mut u64) -> i32;
    fn IORegistryEntryGetParentIterator(
        object: u32,
        plane: *const c_char,
        iterator: *mut u32,
    ) -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IORegistryEntryCreateCFProperty(object: u32, key: Cf, allocator: Cf, options: u32) -> Cf;
}
struct OwnedCf(Cf);
impl Drop for OwnedCf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CFRelease(self.0);
            }
        }
    }
}
struct Io(u32);
impl Drop for Io {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                IOObjectRelease(self.0);
            }
        }
    }
}

fn string(s: &str) -> OwnedCf {
    let s = CString::new(s).unwrap();
    OwnedCf(unsafe { CFStringCreateWithCString(ptr::null(), s.as_ptr(), 0x08000100) })
}
fn boolean(value: Cf) -> Option<bool> {
    if value.is_null() || unsafe { CFGetTypeID(value) != CFBooleanGetTypeID() } {
        return None;
    }
    Some(unsafe { CFBooleanGetValue(value) != 0 })
}
fn text(value: Cf) -> Option<String> {
    if value.is_null() || unsafe { CFGetTypeID(value) != CFStringGetTypeID() } {
        return None;
    }
    let mut bytes = [0i8; 512];
    if unsafe { CFStringGetCString(value, bytes.as_mut_ptr(), bytes.len() as isize, 0x08000100) }
        == 0
    {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(bytes.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}
fn field(dict: Cf, key: Cf) -> Cf {
    if dict.is_null() || unsafe { CFGetTypeID(dict) != CFDictionaryGetTypeID() } {
        return ptr::null();
    }
    unsafe { CFDictionaryGetValue(dict, key) }
}
fn property(object: u32, key: &str) -> OwnedCf {
    let key = string(key);
    OwnedCf(unsafe { IORegistryEntryCreateCFProperty(object, key.0, ptr::null(), 0) })
}
fn characteristics(object: u32, group: &str, key: &str) -> Option<String> {
    let dict = property(object, group);
    let key = string(key);
    text(field(dict.0, key.0))
}

fn classify(session: Cf, device: &str) -> (StorageClass, String) {
    let Ok(name) = CString::new(device) else {
        return (StorageClass::Unknown, device.into());
    };
    let disk = OwnedCf(unsafe { DADiskCreateFromBSDName(ptr::null(), session, name.as_ptr()) });
    if disk.0.is_null() {
        return (StorageClass::Unknown, device.into());
    }
    let description = OwnedCf(unsafe { DADiskCopyDescription(disk.0) });
    let internal = boolean(field(description.0, unsafe {
        kDADiskDescriptionDeviceInternalKey
    }));
    let removable = boolean(field(description.0, unsafe {
        kDADiskDescriptionMediaRemovableKey
    }));
    let ejectable = boolean(field(description.0, unsafe {
        kDADiskDescriptionMediaEjectableKey
    }));
    if internal == Some(false) || removable == Some(true) || ejectable == Some(true) {
        return (StorageClass::RemovableExternal, device.into());
    }
    if internal != Some(true) || removable != Some(false) || ejectable != Some(false) {
        return (StorageClass::Unknown, device.into());
    }
    let media = Io(unsafe { DADiskCopyIOMedia(disk.0) });
    let mut pending = vec![media];
    let mut visited = HashSet::new();
    let mut classes = Vec::new();
    let mut complete = true;
    while let Some(object) = pending.pop() {
        if object.0 == 0 {
            complete = false;
            continue;
        }
        let mut id = 0;
        if unsafe { IORegistryEntryGetRegistryEntryID(object.0, &mut id) } != 0 {
            complete = false;
            continue;
        }
        if !visited.insert(id) {
            continue;
        }
        if visited.len() > 4096 {
            complete = false;
            break;
        }
        if unsafe { IOObjectConformsTo(object.0, b"IOBlockStorageDevice\0".as_ptr().cast()) } != 0 {
            let location = characteristics(
                object.0,
                "Protocol Characteristics",
                "Physical Interconnect Location",
            );
            let medium = characteristics(object.0, "Device Characteristics", "Medium Type");
            classes.push(match (location.as_deref(), medium.as_deref()) {
                (Some("External"), _) => StorageClass::RemovableExternal,
                (Some("Internal"), Some("Solid State")) => StorageClass::InternalSsd,
                (Some("Internal"), Some("Rotational")) => StorageClass::Rotational,
                _ => StorageClass::Unknown,
            });
            continue;
        }
        let mut iterator = 0;
        if unsafe {
            IORegistryEntryGetParentIterator(
                object.0,
                b"IOService\0".as_ptr().cast(),
                &mut iterator,
            )
        } != 0
        {
            complete = false;
            continue;
        }
        let iterator = Io(iterator);
        let mut has_parent = false;
        loop {
            let parent = unsafe { IOIteratorNext(iterator.0) };
            if parent == 0 {
                break;
            }
            has_parent = true;
            pending.push(Io(parent));
        }
        // An unresolved branch must not be hidden by another SSD branch.
        if !has_parent {
            complete = false;
        }
    }
    let mut identities: Vec<_> = visited.into_iter().collect();
    identities.sort();
    let class = if complete {
        combine(classes)
    } else {
        StorageClass::Unknown
    };
    (class, format!("{device}:{identities:?}"))
}

pub(super) fn discover() -> Vec<MountPolicy> {
    let session = OwnedCf(unsafe { DASessionCreate(ptr::null()) });
    let mut buffer: *mut libc::statfs = ptr::null_mut();
    let count = unsafe { libc::getmntinfo(&mut buffer, libc::MNT_NOWAIT) };
    if count <= 0 || buffer.is_null() {
        return vec![];
    }
    // Copy libc's shared mount buffer before consulting other native libraries.
    let entries: Vec<_> = unsafe { std::slice::from_raw_parts(buffer, count as usize) }
        .iter()
        .map(|mount| {
            let root = unsafe { CStr::from_ptr(mount.f_mntonname.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            let device = unsafe { CStr::from_ptr(mount.f_mntfromname.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            (root, device, mount.f_flags)
        })
        .collect();
    let mut mounts = Vec::new();
    for (root, device, flags) in entries {
        let (class, identity) = if flags & libc::MNT_LOCAL == 0 {
            (StorageClass::Remote, device)
        } else if session.0.is_null() || !device.starts_with("/dev/") {
            (StorageClass::Unknown, device)
        } else {
            classify(session.0, &device)
        };
        mounts.push(MountPolicy {
            root: PathBuf::from(root),
            identity,
            class,
        });
    }
    mounts
}
