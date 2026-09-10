use crate::vfs::VfsDriver;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const PREFETCH_N: usize = 5;
pub const PREFETCH_IMAGE_CAP: usize = 10 * 1024 * 1024;
const PREFETCH_OTHER_CAP: usize = 1024 * 1024;
const PREFETCH_HEX_CAP: usize = 64 * 1024;
const MAX_CACHE_SIZE: usize = PREFETCH_N * 4 + 1;
const MAX_CACHE_BYTES: usize = 256 * 1024 * 1024;
const WORKERS: usize = 2;

pub struct PrefetchEntry {
    pub content: Vec<u8>,
    pub file_size: Option<u64>,
}

struct BoundedCache {
    map: HashMap<String, Arc<PrefetchEntry>>,
    order: VecDeque<String>,
    bytes: usize,
    byte_limit: usize,
}

impl BoundedCache {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            byte_limit: MAX_CACHE_BYTES,
        }
    }

    fn insert(&mut self, key: String, entry: Arc<PrefetchEntry>) {
        let allocation = entry.content.capacity();
        if allocation > self.byte_limit {
            return;
        }
        if let Some(old) = self.map.remove(&key) {
            self.bytes -= old.content.capacity();
        }
        self.order.retain(|k| k != &key);
        while self.map.len() >= MAX_CACHE_SIZE || self.bytes + allocation > self.byte_limit {
            if let Some(old_key) = self.order.pop_front() {
                if let Some(old) = self.map.remove(&old_key) {
                    self.bytes -= old.content.capacity();
                    eprintln!(
                        "[viewer prefetch] evict key={old_key:?} bytes={}",
                        old.content.capacity()
                    );
                }
            } else {
                break;
            }
        }
        self.bytes += allocation;
        self.order.push_back(key.clone());
        self.map.insert(key, entry);
    }

    fn get(&mut self, key: &str) -> Option<Arc<PrefetchEntry>> {
        let entry = self.map.get(key)?.clone();
        self.order.retain(|k| k != key);
        self.order.push_back(key.to_string());
        Some(entry)
    }
}

#[derive(Clone)]
struct Request {
    key: String,
    path: String,
    cap: usize,
    driver: Arc<dyn VfsDriver>,
    queued_at: Instant,
}

struct QueueState {
    cache: BoundedCache,
    wanted: HashMap<String, HashSet<String>>,
    queue: VecDeque<Request>,
    in_flight: HashSet<(String, u64)>,
    workers: usize,
    epoch: u64,
    policy_generation: u64,
}

impl QueueState {
    fn wants(&self, key: &str) -> bool {
        self.wanted.values().any(|keys| keys.contains(key))
    }

    fn replace(&mut self, panel: String, requests: Vec<Request>) {
        self.wanted.insert(
            panel.clone(),
            requests.iter().map(|r| r.key.clone()).collect(),
        );
        let mut next = VecDeque::new();
        let mut seen = HashSet::new();
        for request in requests.into_iter().chain(std::mem::take(&mut self.queue)) {
            if !self.wants(&request.key) {
                eprintln!("[viewer prefetch] discard queued path={:?}", request.path);
            } else if seen.insert(request.key.clone()) {
                if self.cache.map.contains_key(&request.key)
                    || self.in_flight.contains(&(request.key.clone(), self.epoch))
                {
                    eprintln!("[viewer prefetch] deduplicate path={:?}", request.path);
                } else {
                    next.push_back(request);
                }
            }
        }
        self.queue = next;
        eprintln!(
            "[viewer prefetch] schedule panel={panel} queued={} active={} cache_bytes={}",
            self.queue.len(),
            self.in_flight.len(),
            self.cache.bytes
        );
    }

    fn finish(&mut self, request: &Request, epoch: u64, entry: Option<PrefetchEntry>) {
        self.in_flight.remove(&(request.key.clone(), epoch));
        if epoch == self.epoch && self.wants(&request.key) {
            if let Some(entry) = entry {
                self.cache.insert(request.key.clone(), Arc::new(entry));
            }
        } else {
            eprintln!(
                "[viewer prefetch] discard completed path={:?}",
                request.path
            );
        }
    }

    fn clear(&mut self) {
        self.epoch += 1;
        self.cache = BoundedCache::new();
        self.queue.clear();
        self.wanted.clear();
        // Running reads still own their slots and keys until they finish.
        eprintln!("[viewer prefetch] clear active={}", self.in_flight.len());
    }
}

pub struct ViewerPrefetchQueue {
    state: Arc<Mutex<QueueState>>,
    policy: Arc<crate::storage::StorageRegistry>,
}

