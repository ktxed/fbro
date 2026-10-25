# fbro Client-Server Protocol (FMP) — v0

**Transport:** WebSocket
**Model:** BE-authoritative, FE as cached projection

---

## 1. Overview

Settings snapshots expose read-only `general.supportsOsNotificationSettings`,
false by default and true on macOS and Windows, including development executables.
It gates the host notification notice and the **Open notification settings** action.
Snapshots also expose read-only `general.osNotificationPlatform`
(`linux`, `macos`, `windows`, or `none`), which selects that notice. The field is
not part of settings patches; clients preserve it across merges. It is not
persisted or writable.
The section is visible when either notification capability is true. If native
delivery is unavailable but the system page is available, the delivery checkbox
is disabled and development guidance is displayed. The two capabilities remain
independent; a settings link never implies native delivery support.
`OpenOsNotificationSettings { requestId }` opens the backend host's notification
settings through PAL on a blocking worker. macOS opens fbro's notification pane.
Windows opens the system Notifications page. It replies with
`OpenOsNotificationSettingsOk { timestamp, requestId }` or
`OpenOsNotificationSettingsFailed { timestamp, requestId, code, message }`, using
`OPEN_OS_NOTIFICATION_SETTINGS_FAILED` for invalid/missing request IDs, unsupported
hosts, worker errors, and native launch failures. No URL/path is accepted from
the client; no preference, authorization state, notification history, or command
journal is changed. The success reply acknowledges the launch request, not a
permission grant. Settings consumes correlated replies and shows failures inline.

Settings snapshots and settings-domain patches expose `general.showButtonBar`
as a boolean. `SetGeneralOption { key: "showButtonBar", value: true | false }`
updates and persists the application-wide bottom button bar visibility, then
broadcasts the authoritative setting to connected clients. Other value types
return `BAD_SETTING` without changing visibility. The on-disk field is
`general.show_button_bar`, defaulting to true when absent in older settings.
The command registry includes `app.toggle_button_bar` (**Toggle Button Bar**)
with no default binding; the frontend uses the same settings intent.

`ClearIconCache { requestId }` clears the macOS native file icon disk and memory
cache on a blocking worker. It returns `ClearIconCacheOk { timestamp, requestId }`
or `ClearIconCacheFailed { timestamp, requestId, code, message }` with
`CLEAR_ICON_CACHE_FAILED` (including unsupported platforms). Success broadcasts
a settings patch updating read-only `general.iconCacheRevision` before replying.
Settings snapshots also provide read-only `general.iconCacheDir`, empty on other
platforms. The intent is recorded in the command journal. Frontend consumers use
correlated replies for operation status; backend notifications provide success
feedback without reducer duplication.


### Adding a shared folder

`AddSharedFolder { requestId, path, panelId? }` adds an accessible local folder to
remote sharing. The backend normalizes and validates the path; optional panelId
provides the local base for relative paths, otherwise Home is used.
It replies to the requesting connection with
`AddSharedFolderOk { timestamp, requestId, targetId, path }` or
`AddSharedFolderFailed { timestamp, requestId, code, message }`.
Failures use `FOLDER_PATH_REQUIRED`, `FOLDER_NOT_FOUND`, `NOT_A_FOLDER`,
`FOLDER_ACCESS_DENIED`, or `FOLDER_UNAVAILABLE`. Success broadcasts the
authoritative shareConfig patch before the acknowledgement. Equivalent existing
paths return their existing target ID without adding a duplicate.

`SharedRoot.origin` additionally accepts `folder`; for these roots sourceId is an
internal local-driver registry key, not a panel Source. shareConfig snapshots
include `version`, and shareConfig StatePatch versions increase monotonically;
older patches are discarded. Existing sharing intents and share-file formats
remain supported. A connection loss releases the input's pending state and
retains its path for review/retry after the next snapshot.

The settings snapshot's `general.enableHeic2any` boolean and settings-domain
patches expose the opt-in HEIC/HEIF preview preference. `SetGeneralOption`
accepts `{ "key": "enableHeic2any", "value": true | false }`; other value
types return `BAD_SETTING`. Persistence uses `enable_heic2any` with a default
of false for older settings files. See [settings.md](./settings.md).
Viewer clients also receive this field in their initial lightweight settings
patch; they do not receive a full state snapshot.

