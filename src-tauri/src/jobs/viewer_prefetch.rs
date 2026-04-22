use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use crate::vfs::VfsDriver;

pub const PREFETCH_N: usize = 2;

// Match the FE's initial request sizes exactly.
const PREFETCH_IMAGE_CAP: usize = 5 * 1024 * 1024; // 5 MiB — IMAGE_LIMIT
const PREFETCH_OTHER_CAP: usize = 1 * 1024 * 1024; // 1 MiB — SMALL_LIMIT

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
    ) {
        let cache = Arc::clone(&self.cache);
        let pending = Arc::clone(&self.pending);

        for (path, size_hint) in neighbors {
            // Skip files above even the image cap — large binaries are virtual-
            // scrolled and the first chunk alone would be pointless to cache.
            if let Some(sz) = size_hint {
                if sz > PREFETCH_IMAGE_CAP as u64 {
                    continue;
                }
            }

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

            let cap = if is_image_ext(&path) {
                PREFETCH_IMAGE_CAP
            } else {
                PREFETCH_OTHER_CAP
            };

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

    /// Clear the entire cache and pending set (call on viewer open/close).
    pub fn clear(&self) {
        self.cache.lock().unwrap().clear();
        self.pending.lock().unwrap().clear();
    }
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
