//! Notification Center adapter. Shared policy stays in notifications::desktop.

use block2::RcBlock;
use objc2::{
    rc::{autoreleasepool, Retained},
    runtime::Bool,
    MainThreadMarker,
};
use objc2_foundation::{NSBundle, NSError, NSProcessInfo, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
    UNNotificationRequest, UNNotificationSettings, UNUserNotificationCenter,
};
use std::ptr::NonNull;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, OnceLock,
};
use std::time::Duration;

const SERVICE_TIMEOUT: Duration = Duration::from_secs(3);
static AUTHORIZATION_REQUESTED: AtomicBool = AtomicBool::new(false);

fn settings_url(major_version: isize) -> &'static str {
    // Match Chromium's notification-pane URL, including the app ID query.
    // Older macOS releases expose the pane through its legacy identifier.
    if major_version >= 13 {
        "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=dev.fbro.app"
    } else {
        "x-apple.systempreferences:com.apple.preference.notifications?id=dev.fbro.app"
    }
}

pub fn open_settings() -> Result<(), String> {
    let major = autoreleasepool(|_| {
        NSProcessInfo::processInfo()
            .operatingSystemVersion()
            .majorVersion
    });
    let output = std::process::Command::new("/usr/bin/open")
        .arg(settings_url(major))
        .output()
        .map_err(|error| format!("Cannot open macOS notification settings: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Cannot open macOS notification settings ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn valid_identity(identifier: Option<&str>, path: &str) -> bool {
    identifier == Some("dev.fbro.app")
        && std::path::Path::new(path)
            .extension()
            .is_some_and(|ext| ext == "app")
}

pub fn supported() -> bool {
    // UNUserNotificationCenter can raise an Objective-C exception for a bare
    // executable. Never call it for cargo tests or `tauri dev`, or borrow a
    // terminal/another application's identity to work around that restriction.
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        autoreleasepool(|_| {
            let bundle = NSBundle::mainBundle();
            let identifier = bundle.bundleIdentifier().map(|value| value.to_string());
            valid_identity(identifier.as_deref(), &bundle.bundlePath().to_string())
        })
    })
}

fn center() -> Result<Retained<UNUserNotificationCenter>, String> {
    if !supported() {
        return Err("OS notifications require the packaged fbro.app (dev.fbro.app)".into());
    }
    Ok(UNUserNotificationCenter::currentNotificationCenter())
}

/// Called only from enabled, foreground UI lifecycle hooks. Does not block the
/// event loop or wait for the user. macOS remembers denial; toggling fbro's
/// preference does not override it or repeatedly display a permission prompt.
pub fn prepare() {
    if MainThreadMarker::new().is_none()
        || !supported()
        || AUTHORIZATION_REQUESTED.swap(true, Ordering::AcqRel)
    {
        return;
    }
    autoreleasepool(|_| {
        eprintln!("[notifications] Requesting macOS alert authorization");
        let completion = RcBlock::new(|granted: Bool, error: *mut NSError| {
            if let Err(error) = native_result(error) {
                // macOS also returns NotificationsNotAllowed when permission
                // is denied. Read actual settings before treating it as a
                // retryable service error or blaming the application's identity.
                let settings_completion =
                    RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
                        let status = unsafe { settings.as_ref() }.authorizationStatus();
                        finish_authorization_failure(&error, status, &AUTHORIZATION_REQUESTED);
                    });
                UNUserNotificationCenter::currentNotificationCenter()
                    .getNotificationSettingsWithCompletionHandler(&settings_completion);
            } else if !granted.as_bool() {
                log_denied();
            } else {
                eprintln!("[notifications] OS notification authorization granted");
            }
        });
        // Only request the interaction we use: no sounds, badges, critical
        // alerts, provisional authorization, or foreground presentation override.
        UNUserNotificationCenter::currentNotificationCenter()
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert,
                &completion,
            );
    });
}

fn log_denied() {
    eprintln!("[notifications] OS notifications denied; permission is controlled by System Settings → Notifications → fbro");
}

fn finish_authorization_failure(
    error: &str,
    status: UNAuthorizationStatus,
    requested: &AtomicBool,
) {
    if status == UNAuthorizationStatus::Denied {
        log_denied();
    } else {
        eprintln!(
            "[notifications] OS authorization failed: {error}; authorizationStatus={}",
            status.0
        );
        requested.store(false, Ordering::Release);
    }
}

fn may_deliver(status: UNAuthorizationStatus) -> bool {
    status == UNAuthorizationStatus::Authorized || status == UNAuthorizationStatus::Provisional
}

pub fn authorized() -> Result<bool, String> {
    autoreleasepool(|_| {
        let center = center()?;
        let (tx, rx) = mpsc::sync_channel(1);
        let completion = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            // The system owns this non-null object for the callback's duration.
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let _ = tx.try_send(may_deliver(status));
        });
        center.getNotificationSettingsWithCompletionHandler(&completion);
        wait_for(rx, "authorization lookup")
    })
}

