use std::collections::HashMap;

use serde_json::Value;

use crate::bro::{launch_bro, list_bros, BroInputItem};
use crate::ws::intents::arg_str;
use crate::ws::protocol::{now, BroInputSchema, BroLaunchInputSchema, BroSchema, ServerMessage};
use crate::ws::runtime::{resolve_panel_id, IntentContext};

pub(crate) async fn handle(name: &str, scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    match name {
        "ListBros" => list_bros_for_request(scope, args, ctx).await,
        "LaunchBro" => launch_selected_bro(args, ctx).await,
        _ => {}
    }
}

struct ListBrosArgs {
    request_id: String,
    target_path: Option<String>,
    suggested_output_dir: Option<String>,
}

impl ListBrosArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            request_id: arg_str(args, "requestId").unwrap_or("").to_string(),
            target_path: arg_str(args, "targetPath").map(String::from),
            suggested_output_dir: arg_str(args, "suggestedOutputDir").map(String::from),
        }
    }
}

struct LaunchBroArgs {
    request_id: String,
    bro_id: String,
    inputs: HashMap<String, String>,
}

impl LaunchBroArgs {
    fn from_args(args: &Value) -> Self {
        Self {
            request_id: arg_str(args, "requestId").unwrap_or("").to_string(),
            bro_id: arg_str(args, "broId").unwrap_or("").to_string(),
            inputs: args
                .get("inputs")
                .and_then(Value::as_object)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

async fn list_bros_for_request(scope: &Value, args: &Value, ctx: &mut IntentContext<'_>) {
    let panel_id = resolve_panel_id(scope, ctx.state).await;
    let parsed = ListBrosArgs::from_args(args);
    if parsed.request_id.is_empty() {
        return;
    }

    let result = {
        let state_guard = ctx.state.lock().await;
        match state_guard.panels.get(&panel_id) {
            Some(panel) => match state_guard.drivers.get(&panel.source_id) {
                Some(driver) => {
                    resolve_inputs(panel, driver.as_ref(), parsed.target_path.as_deref())
                }
                None => Err("Source driver is not available.".to_string()),
            },
            None => Err("Panel not found.".to_string()),
        }
    };

    let inputs = match result {
        Ok(inputs) => inputs,
        Err(message) => {
            let _ = ctx
                .tx
                .send(ServerMessage::BroOptionsFailed {
                    timestamp: now(),
                    request_id: parsed.request_id,
                    message,
                })
                .await;
            return;
        }
    };

    let lookup_inputs = inputs.clone();
    let suggested_output_dir = parsed.suggested_output_dir.clone();
    let bros = match tokio::task::spawn_blocking(move || {
        list_bros(&lookup_inputs, suggested_output_dir.as_deref())
    })
    .await
    {
        Ok(bros) => bros,
        Err(_) => {
            let _ = ctx
                .tx
                .send(ServerMessage::BroOptionsFailed {
                    timestamp: now(),
                    request_id: parsed.request_id,
                    message: "Bro discovery failed.".to_string(),
                })
                .await;
            return;
        }
    };
    let input_schemas = inputs
        .iter()
        .map(|input| BroInputSchema {
            path: input.path.clone(),
            name: input.name.clone(),
            kind: input.kind.clone(),
        })
        .collect::<Vec<_>>();
    let bro_schemas = bros
        .iter()
        .map(|bro| BroSchema {
            id: bro.option.id.clone(),
            name: bro.option.name.clone(),
            version: bro.option.version.clone(),
            source: bro.option.source.as_str().to_string(),
            detail: bro.option.detail.clone(),
            protocol: bro.option.protocol.as_str().to_string(),
            inputs: bro
                .option
                .inputs
                .iter()
                .map(|input| BroLaunchInputSchema {
                    id: input.id.clone(),
                    label: input.label.clone(),
                    kind: input.kind.as_str().to_string(),
                    required: input.required,
                    default_value: input.default_value.clone(),
                })
                .collect(),
        })
        .collect::<Vec<_>>();

    {
        let mut state_guard = ctx.state.lock().await;
        state_guard.bro_requests.insert(
            parsed.request_id.clone(),
            crate::state::BroRequest { inputs, bros },
        );
        if state_guard.bro_requests.len() > 32 {
            if let Some(oldest) = state_guard.bro_requests.keys().next().cloned() {
                state_guard.bro_requests.remove(&oldest);
            }
        }
    }

    let _ = ctx
        .tx
        .send(ServerMessage::BroOptionsReady {
            timestamp: now(),
            request_id: parsed.request_id,
            inputs: input_schemas,
            bros: bro_schemas,
        })
        .await;
}

async fn launch_selected_bro(args: &Value, ctx: &mut IntentContext<'_>) {
    let parsed = LaunchBroArgs::from_args(args);
    if parsed.request_id.is_empty() || parsed.bro_id.is_empty() {
        return;
    }

    let request = {
        let mut state_guard = ctx.state.lock().await;
        state_guard.bro_requests.remove(&parsed.request_id)
    };

    let Some(request) = request else {
        return;
    };

    let Some(bro) = request
        .bros
        .iter()
        .find(|bro| bro.option.id == parsed.bro_id)
        .cloned()
    else {
        return;
    };

    if let Err(message) = launch_bro(&request.inputs, &bro, &parsed.inputs) {
        let _ = ctx
            .tx
            .send(ServerMessage::Error {
                timestamp: now(),
                code: "BRO_LAUNCH_FAILED".to_string(),
                message,
                recoverable: true,
                extra: Some(serde_json::json!({
                    "requestId": parsed.request_id,
                    "broId": parsed.bro_id,
                })),
            })
            .await;
    }
}

fn resolve_inputs(
    panel: &crate::state::Panel,
    driver: &dyn crate::vfs::VfsDriver,
    target_path: Option<&str>,
) -> Result<Vec<BroInputItem>, String> {
    if let Some(target_path) = target_path {
        return panel
            .entries
            .iter()
            .find(|entry| {
                entry.name != ".." && driver.resolve_child(&panel.path, &entry.name) == target_path
            })
            .map(|entry| vec![bro_input_for_entry(panel, driver, entry)])
            .ok_or_else(|| "The requested item is no longer available.".to_string());
    }

    let inputs = if panel.selection.is_empty() {
        entry_at_abs_index(panel, panel.cursor)
            .filter(|entry| entry.name != "..")
            .map(|entry| vec![bro_input_for_entry(panel, driver, entry)])
            .unwrap_or_default()
    } else {
        panel
            .selection
            .iter()
            .filter_map(|index| entry_at_abs_index(panel, *index))
            .filter(|entry| entry.name != "..")
            .map(|entry| bro_input_for_entry(panel, driver, entry))
            .collect()
    };

    if inputs.is_empty() {
        Err("No valid input entries are selected.".to_string())
    } else {
        Ok(inputs)
    }
}

fn entry_at_abs_index<'a>(
    panel: &'a crate::state::Panel,
    abs_index: usize,
) -> Option<&'a crate::ws::protocol::EntrySchema> {
    let local_index = abs_index.saturating_sub(panel.window_start);
    panel.entries.get(local_index)
}

fn bro_input_for_entry(
    panel: &crate::state::Panel,
    driver: &dyn crate::vfs::VfsDriver,
    entry: &crate::ws::protocol::EntrySchema,
) -> BroInputItem {
    BroInputItem {
        path: driver.resolve_child(&panel.path, &entry.name),
        name: entry.name.clone(),
        kind: entry.kind.clone(),
    }
}
