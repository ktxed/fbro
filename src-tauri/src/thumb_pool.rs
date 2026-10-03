//! Thumbnail generation worker pool with visibility-driven priority.
//!
//! The fileserver could just spawn a thread per `/__thumb__/...` request and
//! call `Platform::get_thumbnail()` directly. That works, but it has no idea
//! which thumbnails the user is looking at *right now* versus which ones
//! belong to a slice that has scrolled past — so a fast scroll burst makes
//! visible cards wait behind already-stale work.
//!
//! This pool maintains two FIFO queues (high/low) and a small fixed worker
//! count. Concurrent requests for the same thumbnail and panel location lifetime
//! dedupe onto a single generation. Visibility is a
//! `HashSet<String>` of paths currently rendered in any grid panel; on
//! dequeue, a job whose path is no longer visible is demoted from the high
//! queue to the low queue. Net effect: workers always prefer currently-
//! visible cards, fast-scroll churn doesn't starve them.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::platform::Platform;

const BACKGROUND_COOLDOWN: Duration = Duration::from_secs(12);

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct ThumbOwner {
    panel_id: String,
    generation: u64,
}

struct ThumbContext {
    source_id: String,
    path: String,
    epoch: u64,
    generation: u64,
}

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct ThumbJobKey {
    pub path: String,
    pub size: u32,
    pub mtime: u64,
    /// Separate speculative jobs from requested previews so cancelling a warmup
    /// never cancels a foreground request for the same image.
    pub speculative_generation: Option<u64>,
    /// Explicit inline preview, independent of the grid viewport.
    pub foreground: bool,
    pub owner: Option<ThumbOwner>,
}

/// `Some(bytes)` on success, `None` if the OS thumbnail provider returned nothing.
pub type ThumbResult = Option<Vec<u8>>;

/// Bounded, read-only projection of the worker pool for the Jobs modal.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbQueueSnapshot {
    pub running: usize,
    pub queued: usize,
    pub max_concurrent: usize,
    pub background_wait_ms: u64,
    pub tasks: Vec<ThumbTaskSnapshot>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThumbTaskSnapshot {
    pub path: String,
    pub size: u32,
    pub status: &'static str,
    pub priority: &'static str,
    pub speculative: bool,
}

fn generate_thumbnail(
    platform: &dyn Platform,
    policy: &crate::storage::StorageRegistry,
    key: &ThumbJobKey,
    cancelled: &AtomicBool,
) -> ThumbResult {
    if cancelled.load(Ordering::Relaxed) {
        return None;
    }
    let Some(generation) = key.speculative_generation else {
        // Requested previews never depend on the speculative-read policy.
        return platform
            .get_thumbnail_cancellable(&key.path, key.size, cancelled)
            .map(|(bytes, _)| bytes);
    };
    if generation != policy.generation() {
        return None;
    }
    let permit = policy.authorize(&key.path)?;
    if permit.generation() != generation || !policy.valid(&permit) {
        return None;
    }
    let result =
        platform.get_thumbnail_cancellable(&permit.path.to_string_lossy(), key.size, cancelled);
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
    active: Vec<ThumbJobKey>,
    queued_at: HashMap<ThumbJobKey, Instant>,
    background_ready_at: Instant,
    contexts: HashMap<String, ThumbContext>,
    next_generation: u64,
    cancellation: HashMap<ThumbJobKey, Arc<AtomicBool>>,
}

impl PoolState {
    fn is_foreground(&self, key: &ThumbJobKey) -> bool {
        key.foreground || self.visible.contains(&key.path)
    }

    fn pop_ready(&mut self, now: Instant) -> Option<ThumbJobKey> {
        while let Some(key) = self.high.pop_front() {
            if self.is_foreground(&key) {
                return Some(key);
            }
            self.low.push_back(key);
        }
        if now >= self.background_ready_at {
            self.low.pop_front()
        } else {
            None
        }
    }
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

    fn idle_pool() -> ThumbPool {
        ThumbPool {
            state: Mutex::new(PoolState {
                high: VecDeque::new(),
                low: VecDeque::new(),
                waiters: HashMap::new(),
                visible: HashSet::new(),
                closed: false,
                max_concurrent: 2,
                in_flight: 0,
                active: Vec::new(),
                queued_at: HashMap::new(),
                background_ready_at: Instant::now() + BACKGROUND_COOLDOWN,
                contexts: HashMap::new(),
                next_generation: 0,
                cancellation: HashMap::new(),
            }),
            cvar: Condvar::new(),
            platform: Arc::new(CountingPlatform(AtomicUsize::new(0))),
        }
    }

