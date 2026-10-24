use crate::journal::{RemoteJournal, RemoteKind, RemoteOperation, RemotePhase};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

#[derive(Clone, Debug)]
pub struct PendingTransition {
    pub peer: String,
    pub direction: ConnectionDirection,
    generation: u64,
    pub connected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ConnectionDirection {
    Incoming,
    Outgoing,
}

#[derive(Default)]
pub struct PeerTracker {
    connection_operations: HashMap<(String, ConnectionDirection), RemoteOperation>,
    peers: HashMap<String, PeerState>,
    removed_sources: HashSet<String>,
    source_generations: HashMap<String, u64>,
    transfer_starts: HashMap<(String, String), Instant>,
    transfer_order: VecDeque<(String, String)>,
}

#[derive(Default)]
struct PeerState {
    incoming: usize,
    outgoing: HashSet<String>,
    incoming_edge: ConnectionEdge,
    outgoing_edge: ConnectionEdge,
}

#[derive(Default)]
struct ConnectionEdge {
    reported: bool,
    generation: u64,
}

impl PeerState {
    fn connected(&self, direction: ConnectionDirection) -> bool {
        match direction {
            ConnectionDirection::Incoming => self.incoming > 0,
            ConnectionDirection::Outgoing => !self.outgoing.is_empty(),
        }
    }

    fn edge(&mut self, direction: ConnectionDirection) -> &mut ConnectionEdge {
        match direction {
            ConnectionDirection::Incoming => &mut self.incoming_edge,
            ConnectionDirection::Outgoing => &mut self.outgoing_edge,
        }
    }
}

impl PeerTracker {
    /// Called only after coalescing confirms an actual per-direction edge.
    pub fn journal_transition(
        &mut self,
        journal: &RemoteJournal,
        pending: &PendingTransition,
        name: &str,
    ) {
        let key = (pending.peer.clone(), pending.direction);
        if pending.connected {
            let direction = match pending.direction {
                ConnectionDirection::Incoming => "incoming",
                ConnectionDirection::Outgoing => "outgoing",
            };
            let operation = journal.begin(
                RemoteKind::Connection,
                Some(&pending.peer),
                Some(name),
                direction,
            );
            operation.record(RemotePhase::Connected, serde_json::json!({}));
            self.connection_operations.insert(key, operation);
        } else if let Some(operation) = self.connection_operations.remove(&key) {
            operation.record(RemotePhase::Disconnected, serde_json::json!({}));
        }
    }

    /// The token survives SFTP transport replacement, so execution time must too.
    /// Bound remembered tokens to the same window as notification history.
    pub fn transfer_started_at(&mut self, peer: &str, token: &str) -> Instant {
        self.transfer_started_at_at(peer, token, Instant::now())
    }

    fn transfer_started_at_at(&mut self, peer: &str, token: &str, now: Instant) -> Instant {
        let key = (peer.to_string(), token.to_string());
        if let Some(start) = self.transfer_starts.get(&key) {
            return *start;
        }
        if self.transfer_starts.len() >= 1_000 {
            if let Some(oldest) = self.transfer_order.pop_front() {
                self.transfer_starts.remove(&oldest);
            }
        }
        self.transfer_order.push_back(key.clone());
        self.transfer_starts.insert(key, now);
        now
    }

    pub fn incoming(&mut self, peer: &str, started: bool) -> Option<PendingTransition> {
        self.update(peer, ConnectionDirection::Incoming, |state| {
            state.incoming = if started {
                state.incoming.saturating_add(1)
            } else {
                state.incoming.saturating_sub(1)
            };
        })
    }

    pub fn outgoing(
        &mut self,
        peer: &str,
        source: &str,
        connected: bool,
    ) -> Option<PendingTransition> {
        if self.removed_sources.contains(source) {
            return None;
        }
        self.update(peer, ConnectionDirection::Outgoing, |state| {
            if connected {
                state.outgoing.insert(source.to_string());
            } else {
                state.outgoing.remove(source);
            }
        })
    }

    pub fn register_source(&mut self, source: &str) -> u64 {
        self.removed_sources.remove(source);
        let generation = self
            .source_generations
            .entry(source.to_string())
            .or_default();
        *generation += 1;
        *generation
    }

