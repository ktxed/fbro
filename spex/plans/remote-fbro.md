# Remote fbro — Implementation Plan (SFTP over embedded russh)

## Context

Today fbro is a single-machine file manager. The user runs it on both Linux
and Mac and wants a unified experience: discover the other instance over the
LAN, pair once, then browse and copy files from it as if it were a local
volume — with the embedded viewer streaming video/images smoothly.

**Approach**: don't reinvent any protocol. Use **SFTP** for all file
operations, transported over an **embedded SSH server** (`russh` +
`russh-sftp`) that both peers run in-process. Discovery is **mDNS**. Trust
is a **symmetric 6-digit code pairing** — no TOFU, no editing of system
`~/.ssh/` files, no system `sshd` involvement.

v1 scope is **fbro ↔ fbro only**. Plain-sshd / Termux / Mac-Remote-Login
reach is deferred to v2 (it reintroduces TOFU and a second mDNS service
type — straightforward but additive work).

Read-only in v1 (browse + view + copy *out*). Writes are one-line additions
per SFTP method when we decide to enable them.

## Decisions (locked)

| Concern | Choice |
|---|---|
| v1 scope | fbro ↔ fbro only. Plain-sshd / Termux deferred. |
| Wire protocol | SFTP (RFC draft, `russh-sftp` impl). |
| Transport | SSH via `russh`, embedded in fbro on both ends. |
| Server side on peer | Embedded `russh` listener on port 7879 in fbro's process. No system `sshd`. |
| Rust crates | `russh`, `russh-sftp`, `russh-keys`. (No `openssh`, no `openssh-sftp-client`.) |
| Auth | Ed25519 pubkey installed at pairing time. No agent, no password, no key-path picker. |
| Trust establishment | Symmetric 6-digit code pairing (HKDF over both pubkeys + nonces). No TOFU. |
| Discovery | mDNS, service type `_fbro-remote._tcp.local`, TXT `fbro=1`, `name`, `os`. |
| Persistence | Paired peers → `~/.config/fbro/peers.json`. Discovery state volatile. |
| Path scoping | Source-level: server roots SFTP at `/`, one dir per shared source. Path traversal rejected. |
| Capabilities | Read-only v1: explicit `read_only: bool` on `Source`; `supports = [ListEntries, OpenFolder, CopySelection, InspectItem]`. |

## Architecture

```
┌── Linux fbro (client) ───────┐         ┌── Mac fbro (peer) ────────┐
│  Volume bar → "liviu-mbp"    │         │  embedded russh server    │
│       ▼                      │         │   on :7879                │
│  SftpFsDriver (VfsDriver)    │   SSH   │   └─ SFTP subsystem       │
│       ▼                      │ ──────► │      (russh-sftp)         │
│  russh client                │  :7879  │   └─ wraps VfsDriver      │
│       ▼                      │         │      per shared source    │
│  russh-sftp client           │         │                           │
└──────────────────────────────┘         └───────────────────────────┘
```

The same fbro binary is both server and client. On startup it generates an
Ed25519 identity at `~/.config/fbro/id_ed25519`; that key is the server's
host key *and* the client's identity when connecting out. Pairing exchanges
public halves once; afterward both ends authenticate automatically by key.

The client driver is sync (matching the `VfsDriver` trait); it bridges to
the async `russh` stack via a dedicated tokio runtime owned by the driver
(same pattern any networked driver would use; see `vfs/search.rs` for a
simpler in-tree precedent that uses `futures::executor::block_on()` on a
shared async store).

## Phases

### Phase 1 — `SftpFsDriver` (client side)

New file: `src-tauri/src/vfs/sftp.rs` (~300 lines).

- Holds a `Connection`: a `russh::client::Handle` + a `russh_sftp::client::SftpSession`,
  plus a path→open-handle LRU cache (size 16) for `read_chunk` so repeated
  viewer reads don't reopen the same file.
