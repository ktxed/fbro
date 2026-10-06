use super::*;
use crate::vfs::{local::LocalFsDriver, DirChild, VfsEntry};
use crate::ws::protocol::FilterSpec;
use std::path::PathBuf;

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("fbro-check-hashes-{}", uuid::Uuid::new_v4()));
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
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct FaultDriver {
    fail_save: bool,
    cancel_on_save: Option<Arc<AtomicBool>>,
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
        if path.ends_with("unreadable.b3") || path.ends_with("denied.bin") {
            return Err(VfsError::PermissionDenied("fixture denied".into()));
        }
        LocalFsDriver.open_read(path)
    }
    fn open_write(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        LocalFsDriver.open_write(path)
    }
    fn open_write_new(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        if let Some(cancel) = &self.cancel_on_save {
            cancel.store(true, Ordering::SeqCst);
        }
        if self.fail_save {
            return Err(VfsError::PermissionDenied("fixture denied".into()));
        }
        LocalFsDriver.open_write_new(path)
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
    dir: &TempDir,
    names: &[(&str, &str)],
    driver: Arc<dyn VfsDriver>,
    save_result: bool,
    cancelled: bool,
) -> Vec<ServerMessage> {
    let (tx, mut rx) = mpsc::channel(128);
    check_selection(
        "test-job",
        HashCheckOptions {
            folder: dir.folder(),
            driver,
            save_result,
            inputs: names
                .iter()
                .map(|(name, kind)| dir.input(name, kind))
                .collect(),
        },
        &tx,
        &Arc::new(AtomicBool::new(cancelled)),
    )
    .unwrap();
    let mut messages = Vec::new();
    while let Ok(message) = rx.try_recv() {
        messages.push(message);
    }
    messages
}

fn final_result(messages: &[ServerMessage]) -> &HashCheckBroResultSchema {
    messages
        .iter()
        .rev()
        .find_map(|message| match message {
            ServerMessage::HashCheckBroResult { result, .. } => Some(result),
            _ => None,
        })
        .unwrap()
}

#[test]
fn parses_bare_and_filename_hashes_including_unicode_spaces_and_crlf() {
    let hash = blake3::hash(b"sample").to_hex().to_string();
    for contents in [
        hash.clone(),
        format!("{}\r\n", hash.to_uppercase()),
        format!("{hash}  å file .bin\r\n"),
    ] {
        assert_eq!(parse_expected_hash(&contents, "å file .bin").unwrap(), hash);
    }
    for contents in [
        "invalid".into(),
        format!("{hash}  other.bin\n"),
        format!("{hash}  ../a.bin\n"),
        format!("{hash}\n{hash}"),
        format!("{hash}  a.bin\n{hash}  a.bin\n"),
    ] {
        assert!(parse_expected_hash(&contents, "a.bin").is_err());
    }
}

#[test]
fn checks_pairs_continues_after_errors_and_ignores_non_pairs() {
    let dir = TempDir::new();
    let hash = blake3::hash(b"same").to_hex().to_string();
    for name in ["a.y", "wrong", "broken", "unreadable", "denied.bin"] {
        dir.write(name, "same");
    }
    dir.write("a.y.b3", format!("{hash}  a.y\n"));
    dir.write("wrong.b3", "0".repeat(64));
    dir.write("broken.b3", "invalid");
    dir.write("unreadable.b3", &hash);
    dir.write("denied.bin.b3", &hash);
    dir.write("empty", "");
    dir.write("empty.b3", blake3::hash(b"").to_hex().as_str());
    dir.write("orphan.b3", &hash);
    dir.write(".b3", &hash);
    dir.write("folder.b3", &hash);
    std::fs::create_dir(dir.0.join("folder")).unwrap();
    let messages = run(
        &dir,
        &[
            ("a.y.b3", "file"),
            ("wrong.b3", "file"),
            ("broken.b3", "file"),
            ("unreadable.b3", "file"),
            ("denied.bin.b3", "file"),
            ("empty.b3", "file"),
            ("orphan.b3", "file"),
            (".b3", "file"),
            ("a.y", "file"),
            ("folder.b3", "file"),
        ],
        Arc::new(FaultDriver {
            fail_save: false,
            cancel_on_save: None,
        }),
        false,
        false,
    );
    let result = final_result(&messages);
    assert_eq!(result.ignored_count, 4);
    assert_eq!(
        result
            .items
            .iter()
            .map(|item| &item.status)
            .collect::<Vec<_>>(),
        vec![
            &HashCheckStatus::Matched,
            &HashCheckStatus::Mismatched,
            &HashCheckStatus::Error,
            &HashCheckStatus::Error,
            &HashCheckStatus::Error,
            &HashCheckStatus::Matched,
        ]
    );
    assert!(result.items[1].expected_hash.is_some() && result.items[1].actual_hash.is_some());
    assert!(!messages
        .iter()
        .any(|message| matches!(message, ServerMessage::HashCheckReportSaved { .. })));
    assert!(!dir.0.read_dir().unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("fbro-hash-check")));
}

