use super::*;
use std::io::{Cursor, Read, Write};
use std::sync::{Arc, Mutex as StdMutex};

use crate::platform::{Platform, VolumeInfo};
use crate::state::{AppState, SharedRoot, SharedRootOrigin};
use crate::vfs::{DirChild, VfsDriver, VfsEntry, VfsError};
use crate::ws::protocol::{FilterSpec, SortSpec};
use russh_sftp::server::Handler as SftpServerHandler;

#[derive(Default)]
struct RecordingDriver {
    read_paths: StdMutex<Vec<String>>,
    cancel_during_read: StdMutex<Option<(SharedState, String)>>,
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
        if let Some((state, job_id)) = self.cancel_during_read.lock().unwrap().take() {
            let mut state = state.blocking_lock();
            cancel_outgoing_test_job(&mut state, &job_id);
        }
        let data = b"payload";
        let start = offset as usize;
        if start >= data.len() {
            return Ok(vec![]);
        }
        let end = start.saturating_add(size).min(data.len());
        Ok(data[start..end].to_vec())
    }
}

struct ManifestHashDriver {
    data: Vec<u8>,
    streaming_supported: bool,
    open_reads: StdMutex<usize>,
    read_chunks: StdMutex<usize>,
}

impl ManifestHashDriver {
    fn new(data: &[u8], streaming_supported: bool) -> Self {
        Self {
            data: data.to_vec(),
            streaming_supported,
            open_reads: StdMutex::new(0),
            read_chunks: StdMutex::new(0),
        }
    }
}

impl VfsDriver for ManifestHashDriver {
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
        *self.open_reads.lock().unwrap() += 1;
        if !self.streaming_supported {
            return Err(VfsError::NotSupported("streaming disabled".to_string()));
        }
        Ok(Box::new(Cursor::new(self.data.clone())))
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

    fn exists(&self, _path: &str) -> bool {
        true
    }

    fn file_size(&self, _path: &str) -> Option<u64> {
        Some(self.data.len() as u64)
    }

    fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> {
        Ok(vec![])
    }

