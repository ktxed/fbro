# fbro VFS Specification (FMP) — v0

1. **What a VFS driver is (and is not)**
2. **Core VFS interface (v0)**
3. **Read vs write capabilities**
4. **Cross-VFS copy model (important)**
5. **How jobs use the VFS**
6. **Error & capability contracts**
7. **Why this interface will age well**

---

## 1. What a VFS driver is (conceptually)

A VFS driver is:

> A **pure filesystem-like adapter** that exposes a tree of entries and operations on them, independent of UI, jobs, or protocol.

It is **not**:

* A job runner
* A protocol component
* A panel
* A state store

It **does not**:

* Know about selections
* Know about panels
* Emit UI events

It only:

* Lists
* Reads
* Writes
* Mutates
* Reports errors

---

## 2. VFS Driver Interface (v0)

Below is a **minimal but sufficient** interface.

### 2.1 Driver identity & capabilities

```ts
interface VFSDriver {
  id: string
  name: string
  supports: string[]   // capability flags — see §2.2
}
```

A driver is exposed to panels through a `Source`:

```ts
interface Source {
  id: string
  name: string
  kind: "filesystem" | "virtual"
  supports: string[]
  shadow: boolean
}
```

`kind` answers a single question: **is this a real OS filesystem?**

* `"filesystem"` — local mounted volume. Real paths, watchable, eligible as a copy/move endpoint.
* `"virtual"` — anything else: bookmarks, history, search results, processes, zip/7z archives. Paths are driver-defined opaque strings.

Anything more granular (archive vs flat listing, intra-driver navigation vs handoff) is expressed by **capability flags in `supports`**, not by further kinds.

### 2.2 Capability flags

Capabilities are free-form strings the driver advertises. The BE consults them at decision points; absent capabilities mean the operation is not offered.

Currently defined:

| flag                  | meaning |
|-----------------------|---------|
| `ListEntries`         | Driver can enumerate a directory. |
| `OpenFolder`          | A `directory` entry is a navigable container. |
| `CopySelection`       | Entries can be read as a copy source. |
| `MoveSelection`       | Entries can be moved within the driver. |
| `DeleteSelection`     | Entries can be deleted. |
| `InspectItem`         | Driver provides per-entry metadata beyond `list()`. |
| `InternalNavigation`  | Opening a `directory` entry stays in the same driver (intra-driver tree node) instead of handing off to the local FS driver. Set by archive drivers and procs-hierarchical. |

The flag list is open-ended; new drivers add new flags as needed and gate UI surfaces on their presence.

### 2.3 Shadow overlays

Two related-but-orthogonal "transient" concepts exist alongside `kind`:

* `Source.shadow: bool` — the source itself is transient (not a volume; do not show in the volume bar). Used by zip/7z mounts, procs, and per-job virtual lists.
* `Panel.shadow_source: Option<ShadowSource>` — the panel is currently *inside* an overlay and carries the context needed to exit (parent source id, parent path, optional nested previous shadow). Set when entering an archive or procs; cleared on teardown.

Both are independent of `kind` and of `InternalNavigation`. Search results, for example, are `kind: "virtual"` + `shadow: true` on the source + `shadow_source: Some(...)` on the panel, but *do not* advertise `InternalNavigation` — opening a directory entry hands off to the local FS just like bookmarks.

### 2.4 Global selections source

The global selections source (`global-selections`) is a virtual navigation and
selection tool. It is not an owner of the files it references.

Layout:

* Path `""` lists named global selections as directory entries.
* Path `"<selection name>"` lists that selection's referenced filesystem paths.
* Entries inside a selection carry the original absolute local filesystem path
  as `source_path`.

Deletion semantics are virtual-only:

* Deleting at path `""` removes selected global selection records.
* Deleting inside a named selection removes selected references from that
  selection.
* Neither operation deletes, moves, truncates, or otherwise mutates the
  referenced filesystem paths.
* The driver-level file delete hooks return `NotSupported`; global selection
  deletion is handled by explicit BE intents and store mutations.

The source advertises `DeleteSelection` so the UI can expose deletion of virtual
records and references. That capability must not be interpreted as permission to
delete the referenced real files.

---

## 3. Paths & entries

### 3.1 Path type

Paths are **opaque to the BE**, but stable per driver.

```ts
type VFSPath = string
```

Interpretation:

* POSIX for local FS
* Virtual for others
* Never parsed outside the driver

---

### 3.2 Entry metadata