#[test]
fn report_save_errors_preserve_results_and_never_claim_success() {
    let dir = TempDir::new();
    dir.write("a", "same");
    dir.write("a.b3", blake3::hash(b"same").to_hex().as_str());
    let messages = run(
        &dir,
        &[("a.b3", "file")],
        Arc::new(FaultDriver {
            fail_save: true,
            cancel_on_save: None,
        }),
        true,
        false,
    );
    assert_eq!(
        final_result(&messages).items[0].status,
        HashCheckStatus::Matched
    );
    assert!(messages
        .iter()
        .any(|message| matches!(message, ServerMessage::HashCheckReportFailed { .. })));
    assert!(!messages
        .iter()
        .any(|message| matches!(message, ServerMessage::HashCheckReportSaved { .. })));
}

#[test]
fn orphan_only_selection_can_save_an_empty_report() {
    let dir = TempDir::new();
    dir.write("orphan.b3", "0".repeat(64));
    let messages = run(
        &dir,
        &[("orphan.b3", "file")],
        Arc::new(LocalFsDriver),
        true,
        false,
    );
    assert!(final_result(&messages).items.is_empty());
    assert_eq!(final_result(&messages).ignored_count, 1);
    let saved = messages
        .iter()
        .find_map(|message| match message {
            ServerMessage::HashCheckReportSaved { path, .. } => Some(path),
            _ => None,
        })
        .unwrap();
    assert!(std::fs::read_to_string(saved)
        .unwrap()
        .contains("No matching file pairs."));
}

#[test]
fn cancelled_checks_mark_remaining_pairs_and_do_not_save() {
    let dir = TempDir::new();
    dir.write("a", "same");
    dir.write("a.b3", "0".repeat(64));
    let messages = run(
        &dir,
        &[("a.b3", "file")],
        Arc::new(LocalFsDriver),
        true,
        true,
    );
    assert_eq!(
        final_result(&messages).items[0].status,
        HashCheckStatus::Cancelled
    );
    assert!(!messages
        .iter()
        .any(|message| matches!(message, ServerMessage::HashCheckReportSaved { .. })));
}

#[test]
fn exclusive_report_creation_handles_files_and_symlinks_without_overwriting() {
    let dir = TempDir::new();
    dir.write("fbro-hash-check-123.txt", "keep me");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        dir.0.join("target"),
        dir.0.join("fbro-hash-check-123-1.txt"),
    )
    .unwrap();
    let saved = save_report(&LocalFsDriver, &dir.folder(), 123, "new report").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.0.join("fbro-hash-check-123.txt")).unwrap(),
        "keep me"
    );
    assert_eq!(std::fs::read_to_string(saved).unwrap(), "new report");
    #[cfg(unix)]
    assert!(!dir.0.join("target").exists());
}

