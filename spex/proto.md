# fbro Client-Server Protocol (FMP) — v0

**Transport:** WebSocket
**Model:** BE-authoritative, FE as cached projection

---

## 1. Overview

This protocol defines communication between a **Backend (BE)** and one or more **Frontends (FE)** for an n-panel file manager.

### Core principles

* BE is the **single source of truth**
* FE maintains a **non-authoritative cached projection**
* Communication is **event-based**
* Filesystem mutations occur **only via jobs**
* All user input is expressed as **intents**
* Long-running operations are **interruptible jobs**

---

## 2. Transport & Framing

* Single persistent WebSocket connection
* Messages are UTF-8 JSON objects
* One message = one JSON object
* One exception: embedded-terminal I/O uses binary frames, because PTY traffic
  is high-frequency opaque bytes. Every binary frame is
  `[tag:u8][session_id:u32-le][payload…]`:
  * `0x01` BE → FE: terminal output (`ServerMessage::TerminalOutput`, never
    serialized as JSON; see `ws/server.rs`)
  * `0x02` FE → BE: terminal input (`WsClient.sendTerminalInput`)
  * Frames with an unknown tag or shorter than 5 bytes are ignored.

```text
Client (FE)  ←→  WebSocket  ←→  Server (BE)
```

---

## 3. Message Envelope

Every message MUST include a `type` field.

```json
{
  "type": "MessageType",
  "timestamp": "UTCTimestamp", // utc timestamp
  "...": "payload"
}
```

---

## 4. Message Types (Top-Level)

| Type            | Direction | Description                   |
| --------------- | --------- | ----------------------------- |
| `Hello`         | FE → BE   | Client capability declaration |
| `Welcome`       | BE → FE   | Protocol confirmation         |
| `Intent`        | FE → BE   | User intent                   |
| `StateSnapshot` | BE → FE   | Authoritative snapshot        |
| `StatePatch`    | BE → FE   | Incremental update            |
| `JobEvent`      | BE → FE   | Job lifecycle update          |
| `Error`         | BE → FE   | Protocol or semantic error    |

---

## 5. Connection Handshake

### 5.1 FE → BE: Hello

```json
{
  "type": "Hello",
  "protocolVersion": "0",
  "client": {
    "kind": "gui",
    "name": "fm-ui",
    "supports": ["jobs", "decisions"]
  }
}
```

---

### 5.2 BE → FE: Welcome

```json
{
  "type": "Welcome",
  "protocolVersion": "0",
  "sessionId": "sess-123"
}
```

Immediately followed by a `StateSnapshot` which contains usually a default files listing and panel configuration.

---

## 6. State Model

### 6.1 State Domains

State is divided into **independently versioned domains**:

```text
layout
panels
slots
focus
selections
jobs
sources
globalSelections
settings
```

`settings.general.zshEmacsKeymap` is a boolean in the snapshot and settings
patches. The existing `SetGeneralOption` intent accepts that key; see
[Settings](settings.md) for its default and effect.

The `slots` domain groups panels into **tab groups**. Each slot owns one or more
panel ids; one tab is active at a time. A single-tab slot renders identically
to a pre-tabs panel — the tab bar is only surfaced once `tabPanelIds.length > 1`.

`layout.slots` is the ordered list of slot ids that drives the visible layout.
`layout.panels` is retained as the parallel list of **active panel ids per slot**
(in slot order) for back-compat — clients new enough to understand slots
should read `layout.slots` and dereference through `slots[slotId].activePanelId`.

---

### 6.2 StateSnapshot

Sent:

* On initial connect
* On FE resync request
* After unrecoverable desync

```json
{
  "type": "StateSnapshot",
  "domains": {
    "layout": { ... },
    "panels": {
      "left": { 
        "status": "loading", 
        "source": "sourceId" 
       },
      "right": { 
        "status": "loading",
        "source": "sourceId" 
      }
    },
    "sources": Source[],
    "focus": "left",
    "jobs": []
  }
}
```

Snapshots are **authoritative**.

### 6.2.1 Sources

Source schema:

{
  "id": "uuid",
  "name": "Home",
  "kind": "filesystem",
  "supports": [
    "ListEntries",
    "OpenFolder",
    "CopySelection",
    "MoveSelection",
    "DeleteSelection",
    "InspectItem"
  ],
  "capabilities": {
    "readOnly": false,
    "supportsStreaming": true,
    "supportsJobs": true
  }
}