```ts
interface VFSEntry {
  path: VFSPath
  name: string
  kind: "file" | "directory" | "symlink" | "other"
  targetKind?: "file" | "directory" | "other"
  pathHint?: string
  size?: number
  mtime?: ISO8601
}
```

Rules:

* `size` optional (directories may not know it)
* `mtime` optional
* Never assume filesystem semantics
* Local symlinks retain `kind: "symlink"`; `targetKind` describes a resolvable target and `pathHint` displays its canonical path. An unavailable target has no `targetKind` and shows the stored link target as its hint.
* Local link copies recreate the link with its stored target text, including relative targets. Local delete and rename affect the link itself. `read_link`, `create_symlink`, and `exists_no_follow` are driver hooks used by jobs; unsupported drivers return `NotSupported` for link creation.

---

## 4. Core operations (v0)

### 4.1 List directory (streamable)

```ts
list(
  path: VFSPath,
  options: {
    sort?: SortSpec
    filter?: FilterSpec
    signal?: AbortSignal
  }
): AsyncIterable<VFSEntry>
```

Notes:

* Streaming is natural via `AsyncIterable`
* Cancelled via `AbortSignal`
* Sorting/filtering happens **inside the driver**

---

### 4.2 Stat (single entry)

```ts
stat(path: VFSPath): Promise<VFSEntry>
```

Used for:

* Cursor inspection
* Folder size calculation
* AI queries

---

### 4.3 Read file (streamed)

```ts
openRead(
  path: VFSPath
): AsyncIterable<Uint8Array>
```

Only required for copy & inspect.

---

### 4.4 Write file (streamed)

```ts
openWrite(
  path: VFSPath,
  options: {
    overwrite: boolean
  }
): WritableStream<Uint8Array>
```

The synchronous Rust driver also exposes `open_write_new(path)`, which creates
an entry exclusively and returns `AlreadyExists` for an existing file or link.
Its default is `NotSupported`. Local filesystem drivers implement it using
`OpenOptions::create_new(true)` and the spy driver intercepts/forwards it.
Check hashes report jobs use this hook to resolve filename collisions without
truncating existing entries.

Driver decides:

* Whether overwrite is allowed
* Whether rename is required

---

### 4.5 Delete

```ts
delete(path: VFSPath): Promise<void>
```

Recursive behavior is driver-defined (but must be documented).

For virtual reference sources such as global selections, `DeleteSelection` can
mean "delete the virtual record/reference" rather than "delete the referenced
filesystem object". Such drivers must document this distinction and return
`NotSupported` from raw file delete hooks if invoking them could be mistaken for
deleting real files.

---

### 4.6 Move / rename (same VFS only)

```ts
move(
  source: VFSPath,
  destination: VFSPath
): Promise<void>
```

Rule:

* Only valid **within the same driver**
* Cross-driver moves are copy+delete jobs

---

## 5. Cross-VFS copy (key point)

> Copying should work between *any* write-enabled VFSes.

### 5.1 Copy is NOT a VFS primitive

There is **no**:

```ts
copy(source, destination)
```

at the driver level.

Why?

* Cross-VFS copies are orchestration, not IO
* OS-level optimizations are optional, not required

---

### 5.2 Copy job orchestration

The BE performs:

```text id="epb6b5"
Source VFS
  openRead(path)
    ↓ stream
Destination VFS
  openWrite(path)
```

This means:

* Any readable → any writable VFS works
* Streaming avoids buffering entire files
* Progress tracking is trivial

---

### 5.3 Optional fast-path optimization (later)

You *may* add later:

```ts
copyTo?(
  source: VFSPath,
  targetDriver: VFSDriver,
  targetPath: VFSPath
): Promise<void>
```

But **v0 should not rely on it**.

---

## 6. Jobs & VFS interaction

Local Git repository jobs use `VfsDriver::local_path` to resolve the native
folder before invoking the existing system Git adapter. `is_local` gates this
capability; virtual/remote drivers remain unsupported. The local spy driver
forwards native path mapping to its wrapped driver. Git owns repository metadata
creation, while the backend job owns validation, execution, and panel refresh.

Jobs are **VFS consumers**, never owners.

Example: Copy job loop:

```text id="pt0vib"
for each selected entry:
  if file:
    stream read → write
  if directory:
    mkdir
    recurse
```

When a conflict occurs:

* Job suspends
* Emits `DecisionRequired`
* Waits for FE response
* Resumes with new policy

