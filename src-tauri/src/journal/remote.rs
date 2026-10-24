//! Semantic remote lifecycle records. Calls made under SharedState only claim
//! lifecycle state and enqueue; the dedicated writer owns all SQLite work.

use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex, OnceLock};

use rusqlite::{params, Connection};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug)]
pub enum RemoteKind {
    Transfer,
    Hash,
    Sync,
    Connection,
    Pairing,
}

impl RemoteKind {
    fn name(self) -> &'static str {
        match self {
            Self::Transfer => "remote-transfer",
            Self::Hash => "remote-hash",
            Self::Sync => "remote-sync",
            Self::Connection => "remote-connection",
            Self::Pairing => "remote-pairing",
        }
    }

    fn event_type(self) -> &'static str {
        match self {
            Self::Transfer => "remote.transfer.v1",
            Self::Hash => "remote.hash.v1",
            Self::Sync => "remote.sync.v1",
            Self::Connection => "remote.connection.v1",
            Self::Pairing => "remote.pairing.v1",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum RemotePhase {
    Started,
    Resumed,
    Ended,
    Completed,
    Cancelled,
    Failed,
    Connected,
    Disconnected,
    Requested,
    Paired,
    Rejected,
    Timeout,
    Revoked,
}

impl RemotePhase {
    fn name(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Resumed => "resumed",
            Self::Ended => "ended",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Connected => "connected",
            Self::Disconnected => "disconnected",
            Self::Requested => "requested",
            Self::Paired => "paired",
            Self::Rejected => "rejected",
            Self::Timeout => "timeout",
            Self::Revoked => "revoked",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RemoteJournal {
    queue: Arc<Mutex<Queue>>,
}

#[derive(Debug)]
struct Queue {
    sender: mpsc::Sender<WriteRequest>,
    sequence: u64,
}

#[derive(Debug)]
enum WriteRequest {
    Event(Event),
    Flush(tokio::sync::oneshot::Sender<()>),
}

#[derive(Debug)]
struct Event {
    id: String,
    timestamp_ms: i64,
    operation_id: String,
    kind: RemoteKind,
    phase: RemotePhase,
    details: Value,
    payload: Value,
}

impl Default for RemoteJournal {
    fn default() -> Self {
        static JOURNAL: OnceLock<RemoteJournal> = OnceLock::new();
        JOURNAL
            .get_or_init(|| Self::new(super::database_path()))
            .clone()
    }
}

impl RemoteJournal {
    /// `path` is a SQLite database file, not a directory. No disk work occurs
    /// here; the writer lazily opens it when the first event arrives.
    pub fn new(path: PathBuf) -> Self {
        let (sender, receiver) = mpsc::channel();
        if let Err(error) = std::thread::Builder::new()
            .name("remote-journal".into())
            .spawn(move || write_events(path, receiver))
        {
            eprintln!("[journal] remote writer could not start: {error}");
        }
        Self {
            queue: Arc::new(Mutex::new(Queue {
                sender,
                sequence: 0,
            })),
        }
    }

    pub fn begin(
        &self,
        kind: RemoteKind,
        peer_fingerprint: Option<&str>,
        peer_name: Option<&str>,
        direction: &str,
    ) -> RemoteOperation {
        RemoteOperation {
            inner: Arc::new(Operation {
                journal: self.clone(),
                id: uuid::Uuid::new_v4().to_string(),
                kind,
                direction: direction.into(),
                lifecycle: Mutex::new(Lifecycle {
                    peer_fingerprint: peer_fingerprint.map(str::to_owned),
                    peer_name: peer_name.map(str::to_owned),
                    ..Lifecycle::default()
                }),
            }),
        }
    }

    /// Completes after all previously enqueued writes have been attempted.
    /// Journal errors are diagnostic only and do not affect operation state.
    pub async fn flush(&self) {
        let (done, completion) = tokio::sync::oneshot::channel();
        let sent = self
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sender
            .send(WriteRequest::Flush(done));
        if sent.is_err() || completion.await.is_err() {
            eprintln!("[journal] remote writer unavailable during flush");
        }
    }
}

#[derive(Clone, Debug)]
pub struct RemoteOperation {
    inner: Arc<Operation>,
}

#[derive(Debug)]
struct Operation {
    journal: RemoteJournal,
    id: String,
    kind: RemoteKind,
    direction: String,
    lifecycle: Mutex<Lifecycle>,
}

#[derive(Default, Debug)]
struct Lifecycle {
    started: bool,
    requested: bool,
    connected: bool,
    ended: bool,
    terminal: bool,
    peer_fingerprint: Option<String>,
    peer_name: Option<String>,
}

impl Lifecycle {
    fn claim(&mut self, phase: RemotePhase) -> bool {
        if self.terminal {
            return false;
        }
        match phase {
            RemotePhase::Started if !self.started => self.started = true,
            RemotePhase::Requested if !self.requested => self.requested = true,
            RemotePhase::Connected if !self.connected => self.connected = true,
            RemotePhase::Ended if self.started && !self.ended => self.ended = true,
            RemotePhase::Resumed if self.ended => self.ended = false,
            RemotePhase::Started
            | RemotePhase::Requested
            | RemotePhase::Connected
            | RemotePhase::Ended
            | RemotePhase::Resumed => return false,
            _ => self.terminal = true,
        }
        true
    }
}

impl RemoteOperation {
    pub fn operation_id(&self) -> &str {
        &self.inner.id
    }

    /// Returns true only when this lifecycle event was newly claimed and queued.
    /// Details are supplied by the domain caller; credentials are never inferred.
    pub fn record(&self, phase: RemotePhase, details: Value) -> bool {
        self.record_with(phase, || details)
    }

    /// Build expensive metadata only after claiming a semantic transition.
    pub fn record_with(&self, phase: RemotePhase, build_details: impl FnOnce() -> Value) -> bool {
        let mut lifecycle = self
            .inner
            .lifecycle
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !lifecycle.claim(phase) {
            return false;
        }
        let details = build_details();
        if lifecycle.peer_fingerprint.is_none() {
            lifecycle.peer_fingerprint = details
                .get("peerFingerprint")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        if lifecycle.peer_name.is_none() {
            lifecycle.peer_name = details
                .get("peerName")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        // Claim, sequence allocation, and send are serialized. Callers racing
        // through different operation handles cannot reorder SQLite insertion.
        let mut queue = self
            .inner
            .journal
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        queue.sequence += 1;
        let timestamp_ms = crate::ws::protocol::now() as i64;
        let payload = json!({
            "operationId": self.inner.id,
            "sequence": queue.sequence,
            "timestampMs": timestamp_ms,
            "kind": self.inner.kind.name(),
            "phase": phase.name(),
            "peerFingerprint": lifecycle.peer_fingerprint,
            "peerName": lifecycle.peer_name,
            "direction": self.inner.direction,
            "details": details,
        });
        let event = Event {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp_ms,
            operation_id: self.inner.id.clone(),
            kind: self.inner.kind,
            phase,
            details,
            payload,
        };
        if queue.sender.send(WriteRequest::Event(event)).is_err() {
            eprintln!(
                "[journal] remote writer unavailable while recording {}",
                phase.name()
            );
            return false;
        }
        true
    }
}

fn write_events(path: PathBuf, receiver: mpsc::Receiver<WriteRequest>) {
    let mut connection: Option<Connection> = None;
    // When all journal and operation handles drop, recv drains already queued
    // events before exiting. Explicit flush is the application shutdown barrier.
    while let Ok(request) = receiver.recv() {
        match request {
            WriteRequest::Flush(done) => {
                // Release the file handle before acknowledging shutdown or a
                // test's persistence barrier (Windows cannot unlink open DBs).
                connection = None;
                let _ = done.send(());
            }
            WriteRequest::Event(event) => {
                if connection.is_none() {
                    match super::open_path(&path) {
                        Ok(conn) => {
                            if let Err(error) = conn.busy_timeout(std::time::Duration::from_secs(5))
                            {
                                eprintln!("[journal] remote writer busy timeout: {error}");
                            }
                            connection = Some(conn);
                        }
                        Err(error) => {
                            eprintln!("[journal] remote journal open failed: {error:?}");
                            continue;
                        }
                    }
                }
                if let Some(conn) = &connection {
                    if let Err(error) = conn.execute(
                        "INSERT INTO journal_events (event_id,timestamp_ms,operation_id,job_id,event_type,version,source_id,outcome,payload_json) VALUES (?1,?2,?3,?4,?5,1,?6,?7,?8)",
                        params![event.id, event.timestamp_ms, event.operation_id,
                            event.details.get("jobId").and_then(Value::as_str),
                            event.kind.event_type(), event.details.get("sourceId").and_then(Value::as_str),
                            event.phase.name(), event.payload.to_string()],
                    ) {
                        eprintln!("[journal] remote journal write failed: {error}");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_drains_queued_events_when_final_operation_and_journal_drop() {
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let dir = Cleanup(
            std::env::temp_dir().join(format!("fbro-journal-drain-{}", uuid::Uuid::new_v4())),
        );
        let path = dir.0.join("journal.sqlite3");
        let writer_path = path.clone();
        let (sender, receiver) = mpsc::channel();
        let writer = std::thread::spawn(move || write_events(writer_path, receiver));
        let journal = RemoteJournal {
            queue: Arc::new(Mutex::new(Queue {
                sender,
                sequence: 0,
            })),
        };
        for _ in 0..32 {
            let operation = journal.begin(RemoteKind::Transfer, None, None, "send");
            assert!(operation.record(RemotePhase::Failed, json!({})));
        }
        drop(journal);
        writer
            .join()
            .expect("writer stops after draining disconnected channel");
        let conn = Connection::open(path).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM journal_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 32);
    }
}
