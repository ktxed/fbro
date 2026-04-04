# fbro — Bro Feature & Bro Protocol v0

This document defines the `Pass to Bro` feature and the current **bro protocol**
used by fbro.

A **bro** is a processor selected by the user to handle one or more filesystem
entries. The name comes from "browser processor" / "brocessor".

In v0, bros support two execution contracts:

* `fnf` for fire-and-forget launchers
* `fp` for file-producing reducer bros that are assimilated into the job system

---

## 1. User Feature

### 1.1 Entry points

The feature is exposed through:

* `F1` on the focused panel
* the command palette via `Pass to Bro`
* the context menu via `Pass to Bro...`

### 1.2 Input resolution

Input resolution is BE-authoritative.

For panel-scoped invocation (`F1`, command palette):

* if the panel has a non-empty selection, use the selected entries
* otherwise use the entry under the cursor
* the synthetic `..` entry is ignored

For context-menu invocation:

* use the clicked item only
* do not expand to the panel selection

If no valid inputs remain after resolution, the bro lookup fails and the modal
shows an inline error.

### 1.3 Modal behavior

After invocation, FE opens a modal:

* title: `Pass To Bro...`
* shows a compact summary of the resolved target inputs
* requests available bros from BE
* lets the user select one bro and launch it
* renders required launch inputs for bros that declare them

For the built-in `tar` bro:

* FE passes the other panel path as a suggested output directory during bro lookup
* BE uses that suggestion to prefill the archive output path
* the user can edit the archive path before launch

Keyboard behavior:

* `Up` / `Down`: move selection
* `Enter`: launch selected bro
* `Esc`: close modal

Mouse behavior:

* clicking a bro with no required launch inputs launches it immediately
* clicking a bro with launch inputs selects it and keeps the modal open

---

## 2. Core Concepts

### 2.1 Bro

A bro is a user-selectable processor with:

* `id`
* `name`
* `version`
* `source`
* optional `detail`
* `protocol`
* optional launch `inputs`

### 2.2 Brovider

A **brovider** is a BE-side bro discovery source.

In v0, broviders are internal backend registry entries. There is no user
configuration format yet.

### 2.3 Protocol kind

The current protocol kinds are:

* `fnf` = fire-and-forget
* `fp` = file-producing reducer

`fnf` means:

* BE launches the bro directly
* BE does not create a job
* FE does not wait for completion

`fp` means:

* the bro takes `1..N` input entries and produces exactly one output file
* the bro may declare required launch inputs, including the output file path
* BE creates a first-class job instead of detached launch
* FE observes progress or, at minimum, running and terminal job state

---

## 3. v0 Protocol Envelope

The bro protocol is not a separate transport. It is implemented inside the main
FE ↔ BE websocket protocol as intents plus BE messages.

### 3.1 FE → BE intents

#### `ListBros`

Request bro discovery for a panel and optional explicit target.

```json
{
  "type": "Intent",
  "name": "ListBros",
  "scope": "left",
  "args": {
    "requestId": "req-123",
    "targetPath": "/home/user/file.mp4",
    "suggestedOutputDir": "/home/user/archive-output"
  }
}
```

Notes:

* `scope` identifies the panel whose semantic state should be used
* `targetPath` is optional
* `suggestedOutputDir` is optional
* if `targetPath` is omitted, BE resolves selection-or-cursor for that panel

#### `LaunchBro`

Launch the selected bro for a previously resolved bro request.

```json
{
  "type": "Intent",
  "name": "LaunchBro",
  "args": {
    "requestId": "req-123",
    "broId": "native:tar",
    "inputs": {
      "outputPath": "/home/user/archive-output/file.mp4.tar"
    }
  }
}
```

### 3.2 BE → FE messages

#### `BroOptionsReady`

Sent when BE successfully resolves inputs and bro options.

```json
{
  "type": "BroOptionsReady",
  "timestamp": 1710000000000,
  "requestId": "req-123",
  "inputs": [
    {
      "path": "/home/user/a.mp4",
      "name": "a.mp4",
      "kind": "file"
    }
  ],
  "bros": [
    {
      "id": "native:mpv",
      "name": "mpv",
      "version": "0.40.0",
      "source": "native",
      "detail": "/usr/bin/mpv",
      "protocol": "fnf",
      "inputs": []
    }
  ]
}
```

#### `BroOptionsFailed`

Sent when input resolution or bro discovery fails.

```json
{
  "type": "BroOptionsFailed",
  "timestamp": 1710000000000,
  "requestId": "req-123",
  "message": "No valid input entries are selected."
}
```

#### `Error` with `BRO_LAUNCH_FAILED`

Sent only when BE fails to start the chosen bro process.

```json
{
  "type": "Error",
  "timestamp": 1710000000000,
  "code": "BRO_LAUNCH_FAILED",
  "message": "No such file or directory",
  "recoverable": true,
  "extra": {
    "requestId": "req-123",
    "broId": "native:mpv"
  }
}
```

For `fp` bros, launch success is represented through the existing job messages:

* `JobStarted`
* `JobEvent`

`fp` bros do not have a separate bro-specific completion channel.

---

## 4. Data Shapes

### 4.1 `BroInputItem`

