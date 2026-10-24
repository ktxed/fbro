//! Freedesktop delivery through notify-rust, isolated from application policy.

use notify_rust::{Hint, Notification, Urgency};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const RETAINED_HANDLE_LIMIT: usize = 128;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("Cannot create notification runtime")
    })
}

fn retained_handles() -> &'static Mutex<VecDeque<notify_rust::NotificationHandle>> {
    static HANDLES: OnceLock<Mutex<VecDeque<notify_rust::NotificationHandle>>> = OnceLock::new();
    HANDLES.get_or_init(|| Mutex::new(VecDeque::new()))
}

fn escape_body(message: &str) -> String {
    message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn payload(message: &str) -> Notification {
    let mut notification = Notification::new();
    notification
        .appname("fbro")
        .summary("fbro")
        .icon("fbro")
        .body(&escape_body(message))
        .urgency(Urgency::Normal)
        .hint(Hint::DesktopEntry("fbro".into()));
    notification
}

pub fn send(message: &str) -> Result<(), String> {
    let notification = payload(message);
    let handle = runtime()
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(3), notification.show_async()).await
        })
        .map_err(|_| "Desktop notification service timed out".to_string())?
        .map_err(|error| format!("Cannot send desktop notification: {error}"))?;
    // Some desktops remove a bubble when the sending D-Bus connection closes.
    // Retain recent sending connections without relying on closure signals:
    // daemons can retain messages indefinitely, or close before a listener
    // subscribes. Evict the oldest at the bound so later alerts still get through.
    let retired = {
        let mut handles = retained_handles()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        handles.push_back(handle);
        if handles.len() > RETAINED_HANDLE_LIMIT {
            handles.pop_front()
        } else {
            None
        }
    };
    // Connection teardown also stays outside the brief retention lock.
    drop(retired);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::mpsc;

    #[derive(Debug)]
    struct ReceivedNotification {
        app: String,
        icon: String,
        title: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, zbus::zvariant::OwnedValue>,
        timeout: i32,
    }

    struct FakeNotifications(mpsc::Sender<ReceivedNotification>);

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl FakeNotifications {
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &self,
            app: String,
            _replaces_id: u32,
            icon: String,
            title: String,
            body: String,
            actions: Vec<String>,
            hints: HashMap<String, zbus::zvariant::OwnedValue>,
            timeout: i32,
        ) -> u32 {
            self.0
                .send(ReceivedNotification {
                    app,
                    icon,
                    title,
                    body,
                    actions,
                    hints,
                    timeout,
                })
                .unwrap();
            7
        }
    }

    /// Deliberately isolated from the user's real desktop notification service.
    #[test]
    #[ignore = "requires dbus-run-session and FBRO_NOTIFICATION_TEST_BUS=1"]
    fn linux_notifications_private_bus_roundtrip_and_missing_service() {
        assert_eq!(
            std::env::var("FBRO_NOTIFICATION_TEST_BUS").as_deref(),
            Ok("1")
        );
        let (tx, rx) = mpsc::channel();
        let server = runtime().block_on(async {
            zbus::connection::Builder::session()
                .unwrap()
                .name("org.freedesktop.Notifications")
                .unwrap()
                .serve_at("/org/freedesktop/Notifications", FakeNotifications(tx))
                .unwrap()
                .build()
                .await
                .unwrap()
        });
        send("remote <peer> & file").unwrap();
        let received = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(received.app, "fbro");
        assert_eq!(received.title, "fbro");
        assert_eq!(received.icon, "fbro");
        assert_eq!(received.body, "remote &lt;peer&gt; &amp; file");
        assert!(received.actions.is_empty());
        assert_eq!(received.timeout, -1);
        assert_eq!(
            <&str>::try_from(received.hints.get("desktop-entry").unwrap()).unwrap(),
            "fbro"
        );
        assert_eq!(
            u8::try_from(received.hints.get("urgency").unwrap()).unwrap(),
            1
        );
        // A daemon may retain notifications indefinitely without Closed signals.
        // More than the retention limit must still accept fresh publications.
        for _ in 0..RETAINED_HANDLE_LIMIT + 2 {
            send("no closure signal").unwrap();
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(2)).unwrap().body,
                "no closure signal"
            );
        }
        // Keep the well-known name while removing the interface: a default
        // session bus may otherwise auto-start a real notification daemon.
        runtime()
            .block_on(
                server
                    .object_server()
                    .remove::<FakeNotifications, _>("/org/freedesktop/Notifications"),
            )
            .unwrap();
        assert!(send("missing service").is_err());
    }

    #[test]
    fn linux_notifications_payload_identifies_app_and_escapes_peer_and_file_text() {
        let notification = payload("a<b>& c");
        assert_eq!(notification.appname, "fbro");
        assert_eq!(notification.summary, "fbro");
        assert_eq!(notification.icon, "fbro");
        assert_eq!(notification.body, "a&lt;b&gt;&amp; c");
        assert!(notification
            .hints
            .contains(&Hint::DesktopEntry("fbro".into())));
        assert!(notification.hints.contains(&Hint::Urgency(Urgency::Normal)));
        assert!(notification.actions.is_empty());
    }
}
