use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::{active_job_count, save_session, IntentContext};

pub(crate) async fn handle(name: &str, ctx: &mut IntentContext<'_>) {
    match name {
        "Quit" => quit(ctx).await,
        "ConfirmQuit" => confirm_quit(ctx).await,
        _ => {}
    }
}

async fn quit(ctx: &mut IntentContext<'_>) {
    let active_jobs = active_job_count(ctx.state).await;
    if active_jobs == 0 {
        save_session(ctx.state).await;
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
    save_session(ctx.state).await;
    std::process::exit(0);
}