    fn read_chunk(&self, _path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
        *self.read_chunks.lock().unwrap() += 1;
        let start = offset as usize;
        if start >= self.data.len() {
            return Ok(vec![]);
        }
        let end = start.saturating_add(size).min(self.data.len());
        Ok(self.data[start..end].to_vec())
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
            eject: None,
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

fn empty_test_state() -> SharedState {
    let platform: Arc<dyn Platform> = Arc::new(TestPlatform);
    Arc::new(tokio::sync::Mutex::new(AppState::new_default(
        platform.clone(),
        std::collections::HashMap::new(),
        0,
        crate::thumb_pool::ThumbPool::start(platform, 1),
        Arc::new(crate::fileserver::ThumbCache::new(1)),
    )))
}

#[tokio::test]
async fn remote_peer_notification_is_published_without_any_gui_connection() {
    let state = empty_test_state();
    let service = state.lock().await.notifications.clone();
    let (_, mut updates) = service.subscribe();
    let one = crate::notifications::remote::source_registered(&state, "remote:peer:one").await;
    let two = crate::notifications::remote::source_registered(&state, "remote:peer:two").await;
    let broadcaster = GuiStatusBroadcaster::new(state.clone(), "remote:peer:one".into(), one);
    let other = GuiStatusBroadcaster::new(state.clone(), "remote:peer:two".into(), two);
    use crate::vfs::sftp::StatusBroadcaster;
    broadcaster.emit("remote:peer:one", "connected", None, None);
    other.emit("remote:peer:two", "connected", None, None);
    let update = tokio::time::timeout(Duration::from_secs(4), updates.recv())
        .await.expect("settled peer notification").expect("notification update");
    let crate::notifications::NotificationPatch::Added { added, .. } = update.patch else {
        panic!("expected notification publication");
    };
    assert!(added.channels.contains(&crate::notifications::NotificationChannel::Os));
    assert!(added.message.contains("connected"));
    assert_eq!(service.snapshot().entries.len(), 1, "one alert covers both sources");
    assert!(state.lock().await.gui_senders.is_empty());
}

#[tokio::test]
async fn short_incoming_sessions_keep_callback_order_without_false_peer_alerts() {
    let state = empty_test_state();
    let service = state.lock().await.notifications.clone();
    let (_, mut updates) = service.subscribe();
    let (sender, mut messages) = tokio::sync::mpsc::channel(4);
    state.lock().await.gui_senders.push(sender);
    let events = GuiSessionEvents::new(state.clone());
    events.started("peer");
    events.ended("peer");
    let started = tokio::time::timeout(Duration::from_secs(1), messages.recv()).await.unwrap().unwrap();
    let ended = tokio::time::timeout(Duration::from_secs(1), messages.recv()).await.unwrap().unwrap();
    assert!(matches!(started, ServerMessage::RemoteSessionStarted { .. }));
    assert!(matches!(ended, ServerMessage::RemoteSessionEnded { .. }));
    assert!(tokio::time::timeout(Duration::from_millis(2200), updates.recv()).await.is_err());
    assert!(service.snapshot().entries.is_empty());
}

#[tokio::test]
async fn reverse_peer_connections_reach_os_delivery_while_outgoing_stays_open() {
    struct DeliveryPlatform(tokio::sync::mpsc::UnboundedSender<String>);
    impl Platform for DeliveryPlatform {
        fn home_dir(&self) -> String { "/".into() }
        fn open_in_editor(&self, _: &str) -> Result<String, String> { Err("unused".into()) }
        fn supports_os_notifications(&self) -> bool { true }
        fn send_os_notification(&self, message: &str) -> Result<(), String> {
            self.0.send(message.into()).map_err(|error| error.to_string())
        }
    }
    let state = empty_test_state();
    let (delivered, mut deliveries) = tokio::sync::mpsc::unbounded_channel();
    let service = Arc::new(crate::notifications::NotificationService::with_desktop(
        Arc::new(DeliveryPlatform(delivered)),
    ));
    service.desktop_activity().observe_window("main", Some(false));
    state.lock().await.notifications = service.clone();
    let source = "remote:peer:one";
    let generation = crate::notifications::remote::source_registered(&state, source).await;
    let broadcaster = GuiStatusBroadcaster::new(state.clone(), source.into(), generation);
    use crate::vfs::sftp::StatusBroadcaster;
    broadcaster.emit(source, "connected", None, None);
    let outgoing = tokio::time::timeout(Duration::from_secs(4), deliveries.recv()).await.unwrap().unwrap();
    assert!(outgoing.starts_with("Remote fbro connected:"));
    assert!(outgoing.ends_with("(outgoing)"));

    let sessions = GuiSessionEvents::new(state.clone());
    sessions.started("peer");
    let incoming = tokio::time::timeout(Duration::from_secs(4), deliveries.recv()).await.unwrap().unwrap();
    assert!(incoming.starts_with("Remote fbro connected:"));
    assert!(incoming.ends_with("(incoming)"));
    sessions.ended("peer");
    let disconnected = tokio::time::timeout(Duration::from_secs(4), deliveries.recv()).await.unwrap().unwrap();
    assert!(disconnected.starts_with("Remote fbro disconnected:"));
    assert!(disconnected.ends_with("(incoming)"));

    crate::notifications::remote::source_removed(&state, source).await;
    let outgoing_disconnected = tokio::time::timeout(Duration::from_secs(4), deliveries.recv()).await.unwrap().unwrap();
    assert!(outgoing_disconnected.starts_with("Remote fbro disconnected:"));
    assert!(outgoing_disconnected.ends_with("(outgoing)"));
    assert_eq!(service.snapshot().entries.len(), 4);
    assert!(state.lock().await.gui_senders.is_empty());
}

#[tokio::test]
async fn outgoing_transfer_notification_is_owned_by_the_server_before_gui_delivery() {
    let state = empty_test_state();
    let first = start_outgoing_transfer(&state, "peer", "client:notification-test")
        .await.expect("outgoing transfer");
    let service = state.lock().await.notifications.clone();
    let entries = service.snapshot().entries;
    assert_eq!(entries.len(), 1);
    assert!(entries[0].channels.contains(&crate::notifications::NotificationChannel::Os));
    assert!(state.lock().await.gui_senders.is_empty());
    let _second = start_outgoing_transfer(&state, "peer", "client:notification-test")
        .await.expect("reconnected transfer");
    assert_eq!(service.snapshot().entries.len(), 1, "transfer identity survives transport reconnect");
    finish_outgoing_transfer(state.clone(), first.job_id).await;
    assert!(!service.snapshot().entries.iter().any(|entry| {
        entry.channels.contains(&crate::notifications::NotificationChannel::Os)
            && entry.message.contains("ended")
    }), "short successful transfer must not emit an OS end alert");
}

#[tokio::test]
async fn outgoing_transfer_resumed_after_short_idle_can_publish_a_long_end_notification() {
    let state = empty_test_state();
    let mut transfer = start_outgoing_transfer(&state, "peer", "client:resumed-notification")
        .await.expect("outgoing transfer");
    finish_outgoing_transfer(state.clone(), transfer.job_id.clone()).await;
    {
        let mut guard = state.lock().await;
        let job = guard.job_store.jobs.get_mut(&transfer.job_id).unwrap();
        assert_eq!(job.state, JobState::Completed);
        job.notification.started_at = Some(Instant::now() - Duration::from_secs(31));
    }
    emit_outgoing_progress(&state, &mut transfer, true).await;
    finish_outgoing_transfer(state.clone(), transfer.job_id.clone()).await;
    let service = state.lock().await.notifications.clone();
    assert_eq!(service.snapshot().entries.iter().filter(|entry| {
        entry.channels.contains(&crate::notifications::NotificationChannel::Os)
            && entry.message.contains("ended")
    }).count(), 1, "the first short idle end must not swallow the later long completion");
}

#[tokio::test]
async fn remote_manifest_failures_publish_without_a_gui_from_sync_and_async_workers() {
    let state = empty_test_state();
    let service = state.lock().await.notifications.clone();
    let (sync_job, _) = start_remote_manifest_job(&state, "peer", "/sync", "/sync").await;
    let worker_state = state.clone();
    let worker_job = sync_job.clone();
    tokio::task::spawn_blocking(move || {
        emit_remote_manifest_terminal_sync(
            &worker_state, &worker_job, "Failed", Some("Cannot hash remote file".into()),
        );
    }).await.unwrap();
    // Socket-side application or a second worker callback cannot publish twice.
    emit_remote_manifest_terminal(&state, &sync_job, "Failed", None).await;
    assert_eq!(service.snapshot().entries.len(), 1);

    let (async_job, _) = start_remote_manifest_job(&state, "peer", "/async", "/async").await;
    emit_remote_manifest_terminal(
        &state, &async_job, "Failed", Some("Remote hashing worker stopped".into()),
    ).await;
    let entries = service.snapshot().entries;
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| {
        entry.level == crate::notifications::NotificationLevel::Error
            && entry.channels.contains(&crate::notifications::NotificationChannel::Os)
    }));
    assert!(entries.iter().any(|entry| entry.message.contains("Cannot hash remote file")));
    assert!(entries.iter().any(|entry| entry.message.contains("Remote hashing worker stopped")));
    assert!(state.lock().await.gui_senders.is_empty());
}

