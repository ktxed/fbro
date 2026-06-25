# fbro — Bro Feature & Bro Protocol v0

This document defines the `Pass to Bro` feature and the current **bro protocol**
used by fbro.

A **bro** is a processor selected by the user to handle one or more filesystem
entries. The name comes from "browser processor" / "brocessor".

In v0, bros support two execution contracts:

* `fnf` for fire-and-forget launchers
* `fp` for file-producing reducer bros that are assimilated into the job system

The bro discovery pipeline also includes an initial **pre-bro** advisory pass
that scores and sorts discovered bros against the resolved file inputs.

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
* renders first-class output fields for `fp` bros
* renders additional bro-declared params for bros that declare them

For built-in archive bros like `tar` and `7z`:

* FE passes the other panel path as a suggested output directory during bro lookup
* BE uses that suggestion to prefill the output archive path
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
* `pre`
* optional first-class `output`
* optional bro-declared `params`

### 2.2 Brovider

A **brovider** is a BE-side bro discovery source.

In v0, broviders are internal backend registry entries. There is no user
configuration format yet.

### 2.3 Pre-Bro

A **pre-bro** is the advisory stage that runs after bro discovery and before the
bro list is shown to FE.

It is consultative, not authoritative.

Its current responsibilities are:

* score each discovered bro against the resolved file inputs
* attach a short human-readable fit reason
* sort the bro list before it reaches FE

It does not:

* block launch
* recurse deeply into large selections
* promise that a bro will succeed

### 2.4 Protocol kind

The current protocol kinds are:

* `fnf` = fire-and-forget
* `fp` = file-producing reducer

`fnf` means:

* BE launches the bro directly
* BE does not create a job
* FE does not wait for completion

`fp` means:

