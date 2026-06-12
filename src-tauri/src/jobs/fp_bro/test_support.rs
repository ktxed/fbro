//! Shared helpers for the archive extraction round-trip tests.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::mpsc;

use crate::ws::protocol::ServerMessage;

/// A fresh, unique scratch directory under the system temp dir. Avoids `Date`/`rand`
/// (unavailable here) by combining the process id with a monotonic counter.
pub(super) fn scratch_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("fbro_xt_{tag}_{}_{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

pub(super) fn write_file(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

/// A progress channel whose messages the tests ignore. The receiver is returned so it stays
/// alive for the duration of the test (dropping it is harmless since sends use `try_send`).
pub(super) fn sink() -> (mpsc::Sender<ServerMessage>, mpsc::Receiver<ServerMessage>) {
    mpsc::channel(1024)
}