This protocol defines communication between a **Backend (BE)** and one or more **Frontends (FE)** for an n-panel file manager.

### Core principles

* BE is the **single source of truth**
* FE maintains a **non-authoritative cached projection**
* Communication is **event-based**
* Filesystem mutations occur **only via jobs**
* All user input is expressed as **intents**
* Long-running operations are **interruptible jobs**

---

### Viewer targets

`ResolveViewerFile` is read-only and accepts `{ panelId, requestId }`. The backend
resolves the panel's authoritative cursor through the same resolver used by
`ViewFile`, including real search-result sources and local symlink targets.
The request-local `ViewerFileReady` reply contains `{ timestamp, requestId,
target }`, where `target.kind` is `file`, `directory`, `empty`, or `error`:

- `file` carries `file: { panelId, path, sourceId, filename, fileSize, isLocal,
  initialFindQuery }`; unknown size/query are `null`.
- `directory` carries `name`; `error` carries `message`; `empty` has no payload.

`ViewFile` also accepts optional `filePath` and `initialFindQuery`, alongside
`panelId`, `sourceId`, and `filename`. An explicit file path identifies a resolved
preview target independently of the panel's current cursor. The backend checks
its registered driver and validates local files/symlinks again before opening.
Legacy `{ path, filename }` calls keep their existing behavior.

The preview receives resolver replies through a request-local subscription on
the GUI connection; the main reducer leaves authoritative state unchanged.
Superseded resolver replies are ignored by request ID. Each displayed content
session uses the same isolated viewer client as F3; replacing its source/path
tears down that session and discards its late WebSocket/HTTP results. This avoids
matching chunks from different sources by path alone without changing existing
`FileChunk` or `MarkdownPreview` wire formats.

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
* Backend delivery of panel patches MUST preserve newer authoritative state:
  delayed structural patches cannot restore older cursor/selection or decrease
  the version. Streamed scans still apply queued entries and completion status.
  Window-only patches use their independent `windowVersion` and do not change
  the structural version.
* FE MUST request snapshot if versions skip

Panel navigation's loading patch includes `path`, `sourceId`, and `showHidden`
together. Jump to a hidden search-result file enables the panel's existing
Show hidden flag before scanning, so clients receive the visibility change
atomically with the destination and can display the focused file.

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
  "activePanelId": "left-tab-2",
  "version": 3
}
```

Slot patches use `domain: "slots"`. Patch payloads:

* `upsert` fields: `tabPanelIds`, `activePanelId` (any subset may be present).
* Slot removal: `{ "remove": true }`.

Slot snapshots include their current `version`. Slot patch envelope versions
increase per slot, including changes published by natural terminal exit.
Membership/order/activation patches carry both `tabPanelIds` and
`activePanelId`, so a newer patch is sufficient when an earlier one is delayed.
The frontend ignores stale/duplicate slot updates and resets its acceptance
baseline on `Welcome`, allowing counters to restart after a backend restart.

### 7.2 Tab Intents

| Intent | Args | Behavior |
|---|---|---|
| `panel.tab.new` | `{ slotId, source: "clone" }` | Creates a new tab after the active tab and activates it; clones an active local filesystem listing, otherwise opens the user's Home directory. |
| `panel.tab.close` | `{ panelId }` | Closes the tab; activates the tab to its left. No-op when this is the slot's only tab. |
| `panel.tab.switch` | `{ slotId, panelId }` | Activates `panelId` within `slotId`. |
| `ReorderPanelTab` | `{ slotId, panelId, beforePanelId: string \| null }` | Moves a tab before a sibling in the same slot; explicit null appends. Valid changed requests broadcast only a slot patch with the order and unchanged active tab. Focus and panel state are preserved. Missing/malformed, stale, cross-slot and unchanged requests are no-ops. Recorded in the command journal; no new reply type. |
| `panel.tab.rename` | `{ panelId, title }` | Sets `tabTitle` override (FUTURE — protocol only). |
| `panel.terminal.promote` | `{ panelId }` | Detaches the panel's embedded PTY and inserts it as a sibling terminal tab; source panel returns to listing. |
| `QuickEditFile` | panel id scope; `{ path, filename }` | Opens the explicitly named local file in the configured Nano/Vim embedded terminal editor. Both arguments are required; `path` must match the panel's current directory and `filename` must name a regular file in its complete backend listing. Cursor position, selection, and render-window offsets do not affect the target. Missing/invalid arguments or a changed directory produce the existing Quick Edit error and failed journal result, with no cursor fallback. `path` is the editor's working directory and the scoped panel hosts the terminal. On Windows, also probes Git for Windows `usr\\bin`, Scoop, Chocolatey, and WinGet for nano/vim/micro/edit. The temporary terminal closes when the editor exits. |

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
* Local symlink entries keep `kind: "symlink"`, may include `targetKind: "file" | "directory" | "other"`, and use `pathHint` for the target path. If supplied, `sourcePath` still identifies the entry itself, not the link target.

---

### 8.2 Sorting Behavior (Important)

Changing sort order:

1. Cancels current scan
2. Clears entries
3. Resets cursor to `0`
4. Restarts scan

This is intentional to avoid cursor instability.

Journal uses the same `SetSort` intent and authoritative rescan flow. When
`rememberSortPerLocation` is enabled, Journal panels/tabs share its persisted
source preference; other virtual sources retain their current behavior. The
Journal driver observes all supported columns and both directions, including
numeric millisecond Date ordering. No new protocol message is introduced.

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
  "name": "ToggleSelection",
  "scope": "left",
  "args": { "calculateSize": true }
}
```