The VFS driver:

* Never blocks on UI
* Never asks questions
* Just throws errors

---

## 7. Error contract (critical)

VFS drivers must throw **typed errors**.

```ts
class VFSError extends Error {
  code:
    | "NotFound"
    | "PermissionDenied"
    | "AlreadyExists"
    | "NotSupported"
    | "IOError"
}
```

The BE:

* Maps these to JobDecision or Error messages
* Decides whether retry, skip, or abort

---

## 8. Why this interface is correct

This design:

✔ Keeps VFS simple
✔ Keeps BE in control
✔ Enables AI safely
✔ Enables remote FS
✔ Enables streaming
✔ Avoids premature optimization

Most importantly:

> **It forces all complexity into one place: the BE job engine.**

That’s exactly where it belongs.

---

## 9. Mental checkpoint

At this point you have:

* A BE that feels like a *micro-kernel*
* VFS drivers as *device drivers*
* Jobs as *processes*
* FE as a *display server*

That’s not accidental — it’s a sign the abstraction boundaries are right.

---

## 10. Speculative viewer and thumbnail reads

`VfsDriver::is_local()` means native paths, not fast storage. A shared backend
storage registry independently classifies mounted paths, including nested mounts
that are not shown in the source bar. Source snapshots/patches expose
`storageClass` and `prefetchEligible`.

Only `internal-ssd` enables prefetch. `rotational`, `removable-external`, `remote`,
`virtual`, and `unknown` disable it. There are no overrides or disk benchmarks.
Remote fbro sources always disable prefetch regardless of the remote disk type.

Discovery refreshes OS metadata in a background blocking worker every two seconds:
Linux uses mountinfo and sysfs backing devices; macOS uses DiskArbitration and
IOKit; Windows uses volume extents and storage properties. Missing evidence is
unknown. Linux Btrfs/ZFS pools currently remain unknown because a mount device
alone does not prove the characteristics of every pool member.

Viewer jobs check policy before enqueueing and again before reading. Both the
lexical path and canonical target must qualify. Registry changes invalidate queued
viewer work, cached viewer bytes, and speculative permits; reads already in
progress may finish, but their stale results are discarded.

Thumbnail warmups use `?prefetch=1`. The HTTP handler and thumbnail worker enforce
the same policy; rejected warmups return non-cacheable HTTP 204. Ordinary requested
images, visible grid thumbnails, and requested previews remain available on every
source. Classification changes and skipped warmups are logged.

### Full-file text search in the viewer

Find in a fully loaded small text file remains an in-memory frontend operation.
For larger files, find must not be limited to the viewer's current 64 KiB display
window. The frontend sends `FindInViewerFile`; the backend streams the file through
`VfsDriver::open_read()` and performs a case-insensitive literal scan. This keeps the
feature independent of an installed `rg` executable and makes it work for local,
remote, archive, and other readable VFS sources.

Each occurrence is represented by its absolute byte offset, byte length, and
one-based line number. Results are sent in batches of 256 and capped at 10,000;
the response marks a capped result set as truncated. Byte offsets drive the
existing chunk loader when the user selects the next or previous occurrence,
while line numbers restore useful gutter context after an arbitrary jump. The
scanner carries matching state across read boundaries, so an occurrence split
between two backend read buffers is still found. Queries are debounced in the
frontend, and request ids ensure results from superseded searches are ignored.

## Suggested next step

Two very natural next steps:

1. **Formal job types** (`ScanDirectoryJob`, `CopyJob`, `DeleteJob`)
2. **Conflict resolution policy model** (per-job vs global)

If you want, we can spec one of those next.

Generate hashes reuses `open_write_new` for sidecar creation and same-folder
staging. Overwrite mode flushes and closes the staged writer before
`try_rename`; it never falls back to deleting or truncating an existing sidecar
when atomic placement fails. Failed/cancelled writes remove only paths owned
by that job through `remove_file`.

### Transfer cancellation errors

`VfsError::Cancelled` is an intentional, non-retryable cancellation. SFTP drivers
preserve this typed error inside synchronous `Read` I/O errors; copy jobs inspect
the type rather than matching text. Cancellation bypasses auto-resume and
verification decisions. Interrupted fresh copies follow the keep-partials choice;
resumed copies retain their existing partial. `begin_file_op`/`end_file_op` scope
SFTP UUID transfer identities and dedicated connections; connection replacement
retains the identity, and operation completion releases it.
