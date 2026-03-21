use tokio::sync::mpsc;

use crate::vfs::local::LocalFsDriver;
use crate::ws::protocol::{EntrySchema, FilterSpec, ServerMessage, SortSpec};

const BATCH_SIZE: usize = 50;

pub async fn run_scan(
    panel_id: String,
    path: String,
    sort: SortSpec,
    filter: Option<FilterSpec>,
    version_base: u64,
    tx: mpsc::Sender<ServerMessage>,
) {
    // Run blocking FS read in a spawn_blocking task so we don't stall the async runtime
    let result = tokio::task::spawn_blocking(move || {
        LocalFsDriver::list(&path, &sort, filter.as_ref())
    })
    .await;

    let entries = match result {
        Ok(Ok(e)) => e,
        Ok(Err(err)) => {
            let _ = tx
                .send(ServerMessage::StatePatch {
                    timestamp: crate::ws::protocol::now(),
                    domain: "panels".to_string(),
                    entity_id: Some(panel_id.clone()),
                    version: version_base + 1,
                    patch: serde_json::json!({ "status": "error", "error": err.to_string() }),
                })
                .await;
            return;
        }
        Err(_) => return,
    };

    let schema_entries: Vec<EntrySchema> = entries
        .into_iter()
        .map(|e| EntrySchema {
            name: e.name,
            kind: e.kind.as_str().to_string(),
            size: e.size,
            mtime: e.mtime,
        })
        .collect();

    let total = schema_entries.len();
    let mut version = version_base;

    // Send in batches
    for (batch_idx, chunk) in schema_entries.chunks(BATCH_SIZE).enumerate() {
        version += 1;
        let patch = serde_json::json!({ "appendEntries": chunk });
        if tx
            .send(ServerMessage::StatePatch {
                timestamp: crate::ws::protocol::now(),
                domain: "panels".to_string(),
                entity_id: Some(panel_id.clone()),
                version,
                patch,
            })
            .await
            .is_err()
        {
            return; // receiver dropped (client disconnected)
        }

        // Yield occasionally to avoid starving other tasks
        if batch_idx % 4 == 3 {
            tokio::task::yield_now().await;
        }
    }

    // Mark ready
    version += 1;
    let _ = tx
        .send(ServerMessage::StatePatch {
            timestamp: crate::ws::protocol::now(),
            domain: "panels".to_string(),
            entity_id: Some(panel_id),
            version,
            patch: serde_json::json!({ "status": "ready", "total": total }),
        })
        .await;
}