- A dedicated single-thread tokio runtime owned by the driver. Sync trait
  methods send a request enum over an mpsc to the runtime task and
  `block_on` a oneshot reply. (Justification over the simpler
  `block_on(shared_store)` pattern of `vfs/search.rs`: this driver owns a
  long-lived network connection, manages reconnect/backoff state, and needs
  to cancel in-flight operations on disconnect.)
- Reconnect on disconnect with exponential backoff (1, 2, 4, 8, 16 s, then
  hold). While disconnected, all method calls return `VfsError::IOError`;
  the source shows as "disconnected" in the UI.

Trait-method mapping (against `src-tauri/src/vfs/mod.rs:20-83`):

| `VfsDriver` | SFTP call |
|---|---|
| `is_local()` | `false` |
| `supports_thumbnails()` | `false` (default) |
| `as_any()` | `self` — required, no default; codebase downcasts elsewhere |
| `list(path, sort, filter, show_hidden)` | `read_dir(path)` → collect; sort/filter/hidden applied locally |
| `resolve_child(base, name)` | string join with `/` (SFTP paths are POSIX) |
| `parent_path(path)` | string-level parent |
| `exists` / `file_size` / metadata via list | `stat(path)` / `metadata(path)` |
| `read_dir_entries(path)` | `read_dir(path)` projected to `{name, kind, size, mtime, …}` |
| `read_chunk(path, offset, size)` | open-or-cached handle → `read_at(offset, size)` |
| `open_read(path)` | `open(path)` returning a chunked sync `Read` adapter that issues `read_at` calls in 1 MiB chunks |
| `open_write` / `remove_*` / `create_dir_all` / `try_rename` | `Err(VfsError::NotSupported)` |

`read_chunk` is a real trait method (load-bearing for the viewer) — note it
is not yet listed in `spex/vfs.md`, which still describes the older async
streaming model. The trait is the authoritative contract.

Critical existing files referenced (no changes needed):

- `src-tauri/src/vfs/mod.rs:20-83` — trait definition we implement against.
- `src-tauri/src/ws/intents/viewer.rs:~442` onward — routes
  `is_local()=false` through `ReadFileChunk`; the actual
  `driver.read_chunk()` call lives around `:585`, dispatched via
  `spawn_blocking`. Works unchanged.
- `src-tauri/src/jobs/viewer_prefetch.rs:57-143` — prefetch cache. The
  `is_local()` gate is in `src-tauri/src/ws/intents/panel.rs:~245`, not the
  prefetch module — non-local drivers simply don't get prefetched.
- `src-tauri/src/jobs/file_ops.rs` — copy via `open_read` (`:453`) /
  `open_write` (`:463`); try-rename fast path at `:181`. Works unchanged
  (write side returns `NotSupported` in v1).

### Phase 2 — Discovery module

New file: `src-tauri/src/remote/discovery.rs` (~150 lines).

- Add `mdns-sd` to `Cargo.toml`.
- On startup, spawn a tokio task that browses `_fbro-remote._tcp.local`,
  maintaining a `HashMap<peer_id, DiscoveredPeer>`.
- When the user toggles **Allow remote access** on, also *advertise*
  `_fbro-remote._tcp.local` on port 7879 with TXT records: `fbro=1`,
  `name=<hostname>`, `os=<linux|macos>`, `fp=<pubkey-fp>`.
- Push `StatePatch { domain: "discovered_peers", … }` whenever the set
  changes. *(Confirm `runtime.rs` accepts arbitrary new domains, or add
  the variant — likely a one-line change.)*

```rust
struct DiscoveredPeer {
    id: String,           // "fbro:<pubkey-fp>" once known, else "mdns:<host>:<port>"
    name: String,         // from TXT or hostname
    host: String,
    port: u16,
    os: Option<String>,
    pubkey_fp: Option<String>,  // from TXT `fp=`
    last_seen: SystemTime,
}
```

### Phase 3 — State, sources, intents

#### State changes (`src-tauri/src/state/mod.rs`)

