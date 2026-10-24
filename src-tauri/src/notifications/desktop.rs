//! Native activity ownership and asynchronous delivery. No frontend focus reports.

use super::{Notification, NotificationChannel, NotificationSink};
use crate::platform::Platform;
use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const FOCUS_HANDOFF: Duration = Duration::from_millis(150);
const DELIVERY_QUEUE_CAPACITY: usize = 128;

struct ActivityState {
    enabled: bool,
    windows: HashMap<String, Option<bool>>,
    last_change: Instant,
    generation: u64,
}

pub struct DesktopActivity(Mutex<ActivityState>);

impl Default for DesktopActivity {
    fn default() -> Self {
        Self(Mutex::new(ActivityState {
            enabled: true,
            windows: HashMap::new(),
            last_change: Instant::now(),
            generation: 0,
        }))
    }
}

impl DesktopActivity {
    pub fn set_enabled(&self, enabled: bool) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.enabled != enabled {
            state.enabled = enabled;
            state.generation = state.generation.wrapping_add(1);
        }
    }

    pub fn observe_window(&self, label: &str, focused: Option<bool>) {
        self.observe_at(label, focused, Instant::now());
    }

    fn observe_at(&self, label: &str, focused: Option<bool>, now: Instant) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.windows.get(label) != Some(&focused) {
            state.windows.insert(label.to_string(), focused);
            state.last_change = now;
            state.generation = state.generation.wrapping_add(1);
        }
    }

    pub fn remove_window(&self, label: &str) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.windows.remove(label);
        state.last_change = Instant::now();
        state.generation = state.generation.wrapping_add(1);
    }

    /// None means suppressed, Some(delay) means eligible after the handoff grace.
    fn eligibility(&self, now: Instant) -> Option<(Duration, u64)> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !state.enabled
            || state.windows.is_empty()
            || state
                .windows
                .values()
                .any(|focused| *focused != Some(false))
        {
            return None;
        }
        Some((
            FOCUS_HANDOFF.saturating_sub(now.saturating_duration_since(state.last_change)),
            state.generation,
        ))
    }

    pub fn knows_window(&self, label: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .windows
            .contains_key(label)
    }
}

struct PendingNotification {
    notification: Notification,
    expires_at: Option<u64>,
    not_before: Instant,
    generation: u64,
}

pub struct OsNotificationSink {
    tx: mpsc::SyncSender<PendingNotification>,
    activity: Arc<DesktopActivity>,
}

impl OsNotificationSink {
    pub fn start(platform: Arc<dyn Platform>, activity: Arc<DesktopActivity>) -> Self {
        let (tx, rx) = mpsc::sync_channel::<PendingNotification>(DELIVERY_QUEUE_CAPACITY);
        let worker_activity = activity.clone();
        std::thread::Builder::new()
            .name("os-notifications".into())
            .spawn(move || {
                while let Ok(pending) = rx.recv() {
                    let delay = pending.not_before.saturating_duration_since(Instant::now());
                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                    if expired(pending.expires_at)
                        || worker_activity.eligibility(Instant::now())
                            != Some((Duration::ZERO, pending.generation))
                    {
                        continue;
                    }
                    // Native calls execute without either state mutex held.
                    if let Err(error) = platform.send_os_notification(&pending.notification.message)
                    {
                        eprintln!("[notifications] OS delivery failed: {error}");
                    }
                }
            })
            .expect("Cannot start OS notification worker");
        Self { tx, activity }
    }
}

fn expired(expires_at: Option<u64>) -> bool {
    expires_at.is_some_and(|expires| crate::ws::protocol::now() >= expires)
}

impl NotificationSink for OsNotificationSink {
    fn channel(&self) -> NotificationChannel {
        NotificationChannel::Os
    }

    fn deliver(&self, notification: &Notification) -> Result<(), String> {
        self.deliver_expiring(notification, None)
    }

