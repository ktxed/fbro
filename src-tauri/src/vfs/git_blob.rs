use std::any::Any;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::vfs::{DirChild, VfsDriver, VfsEntry, VfsError};
use crate::ws::protocol::{FilterSpec, SortSpec};

/// Stable source/driver id for the session-scoped git raw-file clipboard.
pub const CLIPBOARD_SOURCE_ID: &str = "git-blob-clipboard";

/// One blob reachable through this driver.
#[derive(Clone, Debug)]
pub struct GitBlobRef {
    pub commit: String,
    pub repo_path: String,
}

/// Streaming, read-only VFS over git blobs (`git show commit:path`).
///
/// Unlike [`super::git_diff::GitDiffSnapshotDriver`], content is never fully
/// buffered in memory: `open_read` pipes `git show` stdout.
pub struct GitBlobDriver {
    repo_root: String,
    /// VFS path (usually the file basename) → blob identity.
    blobs: HashMap<String, GitBlobRef>,
}

impl GitBlobDriver {
    pub fn new(repo_root: impl Into<String>, blobs: HashMap<String, GitBlobRef>) -> Self {
        Self {
            repo_root: repo_root.into(),
            blobs,
        }
    }

    /// Convenience: single clipboard entry named with the path basename.
    pub fn single(repo_root: impl Into<String>, commit: impl Into<String>, repo_path: impl Into<String>) -> Self {
        let repo_path = repo_path.into();
        let name = Path::new(&repo_path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| repo_path.clone());
        let mut blobs = HashMap::new();
        blobs.insert(
            name,
            GitBlobRef {
                commit: commit.into(),
                repo_path,
            },
        );
        Self::new(repo_root, blobs)
    }

    pub fn blob_names(&self) -> impl Iterator<Item = &str> {
        self.blobs.keys().map(String::as_str)
    }

    fn blob(&self, path: &str) -> Result<&GitBlobRef, VfsError> {
        self.blobs
            .get(path)
            .or_else(|| {
                // Allow leading "./" or absolute-looking keys from resolve_child.
                let trimmed = path.trim_start_matches("./");
                self.blobs.get(trimmed)
            })
            .ok_or_else(|| VfsError::NotFound(path.to_string()))
    }

    fn object_spec(blob: &GitBlobRef) -> String {
        format!("{}:{}", blob.commit, blob.repo_path)
    }

    fn read_only() -> VfsError {
        VfsError::NotSupported("Git blob clipboard is read-only".to_string())
    }

    /// Whether `commit:repo_path` exists as a blob in the repository.
    pub fn blob_exists(repo_root: &str, commit: &str, repo_path: &str) -> bool {
        let object = format!("{commit}:{repo_path}");
        Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["cat-file", "-e", &object])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    pub fn blob_size(repo_root: &str, commit: &str, repo_path: &str) -> Result<u64, String> {
        let object = format!("{commit}:{repo_path}");
        let output = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .args(["cat-file", "-s", &object])
            .output()
            .map_err(|e| format!("Git is unavailable: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "Blob not found: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u64>()
            .map_err(|e| format!("Could not parse blob size: {e}"))
    }
}

/// Owns a `git show` child and reads from its stdout. Kills the process on drop.
struct GitShowReader {
    child: Child,
    stdout: std::process::ChildStdout,
}

impl GitShowReader {
    fn spawn(repo_root: &str, object: &str) -> Result<Self, VfsError> {
        let mut child = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .arg("show")
            .arg(object)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| VfsError::IOError(format!("Failed to start git show: {e}")))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| VfsError::IOError("git show produced no stdout".into()))?;
        Ok(Self { child, stdout })
    }
}

impl Read for GitShowReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.stdout.read(buf)
    }
}

impl Drop for GitShowReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl VfsDriver for GitBlobDriver {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn list(
        &self,
        _path: &str,
        _sort: &SortSpec,
        _filter: Option<&FilterSpec>,
        _show_hidden: bool,
    ) -> Result<Vec<VfsEntry>, VfsError> {
        Err(Self::read_only())
    }

