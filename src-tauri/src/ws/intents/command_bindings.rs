fn prepare(
    name: &str,
    args: &Value,
    settings: &crate::state::settings::Settings,
    revision: u64,
) -> Result<crate::state::settings::Settings, ServerMessage> {
    let request_id = super::arg_str(args, "requestId")
        .unwrap_or_default()
        .to_string();
    let fail = |code: &str, message: String| ServerMessage::CommandBindingsFailed {
        timestamp: now(),
        request_id: request_id.clone(),
        code: code.into(),
        message,
        revision,
    };
    let requested_revision = args.get("revision").and_then(Value::as_u64);
    if request_id.is_empty() || requested_revision.is_none() {
        Err(fail(
            "INVALID_BINDINGS",
            "requestId and revision are required".into(),
        ))
    } else if check_revision(revision, requested_revision).is_err() {
        Err(fail(
            "STALE_REVISION",
            "Shortcuts changed; review the current bindings and try again".into(),
        ))
    } else {
        let parsed = (|| {
            if name == "ResetAllCommandBindings" {
                return candidate(&settings, None, None, false);
            }
            let id = super::arg_str(args, "commandId")
                .ok_or_else(|| BindingError::Invalid("commandId is required".into()))?;
            let replace = args
                .get("override")
                .and_then(Value::as_bool)
                .ok_or_else(|| BindingError::Invalid("override must be a boolean".into()))?;
            let bindings = if name == "SetCommandBindings" {
                Some(
                    serde_json::from_value::<Vec<KeyBindingSchema>>(
                        args.get("bindings").cloned().unwrap_or(Value::Null),
                    )
                    .map_err(|e| BindingError::Invalid(format!("Invalid bindings: {e}")))?,
                )
            } else {
                None
            };
            candidate(&settings, Some(id), bindings, replace)
        })();
        parsed.map_err(|error| match error {
            BindingError::Invalid(message) => fail("INVALID_BINDINGS", message),
            BindingError::Conflict(conflicts) => ServerMessage::CommandBindingsConflict {
                timestamp: now(),
                request_id: request_id.clone(),
                revision,
                conflicts,
            },
        })
    }
}

use crate::ws::{
    command_bindings::{candidate, check_revision, effective_commands, persist, BindingError},
    protocol::{now, KeyBindingSchema, ServerMessage},
    runtime::IntentContext,
};
use serde_json::Value;

pub async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    let request_id = super::arg_str(args, "requestId")
        .unwrap_or_default()
        .to_string();
    let mutation = { ctx.state.lock().await.settings_mutation.clone() };
    let _mutation = mutation.lock().await;
    let (revision, settings) = {
        let state = ctx.state.lock().await;
        (state.command_revision, state.settings.clone())
    };
    let fail = |code: &str, message: String| ServerMessage::CommandBindingsFailed {
        timestamp: now(),
        request_id: request_id.clone(),
        code: code.into(),
        message,
        revision,
    };
    let result = prepare(name, args, &settings, revision);
    let next = match result {
        Ok(next) => next,
        Err(reply) => {
            let _ = ctx.tx.send(reply).await;
            return;
        }
    };
    let saved =
        tokio::task::spawn_blocking(move || persist(next, crate::state::settings::save)).await;
    let next = match saved
        .map_err(|e| e.to_string())
        .and_then(|result| result.map_err(|e| e.to_string()))
    {
        Ok(next) => next,
        Err(message) => {
            let _ = ctx
                .tx
                .send(fail(
                    "STORAGE_FAILED",
                    format!("Could not save shortcuts: {message}"),
                ))
                .await;
            return;
        }
    };
    let next_bindings = next.command_bindings;
    let (commands, senders, revision) = {
        let mut state = ctx.state.lock().await;
        state.settings.command_bindings = next_bindings;
        state.command_revision += 1;
        (
            effective_commands(&state.settings),
            state.gui_senders.clone(),
            state.command_revision,
        )
    };
    let patch = ServerMessage::StatePatch {
        timestamp: now(),
        domain: "commands".into(),
        entity_id: None,
        version: revision,
        patch: serde_json::json!({"commands": commands, "commandRevision": revision}),
    };
    for sender in senders {
        let _ = sender.send(patch.clone()).await;
    }
    let _ = ctx.tx.send(patch).await;
    let _ = ctx
        .tx
        .send(ServerMessage::CommandBindingsOk {
            timestamp: now(),
            request_id,
            revision,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::settings::Settings;
    #[test]
    fn stale_confirmations_fail_for_every_mutation_with_current_revision() {
        for name in [
            "SetCommandBindings",
            "ResetCommandBindings",
            "ResetAllCommandBindings",
        ] {
            let args = serde_json::json!({"requestId":"r1", "revision": 3, "commandId":"app.open_jobs", "bindings":[], "override":true});
            let reply = prepare(name, &args, &Settings::default(), 4).err().unwrap();
            let value = serde_json::to_value(reply).unwrap();
            assert_eq!(value["type"], "CommandBindingsFailed");
            assert_eq!(value["requestId"], "r1");
            assert_eq!(value["revision"], 4);
            assert_eq!(value["code"], "STALE_REVISION");
        }
    }
    #[test]
    fn conflict_reply_is_correlated_and_reviewed_again_after_revision_changes() {
        let args = serde_json::json!({"requestId":"review", "revision":0, "commandId":"app.about", "bindings":[{"strokes":[{"key":"j"}],"modes":[]}], "override":false});
        let reply = prepare("SetCommandBindings", &args, &Settings::default(), 0)
            .err()
            .unwrap();
        let value = serde_json::to_value(reply).unwrap();
        assert_eq!(value["type"], "CommandBindingsConflict");
        assert_eq!(value["requestId"], "review");
        assert_eq!(value["conflicts"][0]["commandId"], "app.open_jobs");
        let mut confirmed = args.clone();
        confirmed["override"] = Value::Bool(true);
        assert!(prepare("SetCommandBindings", &confirmed, &Settings::default(), 0).is_ok());
        assert!(prepare("SetCommandBindings", &confirmed, &Settings::default(), 1).is_err());
    }
    #[test]
    fn malformed_requests_never_make_candidates() {
        for args in [
            serde_json::json!({}),
            serde_json::json!({"requestId":"r", "revision":0, "commandId":"app.about", "bindings":null, "override":false}),
            serde_json::json!({"requestId":"r", "revision":0, "commandId":"app.about", "bindings":[]}),
        ] {
            let reply = prepare("SetCommandBindings", &args, &Settings::default(), 0)
                .err()
                .unwrap();
            assert_eq!(
                serde_json::to_value(reply).unwrap()["code"],
                "INVALID_BINDINGS"
            );
        }
    }
}
