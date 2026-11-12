# Settings

## Overview

fbro has a persistent settings system backed by a typed state tree on the BE.
Settings are exposed through a tabbed modal dialog triggered by **Ctrl+,** (or **Cmd+,** on macOS).

## Design decisions

| Concern | Choice |
|---|---|
| Modal shape | Vertical icon-and-label tab list (left) + content area (right) |
| Trigger | Command `app.settings`, default binding **Ctrl+,** / **Cmd+,**, also reachable from the command palette |
| Apply model | Live-apply — no Save button. Each control dispatches its own typed intent on change; BE pushes `StatePatch` |
| Wire surface | Per-setting intents (typed). One fat `UpdateSetting { path, value }` intent was rejected. |
| Persistence | General settings → `settings.json`; local location settings → `fbro.sqlite3` |
| Defaults | Hardcoded in BE `Default` impls; missing or corrupt file → defaults silently, no error |
| Validation | Authoritative on BE. FE controls reflect `state.settings.*` after the BE confirms — no optimistic updates |
| Backdrop click | Does **not** close — settings is a considered action; explicit close (Esc or Close button) required |

## Architecture

```mermaid
flowchart LR
    subgraph FE[Frontend]
        modal[SettingsModal] --> tabs[Tab list]
        modal --> general[SettingsGeneralTab]
        general --> control[Control onChange]
        reducer[Reducer] --> state[state.settings]
    end

    subgraph BE[Backend]
        intent[SetGeneralOption intent] --> mutate[Mutate AppState.settings]
        mutate --> persist[Atomically write settings.json]
        persist --> patch[Create StatePatch]
    end

    control -->|client.sendIntent over WebSocket| intent
    patch -->|broadcast over WebSocket| reducer
```

## State snapshot

`settings` is included in the `StateSnapshot` envelope sent to every client on connect, so clients always have the current settings without a separate request.

`settings.general.supportsManagedThumbnails` is a read-only backend capability,
true on Linux and false for macOS/Windows native providers. It gates the Display
generation/cache controls and the Jobs modal's Thumbnails tab. It is not persisted
or writable through `SetGeneralOption`. Thumbnail streaming remains configurable
on all platforms. Settings patches merge with the snapshot and preserve this flag.

`settings.general.supportsOsNotifications` is a read-only backend capability,
true on Linux, in the packaged macOS `fbro.app`, and in the installed Windows
application whose Start Menu shortcut carries `dev.fbro.app`. It is false for
bare macOS and Windows development/test executables. It describes adapter
availability, independently of macOS user authorization and of the Windows
notification toggles. General settings shows **Show background OS
notifications** enabled only when this capability is true. The section is also
visible when `supportsOsNotificationSettings` is true (macOS and Windows,
including development builds), with a disabled delivery checkbox when native
delivery is unavailable. The macOS explanation says delivery requires the signed
fbro.app. The Windows explanation says delivery requires the installed fbro
application. `general.osNotificationPlatform` is the read-only host
(`linux`, `macos`, `windows`, or `none`) that selects this copy. It is projected
on the settings snapshot, omitted from later settings patches, and preserved by
the snapshot merge. It is not persisted or writable through `SetGeneralOption`.
The persistent
`general.osNotificationsEnabled` preference defaults to true; changing it updates
the native delivery policy immediately and then projects the authoritative backend
value. Unsupported platforms retain the preference without native delivery.
On macOS, enabling while a native window is focused also prepares the system
authorization prompt on the UI thread after releasing shared state. The callback
rechecks focus and the preference before requesting permission. System denial
does not change the persisted preference; permission is controlled separately by
System Settings → Notifications → fbro. See [notifications.md](notifications.md).