`ToggleSelection` resolves the cursor against the backend's complete panel listing
and toggles that entry's selection. An empty listing or invalid cursor is a no-op.
The optional boolean `calculateSize` defaults to `false`. When true, newly selecting
a real directory with an unknown size starts a calculation unless already queued
or running. Deselecting never cancels size work; known sizes, including zero, are
reused. Selection is patched promptly with `selection` and `selectedEntries`,
before size work. The frontend supplies the resolved panel ID for Space commands.

`ToggleDirectorySizes` takes a panel scope and no arguments. It resolves selected
directories from the backend's full listing, or all listed directories if selection
is empty, independently of the frontend's render window. Each running/queued
target is cancelled; each idle target is freshly calculated. Selection is unchanged.
`CalculateSize` and `CancelSizeCalculation` remain available with `{ "name": string }`
for explicit per-folder actions, including Jobs cancellation.

Sizing supports local filesystem drivers and ZIP/7z archive trees. Other drivers
and symlinks are skipped; virtual and remote paths are never passed to the local
filesystem walker. Results use the existing `calculatingSize` / `updateEntrySize`
panel patches, guarded by the source, path, and `panelEpoch`. These read-only size
and selection intents are excluded from the persistent command journal.

`SelectByMask` and `DeselectByMask` take a panel scope (the GUI sends the
originating panel ID) and `{ "mask": string }`. They add or remove every matching
file in the complete current backend listing, including entries outside a Mode B
render window. Missing or whitespace-only masks and panels that are not ready
are no-ops. No new reply type is used: the result is a versioned panel patch with
`panelEpoch`, `selection`, and `selectedEntries`.

Masks contain whitespace-separated alternatives, anchored to whole filenames
and matched case-insensitively. Only `*` (any text, including newlines) and `?`
(one character) are special; all other characters are literal. An exact `*.*`
alternative is equivalent to `*`, including for extensionless files. Only regular
files and symlinks with a known file target match; directories, directory
symlinks, unknown symlinks, `..`, and other entry kinds do not. Existing unrelated
selection is retained. Matching runs off the async executor; results from a
replaced listing epoch are discarded. These intents are excluded from the
persistent command journal, like other selection intents.

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

#### StartFileOp repeated filenames

`StartFileOp` accepts optional `args.duplicateNamePolicy: "exclude" | "suffix"`.
It applies only to Copy from a search-results panel using its real result paths,
not an explicit virtual clipboard source. Omitted values default to `"exclude"`;
unknown values reject an applicable operation with `START_FILE_OP_FAILED`.

The backend processes regular files in the supplied confirmation-list order.
For each exact repeated filename, `"exclude"` drops later files; `"suffix"`
keeps them with backend-generated destination names. The first file keeps its
name. Folders, symlinks, case variants, and nested contents retain their existing
behavior. This policy is separate from `conflictPolicy`, which still governs
existing destination entries and conflicts arising after preparation.

