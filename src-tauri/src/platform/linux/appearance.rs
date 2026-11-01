//! Linux system appearance via the XDG desktop portal
//! (`org.freedesktop.portal.Settings`, namespace `org.freedesktop.appearance`,
//! key `color-scheme`). GNOME, KDE, COSMIC and other portal backends expose
//! it; when the session bus or portal is missing, detection is unavailable.

use crate::platform::appearance::{portal_color_scheme, SystemColorScheme};
use futures_util::StreamExt;
use std::time::Duration;
use tokio::sync::watch;
use zbus::zvariant::{OwnedValue, Value};

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SETTINGS_IFACE: &str = "org.freedesktop.portal.Settings";
const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";
const READ_TIMEOUT: Duration = Duration::from_secs(1);

/// Spawn the portal watcher. The initial read completes (or times out)
/// before returning so the first state snapshot is already correct.
pub async fn start(tx: watch::Sender<Option<SystemColorScheme>>) {
    let connection = match tokio::time::timeout(READ_TIMEOUT, zbus::Connection::session()).await {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => {
            eprintln!("[appearance] session bus unavailable, not following system: {error}");
            return;
        }
        Err(_) => {
            eprintln!("[appearance] session bus connection timed out, not following system");
            return;
        }
    };
    let proxy = match zbus::Proxy::new(&connection, PORTAL_BUS, PORTAL_PATH, SETTINGS_IFACE).await {
        Ok(proxy) => proxy,
        Err(error) => {
            eprintln!("[appearance] portal proxy failed: {error}");
            return;
        }
    };
    // Subscribe before the initial read so a change in between is not lost.
    let signals = proxy.receive_signal("SettingChanged").await;

    match tokio::time::timeout(READ_TIMEOUT, read_color_scheme(&proxy)).await {
        Ok(Ok(value)) => {
            tx.send_replace(value);
        }
        Ok(Err(error)) => eprintln!("[appearance] portal color-scheme unavailable: {error}"),
        Err(_) => eprintln!("[appearance] portal color-scheme read timed out"),
    }

    let mut signals = match signals {
        Ok(signals) => signals,
        Err(error) => {
            eprintln!("[appearance] portal SettingChanged subscription failed: {error}");
            return;
        }
    };
    tokio::spawn(async move {
        // Keep the connection alive for as long as the stream is polled.
        let _connection = connection;
        while let Some(message) = signals.next().await {
            let Ok((namespace, key, value)) =
                message.body().deserialize::<(String, String, OwnedValue)>()
            else {
                continue;
            };
            if namespace != NAMESPACE || key != KEY {
                continue;
            }
            let next = portal_value(&value).and_then(portal_color_scheme);
            tx.send_if_modified(|current| {
                if *current == next {
                    return false;
                }
                *current = next;
                true
            });
        }
    });
}

async fn read_color_scheme(proxy: &zbus::Proxy<'_>) -> Result<Option<SystemColorScheme>, String> {
    // `ReadOne` is portal interface v2; older portals only offer the
    // deprecated `Read`, which wraps the value in an extra variant.
    let value: OwnedValue = match proxy.call("ReadOne", &(NAMESPACE, KEY)).await {
        Ok(value) => value,
        Err(read_one_error) => proxy
            .call("Read", &(NAMESPACE, KEY))
            .await
            .map_err(|error| format!("ReadOne: {read_one_error}; Read: {error}"))?,
    };
    Ok(portal_value(&value).and_then(portal_color_scheme))
}

fn portal_value(value: &Value<'_>) -> Option<u32> {
    match value {
        Value::U32(value) => Some(*value),
        Value::Value(inner) => portal_value(inner),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn portal_value_unwraps_nested_variants() {
        assert_eq!(portal_value(&Value::U32(1)), Some(1));
        assert_eq!(portal_value(&Value::Value(Box::new(Value::U32(2)))), Some(2));
        assert_eq!(portal_value(&Value::Str("dark".into())), None);
    }

    struct FakeSettings(Arc<Mutex<u32>>);

    #[zbus::interface(name = "org.freedesktop.portal.Settings")]
    impl FakeSettings {
        #[zbus(name = "ReadOne")]
        fn read_one(&self, namespace: &str, key: &str) -> zbus::fdo::Result<OwnedValue> {
            if namespace != NAMESPACE || key != KEY {
                return Err(zbus::fdo::Error::Failed("not found".into()));
            }
            Ok(OwnedValue::from(*self.0.lock().unwrap()))
        }

        #[zbus(signal, name = "SettingChanged")]
        async fn setting_changed(
            emitter: &zbus::object_server::SignalEmitter<'_>,
            namespace: &str,
            key: &str,
            value: Value<'_>,
        ) -> zbus::Result<()>;
    }

    /// Deliberately isolated from the user's real portal.
    #[tokio::test]
    #[ignore = "requires dbus-run-session and FBRO_APPEARANCE_TEST_BUS=1"]
    async fn linux_appearance_private_bus_reads_and_follows_setting_changed() {
        assert_eq!(std::env::var("FBRO_APPEARANCE_TEST_BUS").as_deref(), Ok("1"));
        let value = Arc::new(Mutex::new(1u32));
        let server = zbus::connection::Builder::session()
            .unwrap()
            .name(PORTAL_BUS)
            .unwrap()
            .serve_at(PORTAL_PATH, FakeSettings(value.clone()))
            .unwrap()
            .build()
            .await
            .unwrap();

        let (tx, mut rx) = watch::channel(None);
        start(tx).await;
        assert_eq!(*rx.borrow_and_update(), Some(SystemColorScheme::Dark));

        let iface = server
            .object_server()
            .interface::<_, FakeSettings>(PORTAL_PATH)
            .await
            .unwrap();
        let emitter = iface.signal_emitter();
        // Unrelated keys are ignored.
        FakeSettings::setting_changed(emitter, NAMESPACE, "accent-color", Value::U32(2))
            .await
            .unwrap();
        *value.lock().unwrap() = 2;
        FakeSettings::setting_changed(emitter, NAMESPACE, KEY, Value::U32(2))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), rx.changed())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(*rx.borrow(), Some(SystemColorScheme::Light));

        // Without the portal interface, detection is unavailable.
        server
            .object_server()
            .remove::<FakeSettings, _>(PORTAL_PATH)
            .await
            .unwrap();
        let (tx, rx) = watch::channel(None);
        start(tx).await;
        assert_eq!(*rx.borrow(), None);
    }
}
