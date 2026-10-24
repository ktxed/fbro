use super::{event_name, EventRow, RemoteJournal, RemoteKind, RemotePhase};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::PathBuf;

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("fbro-remote-journal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn database(&self) -> PathBuf {
        self.0.join("journal.sqlite3")
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn persists_remote_lifecycle_with_readable_names_and_row_correlations() {
    let dir = TempDir::new();
    let journal = RemoteJournal::new(dir.database());
    let op = journal.begin(
        RemoteKind::Transfer,
        Some("SHA256:peer"),
        Some("Laptop"),
        "send",
    );
    let before = crate::ws::protocol::now();
    assert!(op.record(
        RemotePhase::Started,
        json!({"jobId":"job-1", "sourceId":"remote:peer", "files":["a", "b"]})
    ));
    assert!(op.record(RemotePhase::Completed, json!({"jobId":"job-1", "bytes":42})));
    journal.flush().await;
    drop(op);
    drop(journal);
    let conn = Connection::open(dir.database()).unwrap();
    let mut query = conn.prepare("SELECT event_id,timestamp_ms,operation_id,job_id,event_type,version,source_id,outcome,payload_json FROM journal_events ORDER BY rowid").unwrap();
    let rows = query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 2, "files remain details, not separate events");
    assert_ne!(rows[0].0, rows[1].0);
    for row in &rows {
        assert!(uuid::Uuid::parse_str(&row.0).is_ok());
        assert!(uuid::Uuid::parse_str(&row.2).is_ok());
        assert!(row.1 >= before as i64);
        assert_eq!(row.2, rows[0].2);
        assert_eq!(row.3, "job-1");
        assert_eq!(row.4, "remote.transfer.v1");
        assert_eq!(row.5, 1);
        let payload: Value = serde_json::from_str(&row.8).unwrap();
        assert_eq!(payload["operationId"], row.2);
        assert_eq!(payload["timestampMs"], row.1);
        assert_eq!(payload["peerFingerprint"], "SHA256:peer");
        assert_eq!(payload["peerName"], "Laptop");
        assert_eq!(payload["direction"], "send");
        assert_eq!(payload["kind"], "remote-transfer");
        assert_eq!(payload["phase"], row.7);
        let event = EventRow {
            id: row.0.clone(),
            timestamp_ms: row.1,
            event_type: row.4.clone(),
            outcome: Some(row.7.clone()),
            payload: row.8.clone(),
        };
        assert_eq!(
            event_name(&event),
            format!("remote-transfer_v1_{}_{}.log", row.7, row.0)
        );
    }
    assert_eq!(rows[0].6.as_deref(), Some("remote:peer"));
    let payload: Value = serde_json::from_str(&rows[0].8).unwrap();
    assert_eq!(payload["details"]["files"], json!(["a", "b"]));
    assert_eq!(payload["sequence"], 1);
}

#[tokio::test]
async fn lifecycle_deduplicates_clones_and_allows_resume_and_cancellation_after_end() {
    let dir = TempDir::new();
    let journal = RemoteJournal::new(dir.database());
    let op = journal.begin(RemoteKind::Hash, None, None, "receive");
    assert!(!op.record(RemotePhase::Resumed, json!({})));
    assert!(op.record(RemotePhase::Started, json!({})));
    assert!(!op.clone().record(RemotePhase::Started, json!({})));
    assert!(op.record(RemotePhase::Ended, json!({})));
    assert!(!op.record(RemotePhase::Ended, json!({})));
    assert!(op.record(RemotePhase::Resumed, json!({})));
    assert!(!op.record(RemotePhase::Resumed, json!({})));
    assert!(op.record(RemotePhase::Ended, json!({})));
    assert!(op.record(RemotePhase::Cancelled, json!({})));
    for phase in [
        RemotePhase::Completed,
        RemotePhase::Failed,
        RemotePhase::Started,
        RemotePhase::Resumed,
        RemotePhase::Ended,
    ] {
        assert!(!op.record(phase, json!({})));
    }
    journal.flush().await;
    let conn = Connection::open(dir.database()).unwrap();
    let phases = conn
        .prepare("SELECT outcome FROM journal_events ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        phases,
        ["started", "ended", "resumed", "ended", "cancelled"]
    );
}

#[tokio::test]
async fn reused_job_ids_keep_distinct_operations_and_writer_preserves_sequence() {
    let dir = TempDir::new();
    let journal = RemoteJournal::new(dir.database());
    let first = journal.begin(RemoteKind::Sync, None, None, "send");
    let second = journal.begin(RemoteKind::Sync, None, None, "send");
    assert_ne!(first.operation_id(), second.operation_id());
    for op in [&first, &second] {
        assert!(op.record(RemotePhase::Started, json!({"jobId":"job-reused"})));
        assert!(op.record(RemotePhase::Completed, json!({"jobId":"job-reused"})));
    }
    journal.flush().await;
    let conn = Connection::open(dir.database()).unwrap();
    let rows = conn
        .prepare("SELECT operation_id,payload_json FROM journal_events ORDER BY rowid")
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].0, rows[1].0);
    assert_eq!(rows[2].0, rows[3].0);
    assert_ne!(rows[0].0, rows[2].0);
    for (index, (_, payload)) in rows.iter().enumerate() {
        assert_eq!(
            serde_json::from_str::<Value>(payload).unwrap()["sequence"],
            index + 1
        );
    }
}