    fn resolve_child(&self, _base: &str, name: &str) -> String {
        name.to_string()
    }

    fn parent_path(&self, _path: &str) -> Option<String> {
        None
    }

    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        let blob = self.blob(path)?;
        let object = Self::object_spec(blob);
        let reader = GitShowReader::spawn(&self.repo_root, &object)?;
        Ok(Box::new(reader))
    }

    fn open_write(&self, _path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        Err(Self::read_only())
    }

    fn create_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(Self::read_only())
    }

    fn remove_file(&self, _path: &str) -> Result<(), VfsError> {
        Err(Self::read_only())
    }

    fn remove_dir_all(&self, _path: &str) -> Result<(), VfsError> {
        Err(Self::read_only())
    }

    fn exists(&self, path: &str) -> bool {
        self.blob(path).is_ok()
    }

    fn file_size(&self, path: &str) -> Option<u64> {
        let blob = self.blob(path).ok()?;
        Self::blob_size(&self.repo_root, &blob.commit, &blob.repo_path).ok()
    }

    fn read_dir_entries(&self, _path: &str) -> Result<Vec<DirChild>, VfsError> {
        Err(Self::read_only())
    }

    fn read_chunk(&self, path: &str, offset: u64, size: usize) -> Result<Vec<u8>, VfsError> {
        // Stream only what's needed: skip via read for small offsets.
        let mut reader = self.open_read(path)?;
        let mut skip = offset;
        let mut discard = [0u8; 64 * 1024];
        while skip > 0 {
            let n = skip.min(discard.len() as u64) as usize;
            let read = reader
                .read(&mut discard[..n])
                .map_err(|e| VfsError::IOError(e.to_string()))?;
            if read == 0 {
                return Ok(Vec::new());
            }
            skip -= read as u64;
        }
        let mut buf = vec![0u8; size];
        let mut filled = 0;
        while filled < size {
            let n = reader
                .read(&mut buf[filled..])
                .map_err(|e| VfsError::IOError(e.to_string()))?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        buf.truncate(filled);
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_repo() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let repo = std::env::temp_dir().join(format!(
            "fbro-git-blob-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.email", "fbro-tests@example.invalid"]);
        git(&repo, &["config", "user.name", "fbro tests"]);
        fs::write(repo.join("hello.txt"), "hello from blob\n").unwrap();
        git(&repo, &["add", "hello.txt"]);
        git(&repo, &["commit", "-qm", "initial"]);
        repo
    }

    fn git(repo: &std::path::Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn tip_hash(repo: &std::path::Path) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn open_read_streams_blob_bytes() {
        let repo = temp_repo();
        let root = repo.to_str().unwrap();
        let hash = tip_hash(&repo);
        let driver = GitBlobDriver::single(root, &hash, "hello.txt");

        let mut reader = driver.open_read("hello.txt").unwrap();
        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "hello from blob\n");
        assert_eq!(driver.file_size("hello.txt"), Some(16));
        assert!(driver.exists("hello.txt"));
        assert!(!driver.exists("missing.txt"));

        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn blob_exists_and_size_helpers() {
        let repo = temp_repo();
        let root = repo.to_str().unwrap();
        let hash = tip_hash(&repo);
        assert!(GitBlobDriver::blob_exists(root, &hash, "hello.txt"));
        assert!(!GitBlobDriver::blob_exists(root, &hash, "nope.txt"));
        assert_eq!(GitBlobDriver::blob_size(root, &hash, "hello.txt").unwrap(), 16);
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn read_chunk_supports_offset() {
        let repo = temp_repo();
        let root = repo.to_str().unwrap();
        let hash = tip_hash(&repo);
        let driver = GitBlobDriver::single(root, &hash, "hello.txt");
        let chunk = driver.read_chunk("hello.txt", 6, 4).unwrap();
        assert_eq!(&chunk, b"from");
        fs::remove_dir_all(repo).unwrap();
    }
}