#[tokio::test]
async fn shared_folder_is_listed_resolved_and_removed_without_a_panel_source() {
    let state = empty_test_state();
    let root = SharedRoot {
        id: "folder-test".into(), name: "Folder".into(), path: "/outside/volumes".into(),
        source_id: "share-folder:folder-test".into(), origin: SharedRootOrigin::Folder,
        icon_name: Some("folder".into()),
    };
    {
        let mut guard = state.lock().await;
        guard.sources.clear();
        guard.drivers.clear();
        guard.drivers.insert(root.source_id.clone(), Arc::new(RecordingDriver::default()));
        guard.share_config.shared_roots = vec![root.clone()];
    }
    let mut handler = SftpHandler::new(state.clone(), "peer-test".into());
    let metadata: serde_json::Value = serde_json::from_slice(&handler.sources_metadata_json().await.unwrap()).unwrap();
    assert_eq!(metadata[0]["id"], root.id);
    assert_eq!(metadata[0]["read_only"], true);
    let handle = handler.opendir(1, "/".into()).await.unwrap();
    let listing = handler.readdir(2, handle.handle).await.unwrap();
    assert_eq!(listing.files.len(), 1);
    assert_eq!(listing.files[0].filename, root.id);
    let (_, path, driver_id, _, _) = resolve_source(&state, &root.id).await.unwrap();
    assert_eq!(path, root.path);
    assert_eq!(driver_id, root.source_id);
    {
        let mut guard = state.lock().await;
        guard.share_config.shared_roots.clear();
        crate::remote::shared_folder::sync_drivers(&mut guard.drivers, &[]);
    }
    assert!(resolve_source(&state, &root.id).await.is_none());
    let metadata: serde_json::Value = serde_json::from_slice(&handler.sources_metadata_json().await.unwrap()).unwrap();
    assert_eq!(metadata, serde_json::json!([]));
}