impl ViewerPrefetchQueue {
    pub fn new() -> Self {
        Self::with_policy(crate::storage::registry())
    }

    fn with_policy(policy: Arc<crate::storage::StorageRegistry>) -> Self {
        Self {
            state: Arc::new(Mutex::new(QueueState {
                cache: BoundedCache::new(),
                wanted: HashMap::new(),
                queue: VecDeque::new(),
                in_flight: HashSet::new(),
                workers: 0,
                epoch: 0,
                policy_generation: policy.generation(),
            })),
            policy,
        }
    }

    fn cache_key(source_id: &str, path: &str) -> String {
        format!("{source_id}\0{path}")
    }

    /// Replace this panel's desired files, ordered current-first, then nearest neighbors.
    /// Only local drivers are eligible. Foreground reads never wait on these workers.
    pub fn schedule_neighbors(
        &self,
        panel_id: String,
        neighbors: Vec<(String, Option<u64>)>,
        source_id: String,
        driver: Arc<dyn VfsDriver>,
        image_cap_bytes: usize,
    ) {
        self.invalidate_storage_policy();
        let requests = neighbors.into_iter().filter_map(|(path, size)| {
            let class = self.policy.class_for_path(std::path::Path::new(&path));
            if !driver.is_local() || !class.eligible() {
                eprintln!("[viewer prefetch] skip source={source_id:?} path={path:?} class={class:?} local={}", driver.is_local());
                return None;
            }
            let cap = if is_image_ext(&path) {
                let cap = image_cap_bytes.min(MAX_CACHE_BYTES);
                if size.is_some_and(|size| size > cap as u64) {
                    eprintln!("[viewer prefetch] skip oversized path={path:?} size={size:?} cap={cap}");
                    return None;
                }
                cap
            } else if size.is_some_and(|size| size > image_cap_bytes as u64) {
                PREFETCH_HEX_CAP
            } else { PREFETCH_OTHER_CAP };
            Some(Request { key: Self::cache_key(&source_id, &path), path, cap,
                driver: driver.clone(), queued_at: Instant::now() })
        }).collect();
        let mut state = self.state.lock().unwrap();
        state.replace(panel_id, requests);
        while state.workers < WORKERS && state.workers < state.queue.len() + state.in_flight.len() {
            state.workers += 1;
            let shared = self.state.clone();
            let policy = self.policy.clone();
            tokio::task::spawn_blocking(move || Self::worker(shared, policy));
        }
    }

    fn worker(shared: Arc<Mutex<QueueState>>, policy: Arc<crate::storage::StorageRegistry>) {
        loop {
            let (request, epoch) = {
                let mut state = shared.lock().unwrap();
                let Some(request) = state.queue.pop_front() else {
                    state.workers -= 1;
                    return;
                };
                let epoch = state.epoch;
                state.in_flight.insert((request.key.clone(), epoch));
                eprintln!(
                    "[viewer prefetch] start path={:?} wait_ms={} queued={} active={}",
                    request.path,
                    request.queued_at.elapsed().as_millis(),
                    state.queue.len(),
                    state.in_flight.len()
                );
                (request, state.epoch)
            };
            let started = Instant::now();
            let permit = policy.authorize(&request.path);
            let entry = permit.as_ref().and_then(|permit| {
                // Read the resolved target checked by the registry, not the original symlink.
                let mut resolved = request.clone();
                resolved.path = permit.path.to_string_lossy().into_owned();
                if policy.valid(permit) {
                    Self::read(&resolved)
                } else {
                    None
                }
            });
            let mut state = shared.lock().unwrap();
            if state.policy_generation != policy.generation() {
                state.clear();
                state.policy_generation = policy.generation();
            }
            let entry = if permit.as_ref().is_some_and(|p| policy.valid(p)) {
                entry
            } else {
                None
            };
            let bytes = entry.as_ref().map_or(0, |e| e.content.len());
            state.finish(&request, epoch, entry);
            eprintln!("[viewer prefetch] complete path={:?} elapsed_ms={} bytes={bytes} queued={} active={} cache_bytes={}",
                request.path, started.elapsed().as_millis(), state.queue.len(), state.in_flight.len(), state.cache.bytes);
        }
    }

    fn read(request: &Request) -> Option<PrefetchEntry> {
        let file_size = request.driver.file_size(&request.path);
        if is_image_ext(&request.path) && file_size.is_some_and(|n| n > request.cap as u64) {
            return None;
        }
        let cap = file_size.map_or(request.cap, |size| size.min(request.cap as u64) as usize);
        match request.driver.read_chunk(&request.path, 0, cap) {
            Ok(mut content) => {
                content.shrink_to_fit();
                Some(PrefetchEntry { content, file_size })
            }
            Err(error) => {
                eprintln!(
                    "[viewer prefetch] failed path={:?} error={error}",
                    request.path
                );
                None
            }
        }
    }

