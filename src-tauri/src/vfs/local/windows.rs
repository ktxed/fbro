use std::fs;
use std::io;
use std::path::Path;
use std::os::windows::fs::FileTypeExt;

pub(super) fn is_dir_link(_path: &str, meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink_dir()
}

pub(super) fn create(target: &Path, link: &Path, is_dir: bool) -> io::Result<()> {
    if is_dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

pub(super) fn remove(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink_dir() {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}
