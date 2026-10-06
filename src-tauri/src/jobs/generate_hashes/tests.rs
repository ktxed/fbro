use super::*;
use crate::vfs::{local::LocalFsDriver, DirChild, VfsEntry};
use crate::ws::protocol::{FilterSpec, HashCheckStatus};
use std::{io::Read, path::PathBuf};

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("fbro-generate-hashes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn folder(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
    fn write(&self, name: &str, contents: impl AsRef<[u8]>) {
        std::fs::write(self.0.join(name), contents).unwrap();
    }
    fn input(&self, name: &str, kind: &str) -> BroInputItem {
        BroInputItem {
            name: name.into(),
            kind: kind.into(),
            path: LocalFsDriver.resolve_child(&self.folder(), name),
        }
    }
    fn options(
        &self,
        names: &[(&str, &str)],
        overwrite: bool,
        driver: Arc<dyn VfsDriver>,
    ) -> HashGenerateOptions {
        HashGenerateOptions {
            driver,
            folder: self.folder(),
            inputs: names
                .iter()
                .map(|(name, kind)| self.input(name, kind))
                .collect(),
            overwrite_existing: overwrite,
        }
    }
    fn names(&self) -> Vec<String> {
        self.0
            .read_dir()
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy)]
enum Fault {
    Read,
    Write,
    Flush,
    Rename,
    CancelFlush,
    CancelRename,
    Collision,
}
struct FaultDriver {
    fault: Fault,
    cancel: Arc<AtomicBool>,
}
struct FaultWriter {
    inner: Box<dyn Write + Send>,
    fault: Fault,
    cancel: Arc<AtomicBool>,
}
impl Write for FaultWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if matches!(self.fault, Fault::Write) {
            self.inner.write_all(&buf[..buf.len().min(4)])?;
            return Err(std::io::Error::other("fixture write error"));
        }
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if matches!(self.fault, Fault::Flush) {
            return Err(std::io::Error::other("fixture flush error"));
        }
        if matches!(self.fault, Fault::CancelFlush) {
            self.cancel.store(true, Ordering::SeqCst);
        }
        self.inner.flush()
    }
}
impl VfsDriver for FaultDriver {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn list(
        &self,
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
        hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        LocalFsDriver.list(path, sort, filter, hidden)
    }
    fn resolve_child(&self, base: &str, name: &str) -> String {
        LocalFsDriver.resolve_child(base, name)
    }
    fn parent_path(&self, path: &str) -> Option<String> {
        LocalFsDriver.parent_path(path)
    }
    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        if matches!(self.fault, Fault::Read) && path.ends_with("denied") {
            return Err(VfsError::PermissionDenied("fixture denied".into()));
        }
        LocalFsDriver.open_read(path)
    }
    fn open_write(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        LocalFsDriver.open_write(path)
    }
    fn open_write_new(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        if matches!(self.fault, Fault::Collision) {
            std::fs::write(path, "concurrent hash").unwrap();
        }
        Ok(Box::new(FaultWriter {
            inner: LocalFsDriver.open_write_new(path)?,
            fault: self.fault,
            cancel: self.cancel.clone(),
        }))
    }
    fn try_rename(&self, src: &str, dst: &str) -> Result<bool, VfsError> {
        if matches!(self.fault, Fault::Rename) {
            return Err(VfsError::PermissionDenied("fixture rename denied".into()));
        }
        let outcome = LocalFsDriver.try_rename(src, dst);
        if matches!(self.fault, Fault::CancelRename) {
            self.cancel.store(true, Ordering::SeqCst);
        }
        outcome
    }
    fn create_dir_all(&self, path: &str) -> Result<(), VfsError> {
        LocalFsDriver.create_dir_all(path)
    }
    fn remove_file(&self, path: &str) -> Result<(), VfsError> {
        LocalFsDriver.remove_file(path)
    }
    fn remove_dir_all(&self, path: &str) -> Result<(), VfsError> {
        LocalFsDriver.remove_dir_all(path)
    }
    fn exists(&self, path: &str) -> bool {
        LocalFsDriver.exists(path)
    }
    fn exists_no_follow(&self, path: &str) -> bool {
        LocalFsDriver.exists_no_follow(path)
    }
    fn file_size(&self, path: &str) -> Option<u64> {
        LocalFsDriver.file_size(path)
    }
    fn read_dir_entries(&self, path: &str) -> Result<Vec<DirChild>, VfsError> {
        LocalFsDriver.read_dir_entries(path)
    }
    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
        LocalFsDriver.read_chunk(path, offset, size)
    }
}

