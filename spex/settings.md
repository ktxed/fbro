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
| Persistence | Per subtree: `General` → `~/.config/fbro/settings.json` (atomic temp+rename) |
| Defaults | Hardcoded in BE `Default` impls; missing or corrupt file → defaults silently, no error |
| Validation | Authoritative on BE. FE controls reflect `state.settings.*` after the BE confirms — no optimistic updates |
| Backdrop click | Does **not** close — settings is a considered action; explicit close (Esc or Close button) required |

## Architecture

```
┌── FE ──────────────────────────────────┐     ┌── BE ─────────────────────────┐
│ SettingsModal                          │     │ AppState.settings: Settings   │
│  ├─ tab list (sidebar)                 │     │   └─ general: persisted       │
│  └─ SettingsGeneralTab                 │     │                               │
│                                        │ WS  │ Intent:                       │
│ control onChange → client.sendIntent ──┼────►│  SetGeneralOption { key, val }│
│                                        │     │                               │
│ state.settings ◄── reducer ◄──         │     │ on change:                    │
│                  StatePatch ──────────┼─────┤  1. mutate AppState.settings  │
│                                        │     │  2. write settings.json       │
│                                        │     │     (atomic temp+rename)      │
│                                        │     │  3. push StatePatch           │
└────────────────────────────────────────┘     └───────────────────────────────┘
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

General settings are written to `$XDG_CONFIG_HOME/fbro/settings.json` (falling back to `~/.config/fbro/settings.json`).

Writes are atomic: content is written to `settings.json.tmp` then renamed over `settings.json`. A crash between the two steps leaves a stale `.tmp` file but the last good `settings.json` intact.

On startup, a missing or unparseable file is silently replaced by defaults — no error is shown.

## Key files

| File | Role |
|---|---|
| `src-tauri/src/state/settings.rs` | `Settings` / `GeneralSettings` structs, `load()`, `save_general()` |
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