fn test_manifest_job() -> Arc<Mutex<ManifestJobState>> {
    Arc::new(Mutex::new(ManifestJobState {
        updated_at: unix_millis_now(),
        visible_job_id: "job-test".to_string(),
        last_emit: Some(Instant::now() - OUTGOING_PROGRESS_INTERVAL),
        ..Default::default()
    }))
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

#[test]
fn remote_manifest_hash_streams_when_open_read_is_available() {
    let data = b"remote manifest payload";
    let driver = ManifestHashDriver::new(data, true);
    let state = empty_test_state();
    let job = test_manifest_job();

    let (size, hash) = hash_file_on_remote(
        &state,
        "manifest-test",
        &driver,
        "/src/file.bin",
        "file.bin",
        data.len() as u64,
        &job,
    )
    .expect("streaming hash");

    assert_eq!(size, data.len() as u64);
    assert_eq!(hash, blake3::hash(data).to_hex().to_string());
    assert_eq!(*driver.open_reads.lock().unwrap(), 1);
    assert_eq!(*driver.read_chunks.lock().unwrap(), 0);
}

#[test]
fn remote_manifest_hash_falls_back_to_read_chunk_when_streaming_unsupported() {
    let data = b"remote manifest payload";
    let driver = ManifestHashDriver::new(data, false);
    let state = empty_test_state();
    let job = test_manifest_job();

    let (size, hash) = hash_file_on_remote(
        &state,
        "manifest-test",
        &driver,
        "/src/file.bin",
        "file.bin",
        data.len() as u64,
        &job,
    )
    .expect("fallback hash");

    assert_eq!(size, data.len() as u64);
    assert_eq!(hash, blake3::hash(data).to_hex().to_string());
    assert_eq!(*driver.open_reads.lock().unwrap(), 1);
    assert!(*driver.read_chunks.lock().unwrap() > 0);
}

#[test]
fn remote_manifest_streaming_hash_observes_cancellation() {
    let data = b"remote manifest payload";
    let driver = ManifestHashDriver::new(data, true);
    let state = empty_test_state();
    let job = test_manifest_job();
    job.blocking_lock().cancelled = true;

    let error = hash_file_on_remote(
        &state,
        "manifest-test",
        &driver,
        "/src/file.bin",
        "file.bin",
        data.len() as u64,
        &job,
    )
    .expect_err("cancelled hash should fail");

    assert_eq!(error, "Cancelled");
    assert_eq!(*driver.read_chunks.lock().unwrap(), 0);
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
    assert_eq!(
        manifest_cancel_from_path("/.fbro-manifest/cancel/manifest-h1"),
        Some("manifest-h1".to_string())
    );
    assert!(manifest_status_from_path("/.fbro-manifest/status/../x").is_none());
    assert!(manifest_cancel_from_path("/.fbro-manifest/cancel/../x").is_none());
}

#[test]
fn manifest_status_serializes_file_byte_progress() {
    let response = ManifestStatusResponse {
        done: false,
        cancelled: false,
        processed: 2,
        current_path: Some("movie.mkv".to_string()),
        current_file_bytes_total: 100,
        current_file_bytes_hashed: 40,
        updated_at: 123,
        error: None,
        entries: None,
        errors: None,
    };

    let json = serde_json::to_value(response).unwrap();

    assert_eq!(json["currentPath"], "movie.mkv");
    assert_eq!(json["currentFileBytesTotal"], 100);
    assert_eq!(json["currentFileBytesHashed"], 40);
}


fn cancel_outgoing_test_job(state: &mut AppState, job_id: &str) {
    let job = state.job_store.jobs.get_mut(job_id).unwrap();
    let JobKind::OutgoingTransfer(meta) = &job.kind else {
        panic!("outgoing job expected")
    };
    state
        .cancelled_remote_transfers
        .insert((meta.peer_fp.clone(), meta.transfer_key.clone()));
    job.cancel_tx.send(true).unwrap();
    job.state = JobState::Cancelled;
}

async fn open_test_transfer(handler: &mut SftpHandler, token: &str) -> Result<Handle, StatusCode> {
    handler
        .open(
            1,
            format!("/.fbro-transfer-context/{token}"),
            OpenFlags::READ,
            FileAttributes::default(),
        )
        .await
        .unwrap();
    handler
        .open(
            2,
            "/home-abc123/Downloads/file.jpg".into(),
            OpenFlags::READ,
            FileAttributes::default(),
        )
        .await
}

#[tokio::test]
async fn cancelled_transfer_rejects_reopens_and_reconnects_but_allows_new_jobs_and_peers() {
    let driver = Arc::new(RecordingDriver::default());
    let state = test_state_with_shared_home_and_volume(driver.clone());
    let (tx, mut rx) = tokio::sync::mpsc::channel(32);
    state.lock().await.gui_senders.push(tx);
    let mut handler = SftpHandler::new(state.clone(), "peer".into());
    let file = open_test_transfer(&mut handler, "cancel-token")
        .await
        .unwrap();
    let job_id = handler.transfers["client:cancel-token"].job_id.clone();
    handler.read(3, file.handle.clone(), 0, 3).await.unwrap();
    while rx.try_recv().is_ok() {}
    cancel_outgoing_test_job(&mut *state.lock().await, &job_id);
    assert_eq!(
        handler
            .read(4, file.handle.clone(), 3, 4)
            .await
            .unwrap_err(),
        StatusCode::PermissionDenied
    );
    assert_eq!(
        open_test_transfer(&mut handler, "cancel-token")
            .await
            .unwrap_err(),
        StatusCode::PermissionDenied
    );
    handler
        .note_file_opened(
            "client:cancel-token",
            "/home-abc123/Downloads/file.jpg",
            Some(7),
        )
        .await;
    handler.close(5, file.handle).await.unwrap();
    finish_outgoing_transfer(state.clone(), job_id.clone()).await;
    assert_eq!(
        state.lock().await.job_store.jobs[&job_id].state,
        JobState::Cancelled
    );
    assert!(
        rx.try_recv().is_err(),
        "late progress/completion must not be emitted"
    );
    assert_eq!(driver.read_paths.lock().unwrap().len(), 1);
    let mut reconnect = SftpHandler::new(state.clone(), "peer".into());
    assert_eq!(
        open_test_transfer(&mut reconnect, "cancel-token")
            .await
            .unwrap_err(),
        StatusCode::PermissionDenied
    );
    assert!(
        reconnect.transfers.is_empty(),
        "retry must not create a replacement job"
    );
    let status = reconnect
        .open(
            6,
            "/.fbro-transfer-status/cancel-token".into(),
            OpenFlags::READ,
            FileAttributes::default(),
        )
        .await
        .unwrap();
    let status = reconnect.read(7, status.handle, 0, 128).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.data).unwrap(),
        serde_json::json!({"cancelled": true})
    );
    // Even removal of the UI job cannot revoke cancellation.
    state.lock().await.job_store.jobs.remove(&job_id);
    assert_eq!(
        open_test_transfer(&mut reconnect, "cancel-token")
            .await
            .unwrap_err(),
        StatusCode::PermissionDenied
    );
    let fresh = open_test_transfer(&mut reconnect, "fresh-token")
        .await
        .unwrap();
    assert_eq!(
        reconnect.read(8, fresh.handle, 0, 7).await.unwrap().data,
        b"payload"
    );
    let mut other_peer = SftpHandler::new(state, "other-peer".into());
    let fresh = open_test_transfer(&mut other_peer, "cancel-token")
        .await
        .unwrap();
    assert_eq!(
        other_peer.read(9, fresh.handle, 0, 7).await.unwrap().data,
        b"payload"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_during_blocking_read_discards_bytes_and_keeps_job_cancelled() {
    let driver = Arc::new(RecordingDriver::default());
    let state = test_state_with_shared_home_and_volume(driver.clone());
    let mut handler = SftpHandler::new(state.clone(), "peer".into());
    let file = open_test_transfer(&mut handler, "cancel-during-read")
        .await
        .unwrap();
    let job_id = handler.transfers["client:cancel-during-read"]
        .job_id
        .clone();
    *driver.cancel_during_read.lock().unwrap() = Some((state.clone(), job_id.clone()));
    assert_eq!(
        handler.read(3, file.handle, 0, 7).await.unwrap_err(),
        StatusCode::PermissionDenied
    );
    let transfer = &handler.transfers["client:cancel-during-read"];
    assert_eq!(transfer.progress.total_bytes_copied, 0);
    assert_eq!(
        state.lock().await.job_store.jobs[&job_id].state,
        JobState::Cancelled
    );
}

#[tokio::test]
async fn cancelled_job_ignores_late_lifecycle_events_in_backend_state() {
    let state = empty_test_state();
    let transfer = start_outgoing_transfer(&state, "peer", "client:test")
        .await
        .unwrap();
    cancel_outgoing_test_job(&mut *state.lock().await, &transfer.job_id);
    for event in [
        "Progress",
        "DecisionRequired",
        "Completed",
        "Failed",
        "Resumed",
    ] {
        let mut message = ServerMessage::JobEvent {
            timestamp: now(),
            job_id: transfer.job_id.clone(),
            event: event.into(),
            files: None,
            progress: None,
            search_progress: None,
            decision: None,
            error: None,
            result_path: None,
            result_panel_id: None,
        };
        let accepted = crate::ws::server::apply_job_event_to_state(&state, &mut message).await;
        if matches!(event, "Completed" | "Failed") {
            assert!(accepted);
            assert!(
                matches!(message, ServerMessage::JobEvent { event, .. } if event == "Cancelled")
            );
        } else {
            assert!(!accepted, "{event} must not be forwarded");
        }
        assert_eq!(
            state.lock().await.job_store.jobs[&transfer.job_id].state,
            JobState::Cancelled
        );
    }
}


#[tokio::test(flavor = "multi_thread")]
async fn cancellation_during_remote_verification_discards_digest() {
    let driver = Arc::new(RecordingDriver::default());
    let state = test_state_with_shared_home_and_volume(driver.clone());
    let mut handler = SftpHandler::new(state.clone(), "peer".into());
    open_test_transfer(&mut handler, "cancel-verification")
        .await
        .unwrap();
    let job_id = handler.transfers["client:cancel-verification"]
        .job_id
        .clone();
    *driver.cancel_during_read.lock().unwrap() = Some((state, job_id));
    let encoded =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"/home-abc123/Downloads/file.jpg");
    assert_eq!(
        handler.full_file_digest_json(&encoded).await.unwrap_err(),
        StatusCode::PermissionDenied
    );
}

#[tokio::test]
async fn delayed_completion_cannot_change_cancelled_transfer() {
    let state = empty_test_state();
    let transfer = start_outgoing_transfer(&state, "peer", "client:timer-test")
        .await
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    state.lock().await.gui_senders.push(tx);
    schedule_outgoing_completion(state.clone(), transfer.job_id.clone(), 0);
    cancel_outgoing_test_job(&mut *state.lock().await, &transfer.job_id);
    tokio::time::sleep(OUTGOING_COMPLETE_IDLE + Duration::from_millis(100)).await;
    assert_eq!(
        state.lock().await.job_store.jobs[&transfer.job_id].state,
        JobState::Cancelled
    );
    assert!(rx.try_recv().is_err());
}
