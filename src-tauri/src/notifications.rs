//! Application-lifetime notification history and delivery.

pub mod desktop;
pub mod jobs;
pub mod remote;

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationLevel {
    #[default]
    Info,
    Warning,
    Error,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationChannel {
    Toast,
    Log,
    Os,
}
#[derive(Clone, Debug, Serialize)]
pub struct Notification {
    pub id: String,
    pub timestamp: u64,
    pub message: String,
    pub level: NotificationLevel,
    pub channels: Vec<NotificationChannel>,
}
#[derive(Clone, Debug, Default)]
pub struct NotificationInput {
    pub message: String,
    pub level: Option<NotificationLevel>,
    pub channels: Option<Vec<NotificationChannel>>,
}
impl NotificationInput {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Self::default()
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct NotificationSnapshot {
    pub version: u64,
    pub entries: Vec<Notification>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum NotificationPatch {
    Added {
        added: Notification,
        #[serde(rename = "removedIds")]
        removed_ids: Vec<String>,
    },
    Clear {
        clear: bool,
    },
}
#[derive(Clone, Debug)]
pub struct NotificationUpdate {
    pub version: u64,
    pub patch: NotificationPatch,
}

/// The routing boundary can gain OS sinks without changing history ownership.
pub trait NotificationSink: Send + Sync {
    fn channel(&self) -> NotificationChannel;
    fn deliver(&self, notification: &Notification) -> Result<(), String>;
    fn deliver_expiring(
        &self,
        notification: &Notification,
        _expires_at: Option<u64>,
    ) -> Result<(), String> {
        self.deliver(notification)
    }
}
struct StderrLogSink;
impl NotificationSink for StderrLogSink {
    fn channel(&self) -> NotificationChannel {
        NotificationChannel::Log
    }
    fn deliver(&self, notification: &Notification) -> Result<(), String> {
        writeln!(
            std::io::stderr().lock(),
            "[notifications] id={} timestamp={} level={:?}: {}",
            notification.id,
            notification.timestamp,
            notification.level,
            notification.message
        )
        .map_err(|error| format!("Cannot write notification log: {error}"))
    }
}

#[derive(Default)]
struct NotificationHistory {
    version: u64,
    entries: VecDeque<Notification>,
    request_ids: HashMap<String, String>,
    subscribers: Vec<mpsc::UnboundedSender<NotificationUpdate>>,
}
impl NotificationHistory {
    fn snapshot(&self) -> NotificationSnapshot {
        NotificationSnapshot {
            version: self.version,
            entries: self.entries.iter().cloned().collect(),
        }
    }
    fn emit(&mut self, patch: NotificationPatch) {
        self.version += 1;
        let update = NotificationUpdate {
            version: self.version,
            patch,
        };
        self.subscribers
            .retain(|sender| sender.send(update.clone()).is_ok());
    }
}

pub struct NotificationService {
    capacity: usize,
    history: Mutex<NotificationHistory>,
    sinks: Vec<Arc<dyn NotificationSink>>,
    activity: Arc<desktop::DesktopActivity>,
    desktop_platform: Option<Arc<dyn crate::platform::Platform>>,
}
impl Default for NotificationService {
    fn default() -> Self {
        Self::new(1_000, vec![Arc::new(StderrLogSink)])
    }
}
impl NotificationService {
    pub fn new(capacity: usize, sinks: Vec<Arc<dyn NotificationSink>>) -> Self {
        Self {
            capacity: capacity.max(1),
            history: Mutex::new(NotificationHistory::default()),
            sinks,
            activity: Arc::new(desktop::DesktopActivity::default()),
            desktop_platform: None,
        }
    }
    pub fn with_desktop(platform: Arc<dyn crate::platform::Platform>) -> Self {
        let activity = Arc::new(desktop::DesktopActivity::default());
        let mut service = Self::new(1_000, vec![Arc::new(StderrLogSink)]);
        if platform.supports_os_notifications() {
            service
                .sinks
                .push(Arc::new(desktop::OsNotificationSink::start(
                    platform.clone(),
                    activity.clone(),
                )));
            service.desktop_platform = Some(platform);
        }
        service.activity = activity;
        service
    }
    pub fn desktop_activity(&self) -> Arc<desktop::DesktopActivity> {
        self.activity.clone()
    }
    pub fn set_os_enabled(&self, enabled: bool) {
        self.activity.set_enabled(enabled);
    }
    /// UI-thread lifecycle hook; never request permission from a background job.
    pub fn prepare_os_notifications(&self) {
        if self.activity.authorization_eligible() {
            if let Some(platform) = &self.desktop_platform {
                platform.prepare_os_notifications();
            }
        }
    }
    pub fn publish(&self, input: NotificationInput) -> Notification {
        self.publish_inner(None, input, None)
    }
    pub fn publish_once(&self, event_key: &str, input: NotificationInput) -> Notification {
        self.publish_inner(Some(&format!("event:{event_key}")), input, None)
    }
    pub fn publish_os(
        &self,
        mut input: NotificationInput,
        expires_at: Option<u64>,
    ) -> Notification {
        input
            .channels
            .get_or_insert_with(|| vec![NotificationChannel::Toast, NotificationChannel::Log])
            .push(NotificationChannel::Os);
        self.publish_inner(None, input, expires_at)
    }
    pub fn publish_os_once(
        &self,
        event_key: &str,
        mut input: NotificationInput,
        expires_at: Option<u64>,
    ) -> Notification {
        input
            .channels
            .get_or_insert_with(|| vec![NotificationChannel::Toast, NotificationChannel::Log])
            .push(NotificationChannel::Os);
        self.publish_inner(Some(&format!("event:{event_key}")), input, expires_at)
    }
    /// Request identity prevents duplicate frontend submissions, independently
    /// of text equality. The identity expires with its retained history entry.
    pub fn publish_request(&self, request_id: &str, input: NotificationInput) -> Notification {
        self.publish_inner(Some(&format!("request:{request_id}")), input, None)
    }
    fn publish_inner(
        &self,
        request_id: Option<&str>,
        input: NotificationInput,
        expires_at: Option<u64>,
    ) -> Notification {
        let notification = {
            let mut history = self
                .history
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(id) = request_id.and_then(|request| history.request_ids.get(request)) {
                if let Some(entry) = history.entries.iter().find(|entry| &entry.id == id) {
                    return entry.clone();
                }
            }
            let mut channels = Vec::new();
            // A channel is a delivery choice, so repeated channel names must not
            // trigger repeated delivery of one canonical record.
            for channel in input
                .channels
                .unwrap_or_else(|| vec![NotificationChannel::Toast, NotificationChannel::Log])
            {
                if !channels.contains(&channel) {
                    channels.push(channel);
                }
            }
            let notification = Notification {
                id: uuid::Uuid::new_v4().to_string(),
                timestamp: crate::ws::protocol::now(),
                message: input.message,
                level: input.level.unwrap_or_default(),
                channels,
            };
            history.entries.push_front(notification.clone());
            let mut removed_ids = vec![];
            while history.entries.len() > self.capacity {
                if let Some(removed) = history.entries.pop_back() {
                    removed_ids.push(removed.id);
                }
            }
            history
                .request_ids
                .retain(|_, id| !removed_ids.contains(id));
            if let Some(request) = request_id {
                history
                    .request_ids
                    .insert(request.into(), notification.id.clone());
            }
            history.emit(NotificationPatch::Added {
                added: notification.clone(),
                removed_ids,
            });
            notification
        };
        // Sinks run after the brief history lock is released. Delivery failure
        // never changes the committed record or recursively publishes feedback.
        for sink in &self.sinks {
            if notification.channels.contains(&sink.channel()) {
                if let Err(error) = sink.deliver_expiring(&notification, expires_at) {
                    eprintln!(
                        "[notifications] {:?} delivery failed: {error}",
                        sink.channel()
                    );
                }
            }
        }
        notification
    }
    pub fn snapshot(&self) -> NotificationSnapshot {
        self.history
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .snapshot()
    }
    pub fn clear(&self) {
        let mut history = self
            .history
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        history.entries.clear();
        history.request_ids.clear();
        history.emit(NotificationPatch::Clear { clear: true });
    }
    /// Capturing history and attaching the subscriber use the same lock as
    /// publication: every entry belongs to the snapshot or its ordered queue.
    pub fn subscribe(
        &self,
    ) -> (
        NotificationSnapshot,
        mpsc::UnboundedReceiver<NotificationUpdate>,
    ) {
        let mut history = self
            .history
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (tx, rx) = mpsc::unbounded_channel();
        history.subscribers.retain(|sender| !sender.is_closed());
        history.subscribers.push(tx);
        (history.snapshot(), rx)
    }
}

impl NotificationUpdate {
    pub fn into_server_message(self) -> crate::ws::protocol::ServerMessage {
        crate::ws::protocol::ServerMessage::StatePatch {
            timestamp: crate::ws::protocol::now(),
            domain: "notifications".into(),
            entity_id: None,
            version: self.version,
            patch: serde_json::to_value(self.patch).expect("notification patch is serializable"),
        }
    }
}

/// Classify at the producer boundary, never once per receiving GUI client.
/// Context-dependent errors remain replies for frontend inline routing.
pub fn backend_feedback(message: &crate::ws::protocol::ServerMessage) -> Option<NotificationInput> {
    use crate::ws::protocol::ServerMessage;
    use NotificationLevel::{Error, Info, Warning};
    let (message, level) = match message {
        ServerMessage::Error { code, message, .. } => {
            let level = if matches!(
                code.as_str(),
                "TOUCH_NOT_SUPPORTED" | "NOT_A_FILE" | "PANEL_RECOVERED_HOME"
            ) {
                Warning
            } else if matches!(code.as_str(), "START_FILE_OP_FAILED" | "BRO_LAUNCH_FAILED")
                || code.starts_with("PASTE_EXTERNAL_")
                || code.starts_with("WRITE_EXTERNAL_")
                || code.starts_with("GIT_REVIEW_")
                || code.starts_with("remote.")
                || code.starts_with("sharing.")
            {
                Error
            } else {
                return None;
            };
            (message.clone(), level)
        }
        ServerMessage::JobEvent {
            event,
            error,
            progress,
            ..
        } if event == "FileSkipped" => {
            let file = error
                .as_deref()
                .or_else(|| {
                    progress
                        .as_ref()
                        .and_then(|progress| progress.current_file.as_deref())
                })
                .unwrap_or("file");
            (format!("Skipped copying file {file}"), Warning)
        }
        ServerMessage::HashCheckReportSaved { path, .. } => {
            (format!("Saved hash check report to {path}"), Info)
        }
        ServerMessage::HashCheckReportFailed { message, .. } => (message.clone(), Error),
        ServerMessage::FedupReportSaved { path, .. } => {
            (format!("Saved Fedup report to {path}"), Info)
        }
        ServerMessage::SyncReportSaved { path, .. } => {
            (format!("Saved sync report to {path}"), Info)
        }
        ServerMessage::AddSharedFolderOk { .. } => ("Shared folder".into(), Info),
        ServerMessage::InitGitRepositoryOk { .. } => ("Initialized Git repository".into(), Info),
        ServerMessage::EjectSourceOk { action, .. } => (
            if action == "eject-drive" {
                "Drive ready to remove"
            } else {
                "Volume unmounted"
            }
            .into(),
            Info,
        ),
        ServerMessage::ExternalClipboardWritten {
            op_type,
            item_count,
            ..
        } => {
            let verb = if op_type == "move" { "Cut" } else { "Copied" };
            let noun = if *item_count == 1 { "item" } else { "items" };
            (
                format!("{verb} {item_count} {noun} to the system clipboard."),
                Info,
            )
        }
        ServerMessage::FileClipboardWritten { name, label, .. } => {
            let context = label
                .as_ref()
                .filter(|label| !label.is_empty())
                .map(|label| format!(" @ {label}"))
                .unwrap_or_default();
            (format!("Copied raw file “{name}”{context}"), Info)
        }
        ServerMessage::TextClipboardWritten { what, .. } => (format!("Copied {what}"), Info),
        ServerMessage::DryRunReady { script_path, .. } => {
            (format!("Saved dry-run script to {script_path}"), Info)
        }
        ServerMessage::FedupDeleteDryRunReady { script_path, .. } => (
            format!("Saved Fedup delete dry-run script to {script_path}"),
            Info,
        ),
        _ => return None,
    };
    Some(NotificationInput {
        message,
        level: Some(level),
        channels: None,
    })
}

/// The job metadata owns the claim so duplicate delivery, cancellation state,
/// and multiple GUI clients cannot produce repeated terminal summaries.
pub fn hash_generation_feedback(
    meta: &mut crate::jobs::HashGenerateBroJobMeta,
    event: &str,
) -> Option<NotificationInput> {
    use crate::ws::protocol::HashGenerateStatus;
    let (prefix, level) = match event {
        "Completed" => ("Saved", NotificationLevel::Info),
        "Cancelled" => ("Cancelled — saved", NotificationLevel::Warning),
        "Failed" => ("Failed — saved", NotificationLevel::Error),
        _ => return None,
    };
    if meta.terminal_feedback_published {
        return None;
    }
    meta.terminal_feedback_published = true;
    let saved = meta
        .result
        .as_ref()
        .map(|result| {
            result
                .items
                .iter()
                .filter(|item| {
                    matches!(
                        item.status,
                        HashGenerateStatus::Created | HashGenerateStatus::Replaced
                    )
                })
                .count()
        })
        .unwrap_or(0);
    if saved == 0 {
        return None;
    }
    let errors = meta
        .result
        .as_ref()
        .map(|result| {
            result
                .items
                .iter()
                .filter(|item| item.status == HashGenerateStatus::Error)
                .count()
        })
        .unwrap_or(0);
    let noun = if saved == 1 { "file" } else { "files" };
    let (suffix, level) = if errors > 0 {
        let noun = if errors == 1 { "error" } else { "errors" };
        (format!(" ({errors} {noun})"), NotificationLevel::Error)
    } else {
        (String::new(), level)
    };
    Some(NotificationInput {
        message: format!("{prefix} {saved} hash {noun}{suffix}"),
        level: Some(level),
        channels: None,
    })
}

pub async fn send_feedback(
    state: &crate::state::SharedState,
    sender: &mpsc::Sender<crate::ws::protocol::ServerMessage>,
    message: crate::ws::protocol::ServerMessage,
) -> Result<(), mpsc::error::SendError<crate::ws::protocol::ServerMessage>> {
    if let Some(input) = backend_feedback(&message) {
        let service = state.lock().await.notifications.clone();
        if jobs::is_operation_failure(&message) {
            service.publish_os(input, None);
        } else {
            service.publish(input);
        }
    }
    sender.send(message).await
}

/// One adapter per originating worker, before any GUI delivery or multicast.
/// It commits job state and notification history even if its GUI disconnects.
/// Existing worker APIs retain their bounded channel/backpressure contract.
pub fn worker_feedback_sender(
    state: crate::state::SharedState,
    sender: mpsc::Sender<crate::ws::protocol::ServerMessage>,
) -> mpsc::Sender<crate::ws::protocol::ServerMessage> {
    let (worker_tx, mut worker_rx) = mpsc::channel(512);
    tokio::spawn(async move {
        let notifications = state.lock().await.notifications.clone();
        while let Some(mut message) = worker_rx.recv().await {
            if crate::ws::server::apply_job_event_to_state(&state, &mut message).await {
                if let Some(input) = backend_feedback(&message) {
                    if jobs::is_operation_failure(&message) {
                        notifications.publish_os(input, None);
                    } else {
                        notifications.publish(input);
                    }
                }
                let _ = sender
                    .send(crate::ws::protocol::ServerMessage::AppliedWorkerMessage {
                        message: Box::new(message),
                    })
                    .await;
            }
        }
    });
    worker_tx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct CapturingSink(Mutex<Vec<String>>);
    impl NotificationSink for CapturingSink {
        fn channel(&self) -> NotificationChannel {
            NotificationChannel::Log
        }
        fn deliver(&self, notification: &Notification) -> Result<(), String> {
            self.0.lock().unwrap().push(notification.message.clone());
            Ok(())
        }
    }
    struct FailingSink;
    impl NotificationSink for FailingSink {
        fn channel(&self) -> NotificationChannel {
            NotificationChannel::Log
        }
        fn deliver(&self, _: &Notification) -> Result<(), String> {
            Err("unavailable".into())
        }
    }
    fn service() -> NotificationService {
        NotificationService::new(1_000, vec![Arc::new(CapturingSink::default())])
    }
    #[test]
    fn notifications_os_channel_round_trips_without_changing_defaults() {
        let channel = serde_json::from_str::<NotificationChannel>("\"os\"");
        assert!(
            channel.is_ok(),
            "backend records must support native delivery"
        );
        assert_eq!(serde_json::to_string(&channel.unwrap()).unwrap(), "\"os\"");
        assert_eq!(
            service()
                .publish(NotificationInput::new("ordinary"))
                .channels
                .len(),
            2
        );
    }
    #[test]
    fn notifications_os_event_identity_is_separate_from_frontend_requests() {
        let service = service();
        let frontend = service.publish_request("event:pair", NotificationInput::new("frontend"));
        let first = service.publish_os_once("pair", NotificationInput::new("pairing"), Some(42));
        let repeated = service.publish_os_once("pair", NotificationInput::new("pairing"), Some(42));
        assert_ne!(frontend.id, first.id);
        assert_eq!(first.id, repeated.id);
        assert!(first.channels.contains(&NotificationChannel::Os));
        assert_eq!(service.snapshot().entries.len(), 2);
    }

    #[test]
    fn notifications_native_expiry_is_forwarded_without_entering_history_wire_format() {
        struct ExpiringSink(Mutex<Vec<Option<u64>>>);
        impl NotificationSink for ExpiringSink {
            fn channel(&self) -> NotificationChannel {
                NotificationChannel::Os
            }
            fn deliver(&self, _: &Notification) -> Result<(), String> {
                panic!("expiry path required")
            }
            fn deliver_expiring(
                &self,
                _: &Notification,
                expiry: Option<u64>,
            ) -> Result<(), String> {
                self.0.lock().unwrap().push(expiry);
                Err("no native service".into())
            }
        }
        let sink = Arc::new(ExpiringSink(Mutex::new(vec![])));
        let service = NotificationService::new(1_000, vec![sink.clone()]);
        let entry = service.publish_os(NotificationInput::new("pairing"), Some(123));
        assert_eq!(*sink.0.lock().unwrap(), vec![Some(123)]);
        let snapshot = service.snapshot();
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].id, entry.id);
        assert!(serde_json::to_value(entry)
            .unwrap()
            .get("expiresAt")
            .is_none());
    }
    fn added(update: NotificationUpdate) -> Notification {
        let NotificationPatch::Added { added, .. } = update.patch else {
            panic!("expected added record")
        };
        added
    }
    #[test]
    fn notifications_default_publication_is_retained_and_routed() {
        let service = service();
        let entry = service.publish(NotificationInput::new("Saved report"));
        let snapshot = service.snapshot();
        assert_eq!(
            snapshot.entries.len(),
            1,
            "published feedback must be retained"
        );
        assert_eq!(snapshot.entries[0].id, entry.id);
        assert_eq!(
            entry.channels,
            vec![NotificationChannel::Toast, NotificationChannel::Log]
        );
        assert_eq!(entry.level, NotificationLevel::Info);
        assert!(entry.timestamp > 0);
        assert_eq!(snapshot.version, 1);
    }
    #[test]
    fn notifications_explicit_channels_control_routing_without_discarding_history() {
        let sink = Arc::new(CapturingSink::default());
        let service = NotificationService::new(1_000, vec![sink.clone()]);
        let (_, mut subscriber) = service.subscribe();
        for (message, channels) in [
            ("history", vec![]),
            ("log", vec![NotificationChannel::Log]),
            ("toast", vec![NotificationChannel::Toast]),
        ] {
            service.publish(NotificationInput {
                message: message.into(),
                channels: Some(channels),
                ..Default::default()
            });
        }
        assert_eq!(service.snapshot().entries.len(), 3);
        assert_eq!(*sink.0.lock().unwrap(), vec!["log"]);
        assert!(added(subscriber.try_recv().unwrap()).channels.is_empty());
        assert_eq!(
            added(subscriber.try_recv().unwrap()).channels,
            vec![NotificationChannel::Log]
        );
        assert_eq!(
            added(subscriber.try_recv().unwrap()).channels,
            vec![NotificationChannel::Toast]
        );
    }
    #[test]
    fn notifications_identical_text_is_distinct_and_history_is_newest_first() {
        let service = service();
        let first = service.publish(NotificationInput::new("same"));
        let second = service.publish(NotificationInput::new("same"));
        assert_ne!(first.id, second.id);
        assert_eq!(
            service
                .snapshot()
                .entries
                .iter()
                .map(|x| x.id.as_str())
                .collect::<Vec<_>>(),
            vec![second.id, first.id]
        );
    }
    #[test]
    fn notifications_capacity_eviction_is_in_the_same_versioned_patch() {
        let service = service();
        let (_, mut rx) = service.subscribe();
        let first = service.publish(NotificationInput::new("first"));
        for i in 1..=1_000 {
            service.publish(NotificationInput::new(format!("entry {i}")));
        }
        let snapshot = service.snapshot();
        assert_eq!(snapshot.entries.len(), 1_000);
        assert_eq!(snapshot.entries[0].message, "entry 1000");
        assert_eq!(snapshot.entries.last().unwrap().message, "entry 1");
        let mut final_update = rx.try_recv().unwrap();
        while let Ok(update) = rx.try_recv() {
            final_update = update;
        }
        assert_eq!(final_update.version, 1_001);
        let NotificationPatch::Added { added, removed_ids } = final_update.patch else {
            panic!("expected addition")
        };
        assert_eq!(added.message, "entry 1000");
        assert_eq!(removed_ids, vec![first.id]);
    }
    #[test]
    fn notifications_clear_updates_every_subscriber_and_resets_deduplication() {
        let service = service();
        let first = service.publish_request("request", NotificationInput::new("before"));
        let (_, mut rx) = service.subscribe();
        service.clear();
        assert!(service.snapshot().entries.is_empty());
        let update = rx.try_recv().unwrap();
        assert_eq!(update.version, 2);
        assert!(matches!(
            update.patch,
            NotificationPatch::Clear { clear: true }
        ));
        assert_ne!(
            service
                .publish_request("request", NotificationInput::new("after"))
                .id,
            first.id
        );
    }
    #[test]
    fn notifications_subscription_snapshot_and_updates_share_an_ordered_boundary() {
        let service = service();
        service.publish(NotificationInput::new("before"));
        let (snapshot_a, mut a) = service.subscribe();
        let (snapshot_b, mut b) = service.subscribe();
        assert_eq!(snapshot_a.version, 1);
        assert_eq!(snapshot_b.entries[0].message, "before");
        assert!(
            a.try_recv().is_err(),
            "snapshot entries never replay delivery"
        );
        for i in 0..600 {
            service.publish(NotificationInput::new(i.to_string()));
        }
        for i in 0..600 {
            let a = a.try_recv().unwrap();
            let b = b.try_recv().unwrap();
            assert_eq!(a.version, i + 2);
            assert_eq!(b.version, i + 2);
            assert_eq!(added(a).id, added(b).id);
        }
        assert!(a.try_recv().is_err());
    }

    #[tokio::test]
    async fn notifications_resnapshot_delivers_pending_live_updates_before_covering_their_versions()
    {
        let service = service();
        let (baseline, mut receiver) = service.subscribe();
        let mut delivered_version = baseline.version;
        let first = service.publish(NotificationInput::new("first live toast"));
        let mut requested_snapshot = service.snapshot();
        let second = service.publish(NotificationInput::new("later live toast"));
        let updates = crate::ws::server::notification_updates_before_snapshot(
            &service,
            &mut requested_snapshot,
            &mut receiver,
            &mut delivered_version,
        )
        .await;
        assert_eq!(
            updates.len(),
            2,
            "live toast delivery must precede a snapshot that covers it"
        );
        assert_eq!(added(updates[0].clone()).id, first.id);
        assert_eq!(added(updates[1].clone()).id, second.id);
        assert_eq!(requested_snapshot.version, 2);
        assert_eq!(requested_snapshot.entries.len(), 2);
        assert_eq!(delivered_version, 2);
        assert!(receiver.try_recv().is_err());
    }
    #[test]
    fn notifications_sink_failure_preserves_history_and_other_channels() {
        let service = NotificationService::new(1_000, vec![Arc::new(FailingSink)]);
        let (_, mut rx) = service.subscribe();
        let entry = service.publish(NotificationInput::new("saved despite sink"));
        assert_eq!(service.snapshot().entries.len(), 1);
        assert_eq!(added(rx.try_recv().unwrap()).id, entry.id);
    }
    #[test]
    fn notifications_frontend_request_deduplicates_only_while_record_is_retained() {
        let service = NotificationService::new(1, vec![Arc::new(CapturingSink::default())]);
        let (_, mut rx) = service.subscribe();
        let first = service.publish_request("frontend-1", NotificationInput::new("copied"));
        let duplicate = service.publish_request("frontend-1", NotificationInput::new("copied"));
        assert_eq!(duplicate.id, first.id);
        assert_eq!(service.snapshot().version, 1);
        rx.try_recv().unwrap();
        assert!(rx.try_recv().is_err());
        service.publish(NotificationInput::new("other"));
        assert_ne!(
            service
                .publish_request("frontend-1", NotificationInput::new("copied"))
                .id,
            first.id
        );
    }

    #[test]
    fn notifications_backend_errors_keep_inline_routing_and_classify_global_feedback() {
        use crate::ws::protocol::ServerMessage;
        for (code, expected) in [
            ("TOUCH_NOT_SUPPORTED", Some(NotificationLevel::Warning)),
            ("NOT_A_FILE", Some(NotificationLevel::Warning)),
            ("PANEL_RECOVERED_HOME", Some(NotificationLevel::Warning)),
            ("PASTE_EXTERNAL_FAILED", Some(NotificationLevel::Error)),
            ("WRITE_EXTERNAL_FAILED", Some(NotificationLevel::Error)),
            ("START_FILE_OP_FAILED", Some(NotificationLevel::Error)),
            ("BRO_LAUNCH_FAILED", Some(NotificationLevel::Error)),
            ("GIT_REVIEW_FAILED", Some(NotificationLevel::Error)),
            ("remote.connect", Some(NotificationLevel::Error)),
            ("sharing.pair", Some(NotificationLevel::Error)),
            ("GIT_UNAVAILABLE", None),
            ("GIT_LOG_FAILED", None),
            ("GIT_COMMIT_SHOW_FAILED", None),
            ("NAVIGATE_FAILED", None),
            ("RENAME_FAILED", None),
        ] {
            let input = backend_feedback(&ServerMessage::Error {
                timestamp: 1,
                code: code.into(),
                message: "problem".into(),
                recoverable: true,
                extra: None,
            });
            assert_eq!(
                input.as_ref().and_then(|input| input.level),
                expected,
                "{code}"
            );
            if let Some(input) = input {
                assert_eq!(input.message, "problem");
            }
        }
    }

    #[test]
    fn notifications_backend_clipboard_and_report_feedback_preserves_wording() {
        use crate::ws::protocol::ServerMessage;
        for (message, expected) in [
            (
                ServerMessage::ExternalClipboardWritten {
                    timestamp: 1,
                    op_type: "move".into(),
                    item_count: 2,
                },
                "Cut 2 items to the system clipboard.",
            ),
            (
                ServerMessage::ExternalClipboardWritten {
                    timestamp: 1,
                    op_type: "copy".into(),
                    item_count: 1,
                },
                "Copied 1 item to the system clipboard.",
            ),
            (
                ServerMessage::FileClipboardWritten {
                    timestamp: 1,
                    item_count: 1,
                    name: "file.txt".into(),
                    label: Some("abc123".into()),
                },
                "Copied raw file “file.txt” @ abc123",
            ),
            (
                ServerMessage::TextClipboardWritten {
                    timestamp: 1,
                    what: "full path".into(),
                },
                "Copied full path",
            ),
            (
                ServerMessage::HashCheckReportSaved {
                    timestamp: 1,
                    job_id: "job".into(),
                    path: "/tmp/report".into(),
                },
                "Saved hash check report to /tmp/report",
            ),
            (
                ServerMessage::FedupReportSaved {
                    timestamp: 1,
                    job_id: "job".into(),
                    path: "/tmp/report".into(),
                },
                "Saved Fedup report to /tmp/report",
            ),
            (
                ServerMessage::SyncReportSaved {
                    timestamp: 1,
                    job_id: "job".into(),
                    path: "/tmp/report".into(),
                },
                "Saved sync report to /tmp/report",
            ),
        ] {
            assert_eq!(
                backend_feedback(&message).map(|input| input.message),
                Some(expected.into())
            );
        }
    }

    #[test]
    fn notifications_hash_generation_terminal_summary_is_owned_once_and_counts_committed_rows() {
        use crate::jobs::HashGenerateBroJobMeta;
        use crate::ws::protocol::{
            HashGenerateBroFileSchema, HashGenerateBroResultSchema, HashGenerateStatus,
        };
        for (event, prefix, _level) in [
            ("Completed", "Saved", NotificationLevel::Info),
            ("Cancelled", "Cancelled — saved", NotificationLevel::Warning),
            ("Failed", "Failed — saved", NotificationLevel::Error),
        ] {
            let mut meta = HashGenerateBroJobMeta {
                result: Some(HashGenerateBroResultSchema {
                    ignored_count: 0,
                    items: [
                        HashGenerateStatus::Created,
                        HashGenerateStatus::Replaced,
                        HashGenerateStatus::Skipped,
                        HashGenerateStatus::Error,
                    ]
                    .into_iter()
                    .map(|status| HashGenerateBroFileSchema {
                        source_path: "file".into(),
                        hash_path: "file.b3".into(),
                        status,
                        hash: None,
                        error: None,
                    })
                    .collect(),
                }),
                terminal_feedback_published: false,
            };
            assert!(hash_generation_feedback(&mut meta, "Progress").is_none());
            let input = hash_generation_feedback(&mut meta, event).expect("terminal feedback");
            assert_eq!(input.message, format!("{prefix} 2 hash files (1 error)"));
            assert_eq!(input.level, Some(NotificationLevel::Error));
            assert!(
                hash_generation_feedback(&mut meta, event).is_none(),
                "broadcast recipients cannot publish twice"
            );
        }
        let mut empty = HashGenerateBroJobMeta {
            result: None,
            terminal_feedback_published: false,
        };
        assert!(hash_generation_feedback(&mut empty, "Completed").is_none());
    }
}
