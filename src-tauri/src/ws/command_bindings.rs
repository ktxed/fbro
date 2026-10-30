use crate::state::settings::Settings;
use crate::ws::protocol::{
    CommandBindingConflictSchema, InteractiveCommandSchema, KeyBindingSchema, KeyStrokeSchema,
};

pub(crate) fn check_revision(current: u64, requested: Option<u64>) -> Result<(), &'static str> {
    if requested == Some(current) {
        Ok(())
    } else {
        Err("Shortcuts changed; review the current bindings and try again")
    }
}
pub(crate) fn persist(
    settings: Settings,
    save: impl FnOnce(&Settings) -> std::io::Result<()>,
) -> std::io::Result<Settings> {
    save(&settings)?;
    Ok(settings)
}

#[derive(Debug)]
pub(crate) enum BindingError {
    Invalid(String),
    Conflict(Vec<CommandBindingConflictSchema>),
}

pub(crate) fn effective_commands(settings: &Settings) -> Vec<InteractiveCommandSchema> {
    let mut commands = crate::ws::commands::build_interactive_commands();
    for command in &mut commands {
        if let Some(bindings) = settings.command_bindings.get(&command.id) {
            if validate(bindings).is_ok() {
                command.bindings = bindings.clone();
                command.customized = true;
            }
        }
    }
    commands
}

