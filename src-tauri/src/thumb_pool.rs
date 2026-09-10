//! Thumbnail generation worker pool with visibility-driven priority.
//!
//! The fileserver could just spawn a thread per `/__thumb__/...` request and
//! call `Platform::get_thumbnail()` directly. That works, but it has no idea
//! which thumbnails the user is looking at *right now* versus which ones
//! belong to a slice that has scrolled past — so a fast scroll burst makes
//! visible cards wait behind already-stale work.
//!
//! This pool maintains two FIFO queues (high/low) and a small fixed worker
//! count. Each job is keyed by `(path, size, mtime)` so concurrent requests
//! for the same thumbnail dedupe onto a single generation. Visibility is a
//! `HashSet<String>` of paths currently rendered in any grid panel; on
//! dequeue, a job whose path is no longer visible is demoted from the high
//! queue to the low queue. Net effect: workers always prefer currently-
//! visible cards, fast-scroll churn doesn't starve them.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;

use crate::platform::Platform;

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct ThumbJobKey {
    pub path: String,
    pub size: u32,
    pub mtime: u64,
    /// Separate speculative jobs from requested previews so cancelling a warmup
    /// never cancels a foreground request for the same image.
    pub speculative_generation: Option<u64>,
}

/// `Some(bytes)` on success, `None` if the OS thumbnail provider returned nothing.
pub type ThumbResult = Option<Vec<u8>>;

fn generate_thumbnail(
    platform: &dyn Platform,
    policy: &crate::storage::StorageRegistry,
    key: &ThumbJobKey,
) -> ThumbResult {
    let Some(generation) = key.speculative_generation else {
        // Requested previews never depend on the speculative-read policy.
        return platform
            .get_thumbnail(&key.path, key.size)
            .map(|(bytes, _)| bytes);
    };
    if generation != policy.generation() {
        return None;
    }
    let permit = policy.authorize(&key.path)?;
    if permit.generation() != generation || !policy.valid(&permit) {
        return None;
    }
    let result = platform.get_thumbnail(&permit.path.to_string_lossy(), key.size);
    if !policy.valid(&permit) {
        return None;
    }
    result.map(|(bytes, _)| bytes)
}

struct PoolState {
    high: VecDeque<ThumbJobKey>,
    low: VecDeque<ThumbJobKey>,
    waiters: HashMap<ThumbJobKey, Vec<mpsc::Sender<ThumbResult>>>,
    visible: HashSet<String>,
    closed: bool,
    /// Maximum number of workers allowed to be processing a job at the
    /// same time. Workers above this count idle on the cvar. Tuned at
    /// runtime: dialed down when an embedded viewer is open so thumbnail
    /// decoding doesn't fight the viewer for CPU/disk.
    max_concurrent: usize,
    in_flight: usize,
    queued_at: HashMap<ThumbJobKey, Instant>,
}

pub struct ThumbPool {
    state: Mutex<PoolState>,
    cvar: Condvar,
    platform: Arc<dyn Platform>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingPlatform(AtomicUsize);
    impl Platform for CountingPlatform {
        fn home_dir(&self) -> String {
            String::new()
        }
        fn open_in_editor(&self, _: &str) -> Result<String, String> {
            unreachable!()
        }
        fn get_thumbnail(&self, _: &str, _: u32) -> Option<(Vec<u8>, &'static str)> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Some((vec![1], "image/png"))
        }
    }

