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
* No binary frames in v0

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
focus
selections
jobs
```

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