#[test]
fn cancellation_after_report_commit_does_not_relabel_a_saved_run() {
    let dir = TempDir::new();
    dir.write("a", "");
    dir.write("a.b3", blake3::hash(b"").to_hex().as_str());
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = mpsc::channel(128);
    let cancelled = check_selection(
        "test-job",
        HashCheckOptions {
            driver: Arc::new(FaultDriver {
                fail_save: false,
                cancel_on_save: Some(cancel.clone()),
            }),
            folder: dir.folder(),
            inputs: vec![dir.input("a.b3", "file")],
            save_result: true,
        },
        &tx,
        &cancel,
    )
    .unwrap();
    assert!(!cancelled);
    assert!(cancel.load(Ordering::SeqCst));
    let mut saved = false;
    while let Ok(message) = rx.try_recv() {
        if let ServerMessage::HashCheckReportSaved { path, .. } = message {
            saved = true;
            assert!(std::path::Path::new(&path).exists());
        }
    }
    assert!(saved);
}

#[tokio::test]
async fn cancellation_before_start_sends_final_rows_before_cancelled_terminal() {
    let dir = TempDir::new();
    dir.write("a", "");
    dir.write("a.b3", blake3::hash(b"").to_hex().as_str());
    let (tx, mut rx) = mpsc::channel(128);
    let (_cancel_tx, cancel_rx) = watch::channel(true);
    run_check_hashes(
        "test-job".into(),
        HashCheckOptions {
            driver: Arc::new(LocalFsDriver),
            folder: dir.folder(),
            inputs: vec![dir.input("a.b3", "file")],
            save_result: true,
        },
        tx,
        cancel_rx,
    )
    .await;
    let mut messages = Vec::new();
    while let Ok(message) = rx.try_recv() {
        messages.push(message);
    }
    assert_eq!(
        final_result(&messages).items[0].status,
        HashCheckStatus::Cancelled
    );
    assert!(
        matches!(messages.last(), Some(ServerMessage::JobEvent { event, .. }) if event == "Cancelled")
    );
    assert!(!messages
        .iter()
        .any(|message| matches!(message, ServerMessage::HashCheckReportSaved { .. })));
}

#[test]
fn large_reader_reports_intermediate_progress_with_bounded_reads_and_cancellation() {
    struct GeneratedReader {
        remaining: usize,
    }
    impl Read for GeneratedReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            assert!(buf.len() <= HASH_BUFFER_SIZE);
            let n = self.remaining.min(buf.len()).min(256 * 1024);
            buf[..n].fill(7);
            self.remaining -= n;
            Ok(n)
        }
    }
    let size = 65 * 1024 * 1024;
    let cancel = AtomicBool::new(false);
    let mut updates = Vec::new();
    let actual = hash_reader(GeneratedReader { remaining: size }, &cancel, |count| {
        updates.push(count)
    })
    .unwrap();
    let mut expected = blake3::Hasher::new();
    for _ in 0..65 {
        expected.update(&vec![7; 1024 * 1024]);
    }
    assert_eq!(actual, expected.finalize().to_hex().as_str());
    assert!(updates[0] > 0 && updates[0] < size as u64);
    assert_eq!(updates.last(), Some(&(size as u64)));
    assert!(
        hash_reader(GeneratedReader { remaining: size }, &cancel, |_| {
            cancel.store(true, Ordering::SeqCst);
        })
        .is_err()
    );
}

#[test]
fn stream_errors_are_reported_and_oversized_sidecars_are_rejected() {
    struct BrokenReader;
    impl Read for BrokenReader {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("fixture read error"))
        }
    }
    assert!(hash_reader(BrokenReader, &AtomicBool::new(false), |_| {})
        .unwrap_err()
        .contains("fixture read error"));
    let dir = TempDir::new();
    dir.write("a.b3", vec![b'0'; SIDECAR_LIMIT as usize + 1]);
    assert!(
        read_expected_hash(&LocalFsDriver, &dir.input("a.b3", "file").path, "a")
            .unwrap_err()
            .contains("64 KiB")
    );
}