    #[test]
    fn revoked_and_unknown_warmups_skip_provider_but_requested_previews_work() {
        let policy = crate::storage::StorageRegistry::default();
        let platform = CountingPlatform(AtomicUsize::new(0));
        let root = std::env::temp_dir().join(format!("fbro-thumb-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("image.jpg");
        std::fs::write(&path, b"test").unwrap();
        let mut key = ThumbJobKey {
            path: path.to_string_lossy().into_owned(),
            size: 128,
            mtime: 0,
            speculative_generation: Some(policy.generation()),
        };
        assert!(generate_thumbnail(&platform, &policy, &key).is_none());
        assert_eq!(platform.0.load(Ordering::SeqCst), 0);
        policy.allow_test_root(&root);
        key.speculative_generation = Some(policy.generation());
        assert!(generate_thumbnail(&platform, &policy, &key).is_some());
        assert_eq!(platform.0.load(Ordering::SeqCst), 1);
        policy.set_test_root(&root, crate::storage::StorageClass::RemovableExternal);
        assert!(generate_thumbnail(&platform, &policy, &key).is_none());
        key.speculative_generation = Some(policy.generation());
        assert!(generate_thumbnail(&platform, &policy, &key).is_none());
        assert_eq!(platform.0.load(Ordering::SeqCst), 1);
        key.speculative_generation = None;
        assert!(generate_thumbnail(&platform, &policy, &key).is_some());
        assert_eq!(platform.0.load(Ordering::SeqCst), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}

impl ThumbPool {
    pub fn start(platform: Arc<dyn Platform>, workers: usize) -> Arc<Self> {
        let workers = workers.max(1);
        let pool = Arc::new(Self {
            state: Mutex::new(PoolState {
                high: VecDeque::new(),
                low: VecDeque::new(),
                waiters: HashMap::new(),
                visible: HashSet::new(),
                closed: false,
                max_concurrent: workers,
                in_flight: 0,
                queued_at: HashMap::new(),
            }),
            cvar: Condvar::new(),
            platform,
        });
        for _ in 0..workers {
            let p = Arc::clone(&pool);
            thread::spawn(move || p.worker_loop());
        }
        pool
    }

    /// Cap how many workers may be processing a job concurrently. Used to
    /// yield CPU/disk to the embedded viewer when it's active.
    pub fn set_max_concurrent(&self, n: usize) {
        let n = n.max(1);
        let mut g = self.state.lock().unwrap();
        if g.max_concurrent == n {
            return;
        }
        g.max_concurrent = n;
        // Wake idle workers in case the cap was raised.
        self.cvar.notify_all();
    }

    /// Replace the visibility set. Wakes workers in case demoted high jobs
    /// are now visible again, or vice versa.
    pub fn set_visible(&self, paths: HashSet<String>) {
        let mut g = self.state.lock().unwrap();
        if g.visible == paths {
            return;
        }
        g.visible = paths;
        // Re-classify queued items: any low-queue item now visible should
        // jump to the high queue. Take a snapshot first so the closure
        // doesn't have to borrow `g` while `g.low` is being mutated.
        let visible_snapshot = g.visible.clone();
        let mut promoted: Vec<ThumbJobKey> = Vec::new();
        g.low.retain(|k| {
            if visible_snapshot.contains(&k.path) {
                promoted.push(k.clone());
                false
            } else {
                true
            }
        });
        for k in promoted {
            g.high.push_back(k);
        }
        self.cvar.notify_all();
    }

    /// Submit a thumbnail request and return a receiver that yields the result
    /// once a worker has produced it. Multiple concurrent calls for the same
    /// `(path, size, mtime)` dedupe onto a single generation.
    pub fn request(&self, key: ThumbJobKey) -> mpsc::Receiver<ThumbResult> {
        let (tx, rx) = mpsc::channel();
        let mut g = self.state.lock().unwrap();
        let entry = g.waiters.entry(key.clone()).or_default();
        let already_queued = !entry.is_empty();
        entry.push(tx);
        if !already_queued {
            g.queued_at.insert(key.clone(), Instant::now());
            if g.visible.contains(&key.path) {
                g.high.push_back(key.clone());
            } else {
                g.low.push_back(key.clone());
            }
            self.cvar.notify_one();
        }
        eprintln!("[thumbnail] request path={:?} size={} deduplicated={already_queued} queued={} active={}",
            key.path, key.size,
            g.high.len() + g.low.len(), g.in_flight);
        rx
    }

    /// Block until a job is ready AND a concurrency permit is available.
    /// Increments in_flight on success; the caller must call `release_permit`
    /// after the job is processed.
    fn dequeue(&self) -> Option<ThumbJobKey> {
        let mut g = self.state.lock().unwrap();
        loop {
            if g.closed {
                return None;
            }
            // Don't pop work we can't actually run right now — the cap might
            // be exceeded if it was just lowered.
            if g.in_flight >= g.max_concurrent {
                g = self.cvar.wait(g).unwrap();
                continue;
            }
            // Drain high queue first, demoting any job whose path is no
            // longer in the visible set.
            let mut picked: Option<ThumbJobKey> = None;
            while let Some(k) = g.high.pop_front() {
                if g.visible.contains(&k.path) {
                    picked = Some(k);
                    break;
                }
                g.low.push_back(k);
            }
            if picked.is_none() {
                picked = g.low.pop_front();
            }
            if let Some(k) = picked {
                g.in_flight += 1;
                let wait = g
                    .queued_at
                    .remove(&k)
                    .map_or(0, |t| t.elapsed().as_millis());
                eprintln!(
                    "[thumbnail] start path={:?} size={} wait_ms={wait} queued={} active={}",
                    k.path,
                    k.size,
                    g.high.len() + g.low.len(),
                    g.in_flight
                );
                return Some(k);
            }
            g = self.cvar.wait(g).unwrap();
        }
    }

    fn release_permit(&self) {
        let mut g = self.state.lock().unwrap();
        if g.in_flight > 0 {
            g.in_flight -= 1;
        }
        // Wake one waiter — there might be a queued job and now a free permit.
        self.cvar.notify_one();
    }

    fn worker_loop(self: Arc<Self>) {
        while let Some(key) = self.dequeue() {
            let started = Instant::now();
            let policy = crate::storage::registry();
            let result = generate_thumbnail(self.platform.as_ref(), &policy, &key);
            eprintln!(
                "[thumbnail] complete path={:?} size={} elapsed_ms={} bytes={} success={}",
                key.path,
                key.size,
                started.elapsed().as_millis(),
                result.as_ref().map_or(0, Vec::len),
                result.is_some()
            );
            let waiters = {
                let mut g = self.state.lock().unwrap();
                g.waiters.remove(&key).unwrap_or_default()
            };
            self.release_permit();
            for w in waiters {
                let _ = w.send(result.clone());
            }
        }
    }
}
