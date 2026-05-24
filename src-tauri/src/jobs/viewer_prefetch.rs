use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::vfs::VfsDriver;

pub const PREFETCH_N: usize = 5;

// Match the FE's initial request sizes exactly.
pub const PREFETCH_IMAGE_CAP: usize = 10 * 1024 * 1024; // 10 MiB — IMAGE_LIMIT (baseline)
const PREFETCH_OTHER_CAP: usize = 1 * 1024 * 1024; // 1 MiB — SMALL_LIMIT
                                                   // For large non-image files (> IMAGE_CAP): preload just enough for the initial hex view.
const PREFETCH_HEX_CAP: usize = 64 * 1024; // 64 KiB — matches FE CHUNK_SIZE

// Keep at most this many entries to bound memory usage.
const MAX_CACHE_SIZE: usize = PREFETCH_N * 4 + 1; // 9 for N=2

pub struct PrefetchEntry {
    pub content: Vec<u8>,
    pub file_size: Option<u64>,
}

struct BoundedCache {
    map: HashMap<String, Arc<PrefetchEntry>>,
    order: VecDeque<String>,
}

impl BoundedCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn insert(&mut self, key: String, entry: Arc<PrefetchEntry>) {
        if !self.map.contains_key(&key) {
            if self.order.len() >= MAX_CACHE_SIZE {
                if let Some(old_key) = self.order.pop_front() {
                    self.map.remove(&old_key);
                }
            }
            self.order.push_back(key.clone());
        }
        self.map.insert(key, entry);
    }

    fn get(&self, key: &str) -> Option<Arc<PrefetchEntry>> {
        self.map.get(key).cloned()
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

pub struct ViewerPrefetchQueue {
    cache: Arc<Mutex<BoundedCache>>,
    pending: Arc<Mutex<HashSet<String>>>,
}

impl ViewerPrefetchQueue {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(BoundedCache::new())),
            pending: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn cache_key(source_id: &str, path: &str) -> String {
        format!("{source_id}\0{path}")
    }

    /// Schedule background prefetch for a list of (full_path, size_hint) pairs.
    /// Call this from the viewer intent handler after determining which entries
    /// are the N neighbors of the current file.
    /// `source_id` is used only as a cache namespace; callers should only pass
    /// entries from local, non-shadow sources.
    pub fn schedule_neighbors(
        &self,
        neighbors: Vec<(String, Option<u64>)>,
        source_id: String,
        driver: Arc<dyn VfsDriver>,
        image_cap_bytes: usize,
    ) {
        let cache = Arc::clone(&self.cache);
        let pending = Arc::clone(&self.pending);

        for (path, size_hint) in neighbors {
            // Choose prefetch cap based on file type and size:
            //   - images ≤ image_cap_bytes  → image_cap_bytes (full image for the viewer)
            //   - images >  image_cap_bytes → skip (too large to be useful)
            //   - non-images ≤ OTHER_CAP → OTHER_CAP (full small binary/text)
            //   - non-images > image_cap_bytes → HEX_CAP (just enough for the initial hex view)
            let cap = if is_image_ext(&path) {
                if let Some(sz) = size_hint {
                    if sz > image_cap_bytes as u64 {
                        continue;
                    }
                }
                image_cap_bytes
            } else {
                match size_hint {
                    Some(sz) if sz > image_cap_bytes as u64 => PREFETCH_HEX_CAP,
                    _ => PREFETCH_OTHER_CAP,
                }
            };

            let key = Self::cache_key(&source_id, &path);

            {
                let cache_guard = cache.lock().unwrap();
                if cache_guard.get(&key).is_some() {
                    continue; // already cached
                }
            }
            {
                let mut pending_guard = pending.lock().unwrap();
                if !pending_guard.insert(key.clone()) {
                    continue; // already in flight
                }
            }

            let cache2 = Arc::clone(&cache);
            let pending2 = Arc::clone(&pending);
            let path2 = path.clone();
            let key2 = key.clone();
            let driver2 = Arc::clone(&driver);

            tokio::task::spawn_blocking(move || {
                let file_size = driver2.file_size(&path2);
                let content = match driver2.read_chunk(&path2, 0, cap) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        pending2.lock().unwrap().remove(&key2);
                        return;
                    }
                };
                let entry = Arc::new(PrefetchEntry { content, file_size });
                cache2.lock().unwrap().insert(key2.clone(), entry);
                pending2.lock().unwrap().remove(&key2);
            });
        }
    }

    /// Return cached content for a file if available, without blocking.
    pub fn get_cached(&self, source_id: &str, path: &str) -> Option<Arc<PrefetchEntry>> {
        let key = Self::cache_key(source_id, path);
        self.cache.lock().unwrap().get(&key)
    }

    /// Look up cached content by absolute path alone (for the fileserver, which
    /// has no source_id context).  Cache keys are "{source_id}\0{path}"; null
    /// bytes never appear in real paths so the suffix match is unambiguous.
    /// O(n) but n ≤ MAX_CACHE_SIZE so it's negligible.
    pub fn get_cached_by_path(&self, path: &str) -> Option<Arc<PrefetchEntry>> {
        let cache = self.cache.lock().unwrap();
        let suffix = format!("\0{path}");
        cache
            .map
            .iter()
            .find(|(k, _)| k.ends_with(&suffix))
            .map(|(_, v)| Arc::clone(v))
    }

    /// Clear the entire cache and pending set (call on viewer open/close).
    pub fn clear(&self) {
        self.cache.lock().unwrap().clear();
        self.pending.lock().unwrap().clear();
    }

    pub fn stats(&self) -> PrefetchCacheStats {
        let cache = self.cache.lock().unwrap();
        PrefetchCacheStats {
            entry_count: cache.map.len(),
            total_bytes: cache.map.values().map(|e| e.content.len()).sum(),
            max_entries: MAX_CACHE_SIZE,
        }
    }
}

pub struct PrefetchCacheStats {
    pub entry_count: usize,
    pub total_bytes: usize,
    pub max_entries: usize,
}

/// Returns true for file extensions that the FE treats as images (uses IMAGE_LIMIT).
/// Keep in sync with the FE's `isImageExt()` in viewers/media.ts.
pub fn is_image_ext(path: &str) -> bool {
    let lower = path.to_lowercase();
    matches!(
        lower.rsplit('.').next().unwrap_or(""),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tiff" | "tif"
    )
}
