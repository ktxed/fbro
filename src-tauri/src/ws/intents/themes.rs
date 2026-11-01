//! Custom color scheme intents: create, update and delete files in
//! `<config_dir>/themes/`. See `spex/settings.md` ("Custom color schemes").

use std::collections::BTreeMap;

use serde_json::Value;

use super::arg_str;
use crate::themes::{self, TerminalColors, Theme};
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::IntentContext;

pub(crate) async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    // Same gate as settings writes so patches and settings.json stay ordered.
    let mutation = { ctx.state.lock().await.settings_mutation.clone() };
    let _mutation = mutation.lock().await;
    let request_id = arg_str(args, "requestId").unwrap_or_default().to_string();
    let result = if request_id.trim().is_empty() {
        Err(("BAD_COLOR_SCHEME", "A requestId is required".to_string()))
    } else {
        match name {
            "CreateColorScheme" => create(args, ctx).await,
            "UpdateColorScheme" => update(args, ctx).await,
            "DeleteColorScheme" => delete(args, ctx).await,
            _ => return,
        }
    };
    let reply = match result {
        Ok(id) => ServerMessage::ColorSchemeSaved { timestamp: now(), request_id, id },
        Err((code, message)) => ServerMessage::ColorSchemeFailed {
            timestamp: now(),
            request_id,
            code: code.to_string(),
            message,
        },
    };
    let _ = ctx.tx.send(reply).await;
}

type Failure = (&'static str, String);

fn bad(message: impl Into<String>) -> Failure {
    ("BAD_COLOR_SCHEME", message.into())
}

fn io_failure(context: &str, error: impl std::fmt::Display) -> Failure {
    ("THEME_IO", format!("{context}: {error}"))
}

fn string_map(value: Option<&Value>, what: &str) -> Result<BTreeMap<String, String>, Failure> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let object = value.as_object().ok_or_else(|| bad(format!("{what} must be an object")))?;
    object
        .iter()
        .map(|(k, v)| {
            v.as_str()
                .map(|s| (k.clone(), s.trim().to_string()))
                .ok_or_else(|| bad(format!("{what}.{k} must be a string")))
        })
        .collect()
}

/// Apply the provided terminal keys (camelCase on the wire) onto `terminal`.
fn merge_terminal(terminal: &mut TerminalColors, value: Option<&Value>) -> Result<(), Failure> {
    let provided = string_map(value, "terminal")?;
    for (key, color) in provided {
        match key.as_str() {
            "background" => terminal.background = Some(color),
            "foreground" => terminal.foreground = Some(color),
            "cursor" => terminal.cursor = Some(color),
            "selectionBackground" => terminal.selection_background = Some(color),
            other => return Err(bad(format!("Unknown terminal color: {other}"))),
        }
    }
    Ok(())
}

async fn reload(ctx: &IntentContext<'_>) -> Result<(), Failure> {
    let catalog = tokio::task::spawn_blocking(themes::load_all)
        .await
        .map_err(|e| io_failure("Reload color schemes", e))?;
    crate::ws::appearance::publish_color_schemes(ctx.state, catalog).await;
    Ok(())
}

async fn create(args: &Value, ctx: &mut IntentContext<'_>) -> Result<String, Failure> {
    let mut theme = Theme {
        version: themes::FORMAT_VERSION,
        name: arg_str(args, "name").unwrap_or_default().trim().to_string(),
        description: arg_str(args, "description").unwrap_or_default().trim().to_string(),
        mode: arg_str(args, "mode").unwrap_or("dark").to_string(),
        base: arg_str(args, "base").unwrap_or(themes::BASE_DARK).to_string(),
        colors: string_map(args.get("colors"), "colors")?,
        terminal: TerminalColors::default(),
    };
    merge_terminal(&mut theme.terminal, args.get("terminal"))?;
    themes::validate_theme(&theme).map_err(bad)?;
    let slug = tokio::task::spawn_blocking(move || {
        let dir = themes::themes_dir();
        let slug = themes::unique_slug(&theme.name, &themes::taken_slugs(&dir));
        themes::save_to(&dir, &slug, &theme).map(|_| slug)
    })
    .await
    .map_err(|e| io_failure("Save color scheme", e))?
    .map_err(|e| io_failure("Save color scheme", e))?;
    reload(ctx).await?;
    Ok(format!("{}{slug}", themes::CUSTOM_PREFIX))
}

async fn update(args: &Value, ctx: &mut IntentContext<'_>) -> Result<String, Failure> {
    let id = arg_str(args, "id").unwrap_or_default().to_string();
    let slug = themes::slug_of(&id).ok_or_else(|| bad("Unknown color scheme"))?.to_string();
    let name = arg_str(args, "name").map(|s| s.trim().to_string());
    let description = arg_str(args, "description").map(|s| s.trim().to_string());
    let mode = arg_str(args, "mode").map(str::to_string);
    let colors = string_map(args.get("colors"), "colors")?;
    let mut terminal = TerminalColors::default();
    merge_terminal(&mut terminal, args.get("terminal"))?;
    tokio::task::spawn_blocking(move || -> Result<(), Failure> {
        let dir = themes::themes_dir();
        let path = dir.join(format!("{slug}.toml"));
        let mut theme = themes::read_theme(&path).map_err(|e| bad(format!("Cannot update: {e}")))?;
        if let Some(name) = name {
            theme.name = name;
        }
        if let Some(description) = description {
            theme.description = description;
        }
        if let Some(mode) = mode {
            theme.mode = mode;
        }
        theme.colors.extend(colors);
        for (slot, value) in [
            (&mut theme.terminal.background, terminal.background),
            (&mut theme.terminal.foreground, terminal.foreground),
            (&mut theme.terminal.cursor, terminal.cursor),
            (&mut theme.terminal.selection_background, terminal.selection_background),
        ] {
            if value.is_some() {
                *slot = value;
            }
        }
        themes::validate_theme(&theme).map_err(bad)?;
        themes::save_to(&dir, &slug, &theme).map_err(|e| io_failure("Save color scheme", e))
    })
    .await
    .map_err(|e| io_failure("Save color scheme", e))??;
    reload(ctx).await?;
    Ok(id)
}

async fn delete(args: &Value, ctx: &mut IntentContext<'_>) -> Result<String, Failure> {
    let id = arg_str(args, "id").unwrap_or_default().to_string();
    let slug = themes::slug_of(&id).ok_or_else(|| bad("Unknown color scheme"))?.to_string();
    let base = tokio::task::spawn_blocking(move || -> Result<String, Failure> {
        let dir = themes::themes_dir();
        let base = themes::read_theme(&dir.join(format!("{slug}.toml")))
            .map(|t| t.base)
            .unwrap_or_else(|_| themes::BASE_DARK.to_string());
        themes::delete_from(&dir, &slug).map_err(|e| io_failure("Delete color scheme", e))?;
        Ok(base)
    })
    .await
    .map_err(|e| io_failure("Delete color scheme", e))??;
    {
        // A deleted manual choice falls back to the scheme it was based on.
        let mut state_guard = ctx.state.lock().await;
        if state_guard.settings.general.color_scheme == id {
            state_guard.settings.general.color_scheme = base;
            if let Err(error) = crate::state::settings::save(&state_guard.settings) {
                eprintln!("[settings] Failed to save settings: {error}");
            }
        }
    }
    reload(ctx).await?;
    Ok(id)
}
