use super::*;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex as StdMutex};

use crate::platform::{Platform, VolumeInfo};
use crate::state::{AppState, SharedRoot, SharedRootOrigin};
use crate::vfs::{DirChild, VfsDriver, VfsEntry, VfsError};
use crate::ws::protocol::{FilterSpec, SortSpec};
use russh_sftp::server::Handler as SftpServerHandler;

#[derive(Default)]
struct RecordingDriver {
    read_paths: StdMutex<Vec<String>>,
}

impl VfsDriver for RecordingDriver {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn list(
        &self,
        _path: &str,
        _sort: &SortSpec,
        _filter: Option<&FilterSpec>,
        _show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        Ok(vec![])
    }

    fn resolve_child(&self, base: &str, name: &str) -> String {
        compose_driver_path(base, name)
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        path.rsplit_once('/').map(|(parent, _)| parent.to_string())
    }

    fn open_read(&self, _path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        Err(VfsError::NotSupported(
            "open_read unused in test".to_string(),
        ))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(VfsError::NotSupported(
            "open_write unused in test".to_string(),
        ))
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "create_dir_all unused in test".to_string(),
        ))
    }

    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "remove_file unused in test".to_string(),
        ))
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(VfsError::NotSupported(
            "remove_dir_all unused in test".to_string(),
        ))
    }

    fn exists(&self, path: &str) -> bool {
        path == "/home/user/Downloads/file.jpg"
    }

    fn file_size(&self, path: &str) -> Option<u64> {
        (path == "/home/user/Downloads/file.jpg").then_some(7)
    }

    fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> {
        Ok(vec![])
    }

    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
        self.read_paths.lock().unwrap().push(path.to_string());
        let data = b"payload";
        let start = offset as usize;
        if start >= data.len() {
            return Ok(vec![]);
        }
        let end = start.saturating_add(size).min(data.len());
        Ok(data[start..end].to_vec())
    }
}

struct TestPlatform;

impl Platform for TestPlatform {
    fn home_dir(&self) -> String {
        "/home/user".to_string()
    }

    fn open_in_editor(&self, path: &str) -> Result<String, String> {
        Ok(path.to_string())
    }

    fn list_volumes(&self) -> Vec<VolumeInfo> {
        vec![VolumeInfo {
            id: "vol-root".to_string(),
            name: "/".to_string(),
            root_path: "/".to_string(),
            mounted: true,
            device_path: None,
        }]
    }
}

fn test_state_with_shared_home_and_volume(driver: Arc<RecordingDriver>) -> SharedState {
    let platform: Arc<dyn Platform> = Arc::new(TestPlatform);
    let mut state = AppState::new_default(
        platform.clone(),
        std::collections::HashMap::new(),
        0,
        crate::thumb_pool::ThumbPool::start(platform, 1),
        Arc::new(crate::fileserver::ThumbCache::new(1)),
    );
    state
        .drivers
        .insert("vol-root".to_string(), driver.clone() as Arc<dyn VfsDriver>);
    state.share_config.shared_roots = vec![
        SharedRoot {
            id: "home-abc123".to_string(),
            name: "Home".to_string(),
            path: "/home/user".to_string(),
            source_id: "vol-root".to_string(),
            origin: SharedRootOrigin::Home,
            icon_name: None,
        },
        SharedRoot {
            id: "vol-Macintosh HD".to_string(),
            name: "Macintosh HD".to_string(),
            path: "/Volumes/Macintosh HD".to_string(),
            source_id: "vol-root".to_string(),
            origin: SharedRootOrigin::Source,
            icon_name: None,
        },
    ];
    Arc::new(tokio::sync::Mutex::new(state))
}

#[test]
fn parse_root() {
    let (sid, rest) = parse_path("/");
    assert_eq!(sid, None);
    assert_eq!(rest, "");
}

#[test]
fn parse_source_root() {
    let (sid, rest) = parse_path("/src1");
    assert_eq!(sid.as_deref(), Some("src1"));
    assert_eq!(rest, "");
}

#[test]
fn parse_nested() {
    let (sid, rest) = parse_path("/src1/foo/bar");
    assert_eq!(sid.as_deref(), Some("src1"));
    assert_eq!(rest, "foo/bar");
}

#[test]
fn outgoing_transfer_key_splits_sibling_subtrees() {
    assert_eq!(
        outgoing_transfer_key("/src1/a/pics1/img.jpg"),
        "/src1/a/pics1"
    );
    assert_eq!(
        outgoing_transfer_key("/src1/a/pics2/img.jpg"),
        "/src1/a/pics2"
    );
    assert_eq!(outgoing_transfer_key("/src1/file.jpg"), "/src1/file.jpg");
}

#[test]
fn transfer_context_marker_accepts_job_ids() {
    assert_eq!(
        transfer_context_from_marker_path("/.fbro-transfer-context/job-42").as_deref(),
        Some("client:job-42")
    );
    assert!(transfer_context_from_marker_path("/.fbro-transfer-context/../x").is_none());
    assert!(transfer_context_from_marker_path("/src1/file.jpg").is_none());
}