fn run(
    options: HashGenerateOptions,
    cancel: &Arc<AtomicBool>,
) -> (HashGenerateBroResultSchema, bool) {
    let (tx, mut rx) = mpsc::channel(4096);
    let cancelled = generate_selection("test-job", options, &tx, cancel).unwrap();
    let mut result = None;
    while let Ok(message) = rx.try_recv() {
        if let ServerMessage::HashGenerateBroResult {
            result: snapshot, ..
        } = message
        {
            result = Some(snapshot);
        }
    }
    (result.unwrap(), cancelled)
}
fn fault_driver(fault: Fault, cancel: &Arc<AtomicBool>) -> Arc<dyn VfsDriver> {
    Arc::new(FaultDriver {
        fault,
        cancel: cancel.clone(),
    })
}

#[test]
fn generates_empty_unicode_files_and_ignores_directories_sidecars_and_links() {
    let dir = TempDir::new();
    dir.write("empty", "");
    dir.write("å file .bin", "sample");
    dir.write("orphan.b3", "existing");
    std::fs::create_dir(dir.0.join("folder")).unwrap();
    dir.write("folder/nested", "not selected");
    let mut inputs = vec![
        ("empty", "file"),
        ("å file .bin", "file"),
        ("orphan.b3", "file"),
        ("folder", "directory"),
    ];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.0.join("empty"), dir.0.join("link")).unwrap();
        inputs.push(("link", "symlink"));
    }
    let (result, cancelled) = run(
        dir.options(&inputs, false, Arc::new(LocalFsDriver)),
        &Arc::new(AtomicBool::new(false)),
    );
    assert!(!cancelled);
    assert_eq!(result.items.len(), 2);
    assert_eq!(result.ignored_count, inputs.len() - 2);
    assert!(result
        .items
        .iter()
        .all(|item| item.status == HashGenerateStatus::Created));
    assert_eq!(
        std::fs::read_to_string(dir.0.join("empty.b3")).unwrap(),
        format!("{}  empty\n", blake3::hash(b"").to_hex())
    );
    assert_eq!(
        std::fs::read_to_string(dir.0.join("å file .bin.b3")).unwrap(),
        format!("{}  å file .bin\n", blake3::hash(b"sample").to_hex())
    );
    assert!(!dir.0.join("folder/nested.b3").exists());
    assert!(!dir.0.join("orphan.b3.b3").exists());
}

#[test]
fn existing_hashes_are_skipped_without_reading_and_overwritten_only_when_requested() {
    let dir = TempDir::new();
    dir.write("denied", "new");
    dir.write("denied.b3", "old");
    let cancel = Arc::new(AtomicBool::new(false));
    let (result, _) = run(
        dir.options(
            &[("denied", "file")],
            false,
            fault_driver(Fault::Read, &cancel),
        ),
        &cancel,
    );
    assert_eq!(result.items[0].status, HashGenerateStatus::Skipped);
    assert_eq!(
        std::fs::read_to_string(dir.0.join("denied.b3")).unwrap(),
        "old"
    );
    let (result, _) = run(
        dir.options(&[("denied", "file")], true, Arc::new(LocalFsDriver)),
        &cancel,
    );
    assert_eq!(result.items[0].status, HashGenerateStatus::Replaced);
    assert_eq!(
        std::fs::read_to_string(dir.0.join("denied.b3")).unwrap(),
        format!("{}  denied\n", blake3::hash(b"new").to_hex())
    );
    assert!(!dir
        .names()
        .iter()
        .any(|name| name.starts_with(".fbro-hash-")));
}