Generated names append `-` and four lowercase hexadecimal UUID characters before
the final extension, or at the end for extensionless names and dotfiles. Original
batch names, generated names, and existing destination entries are reserved;
exhausting 128 attempts rejects the operation. Foreground, background, and dry
mode use the same prepared entries and totals.

`FileOpFileSchema` / TS `FileOpFile` have optional `destName` in job messages.
It reports the backend-resolved name when present. Per-file `destName` supplied
by clients is ignored; the existing top-level single-item `destName` argument
remains the supported explicit rename input. `sourcePath` always identifies the
original source and is preserved in backend selection patches and snapshots.

Job file metadata can also include a backend-resolved `location`:

```ts
type JobFileLocation = {
  sourceId: string;
  path: string;
  parentPath: string;
  cursorHint: string;
};
```

`path` is the path to copy in that Source's path space. Local locations contain
the actual absolute host path, including when an outgoing transfer exposes it
under a different shared-root display path. `parentPath` and `cursorHint` reveal
the file in a listing. Display paths and transfer digest identities remain
independent of this location. The backend resolves locations from the original
operation driver at launch, so panel navigation while a job waits does not
change their meaning. Internal shared-folder drivers resolve to a mounted local
panel Source by host path. Metadata omits `location` when no navigable source
location is available; clients must not interpret display or relative virtual
paths as host paths. Serving-side transfer metadata describes only source files.
Client-supplied locations do not override backend resolution.

To reveal a job file, `NavigateToHistoryEntry` accepts `{ panelId, sourceId,
path, cursorHint? }`, with `path = location.parentPath`. The optional explicit
`cursorHint` takes precedence over the saved history cursor hint. Source
switching, archive restoration, search lifetime checks, and unavailable-source
errors use the existing history navigation lifecycle. Copying uses
`location.path` directly.

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

`Cancelled` is absorbing. Late progress, decisions, and resume events cannot
reactivate a cancelled job. A worker’s late `Completed`/`Failed` acknowledgement
is normalized to `Cancelled`, allowing queue cleanup after the worker drains.
The frontend also ignores non-cancellation lifecycle events for cancelled jobs.
Cancelling an outgoing transfer broadcasts `Cancelled` to all GUI/observer
state clients, including the requesting connection.


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

Search `JobEvent`s carry `searchProgress: { scanned, matched, currentPath,
estimatedPercent, newResults }`. `scanned` counts inspected entries independently
of matches. `currentPath` identifies the most recently started still-active
worker folder (empty when no folder is active). Initial and periodic activity
updates are sent even with an empty `newResults` array, approximately every
200 ms. Result batches may also emit when they reach 50 entries.

`estimatedPercent` is the integer percentage of completed / discovered folders,
capped at 99 while running; discovering more folders can revise it downward.
The frontend displays the estimate without calculating its own total and uses
an indeterminate bar if older payloads omit the field. Search terminal events
also carry `searchProgress`, after buffered results and final counters have been
flushed. `Completed` reports 100; `Cancelled` retains the final estimate below
100. Terminal snapshots have an empty current path and no new results. Search
cancellation drains workers before sending its terminal event.

### 11.3 Thumbnail queue inspection

`QueryThumbnailQueue { requestId }` is a read-only intent. It replies to the
requesting connection with `ThumbnailQueueReady { timestamp, requestId, queue }`.
`queue` contains `running`, `queued`, `maxConcurrent`, `backgroundWaitMs`
(remaining background cooldown, zero when no background work is held), and `tasks` (at most
100 entries). Each task has `path`, `size` (pixels), `status` (`Running`, `Cancelling`, or
`Queued`), `priority` (`foreground`, `visible`, or `background`), and `speculative` (warmup).
Counts cover the complete deduplicated pool. Tasks list running work first,
then queued work in dispatch order, accounting for visibility demotion.
Abandoned queued tasks disappear immediately; active tasks become `Cancelling`
until their worker returns. Completed tasks disappear; they are not retained as file-job history.
The read-only `settings.general.supportsManagedThumbnails` snapshot field is
provided by the backend PAL (true on Linux, false on macOS/Windows). The Jobs modal
shows its Thumbnails tab only when this capability is true, and queries immediately
when the tab opens and once per second while it remains mounted and selected.
The query itself remains available on all platforms.

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