fn normalize(key: &str) -> String {
    if key.chars().count() == 1 {
        key.to_lowercase()
    } else {
        key.to_string()
    }
}
fn symbol(key: &str) -> bool {
    key.chars().count() == 1
        && !key.trim().is_empty()
        && !key.chars().all(|c| c.is_ascii_alphanumeric())
}
fn accepts_shift(stroke: &KeyStrokeSchema, shift: bool) -> bool {
    stroke.shift == shift || (!stroke.shift && shift && symbol(&stroke.key))
}
/// Dispatch/capture canonicalize physical letters/digits before matching.
fn strokes_overlap(a: &KeyStrokeSchema, b: &KeyStrokeSchema) -> bool {
    if (a.ctrl, a.alt, a.meta) != (b.ctrl, b.alt, b.meta) {
        return false;
    }
    [false, true].into_iter().any(|shift| {
        accepts_shift(a, shift) && accepts_shift(b, shift) && {
            let ak = normalize(&a.key);
            let bk = normalize(&b.key);
            ak == bk
        }
    })
}
fn modes(binding: &KeyBindingSchema) -> Vec<String> {
    if binding.modes.is_empty() {
        vec!["list".into(), "grid".into()]
    } else {
        binding.modes.clone()
    }
}
fn overlap(a: &KeyBindingSchema, b: &KeyBindingSchema) -> Vec<String> {
    if !strokes_overlap(&a.strokes[0], &b.strokes[0]) {
        return vec![];
    }
    modes(a)
        .into_iter()
        .filter(|mode| modes(b).contains(mode))
        .collect()
}
fn validate(bindings: &[KeyBindingSchema]) -> Result<(), BindingError> {
    for (i, binding) in bindings.iter().enumerate() {
        if binding.strokes.len() != 1 {
            return Err(BindingError::Invalid(
                "Exactly one stroke per binding is supported".into(),
            ));
        }
        let stroke = &binding.strokes[0];
        let key = normalize(&stroke.key);
        let valid_named = [
            "Enter",
            "Escape",
            "Tab",
            "Backspace",
            "Delete",
            "Insert",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
        ];
        let function = key
            .strip_prefix('F')
            .and_then(|v| v.parse::<u8>().ok())
            .is_some_and(|n| (1..=24).contains(&n) && key == format!("F{n}"));
        if !(key.chars().count() == 1 && !key.chars().any(char::is_control))
            && !valid_named.contains(&key.as_str())
            && !function
        {
            return Err(BindingError::Invalid("Unsupported key".into()));
        }
        if binding.modes.iter().any(|m| m != "list" && m != "grid")
            || binding
                .modes
                .iter()
                .enumerate()
                .any(|(j, m)| binding.modes[..j].contains(m))
        {
            return Err(BindingError::Invalid(
                "Modes must contain list and/or grid without duplicates".into(),
            ));
        }
        if key == "ContextMenu" || (key == "F10" && stroke.shift) {
            return Err(BindingError::Invalid(
                "ContextMenu and Shift+F10 are reserved".into(),
            ));
        }
        if (stroke.ctrl || stroke.meta) && ["+", "=", "-", "0"].contains(&key.as_str()) {
            return Err(BindingError::Invalid(
                "Ctrl/Cmd zoom shortcuts are reserved".into(),
            ));
        }
        if bindings[..i]
            .iter()
            .any(|other| !overlap(binding, other).is_empty())
        {
            return Err(BindingError::Invalid(
                "Bindings of the same command overlap".into(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn candidate(
    settings: &Settings,
    id: Option<&str>,
    bindings: Option<Vec<KeyBindingSchema>>,
    replace: bool,
) -> Result<Settings, BindingError> {
    let mut next = settings.clone();
    let Some(id) = id else {
        next.command_bindings.clear();
        return Ok(next);
    };
    let registry = effective_commands(settings);
    let target = registry
        .iter()
        .find(|command| command.id == id)
        .ok_or_else(|| BindingError::Invalid("Unknown command ID".into()))?;
    let reset = bindings.is_none();
    let bindings = bindings.unwrap_or_else(|| target.default_bindings.clone());
    validate(&bindings)?;
    let mut conflicts = vec![];
    for command in registry.iter().filter(|command| command.id != id) {
        let mut conflicting_modes = vec![];
        let mut retained = vec![];
        for old in &command.bindings {
            let removed: Vec<String> = bindings.iter().flat_map(|new| overlap(old, new)).collect();
            for mode in &removed {
                if !conflicting_modes.contains(mode) {
                    conflicting_modes.push(mode.clone());
                }
            }
            if removed.is_empty() {
                retained.push(old.clone());
            } else {
                let remaining: Vec<String> = modes(old)
                    .into_iter()
                    .filter(|m| !removed.contains(m))
                    .collect();
                if !remaining.is_empty() {
                    let mut binding = old.clone();
                    binding.modes = remaining;
                    retained.push(binding);
                }
            }
        }
        if !conflicting_modes.is_empty() {
            conflicts.push(CommandBindingConflictSchema {
                command_id: command.id.clone(),
                title: command.title.clone(),
                modes: conflicting_modes,
            });
            if replace {
                next.command_bindings.insert(command.id.clone(), retained);
            }
        }
    }
    if !replace && !conflicts.is_empty() {
        return Err(BindingError::Conflict(conflicts));
    }
    if reset {
        next.command_bindings.remove(id);
    } else {
        next.command_bindings.insert(id.into(), bindings);
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ws::commands::build_interactive_commands;
    fn settings() -> Settings {
        Settings::default()
    }
    fn binding(key: &str, modes: &[&str]) -> KeyBindingSchema {
        KeyBindingSchema {
            strokes: vec![KeyStrokeSchema {
                key: key.into(),
                shift: false,
                ctrl: false,
                alt: false,
                meta: false,
            }],
            modes: modes.iter().map(|s| s.to_string()).collect(),
        }
    }
    fn set(
        s: &Settings,
        id: &str,
        bindings: Vec<KeyBindingSchema>,
        replace: bool,
    ) -> Result<Settings, BindingError> {
        candidate(s, Some(id), Some(bindings), replace)
    }
    #[test]
    fn absent_defaults_and_explicit_unassigned_survive_roundtrip() {
        let s = set(&settings(), "app.open_jobs", vec![], false).unwrap();
        let s: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        let commands = effective_commands(&s);
        let jobs = commands.iter().find(|c| c.id == "app.open_jobs").unwrap();
        assert!(jobs.customized && jobs.bindings.is_empty());
        let default = commands.iter().find(|c| c.id == "file.view").unwrap();
        assert!(!default.customized && default.bindings == default.default_bindings);
    }
    #[test]
    fn duplicate_modes_and_invalid_strokes_are_rejected() {
        let b = binding("F12", &[]);
        assert!(set(&settings(), "app.open_jobs", vec![b.clone(), b], false).is_err());
        let mut b = binding("F12", &[]);
        b.strokes.clear();
        assert!(set(&settings(), "app.open_jobs", vec![b], false).is_err());
        assert!(set(
            &settings(),
            "app.open_jobs",
            vec![binding("F12", &["bad"])],
            false
        )
        .is_err());
    }
    #[test]
    fn conflicts_transfer_only_overlapping_modes_and_reset_reviews() {
        let s = set(
            &settings(),
            "app.open_jobs",
            vec![binding("F12", &[])],
            false,
        )
        .unwrap();
        assert!(matches!(
            set(&s, "app.about", vec![binding("F12", &["list"])], false),
            Err(BindingError::Conflict(_))
        ));
        let s = set(&s, "app.about", vec![binding("F12", &["list"])], true).unwrap();
        assert_eq!(s.command_bindings["app.open_jobs"][0].modes, vec!["grid"]);
        let s = set(&s, "app.about", vec![binding("j", &[])], false).unwrap();
        assert!(matches!(
            candidate(&s, Some("app.open_jobs"), None, false),
            Err(BindingError::Conflict(_))
        ));
        let s = candidate(&s, Some("app.open_jobs"), None, true).unwrap();
        assert!(s.command_bindings["app.about"].is_empty());
        assert!(!s.command_bindings.contains_key("app.open_jobs"));
        assert!(candidate(&s, None, None, false)
            .unwrap()
            .command_bindings
            .is_empty());
    }
    #[test]
    fn shifted_symbol_digit_and_zoom_overlap() {
        let symbol = binding("+", &[]);
        let mut shifted = symbol.clone();
        shifted.strokes[0].shift = true;
        assert!(strokes_overlap(&symbol.strokes[0], &shifted.strokes[0]));
        let mut digit = binding("1", &[]);
        digit.strokes[0].shift = true;
        let mut exclamation = binding("!", &[]);
        exclamation.strokes[0].shift = true;
        assert!(!strokes_overlap(&digit.strokes[0], &exclamation.strokes[0]));
        for key in ["+", "=", "-", "0"] {
            let mut b = binding(key, &[]);
            b.strokes[0].ctrl = true;
            assert!(set(&settings(), "app.open_jobs", vec![b], false).is_err());
        }
    }
    #[test]
    fn modes_allow_disjoint_bindings_and_preserve_unrelated_strokes() {
        let next = set(
            &settings(),
            "app.open_jobs",
            vec![
                binding("F12", &["list"]),
                binding("F12", &["grid"]),
                binding("F11", &[]),
            ],
            false,
        )
        .unwrap();
        let next = set(&next, "app.about", vec![binding("F12", &[])], true).unwrap();
        assert_eq!(
            next.command_bindings["app.open_jobs"],
            vec![binding("F11", &[])]
        );
        let next = set(&next, "app.open_jobs", vec![], false).unwrap();
        let restored = candidate(&next, None, None, false).unwrap();
        assert!(effective_commands(&restored)
            .iter()
            .all(|c| c.bindings == c.default_bindings && !c.customized));
    }
    #[test]
    fn canonical_key_names_and_modifier_distinctions_match_dispatch() {
        assert!(set(
            &settings(),
            "app.open_jobs",
            vec![binding("ENTER", &[])],
            false
        )
        .is_err());
        assert!(set(
            &settings(),
            "app.open_jobs",
            vec![binding("Esc", &[])],
            false
        )
        .is_err());
        assert!(set(
            &settings(),
            "app.open_jobs",
            vec![binding("Space", &[])],
            false
        )
        .is_err());
        let mut a = binding("a", &[]);
        let mut b = binding("A", &[]);
        assert!(strokes_overlap(&a.strokes[0], &b.strokes[0]));
        b.strokes[0].alt = true;
        assert!(!strokes_overlap(&a.strokes[0], &b.strokes[0]));
        a.strokes[0].key = "=".into();
        a.strokes[0].shift = true;
        b.strokes[0].key = "+".into();
        b.strokes[0].alt = false;
        assert!(!strokes_overlap(&a.strokes[0], &b.strokes[0]));
        for ctrl in [false, true] {
            for key in ["+", "=", "-", "0"] {
                let mut reserved = binding(key, &[]);
                reserved.strokes[0].ctrl = ctrl;
                reserved.strokes[0].meta = !ctrl;
                reserved.strokes[0].alt = true;
                reserved.strokes[0].shift = true;
                assert!(set(&settings(), "app.open_jobs", vec![reserved], false).is_err());
            }
        }
    }
    #[test]
    fn stale_revision_rejects_even_confirmed_override() {
        assert!(check_revision(3, Some(2)).is_err());
        assert!(check_revision(3, None).is_err());
        assert!(check_revision(3, Some(3)).is_ok());
    }
    #[test]
    fn persistence_failure_never_returns_an_activatable_candidate() {
        let original = settings();
        let next = set(&original, "app.open_jobs", vec![], false).unwrap();
        let result = persist(next, |_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "test failure",
            ))
        });
        assert!(result.is_err());
        assert!(original.command_bindings.is_empty());
    }
    #[test]
    fn settings_file_persists_overrides_and_reports_storage_failure() {
        let dir = std::env::temp_dir().join(format!(
            "fbro-shortcuts-{}-{}",
            std::process::id(),
            crate::ws::protocol::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(dir.clone());
        let next = set(&settings(), "app.open_jobs", vec![], false).unwrap();
        let path = dir.join("settings.json");
        crate::state::settings::save_to_path(&next, &path).unwrap();
        let reloaded: Settings =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(reloaded.command_bindings["app.open_jobs"].is_empty());
        let blocked = dir.join("blocked.json");
        std::fs::create_dir(&blocked).unwrap();
        assert!(crate::state::settings::save_to_path(&next, &blocked).is_err());
    }
    #[test]
    fn dispatch_conflict_fixtures_agree() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/keyboard-binding-semantics.json"
        ))
        .unwrap();
        for fixture in fixtures.as_array().unwrap() {
            let left: KeyBindingSchema = serde_json::from_value(fixture["left"].clone()).unwrap();
            let right: KeyBindingSchema = serde_json::from_value(fixture["right"].clone()).unwrap();
            assert_eq!(
                !overlap(&left, &right).is_empty(),
                fixture["overlap"].as_bool().unwrap(),
                "{fixture}"
            );
        }
    }
    #[test]
    fn fixed_context_menu_shortcuts_are_reserved() {
        let mut b = binding("F10", &[]);
        b.strokes[0].shift = true;
        assert!(validate(&[b]).is_err());
        assert!(validate(&[binding("ContextMenu", &[])]).is_err());
    }
    #[test]
    fn platform_defaults_are_preserved() {
        let commands = build_interactive_commands();
        let rename = commands.iter().find(|c| c.id == "file.rename").unwrap();
        assert_eq!(
            rename.bindings[0].strokes[0].key,
            if cfg!(target_os = "macos") { "2" } else { "F2" }
        );
        let settings = commands.iter().find(|c| c.id == "app.settings").unwrap();
        assert_eq!(
            settings.bindings[0].strokes[0].meta,
            cfg!(target_os = "macos")
        );
        assert_eq!(
            settings.bindings[0].strokes[0].ctrl,
            !cfg!(target_os = "macos")
        );
    }
    #[test]
    fn registry_defaults_remain_uncustomized() {
        assert!(build_interactive_commands().iter().all(|c| !c.customized));
    }
}
