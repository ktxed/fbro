use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::{DirChild, VfsDriver, VfsEntry, VfsError};
use crate::ws::protocol::{FilterSpec, SortSpec};

/// A VFS driver that wraps an inner driver and intercepts mutating operations,
/// forwarding them to an external lfs-spy TUI for approval via Unix domain socket.
pub struct SpyFsDriver {
    inner: Arc<dyn VfsDriver>,
    sock_path: String,
}

impl SpyFsDriver {
    pub fn new(inner: Arc<dyn VfsDriver>, sock_path: String) -> Self {
        Self { inner, sock_path }
    }

    /// Send an intercept request to lfs-spy and wait for a response.
    /// Returns `Ok(chunk_delay_ms)` if approved (0 = no delay).
    /// Returns `Err(PermissionDenied)` if denied.
    /// Returns `Ok(0)` (pass-through) if lfs-spy is not running.
    fn intercept(&self, op: &str, args: serde_json::Value) -> Result<u64, VfsError> {
        let stream = match UnixStream::connect(&self.sock_path) {
            Ok(s) => s,
            Err(_) => return Ok(0), // lfs-spy not running — pass through
        };

        stream.set_read_timeout(Some(Duration::from_secs(60))).ok();
        stream.set_write_timeout(Some(Duration::from_secs(5))).ok();

        let id = uuid::Uuid::new_v4().to_string();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let request = json!({ "id": id, "op": op, "args": args, "ts": ts });
        let mut msg = serde_json::to_string(&request)
            .map_err(|e| VfsError::IOError(format!("spy serialize: {e}")))?;
        msg.push('\n');

        let mut writer = stream
            .try_clone()
            .map_err(|e| VfsError::IOError(format!("spy clone: {e}")))?;
        writer
            .write_all(msg.as_bytes())
            .map_err(|e| VfsError::IOError(format!("spy write: {e}")))?;
        writer
            .flush()
            .map_err(|e| VfsError::IOError(format!("spy flush: {e}")))?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| VfsError::IOError(format!("spy read: {e}")))?;

        let resp: serde_json::Value = serde_json::from_str(line.trim())
            .map_err(|e| VfsError::IOError(format!("spy parse: {e}")))?;

        match resp.get("action").and_then(|v| v.as_str()) {
            Some("approve") => {
                let delay = resp
                    .get("chunk_delay_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                Ok(delay)
            }
            Some("deny") => Err(VfsError::PermissionDenied(format!("lfs-spy denied: {op}"))),
            other => Err(VfsError::IOError(format!(
                "spy unexpected action: {other:?}"
            ))),
        }
    }
}

// ─── Delayed reader ───────────────────────────────────────────────────────────

/// Wraps a `Read` impl and sleeps `delay` before each `read()` call.
/// Used to simulate slow transfers for testing.
struct DelayedReader<R: Read + Send> {
    inner: R,
    delay: Duration,
}

impl<R: Read + Send> Read for DelayedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        std::thread::sleep(self.delay);
        self.inner.read(buf)
    }
}

// ─── VfsDriver impl ───────────────────────────────────────────────────────────

impl VfsDriver for SpyFsDriver {
    fn is_local(&self) -> bool {
        self.inner.is_local()
    }

    // ── Pass-through (non-mutating) ──────────────────────────────────────────

    fn list(
        &self,
        path: &str,
        sort: &SortSpec,
        filter: Option<&FilterSpec>,
        show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        self.inner.list(path, sort, filter, show_hidden)
    }

    fn resolve_child(&self, base: &str, name: &str) -> String {
        self.inner.resolve_child(base, name)
    }

    fn parent_path(&self, path: &str) -> Option<String> {
        self.inner.parent_path(path)
    }

    fn exists(&self, path: &str) -> bool {
        self.inner.exists(path)
    }

    fn file_size(&self, path: &str) -> Option<u64> {
        self.inner.file_size(path)
    }

    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
        self.inner.read_chunk(path, offset, size)
    }

    fn read_dir_entries(&self, path: &str) -> Result<Vec<DirChild>, VfsError> {
        self.inner.read_dir_entries(path)
    }

    // ── Intercepted (mutating) ───────────────────────────────────────────────

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        let chunk_delay_ms = self.intercept("open_read", json!({ "path": path }))?;
        let reader = self.inner.open_read(path)?;
        if chunk_delay_ms > 0 {
            Ok(Box::new(DelayedReader {
                inner: reader,
                delay: Duration::from_millis(chunk_delay_ms),
            }))
        } else {
            Ok(reader)
        }
    }

    fn open_write(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        self.intercept("open_write", json!({ "path": path }))?;
        self.inner.open_write(path)
    }

    fn create_dir_all(&self, path: &str) -> Result<(), VfsError> {
        self.intercept("create_dir_all", json!({ "path": path }))?;
        self.inner.create_dir_all(path)
    }

    fn remove_file(&self, path: &str) -> Result<(), VfsError> {
        self.intercept("remove_file", json!({ "path": path }))?;
        self.inner.remove_file(path)
    }

    fn remove_dir_all(&self, path: &str) -> Result<(), VfsError> {
        self.intercept("remove_dir_all", json!({ "path": path }))?;
        self.inner.remove_dir_all(path)
    }

    fn try_rename(&self, src: &str, dst: &str) -> Result<bool, VfsError> {
        self.intercept("try_rename", json!({ "src": src, "dst": dst }))?;
        self.inner.try_rename(src, dst)
    }
}
