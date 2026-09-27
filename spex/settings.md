# Settings

## Overview

fbro has a persistent settings system backed by a typed state tree on the BE.
Settings are exposed through a tabbed modal dialog triggered by **Ctrl+,** (or **Cmd+,** on macOS).

## Design decisions

| Concern | Choice |
|---|---|
| Modal shape | Vertical tab list (left) + content area (right) |
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
| `rememberSortPerLocation` | `bool` | `false` | Remember and share sorting for each local filesystem location. |
| `zshEmacsKeymap` | `bool` | `false` | Start new interactive zsh terminals with `-o emacs`. Other shells and Quick Edit commands are unaffected. Existing terminal sessions keep their current keymap. |
| `fedupDefaultIgnorePatterns` | `string` | `.git`, `.idea`, `.vscode`, `.venv`, `node_modules`, `__MACOSX`, `._*`, `.DS_Store`, `artifacts/e2e`, `dist/assets`, `target/debug`, `target/release`, `target/bundle`, `.m2` | Newline/comma separated wildcard name/path patterns Fedup Bro can use to skip files and folders before scanning. |

Unknown key or type mismatch → `Error { code: "BAD_SETTING", recoverable: true }`.

## StatePatch: settings domain

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
| `fbro.sqlite3` | Mutable structured application state. Currently the `settings_location` table stores sort column and direction by normalized local path. | SQLite. Settings tables use a `settings_` prefix. Sort changes use an UPSERT; reset and retention expiry use `DELETE`. `PRAGMA user_version` versions its schema. |
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

Preferences associated with a local filesystem directory are stored in the
`settings_location` table of `fbro.sqlite3`. This database owns mutable,
structured application state; the separate `journal.sqlite3` database remains
an append-only audit log. The domain model is `LocationSettings`; sorting is
its first setting and consists of the sort column plus direction.

Schema version 2 uses the `settings_location` name. It migrates version 1
`fbro.sqlite3` databases from `location_settings`, and also migrates valid rows
from the older `journal.sqlite3` location table. Each old table is removed after
its rows are copied. Existing `journal_events` are not modified.
`PRAGMA user_version` tracks future `fbro.sqlite3` schema migrations.

Rows are keyed by normalized absolute local path. They are loaded into
`AppState.location_settings` at startup and updated with an UPSERT whenever
sorting changes while **Remember sorting per location** is enabled. Panels and
tabs then share the same preference for a location, and an unseen location uses
name ascending. When disabled, sorting remains panel-local and navigation keeps
the panel's current sort. Remote, archive, search, and other virtual sources do
not participate.

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

## Adding a new tab

1. Add a slice to `Settings` (BE) and `SettingsState` (FE).
2. Add typed intents for its mutations (BE handler + FE `sendIntent` calls).
3. Write `SettingsXTab.tsx`.
4. Add the tab id to `TABS` in `SettingsModal.tsx`.
5. No changes to the modal shell or state machinery needed.