    fn deliver_expiring(
        &self,
        notification: &Notification,
        expires_at: Option<u64>,
    ) -> Result<(), String> {
        let Some((delay, generation)) = self.activity.eligibility(Instant::now()) else {
            return Ok(());
        };
        if expired(expires_at) {
            return Ok(());
        }
        self.tx
            .try_send(PendingNotification {
                notification: notification.clone(),
                expires_at,
                not_before: Instant::now() + delay,
                generation,
            })
            .map_err(|error| format!("Cannot enqueue OS notification: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delay(activity: &DesktopActivity, now: Instant) -> Option<Duration> {
        activity.eligibility(now).map(|(delay, _)| delay)
    }

    #[test]
    fn desktop_notifications_unknown_focus_and_focused_windows_suppress_delivery() {
        let activity = DesktopActivity::default();
        let now = Instant::now();
        assert_eq!(delay(&activity, now), None);
        activity.observe_at("main", Some(false), now - FOCUS_HANDOFF);
        assert_eq!(delay(&activity, now), Some(Duration::ZERO));
        activity.observe_at("viewer", None, now);
        assert_eq!(delay(&activity, now), None);
        activity.observe_at("viewer", Some(true), now);
        assert_eq!(delay(&activity, now), None);
        activity.remove_window("viewer");
        assert!(delay(&activity, Instant::now()).is_some());
    }

    #[test]
    fn desktop_notifications_focus_handoff_never_counts_as_background() {
        let activity = DesktopActivity::default();
        let now = Instant::now();
        activity.observe_at("main", Some(false), now);
        assert_eq!(delay(&activity, now), Some(FOCUS_HANDOFF));
        activity.observe_at("diff", Some(true), now + Duration::from_millis(20));
        assert_eq!(delay(&activity, now + FOCUS_HANDOFF), None);
        activity.set_enabled(false);
        activity.observe_at("diff", Some(false), now);
        assert_eq!(delay(&activity, now + FOCUS_HANDOFF), None);
    }

    struct FakePlatform(mpsc::Sender<String>);
    impl Platform for FakePlatform {
        fn home_dir(&self) -> String {
            "/".into()
        }
        fn open_in_editor(&self, _: &str) -> Result<String, String> {
            Err("unused".into())
        }
        fn send_os_notification(&self, message: &str) -> Result<(), String> {
            self.0
                .send(message.to_string())
                .map_err(|error| error.to_string())
        }
    }

    fn notification(message: &str) -> Notification {
        Notification {
            id: message.into(),
            timestamp: crate::ws::protocol::now(),
            message: message.into(),
            level: super::super::NotificationLevel::Info,
            channels: vec![NotificationChannel::Os],
        }
    }

    #[test]
    fn desktop_notifications_worker_delivers_background_and_discards_expired_pairing() {
        let (tx, rx) = mpsc::channel();
        let activity = Arc::new(DesktopActivity::default());
        activity.observe_at("main", Some(false), Instant::now() - FOCUS_HANDOFF);
        let sink = OsNotificationSink::start(Arc::new(FakePlatform(tx)), activity);
        sink.deliver_expiring(&notification("expired"), Some(1))
            .unwrap();
        sink.deliver(&notification("background")).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            "background"
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn desktop_notifications_foreground_return_before_submission_discards_pending() {
        let (tx, rx) = mpsc::channel();
        let activity = Arc::new(DesktopActivity::default());
        activity.observe_window("main", Some(false));
        let sink = OsNotificationSink::start(Arc::new(FakePlatform(tx)), activity.clone());
        sink.deliver(&notification("pending")).unwrap();
        activity.observe_window("viewer", Some(true));
        assert!(rx.recv_timeout(Duration::from_millis(350)).is_err());
    }

    struct BlockingPlatform {
        sent: mpsc::Sender<String>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl Platform for BlockingPlatform {
        fn home_dir(&self) -> String {
            "/".into()
        }
        fn open_in_editor(&self, _: &str) -> Result<String, String> {
            Err("unused".into())
        }
        fn send_os_notification(&self, message: &str) -> Result<(), String> {
            self.sent.send(message.into()).unwrap();
            if message == "first" {
                self.release.lock().unwrap().recv().unwrap();
            }
            Ok(())
        }
    }

    #[test]
    fn desktop_notifications_pending_alerts_never_replay_after_focus_or_setting_roundtrip() {
        for change_setting in [false, true] {
            let (tx, rx) = mpsc::channel();
            let (release_tx, release_rx) = mpsc::channel();
            let activity = Arc::new(DesktopActivity::default());
            activity.observe_at("main", Some(false), Instant::now() - FOCUS_HANDOFF);
            let platform = Arc::new(BlockingPlatform {
                sent: tx,
                release: Mutex::new(release_rx),
            });
            let sink = OsNotificationSink::start(platform, activity.clone());
            sink.deliver(&notification("first")).unwrap();
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "first");
            sink.deliver(&notification("stale")).unwrap();
            if change_setting {
                activity.set_enabled(false);
                activity.set_enabled(true);
            } else {
                activity.observe_window("main", Some(true));
                activity.observe_at("main", Some(false), Instant::now() - FOCUS_HANDOFF);
            }
            release_tx.send(()).unwrap();
            sink.deliver(&notification("fresh")).unwrap();
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "fresh");
            assert!(rx.try_recv().is_err());
        }
    }
}
