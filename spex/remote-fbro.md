# Remote fbro

Two fbro instances can pair over the network so that one browses, copies from,
and validates the other's filesystem. This document describes the remote
transport and the three features built on top of it: **Sync Bro** (integrity
compare), **streaming thumbnails**, and **resumable transfers**.

See also: [spec.md](./spec.md) (overall architecture), [vfs.md](./vfs.md) (the
driver abstraction), [proto.md](./proto.md) (FE↔BE protocol), and
[thumbnails.md](./thumbnails.md) (local thumbnail generation).

---

## 1. Transport

Remote fbro is **SFTP over SSH**, with both ends embedded in the app — there is
no system `sshd` and no external `ssh`/`sftp` binary.

- **Crates:** `russh` (SSH) + `russh-sftp` (SFTP subsystem), both client and
  server. Integrity hashing uses `blake3`.
- **Server:** `FbroServer` / `SftpHandler` in `src-tauri/src/remote/server.rs`
  expose the local sources over SFTP. Default port `DEFAULT_PORT = 7879`.
- **Client:** `SftpFsDriver` in `src-tauri/src/vfs/sftp.rs` is a normal
  `VfsDriver`, so a mounted remote source behaves like any other panel source.
- **Discovery & pairing:** peers are found via mDNS (`_fbro-remote._tcp.local`,
  `src-tauri/src/remote/discovery.rs`). Pairing is an explicit confirm-on-both-
  ends handshake; authentication is **Ed25519 public key** (no passwords).
  The client accepts only the paired raw Ed25519 host key and rejects SSH
  certificates. RSA support is disabled in the embedded SSH library.
- **Read-only by default:** a mounted remote source advertises `readOnly` and
  does not expose write capabilities (`DeleteSelection`, `NewFolder`, …).

### Connection model

- **Shared session** for browsing/listing/reads: a single persistent SFTP
  session managed by a supervisor task with ~10 s heartbeats and automatic
  reconnect (exponential backoff, capped at 30 s).
- **Per-job connections** for file operations: each copy/transfer job gets its
  own dedicated SFTP connection (`job_sftp` / `drop_job_connection`, scoped by
  `begin_file_op` / `end_file_op` and the `CURRENT_FILE_OP_JOB` thread-local) so
  concurrent jobs don't contend on one channel.
- **Read-ahead streaming:** single-file reads are prefetched by
  `run_read_ahead` using `SFTP_READ_AHEAD_CHUNKS = 16` workers of
  `SFTP_READ_CHUNK_SIZE = 256 KiB`, reordered through a `BTreeMap` and emitted
  in offset order.

### Control-path convention

The remote peer is not a generic SFTP server. `SftpHandler::open` inspects the
requested path first: if it matches a `/.fbro-*` control path it performs an
operation and returns the result as in-memory bytes (`HandleState::ControlData`)
instead of opening a real file. This keeps everything inside the one SFTP
channel — no extra ports, no exec channel.

| Control path | Purpose |
|---|---|
| `/.fbro-sources.json` | List shared sources + capabilities (readOnly, thumbnails, manifest support) |
| `/.fbro-thumbnail/{size}/{b64 path}` | Generate a PNG thumbnail on the remote (streaming thumbnails) |
| `/.fbro-manifest/start/{b64 path}` | Begin building an integrity manifest on the remote (Sync Bro) |
| `/.fbro-manifest/status/{job_id}` | Poll manifest progress / collect the finished manifest |
| `/.fbro-manifest/cancel/{job_id}` | Cancel a remote manifest job |
| `/.fbro-transfer-context/{token}` | Tag the current read session with a transfer identity |
| `/.fbro-transfer-status/{token}` | Read `{ "cancelled": boolean }` for this peer’s transfer |
| `/.fbro-integrity/result/{token}/{b64 path}` | Read the server's streaming digest for a transfer |
| `/.fbro-integrity/file/{b64 path}` | Read a full-file blake3 digest of a remote file |

Paths are base64-url-no-pad encoded. The digest algorithm is advertised as
`blake3` (`FBRO_INTEGRITY_ALGORITHM`).

---

## 2. Sync Bro — integrity compare

**Goal:** prove that a local directory tree and a remote directory tree hold
byte-identical files — i.e. that a transfer landed correctly. It is the built-in
bro `builtin:sync-bro`, offered when one panel is local and the other remote.

Entry points: `launch_sync_bro` (`src-tauri/src/ws/intents/bro.rs`) →
`run_sync_bro` (`src-tauri/src/jobs/integrity_compare.rs`).

### How it works

1. **Two manifests, built concurrently.** `run_sync_bro` spawns two
   `spawn_blocking` workers — one per side — and builds both manifests in
   parallel. A manifest is a sorted list of `{ path, size, blake3 }` entries.