```json
{
  "path": "/abs/path/to/item",
  "name": "item",
  "kind": "file | directory | symlink | other"
}
```

### 4.2 `BroItem`

```json
{
  "id": "flatpak:org.videolan.VLC",
  "name": "VLC",
  "version": "3.0.21",
  "source": "flatpak",
  "detail": "org.videolan.VLC",
  "protocol": "fnf",
  "inputs": []
}
```

`fp` example:

```json
{
  "id": "native:tar",
  "name": "tar",
  "version": "1.35",
  "source": "native",
  "detail": "/usr/bin/tar",
  "protocol": "fp",
  "inputs": [
    {
      "id": "outputPath",
      "label": "Archive path",
      "kind": "path",
      "required": true,
      "defaultValue": "/home/user/out/source.tar"
    }
  ]
}
```

### 4.3 `BroLaunchInput`

```json
{
  "id": "outputPath",
  "label": "Archive path",
  "kind": "path",
  "required": true,
  "defaultValue": "/home/user/archive-output/file.mp4.tar"
}
```

Field meanings:

* `id`: opaque backend-selected identifier
* `name`: display name
* `version`: best-effort version string
* `source`: install/discovery source, e.g. `native`, `flatpak`
* `detail`: secondary display info
* `protocol`: current execution contract
* `inputs`: launch-input declarations; on the wire this field is always present and is `[]` when the bro requires no extra inputs

---

## 5. Discovery Model

### 5.1 Cross-platform shape

The feature exists on all supported platforms at the UI/protocol level.

That means:

* FE can open the modal everywhere
* FE and BE speak the same bro messages everywhere
* unsupported platforms may legitimately return an empty bro list

### 5.2 v0 built-in discovery

v0 ships built-in bro discovery for Linux and macOS, excluding Windows.

Current built-in bros by platform:

Linux:

* native `mpv`
* native `vlc`
* Flatpak `io.mpv.Mpv`
* Flatpak `org.videolan.VLC`
* native `tar`

macOS:

* native `mpv` from `PATH`
* native `mpv` from discovered `.app` bundles
* native `tar`

Discovery rules:

* native bros are detected from executables in `PATH`
* Flatpak bros are detected from installed app IDs
* native and Flatpak installs are listed separately if both exist
* version is best-effort and may fall back to `unknown`
* `tar` is only exposed on non-Windows platforms and declares one required launch input: archive output path
* macOS media discovery is currently specialized only for `mpv`

---

## 6. Launch Semantics

### 6.1 Input passing

In v0, fbro passes the resolved input paths **raw** unless the selected bro uses
an alternate launch recipe.

This means:

* no media filtering
* no compatibility gating
* directories and mixed selections are passed through unchanged

The selected bro decides what to do with the provided paths.

The built-in `tar` bro is the first `fp` bro and requires an extra launch input:

* `outputPath`: destination archive path
* BE prefills it from the other panel path when FE provides `suggestedOutputDir`
* `tar` is launched from the parent directory of the selected inputs
* selected entries are passed by entry name so the archive does not capture absolute input paths

### 6.2 Current launch recipes

Current launch forms:

* native `mpv`: `mpv -- <paths...>`
* native `vlc`: `vlc <paths...>`
* Flatpak `mpv` / `VLC`: `flatpak run --file-forwarding <APP_ID> @@ <paths...> @@`
* native `tar`: `tar -cf <outputPath> <entry names...>` with working directory set to the parent directory of the selected inputs

### 6.3 `fnf` contract

After `fnf` launch:

* fbro does not track process lifetime
* fbro does not capture stdout/stderr as protocol output
* fbro does not create a background job entry
* fbro does not refresh panel state unless some unrelated watcher event occurs

### 6.4 `fp` contract

For `fp` bros:

* BE validates the declared output file path before start
* if the output path already exists, BE requests an overwrite-or-skip decision
* BE creates a job and runs the bro in the dedicated FP bro queue
* only one FP bro runs at a time
* regular copy/move/delete jobs remain separate from the FP bro queue
* progress reporting is optional per bro; completion-only reporting is valid

Result handling:

* an FP bro job carries the output file path as result metadata
* on success, FE can reveal the result file in a panel and copy its path
* the preferred result panel is the opposite panel when available, otherwise the source panel

Current built-in `fp` bro:

* `tar`

---

## 7. Backend Responsibilities

For bro requests, BE is responsible for:

* resolving semantic inputs from panel state
* excluding invalid synthetic entries like `..`
* discovering bros from registered broviders
* caching request data by `requestId`
* validating the selected `broId` against the cached request
* validating required launch inputs for bros that declare them
* dispatching `fnf` bros to direct launch
* dispatching `fp` bros into the FP bro queue and job flow

FE must not reimplement selection semantics or derive its own input set.

---

## 8. Future Extensions

The v0 design intentionally leaves room for richer bro protocols later.

Examples:

* bros that require more than one launch input before launch
* bros that return a result synchronously
* progress-aware FP bros with richer reporting than coarse running/completed state
* bros that mutate files and need structured result reporting

Possible future protocol kinds could include:

* blocking request/response
* interactive multi-step bro sessions

Those are explicitly out of scope for v0.
