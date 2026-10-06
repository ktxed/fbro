# fbro - Architecture & Protocol Summary

*fbro* aims to be an N-Panel Orthodox File Manager.
It features by default 2 panels (a penal is a listing of files and folders). The user can move from panel to panel using the tab key.
Once in a panel they can select files and perform operations like copy, delete, compress, etc. On a folder item pressing the enter key will navigate deeper in that folder.


This document summarizes the agreed architecture and interaction model for a personal, opinionated, cross-platform n-panel file manager.

The context-sensitive operation buttons shown beside the cursor entry are
specified in [inline-operations.md](./inline-operations.md).

---

## 1. Frontend (FE) and Backend (BE)

### Shared viewer content

The separate F3 window and Ctrl+F3 quick preview use `FileViewer` for content
loading, automatic renderer detection, chunked scrolling, and rendering.
`ViewerApp` owns native window title/maximize/fullscreen/close behavior;
`EmbeddedViewerPanel` owns cursor-following, panel focus, and its minimal header.
Window URL configuration is parsed by the window host and passed explicitly to
shared content. Existing native media elements, PDF.js, image/HEIC renderers and
Mermaid remain the rendering providers; there is no second preview renderer.

Quick preview omits full-viewer controls and their keyboard handlers. It renders
Markdown by default but keeps file navigation in the source listing. The backend
resolves its target using the same logic as F3. Open viewer opens that displayed
target while retaining quick preview; it does not derive a file from the hidden
panel's selection. See [Keyboard](keyboard.md) and [Protocol](proto.md).

### Markdown viewer images

The backend resolves image paths relative to the viewed Markdown file through
its local VFS driver. Markdown image syntax and HTML `<img>` tags share the
same policy: only verified local raster images within the viewer image size
limit are served. Remote URLs, SVGs, and unsupported paths are not loaded.

HTML fragments are parsed with an HTML5 parser and rebuilt from an allowlist:
`img` supports `src`, escaped `alt`/`title`, and positive integer `width`/`height`;
`p` and `div` support `align="left|center|right"`, rendered as preview CSS
classes. Other HTML elements and attributes are discarded. Raw author HTML
never passes directly to the webview, and Comrak's general raw-HTML option
remains disabled.

The app is composed of a BE part and a FE part. The communicate based on a websocket protocol.
Additional details about the FE - BE protocl are available in [./proto.md](./proto.md)

### Backend (BE) – **Authoritative Core**

The BE is the **single source of truth** and owns all semantic state.

**Responsibilities**
- Panel lifecycle and layout (n-panel graph)
- Directory listings and metadata per panel
- Selection and cursor state
- Focus management
- Virtual File System (VFS) + Platform Abstraction Layer (PAL)
- Job system (copy, move, delete, inspect, scan)
- Validation, permissions, safety rules
- Streaming state updates
- Decision handling (confirmations, conflicts)

**Key rule**
> No filesystem mutation occurs outside a job.

---

### Frontend (FE) – **Stateless Renderer**

The FE is a **non-authoritative projection** of BE state.

**Responsibilities**
- Render panels, layout, previews, jobs
- Display spinners, progress, dialogs
- Map user input (keys, mouse) → intents
- Maintain *ephemeral UI state only*

**FE-only (allowed)**
- Scroll offset
- Hover / drag state
- Virtualization window
- Animation state

**FE must NOT**
- Decide selection semantics
- Mutate filesystem state
- Invent panel or job state
- Apply semantic sorting or filtering

Full-screen dialog presentation, focus, and input ownership follow
[Modal Stack](modal-stack.md).

If FE desyncs → request snapshot.

---

## 2. Intent Model

Intents describe **what the user wants**, never *how* it was triggered.

### Base Intent Envelope