Key ideas:
- supports is authoritative
- FE never guesses what is allowed
- AI can reason safely using this list
- Jobs are validated against source capabilities

Mounted removable filesystem Sources may also include `eject: { driveName,
mountedVolumeCount, canEjectDrive }`. The absence of `eject` means the client
must not offer an eject action. Physical device paths and drive IDs stay in
the backend.

`EjectSource` intent args are `{ sourceId, action, requestId,
expectedMountedVolumeCount }`, where `action` is `unmount-volume` or
`eject-drive`. The backend re-enumerates, rejects changed whole-drive volume
counts and active/queued file jobs on affected volumes, and replies with
`EjectSourceOk { requestId, sourceId, action }` or
`EjectSourceFailed { requestId, sourceId, code, message }`. Eject failures
use `EJECT_SOURCE_*` codes. Sources and affected panels are patched before a
success reply; a partially completed operation is reconciled before failure.

Important:

Panels reference sources by ID
Source objects are global
Panels are just views into a source

---

### 6.3 StatePatch

Incremental update scoped to a domain.

```json
{
  "type": "StatePatch",
  "domain": "panels",
  "entityId": "left",
  "version": 12,
  "patch": {
    "status": "ready"
  }
}
```

Rules:

* Versions MUST be monotonic per domain/entity
* FE MUST request snapshot if versions skip

---

## 7. Panel Model (v0)

```json
{
  "panelId": "left",
  "slotId": "left",
  "tabKind": "listing",
  "tabTitle": null,
  "path": "/home/user",
  "status": "loading | ready | error",
  "sort": {
    "by": "name",
    "order": "asc"
  },
  "filter": null,
  "cursor": 0,
  "entries": []
}
```

* `slotId` — id of the slot this panel belongs to. Panels sharing a slot are tabs.
* `tabKind` — `"listing"` (default folder view) or `"terminal"` (promoted PTY).
* `tabTitle` — user-supplied tab title; omitted means "derive from path/cwd".

### 7.1 Slot Model

```json
{
  "slotId": "left",
  "tabPanelIds": ["left", "left-tab-2"],
  "activePanelId": "left-tab-2"
}
```

Slot patches use `domain: "slots"`. Patch payloads:

* `upsert` fields: `tabPanelIds`, `activePanelId` (any subset may be present).
* Slot removal: `{ "remove": true }`.

### 7.2 Tab Intents

| Intent | Args | Behavior |
|---|---|---|
| `panel.tab.new` | `{ slotId, source: "clone" }` | Creates a new tab after the active tab and activates it; clones an active local filesystem listing, otherwise opens the user's Home directory. |
| `panel.tab.close` | `{ panelId }` | Closes the tab; activates the tab to its left. No-op when this is the slot's only tab. |
| `panel.tab.switch` | `{ slotId, panelId }` | Activates `panelId` within `slotId`. |
| `panel.tab.rename` | `{ panelId, title }` | Sets `tabTitle` override (FUTURE — protocol only). |
| `panel.terminal.promote` | `{ panelId }` | Detaches the panel's embedded PTY and inserts it as a sibling terminal tab; source panel returns to listing. |
| `QuickEditFile` | panel id scope | Opens the cursor file in the configured Nano/Vim embedded terminal editor. On Windows, also probes Git for Windows `usr\\bin`, Scoop, Chocolatey, and WinGet for nano/vim/micro/edit. The temporary terminal closes when the editor exits. |

---

## 8. Streaming Directory Listings

Panels are populated via background scan jobs.

### 8.1 Entries Appended

```json
{
  "type": "StatePatch",
  "domain": "panels",
  "entityId": "left",
  "version": 13,
  "patch": {
    "appendEntries": [
      {
        "name": "file.txt",
        "kind": "file",
        "size": 1234,
        "mtime": "2026-03-10T10:00:00Z"
      }
    ]
  }
}
```

Notes:

* Entries MAY arrive incrementally
* BE controls ordering
* FE MUST NOT re-sort
* Flat virtual listings MAY include `sourcePath` plus an optional `pathHint`; search results use `pathHint` only for exact-name duplicates and compute it from the complete result set before window slicing.

---

### 8.2 Sorting Behavior (Important)

Changing sort order:

1. Cancels current scan
2. Clears entries
3. Resets cursor to `0`
4. Restarts scan

This is intentional to avoid cursor instability.

---

## 9. Intents

