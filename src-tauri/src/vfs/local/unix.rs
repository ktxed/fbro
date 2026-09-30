use std::fs;
use std::io;
use std::path::Path;

pub(super) fn is_dir_link(path: &str, _meta: &fs::Metadata) -> bool {
    fs::metadata(path).map(|meta| meta.is_dir()).unwrap_or(false)
}

pub(super) fn create(target: &Path, link: &Path, _is_dir: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

pub(super) fn remove(path: &Path) -> io::Result<()> {
    fs::remove_file(path)
}