`general.supportsOsNotificationSettings` is a read-only PAL capability, true on
macOS and Windows, including bare development executables, and false by default
on other platforms. It is neither persisted nor writable. When true, General →
Notifications offers host-specific help. On macOS it explains that permission
must be enabled in **System Settings → Notifications → fbro** and that denial
prevents another prompt. On Windows it explains that banners are controlled in
**Settings → System → Notifications → fbro**, that Windows does not ask for
permission, and that Focus assist can hide banners. **Open notification settings** sends
`OpenOsNotificationSettings { requestId }` to the backend. The button is reachable
with Tab and native Enter/Space activation, retains its focus while opening,
guards duplicate requests and displays “Opening…”, and shows correlated failures
inline. Disconnect clears the pending request and reports an inline error.
The checkbox remains fbro's independent delivery preference; the notice does not
assert current system authorization. Opening the system pane never changes a
permission or the app preference.
The explanatory paragraphs live in an info tooltip beside the Notifications
heading, leaving the controls visible in the compact section. Its labelled info
button opens on hover, focus, or native activation. Escape dismisses the tooltip
before Settings, retains button focus, and subsequent Tab/blur dismisses it.
Opening the installed app's system pane does not initialize Notification Center
in the development executable. Development guidance is included in the same
tooltip. The preference is preserved across development
and packaged runs; the disabled control never claims delivery is available in dev.

```json
{
  "type": "StateSnapshot",
  "domains": {
    "settings": {
      "general": {
        "showHiddenDefault": false
      }
    }
  }
}
```

## System appearance

Settings → Display → Color Scheme has a **Match system appearance** checkbox
above the **Scheme** select. Its writable preference, `followSystemColorScheme`,
persists as `general.follow_system_color_scheme`; older settings files default
to false. The backend detects the OS preference through the platform layer
([platform.md](platform.md) §4.1) and projects two read-only fields:

| Field | Values | Meaning |
|---|---|---|
| `general.systemColorScheme` | `"dark"`, `"light"`, `null` | Detected OS preference; `null` while unavailable (no portal/session bus, unsupported platform) |
| `general.activeColorScheme` | `"fbro-dark"`, `"daylight"` | Scheme every client renders: the OS-mapped scheme when following and detection is available, otherwise `colorScheme` |

Both appear in the snapshot, every settings patch, and the viewer lightweight
patch, are not persisted, and are rejected by `SetGeneralOption`. An OS change
broadcasts `{ systemColorScheme, activeColorScheme }` to GUI and viewer clients,
serialized with other settings writes. New viewer and diff windows take their
native background and `colorScheme` URL parameter from the active scheme.

While following with detection available, the Scheme select shows the preserved
manual choice but is disabled (and leaves the Tab order); the hint reads
"Following system: <label>". If detection is unavailable, the select stays
enabled and the hint reads "System appearance unavailable; using <label>.".
The checkbox keeps focus while the backend confirms a change.

## Custom color schemes

Users can create their own schemes from any scheme as a template and edit them
in **Settings → Color Scheme**. That tab exists only in a Settings session that
was opened with **Customize…** in Display → Color Scheme (`OPEN_SETTINGS` with
`showColorSchemeEditor`, stored in `pendingSettings`). `app.settings`, the
command palette, and the Bros shortcut never show it; closing Settings drops it.

**Files.** One TOML file per scheme in `<config_dir>/themes/<slug>.toml`
(`crate::config::config_dir()`, so `~/.config/fbro/themes/` on Linux). The id is
`custom:<slug>`; the file stem must match `[a-z0-9][a-z0-9_-]*`.

```toml
version = 1
name = "Midnight"
description = "Based on fbro Dark"
mode = "dark"          # dark | light: native controls, mermaid, window background
base = "fbro-dark"     # built-in that supplies any token missing below

[colors]               # CSS token names without "--"
bg = "#0d0d0d"
bg-selected = "rgba(220, 170, 20, 0.30)"

[terminal]             # omitted keys come from the base
background = "#1e1e1e"
foreground = "#dddddd"
cursor = "#d6e8ff"
selection_background = "#2d228a"
```

New schemes are written complete (every token in `src/theme/tokens.ts`, copied
from the rendered template); a hand-written file may omit tokens, which then
cascade from `base` through `data-color-scheme-base` in `styles.css`.

**Bundled schemes.** fbro ships Matrix, Magenta, Windows XP, IntelliJ Dark/Light,
CDE Motif and Solarized Dark/Light as `src-tauri/themes/*.toml`, embedded with
`include_str!` and loaded ahead of the user's files. They use the same format,
appear in `colorSchemes.items` with `bundled: true` and id `bundled:<slug>`, are
read-only (`UpdateColorScheme`/`DeleteColorScheme` fail with `BAD_COLOR_SCHEME`),
and are never written to the themes folder, so a user file with the same stem
(`custom:matrix`) is a separate scheme that cannot shadow one. Duplicate them to
edit. To add one: drop a complete TOML (same token set as the others) into
`src-tauri/themes/` and list it in `BUNDLED_SOURCES`; a unit test checks that every
bundled scheme is valid, uniquely named, and defines the same tokens.

