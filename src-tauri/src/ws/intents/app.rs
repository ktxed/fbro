use serde_json::Value;

use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{active_job_count, save_session, IntentContext};

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "Quit" => quit(ctx).await,
        "ConfirmQuit" => confirm_quit(ctx).await,
        "SetFontSize" => set_font_size(args, ctx).await,
        _ => {}
    }
}

async fn quit(ctx: &mut IntentContext<'_>) {
    let active_jobs = active_job_count(ctx.state).await;
    if active_jobs == 0 {
        save_session(ctx.state, ctx.app_handle).await;
        std::process::exit(0);
    } else {
        let _ = ctx
            .tx
            .send(ServerMessage::QuitConfirmRequired {
                timestamp: now(),
                active_jobs,
            })
            .await;
    }
}

async fn confirm_quit(ctx: &mut IntentContext<'_>) {
    save_session(ctx.state, ctx.app_handle).await;
    std::process::exit(0);
}

async fn set_font_size(args: &Value, ctx: &mut IntentContext<'_>) {
    let Some(value) = args.get("value").and_then(Value::as_u64) else { return };
    let font_size = value.clamp(8, 24) as u8;
    let gui_senders = {
        let mut state_guard = ctx.state.lock().await;
        state_guard.font_size = font_size;
        state_guard.gui_senders.clone()
    };
    save_session(ctx.state, ctx.app_handle).await;
    let patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "settings".to_string(),
        entity_id: None,
        version: 0,
        patch: serde_json::json!({ "fontSize": font_size }),
    };
    for sender in &gui_senders {
        let _ = sender.send(patch.clone()).await;
    }
    let _ = ctx.tx.send(patch).await;
}
