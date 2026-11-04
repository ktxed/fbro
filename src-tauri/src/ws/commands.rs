use crate::ws::protocol::{InteractiveCommandSchema, KeyBindingSchema, KeyStrokeSchema};

/// Platform whose default bindings the registry describes. The running app
/// always uses [`CommandPlatform::current`]; the others exist so the user-guide
/// site can publish every platform's defaults from one build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandPlatform {
    Linux,
    MacOs,
    Windows,
}

impl CommandPlatform {
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

fn keybinding(key: &str) -> KeyBindingSchema {
    keybinding_with_modifiers(key, false, false, false, false)
}

fn keybinding_with_modifiers(
    key: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
) -> KeyBindingSchema {
    KeyBindingSchema {
        strokes: vec![KeyStrokeSchema {
            key: key.to_string(),
            shift,
            ctrl,
            alt,
            meta,
        }],
        modes: Vec::new(),
    }
}

fn toolbar_binding(mac: bool, function_key: &str, mac_key: &str) -> KeyBindingSchema {
    if mac {
        keybinding(mac_key)
    } else {
        keybinding(function_key)
    }
}

fn mod_binding(mac: bool, key: &str, shift: bool) -> KeyBindingSchema {
    if mac {
        keybinding_with_modifiers(key, shift, false, false, true)
    } else {
        keybinding_with_modifiers(key, shift, true, false, false)
    }
}

fn list_only(mut kb: KeyBindingSchema) -> KeyBindingSchema {
    kb.modes = vec!["list".to_string()];
    kb
}

fn grid_only(mut kb: KeyBindingSchema) -> KeyBindingSchema {
    kb.modes = vec!["grid".to_string()];
    kb
}

fn command(
    id: &str,
    category: &str,
    title: &str,
    bindings: Vec<KeyBindingSchema>,
    intent_name: Option<&str>,
) -> InteractiveCommandSchema {
    InteractiveCommandSchema {
        customized: false,
        id: id.to_string(),
        category: category.to_string(),
        title: title.to_string(),
        bindings: bindings.clone(),
        default_bindings: bindings,
        intent_name: intent_name.map(str::to_string),
    }
}

pub(crate) fn build_interactive_commands() -> Vec<InteractiveCommandSchema> {
    build_interactive_commands_for(CommandPlatform::current())
}

pub(crate) fn build_interactive_commands_for(
    platform: CommandPlatform,
) -> Vec<InteractiveCommandSchema> {
    let mac = platform == CommandPlatform::MacOs;
    let mut commands = vec![
        command(
            "app.command_palette",
            "App",
            "Open Command Palette",
            vec![mod_binding(mac, "p", true)],
            None,
        ),
        // `?` is the conventional help key; an unshifted symbol binding also
        // accepts the Shift-produced character on layouts where `?` needs Shift.
        command(
            "app.cheat_sheet",
            "App",
            "Keyboard Cheat Sheet",
            vec![keybinding("?")],
            None,
        ),
        command("app.about", "App", "About fbro", vec![], None),
        command(
            "app.toggle_button_bar",
            "App",
            "Toggle Button Bar",
            vec![],
            None,
        ),
        command(
            "app.search",
            "App",
            "Search",
            vec![mod_binding(mac, "f", false)],
            None,
        ),
        command(
            "app.open_jobs",
            "App",
            "Open Jobs",
            vec![keybinding("j")],
            None,
        ),
        command(
            "app.install_fbro_command",
            "App",
            "Install fbro Shell Command",
            vec![],
            None,
        ),
        command(
            "panel.navigate_source_root",
            "Navigation",
            "Go To Source Root",
            vec![keybinding("/")],
            Some("NavigateSourceRoot"),
        ),
        command(
            "panel.refresh",
            "Navigation",
            "Refresh",
            vec![list_only(keybinding("r"))],
            Some("Refresh"),
        ),
        command(
            "file.info",
            "File",
            "Inspect Item",
            vec![keybinding("i")],
            None,
        ),
        command(
            "file.rename",
            "File",
            "Rename",
            vec![toolbar_binding(mac, "F2", "2")],
            None,
        ),
        command(
            "panel.pass_to_bro",
            "File",
            "Pass to Bro",
            vec![toolbar_binding(mac, "F1", "1")],
            Some("ListBros"),
        ),
        command(
            "file.compare",
            "File",
            "Compare",
            vec![],
            Some("CompareFiles"),
        ),
        command(
            "file.view",
            "File",
            "View",
            vec![toolbar_binding(mac, "F3", "3")],
            None,
        ),
        command(
            "panel.viewer.embed",
            "View",
            "Toggle Embedded Viewer",
            vec![if mac {
                keybinding_with_modifiers("3", false, false, false, true) // Cmd+3 (F-keys need `fn` on Mac)
            } else {
                keybinding_with_modifiers("F3", false, true, false, false) // Ctrl+F3
            }],
            None,
        ),
        command(
            "file.edit",
            "File",
            "Edit",
            vec![toolbar_binding(mac, "F4", "4")],
            None,
        ),
        command(
            "file.touch",
            "File",
            "New File",
            vec![if mac {
                keybinding_with_modifiers("4", true, false, false, false) // Shift+4
            } else {
                keybinding_with_modifiers("F4", true, false, false, false) // Shift+F4
            }],
            None,
        ),
        command(
            "file.copy",
            "File Operation",
            "Copy",
            vec![toolbar_binding(mac, "F5", "5")],
            None,
        ),
        command(
            "file.copy_external",
            "File Operation",
            "Copy To Host Clipboard",
            vec![mod_binding(mac, "c", false)],
            Some("WriteExternalClipboard"),
        ),
        command(
            "file.move",
            "File Operation",
            "Move",
            vec![toolbar_binding(mac, "F6", "6")],
            None,
        ),
        command(
            "file.cut_external",
            "File Operation",
            "Cut To Host Clipboard",
            if mac {
                vec![]
            } else {
                vec![mod_binding(mac, "x", false)]
            },
            Some("WriteExternalClipboard"),
        ),
        command(
            "file.paste_external",
            "File Operation",
            "Paste External Clipboard",
            vec![mod_binding(mac, "v", false)],
            Some("PasteExternalClipboard"),
        ),
        command(
            "file.paste_external_move",
            "File Operation",
            "Paste External Clipboard As Move",
            if mac {
                vec![keybinding_with_modifiers("v", false, false, true, true)]
            } else {
                vec![]
            },
            Some("PasteExternalClipboard"),
        ),
        command(
            "file.mkdir",
            "File",
            "Create Directory",
            vec![toolbar_binding(mac, "F7", "7")],
            None,
        ),
        command(
            "file.delete",
            "File Operation",
            "Delete",
            vec![toolbar_binding(mac, "F8", "8")],
            None,
        ),
        command(
            "file.set_permissions",
            "File",
            "Set Permissions…",
            vec![],
            None,
        ),
        command(
            "app.open_terminal",
            "App",
            "Open Terminal",
            vec![toolbar_binding(mac, "F9", "9")],
            Some("OpenTerminal"),
        ),
        command(
            "app.quit",
            "App",
            "Quit",
            vec![toolbar_binding(mac, "F10", "0")],
            Some("Quit"),
        ),
        command(
            "panel.cursor.up",
            "Navigation",
            "Move Cursor Up",
            vec![keybinding("ArrowUp")],
            Some("MoveCursor"),
        ),
        command(
            "panel.cursor.down",
            "Navigation",
            "Move Cursor Down",
            vec![keybinding("ArrowDown")],
            Some("MoveCursor"),
        ),
        command(
            "panel.cursor.first",
            "Navigation",
            "Move Cursor To First Entry",
            vec![
                keybinding("Home"),
                keybinding_with_modifiers("ArrowUp", false, false, false, true),
            ],
            Some("MoveCursorToEdge"),
        ),
        command(
            "panel.cursor.last",
            "Navigation",
            "Move Cursor To Last Entry",
            vec![
                keybinding("End"),
                keybinding_with_modifiers("ArrowDown", false, false, false, true),
            ],
            Some("MoveCursorToEdge"),
        ),
        command(
            "panel.selection.extend_up",
            "Selection",
            "Extend Selection Up",
            vec![keybinding_with_modifiers(
                "ArrowUp", true, false, false, false,
            )],
            Some("MoveCursorWithSelect"),
        ),
        command(
            "panel.selection.extend_down",
            "Selection",
            "Extend Selection Down",
            vec![keybinding_with_modifiers(
                "ArrowDown",
                true,
                false,
                false,
                false,
            )],
            Some("MoveCursorWithSelect"),
        ),
        command(
            "panel.selection.extend_first",
            "Selection",
            "Extend Selection To First Entry",
            vec![keybinding_with_modifiers("Home", true, false, false, false)],
            Some("MoveCursorWithSelect"),
        ),
        command(
            "panel.selection.extend_last",
            "Selection",
            "Extend Selection To Last Entry",
            vec![keybinding_with_modifiers("End", true, false, false, false)],
            Some("MoveCursorWithSelect"),
        ),
        command(
            "panel.selection.toggle_or_size",
            "Selection",
            "Toggle Selection and Calculate Folder Size",
            vec![keybinding(" ")],
            None,
        ),
        command(
            "panel.selection.calculate_sizes",
            "Selection",
            "Toggle Selected Directory Sizes",
            vec![keybinding_with_modifiers(" ", true, false, false, false)],
            None,
        ),
        command(
            "panel.selection.select_by_mask",
            "Selection",
            "Select Files",
            vec![keybinding("+")],
            None,
        ),
        command(
            "panel.selection.deselect_by_mask",
            "Selection",
            "Deselect Files",
            vec![keybinding("-")],
            None,
        ),
        command(
            "panel.selection.select_all",
            "Selection",
            "Select All",
            vec![mod_binding(mac, "a", false)],
            Some("SelectAll"),
        ),
        command(
            "panel.selection.clear",
            "Selection",
            "Clear Selection",
            vec![mod_binding(mac, "a", true)],
            Some("ClearSelection"),
        ),
        command(
            "panel.selection.invert",
            "Selection",
            "Invert Selection",
            vec![mod_binding(mac, "i", false)],
            Some("InvertSelection"),
        ),
        command(
            "panel.cursor.page_up",
            "Navigation",
            "Page Up",
            vec![keybinding("PageUp")],
            Some("MoveCursorBy"),
        ),
        command(
            "panel.cursor.page_down",
            "Navigation",
            "Page Down",
            vec![keybinding("PageDown")],
            Some("MoveCursorBy"),
        ),
        command(
            "panel.open",
            "Navigation",
            "Open Entry",
            vec![keybinding("Enter")],
            None,
        ),
        command(
            "panel.open_entry",
            "Navigation",
            "Open Entry",
            vec![list_only(keybinding("ArrowRight"))],
            Some("OpenEntry"),
        ),
        command(
            "panel.navigate_up",
            "Navigation",
            "Go Up",
            vec![keybinding("Backspace"), list_only(keybinding("ArrowLeft"))],
            Some("NavigateUp"),
        ),
        command(
            "panel.cursor.left",
            "Navigation",
            "Cursor Left",
            vec![grid_only(keybinding("ArrowLeft"))],
            None,
        ),
        command(
            "panel.cursor.right",
            "Navigation",
            "Cursor Right",
            vec![grid_only(keybinding("ArrowRight"))],
            None,
        ),
        command(
            "panel.selection.extend_left",
            "Selection",
            "Extend Selection Left",
            vec![grid_only(keybinding_with_modifiers(
                "ArrowLeft",
                true,
                false,
                false,
                false,
            ))],
            None,
        ),
        command(
            "panel.selection.extend_right",
            "Selection",
            "Extend Selection Right",
            vec![grid_only(keybinding_with_modifiers(
                "ArrowRight",
                true,
                false,
                false,
                false,
            ))],
            None,
        ),
        command(
            "panel.navigate_home",
            "Navigation",
            "Go Home",
            vec![keybinding("~")],
            Some("NavigateHome"),
        ),
        command(
            "panel.navigate_to_path",
            "Navigation",
            "Go To Path",
            vec![mod_binding(mac, "g", false)],
            None,
        ),
        command(
            "panel.history.open",
            "Navigation",
            "Browse History",
            vec![keybinding("h")],
            None,
        ),
        command(
            "panel.history.back",
            "Navigation",
            "Go Back",
            vec![keybinding("[")],
            None,
        ),
        command(
            "panel.history.forward",
            "Navigation",
            "Go Forward",
            vec![keybinding("]")],
            None,
        ),
        command(
            "panel.focus.switch",
            "Navigation",
            "Switch Focus",
            vec![keybinding("Tab")],
            Some("SwitchFocus"),
        ),
        command(
            "panel.source.next",
            "Navigation",
            "Next Source",
            vec![keybinding_with_modifiers("Tab", false, true, false, false)],
            None,
        ),
        command(
            "panel.source.previous",
            "Navigation",
            "Previous Source",
            vec![
                keybinding_with_modifiers("Tab", true, false, false, false),
                keybinding_with_modifiers("Tab", true, true, false, false),
            ],
            None,
        ),
        command(
            "panel.filter.open",
            "Navigation",
            "Open Filter",
            vec![keybinding("f")],
            None,
        ),
        command(
            "panel.path.sync",
            "Navigation",
            "Sync Other Panel To This Path",
            vec![keybinding("s")],
            None,
        ),
        command(
            "panel.context_menu",
            "Navigation",
            "Open Context Menu",
            vec![keybinding("c")],
            None,
        ),
        command(
            "panel.toggle_hidden",
            "Navigation",
            "Toggle Hidden Files",
            vec![keybinding(".")],
            Some("ToggleHidden"),
        ),
        command(
            "panel.toggle_view_mode",
            "Panel",
            "Toggle Grid View",
            vec![keybinding("g")],
            None,
        ),
        command(
            "panel.sort.open",
            "Panel",
            "Sort Panel",
            vec![keybinding_with_modifiers("s", true, false, false, false)],
            None,
        ),
        command(
            "panel.git.actions",
            "Panel",
            "Git Repository…",
            vec![keybinding_with_modifiers("g", true, false, false, false)],
            None,
        ),
        command(
            "git.init",
            "Git",
            "Initialize Git repository…",
            vec![],
            None,
        ),
        command(
            "panel.bookmarks.open",
            "Panel",
            "Bookmarks…",
            vec![mod_binding(mac, "d", false)],
            None,
        ),
        command(
            "panel.bookmarks.browse",
            "Panel",
            "Browse Bookmarks",
            vec![mod_binding(mac, "d", true)],
            None,
        ),
        command(
            "app.settings",
            "App",
            "Settings…",
            vec![keybinding_with_modifiers(
                ",",
                false,
                !mac,
                false,
                mac,
            )],
            None,
        ),
        command(
            "panel.toggle_embedded_terminal",
            "Panel",
            "Toggle Embedded Terminal",
            vec![keybinding("t")],
            None,
        ),
        command(
            "panel.tab.new",
            "Panel",
            "New Tab",
            vec![mod_binding(mac, "t", false)],
            None,
        ),
        command(
            "panel.tab.close",
            "Panel",
            "Close Tab",
            vec![mod_binding(mac, "w", false)],
            None,
        ),
        command(
            "panel.tab.next",
            "Panel",
            "Next Tab",
            vec![keybinding_with_modifiers("j", true, false, false, false)],
            None,
        ),
        command(
            "panel.tab.prev",
            "Panel",
            "Previous Tab",
            vec![keybinding_with_modifiers("k", true, false, false, false)],
            None,
        ),
    ];
    commands.push(command(
        "file.quick_edit",
        "File",
        "Quick Edit",
        vec![if mac {
            keybinding_with_modifiers("4", false, false, false, true)
        } else {
            keybinding_with_modifiers("F4", false, true, false, false)
        }],
        Some("QuickEditFile"),
    ));
    // Built-in editor tab for an existing file: context menu and palette only.
    commands.push(command("file.edit_builtin", "File", "Edit in Editor Tab", vec![], Some("OpenEditorTab")));
    // New Note. Cmd+Shift+4 is the macOS screenshot shortcut and never reaches
    // the app, so macOS uses Cmd+Alt+4.
    commands.push(command(
        "notes.new",
        "Notes",
        "New Note",
        vec![if mac {
            keybinding_with_modifiers("4", false, false, true, true)
        } else {
            keybinding_with_modifiers("F4", true, true, false, false)
        }],
        Some("CreateNote"),
    ));
    commands.push(command("notes.open", "Notes", "Open Notes", vec![], Some("OpenNotes")));
    if platform != CommandPlatform::Windows {
        commands.push(command(
            "app.eject_source",
            "App",
            "Eject Removable Source",
            vec![],
            None,
        ));
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::{build_interactive_commands, build_interactive_commands_for, CommandPlatform};

    /// Default bindings for every platform, as published by the user-guide
    /// site (`site/src/data/commands.json`).
    fn site_commands_json() -> String {
        let platform = |platform| {
            build_interactive_commands_for(platform)
                .into_iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "category": c.category,
                        "title": c.title,
                        "bindings": c.default_bindings,
                    })
                })
                .collect::<Vec<_>>()
        };
        let data = serde_json::json!({
            "generatedBy": "cargo test site_command_data_is_current (src-tauri/src/ws/commands.rs)",
            "platforms": {
                "linux": platform(CommandPlatform::Linux),
                "macos": platform(CommandPlatform::MacOs),
                "windows": platform(CommandPlatform::Windows),
            },
        });
        serde_json::to_string_pretty(&data).unwrap() + "\n"
    }

    /// The site's keyboard reference reads a committed copy of the registry so
    /// the Pages build needs no Rust toolchain. Regenerate it with
    /// `FBRO_UPDATE_SITE_DATA=1 cargo test site_command_data_is_current`.
    #[test]
    fn site_command_data_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../site/src/data/commands.json");
        let expected = site_commands_json();
        if std::env::var_os("FBRO_UPDATE_SITE_DATA").is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &expected).unwrap();
            return;
        }
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            actual == expected,
            "{} is stale; run FBRO_UPDATE_SITE_DATA=1 cargo test site_command_data_is_current",
            path.display()
        );
    }

    #[test]
    fn platform_variants_differ_only_where_expected() {
        let find = |platform, id: &str| {
            build_interactive_commands_for(platform)
                .into_iter()
                .find(|c| c.id == id)
                .unwrap_or_else(|| panic!("{id}"))
        };
        let linux = find(CommandPlatform::Linux, "file.copy");
        assert_eq!(linux.bindings[0].strokes[0].key, "F5");
        let mac = find(CommandPlatform::MacOs, "file.copy");
        assert_eq!(mac.bindings[0].strokes[0].key, "5");
        let palette = find(CommandPlatform::MacOs, "app.command_palette");
        assert!(palette.bindings[0].strokes[0].meta);
        let has_eject = |platform| {
            build_interactive_commands_for(platform)
                .iter()
                .any(|c| c.id == "app.eject_source")
        };
        assert!(has_eject(CommandPlatform::Linux));
        assert!(!has_eject(CommandPlatform::Windows));
    }

    #[test]
    fn about_is_available_without_reserving_a_shortcut() {
        let commands = build_interactive_commands();
        let about = commands
            .iter()
            .find(|command| command.id == "app.about")
            .expect("About command should be registered");
        assert_eq!(about.category, "App");
        assert_eq!(about.title, "About fbro");
        assert!(about.bindings.is_empty());
    }

    #[test]
    fn cheat_sheet_binding_is_question_mark_and_not_shared() {
        let commands = build_interactive_commands();
        let sheet = commands.iter().find(|c| c.id == "app.cheat_sheet").expect("cheat sheet command");
        assert_eq!(sheet.category, "App");
        assert_eq!(sheet.bindings.len(), 1);
        let stroke = &sheet.bindings[0].strokes[0];
        assert_eq!(stroke.key, "?");
        assert!(!stroke.shift && !stroke.ctrl && !stroke.alt && !stroke.meta);
        for other in commands.iter().filter(|c| c.id != "app.cheat_sheet") {
            assert!(
                !other.bindings.iter().any(|b| b.strokes.iter().any(|s| s.key == "?")),
                "{} shares the ? binding",
                other.id
            );
        }
    }

    #[test]
    fn new_note_binding_is_not_shared() {
        let commands = build_interactive_commands();
        let find = |id: &str| commands.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("{id}"));
        let edit = find("notes.new");
        let stroke = |c: &super::InteractiveCommandSchema| {
            c.bindings.iter().map(|b| serde_json::to_string(&b.strokes).unwrap()).collect::<Vec<_>>()
        };
        let edit_strokes = stroke(edit);
        assert_eq!(edit_strokes.len(), 1);
        for other in commands.iter().filter(|c| c.id != "notes.new") {
            assert!(!stroke(other).iter().any(|s| edit_strokes.contains(s)), "{} shares the binding", other.id);
        }
        assert!(find("file.edit_builtin").bindings.is_empty());
        assert!(find("notes.open").bindings.is_empty());
    }
}
