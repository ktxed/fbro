use super::*;
use crate::vfs::local::LocalFsDriver;
use crate::vfs::{DirChild, VfsEntry, VfsError};
use crate::ws::protocol::{FilterSpec, SortSpec};
use std::io::Read;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("fbro-editor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> String {
        crate::vfs::local::normalize_path(self.0.join(name))
    }
    fn write(&self, name: &str, contents: impl AsRef<[u8]>) -> String {
        std::fs::write(self.0.join(name), contents).unwrap();
        self.path(name)
    }
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Local driver whose rename always fails, to exercise temp-file cleanup.
struct FailingRename;

impl VfsDriver for FailingRename {
    fn is_local(&self) -> bool {
        true
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn list(&self, path: &str, sort: &SortSpec, filter: Option<&FilterSpec>, hidden: bool) -> Result<Vec<VfsEntry>, VfsError> {
        LocalFsDriver.list(path, sort, filter, hidden)
    }
    fn resolve_child(&self, base: &str, name: &str) -> String {
        LocalFsDriver.resolve_child(base, name)
    }
    fn parent_path(&self, path: &str) -> Option<String> {
        LocalFsDriver.parent_path(path)
    }
    fn open_read(&self, path: &str) -> Result<Box<dyn Read + Send>, VfsError> {
        LocalFsDriver.open_read(path)
    }
    fn open_write(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        LocalFsDriver.open_write(path)
    }
    fn open_write_new(&self, path: &str) -> Result<Box<dyn Write + Send>, VfsError> {
        LocalFsDriver.open_write_new(path)
    }
    fn try_rename(&self, _src: &str, _dst: &str) -> Result<bool, VfsError> {
        Err(VfsError::PermissionDenied("fixture denied".into()))
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

#[test]
fn refuses_files_it_cannot_edit() {
    let dir = TempDir::new();
    let big = dir.write("big.txt", vec![b'a'; MAX_EDITOR_BYTES as usize + 1]);
    let binary = dir.write("bin.dat", [0xff, 0xfe, 0x00, 0x80]);
    std::fs::create_dir(dir.0.join("folder")).unwrap();
    assert_eq!(load(&big).err(), Some(EditorError::TooLarge));
    assert_eq!(load(&binary).err(), Some(EditorError::NotUtf8));
    assert_eq!(load(&dir.path("folder")).err(), Some(EditorError::NotRegularFile));
    assert_eq!(load(&dir.path("absent")).err(), Some(EditorError::Missing));
}

#[test]
fn round_trips_crlf_bom_and_missing_final_newline() {
    let dir = TempDir::new();
    for original in [
        b"a\r\nb\r\n".to_vec(),
        b"\xEF\xBB\xBFa\r\nb".to_vec(),
        b"a\nb".to_vec(),
        b"".to_vec(),
    ] {
        let path = dir.write("f.txt", &original);
        let loaded = load(&path).unwrap();
        assert!(!loaded.text.contains('\r'));
        save(&LocalFsDriver, &path, &loaded.text, &loaded.disk, false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
    let path = dir.write("g.txt", "one\r\ntwo\r\n");
    let loaded = load(&path).unwrap();
    save(&LocalFsDriver, &path, "one\nthree\n", &loaded.disk, false).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"one\r\nthree\r\n");
}

#[test]
fn saves_atomically_without_leaving_temp_files() {
    let dir = TempDir::new();
    let path = dir.write("note.md", "old");
    let loaded = load(&path).unwrap();
    let disk = save(&LocalFsDriver, &path, "new text", &loaded.disk, false).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new text");
    assert_eq!(dir.names(), ["note.md"]);
    // The returned state is current: a second save is not stale.
    save(&LocalFsDriver, &path, "newer", &disk, false).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer");
}

#[cfg(unix)]
#[test]
fn keeps_permissions_hard_links_and_symlinks() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let dir = TempDir::new();
    let path = dir.write("script.sh", "echo 1");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o751)).unwrap();
    let loaded = load(&path).unwrap();
    save(&LocalFsDriver, &path, "echo 2", &loaded.disk, false).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o751);

