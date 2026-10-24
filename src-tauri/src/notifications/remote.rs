//! Remote publications belong to peer/handshake boundaries, before GUI fan-out.

#[path = "remote/peer_state.rs"]
mod peer_state;

pub use peer_state::PeerTracker as RemoteNotificationState;
use peer_state::{ConnectionDirection, PendingTransition};

use super::{NotificationInput, NotificationService};
use crate::state::SharedState;
use std::time::Duration;

const PEER_QUIET_PERIOD: Duration = Duration::from_secs(2);

fn source_peer(source_id: &str) -> Option<&str> {
    source_id
        .strip_prefix("remote:")?
        .split_once(':')
        .map(|(peer, _)| peer)
}

pub async fn incoming_session(state: &SharedState, fingerprint: &str, started: bool) {
    let pending = state
        .lock()
        .await
        .remote_notifications
        .incoming(fingerprint, started);
    schedule_transition(state, pending);
}

pub async fn source_registered(state: &SharedState, source_id: &str) -> u64 {
    state
        .lock()
        .await
        .remote_notifications
        .register_source(source_id)
}

pub async fn source_status(
    state: &SharedState,
    source_id: &str,
    generation: u64,
    status: &str,
) -> bool {
    let Some(peer) = source_peer(source_id) else {
        return false;
    };
    let pending = {
        let mut guard = state.lock().await;
        if !guard
            .remote_notifications
            .accepts_source_generation(source_id, generation)
        {
            return false;
        }
        // Unknown/transitional statuses may still be projected to the GUI, but
        // only ready/lost states change the logical connection.
        if matches!(
            status,
            "connected" | "reconnecting" | "disconnected" | "dead"
        ) {
            guard
                .remote_notifications
                .outgoing(peer, source_id, status == "connected")
        } else {
            None
        }
    };
    schedule_transition(state, pending);
    true
}

pub async fn source_removed(state: &SharedState, source_id: &str) {
    let Some(peer) = source_peer(source_id) else {
        return;
    };
    let pending = state
        .lock()
        .await
        .remote_notifications
        .remove_source(peer, source_id);
    schedule_transition(state, pending);
}

fn schedule_transition(state: &SharedState, pending: Option<PendingTransition>) {
    let Some(pending) = pending else { return };
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(PEER_QUIET_PERIOD).await;
        settle_transition(&state, pending).await;
    });
}

async fn settle_transition(state: &SharedState, pending: PendingTransition) {
    let (notifications, name) = {
        let mut guard = state.lock().await;
        if !guard.remote_notifications.confirm(&pending) {
            return;
        }
        let name = guard
            .share_config
            .paired_peers
            .get(&pending.peer)
            .map(|peer| peer.name.clone())
            .unwrap_or_else(|| "remote fbro".into());
        let journal = guard.remote_journal.clone();
        guard
            .remote_notifications
            .journal_transition(&journal, &pending, &name);
        (guard.notifications.clone(), name)
    };
    let status = if pending.connected {
        "connected"
    } else {
        "disconnected"
    };
    let direction = match pending.direction {
        ConnectionDirection::Incoming => "incoming",
        ConnectionDirection::Outgoing => "outgoing",
    };
    notifications.publish_os(
        NotificationInput::new(format!("Remote fbro {status}: {name} ({direction})")),
        None,
    );
}

pub fn pairing_requested(
    notifications: &NotificationService,
    initiator: bool,
    handshake_id: &str,
    peer_name: &str,
    expires_at: u64,
) {
    if !initiator {
        notifications.publish_os_once(
            &format!("pairing:{handshake_id}"),
            NotificationInput::new(format!(
                "Incoming remote fbro pairing request from {peer_name}"
            )),
            Some(expires_at),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_publishes_incoming_only_and_once_per_handshake() {
        let notifications = NotificationService::default();
        pairing_requested(&notifications, true, "outgoing", "other", u64::MAX);
        assert!(notifications.snapshot().entries.is_empty());
        pairing_requested(
            &notifications,
            false,
            "private-handshake",
            "other",
            u64::MAX,
        );
        pairing_requested(
            &notifications,
            false,
            "private-handshake",
            "other",
            u64::MAX,
        );
        let entries = notifications.snapshot().entries;
        assert_eq!(entries.len(), 1);
        assert!(entries[0]
            .channels
            .contains(&super::super::NotificationChannel::Os));
        assert!(entries[0].message.contains("other"));
        assert!(!entries[0].message.contains("private-handshake"));
    }
}