- Add `discovered_peers: HashMap<String, DiscoveredPeer>` to `AppState`.
- Add `share_config: ShareConfig` (see below).
- Extend `Source`:
  - `kind: "remote"` as a third value alongside existing `"filesystem"` /
    `"virtual"`. Existing switches on `kind` (volume-bar rendering, etc.)
    must learn the new value.
  - `read_only: bool` — new field, defaults `false`. Set `true` for remote
    sources in v1.
  - `peer_id: Option<String>` — keyed to the `paired_peers` table for
    remote sources.
- The remote source's `supports` list is exactly
  `["ListEntries", "OpenFolder", "CopySelection", "InspectItem"]` — no
  write-related tokens.
- Update the capability projection in `src-tauri/src/ws/runtime.rs:178-182`:

  ```rust
  capabilities: SourceCapabilities {
      read_only: src.shadow || src.read_only,
      supports_streaming: true,
      supports_jobs: !src.shadow && !src.read_only,
  }
  ```

- Persist paired peers to `~/.config/fbro/peers.json` (loaded at startup,
  written on pair/revoke). `share.enabled` and `shared_source_ids` are
  volatile in v1. Discovered peers are never persisted.

```rust
pub struct ShareConfig {
    pub enabled: bool,
    pub shared_source_ids: HashSet<String>,
    pub paired_peers: HashMap<PubkeyFingerprint, PairedPeer>,  // mirror of peers.json
}

pub struct PairedPeer {
    pub fingerprint: String,    // SHA256 of pubkey
    pub pubkey: Vec<u8>,
    pub name: String,
    pub os: Option<String>,
    pub paired_at: SystemTime,
    pub last_seen: Option<SystemTime>,
}
```

#### Protocol additions (`src-tauri/src/ws/protocol.rs`)

- Extend `SourceSchema` with an optional `peer: { name, fingerprint, os }`
  block.

Inbound intents:

- `EnableSharing { enabled }`
- `SetSharedSources { source_ids: Vec<String> }`
- `StartPairing { peer_id }` — initiator side; opens a pairing channel to
  the target peer.
- `ConfirmPairing { handshake_id, accept: bool }` — answer after the user
  has compared the 6-digit code; must arrive within the 60 s window.
- `RevokePairing { fingerprint }`
- `ConnectRemotePeer { peer_id }` — no `auth` field; the only auth path is
  the paired pubkey.
- `DisconnectRemotePeer { source_id }`

Outbound events:

- `PairingRequested { handshake_id, code, name, os, pubkey_algo, expires_at }` —
  drives the modal on both sides; `code` is the zero-padded 6-digit string.
- `PairingResolved { handshake_id, outcome: "paired" | "rejected" | "timeout" }`
- `RemoteSessionStarted { fingerprint, source_id }` / `RemoteSessionEnded`

#### Intent handlers

- `src-tauri/src/ws/intents/sharing.rs` (new) — `EnableSharing`,
  `SetSharedSources`, `StartPairing`, `ConfirmPairing`, `RevokePairing`.
- `src-tauri/src/ws/intents/remote.rs` (new) — `ConnectRemotePeer`,
  `DisconnectRemotePeer`.

`ConnectRemotePeer` flow:

1. Look up the peer. If `paired_peers` doesn't contain it, return an error
   directing the FE to dispatch `StartPairing` first.
2. Open a `russh` client session to `peer.host:peer.port`, authenticating
   with our local Ed25519 key. The peer's server verifies our key against
   its own `paired_peers` allowlist.
3. Open the SFTP subsystem (`russh-sftp`). Issue the
   `fbro@anthropic.com/list-sources` extended request (defined below) to
   fetch the peer's source list with proper names/icons.
4. Construct `SftpFsDriver`. Insert into `state.drivers`. For each shared
   source returned, register a corresponding
   `Source { kind: "remote", read_only: true, peer_id, … }` with
   `mounted: true`. Push `StatePatch`.