**Validation (backend, `themes.rs`).** Token names match `[a-z][a-z0-9-]*`; values
are `#rgb[a]`, `#rrggbb[aa]`, `rgb()` or `rgba()` only, so a theme can never inject
CSS into the generated stylesheet. `mode` is `dark`/`light`, `base` a built-in.
A file that fails to parse or validate is listed in `colorSchemes.errors` with
its message, cannot be selected, and is never rewritten. Unknown color keys in a
valid file are preserved by updates.

**Projection.** `general.colorSchemes = { dir, items, errors }` is read-only; it is
in the snapshot, every settings patch, and the viewer lightweight patch, and is
rejected by `SetGeneralOption`. Each item carries `id, name, description, mode,
base, colors` and a terminal palette already resolved against the base.
`activeColorScheme` may be a built-in id or a loaded `custom:<slug>` / `bundled:<slug>`. A
`colorScheme` that names a custom scheme whose file is missing or invalid renders
as `fbro-dark`; the preference is kept, so restoring the file restores the scheme.
Following the system still maps to the built-ins. `SetGeneralOption colorScheme`
accepts `custom:<slug>` or `bundled:<slug>` only while that scheme is loaded.

**Import.** A watcher on the themes folder (`notify`, debounced 300 ms) reloads
the catalog and broadcasts it, so copying a `.toml` file in imports it and editing
one restyles open windows. The editor's own writes reload to an identical catalog
and publish nothing. **Open themes folder** navigates the focused panel there.

**Intents** (`ws/intents/themes.rs`, serialized with settings writes; see
[proto.md](proto.md)): `CreateColorScheme`, `UpdateColorScheme`,
`DeleteColorScheme`. Deleting the manual scheme resets `colorScheme` to the
deleted scheme's base.