fn payload(message: &str) -> Retained<UNNotificationRequest> {
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str("fbro"));
    // Native bodies are plain text, unlike freedesktop's markup bodies.
    content.setBody(&NSString::from_str(message));
    UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(&uuid::Uuid::new_v4().to_string()),
        &content,
        None,
    )
}

pub fn send(message: &str) -> Result<(), String> {
    autoreleasepool(|_| {
        let center = center()?;
        let (tx, rx) = mpsc::sync_channel(1);
        let completion = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.try_send(native_result(error));
        });
        center.addNotificationRequest_withCompletionHandler(&payload(message), Some(&completion));
        wait_for(rx, "submission")?
    })
}

fn native_result(error: *mut NSError) -> Result<(), String> {
    // NSError is borrowed only during the system completion callback; convert
    // it to owned Rust text before crossing threads or returning from the block.
    match unsafe { error.as_ref() } {
        Some(error) => Err(format!(
            "{} ({}:{})",
            error.localizedDescription(),
            error.domain(),
            error.code()
        )),
        None => Ok(()),
    }
}

fn wait_for<T>(rx: mpsc::Receiver<T>, operation: &str) -> Result<T, String> {
    rx.recv_timeout(SERVICE_TIMEOUT)
        .map_err(|error| format!("Notification Center {operation} did not complete: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_notifications_development_keeps_settings_available_without_native_delivery() {
        use crate::platform::Platform;
        let platform = super::super::MacOSPlatform;
        assert!(platform.supports_os_notification_settings());
        assert!(!platform.supports_os_notifications(), "Cargo's bare test executable must not initialize Notification Center");
    }

    #[test]
    fn macos_notifications_settings_link_targets_fbro_on_supported_os_generations() {
        for version in [10, 11, 12] {
            assert_eq!(
                settings_url(version),
                "x-apple.systempreferences:com.apple.preference.notifications?id=dev.fbro.app"
            );
        }
        for version in [13, 14, 15, 26, 27] {
            assert_eq!(settings_url(version), "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=dev.fbro.app");
        }
    }

    #[test]
    fn macos_notifications_require_fbro_app_identity() {
        assert!(valid_identity(
            Some("dev.fbro.app"),
            "/Applications/fbro.app"
        ));
        assert!(!valid_identity(None, "/tmp/fbro"));
        assert!(!valid_identity(Some("dev.fbro.app"), "/tmp/fbro"));
        assert!(!valid_identity(
            Some("com.apple.Terminal"),
            "/Applications/Terminal.app"
        ));
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../../../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], "dev.fbro.app");
    }

    #[test]
    fn macos_notifications_authorization_fails_closed() {
        assert!(may_deliver(UNAuthorizationStatus::Authorized));
        assert!(may_deliver(UNAuthorizationStatus::Provisional));
        for status in [
            UNAuthorizationStatus::NotDetermined,
            UNAuthorizationStatus::Denied,
            UNAuthorizationStatus(999),
        ] {
            assert!(!may_deliver(status));
        }
    }

    #[test]
    fn macos_notifications_not_allowed_error_respects_actual_denial_without_retry() {
        autoreleasepool(|_| {
            let error = unsafe {
                NSError::errorWithDomain_code_userInfo(
                    &NSString::from_str("UNErrorDomain"),
                    1,
                    None,
                )
            };
            let message = native_result(Retained::as_ptr(&error).cast_mut()).unwrap_err();
            let requested = AtomicBool::new(true);
            finish_authorization_failure(&message, UNAuthorizationStatus::Denied, &requested);
            assert!(requested.load(Ordering::Acquire));
            // The same native error with no remembered decision needs identity
            // or service diagnostics and may be retried on a later focus gain.
            finish_authorization_failure(
                &message,
                UNAuthorizationStatus::NotDetermined,
                &requested,
            );
            assert!(!requested.load(Ordering::Acquire));
        });
    }

    #[test]
    fn macos_notifications_payload_is_plain_unique_immediate_and_informational() {
        autoreleasepool(|_| {
            let request = payload("peer <雪> & file\nsecond line");
            let content = request.content();
            assert_eq!(content.title().to_string(), "fbro");
            assert_eq!(content.body().to_string(), "peer <雪> & file\nsecond line");
            assert!(request.trigger().is_none());
            assert!(content.sound().is_none());
            assert!(content.badge().is_none());
            assert!(content.categoryIdentifier().is_empty());
            assert_ne!(request.identifier(), payload("same").identifier());
        });
    }

    #[test]
    fn macos_notifications_native_completion_errors_are_owned() {
        autoreleasepool(|_| {
            assert!(native_result(std::ptr::null_mut()).is_ok());
            // No userInfo dictionary: there are no untyped object requirements.
            let error = unsafe {
                NSError::errorWithDomain_code_userInfo(
                    &NSString::from_str("test.notifications"),
                    7,
                    None,
                )
            };
            let result = native_result(Retained::as_ptr(&error).cast_mut()).unwrap_err();
            assert!(result.contains("test.notifications:7"));
        });
        let (tx, rx) = mpsc::channel::<()>();
        drop(tx);
        assert!(wait_for(rx, "test")
            .unwrap_err()
            .contains("test did not complete"));
    }
}
