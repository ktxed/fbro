//! macOS system appearance: the global `AppleInterfaceStyle` user default,
//! watched through the distributed `AppleInterfaceThemeChangedNotification`.
//! Reading the default (rather than `NSApp.effectiveAppearance`) reports the
//! system preference even if fbro ever forces its own window appearance.

use crate::platform::appearance::{macos_interface_style, SystemColorScheme};
use block2::RcBlock;
use objc2_foundation::{
    NSDistributedNotificationCenter, NSNotification, NSOperationQueue, NSString, NSUserDefaults,
};
use std::ptr::NonNull;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

const THEME_CHANGED: &str = "AppleInterfaceThemeChangedNotification";
/// The notification can arrive before cfprefsd publishes the new value;
/// a second read shortly after catches that case.
const SETTLE_DELAY: Duration = Duration::from_millis(300);

fn read() -> SystemColorScheme {
    let defaults = NSUserDefaults::standardUserDefaults();
    let style = defaults.stringForKey(&NSString::from_str("AppleInterfaceStyle"));
    macos_interface_style(style.map(|style| style.to_string()).as_deref())
}

/// Global `com.apple.keyboard.fnState` (System Settings → Keyboard → Keyboard
/// Shortcuts → Function Keys → "Use F1, F2, etc. keys as standard function
/// keys"). Absent means the factory default: media keys, F-keys need Fn.
pub fn function_keys_standard() -> bool {
    NSUserDefaults::standardUserDefaults()
        .boolForKey(&NSString::from_str("com.apple.keyboard.fnState"))
}

pub async fn start(tx: watch::Sender<Option<SystemColorScheme>>) {
    tx.send_replace(Some(read()));
    let (changed_tx, mut changed_rx) = mpsc::unbounded_channel::<()>();

    // Distributed notifications are delivered through the main run loop, so
    // register there; the block only forwards a wake-up to the tokio task.
    let register = RcBlock::new(move || {
        let changed_tx = changed_tx.clone();
        let observer = RcBlock::new(move |_notification: NonNull<NSNotification>| {
            let _ = changed_tx.send(());
        });
        let name = NSString::from_str(THEME_CHANGED);
        let token = unsafe {
            NSDistributedNotificationCenter::defaultCenter()
                .addObserverForName_object_queue_usingBlock(
                    Some(&name),
                    None,
                    Some(&NSOperationQueue::mainQueue()),
                    &observer,
                )
        };
        // The observer lives for the whole process; never removed.
        std::mem::forget(token);
    });
    unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&register) };

    tokio::spawn(async move {
        while changed_rx.recv().await.is_some() {
            publish(&tx, read());
            tokio::time::sleep(SETTLE_DELAY).await;
            publish(&tx, read());
        }
    });
}

fn publish(tx: &watch::Sender<Option<SystemColorScheme>>, value: SystemColorScheme) {
    tx.send_if_modified(|current| {
        if *current == Some(value) {
            return false;
        }
        *current = Some(value);
        true
    });
}