#[tokio::test]
async fn terminal_phases_settle_once_even_when_execution_fails_before_start() {
    let dir = TempDir::new();
    let journal = RemoteJournal::new(dir.database());
    for phase in [
        RemotePhase::Completed,
        RemotePhase::Cancelled,
        RemotePhase::Failed,
        RemotePhase::Disconnected,
        RemotePhase::Paired,
        RemotePhase::Rejected,
        RemotePhase::Timeout,
        RemotePhase::Revoked,
    ] {
        let op = journal.begin(RemoteKind::Pairing, None, None, "outbound");
        assert!(op.record(phase, json!({})));
        assert!(!op.record(phase, json!({})));
        assert!(!op.record(RemotePhase::Requested, json!({})));
    }
    let connection = journal.begin(RemoteKind::Connection, None, None, "outbound");
    assert!(connection.record(RemotePhase::Connected, json!({})));
    assert!(!connection.record(RemotePhase::Connected, json!({})));
    assert!(connection.record(RemotePhase::Disconnected, json!({})));
    let pairing = journal.begin(RemoteKind::Pairing, None, None, "inbound");
    assert!(pairing.record(RemotePhase::Requested, json!({})));
    assert!(!pairing.record(RemotePhase::Requested, json!({})));
    journal.flush().await;
}

#[tokio::test]
async fn persistence_failure_is_best_effort_and_writer_survives() {
    let dir = TempDir::new();
    let blocked = dir.0.join("blocked");
    std::fs::write(&blocked, "not a directory").unwrap();
    let journal = RemoteJournal::new(blocked.join("journal.sqlite3"));
    let op = journal.begin(RemoteKind::Transfer, None, None, "send");
    assert!(op.record(RemotePhase::Started, json!({})));
    journal.flush().await;
    assert!(op.record(RemotePhase::Failed, json!({"error":"execution failed"})));
    journal.flush().await;
}

#[tokio::test]
async fn learned_peer_identity_fills_missing_context_without_overwriting_reserved_fields() {
    let dir = TempDir::new();
    let journal = RemoteJournal::new(dir.database());
    let op = journal.begin(RemoteKind::Pairing, None, None, "outbound");
    assert!(op.record(RemotePhase::Requested, json!({"peerFingerprint":"learned-peer", "peerName":"Desktop", "operationId":"forged", "phase":"forged", "sequence":999})));
    assert!(op.record(
        RemotePhase::Paired,
        json!({"peerFingerprint":"replacement"})
    ));
    journal.flush().await;
    let conn = Connection::open(dir.database()).unwrap();
    let rows = conn
        .prepare("SELECT payload_json FROM journal_events ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for (index, row) in rows.iter().enumerate() {
        let payload: Value = serde_json::from_str(row).unwrap();
        assert_eq!(payload["operationId"], op.operation_id());
        assert_eq!(payload["peerFingerprint"], "learned-peer");
        assert_eq!(payload["peerName"], "Desktop");
        assert_eq!(payload["sequence"], index + 1);
    }
}

#[tokio::test]
async fn concurrent_operations_are_persisted_in_enqueued_sequence_order() {
    let dir = TempDir::new();
    let journal = RemoteJournal::new(dir.database());
    let gate = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let journal = journal.clone();
            let gate = gate.clone();
            std::thread::spawn(move || {
                let op = journal.begin(RemoteKind::Transfer, None, None, "send");
                gate.wait();
                assert!(op.record(RemotePhase::Started, json!({})));
                assert!(op.record(RemotePhase::Completed, json!({})));
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    journal.flush().await;
    let conn = Connection::open(dir.database()).unwrap();
    let payloads = conn
        .prepare("SELECT payload_json FROM journal_events ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(payloads.len(), 16);
    for (index, payload) in payloads.iter().enumerate() {
        assert_eq!(
            serde_json::from_str::<Value>(payload).unwrap()["sequence"],
            index + 1
        );
    }
}

#[tokio::test]
async fn writer_recovers_after_sqlite_rejects_an_insert_without_affecting_lifecycle() {
    let dir = TempDir::new();
    let journal = RemoteJournal::new(dir.database());
    let op = journal.begin(RemoteKind::Transfer, None, None, "send");
    assert!(op.record(RemotePhase::Started, json!({})));
    journal.flush().await;
    let conn = Connection::open(dir.database()).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_event BEFORE INSERT ON journal_events BEGIN SELECT RAISE(ABORT, 'test journal failure'); END;").unwrap();
    assert!(op.record(RemotePhase::Ended, json!({})));
    journal.flush().await;
    conn.execute_batch("DROP TRIGGER reject_event;").unwrap();
    assert!(op.record(RemotePhase::Cancelled, json!({})));
    journal.flush().await;
    let phases = conn
        .prepare("SELECT outcome FROM journal_events ORDER BY rowid")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(phases, ["started", "cancelled"]);
}

#[tokio::test]
async fn rejected_transitions_do_not_build_file_snapshots() {
    let dir = TempDir::new();
    let journal = RemoteJournal::new(dir.database());
    let operation = journal.begin(RemoteKind::Transfer, None, None, "outgoing");
    assert!(operation.record(RemotePhase::Started, json!({})));
    assert!(!operation.record_with(RemotePhase::Started, || panic!(
        "duplicate start built metadata"
    )));
    assert!(!operation.record_with(RemotePhase::Resumed, || panic!(
        "active progress built metadata"
    )));
    assert!(operation.record_with(RemotePhase::Ended, || json!({"files": ["a.txt"]})));
    assert!(!operation.record_with(RemotePhase::Ended, || panic!(
        "duplicate end built metadata"
    )));
    journal.flush().await;
}
