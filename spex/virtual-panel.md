# Virtual Panel Rendering

This document describes the dual-mode rendering strategy used by fbro panels to handle directories of any size efficiently.

---

## Problem

When a directory contains thousands of files (5k+), sending all entries to the FE and rendering them as DOM nodes causes:

- Large WebSocket payloads during `appendEntries` batching
- A heavy DOM with thousands of rows
- Sluggish scrolling and high memory usage

## Solution: Dual-Mode Rendering

Panels operate in one of two modes, chosen automatically per directory based on entry count.

### Mode A — Full (default)

Used when a directory has **<= 1500 entries** (constant `WINDOWED_THRESHOLD` in `scan.rs`).

Behaviour is unchanged from the original design:

1. `run_scan` sends entries in batched `appendEntries` patches (50 entries per batch).
2. A final `StatePatch` with `status: "ready"` signals scan completion.
3. FE accumulates all entries in `panel.entries` and renders them as regular DOM nodes.

### Mode B — Windowed / Virtual Scroll

Used when a directory has **> 1500 entries**.

1. `run_scan` collects and sorts all entries, then stores the **full list** directly in the BE panel state (`panel.entries`).
2. Instead of batched `appendEntries`, the BE sends a single `StatePatch`:
   ```json
   {
     "status": "ready",
     "totalCount": 6000,
     "windowStart": 0,
     "windowEntries": [ ...first 200 entries... ],
     "cursor": 0,
     "selection": []
   }
   ```
3. The FE stores `totalCount` and `windowStart` alongside the window slice in `panel.entries`.
4. The FE renders a virtual scroll container whose total height equals `totalCount * ROW_HEIGHT` (22px per row). Only the window entries are rendered as absolutely-positioned DOM nodes.
5. On scroll, the FE sends a debounced (50ms) `RequestWindow` intent with the desired `{ start, count }` range (typically the visible area plus a one-screen buffer on each side).
6. The BE slices its full entry list and responds with a `StatePatch` containing `{ windowStart, windowEntries, totalCount }`.

---

## Protocol Additions

### New Intent: `RequestWindow`

Sent by the FE when the user scrolls in a windowed panel.

```json
{
  "type": "Intent",
  "name": "RequestWindow",
  "scope": "focused-panel",
  "args": { "start": 400, "count": 200 }
}
```

The BE responds with a `StatePatch` containing `windowStart`, `windowEntries`, and `totalCount`.

Grid requests also include `{ panelEpoch, visibleStart, visibleCount }` for
the actual viewport, excluding the render buffer. The backend resolves those
absolute indices against its own listing for thumbnail priority. Reports for
old listing epochs are ignored. This geometry is accepted in Mode A too,
where `RequestWindow` does not need to send a render-slice patch. Only active
listing tabs contribute to thumbnail visibility. See [Thumbnails](thumbnails.md)
for the 12-second background-generation cooldown.

### Extended StatePatch Fields

| Field | Type | Description |
|-------|------|-------------|
| `totalCount` | `number` | Total entry count in the directory. Present only in Mode B. |
| `windowStart` | `number` | Absolute index of the first entry in the window slice. |
| `windowEntries` | `VFSEntry[]` | The window slice replacing `panel.entries` on the FE. |

When `totalCount` and `windowStart` are set to `null` in a patch (e.g. during navigation or rescan), the panel exits windowed mode.

---

## Cursor and Selection

### Empty folder presentation

In either list or grid view, a folder listing with `status: "ready"` and zero
total visible entries renders an empty-folder indicator and a vertical list
of available suggestions. Loading, error states, and unloaded virtual windows
do not count as empty. Search and global-selection collections are excluded.

The message is “🙄 This folder is empty. Someone should probably do something
about that.” With hidden files disabled, the first sentence instead reads
“🙄 This folder has no visible items.” Emptiness refers to the backend-provided
visible listing; no additional filesystem scan is performed. A filtered empty
listing says “No matching items.” and offers no creation suggestions.

Suggestions are an ordered action list filtered by backend source capabilities.
“Create a new file” comes first, requiring `Touch` and invoking the existing
`file.touch` command and New File dialog. “Create a new folder” comes second,
requiring `NewFolder` and invoking `file.mkdir` and the Create Directory dialog.
Sources without either capability display the indicator alone. Shortcuts come from the command
registry. Focus behavior is specified in [Keyboard](keyboard.md).

- **Cursor** is always an absolute index into the full entry list, same as Mode A.
- **Selection** is stored as an array of absolute indices on both BE and FE.
- `SelectByMask` / `DeselectByMask`, like Select All and Invert Selection,
  resolve matches from the complete backend listing. They never restrict
  matching to the rendered window. Their `+` / `-` prompts request a window
  around the cursor when its entry is unloaded, so the extension prefill still
  reflects the cursor filename after scrolling elsewhere.
- The FE uses a `Set` for O(1) selection lookup during rendering.
- When the cursor moves outside the current window (via keyboard navigation), the BE automatically sends a new window patch centered on the cursor position.

---

## Entry Resolution in Windowed Mode

Since the FE only holds a slice of entries, converting an absolute index to a local array index requires subtracting `windowStart`:

```
localIndex = absoluteIndex - panel.windowStart
entry = panel.entries[localIndex]
```

This applies to all FE code that accesses entries by cursor or selection index (Panel.tsx, Toolbar.tsx, etc.).

---

## Key Constants

| Constant | Location | Value | Purpose |
|----------|----------|-------|---------|
| `WINDOWED_THRESHOLD` | `scan.rs` | 1500 | Entry count above which Mode B activates |
| `INITIAL_WINDOW_SIZE` | `scan.rs` | 200 | Number of entries in the first window |
| `ROW_HEIGHT` | `Panel.tsx` / CSS `--row-height` | 22px | Fixed row height used for virtual scroll math |

---

## Files Involved

| File | Role |
|------|------|
| `src-tauri/src/jobs/scan.rs` | Mode detection, window slicing on initial scan |
| `src-tauri/src/state/mod.rs` | `Panel` struct: `windowed`, `window_start`, `window_count` fields |
| `src-tauri/src/ws/server.rs` | `RequestWindow` handler, cursor-triggered window updates, `send_window` helper |
| `src-tauri/src/ws/protocol.rs` | `PanelSnapshot`: optional `total_count`, `window_start` |
| `src/ws/types.ts` | `PanelState`: `totalCount?`, `windowStart?` |
| `src/store/index.ts` | Handles `windowEntries`, `totalCount`, `windowStart` patches |
| `src/components/Panel.tsx` | Virtual scroll rendering, debounced `RequestWindow` on scroll |
| `src/components/Toolbar.tsx` | `entryAt()` helper for windowed index resolution |

---

## Lifecycle

```
Directory opened
    |
    v
run_scan collects & sorts all entries
    |
    +-- entries <= 1500 --> Mode A: batched appendEntries (current behaviour)
    |
    +-- entries > 1500  --> Mode B:
            |
            +-- Store full list in BE panel.entries
            +-- Send initial window (200 entries centered on cursor)
            +-- FE renders virtual scroll container
            |
            +-- User scrolls --> FE sends RequestWindow --> BE sends new window
            +-- Cursor moves outside window --> BE auto-sends new window
            |
            +-- Navigate away / rescan --> windowed state reset, mode re-evaluated
```