`DisconnectRemotePeer`: drop the driver (its task closes the SSH session),
unmount the sources, push patch.

### Phase 4 — Server (the *other* side of the same process)

New files in `src-tauri/src/remote/`:

- `keys.rs` — generate `~/.config/fbro/id_ed25519` (Ed25519, no passphrase,
  0600) on first launch; load on subsequent launches. Fingerprint helpers
  (SHA256 of pubkey, base64).
- `server.rs` — `russh::server::Server` impl. Listens on 7879 when
  `share_config.enabled`. Authenticates incoming connections against
  `share_config.paired_peers`. Hosts the SFTP subsystem and the
  `fbro-pair@anthropic.com` channel type for pairing.
- `pairing.rs` — pairing handshake state machine, 6-digit code derivation,
  `paired_peers` map, `peers.json` I/O.

Pairing flow (replaces both host-key TOFU and `authorized_keys` editing):

1. User on **A** opens Settings → Sharing → toggles **Allow remote access**.
   fbro starts the russh listener and advertises via mDNS.
2. User on **B** sees A in the discovered-peers list, clicks **Pair**.
   FE dispatches `StartPairing { peer_id: A }`.
3. fbro on B opens a `fbro-pair@anthropic.com` channel to A's listener,
   sending B's pubkey, a 16-byte nonce, machine name, and OS. The pairing
   channel auth is `none` — pairing is the bootstrap for trust, not
   protected by it.
4. fbro on A replies with A's pubkey and a 16-byte nonce. Both sides now
   have the inputs to compute a 6-digit code:

   ```
   code = base10(HKDF-SHA256(
       salt   = nonce_a || nonce_b,
       ikm    = pubkey_a || pubkey_b,   // sorted lexicographically
       info   = "fbro-pair-v1",
       length = 4 bytes
   )) mod 1_000_000
   ```

5. Both sides emit `PairingRequested` with the same `code`. The FE on each
   side shows: *"Machine `liviu-mbp` wants to pair. Confirm this code is
   shown on the other device: `847 392`. [Confirm] [Cancel]"*
6. Both sides must `ConfirmPairing { accept: true }` within 60 s. On
   both-confirm: A stores B's pubkey in `paired_peers` and writes
   `peers.json`; B does the same with A's. `PairingResolved` fires on both
   sides with `outcome: "paired"`. Subsequent connections authenticate by
   key with no further prompts.
7. On either-cancel or timeout: emit `PairingResolved` with the appropriate
   outcome; no key is stored.

A MITM `M` who intercepts the pairing channel can substitute its own
pubkeys/nonces, but then A and B see *different* codes (each derived
against M's keys, not each other's), so the human comparison fails.

#### Server SFTP semantics

When a paired peer connects, the SFTP virtual filesystem is rooted at `/`
and shows one directory per shared source:

```
/
├── home-photos/        ← shared source "home-photos"
├── archive-2024/
└── work/
```

Every incoming path is parsed as `/<source_id>/<rest>` and validated:

- `source_id` must be in `shared_source_ids`. Otherwise `SSH_FX_NO_SUCH_FILE`.
- `<rest>` is passed to that source's `VfsDriver`. The server additionally
  rejects any resolved path that escapes the source root (defence in depth
  against `..`).
- Write methods (`open_write`, `remove`, `rename`, `mkdir`) return
  `SSH_FX_PERMISSION_DENIED` in v1.

Because this routes through `VfsDriver`, sharing a remote source (e.g.
chained: A shares an SFTP-mounted source from C) Just Works for read paths.

#### Custom SFTP extension

The client issues a non-standard SFTP extended request
`fbro@anthropic.com/list-sources` returning:

```json
[{"id": "home-photos", "name": "Photos", "icon": "image", "read_only": true}, …]
```

This lets the volume-bar render shared sources with proper names/icons
instead of bare directory entries.

### Phase 5 — FE

