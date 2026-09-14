use crate::ws::protocol::{InteractiveCommandSchema, KeyBindingSchema};

fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

fn keybinding(key: &str) -> KeyBindingSchema {
    KeyBindingSchema {
        key: key.to_string(),
        shift: false,
        ctrl: false,
        alt: false,
        meta: false,
        modes: Vec::new(),
    }
}

fn keybinding_with_modifiers(
    key: &str,
    shift: bool,
    ctrl: bool,
    alt: bool,
    meta: bool,
) -> KeyBindingSchema {
    KeyBindingSchema {
        key: key.to_string(),
        shift,
        ctrl,
        alt,
        meta,
        modes: Vec::new(),
    }
}

fn toolbar_binding(function_key: &str, mac_key: &str) -> KeyBindingSchema {
    if is_macos() {
        keybinding(mac_key)
    } else {
        keybinding(function_key)
    }
}

fn mod_binding(key: &str, shift: bool) -> KeyBindingSchema {
    if is_macos() {
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
        id: id.to_string(),
        category: category.to_string(),
        title: title.to_string(),
        bindings: bindings.clone(),
        default_bindings: bindings,
        intent_name: intent_name.map(str::to_string),
    }
}

pub(crate) fn build_interactive_commands() -> Vec<InteractiveCommandSchema> {
    let mut commands = vec![
        command(
            "app.command_palette",
            "App",
            "Open Command Palette",
            vec![mod_binding("p", true)],
            None,
        ),
        command("app.about", "App", "About fbro", vec![], None),
        command(
            "app.search",
            "App",
            "Search",
            vec![mod_binding("f", false)],
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
            vec![toolbar_binding("F2", "2")],
            None,
        ),
        command(
            "panel.pass_to_bro",
            "File",
            "Pass to Bro",
            vec![toolbar_binding("F1", "1")],
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
            vec![toolbar_binding("F3", "3")],
            None,
        ),
        command(
            "panel.viewer.embed",
            "View",
            "Toggle Embedded Viewer",
            vec![if is_macos() {
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
            vec![toolbar_binding("F4", "4")],
            None,
        ),
        command(
            "file.touch",
            "File",
            "New File",
            vec![if is_macos() {
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
            vec![toolbar_binding("F5", "5")],
            None,
        ),
        command(
            "file.copy_external",
            "File Operation",
            "Copy To Host Clipboard",
            vec![mod_binding("c", false)],
            Some("WriteExternalClipboard"),
        ),
        command(
            "file.move",
            "File Operation",
            "Move",
            vec![toolbar_binding("F6", "6")],
            None,
        ),
        command(
            "file.cut_external",
            "File Operation",
            "Cut To Host Clipboard",
            if is_macos() {
                vec![]
            } else {
                vec![mod_binding("x", false)]
            },
            Some("WriteExternalClipboard"),
        ),
        command(
            "file.paste_external",
            "File Operation",
            "Paste External Clipboard",
            vec![mod_binding("v", false)],
            Some("PasteExternalClipboard"),
        ),
        command(
            "file.paste_external_move",
            "File Operation",
            "Paste External Clipboard As Move",
            if is_macos() {
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
            vec![toolbar_binding("F7", "7")],
            None,
        ),
        command(
            "file.delete",
            "File Operation",
            "Delete",
            vec![toolbar_binding("F8", "8")],
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
            vec![toolbar_binding("F9", "9")],
            Some("OpenTerminal"),
        ),
        command(
            "app.quit",
            "App",
            "Quit",
            vec![toolbar_binding("F10", "0")],
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
            "Toggle Selection Or Directory Size",
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
            "panel.selection.select_all",
            "Selection",
            "Select All",
            vec![mod_binding("a", false)],
            Some("SelectAll"),
        ),
        command(
            "panel.selection.clear",
            "Selection",
            "Clear Selection",
            vec![mod_binding("a", true)],
            Some("ClearSelection"),
        ),
        command(
            "panel.selection.invert",
            "Selection",
            "Invert Selection",
            vec![mod_binding("i", false)],
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
            vec![mod_binding("g", false)],
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
            "panel.bookmarks.open",
            "Panel",
            "Bookmarks…",
            vec![mod_binding("d", false)],
            None,
        ),
        command(
            "panel.bookmarks.browse",
            "Panel",
            "Browse Bookmarks",
            vec![mod_binding("d", true)],
            None,
        ),
        command(
            "app.settings",
            "App",
            "Settings…",
            vec![keybinding_with_modifiers(
                ",",
                false,
                !is_macos(),
                false,
                is_macos(),
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
            vec![mod_binding("t", false)],
            None,
        ),
        command(
            "panel.tab.close",
            "Panel",
            "Close Tab",
            vec![mod_binding("w", false)],
            None,
        ),
        command(
            "panel.tab.next",
            "Panel",
            "Next Tab",
            vec![keybinding_with_modifiers("k", true, false, false, false)],
            None,
        ),
        command(
            "panel.tab.prev",
            "Panel",
            "Previous Tab",
            vec![keybinding_with_modifiers("j", true, false, false, false)],
            None,
        ),
    ];
    commands.push(command(
        "file.quick_edit",
        "File",
        "Quick Edit",
        vec![if is_macos() {
            keybinding_with_modifiers("4", false, false, false, true)
        } else {
            keybinding_with_modifiers("F4", false, true, false, false)
        }],
        Some("QuickEditFile"),
    ));
    commands
}

#[cfg(test)]
mod tests {
    use super::build_interactive_commands;

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
}