    pub fn get_cached(&self, source_id: &str, path: &str) -> Option<Arc<PrefetchEntry>> {
        self.invalidate_storage_policy();
        self.state
            .lock()
            .unwrap()
            .cache
            .get(&Self::cache_key(source_id, path))
    }

    /// The HTTP server only handles host paths; source namespaces remain intact for WS reads.
    pub fn get_cached_by_path(&self, path: &str) -> Option<Arc<PrefetchEntry>> {
        self.invalidate_storage_policy();
        let mut state = self.state.lock().unwrap();
        let suffix = format!("\0{path}");
        let key = state
            .cache
            .map
            .keys()
            .find(|k| k.ends_with(&suffix))
            .cloned()?;
        state.cache.get(&key)
    }

    pub fn clear(&self) {
        self.state.lock().unwrap().clear();
    }

    /// A topology change invalidates even formerly fast mounts (the same path
    /// may now name a different device). Running readers retain their slots.
    pub fn invalidate_storage_policy(&self) {
        let mut state = self.state.lock().unwrap();
        let generation = self.policy.generation();
        if state.policy_generation != generation {
            state.clear();
            state.policy_generation = generation;
        }
    }

    pub fn cancel_panel(&self, panel_id: &str) {
        let mut state = self.state.lock().unwrap();
        state.replace(panel_id.to_string(), Vec::new());
        state.wanted.remove(panel_id);
    }

    pub fn stats(&self) -> PrefetchCacheStats {
        let state = self.state.lock().unwrap();
        PrefetchCacheStats {
            entry_count: state.cache.map.len(),
            total_bytes: state.cache.map.values().map(|e| e.content.len()).sum(),
            max_entries: MAX_CACHE_SIZE,
        }
    }
}

pub struct PrefetchCacheStats {
    pub entry_count: usize,
    pub total_bytes: usize,
    pub max_entries: usize,
}