- **Volume bar**: new "Remote peers" section. Discovered peers appear with
  "Pair" (if unpaired) or "Connect" (if paired). Paired peers persist
  across restarts via `peers.json`.
- **Pairing modal** (on both sides): name, OS, 6-digit code, countdown
  timer, Confirm/Cancel. Reuses the existing `DecisionRequired` UI pattern.
- **Disconnected state**: spinner overlay on the panel + one-line banner
  "Reconnecting to liviu-mbp…" while the driver is in backoff.
- **Read-only badge**: small lock icon on the source row.
- **Settings → Sharing** (new section):
  - Toggle **Allow remote access** (drives `EnableSharing`).
  - Shared-source picker (multi-select over local sources;
    `SetSharedSources`).
  - Paired-peer list with name, OS, last-seen, **Revoke** button.
  - Active connections indicator (driven by `RemoteSessionStarted/Ended`).

## Alternatives considered

### Spacedrive's P2P stack (Iroh + HLC sync) — rejected for v1

Spacedrive uses Iroh (QUIC + NAT traversal + relay fallback), Ed25519
device identity, BIP39 pairing, and a CRDT-style sync model with Hybrid
Logical Clocks. Their *Library Sync* moves metadata; their *File Sync*
moves whole files via push-based, resumable, concurrent block transfer
with hash-checked dedup.

Why it's the wrong shape for fbro:

- **Sync, not browse.** Spacedrive replicates metadata everywhere first,
  then transfers whole files in the background. fbro needs live
  random-access reads — opening a 4 GB MP4 from a peer must stream, not
  pre-copy.
- **No range reads.** File Sync is whole-file (delta transfer is "future").
  Kills the embedded viewer use case.
- **Indexer-per-peer assumption.** Every Spacedrive device runs a
  long-running content-addressed indexer; fbro is a stateless browser.
- **v3 deprioritised P2P.** Spacedrive itself dropped working v2 P2P
  because they couldn't ship it stable, and v3 focuses on local search
  before sync.

Worth stealing later:

- **Iroh as transport** for off-LAN reach (NAT traversal + relay) when v1's
  "user runs Tailscale if they want WAN" stops being acceptable. Would
  replace `russh` for transport while keeping SFTP-shaped semantics on top.
- **Content hashing for dedup** if/when bidirectional sync becomes a
  feature.

Conclusion: SFTP over embedded `russh` remains the right v1 choice. Live
remote browsing is closer to NFS/SFTP/9P than to a sync protocol.

### System `sshd` + `internal-sftp` — rejected for v1

The original draft of this plan used the OS `sshd` running an
`internal-sftp` subsystem on the peer. Three real problems:

1. **Setup friction.** Mac users would have to enable Remote Login
   system-wide; Linux users need `openssh-server` installed and enabled.
2. **No source-level enforcement.** A successful SSH login grants whatever
   filesystem access the Unix user has; `ChrootDirectory` narrows it only
   at static config time with awkward ownership constraints.
3. **Authorized-keys collision.** Pairing would mean writing into the
   user's `~/.ssh/authorized_keys`, mixing fbro keys with the user's own;
   revoking would mean editing that file.

Embedding `russh` solves all three: zero setup, source-level access
control routed through `VfsDriver`, and a separate pairing list under
fbro's control.

### Two SSH stacks (system `ssh` client + `russh` server) — rejected

Earlier drafts split the stacks: client used the `openssh` crate (wraps
system `ssh` binary, giving free ssh-agent / pty / known_hosts), server
used `russh`. Rejected because pairing makes the ssh-agent / password
infrastructure unnecessary (we only ever auth by a pubkey we generated and
installed ourselves), and shipping two SSH implementations doubles attack
surface and upgrade burden. `russh` on both sides keeps the dependency
graph tight.

## Out of scope for v1

- **Plain-sshd / Termux / Mac-Remote-Login peers.** Re-introduces TOFU
  (host-key fingerprint confirmation), a second mDNS service type
  (`_sftp-ssh._tcp.local`), and an auth-method dialog (agent / key /
  password). Additive work for v2.
