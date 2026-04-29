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
    vec![
        command(
            "app.command_palette",
            "App",
            "Open Command Palette",
            vec![mod_binding("p", true)],
            None,
        ),
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
            "panel.navigate_source_root",
            "Navigation",
            "Go To Source Root",
            vec![keybinding("/")],
            Some("NavigateSourceRoot"),
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
            vec![keybinding("ArrowRight")],
            Some("OpenEntry"),
        ),
        command(
            "panel.navigate_up",
            "Navigation",
            "Go Up",
            vec![keybinding("Backspace"), keybinding("ArrowLeft")],
            Some("NavigateUp"),
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
            vec![keybinding("g")],
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
    ]
}