pub fn is_image_ext(path: &str) -> bool {
    matches!(
        path.to_lowercase().rsplit('.').next().unwrap_or(""),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tiff" | "tif"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::local::LocalFsDriver;
    use crate::vfs::{DirChild, VfsEntry, VfsError};
    use crate::ws::protocol::{FilterSpec, SortSpec};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Barrier;

    struct CountingDriver {
        local: bool,
        calls: AtomicUsize,
    }
    impl VfsDriver for CountingDriver {
        fn is_local(&self) -> bool {
            self.local
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn list(
            &self,
            _: &str,
            _: &SortSpec,
            _: Option<&FilterSpec>,
            _: bool,
        ) -> Result<Vec<VfsEntry>, VfsError> {
            unreachable!()
        }
        fn resolve_child(&self, _: &str, _: &str) -> String {
            unreachable!()
        }
        fn parent_path(&self, _: &str) -> Option<String> {
            unreachable!()
        }
        fn open_read(&self, _: &str) -> Result<Box<dyn std::io::Read + Send>, VfsError> {
            unreachable!()
        }
        fn open_write(&self, _: &str) -> Result<Box<dyn std::io::Write + Send>, VfsError> {
            unreachable!()
        }
        fn create_dir_all(&self, _: &str) -> Result<(), VfsError> {
            unreachable!()
        }
        fn remove_file(&self, _: &str) -> Result<(), VfsError> {
            unreachable!()
        }
        fn remove_dir_all(&self, _: &str) -> Result<(), VfsError> {
            unreachable!()
        }
        fn exists(&self, _: &str) -> bool {
            unreachable!()
        }
        fn read_dir_entries(&self, _: &str) -> Result<Vec<DirChild>, VfsError> {
            unreachable!()
        }
        fn file_size(&self, _: &str) -> Option<u64> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Some(4)
        }
        fn read_chunk(&self, _: &str, _: u64, _: usize) -> Result<Vec<u8>, VfsError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![1; 4])
        }
    }

    #[test]
    fn rejected_sources_do_not_schedule_workers_or_call_driver() {
        use crate::storage::{StorageClass, StorageRegistry};
        let fixture = Fixture::new();
        let path = fixture.0.join("image.jpg").to_string_lossy().into_owned();
        for (local, class) in [
            (false, StorageClass::InternalSsd),
            (true, StorageClass::Remote),
            (true, StorageClass::RemovableExternal),
            (true, StorageClass::Rotational),
            (true, StorageClass::Virtual),
            (true, StorageClass::Unknown),
        ] {
            let policy = Arc::new(StorageRegistry::default());
            policy.set_test_root(&fixture.0, class);
            let queue = ViewerPrefetchQueue::with_policy(policy);
            let driver = Arc::new(CountingDriver {
                local,
                calls: AtomicUsize::new(0),
            });
            // No Tokio runtime: attempting to spawn any worker also fails this test.
            queue.schedule_neighbors(
                "left".into(),
                vec![(path.clone(), Some(4))],
                "source".into(),
                driver.clone(),
                PREFETCH_IMAGE_CAP,
            );
            assert_eq!(driver.calls.load(Ordering::SeqCst), 0);
            let state = queue.state.lock().unwrap();
            assert!(state.queue.is_empty());
            assert_eq!(state.workers, 0);
        }
    }

    #[test]
    fn worker_rechecks_policy_and_invalidation_removes_cached_bytes() {
        use crate::storage::{StorageClass, StorageRegistry};
        let fixture = Fixture::new();
        let path = fixture.0.join("image.jpg");
        std::fs::write(&path, b"test").unwrap();
        let policy = Arc::new(StorageRegistry::default());
        policy.allow_test_root(&fixture.0);
        let queue = ViewerPrefetchQueue::with_policy(policy.clone());
        queue.invalidate_storage_policy();
        let driver = Arc::new(CountingDriver {
            local: true,
            calls: AtomicUsize::new(0),
        });
        let mut req = request(path.to_str().unwrap());
        req.driver = driver.clone();
        {
            let mut state = queue.state.lock().unwrap();
            state.replace("left".into(), vec![req.clone()]);
            state.workers = 1;
        }
        ViewerPrefetchQueue::worker(queue.state.clone(), policy.clone());
        assert_eq!(driver.calls.load(Ordering::SeqCst), 2);
        assert!(queue.get_cached("local", path.to_str().unwrap()).is_some());
        {
            let mut state = queue.state.lock().unwrap();
            state.cache = BoundedCache::new();
            state.replace("left".into(), vec![req.clone()]);
            state.workers = 1;
        }
        policy.set_test_root(&fixture.0, StorageClass::RemovableExternal);
        ViewerPrefetchQueue::worker(queue.state.clone(), policy.clone());
        assert_eq!(
            driver.calls.load(Ordering::SeqCst),
            2,
            "revoked worker must not touch driver"
        );
        assert!(queue.get_cached("local", path.to_str().unwrap()).is_none());
        policy.allow_test_root(&fixture.0);
        queue
            .state
            .lock()
            .unwrap()
            .cache
            .insert(req.key, Arc::new(entry(4)));
        assert!(
            queue.get_cached("local", path.to_str().unwrap()).is_none(),
            "new generation clears cache"
        );
    }

    fn request(path: &str) -> Request {
        Request {
            key: ViewerPrefetchQueue::cache_key("local", path),
            path: path.into(),
            cap: PREFETCH_IMAGE_CAP,
            driver: Arc::new(LocalFsDriver),
            queued_at: Instant::now(),
        }
    }

    fn entry(size: usize) -> PrefetchEntry {
        PrefetchEntry {
            content: vec![1; size],
            file_size: Some(size as u64),
        }
    }

    #[test]
    fn cache_bounds_allocations_and_evicts_least_recently_used() {
        let mut cache = BoundedCache::new();
        cache.byte_limit = 12;
        cache.insert("a".into(), Arc::new(entry(4)));
        cache.insert("b".into(), Arc::new(entry(4)));
        cache.insert("c".into(), Arc::new(entry(4)));
        cache.get("a").unwrap();
        cache.insert("d".into(), Arc::new(entry(4)));
        assert!(!cache.map.contains_key("b"));
        assert_eq!(cache.bytes, 12);
        cache.insert("large".into(), Arc::new(entry(13)));
        assert!(!cache.map.contains_key("large"));
        cache.insert("a".into(), Arc::new(entry(2)));
        assert_eq!(cache.bytes, 10);
        cache.byte_limit = MAX_CACHE_BYTES;
        for n in 0..100 {
            cache.insert(n.to_string(), Arc::new(entry(1)));
        }
        assert_eq!(cache.map.len(), MAX_CACHE_SIZE);
        assert_eq!(cache.bytes, MAX_CACHE_SIZE);
    }

    #[test]
    fn replacement_keeps_other_panel_and_deduplicates_running_reads() {
        let queue = ViewerPrefetchQueue::new();
        let mut state = queue.state.lock().unwrap();
        state.replace("left".into(), vec![request("a"), request("b")]);
        state.replace("right".into(), vec![request("b"), request("c")]);
        state.in_flight.insert((request("b").key, 0));
        state.replace("left".into(), vec![request("d"), request("b")]);
        let paths: Vec<_> = state.queue.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(paths, ["d", "c"]);
    }

    #[test]
    fn obsolete_and_cleared_reads_cannot_repopulate_cache() {
        let queue = ViewerPrefetchQueue::new();
        let mut state = queue.state.lock().unwrap();
        let old = request("old.jpg");
        state.replace("left".into(), vec![old.clone()]);
        state.in_flight.insert((old.key.clone(), 0));
        state.replace("left".into(), vec![request("new.jpg")]);
        state.finish(&old, 0, Some(entry(4)));
        assert!(state.cache.map.is_empty());
        state.in_flight.insert((old.key.clone(), 0));
        state.clear();
        // Reopening the same file must schedule a fresh read, despite the old read.
        state.replace("left".into(), vec![old.clone()]);
        assert_eq!(state.queue.len(), 1);
        state.finish(&old, 0, Some(entry(4)));
        assert!(state.cache.map.is_empty());
        state.finish(&old, 1, Some(entry(4)));
        assert!(state.cache.map.contains_key(&old.key));
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("fbro-prefetch-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn small_images_do_not_allocate_the_configured_image_limit() {
        let fixture = Fixture::new();
        let path = fixture.0.join("small.jpg");
        std::fs::write(&path, vec![7; 123_456]).unwrap();
        let path = path.to_str().unwrap();
        for cap in [PREFETCH_IMAGE_CAP, 512 * 1024 * 1024] {
            let bytes = LocalFsDriver.read_chunk(path, 0, cap).unwrap();
            assert_eq!(bytes.len(), 123_456);
            assert_eq!(bytes.capacity(), 123_456);
            let mut req = request(path);
            req.cap = cap;
            let cached = ViewerPrefetchQueue::read(&req).unwrap();
            assert_eq!(cached.content.capacity(), 123_456);
            assert_eq!(cached.file_size, Some(123_456));
        }
        assert!(LocalFsDriver
            .read_chunk(path, 200_000, 1024)
            .unwrap()
            .is_empty());
        let mut req = request(path);
        req.cap = 100;
        assert!(ViewerPrefetchQueue::read(&req).is_none());
    }

    #[test]
    fn rapid_navigation_has_two_workers_and_only_the_latest_neighborhood() {
        let fixture = Fixture::new();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(WORKERS)
            .build()
            .unwrap();
        runtime.block_on(async {
            // Occupy the blocking pool so scheduling assertions are deterministic.
            let entered = Arc::new(Barrier::new(WORKERS + 1));
            let release = Arc::new(Barrier::new(WORKERS + 1));
            for _ in 0..WORKERS {
                let (entered, release) = (entered.clone(), release.clone());
                tokio::task::spawn_blocking(move || {
                    entered.wait();
                    release.wait();
                });
            }
            entered.wait();
            let policy = Arc::new(crate::storage::StorageRegistry::default());
            policy.allow_test_root(&fixture.0);
            let queue = ViewerPrefetchQueue::with_policy(policy);
            for cursor in 0..60 {
                let neighbors = (cursor..cursor + 11)
                    .map(|i| {
                        let path = fixture.0.join(format!("{i}.jpg"));
                        std::fs::write(&path, [1; 32]).unwrap();
                        (path.to_string_lossy().into_owned(), Some(32))
                    })
                    .collect();
                queue.schedule_neighbors(
                    "left".into(),
                    neighbors,
                    "local".into(),
                    Arc::new(LocalFsDriver),
                    PREFETCH_IMAGE_CAP,
                );
            }
            let (workers, queued) = {
                let state = queue.state.lock().unwrap();
                (state.workers, state.queue.len())
            };
            // Release before assertions so a failure cannot strand runtime shutdown.
            release.wait();
            assert_eq!(workers, WORKERS);
            assert_eq!(queued, 11);
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if queue.state.lock().unwrap().workers == 0 {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(queue.stats().entry_count, 11);
            assert_eq!(queue.stats().total_bytes, 11 * 32);
            let obsolete = fixture.0.join("0.jpg");
            assert!(queue
                .get_cached("local", obsolete.to_str().unwrap())
                .is_none());
            let current = fixture.0.join("59.jpg");
            let a = queue
                .get_cached("local", current.to_str().unwrap())
                .unwrap();
            let b = queue.get_cached_by_path(current.to_str().unwrap()).unwrap();
            assert!(Arc::ptr_eq(&a, &b));
        });
    }
}
