use serde_json::Value;

use crate::ws::protocol::{now, PrefetchCacheSchema, ServerMessage};
use crate::ws::runtime::IntentContext;

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "QueryCacheStatus" => {
            send_cache_status(ctx).await;
        }
        "ClearCache" => {
            let cache_name = args.get("cache").and_then(Value::as_str).unwrap_or("prefetch");
            if cache_name == "prefetch" {
                ctx.viewer_prefetch.clear();
                send_cache_status(ctx).await;
            }
        }
        _ => {}
    }
}

async fn send_cache_status(ctx: &mut IntentContext<'_>) {
    let stats = ctx.viewer_prefetch.stats();
    let _ = ctx
        .tx
        .send(ServerMessage::CacheStatusReady {
            timestamp: now(),
            prefetch_cache: PrefetchCacheSchema {
                entry_count: stats.entry_count,
                total_bytes: stats.total_bytes,
                max_entries: stats.max_entries,
            },
        })
        .await;
}