    fn key(path: &str) -> ThumbJobKey {
        ThumbJobKey {
            path: path.into(),
            size: 128,
            mtime: 0,
            speculative_generation: None,
            foreground: false,
            owner: None,
        }
    }

    fn context(panel: &str, path: &str, epoch: u64) -> (String, String, String, u64) {
        (panel.into(), "local".into(), path.into(), epoch)
    }

    #[test]
    fn navigation_clears_queue_and_rejects_delayed_requests_without_affecting_other_panel() {
        let pool = idle_pool();
        pool.sync_contexts(vec![context("left", "/a", 1), context("right", "/a", 2)]);
        let left = pool.request_scoped(key("/a/image"), "left", 1);
        let right = pool.request_scoped(key("/a/image"), "right", 2);
        pool.sync_contexts(vec![context("left", "/b", 3), context("right", "/a", 2)]);
        assert_eq!(left.try_recv().unwrap(), None);
        assert_eq!(pool.snapshot().queued, 1);
        assert!(matches!(right.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert_eq!(
            pool.request_scoped(key("/a/late"), "left", 1)
                .try_recv()
                .unwrap(),
            None
        );
        assert_eq!(pool.snapshot().queued, 1);
        pool.sync_contexts(vec![]);
        assert_eq!(right.try_recv().unwrap(), None);
        assert_eq!(pool.snapshot().queued, 0);
    }

    #[test]
    fn rescan_preserves_work_but_updates_request_epoch() {
        let pool = idle_pool();
        pool.sync_contexts(vec![context("left", "/a", 1)]);
        let first = pool.request_scoped(key("/a/image"), "left", 1);
        pool.sync_contexts(vec![context("left", "/a", 2)]);
        let second = pool.request_scoped(key("/a/image"), "left", 2);
        assert_eq!(pool.snapshot().queued, 1);
        assert!(matches!(first.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(matches!(second.try_recv(), Err(mpsc::TryRecvError::Empty)));
        assert!(!pool.accepts_request("left", 1));
        assert!(pool.accepts_request("left", 2));
    }

    #[test]
    fn active_cancellation_cannot_complete_new_work_after_returning_to_location() {
        let pool = idle_pool();
        pool.sync_contexts(vec![context("left", "/a", 1)]);
        pool.set_visible(HashSet::from(["/a/image".into()]));
        let old = pool.request_scoped(key("/a/image"), "left", 1);
        let active = pool.dequeue().unwrap();
        let cancelled = pool.state.lock().unwrap().cancellation[&active].clone();
        pool.sync_contexts(vec![context("left", "/b", 2)]);
        assert!(cancelled.load(Ordering::Relaxed));
        assert_eq!(old.try_recv().unwrap(), None);
        assert_eq!(pool.snapshot().tasks[0].status, "Cancelling");
        let platform = CountingPlatform(AtomicUsize::new(0));
        assert_eq!(
            generate_thumbnail(
                &platform,
                &crate::storage::StorageRegistry::default(),
                &active,
                &cancelled
            ),
            None
        );
        assert_eq!(platform.0.load(Ordering::SeqCst), 0);
        pool.sync_contexts(vec![context("left", "/a", 3)]);
        let new = pool.request_scoped(key("/a/image"), "left", 3);
        assert!(pool.complete(&active).is_empty());
        assert_eq!((pool.snapshot().running, pool.snapshot().queued), (0, 1));
        assert!(matches!(new.try_recv(), Err(mpsc::TryRecvError::Empty)));
        let next = pool.dequeue().unwrap();
        assert_ne!(active.owner, next.owner);
        assert_eq!(pool.complete(&next).len(), 1);
    }

    #[test]
    fn queue_snapshot_deduplicates_and_tracks_priority_and_completion() {
        let pool = idle_pool();
        pool.set_visible(HashSet::from(["old-visible".into()]));
        let _a = pool.request(key("old-visible"));
        let _b = pool.request(key("background"));
        let _c = pool.request(key("new-visible"));
        let _duplicate = pool.request(key("new-visible"));
        pool.set_visible(HashSet::from(["new-visible".into()]));
        let snapshot = pool.snapshot();
        assert_eq!(snapshot.queued, 3);
        assert_eq!(snapshot.running, 0);
        assert_eq!(
            snapshot
                .tasks
                .iter()
                .map(|task| task.path.as_str())
                .collect::<Vec<_>>(),
            ["new-visible", "background", "old-visible"]
        );
        assert_eq!(snapshot.tasks[0].priority, "visible");
        assert_eq!(snapshot.tasks[1].priority, "background");
        let active = pool.dequeue().unwrap();
        assert_eq!(active.path, "new-visible");
        pool.set_max_concurrent(1);
        let snapshot = pool.snapshot();
        assert_eq!(
            (snapshot.running, snapshot.queued, snapshot.max_concurrent),
            (1, 2, 1)
        );
        assert_eq!(snapshot.tasks[0].status, "Running");
        assert_eq!(pool.complete(&active).len(), 2);
        let snapshot = pool.snapshot();
        assert_eq!((snapshot.running, snapshot.queued), (0, 2));
        assert!(snapshot.tasks.iter().all(|task| task.status == "Queued"));
    }

    #[test]
    fn queue_snapshot_bounds_details_but_preserves_total_and_wire_names() {
        let pool = idle_pool();
        for index in 0..105 {
            let _receiver = pool.request(key(&format!("image-{index}")));
        }
        let mut warmup = key("warmup");
        warmup.speculative_generation = Some(1);
        let _receiver = pool.request(warmup);
        pool.set_visible(HashSet::from(["warmup".into()]));
        let snapshot = pool.snapshot();
        assert_eq!(snapshot.queued, 106);
        assert_eq!(snapshot.tasks.len(), 100);
        assert!(snapshot.tasks[0].speculative);
        let wire = serde_json::to_value(snapshot).unwrap();
        assert_eq!(wire["maxConcurrent"], 2);
        assert_eq!(wire["tasks"][0]["status"], "Queued");
        assert_eq!(wire["tasks"][0]["priority"], "visible");
    }

    #[test]
    fn offscreen_work_waits_for_cooldown_and_visible_work_bypasses_it() {
        let pool = idle_pool();
        pool.set_visible(HashSet::from(["old".into()]));
        let _old = pool.request(key("old"));
        let _background = pool.request(key("background"));
        pool.set_visible(HashSet::from(["new".into()]));
        let _new = pool.request(key("new"));
        let mut state = pool.state.lock().unwrap();
        let deadline = state.background_ready_at;
        assert_eq!(
            state
                .pop_ready(deadline - Duration::from_secs(12))
                .unwrap()
                .path,
            "new"
        );
        assert!(state
            .pop_ready(deadline - Duration::from_nanos(1))
            .is_none());
        assert_eq!(state.pop_ready(deadline).unwrap().path, "background");
        assert_eq!(state.pop_ready(deadline).unwrap().path, "old");
    }

    #[test]
    fn unchanged_viewport_does_not_postpone_background_but_completion_does() {
        let pool = idle_pool();
        let paths = HashSet::from(["visible".into()]);
        pool.set_visible(paths.clone());
        let deadline = pool.state.lock().unwrap().background_ready_at;
        pool.set_visible(paths);
        assert_eq!(pool.state.lock().unwrap().background_ready_at, deadline);
        let _visible = pool.request(key("visible"));
        let _background = pool.request(key("background"));
        let active = pool.dequeue().unwrap();
        pool.state.lock().unwrap().background_ready_at = Instant::now();
        pool.complete(&active);
        assert!(pool.snapshot().background_wait_ms > 11_000);
    }

    #[test]
    fn explicit_preview_is_immediate_without_a_grid_viewport() {
        let pool = idle_pool();
        let _background = pool.request(key("background"));
        let mut preview = key("preview");
        preview.foreground = true;
        let _preview = pool.request(preview);
        pool.set_visible(HashSet::from(["other".into()]));
        assert_eq!(pool.snapshot().tasks[0].priority, "foreground");
        assert_eq!(pool.dequeue().unwrap().path, "preview");
    }

    #[test]
    fn worker_wakes_at_deadline_and_can_be_interrupted_by_visible_work() {
        let pool = Arc::new(idle_pool());
        let _background = pool.request(key("background"));
        pool.state.lock().unwrap().background_ready_at = Instant::now() + Duration::from_millis(30);
        let (tx, rx) = mpsc::channel();
        let worker_pool = Arc::clone(&pool);
        let worker = thread::spawn(move || {
            let _ = tx.send(worker_pool.dequeue());
        });
        let result = rx.recv_timeout(Duration::from_secs(2));
        pool.state.lock().unwrap().closed = true;
        pool.cvar.notify_all();
        worker.join().unwrap();
        assert_eq!(result.unwrap().unwrap().path, "background");

        let pool = Arc::new(idle_pool());
        let _background = pool.request(key("background"));
        let (tx, rx) = mpsc::channel();
        let worker_pool = Arc::clone(&pool);
        let worker = thread::spawn(move || {
            let _ = tx.send(worker_pool.dequeue());
        });
        pool.set_visible(HashSet::from(["visible".into()]));
        let _visible = pool.request(key("visible"));
        let result = rx.recv_timeout(Duration::from_secs(2));
        pool.state.lock().unwrap().closed = true;
        pool.cvar.notify_all();
        worker.join().unwrap();
        assert_eq!(result.unwrap().unwrap().path, "visible");
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
            foreground: false,
            owner: None,
        };
        assert!(generate_thumbnail(&platform, &policy, &key, &AtomicBool::new(false)).is_none());
        assert_eq!(platform.0.load(Ordering::SeqCst), 0);
        policy.allow_test_root(&root);
        key.speculative_generation = Some(policy.generation());
        assert!(generate_thumbnail(&platform, &policy, &key, &AtomicBool::new(false)).is_some());
        assert_eq!(platform.0.load(Ordering::SeqCst), 1);
        policy.set_test_root(&root, crate::storage::StorageClass::RemovableExternal);
        assert!(generate_thumbnail(&platform, &policy, &key, &AtomicBool::new(false)).is_none());
        key.speculative_generation = Some(policy.generation());
        assert!(generate_thumbnail(&platform, &policy, &key, &AtomicBool::new(false)).is_none());
        assert_eq!(platform.0.load(Ordering::SeqCst), 1);
        key.speculative_generation = None;
        assert!(generate_thumbnail(&platform, &policy, &key, &AtomicBool::new(false)).is_some());
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
                active: Vec::new(),
                queued_at: HashMap::new(),
                background_ready_at: Instant::now() + BACKGROUND_COOLDOWN,
                contexts: HashMap::new(),
                next_generation: 0,
                cancellation: HashMap::new(),
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

    pub fn snapshot(&self) -> ThumbQueueSnapshot {
        let g = self.state.lock().unwrap();
        // Reflect dequeue order, including high-queue entries awaiting demotion.
        let visible = g.high.iter().filter(|k| g.is_foreground(k));
        let background = g
            .low
            .iter()
            .chain(g.high.iter().filter(|k| !g.is_foreground(k)));
        let tasks = g
            .active
            .iter()
            .map(|k| {
                (
                    k,
                    if g.cancellation
                        .get(k)
                        .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
                    {
                        "Cancelling"
                    } else {
                        "Running"
                    },
                )
            })
            .chain(visible.chain(background).map(|k| (k, "Queued")))
            .take(100)
            .map(|(k, status)| ThumbTaskSnapshot {
                path: k.path.clone(),
                size: k.size,
                status,
                priority: if k.foreground {
                    "foreground"
                } else if g.visible.contains(&k.path) {
                    "visible"
                } else {
                    "background"
                },
                speculative: k.speculative_generation.is_some(),
            })
            .collect();
        ThumbQueueSnapshot {
            running: g.in_flight,
            queued: g.high.len() + g.low.len(),
            max_concurrent: g.max_concurrent,
            background_wait_ms: if g.low.is_empty() && g.high.iter().all(|k| g.is_foreground(k)) {
                0
            } else {
                g.background_ready_at
                    .saturating_duration_since(Instant::now())
                    .as_millis() as u64
            },
            tasks,
        }
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

    /// Keep work only for locations still displayed in an active listing tab.
    /// Listing rescans update admission epochs without cancelling the same location.
    pub fn sync_contexts(&self, contexts: Vec<(String, String, String, u64)>) {
        let mut g = self.state.lock().unwrap();
        let mut current = HashMap::new();
        for (panel_id, source_id, path, epoch) in contexts {
            let generation = match g.contexts.get(&panel_id) {
                Some(old) if old.source_id == source_id && old.path == path => old.generation,
                _ => {
                    g.next_generation += 1;
                    g.next_generation
                }
            };
            current.insert(
                panel_id,
                ThumbContext {
                    source_id,
                    path,
                    epoch,
                    generation,
                },
            );
        }
        g.contexts = current;
        let abandoned: Vec<_> = g
            .cancellation
            .keys()
            .filter(|key| {
                key.owner.as_ref().is_some_and(|owner| {
                    !g.contexts
                        .get(&owner.panel_id)
                        .is_some_and(|context| context.generation == owner.generation)
                })
            })
            .cloned()
            .collect();
        for key in abandoned {
            if let Some(cancel) = g.cancellation.get(&key) {
                cancel.store(true, Ordering::Relaxed);
            }
            g.high.retain(|queued| queued != &key);
            g.low.retain(|queued| queued != &key);
            g.queued_at.remove(&key);
            for waiter in g.waiters.remove(&key).unwrap_or_default() {
                let _ = waiter.send(None);
            }
            if !g.active.contains(&key) {
                g.cancellation.remove(&key);
            }
        }
        self.cvar.notify_all();
    }

    /// Reject delayed HTTP requests from a location the panel has left.
    pub fn accepts_request(&self, panel_id: &str, epoch: u64) -> bool {
        self.state
            .lock()
            .unwrap()
            .contexts
            .get(panel_id)
            .is_some_and(|context| context.epoch == epoch)
    }

    pub fn request_scoped(
        &self,
        mut key: ThumbJobKey,
        panel_id: &str,
        epoch: u64,
    ) -> mpsc::Receiver<ThumbResult> {
        let owner = {
            let g = self.state.lock().unwrap();
            g.contexts
                .get(panel_id)
                .filter(|context| context.epoch == epoch)
                .map(|context| ThumbOwner {
                    panel_id: panel_id.into(),
                    generation: context.generation,
                })
        };
        let Some(owner) = owner else {
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(None);
            return rx;
        };
        key.owner = Some(owner);
        self.request(key)
    }

    /// Replace the visibility set. Wakes workers in case demoted high jobs
    /// are now visible again, or vice versa.
    pub fn set_visible(&self, paths: HashSet<String>) {
        let mut g = self.state.lock().unwrap();
        if g.visible == paths {
            return;
        }
        g.visible = paths;
        g.background_ready_at = Instant::now() + BACKGROUND_COOLDOWN;
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
    /// key, including its panel location lifetime, dedupe onto a single generation.
    pub fn request(&self, key: ThumbJobKey) -> mpsc::Receiver<ThumbResult> {
        let (tx, rx) = mpsc::channel();
        let mut g = self.state.lock().unwrap();
        if key.owner.as_ref().is_some_and(|owner| {
            !g.contexts
                .get(&owner.panel_id)
                .is_some_and(|context| context.generation == owner.generation)
        }) {
            let _ = tx.send(None);
            return rx;
        }
        let entry = g.waiters.entry(key.clone()).or_default();
        let already_queued = !entry.is_empty();
        entry.push(tx);
        if !already_queued {
            g.cancellation
                .insert(key.clone(), Arc::new(AtomicBool::new(false)));
            g.queued_at.insert(key.clone(), Instant::now());
            if g.is_foreground(&key) {
                g.background_ready_at = Instant::now() + BACKGROUND_COOLDOWN;
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
    /// Increments in_flight on success; the caller must call `complete`
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
            if let Some(k) = g.pop_ready(Instant::now()) {
                g.in_flight += 1;
                g.active.push(k.clone());
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
            if !g.low.is_empty() {
                let wait = g
                    .background_ready_at
                    .saturating_duration_since(Instant::now());
                g = self.cvar.wait_timeout(g, wait).unwrap().0;
            } else {
                g = self.cvar.wait(g).unwrap();
            }
        }
    }

    fn complete(&self, key: &ThumbJobKey) -> Vec<mpsc::Sender<ThumbResult>> {
        let mut g = self.state.lock().unwrap();
        if g.is_foreground(key) {
            g.background_ready_at = Instant::now() + BACKGROUND_COOLDOWN;
        }
        g.active.retain(|active| active != key);
        let waiters = g.waiters.remove(key).unwrap_or_default();
        g.cancellation.remove(key);
        if g.in_flight > 0 {
            g.in_flight -= 1;
        }
        // Wake one waiter — there might be a queued job and now a free permit.
        self.cvar.notify_one();
        waiters
    }

    fn worker_loop(self: Arc<Self>) {
        while let Some(key) = self.dequeue() {
            let started = Instant::now();
            let policy = crate::storage::registry();
            let cancelled = self
                .state
                .lock()
                .unwrap()
                .cancellation
                .get(&key)
                .cloned()
                .unwrap_or_else(|| Arc::new(AtomicBool::new(true)));
            let result = generate_thumbnail(self.platform.as_ref(), &policy, &key, &cancelled);
            eprintln!(
                "[thumbnail] complete path={:?} size={} elapsed_ms={} bytes={} success={}",
                key.path,
                key.size,
                started.elapsed().as_millis(),
                result.as_ref().map_or(0, Vec::len),
                result.is_some()
            );
            let waiters = self.complete(&key);
            for w in waiters {
                let _ = w.send(result.clone());
            }
        }
    }
}
