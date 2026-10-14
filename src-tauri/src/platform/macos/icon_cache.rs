use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

pub(super) struct IconCache {
    memory: HashMap<(String, bool), Option<Vec<u8>>>,
    pub revision: u64,
}

pub(super) fn cache() -> &'static Mutex<IconCache> {
    static CACHE: OnceLock<Mutex<IconCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(IconCache {
            memory: HashMap::new(),
            revision: 0,
        })
    })
}

pub(super) fn directory() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches/fbro/icons/v1"))
}

impl IconCache {
    // Serialize disk reads/writes and native lookup with clearing so an old lookup
    // cannot repopulate the cache after a successful clear.
    pub fn get_or_generate(
        &mut self,
        directory: Option<&Path>,
        value: &str,
        identifier: bool,
        generate: impl FnOnce() -> Option<Vec<u8>>,
    ) -> Option<Vec<u8>> {
        let key = (value.to_string(), identifier);
        if let Some(bytes) = self.memory.get(&key) {
            return bytes.clone();
        }
        let filename = format!(
            "{}.png",
            blake3::hash(format!("{identifier}:{value}").as_bytes()).to_hex()
        );
        let path = directory.map(|dir| dir.join(filename));
        let disk = path.as_ref().and_then(|path| {
            let size = std::fs::metadata(path).ok()?.len();
            if size > 4 * 1024 * 1024 {
                return None;
            }
            let bytes = std::fs::read(path).ok()?;
            valid_png(&bytes).then_some(bytes)
        });
        let bytes = disk.or_else(|| {
            let bytes = generate()?;
            if let Some(path) = &path {
                if let Err(error) = save(path, &bytes) {
                    eprintln!("[icons] Could not persist icon: {error}");
                }
            }
            Some(bytes)
        });
        if self.memory.len() >= 256 {
            self.memory.clear();
        }
        self.memory.insert(key, bytes.clone());
        bytes
    }

    pub fn clear(&mut self, directory: &Path) -> Result<u64, String> {
        match std::fs::remove_dir_all(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Clear file icon cache: {error}")),
        }
        self.memory.clear();
        self.revision += 1;
        Ok(self.revision)
    }
}

fn valid_png(bytes: &[u8]) -> bool {
    // Native PNGs have an IHDR first and a complete IEND last; reject truncated
    // cache writes before exposing them to the WebView.
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        && bytes.get(12..16) == Some(b"IHDR")
        && bytes.ends_with(b"\0\0\0\0IEND\xaeB`\x82")
}

fn save(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn persistent_icons_survive_memory_reset_and_clear_forces_regeneration() {
        let dir =
            TempDir(std::env::temp_dir().join(format!("fbro-icons-{}", uuid::Uuid::new_v4())));
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDRtest\0\0\0\0IEND\xaeB`\x82".to_vec();
        let mut first = IconCache {
            memory: HashMap::new(),
            revision: 0,
        };
        assert_eq!(
            first.get_or_generate(Some(&dir.0), "../PNG", false, || Some(png.clone())),
            Some(png.clone())
        );
        let mut restarted = IconCache {
            memory: HashMap::new(),
            revision: 0,
        };
        assert_eq!(
            restarted.get_or_generate(Some(&dir.0), "../PNG", false, || panic!("disk hit")),
            Some(png.clone())
        );
        assert_eq!(restarted.clear(&dir.0).unwrap(), 1);
        assert_eq!(
            restarted.get_or_generate(Some(&dir.0), "../PNG", false, || Some(png.clone())),
            Some(png)
        );
        let file = std::fs::read_dir(&dir.0)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(file, b"truncated").unwrap();
        restarted.memory.clear();
        assert!(restarted
            .get_or_generate(Some(&dir.0), "../PNG", false, || None)
            .is_none());
        assert_eq!(restarted.clear(&dir.0).unwrap(), 2);
        assert_eq!(restarted.clear(&dir.0).unwrap(), 3);
    }
}