    // A hard link must keep pointing at the same data after saving.
    let linked = dir.write("linked.txt", "one");
    std::fs::hard_link(&linked, dir.0.join("alias.txt")).unwrap();
    let inode = std::fs::metadata(&linked).unwrap().ino();
    let loaded = load(&linked).unwrap();
    save(&LocalFsDriver, &linked, "two", &loaded.disk, false).unwrap();
    assert_eq!(std::fs::metadata(&linked).unwrap().ino(), inode);
    assert_eq!(std::fs::read_to_string(dir.0.join("alias.txt")).unwrap(), "two");

    // Editing through a symlink writes the target and keeps the link.
    let target = dir.write("target.md", "x");
    std::os::unix::fs::symlink(&target, dir.0.join("link.md")).unwrap();
    let resolved = canonical_path(&dir.path("link.md")).unwrap();
    assert_eq!(resolved, canonical_path(&target).unwrap());
    let loaded = load(&resolved).unwrap();
    save(&LocalFsDriver, &resolved, "y", &loaded.disk, false).unwrap();
    assert!(std::fs::symlink_metadata(dir.0.join("link.md")).unwrap().file_type().is_symlink());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "y");
}

#[test]
fn failed_rename_removes_the_temp_file_and_keeps_the_original() {
    let dir = TempDir::new();
    let path = dir.write("note.md", "keep");
    let loaded = load(&path).unwrap();
    let error = save(&FailingRename, &path, "lost", &loaded.disk, false).unwrap_err();
    assert_eq!(error.code(), "IO");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep");
    assert_eq!(dir.names(), ["note.md"]);
}

#[test]
fn detects_same_size_external_edits_and_honours_force() {
    let dir = TempDir::new();
    let path = dir.write("note.md", "aaaa");
    let loaded = load(&path).unwrap();
    // Same size, written within the same second: only the hash can tell.
    std::fs::write(&path, "bbbb").unwrap();
    assert_eq!(disk_change(&path, &loaded.disk).unwrap(), DiskChange::Modified);
    assert_eq!(
        save(&LocalFsDriver, &path, "mine", &loaded.disk, false).err(),
        Some(EditorError::Stale)
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "bbbb");
    save(&LocalFsDriver, &path, "mine", &loaded.disk, true).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "mine");
}

#[test]
fn reports_missing_files_and_can_write_them_back() {
    let dir = TempDir::new();
    let path = dir.write("note.md", "x");
    let loaded = load(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(disk_change(&path, &loaded.disk).unwrap(), DiskChange::Missing);
    assert_eq!(save(&LocalFsDriver, &path, "y", &loaded.disk, false).err(), Some(EditorError::Missing));
    save(&LocalFsDriver, &path, "y", &loaded.disk, true).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "y");
}

#[test]
fn conflict_copies_never_overwrite() {
    let dir = TempDir::new();
    let path = dir.write("note.md", "x");
    let loaded = load(&path).unwrap();
    let first = write_conflict_copy(&LocalFsDriver, &path, "a", &loaded.disk, "2026-11-02 101500").unwrap();
    let second = write_conflict_copy(&LocalFsDriver, &path, "b", &loaded.disk, "2026-11-02 101500").unwrap();
    assert!(first.ends_with("note (conflict 2026-11-02 101500).md"));
    assert!(second.ends_with("note (conflict 2026-11-02 101500 2).md"));
    assert_eq!(std::fs::read_to_string(first).unwrap(), "a");
    assert_eq!(std::fs::read_to_string(second).unwrap(), "b");
}

#[test]
fn untouched_empty_note_detection() {
    let dir = TempDir::new();
    let empty = dir.write("empty.md", "");
    let loaded = load(&empty).unwrap();
    assert!(is_untouched_empty_note(&empty, None));
    assert!(is_untouched_empty_note(&empty, Some(&loaded.disk)));
    std::fs::write(&empty, "typed elsewhere").unwrap();
    assert!(!is_untouched_empty_note(&empty, Some(&loaded.disk)));
    assert!(!is_untouched_empty_note(&dir.path("absent.md"), None));
}