#[test]
fn detects_dotdot() {
    assert!(has_dotdot("/src1/../etc"));
    assert!(has_dotdot("../etc"));
    assert!(!has_dotdot("/src1/foo..bar"));
    assert!(!has_dotdot("/src1/foo"));
}

#[test]
fn entry_to_file_round_trips_dir_vs_file() {
    // Ensures we set the correct FileMode bits — historical bug.
    let f = entry_to_file(&DirEntry {
        name: "foo.txt".to_string(),
        is_dir: false,
        size: Some(42),
        mtime: Some(0),
    });
    assert!(
        !f.attrs.is_dir(),
        "regular file should not be reported as dir"
    );
    assert!(
        f.attrs.is_regular(),
        "regular file should be marked regular"
    );

    let d = entry_to_file(&DirEntry {
        name: "subdir".to_string(),
        is_dir: true,
        size: None,
        mtime: Some(0),
    });
    assert!(d.attrs.is_dir(), "dir should be reported as dir");
    assert!(!d.attrs.is_regular(), "dir should not be marked regular");
}

#[test]
fn compose_paths() {
    assert_eq!(compose_driver_path("/home/u", ""), "/home/u");
    assert_eq!(compose_driver_path("/home/u", "a/b"), "/home/u/a/b");
    assert_eq!(compose_driver_path("/home/u/", "a"), "/home/u/a");
    assert_eq!(compose_driver_path("", "a/b"), "a/b");
    assert_eq!(compose_driver_path("", ""), "");
}

#[tokio::test]
async fn home_read_uses_backing_source_when_volume_is_also_shared() {
    let driver = Arc::new(RecordingDriver::default());
    let state = test_state_with_shared_home_and_volume(driver.clone());
    let mut handler = SftpHandler::new(state, "peer".to_string());

    let handle = handler
        .open(
            1,
            "/home-abc123/Downloads/file.jpg".to_string(),
            OpenFlags::READ,
            FileAttributes::default(),
        )
        .await
        .expect("open shared-root file");

    let data = handler
        .read(2, handle.handle, 0, 7)
        .await
        .expect("read shared-root file");

    assert_eq!(data.data, b"payload");
    assert_eq!(
        driver.read_paths.lock().unwrap().as_slice(),
        ["/home/user/Downloads/file.jpg"]
    );
}

#[test]
fn covered_ranges_ignore_repeated_reads() {
    let mut ranges = Vec::new();
    assert_eq!(add_covered_range(&mut ranges, 0, 100), 100);
    assert_eq!(add_covered_range(&mut ranges, 0, 100), 0);
    assert_eq!(covered_len(&ranges), 100);
}

#[test]
fn covered_ranges_merge_overlaps() {
    let mut ranges = Vec::new();
    assert_eq!(add_covered_range(&mut ranges, 0, 100), 100);
    assert_eq!(add_covered_range(&mut ranges, 50, 150), 50);
    assert_eq!(add_covered_range(&mut ranges, 140, 200), 50);
    assert_eq!(covered_len(&ranges), 200);
}

#[test]
fn covered_ranges_handle_out_of_order_reads() {
    let mut ranges = Vec::new();
    assert_eq!(add_covered_range(&mut ranges, 200, 300), 100);
    assert_eq!(add_covered_range(&mut ranges, 0, 100), 100);
    assert_eq!(add_covered_range(&mut ranges, 100, 200), 100);
    assert_eq!(covered_len(&ranges), 300);
}

#[test]
fn ordered_digest_handles_out_of_order_chunks() {
    let data = b"abcdefghijklmnopqrstuvwxyz";
    let mut digest = OrderedTransferDigest::new(Some(data.len() as u64));
    digest.add_chunk(10, &data[10..]);
    assert!(!digest.is_complete());
    digest.add_chunk(0, &data[..10]);
    assert!(digest.is_complete());
    assert_eq!(
        digest.finalized.as_deref(),
        Some(blake3::hash(data).to_hex().as_str())
    );
}

#[test]
fn ordered_digest_ignores_repeated_reads() {
    let data = b"hello world";
    let mut digest = OrderedTransferDigest::new(Some(data.len() as u64));
    digest.add_chunk(0, data);
    digest.add_chunk(0, data);
    assert!(digest.is_complete());
    assert_eq!(
        digest.finalized.as_deref(),
        Some(blake3::hash(data).to_hex().as_str())
    );
}

#[test]
fn integrity_result_path_parses_job_and_path() {
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"/src/file.bin");
    let path = format!("/.fbro-integrity/result/job-42/{encoded}");
    let (job, encoded_path) = integrity_result_from_path(&path).unwrap();
    assert_eq!(job, "job-42");
    assert_eq!(encoded_path, encoded);
    assert!(integrity_result_from_path("/.fbro-integrity/result/../abc").is_none());
}

#[test]
fn manifest_paths_parse_start_and_status() {
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"/src");
    assert_eq!(
        manifest_start_from_path(&format!("/.fbro-manifest/start/{encoded}")),
        Some(encoded)
    );
    assert_eq!(
        manifest_status_from_path("/.fbro-manifest/status/manifest-h1"),
        Some("manifest-h1".to_string())
    );
    assert!(manifest_status_from_path("/.fbro-manifest/status/../x").is_none());
}