## Git branch creation

`GitCreateBranch` accepts `{ panelId, branch, activityId }` and creates and
switches to a local branch from the current HEAD at execution time. The backend
validates literal names with Git, rejecting empty/option-like names and revision
shorthand. Existing refs are never overwritten. Local changes are preserved;
there is no stash, push, or upstream setup. Detached HEAD is supported.
A short atomic job runs on the blocking pool using the existing Git activity
lifecycle: `GitActionFinished` correlates by `activityId`, with success/failure
and an error message. Successful panel rescan precedes completion. Closing the
modal backgrounds the activity without interrupting an atomic switch.

## Git branch listing and remote checkout

`GitRepoStatus.branches` remains the local branch-name list. `remoteBranches`
adds `{ name, refName }` entries for non-symbolic refs under `refs/remotes/`,
ordered by ref name. Names include the remote prefix (for example
`origin/feature/nested`); `refName` is the full identity. Listing is local and
performs no fetch. Symbolic aliases such as `origin/HEAD` are excluded.

`GitCheckoutRemoteBranch` accepts `{ panelId, remoteRef, localBranch?, activityId }`.
The backend validates the exact remote ref against fresh repository state. With
no explicit local name, it prefers the conventional local branch if it tracks
that remote ref, otherwise reuses the sole tracking local branch, otherwise
creates a tracking branch using the remote branch's name. An occupied name or
multiple tracking candidates returns `GitActionFinished` with `success: false`,
`code: GIT_LOCAL_BRANCH_NAME_REQUIRED`, `suggestedBranchName`, and an explanatory
`message`, without mutation or a separate generic error. The frontend requests
a new name inline; retry sends `localBranch` and creates a new tracking branch.
Existing branches and their upstreams are never overwritten or reassigned.
These optional reply fields are omitted from ordinary Git action completions.

Local and remote checkout run as short atomic jobs on the blocking pool, using
the existing correlated activity, journal, and rescan-before-completion flow.
`GitSwitchBranch` accepts literal existing local names only. Checkout preserves
local work; conflicting changes or worktree ownership are reported as Git
failures. Closing the modal backgrounds started checkout without interrupting it.
No automatic stash, force, fetch, pull, or detached checkout is performed.

## Git repository initialization

`InitGitRepository` accepts `{ panelId, sourceId, targetPath, branch, requestId }`.
The palette targets the active panel's current folder; a single-directory
context-menu action targets that directory. The backend verifies the panel's
source and that the target is its current folder or a direct child. Only
mounted, writable local filesystem sources and listing tabs are supported.

A short atomic Git job resolves the native path through the source driver and
runs system Git on the blocking pool. The folder must already exist. Literal
branch names use the same Git validation as branch creation; existing `.git`
files/directories and bare repository roots are rejected. A folder underneath
an existing parent repository may become an independent nested repository.
Initialization preserves existing files without staging, committing, or setting
a remote. Related local panels rescan while preserving cursor and selection.

Replies are `InitGitRepositoryOk { timestamp, panelId, requestId, path, branch }`
and `InitGitRepositoryFailed { timestamp, panelId, requestId, code, message }`,
where the failure code is `GIT_INIT_FAILED`. Every attempt and its outcome are
recorded in the journal. The modal defaults the branch to `main`, closes on its
matching success, and retains input on matching failure. Closing during work
backgrounds initialization; a late reply cannot close or change a newer form.
Background results provide transient feedback.

## Check hashes Bro messages

`LaunchBro` accepts `builtin:check-hashes` with
`params: { "saveResult": "true" | "false" }`. `BroParam.kind` additionally
supports `"bool"`; an omitted optional boolean uses the declared default.
A normal `JobStarted` has `kind: "hash-check-bro"`, `opType: "Check hashes"`,
`supportsPause: false`, and the original `requestId`. Progress, cancellation,
and terminal state use the existing job messages and intents.

The new backend messages are correlated by `jobId`:

- `HashCheckBroResult { timestamp, jobId, result: { ignoredCount, items } }`
  replaces the result projection with an initial/final snapshot.