2. **The remote tree is hashed on the remote host.** For the remote side the
   client calls `driver.remote_manifest(...)`, which drives the
   `/.fbro-manifest/start|status|cancel` control paths. The remote walks and
   hashes **its own files** (`build_manifest_on_remote`) and returns only the
   manifest — **file bytes never cross the wire**. The client polls status every
   `REMOTE_MANIFEST_POLL_INTERVAL = 500 ms` (stale guard
   `REMOTE_MANIFEST_STALE_TIMEOUT = 60 s`). If the remote does not advertise
   manifest support the remote side fails fast rather than streaming bytes back
   to hash locally.
3. **Both sides use multi-core hashing.** Real local files (on either host) are
   hashed by `hash_local_file_blake3` (`src-tauri/src/vfs/mod.rs`): the file is
   memory-mapped and hashed in 64 MiB windows with `blake3::Hasher::update_rayon`
   (all cores), checking cancellation and reporting byte progress between
   windows. Drivers without a real local path fall back to a single buffered
   streaming read. This is the main reason large-file syncs are fast.
4. **Compare and classify.** `compare_manifests` produces a `CompareSummary`:
   - `matched` — same size and hash
   - `differing` — present on both sides but size/hash differ (carries both
     sizes and both hashes)
   - `local_only` / `remote_only` — present on one side only
   - `unverified` — a hash error on one side prevented comparison
   - `local_errors` / `remote_errors` — files that could not be hashed

### Results in the modal; report file is optional

The summary is sent to the FE as a dedicated `SyncBroResult` message
(`src-tauri/src/ws/protocol.rs`) just before the job's `Completed` event, and
rendered in the job modal (`OperationModal.tsx`): headline counts plus scrollable
categorized lists. **No report file is written automatically.** A "Save report…"
button round-trips the result through the `SaveSyncReport` intent; the backend
formats it (`format_sync_report_from_schema`, reusing the single `format_report`
formatter) and writes `fbro-sync-report.txt` into the local folder, then confirms
with a `SyncReportSaved` message.

E2E coverage lives in `tests/e2e/smoke/cases/remote.mjs` (launch Sync Bro, assert
the structured result arrives with no auto-written file, then save on demand).

---

## 3. Streaming thumbnails

A remote panel in grid view shows real thumbnails without downloading whole
files. See [thumbnails.md](./thumbnails.md) for how thumbnails are generated
locally; this section covers the remote path.

### Server side

`/.fbro-thumbnail/{size}/{b64 path}` is parsed by `thumbnail_request_from_path`
(size clamped to `[32, 512]`). `thumbnail_bytes` resolves the source driver,
checks `settings.general.stream_thumbnails` and `driver.supports_thumbnails()`,
then calls the platform thumbnailer (`platform.get_thumbnail`, e.g.
`src-tauri/src/platform/linux/thumbnail.rs`) on a blocking task and returns PNG
bytes. The bytes are served from memory via the control-data handle. Remote
thumbnail support is advertised per-source in `/.fbro-sources.json` as
`supports_thumbnails = stream_thumbnails && driver.supports_thumbnails()`.

### Client side

The FE never opens the SFTP control path directly. Instead it points `<img>` at
the **local HTTP fileserver** (`src-tauri/src/fileserver.rs`):

```
http://127.0.0.1:{fileServerPort}/__remote_thumb__/{sourceId}/{size}/{mtime}/{fileSize}/{relPath}
```

`handle_remote_thumb` checks a disk cache (`fbro/remote-thumbnails`, keyed by
`blake3(sourceId + path + mtime + fileSize + size)`, ~7-day TTL); on a miss it
fetches the PNG via `SftpFsDriver::remote_thumbnail` (which opens the
`/.fbro-thumbnail/...` control path) and caches it. `mtime`/`fileSize` are in the
key so edits invalidate the cache. Local thumbnails use the sibling
`/__thumb__/{size}/{path}` route.

The FE uses this only when `panel.viewMode === "grid" && panel.supportsThumbnails`
(see `src/components/Panel.tsx`, grid size `GRID_THUMB_SIZE = 128`).

---

## 4. Resumable transfers

Copying between hosts must survive interruptions (network drops, cancel) and
prove the result is byte-correct. Two flows exist:

- **Pull copy** — a normal copy job whose source is a remote driver. Driven by
  `copy_file_with_verification` in `src-tauri/src/jobs/file_ops.rs`.
- **Outgoing transfer** — the *remote* peer pushing files to us. Tracked
  server-side as `OutgoingTransfer` (`JobKind::OutgoingTransfer`,
  `src-tauri/src/remote/server.rs`); a transfer is lazily created when the peer
  first opens one of our files and is keyed to a job via the transfer-context
  marker.

### Resume

`resume_candidate` (`file_ops.rs`) decides whether an interrupted copy can
continue: it requires `src.supports_read_at()` and `dst.supports_write_at()` and
a destination that is non-empty but smaller than the source. The resume offset
**rewinds `COPY_RESUME_REWIND_BYTES = 1 MiB`** from the destination size to
re-hash and overwrite any partially written tail, then reads with
`open_read_at(src, offset)` and writes with `open_write_at(dst, offset)`. On the
SFTP client, `open_read_at` spawns the read-ahead workers starting at `offset`.

### Verification (triple digest)

A completed transfer is verified by comparing **three** blake3 digests of the
same bytes (`compare_transfer_digests`):