- Writes / deletes / renames / mkdir on remote sources (return
  `NotSupported`).
- Server-side sort/filter, content hashing, live watch — would need an
  optional `_fbro._tcp` SSH subsystem alongside SFTP. Deferred.
- NAT traversal / off-LAN reach — users overlay Tailscale/WireGuard if they
  need it.
- Persisting `share.enabled` and `shared_source_ids` across restarts. Both
  are intentionally volatile in v1 to force an explicit user gesture each
  session and avoid designing a persistence schema we'd want to revisit.

## Verification

- **Unit**: `SftpFsDriver` against an in-process `russh-sftp` server
  fixture in CI; conformance tests for every trait method.
- **Integration**: spin up two fbro instances in the same test process,
  pair them, run the full intent flow via a headless WebSocket client.
- **Manual end-to-end** (Linux ↔ Mac, both fbro):
  - Pair once via 6-digit code; both sides confirm. Verify
    `peers.json` contains the peer on both ends after restart.
  - Browse the peer's shared source from the volume bar.
  - Open a 2 GB MP4 in the embedded viewer (verifies chunked read +
    prefetch behavior under network latency).
  - Copy a 200 MB file Mac → Linux (verifies sequential `open_read`
    throughput).
  - Kill the russh server mid-list (verifies reconnect + disconnected-state UI).
  - Revoke pairing on one side → in-flight session is killed; reconnect
    attempt fails at auth.
  - Toggle **Allow remote access** off → mDNS advertisement disappears,
    port closes, in-flight sessions terminate cleanly.
  - Deselect a source from the share set → peer's existing session sees
    the source disappear on next `read_dir("/")` and any open handle into
    it returns `SSH_FX_NO_SUCH_FILE` on the next read.
  - Path traversal: attempt `/<shared>/../<not-shared>` from a paired
    client → rejected with `SSH_FX_PERMISSION_DENIED`.
- **Sanity (spec compliance)**: connect to the embedded server with a
  stock SFTP client using a paired key (e.g. the `sftp` CLI or
  `russh`'s own client binary) → works. Proves we're SFTP-spec-compliant,
  not relying on our own client.

## Critical files

**To create:**
- `src-tauri/src/vfs/sftp.rs` — `SftpFsDriver`
- `src-tauri/src/remote/mod.rs`
- `src-tauri/src/remote/discovery.rs` — mDNS browse + advertise
- `src-tauri/src/remote/server.rs` — russh listener + SFTP subsystem
- `src-tauri/src/remote/keys.rs` — Ed25519 keypair, fingerprint helpers
- `src-tauri/src/remote/pairing.rs` — pairing protocol, paired-peers map, `peers.json` I/O
- `src-tauri/src/ws/intents/remote.rs` — `Connect` / `Disconnect`
- `src-tauri/src/ws/intents/sharing.rs` — `EnableSharing` / `SetSharedSources` / `StartPairing` / `ConfirmPairing` / `RevokePairing`
- FE: `src/components/SettingsSharing.tsx`, `PairingRequestModal.tsx`

**To modify:**
- `src-tauri/src/vfs/mod.rs` — `pub mod sftp;`
- `src-tauri/src/state/mod.rs` — `discovered_peers`, `share_config`,
  `Source.kind="remote"`, `Source.read_only`, `Source.peer_id`
- `src-tauri/src/ws/protocol.rs` — new intents, outbound events,
  optional `peer` block on `SourceSchema`
- `src-tauri/src/ws/runtime.rs` — dispatch new intents; updated capability
  projection (`:178-182`); confirm `StatePatch.domain` accepts
  `discovered_peers` or add the variant
- `src-tauri/src/main.rs` — start discovery task; start russh server when
  sharing is enabled
- `Cargo.toml` — add `russh`, `russh-sftp`, `russh-keys`, `mdns-sd`,
  `hkdf`, `sha2`
- FE: volume-bar component, settings page