- `HashCheckBroFileResult { timestamp, jobId, item }` replaces one existing
  row identified by `hashPath`; these updates are reliable and ordered.
- `HashCheckReportSaved { timestamp, jobId, path }` confirms a flushed report.
- `HashCheckReportFailed { timestamp, jobId, message }` reports saving failure
  without discarding hash-check results or failing the completed check job.

Each item contains `hashPath`, `counterpartPath`, `status`, `expectedHash`,
`actualHash`, and `error`. The final three fields are nullable. Status values
are `pending`, `checking`, `matched`, `mismatched`, `error`, and `cancelled`.
The final result snapshot is sent before terminal `JobEvent`. Backend job
metadata retains the complete result and report outcome. See `bro.md` §7.2.

### Generate hashes Bro messages

`LaunchBro` with `broId: "builtin:generate-hashes"` accepts
`params.overwriteExisting` as the string `"true"` or `"false"` (default false).
Discovery emits the usual bool parameter. `JobStarted` uses kind
`"hash-generate-bro"`, opType `"Generate hashes"`, and the launch requestId.

- `HashGenerateBroResult { timestamp, jobId, result }` carries the initial/final
  full snapshot: `{ ignoredCount, items }`.
- `HashGenerateBroFileResult { timestamp, jobId, item }` updates a row by hashPath.
- Each item contains `{ sourcePath, hashPath, status, hash, error }`; hash/error
  are nullable and status is `pending | hashing | created | replaced | skipped |
  error | cancelled`.

Result messages update backend job metadata before forwarding. Normal
`JobEvent` byte progress and terminal lifecycle events are reused. Final results
precede the terminal event, allowing the frontend to confirm the saved count
without optimistic file state. Only created/replaced rows count as saved; a
late cancellation never changes an already committed row into cancelled.

---

## Notification service

`PublishNotification` accepts `{ requestId, message, level?, channels? }`.
Levels are `info`, `warning`, and `error`; frontend-requestable channels are
`toast` and `log`. Defaults are `info` and both channels. An explicit
`channels: []` records history without delivery. Backend-selected operation
publications may additionally use the `os` delivery channel; requesting `os`
through `PublishNotification` fails with `INVALID_NOTIFICATION`.
Backend-generated records are
`{ id, timestamp, message, level, channels }`, with UTC millisecond timestamps.

The reply is `PublishNotificationOk { timestamp, requestId, notificationId }`
or `PublishNotificationFailed { timestamp, requestId, code, message }`.
Repeated requests with the same request ID return the retained original record
without publishing again. Invalid input uses `INVALID_NOTIFICATION`.

`ClearNotifications { requestId }` replies with
`ClearNotificationsOk { timestamp, requestId }` or
`ClearNotificationsFailed { timestamp, requestId, code, message }`.
An invalid request uses `INVALID_NOTIFICATION_REQUEST`. Clear emits no notification.

`StateSnapshot.domains.notifications` is `{ version, entries }`, newest first by
publication order. `StatePatch` uses domain `notifications`, no entity ID, and a
monotonically increasing version. The patch is either
`{ added: Notification, removedIds: string[] }` (publication and oldest-entry
eviction together) or `{ clear: true }`. Retention is the latest 1,000 entries in
backend process memory. State clients subscribe atomically with initial history
capture; viewer clients do not receive this domain.
Later requested snapshots deliver pending live notification patches through the
captured notification version before installing the snapshot, preserving toast delivery.

Frontend history mirrors snapshots and ordered patches. Only live added records
requesting toast delivery join the FIFO toast queue. Snapshots do not replay
toasts. Operation acknowledgements, including `*ClipboardWritten`, retain their
wire formats; notification feedback is separately published once at its owner.
`os` channels in records describe requested delivery, not acknowledgement that a
desktop displayed a notification. Frontends render these records without invoking
native delivery or replaying it from snapshots. Native eligibility, focus,
expiry, and operation correlation remain backend-only policy. Settings snapshots
include `general.osNotificationsEnabled` and read-only
`general.supportsOsNotifications`, `general.supportsOsNotificationSettings`, and
`general.osNotificationPlatform`; the preference is changed through
`SetGeneralOption { key: "osNotificationsEnabled", value: boolean }`.
See [notifications.md](./notifications.md).
