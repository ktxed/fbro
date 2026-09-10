use super::*;

fn mount(root: &str, class: StorageClass) -> MountPolicy {
    MountPolicy {
        root: root.into(),
        identity: root.into(),
        class,
    }
}

#[test]
fn only_internal_ssd_allows_prefetch() {
    for class in [
        StorageClass::Rotational,
        StorageClass::RemovableExternal,
        StorageClass::Remote,
        StorageClass::Virtual,
        StorageClass::Unknown,
    ] {
        assert!(!class.eligible());
        assert!(!combine([StorageClass::InternalSsd, class]).eligible());
    }
    assert!(combine([StorageClass::InternalSsd, StorageClass::InternalSsd]).eligible());
    assert!(!combine([]).eligible());
}

#[test]
fn remote_and_virtual_sources_never_inherit_host_storage() {
    let policy = StorageRegistry::default();
    policy.allow_test_root(&std::env::temp_dir());
    let root = std::env::temp_dir();
    let root = root.to_str().unwrap();
    assert_eq!(
        policy.source_class("remote", true, root),
        StorageClass::Remote
    );
    assert_eq!(
        policy.source_class("virtual", true, root),
        StorageClass::Virtual
    );
    assert_eq!(
        policy.source_class("filesystem", false, root),
        StorageClass::Unknown
    );
    assert_eq!(
        policy.source_class("filesystem", true, root),
        StorageClass::InternalSsd
    );
}

#[cfg(unix)]
#[test]
fn longest_mount_match_includes_hidden_nested_mounts_and_component_boundaries() {
    let mounts = vec![
        mount("/", StorageClass::InternalSsd),
        mount("/home/user/card", StorageClass::RemovableExternal),
        mount("/home/user/card/network", StorageClass::Remote),
    ];
    for (path, expected) in [
        ("/home/user/card/a.jpg", StorageClass::RemovableExternal),
        ("/home/user/card/network/a.jpg", StorageClass::Remote),
        ("/home/user/card2/a.jpg", StorageClass::InternalSsd),
        ("relative.jpg", StorageClass::Unknown),
        ("/home/user/card/../a.jpg", StorageClass::Unknown),
    ] {
        assert_eq!(classify_path(&mounts, Path::new(path)), expected, "{path}");
    }
}

#[cfg(unix)]
#[test]
fn symlink_targets_and_mount_replacement_revoke_permits() {
    let root = std::env::temp_dir().join(format!("fbro-storage-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("card")).unwrap();
    std::fs::write(root.join("image.jpg"), b"image").unwrap();
    std::fs::write(root.join("card/image.jpg"), b"image").unwrap();
    std::os::unix::fs::symlink(root.join("card/image.jpg"), root.join("link.jpg")).unwrap();
    let policy = StorageRegistry::default();
    let mut mounts = vec![
        mount(root.to_str().unwrap(), StorageClass::InternalSsd),
        mount(
            root.join("card").to_str().unwrap(),
            StorageClass::RemovableExternal,
        ),
    ];
    policy.replace(mounts.clone());
    assert!(policy
        .authorize(root.join("link.jpg").to_str().unwrap())
        .is_none());
    let permit = policy
        .authorize(root.join("image.jpg").to_str().unwrap())
        .unwrap();
    assert!(!policy.replace(mounts.clone()));
    assert!(policy.valid(&permit));
    mounts[0].identity = "replacement SSD at same path".into();
    assert!(policy.replace(mounts));
    assert!(!policy.valid(&permit));
    policy.replace(vec![]);
    assert!(policy
        .authorize(root.join("image.jpg").to_str().unwrap())
        .is_none());
    std::fs::remove_dir_all(root).unwrap();
}