1. **Remote full-file digest** — read from `/.fbro-integrity/file/{b64 path}`
   (`file_digest`).
2. **Server streaming digest** — computed on the remote *as it serves the bytes*
   and read from `/.fbro-integrity/result/{token}/{b64 path}` (`transfer_digest`).
   The server accumulates this with `OrderedTransferDigest`, which buffers
   out-of-order chunks in a `BTreeMap`, hashes contiguous runs (`drain_contiguous`),
   and finalizes once every byte is covered.
3. **Local destination digest** — hashed from the file just written.

Size and hash must agree across all three. The streaming digest is tied to the
job by the `/.fbro-transfer-context/{token}` marker, which the client opens
(`mark_remote_transfer_context`) before reading so the server knows which
transfer to accumulate.

### Decisions, cancellation, and partial files

- **Conflicts & verification failures** are surfaced to the FE as
  `DecisionRequired` events carrying a `ConflictDecisionSchema`. File conflicts
  offer `resume` (when a resume candidate exists), `overwrite`, `skip`, `cancel`;
  a verification mismatch uses `kind: "verification"` with `retry` / `skip` /
  `abort`. Conflict policy may also be preset (`ask` / `skip` / `overwrite` /
  `resume`).
- **Outgoing cancellation is final on both peers.** Each client operation owns a
  UUID token, stable across retries/reconnects and distinct from its local job id.
  Cancellation records `(authenticated peer fingerprint, client transfer key)`
  in shared backend memory for the app lifetime, including after UI job removal.
  Further opens/reads with that identity return SFTP `PermissionDenied`; a status
  control read reports cancellation without registering or restarting a transfer.
  The client checks status before attempts and after stream errors, preserves a
  typed cancellation through read-ahead, stops its workers, and reports `Cancelled`
  without automatic resume or a verification prompt. Blocking reads and remote
  verification hashing observe cancellation before returning their result.
  Browsing, other transfers, and queued jobs continue; a new copy uses a fresh token.
  Older servers returning `NoSuchFile`/`OpUnsupported` for status retain legacy
  behavior. Both peers must be updated for reliable cancellation propagation.
  This reuses the existing SFTP control mechanism: SFTP v3 has no cancellation
  status, and russh-sftp’s server handler cannot attach a custom status message.
- **Cancel keeps or deletes partials.** A job carries
  `keep_partial_files_on_cancel`. For remote copies the cancel dialog offers
  *Keep Partials* vs *Delete Partials*. Rule: a freshly started copy
  (`resume_offset == 0`) with keep-partials off **deletes** the incomplete
  destination; a resumed copy (`resume_offset > 0`) is **always kept** so it can
  be resumed again.

---

## 5. Relevant settings

### Shared folders

Settings → Sharing retains the source/Home/bookmark checkboxes and adds a
Search-style path input, suggestions select, and Add button. Current folder is
offered from the active local filesystem listing; source/Home/bookmark
suggestions come from the backend. Selecting a suggestion fills the input.
Enter or Add submits once, shows “Adding…”, and waits for backend confirmation.
Failures retain the entered path and show an inline error. Input history uses
the existing Alt+Down interaction; Escape dismisses history before Settings.
Folder configuration remains available while remote access is off.

`AddSharedFolder` validates an existing, enumerable local directory with the
running user's actual permissions on a blocking worker. `~` expands to Home;
relative paths resolve against the supplied local panel, otherwise Home.
Canonical-equivalent existing shares are reused. New roots have stable hashed
IDs, basename labels, and `origin: "folder"`. Their local VFS drivers are
internal registry entries, independent of mounted volume Sources. Peers receive
the same read-only SFTP capabilities as existing shares, including thumbnails,
integrity manifests, and transfers. Normal filesystem permissions apply to
each child read; validation does not recursively preflight the tree.

Added folders form a separate checkbox group. Unchecking removes the root and
its internal driver. Version-2 share.json persists folder roots alongside older
source/Home/bookmark roots; startup recreates the drivers even if a saved folder
is temporarily unavailable. Such folders remain removable in Settings, and
remote access fails normally until their paths become accessible again.

Sharing patches broadcast to GUI/observer connections with increasing versions;
clients discard older sharing patches. New roots appear on the peer's next
Connect/Retry last; existing remote mounts do not automatically discover shares.

- `settings.general.stream_thumbnails` — gates remote (and local) thumbnail
  generation; when off, peers advertise no thumbnail support.
- Sharing toggles (`EnableSharing`, `SetSharedTargets`, `SetSharedSources`,
  IPv6) control which local sources are exposed to paired peers.
- Conflict policy and verification policy are passed when a copy job is started.

---

## 6. Troubleshooting: mDNS discovery

If a peer (e.g. a Mac) cannot see a Linux instance, the usual cause is the
firewall blocking mDNS re-announcements. Allow UDP 5353:

```sh
# UFW
sudo ufw allow 5353/udp comment 'mDNS'

# or, if firewalld is in play
sudo firewall-cmd --permanent --add-service=mdns
sudo firewall-cmd --reload
```