#[test]
fn directory_and_symlink_destinations_are_never_followed_or_overwritten() {
    let dir = TempDir::new();
    dir.write("a", "contents");
    std::fs::create_dir(dir.0.join("a.b3")).unwrap();
    let mut inputs = vec![("a", "file")];
    #[cfg(unix)]
    {
        dir.write("b", "contents");
        dir.write("target", "keep");
        std::os::unix::fs::symlink(dir.0.join("target"), dir.0.join("b.b3")).unwrap();
        inputs.push(("b", "file"));
    }
    for overwrite in [false, true] {
        let (result, _) = run(
            dir.options(&inputs, overwrite, Arc::new(LocalFsDriver)),
            &Arc::new(AtomicBool::new(false)),
        );
        assert!(result.items.iter().all(|item| item.status
            == if overwrite {
                HashGenerateStatus::Error
            } else {
                HashGenerateStatus::Skipped
            }));
    }
    assert!(dir.0.join("a.b3").is_dir());
    #[cfg(unix)]
    assert_eq!(
        std::fs::read_to_string(dir.0.join("target")).unwrap(),
        "keep"
    );
}

#[test]
fn write_flush_and_rename_failures_preserve_old_hash_and_clean_partial_outputs() {
    for fault in [Fault::Write, Fault::Flush, Fault::Rename] {
        for overwrite in [false, true] {
            if !overwrite && matches!(fault, Fault::Rename) {
                continue;
            }
            let dir = TempDir::new();
            dir.write("a", "contents");
            if overwrite {
                dir.write("a.b3", "old");
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let (result, cancelled) = run(
                dir.options(&[("a", "file")], overwrite, fault_driver(fault, &cancel)),
                &cancel,
            );
            assert!(!cancelled);
            assert_eq!(result.items[0].status, HashGenerateStatus::Error);
            if overwrite {
                assert_eq!(std::fs::read_to_string(dir.0.join("a.b3")).unwrap(), "old");
            } else {
                assert!(!dir.0.join("a.b3").exists());
            }
            assert!(!dir
                .names()
                .iter()
                .any(|name| name.starts_with(".fbro-hash-")));
        }
    }
}

#[test]
fn read_failure_does_not_prevent_later_files() {
    let dir = TempDir::new();
    dir.write("denied", "contents");
    dir.write("ok", "contents");
    let cancel = Arc::new(AtomicBool::new(false));
    let (result, _) = run(
        dir.options(
            &[("denied", "file"), ("ok", "file")],
            false,
            fault_driver(Fault::Read, &cancel),
        ),
        &cancel,
    );
    assert_eq!(result.items[0].status, HashGenerateStatus::Error);
    assert_eq!(result.items[1].status, HashGenerateStatus::Created);
    assert!(!dir.0.join("denied.b3").exists());
}

#[test]
fn concurrent_creation_is_skipped_without_overwriting() {
    let dir = TempDir::new();
    dir.write("a", "contents");
    let cancel = Arc::new(AtomicBool::new(false));
    let (result, _) = run(
        dir.options(
            &[("a", "file")],
            false,
            fault_driver(Fault::Collision, &cancel),
        ),
        &cancel,
    );
    assert_eq!(result.items[0].status, HashGenerateStatus::Skipped);
    assert_eq!(
        std::fs::read_to_string(dir.0.join("a.b3")).unwrap(),
        "concurrent hash"
    );
}

#[test]
fn cancellation_before_commit_cleans_output_and_preserves_previous_hash() {
    for overwrite in [false, true] {
        let dir = TempDir::new();
        dir.write("a", "contents");
        if overwrite {
            dir.write("a.b3", "old");
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let (result, cancelled) = run(
            dir.options(
                &[("a", "file")],
                overwrite,
                fault_driver(Fault::CancelFlush, &cancel),
            ),
            &cancel,
        );
        assert!(cancelled);
        assert_eq!(result.items[0].status, HashGenerateStatus::Cancelled);
        if overwrite {
            assert_eq!(std::fs::read_to_string(dir.0.join("a.b3")).unwrap(), "old");
        } else {
            assert!(!dir.0.join("a.b3").exists());
        }
        assert!(!dir
            .names()
            .iter()
            .any(|name| name.starts_with(".fbro-hash-")));
    }
}

#[test]
fn cancellation_after_commit_keeps_saved_file_and_cancels_remaining_files() {
    let dir = TempDir::new();
    dir.write("a", "contents");
    dir.write("b", "contents");
    let cancel = Arc::new(AtomicBool::new(false));
    let (result, cancelled) = run(
        dir.options(
            &[("a", "file"), ("b", "file")],
            true,
            fault_driver(Fault::CancelRename, &cancel),
        ),
        &cancel,
    );
    assert!(cancelled);
    assert_eq!(result.items[0].status, HashGenerateStatus::Created);
    assert_eq!(result.items[1].status, HashGenerateStatus::Cancelled);
    assert!(dir.0.join("a.b3").exists());
    assert!(!dir.0.join("b.b3").exists());
    cancel.store(false, Ordering::SeqCst);
    let (result, cancelled) = run(
        dir.options(
            &[("a", "file")],
            true,
            fault_driver(Fault::CancelRename, &cancel),
        ),
        &cancel,
    );
    assert!(!cancelled);
    assert_eq!(result.items[0].status, HashGenerateStatus::Replaced);
}

#[tokio::test]
async fn generated_hashes_round_trip_through_checker_including_linebreak_names() {
    let dir = TempDir::new();
    dir.write("å file", "sample");
    dir.write("empty", "");
    let mut names = vec!["å file", "empty"];
    #[cfg(unix)]
    {
        dir.write("line\nbreak", "sample");
        names.push("line\nbreak");
    }
    let inputs = names.iter().map(|name| (*name, "file")).collect::<Vec<_>>();
    let (tx, mut rx) = mpsc::channel(128);
    let (_, cancel_rx) = watch::channel(false);
    run_generate_hashes(
        "generate".into(),
        dir.options(&inputs, false, Arc::new(LocalFsDriver)),
        tx,
        cancel_rx,
    )
    .await;
    let mut terminal = None;
    while let Ok(message) = rx.try_recv() {
        if let ServerMessage::JobEvent { event, .. } = message {
            if event != "Progress" {
                terminal = Some(event);
            }
        }
    }
    assert_eq!(terminal.as_deref(), Some("Completed"));
    let (tx, mut rx) = mpsc::channel(128);
    let (_, cancel_rx) = watch::channel(false);
    crate::jobs::check_hashes::run_check_hashes(
        "check".into(),
        crate::jobs::check_hashes::HashCheckOptions {
            driver: Arc::new(LocalFsDriver),
            folder: dir.folder(),
            inputs: names
                .iter()
                .map(|name| dir.input(&format!("{name}.b3"), "file"))
                .collect(),
            save_result: false,
        },
        tx,
        cancel_rx,
    )
    .await;
    let mut final_result = None;
    while let Ok(message) = rx.try_recv() {
        if let ServerMessage::HashCheckBroResult { result, .. } = message {
            final_result = Some(result);
        }
    }
    let result = final_result.unwrap();
    assert_eq!(result.items.len(), names.len());
    assert!(result
        .items
        .iter()
        .all(|item| item.status == HashCheckStatus::Matched));
}

#[tokio::test]
async fn pre_cancelled_job_sends_final_results_before_terminal_event_and_saves_nothing() {
    let dir = TempDir::new();
    dir.write("a", "contents");
    let (tx, mut rx) = mpsc::channel(128);
    let (_, cancel_rx) = watch::channel(true);
    run_generate_hashes(
        "generate".into(),
        dir.options(&[("a", "file")], false, Arc::new(LocalFsDriver)),
        tx,
        cancel_rx,
    )
    .await;
    let mut messages = Vec::new();
    while let Ok(message) = rx.try_recv() {
        messages.push(message);
    }
    assert!(
        matches!(messages.last(), Some(ServerMessage::JobEvent { event, .. }) if event == "Cancelled")
    );
    assert!(
        matches!(messages.iter().rev().nth(1), Some(ServerMessage::HashGenerateBroResult { result, .. }) if result.items[0].status == HashGenerateStatus::Cancelled)
    );
    assert!(!dir.0.join("a.b3").exists());
}

#[test]
fn full_selection_is_processed_beyond_discovery_sample() {
    let dir = TempDir::new();
    let names = (0..121).map(|i| format!("file-{i}")).collect::<Vec<_>>();
    for name in &names {
        dir.write(name, "");
    }
    let inputs = names
        .iter()
        .map(|name| (name.as_str(), "file"))
        .collect::<Vec<_>>();
    let (result, _) = run(
        dir.options(&inputs, false, Arc::new(LocalFsDriver)),
        &Arc::new(AtomicBool::new(false)),
    );
    assert_eq!(result.items.len(), 121);
    assert!(result
        .items
        .iter()
        .all(|item| item.status == HashGenerateStatus::Created));
}