    pub fn accepts_source_generation(&self, source: &str, generation: u64) -> bool {
        !self.removed_sources.contains(source)
            && self.source_generations.get(source) == Some(&generation)
    }

    pub fn remove_source(&mut self, peer: &str, source: &str) -> Option<PendingTransition> {
        let pending = self.outgoing(peer, source, false);
        self.removed_sources.insert(source.to_string());
        pending
    }

    fn update(
        &mut self,
        peer: &str,
        direction: ConnectionDirection,
        mutate: impl FnOnce(&mut PeerState),
    ) -> Option<PendingTransition> {
        let state = self.peers.entry(peer.to_string()).or_default();
        let before = state.connected(direction);
        mutate(state);
        let connected = state.connected(direction);
        if connected == before {
            return None;
        }
        let edge = state.edge(direction);
        edge.generation += 1;
        (connected != edge.reported).then(|| PendingTransition {
            peer: peer.to_string(),
            direction,
            generation: edge.generation,
            connected,
        })
    }

    pub fn confirm(&mut self, pending: &PendingTransition) -> bool {
        let Some(state) = self.peers.get_mut(&pending.peer) else {
            return false;
        };
        let edge = state.edge(pending.direction);
        if edge.generation != pending.generation || edge.reported == pending.connected {
            return false;
        }
        edge.reported = pending.connected;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct JournalFile(std::path::PathBuf);
    impl Drop for JournalFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[tokio::test]
    async fn journal_connection_reuses_interval_and_keeps_directions_separate() {
        let mut tracker = PeerTracker::default();
        let path = std::env::temp_dir().join(format!(
            "fbro-connection-journal-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let _file = JournalFile(path.clone());
        let journal = crate::journal::RemoteJournal::new(path.clone());
        let connected = tracker.outgoing("peer", "source", true).unwrap();
        assert!(tracker.confirm(&connected));
        tracker.journal_transition(&journal, &connected, "other");
        let outgoing = tracker
            .connection_operations
            .get(&("peer".into(), ConnectionDirection::Outgoing))
            .unwrap()
            .clone();
        let incoming = tracker.incoming("peer", true).unwrap();
        assert!(tracker.confirm(&incoming));
        tracker.journal_transition(&journal, &incoming, "other");
        assert_ne!(
            outgoing.operation_id(),
            tracker
                .connection_operations
                .get(&("peer".into(), ConnectionDirection::Incoming))
                .unwrap()
                .operation_id()
        );
        let disconnected = tracker.outgoing("peer", "source", false).unwrap();
        assert!(tracker.confirm(&disconnected));
        tracker.journal_transition(&journal, &disconnected, "other");
        assert!(!outgoing.record(
            crate::journal::RemotePhase::Disconnected,
            serde_json::json!({})
        ));
        assert_eq!(tracker.connection_operations.len(), 1);
        let reconnected = tracker.outgoing("peer", "source", true).unwrap();
        assert!(tracker.confirm(&reconnected));
        tracker.journal_transition(&journal, &reconnected, "other");
        assert_ne!(
            outgoing.operation_id(),
            tracker
                .connection_operations
                .get(&("peer".into(), ConnectionDirection::Outgoing))
                .unwrap()
                .operation_id()
        );
        journal.flush().await;
        let conn = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM journal_events", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            4
        );
        drop(conn);
    }

    #[test]
    fn incoming_connect_and_disconnect_are_visible_while_outgoing_stays_connected() {
        let mut tracker = PeerTracker::default();
        let outgoing = tracker.outgoing("peer", "source", true).unwrap();
        assert!(tracker.confirm(&outgoing));
        let incoming = tracker
            .incoming("peer", true)
            .expect("reverse connection must have its own notification");
        assert!(tracker.confirm(&incoming));
        let disconnected = tracker
            .incoming("peer", false)
            .expect("reverse disconnect must be visible while outgoing is open");
        assert!(!disconnected.connected);
        assert!(tracker.confirm(&disconnected));
        assert!(tracker.outgoing("peer", "source", true).is_none());
    }

    #[test]
    fn outgoing_connect_and_disconnect_are_visible_while_incoming_stays_connected() {
        let mut tracker = PeerTracker::default();
        let incoming = tracker.incoming("peer", true).unwrap();
        assert!(tracker.confirm(&incoming));
        let outgoing = tracker
            .outgoing("peer", "source", true)
            .expect("outgoing connection must have its own notification");
        assert!(tracker.confirm(&outgoing));
        let disconnected = tracker
            .outgoing("peer", "source", false)
            .expect("outgoing disconnect must be visible while incoming is open");
        assert!(!disconnected.connected);
        assert!(tracker.confirm(&disconnected));
        assert!(tracker.incoming("peer", true).is_none());
    }

    #[test]
    fn many_sources_and_incoming_sessions_form_one_connection_per_direction() {
        let mut tracker = PeerTracker::default();
        let connected = tracker
            .outgoing("peer", "a", true)
            .expect("first source connects");
        assert!(tracker.outgoing("peer", "a", true).is_none());
        assert!(tracker.outgoing("peer", "b", true).is_none());
        let incoming = tracker.incoming("peer", true).unwrap();
        assert!(tracker.incoming("peer", true).is_none());
        assert!(tracker.confirm(&incoming));
        assert!(tracker.confirm(&connected));
        assert!(
            !tracker.confirm(&connected),
            "a settled edge is claimed once"
        );
        assert!(tracker.outgoing("peer", "a", false).is_none());
        assert!(tracker.incoming("peer", false).is_none());
        let outgoing_disconnected = tracker.outgoing("peer", "b", false).unwrap();
        assert!(tracker.confirm(&outgoing_disconnected));
        let disconnected = tracker
            .incoming("peer", false)
            .expect("last session disconnects");
        assert!(!disconnected.connected);
        assert!(tracker.confirm(&disconnected));
        assert!(tracker.incoming("peer", false).is_none());
    }

    #[test]
    fn reconnect_and_initial_failure_do_not_publish_spurious_disconnects() {
        let mut tracker = PeerTracker::default();
        assert!(tracker.outgoing("peer", "a", false).is_none());
        let short = tracker.outgoing("peer", "a", true).unwrap();
        assert!(tracker.outgoing("peer", "a", false).is_none());
        assert!(
            !tracker.confirm(&short),
            "short probe never becomes a logical connection"
        );
        let connected = tracker.outgoing("peer", "a", true).unwrap();
        assert!(tracker.confirm(&connected));
        let lost = tracker.outgoing("peer", "a", false).unwrap();
        assert!(tracker.outgoing("peer", "a", true).is_none());
        assert!(
            !tracker.confirm(&lost),
            "recovery invalidates pending disconnect"
        );
    }

    #[test]
    fn explicit_disconnect_ignores_late_supervisor_status_until_registered_again() {
        let mut tracker = PeerTracker::default();
        let connected = tracker.outgoing("peer", "a", true).unwrap();
        assert!(tracker.confirm(&connected));
        let removed = tracker.remove_source("peer", "a").unwrap();
        assert!(tracker.outgoing("peer", "a", true).is_none());
        assert!(tracker.confirm(&removed));
        tracker.register_source("a");
        let reconnected = tracker.outgoing("peer", "a", true).unwrap();
        assert!(tracker.confirm(&reconnected));
    }

    #[test]
    fn replacing_a_driver_rejects_the_old_supervisors_queued_events() {
        let mut tracker = PeerTracker::default();
        let old = tracker.register_source("a");
        assert!(tracker.accepts_source_generation("a", old));
        tracker.remove_source("peer", "a");
        assert!(!tracker.accepts_source_generation("a", old));
        let current = tracker.register_source("a");
        assert!(!tracker.accepts_source_generation("a", old));
        assert!(tracker.accepts_source_generation("a", current));
    }

    #[test]
    fn transfer_clock_survives_replacement_sessions_and_bounds_completed_history() {
        use std::time::Duration;
        let mut tracker = PeerTracker::default();
        let start = Instant::now();
        assert_eq!(tracker.transfer_started_at_at("peer", "0", start), start);
        assert_eq!(
            tracker.transfer_started_at_at("peer", "0", start + Duration::from_secs(30)),
            start
        );
        for token in 1..=1_000 {
            tracker.transfer_started_at_at("peer", &token.to_string(), start);
        }
        let later = start + Duration::from_secs(60);
        assert_eq!(tracker.transfer_started_at_at("peer", "0", later), later);
        assert_eq!(tracker.transfer_starts.len(), 1_000);
    }
}