Intents represent **user intent**, not UI actions.

### 9.1 Intent Envelope

```json
{
  "type": "Intent",
  "name": "IntentName",
  "scope": {},
  "args": {},
  "options": {}
}
```

---

### 9.2 Example: Toggle Selection at Cursor

```json
{
  "type": "Intent",
  "name": "ToggleSelectionAtCursor",
  "scope": "focused-panel"
}
```

---

### 9.3 Example: Copy Selection (F5)

```json
{
  "type": "Intent",
  "name": "CopySelection",
  "scope": {
    "from": "focused-panel",
    "to": "other-panel"
  },
  "options": {
    "confirm": true,
    "background": false
  }
}
```

---

### 9.4 Global Selection Deletion

Global selections are virtual state, not filesystem ownership. Deleting from the
global selections source mutates only the global selection store.

Root-level deletion removes named global selection records:

```json
{
  "type": "Intent",
  "name": "RemoveGlobalSelections",
  "args": {
    "names": ["Selection 1", "Build artifacts"]
  }
}
```

Deletion inside a named global selection removes references by their stored
source paths:

```json
{
  "type": "Intent",
  "name": "RemoveGlobalSelectionItems",
  "args": {
    "panelId": "left",
    "selectionName": "Selection 1",
    "paths": ["/home/user/a.txt", "/home/user/b.txt"]
  }
}
```

Backend behavior:

* `RemoveGlobalSelections` removes only virtual selection records.
* `RemoveGlobalSelectionItems` removes only virtual item references.
* Neither intent deletes, moves, truncates, or otherwise mutates referenced
  filesystem paths.
* The BE emits `StatePatch` messages for `globalSelections`, `sources` when
  source visibility changes, and rescanned global-selection panels.
* `StartFileOp` with `opType: "delete"` from the global selections source is
  treated as a defensive compatibility path and resolves to the same virtual
  store mutations, not a filesystem delete job.

---

## 10. Panel Filtering

Filtering is semantic and BE-owned.

### Example: Prefix Filter

```json
{
  "type": "Intent",
  "name": "SetPanelFilter",
  "scope": "focused-panel",
  "args": {
    "mode": "prefix",
    "value": "img_"
  }
}
```

BE behavior:

* Clears entries
* Restarts scan
* Streams filtered results only

---

## 11. Jobs

### 11.1 Job Lifecycle

```text
Created → Running → AwaitingDecision → Running → Completed
                      ↘ Cancelled
```

Jobs are **cooperatively suspendable**.

---

### 11.2 JobEvent

```json
{
  "type": "JobEvent",
  "jobId": "job-42",
  "event": "Progress",
  "progress": {
    "current": 10,
    "total": 120
  }
}
```

---

## 12. Job Decision Requests

### 12.1 Decision Required (BE → FE)

```json
{
  "type": "JobEvent",
  "jobId": "job-42",
  "event": "DecisionRequired",
  "decision": {
    "kind": "FileConflict",
    "source": "/src/a.jpg",
    "destination": "/dst/a.jpg",
    "options": [
      "overwrite",
      "skip",
      "rename",
      "cancel"
    ],
    "supportsApplyToAll": true
  }
}
```

Job enters `AwaitingDecision`.

---

### 12.2 Provide Decision (FE → BE)

```json
{
  "type": "Intent",
  "name": "ProvideJobDecision",
  "args": {
    "jobId": "job-42",
    "decision": "skip",
    "applyToAll": true
  }
}
```

---

## 13. Error Handling

### Error Message

```json
{
  "type": "Error",
  "code": "InvalidIntent",
  "message": "Cannot copy: no selection",
  "recoverable": true
}
```

FE SHOULD:

* Display error
* Continue session if recoverable

---

## 14. Desync & Recovery

If FE detects:

* Missing versions
* Invalid patch
* Unknown entity

FE MUST send:

```json
{
  "type": "Intent",
  "name": "RequestStateSnapshot",
  "args": {
    "domain": "panels",
    "entityId": "left"
  }
}
```

BE responds with scoped snapshot.

---

## 15. Non-Goals (v0)

Explicitly out of scope:

* Binary transfers
* Plugin loading
* Multi-user state
* Security/auth
* Undo/redo

---

## 16. Design Summary

* BE publishes **facts over time**
* FE mirrors and renders
* Jobs mutate state
* Decisions are data
* UI, TUI all speak the same protocol

---