```json
{
  "type": "intent",
  "name": "IntentName",
  "scope": { },
  "args": { },
  "options": { }
}
````

* `scope` → where the intent applies
* `args` → intent-specific data
* `options` → confirmation, backgrounding, dry-run, etc.

---

### Example 1: Toggle selection at cursor (Space)

```json
{
  "type": "intent",
  "name": "ToggleSelectionAtCursor",
  "scope": "focused-panel"
}
```

---

### Example 2: Copy selection between panels (F5)

```json
{
  "type": "intent",
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

This will trigger the creation of a CopyJob job on BE side (see below).
In this case BE will issue a StatePatch message for the jobs domain containing the new job.

example:
```json
{
  "type": "StateSnapshot",
  "domains": {
    "jobs": [
        {     
            "jobId": "job-42",
            "type": "CopyJob",
            "state": "Running"
        }
    ]
  }
}
```

BE can issue entity specific Events e.g. to inform of job progress so that the UI can update progress bars.

---

## 3. Jobs

Jobs represent **long-running or mutating operations**.
They are **interruptible state machines**, not blocking calls.

### Job Lifecycle (minimal)

```text
Created → Running → AwaitingDecision → Running → Completed
                      ↘ Cancelled
```

---

### Job Base Schema

```json
{
  "jobId": "job-42",
  "type": "CopyJob",
  "state": "Running",
  "progress": {
    "current": 10,
    "total": 120
  },
  "context": { }
}
```

---

### Example: Copy Job

```json
{
  "jobId": "job-42",
  "type": "CopyJob",
  "source": "/src",
  "destination": "/dst",
  "state": "Running"
}
```

---

## 4. Job Decision Requests (BE → FE)

When a job encounters ambiguity (e.g. duplicate files), it **pauses** and requests a decision.

### Decision Event

```json
{
  "type": "JobAwaitingDecision",
  "jobId": "job-42",
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

The job transitions to `AwaitingDecision`.

---

### Decision Intent (FE → BE)

```json
{
  "type": "intent",
  "name": "ProvideJobDecision",
  "args": {
    "jobId": "job-42",
    "decision": "overwrite",
    "applyToAll": true
  }
}
```

The BE validates and resumes the job.

---

## 5. BE–FE State Sharing Model

### Core Principles

* BE is authoritative
* FE maintains a **cached projection**
* State updates are **partial, scoped, versioned**
* Snapshots reset state, patches advance it

---

### State Domains

```text
Layout
Panels
Focus
Selections
Jobs
```

Each domain:

* Can be snapshotted independently
* Emits granular updates
* Has monotonic versioning

---

### Initial Snapshot (on connect)

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
    "sources": Source[], // see Source section in the protocol documentation
    "focus": "left",
    "jobs": []
  }
}
```

---

## 6. Panel Listings & Streaming

Panels are loaded via background scan jobs.

### Panel State (simplified)

```json
{
  "panelId": "left",
  "path": "/home/user",
  "status": "loading",
  "entries": [],
  "cursor": 0,
  "selection": []
}
```

### How panels bind to sources (BE-side)

On the BE:

```text
Panel
 ├── sourceId
 ├── currentPath (VFS path)
 ├── filter
 ├── sort
 └── cursor / selection
```

When a panel has:

* `sourceId`
* `currentPath`

The BE knows:

* Which VFS backend to use
* Which operations are legal
* Which job types can be created

The FE does not need to know *how* that works.

---

### Streaming Entries (BE → FE)

```json
{
  "type": "PanelEntriesAppended",
  "panelId": "left",
  "version": 12,
  "entries": [
    {
      "name": "file1.txt",
      "type": "file",
      "size": 1234,
      "mtime": "2026-03-10T10:00:00Z"
    }
  ]
}
```

Entries may arrive incrementally (important for network FS).

---

### Sorting Behavior (Important)

* Sorting is **BE-owned**
* Changing sort order:

  * Cancels current scan
  * Clears panel entries
  * Restarts scan
  * Cursor resets to beginning

This avoids reordering glitches during streaming.

---

## 7. Panel Filtering Intent

Filtering is a semantic operation and must be requested explicitly.

### Example: Filter by filename prefix

```json
{
  "type": "intent",
  "name": "SetPanelFilter",
  "scope": "focused-panel",
  "args": {
    "mode": "prefix",
    "value": "img_"
  }
}
```

**BE behavior**

* Applies filter to panel
* Clears existing entries
* Restarts directory scan
* Streams only matching entries

FE never filters locally.

---

## 8. VFS

Additional details follow in [vfs.md](vfs.md)

The Backend (BE) exposes filesystem-like data through a **Virtual File System (VFS)** abstraction.
The VFS provides a uniform interface over heterogeneous data sources such as local filesystems, mounted volumes, remote filesystems, archives, or virtual datasets.

### Core Principles

* The VFS is **BE-internal** and never exposed directly to the Frontend (FE)
* Each VFS instance is exposed to the FE as a **Source**
* All filesystem access and mutation occurs through the VFS
* The BE is the **single authority** over VFS state and behavior
* The FE never assumes filesystem semantics or capabilities

---

### Sources

A **Source** represents a rooted VFS entry point with a defined set of supported operations.

Conceptually, a Source is:

* A mounted filesystem or volume
* A virtual filesystem (e.g. search results, recent files)
* A remote or network-backed filesystem

Each Source declares the **intents it supports**, allowing the BE to validate all user actions before execution.

Sources are:

* Identified by a stable `id`
* Immutable for the lifetime of a session (v0)
* Shared across panels

Panels always reference a Source by ID.

---

### Panels and VFS Binding

A Panel is a **view** into a Source at a specific VFS path.

A panel is defined by:

* `sourceId`
* Current VFS path
* Sorting and filtering parameters
* Cursor and selection state

The FE does not enumerate or interpret filesystem data directly.
All listing, sorting, filtering, and metadata resolution are performed by the BE via the VFS.

---

### Capabilities and Intent Validation

Each Source advertises a set of supported intents (e.g. copy, delete, move).

When an intent is received:

1. The BE resolves the target panel
2. The panel resolves to a Source
3. The Source’s capabilities are checked
4. Unsupported intents are rejected deterministically

This model:

* Prevents invalid operations
* Enables safe AI-driven actions
* Keeps the FE stateless and simple

---

### Directory Listing and Streaming

Directory listings are performed via VFS drivers and streamed incrementally to the FE.

* Listings may arrive progressively
* Sorting and filtering are applied inside the VFS
* Changing sort or filter cancels the active scan and restarts it
* The BE controls ordering and cursor stability

The FE treats listings as append-only streams and never reorders entries.

---

### Cross-VFS Operations

The VFS abstraction allows operations across different Sources.

* **Move/Rename** is only valid within the same VFS
* **Copy** is supported between any readable and writable VFS pair

Cross-VFS copying is orchestrated by the BE using streamed reads and writes.
Optimized OS-level copies are an implementation detail and not required by the protocol.

---

### Jobs and the VFS

All mutating filesystem operations are executed as **Jobs**.

* Jobs consume VFS operations
* Jobs may be long-running and interruptible
* Jobs may request user decisions (e.g. conflicts)
* VFS drivers never interact with the UI or user directly

The VFS reports errors; the BE translates them into job events or decision requests.

```