**Editor.** Left: filter box, a one-tab-stop listbox of tokens grouped as Surfaces,
Panel rows & cursor, File types, Borders & accent, Status & feedback, Toasts,
Terminal and Code (the editor tab's `--syntax-*` colours; arrows/Home/End), and a pane for the selected token with a swatch
picker (`react-colorful` with alpha) and a text field; Enter on a row moves focus
into the field, Escape returns it to the row. Right: a live preview of a
miniature fbro drawn with the draft colors; clicking an element selects its token
and the selected token's elements are outlined. Edits apply to the preview
immediately and save after 300 ms (live-apply, no Save button); when the edited
scheme is active the whole app and open viewers update from the backend patch.
Invalid values are shown but never sent. **Duplicate…** snapshots every token
from the rendered scheme (`getComputedStyle`) into a new file and is the only
way to start from a built-in, which is read-only. **Revert** restores the values
from when the scheme was selected; **Delete…** confirms inline. Escape closes the
open prompt, then returns from the editor pane to the list, then closes Settings.

## macOS file icon cache

Settings → Display shows a File icons section only when the read-only backend
`general.iconCacheDir` is nonempty (macOS). PNGs persist under
`$HOME/Library/Caches/fbro/icons/v1`. The keyboard-accessible Clear file icon
cache button sends `ClearIconCache { requestId }`, disables itself with
“Clearing…” until the correlated reply, and shows errors inline. Disconnection
releases the pending state. Backend success publishes “Cleared file icon cache”.
The authoritative `general.iconCacheRevision` patch refreshes panel icon URLs;
visible icons may immediately repopulate the cache. Neither field is persisted
in settings.json or writable with `SetGeneralOption`.

## Button-bar keys on macOS

Mac keyboards send brightness/media actions from the F-row unless Fn is held,
and Touch Bar Macs have no physical F-keys, so macOS defaults the button-bar
commands to the number keys 1–0. `general.systemFunctionKeysStandard` is a
read-only projection of the global `com.apple.keyboard.fnState` default ("Use
F1, F2, etc. keys as standard function keys"): `true`/`false` on macOS, `null`
elsewhere. It is read through the platform layer when the snapshot or a
settings patch is built, so a system change shows up after reconnecting or the
next settings change. It is neither persisted nor writable.

When it is `true`, `buttonBarKeys` is `numbers` and `functionKeysOfferAnswered`
is false, the GUI shows a one-time dialog (`function-keys-offer`). **Use
function keys** (initial focus) sends `buttonBarKeys = function`; **Keep number
keys** sends `functionKeysOfferAnswered = true`. Both buttons disable while the
answer is pending and the dialog closes when the settings patch lands. Escape
postpones the offer to the next launch without persisting anything.

Settings → Shortcuts shows a **Button bar keys** select whenever the projection
is non-null, with a hint describing the detected system setting.

## Intent: `SetGeneralOption`

**Direction:** FE → BE

```json
{
  "type": "Intent",
  "name": "SetGeneralOption",
  "args": {
    "key": "showHiddenDefault",
    "value": true
  }
}
```

Supported keys:

| Key | Type | Default | Effect |
|---|---|---|---|
| `showHiddenDefault` | `bool` | `false` | Whether new panels start with hidden files visible. Changing this also applies immediately to all open panels. |
| `showButtonBar` | `bool` | `true` | Show the classic bottom command bar. Settings → Display provides **Show bottom button bar**; the command palette provides **Toggle Button Bar** (`app.toggle_button_bar`, no default binding). Hiding the bar reclaims its space while preserving global shortcuts and zoom. The preference applies to all GUI clients connected to the backend. |
| `osNotificationsEnabled` | `bool` | `true` | Deliver selected operation notifications through the OS while no native fbro window is focused. Completion and cancellation require at least 30 seconds of execution; failures, pairing requests, transfer starts, and connection changes bypass that cutoff. The General settings control is capability-gated. See [notifications.md](notifications.md). |
| `rememberSortPerLocation` | `bool` | `false` | Remember and share sorting for each local filesystem location and for the Journal source. |
| `enableHeic2any` | `bool` | `false` | Enable local browser conversion of HEIC/HEIF previews in the embedded and detached viewers on Windows (including Windows 10) and Linux. The control is hidden on macOS, which retains native decoding. Existing image caps apply. Changes update open viewers through backend patches; conversion is preview-only and does not write files. |
| `colorScheme` | `string` | `"fbro-dark"` | Manual scheme: `fbro-dark`, `daylight`, or a loaded `custom:<slug>` or `bundled:<slug>` ([Custom color schemes](#custom-color-schemes)). Other values return `BAD_SETTING`. Preserved while following the system, so unfollowing restores it. |
| `followSystemColorScheme` | `bool` | `false` | Match the OS light/dark preference: dark → `fbro-dark`, light → `daylight`. Applies live, to all clients. While the preference is unavailable the manual `colorScheme` applies. See [System appearance](#system-appearance). |
| `zshEmacsKeymap` | `bool` | `false` | On macOS, start new interactive zsh terminals with `-o emacs`. The control is hidden and the setting has no effect on other platforms. Other shells and Quick Edit commands are unaffected. Existing terminal sessions keep their current keymap. |
| `notesFolder` | `string` | `""` | Folder where notes live. Empty means the default `<config dir>/scratch`; otherwise an absolute path, with a leading `~` expanded to the home folder. A path to an existing non-folder, or a relative path, returns `BAD_SETTING`. The folder is created on first use. Changing it swaps the note store immediately (New Note and the Scratch source-bar entry use it); existing notes are not moved, and open editor tabs keep editing their files. The read-only `notesFolderResolved` reports the folder in use. Settings → General → Notes commits the field on Enter or blur, shows rejections inline, and offers **Use default folder**. |
| `buttonBarKeys` | `string` | `"numbers"` | macOS only: default keys of the button-bar commands (Bro, Rename, View, Edit, Copy, Move, MkDir, Delete, Shell, Quit): `numbers` (1–0) or `function` (F1–F10). Other platforms always use F-keys and ignore it. Setting it also sets `functionKeysOfferAnswered`; a change rebuilds the registry defaults, bumps `commandRevision` and broadcasts a `commands` patch before the settings patch. See [Button-bar keys on macOS](#button-bar-keys-on-macos). |
| `functionKeysOfferAnswered` | `bool` | `false` | macOS only: the one-time offer to switch to function keys was answered and is not shown again. |
| `fedupDefaultIgnorePatterns` | `string` | `.git`, `.idea`, `.vscode`, `.venv`, `node_modules`, `__MACOSX`, `._*`, `.DS_Store`, `artifacts/e2e`, `dist/assets`, `target/debug`, `target/release`, `target/bundle`, `.m2` | Newline/comma separated wildcard name/path patterns Fedup Bro can use to skip files and folders before scanning. |

Unknown key or type mismatch → `Error { code: "BAD_SETTING", recoverable: true }`.

Button bar visibility persists as `general.show_button_bar` in `settings.json`.
Older settings files without the field default to visible. Both controls use
`SetGeneralOption`; visibility changes only after a backend settings patch.

OS notification preference persists as `general.os_notifications_enabled`.
Older settings files without this field default to enabled. A non-boolean value
is rejected; the capability `supportsOsNotifications` is neither persisted nor
writable through `SetGeneralOption`.

## StatePatch: settings domain

Detached viewer clients receive the current `general.enableHeic2any` in their
initial lightweight display-settings patch, as well as subsequent changes.
HEIC2ANY decoding errors from its older libheif can fall back to the bundled,
lazy-loaded `heic-to` browser decoder under the same toggle.

When a setting changes the BE broadcasts a patch to all connected clients:

```json
{
  "type": "StatePatch",
  "domain": "settings",
  "version": 0,
  "patch": {
    "general": {
      "showHiddenDefault": true
    }
  }
}
```

## Persistence

All persistent settings are owned and written by the BE. The FE sends typed
intents and updates its projection from `StatePatch`; it never writes these
files directly.

The configuration directory is:

| Platform | Directory |
|---|---|
| Linux and macOS | `$XDG_CONFIG_HOME/fbro`, falling back to `$HOME/.config/fbro` |
| Windows | `%USERPROFILE%\.config\fbro` |

### Storage map

| File | Contents | Format and write behavior |
|---|---|---|
| `settings.json` | `GeneralSettings`, saved panel view modes, modal sizes, and the internal location-sort retention timestamp | Pretty-printed JSON using Rust `snake_case` field names. The complete `Settings` value is rewritten after an accepted JSON-backed settings change. |
| `fbro.sqlite3` | Mutable structured application state. Currently the `settings_location` table stores sort column and direction by normalized local path or Journal source key. | SQLite. Settings tables use a `settings_` prefix. Sort changes use an UPSERT; reset and retention expiry use `DELETE`. `PRAGMA user_version` versions its schema. |
| `journal.sqlite3` | Audit events, including outcomes of manual location-sort resets and automatic expiry cleanup. | Append-only SQLite `journal_events`; ordinary setting changes are not journaled. It does not store current setting values. |

`session.toml` and `bookmarks.toml` share the configuration directory but are
not settings stores. `session.toml` holds restorable window/panel/tab state;
settings such as `remember_window_state` and `persist_panel_tabs` decide which
of that state is written. `bookmarks.toml` owns the bookmark list.

### `settings.json`

Writes are atomic: content is written to `settings.json.tmp` then renamed over `settings.json`. A crash between the two steps leaves a stale `.tmp` file but the last good `settings.json` intact.

On startup, a missing or unparseable file is loaded as hardcoded defaults with
no user-facing error. The file on disk is not rewritten until the next
successful settings save. Missing fields that carry Serde defaults are filled
individually, allowing compatible settings additions.

### Local location settings

Preferences associated with a local filesystem directory or the Journal source are stored in the
`settings_location` table of `fbro.sqlite3`. This database owns mutable,
structured application state; the separate `journal.sqlite3` database remains
an append-only audit log. The domain model is `LocationSettings`; sorting is
its first setting and consists of the sort column plus direction.

Schema version 2 uses the `settings_location` name. It migrates version 1
`fbro.sqlite3` databases from `location_settings`, and also migrates valid rows
from the older `journal.sqlite3` location table. Each old table is removed after
its rows are copied. Existing `journal_events` are not modified.
`PRAGMA user_version` tracks future `fbro.sqlite3` schema migrations.

Filesystem rows are keyed by normalized absolute local path. Journal uses the
reserved `source:journal` key independently of its empty/slash root, so its
preference cannot collide with the filesystem root. No schema migration is
needed, and existing filesystem keys retain their meaning. Rows are loaded into
`AppState.location_settings` at startup and updated with an UPSERT whenever
sorting changes while **Remember sorting per location** is enabled. Panels and
tabs then share the same preference for a location, and an unseen location uses
name ascending. When disabled, sorting remains panel-local and navigation keeps
the panel's current sort. Journal follows the same enable/disable, sharing,
reset, remembered-location count, and 30-day retention policy. Remote, archive,
search, and other virtual sources do not participate.

The General settings UI reports the number of valid remembered locations and
offers a confirmed reset. Disabling retains rows for 30 days; re-enabling within
that window reuses them. Expired rows are removed at the next startup or
re-enable attempt. Manual resets and expiry cleanup record their outcomes and
removed counts in the append-only audit journal.

Ordinary location-setting writes are storage only and are not added to the
visible journal.

## Key files

| File | Role |
|---|---|
| `src-tauri/src/config.rs` | Resolves the platform-specific configuration directory |
| `src-tauri/src/state/settings.rs` | `Settings` / `GeneralSettings` structs and JSON `load()` / `save()` |
| `src-tauri/src/state/location_settings.rs` | SQLite-backed per-location settings model and persistence |
| `src-tauri/src/ws/intents/settings.rs` | `SetGeneralOption` intent handler |
| `src-tauri/src/ws/protocol.rs` | `SettingsSnapshot`, `GeneralSettingsSchema` |
| `src-tauri/src/ws/commands.rs` | `app.settings` command + Ctrl+,/Cmd+, binding |
| `src/ws/types.ts` | `GeneralSettings`, `SettingsState` TypeScript types |
| `src/store/index.ts` | `settings` slice, `pendingSettings`, `OPEN_SETTINGS`/`CLOSE_SETTINGS` |
| `src/components/SettingsModal.tsx` | Modal shell — tab sidebar + content area |
| `src/components/SettingsGeneralTab.tsx` | General tab content |
| `src-tauri/src/themes.rs` | Custom scheme files: model, validation, load/save/delete, folder watcher |
| `src-tauri/src/ws/intents/themes.rs` | `CreateColorScheme` / `UpdateColorScheme` / `DeleteColorScheme` |
| `src-tauri/src/ws/appearance.rs` | Active-scheme projection, catalog publish/reload, folder watcher task |
| `src/theme/colorSchemes.ts`, `tokens.ts` | Scheme resolution; registry of editable color tokens |
| `src/components/SettingsColorSchemeTab.tsx`, `ColorSchemePreview.tsx` | Color Scheme editor and live preview |
| `src/components/CustomSchemeStyles.tsx` | Injects `[data-color-scheme="custom:…"]` token rules |

## Adding a new tab

1. Add a slice to `Settings` (BE) and `SettingsState` (FE).
2. Add typed intents for its mutations (BE handler + FE `sendIntent` calls).
3. Write `SettingsXTab.tsx`.
4. Add the tab id to `TABS` in `SettingsModal.tsx`.
5. No changes to the modal shell or state machinery needed.

## Keyboard Shortcuts

The Shortcuts tab (keyboard icon) uses the backend command registry rather than the
general-options tree. `settings.json` stores `command_bindings`, a map from
command ID to an ordered binding array. A missing entry uses the platform
defaults; `[]` explicitly unassigns the command. Each binding has `strokes`
(exactly one `{key,shift?,ctrl?,alt?,meta?}` in V1) and `modes` (empty means both
views). Snapshots include effective commands, default bindings, `customized`,
and `commandRevision`.

Add/Edit opens a recording modal, then a review step with Save and Cancel.
Other actions apply only after backend confirmation. Save failures leave the
editor open with an inline error. Per-command reset reviews conflicts like any
other save. Reset all uses a nested confirmation initially focused on Cancel
and clears every override in one persisted transaction. Closing nested dialogs
restores the opener through the shared modal stack. Shortcut writes and other
runtime settings writes share a serialization gate to prevent lost settings.

The settings sidebar uses decorative Tabler icons for General, Display, Bros,
Sharing, and Shortcuts. Labels stay on one line and the sidebar sizes to its
content, including at larger text sizes. Tabs retain their existing order,
roving keyboard focus, and selected-state semantics.

Shortcuts search uses an inline magnifying-glass icon and an accessible Search
commands label. The All / Customized / Unassigned filter has an inline funnel icon and sits to
its right with
an accessible Show commands label; keyboard order remains search then filter.