* the bro takes `1..N` input entries and produces exactly one output file
* the output file is first-class protocol data
* the bro may additionally declare launch params
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
    "broId": "native:7z",
    "output": "/home/user/archive-output/file.mp4.7z",
    "params": {
      "compressionLevel": "5",
      "password": "secret"
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
      "pre": {
        "score": 100,
        "reason": "Strong fit for this media file"
      },
      "params": []
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
  "pre": {
    "score": 100,
    "reason": "Strong fit for this media file"
  },
  "params": []
}
```

`fp` example:

```json
{
  "id": "native:7z",
  "name": "7z",
  "version": "unknown",
  "source": "native",
  "detail": "/usr/bin/7z",
  "protocol": "fp",
  "pre": {
    "score": 98,
    "reason": "Strong fit for archiving this selection"
  },
  "output": {
    "label": "Archive path",
    "defaultValue": "/home/user/out/source.7z"
  },
  "params": [
    {
      "id": "compressionLevel",
      "label": "Compression level",
      "kind": "int",
      "required": true,
      "defaultValue": "5",
      "minValue": 0,
      "maxValue": 9
    },
    {
      "id": "password",
      "label": "Password",
      "kind": "password",
      "required": false
    }
  ]
}
```

### 4.3 `BroOutput`

```json
{
  "label": "Archive path",
  "defaultValue": "/home/user/archive-output/file.mp4.7z"
}
```

### 4.4 `BroParam`

```json
{
  "id": "compressionLevel",
  "label": "Compression level",
  "kind": "int",
  "required": true,
  "defaultValue": "5",
  "minValue": 0,
  "maxValue": 9
}
```

Field meanings:

* `id`: opaque backend-selected identifier
* `name`: display name
* `version`: best-effort version string
* `source`: install/discovery source, e.g. `native`, `flatpak`
* `detail`: secondary display info
* `protocol`: current execution contract
* `pre`: advisory pre-bro score + reason used to sort and explain the bro list
* `output`: first-class output-file declaration for `fp` bros
* `params`: bro-declared launch params; on the wire this field is always present and is `[]` when the bro requires no extra params

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
* built-in `Fedup Bro`
* native `7z`
* native `tar`

macOS:

* native `mpv` from `PATH`
* native `mpv` from discovered `.app` bundles
* built-in `Fedup Bro`
* native `7z`
* native `tar`

Discovery rules:

* native bros are detected from executables in `PATH`
* Flatpak bros are detected from installed app IDs
* native and Flatpak installs are listed separately if both exist
* version is best-effort and may fall back to `unknown`
* `tar` and `7z` are only exposed on non-Windows platforms
* `7z` is an external native dependency, not a bundled tool
* `7z` probes native binaries in `PATH` with fallback order `7z`, `7zz`, `7za`, `7zr`
* `7z` also probes common Homebrew install paths on macOS when GUI PATH resolution is thin
* macOS media discovery is currently specialized only for `mpv`

Operational note:

* on systems where `7z` is expected, it must be installed separately
* on macOS, `brew install sevenzip` is the intended setup path

After discovery, pre-bro scoring sorts the list cheaply from the resolved
selection shape:

* media bros rank high for obvious media selections
* archive bros rank high for multi-file or directory-heavy selections
* Fedup Bro ranks high for global selections with multiple files or folders
* weak fits are still shown with a lower score and an explanatory reason

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

For `fp` bros:

* the output path is first-class protocol data
* BE prefills it from the other panel path when FE provides `suggestedOutputDir`
* the process is launched from the parent directory of the selected inputs
* selected entries are passed by entry name so archives do not capture absolute input paths

### 6.2 Current launch recipes

Current launch forms:

* native `mpv`: `mpv -- <paths...>`
* native `vlc`: `vlc <paths...>`
* Flatpak `mpv` / `VLC`: `flatpak run --file-forwarding <APP_ID> @@ <paths...> @@`
* native `tar`: `tar -cf <outputPath> <entry names...>` with working directory set to the parent directory of the selected inputs
* native `7z`: `7z a -mx<level> [-p<password>] <outputPath> -- <entry names...>` with working directory set to the parent directory of the selected inputs
* built-in `Fedup Bro`: backend job that scans a global selection for exact duplicate files and returns a structured report

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

* `7z`
* `tar`

---

## 7. Built-In Operation Bros

Some built-in bros are shown in the bro modal but are not simple command
launchers and are not `fp` archive-style reducers. They are backend operation
bros: FE launches them through the same `LaunchBro` intent, then BE creates a
first-class job and sends job/result messages.

In the current wire shape these bros may still advertise `protocol: "fnf"` so
the v0 modal can launch them without first-class output fields. The launch
recipe, not the public protocol string alone, decides whether BE starts a
custom job.

### 7.1 Fedup Bro

Fedup Bro is the built-in duplicate finder. It is intentionally the most
stateful built-in bro because it scans trees, hashes files, aggregates
structured results, and can later grow mutation actions such as move/delete.

#### Discovery and invocation

Fedup Bro is exposed only from the global selections source in v1.

Expected workflow:

* user adds one or more folders/files to a global selection
* user invokes `Pass to Bro` from the global selections root
* BE resolves exactly one selected global selection to its referenced paths
* Fedup Bro appears as `builtin:fedup`
* launching it starts a `fedup-bro` job

The restriction to global selections is deliberate. It matches the target
workflow and avoids implying that the first implementation is fully VFS-aware
for arbitrary panel sources.

#### Scan semantics

Fedup Bro report mode:

* recursively walks local directory inputs
* accepts local file inputs
* ignores non-file entries
* ignores zero-byte files
* records unreadable paths as per-path errors instead of failing the whole job
* buckets files by byte size
* hashes only size buckets containing more than one file
* uses full-file BLAKE3 hashes, not sample hashes
* groups files with identical size and identical BLAKE3 hash
* chooses one keeper per group using oldest modification time, with path order
  as deterministic tie-break

The full-file hash is the safety boundary for any future mutation action.
Approximate/sample hashing may be added later only as an explicit fast mode,
and destructive actions must still verify exact content.

#### Job and progress

Launching Fedup Bro sends a normal `JobStarted` message:

```json
{
  "kind": "fedup-bro",
  "opType": "Fedup Bro",
  "initialState": "Running",
  "supportsPause": false
}
```

Progress is sent through normal `JobEvent { event: "Progress" }` messages.
During scanning, progress is coarse. During hashing, progress includes the
current file and byte-level progress for that file when available.

Cancellation uses the regular `CancelJob` intent and terminates the job with a
normal `JobEvent { event: "Cancelled" }`.

#### Result shape

Fedup Bro sends a bro-specific result before terminal completion:

```json
{
  "type": "FedupBroResult",
  "jobId": "job-12",
  "result": {
    "duplicateGroups": 1,
    "duplicateFiles": 2,
    "reclaimableBytes": 1048576,
    "scannedFiles": 24,
    "scannedBytes": 93218422,
    "groups": [
      {
        "hash": "b23f...",
        "size": 524288,
        "keeper": {
          "path": "/home/user/Documents/a.txt",
          "size": 524288,
          "mtime": 1710000000,
          "hash": "b23f..."
        },
        "duplicates": [
          {
            "path": "/home/user/Documents/copy/a.txt",
            "size": 524288,
            "mtime": 1710000300,
            "hash": "b23f..."
          }
        ]
      }
    ],
    "errors": [
      {
        "path": "/home/user/Documents/private",
        "message": "Cannot read directory: Permission denied"
      }
    ]
  }
}
```

FE stores this result on the job and renders a report view with:

* duplicate group count
* duplicate file count
* reclaimable bytes
* scanned file count
* scan/hash errors
* keeper and duplicate paths for the first result groups

#### Saving reports

Fedup Bro does not write a report automatically on completion. The completed
job modal offers `Save report...`, which sends:

```json
{
  "type": "Intent",
  "name": "SaveFedupReport",
  "args": {
    "jobId": "job-12",
    "result": { "...": "FedupBroResult payload" }
  }
}
```

BE formats the current result as text and writes `fbro-fedup-report.txt`.

Default destination:

* if the first resolved input is a directory, save inside that directory
* otherwise save beside the first resolved file

Examples:

* first input `/home/user/Documents` -> `/home/user/Documents/fbro-fedup-report.txt`
* first input `/home/user/Documents/a.txt` -> `/home/user/Documents/fbro-fedup-report.txt`

On success BE sends:

```json
{
  "type": "FedupReportSaved",
  "jobId": "job-12",
  "path": "/home/user/Documents/fbro-fedup-report.txt"
}
```

FE stores the saved path on the job and shows the normal transient save hint.

#### Current limits

Phase 1 is report-only.

Not implemented yet:

* moving duplicates to a destination folder
* deleting duplicates
* user-selectable keeper policy
* directory picker or bro param UI for Fedup actions
* remote/archive/global-VFS mutation support
* automatic report writing

Future move/delete modes must be implemented as explicit mutation actions, not
as side effects of report mode.

---

## 8. Backend Responsibilities

For bro requests, BE is responsible for:

* resolving semantic inputs from panel state
* excluding invalid synthetic entries like `..`
* discovering bros from registered broviders
* caching request data by `requestId`
* validating the selected `broId` against the cached request
* validating first-class output for `fp` bros
* validating bro-declared params for bros that declare them
* dispatching `fnf` bros to direct launch
* dispatching `fp` bros into the FP bro queue and job flow
* dispatching built-in operation bros such as Fedup Bro into their custom job
  flow
* preserving BE-authoritative input resolution for global selections

FE must not reimplement selection semantics or derive its own input set.

---

## 9. Future Extensions

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
