# fbro — Platform Abstraction Layer (PAL)

The Platform Abstraction Layer is a BE-internal subsystem that resolves
platform-specific environment attributes and services: home directory, volume
enumeration, icon lookup, native thumbnailers, "open with" application
resolution, terminal/file-manager hand-off, and the host clipboard.

Implementations live under `src/platform/{linux,macos,windows}`; all consumers
go through the shared `dyn Platform` trait so the rest of the BE contains no
`#[cfg(target_os = ...)]` branches.

---

## 1. Design Principles

* **BE-internal** — the FE never queries platform attributes directly.
* **Injected** — `main.rs` calls `platform::create_platform()` once at startup
  and passes the resulting `Arc<dyn Platform>` to `AppState`, `ThumbPool`, and
  the WebSocket runtime.
* **Trait, multiple implementations** — one impl per target OS
  (`LinuxPlatform`, `MacOSPlatform`, `WindowsPlatform`), selected by `#[cfg]`
  inside `create_platform()`.
* **Default-method opt-in** — every method except `home_dir` and
  `open_in_editor` has a no-op default, so a partial implementation still
  compiles. Capabilities a platform lacks simply return `None` / `vec![]` /
  `Err("not supported")`.
* **No filesystem mutations through PAL itself** — the PAL queries the host
  and delegates to it (e.g. `open_file` hands a path to the OS); it never
  writes through `VFSDriver` interfaces.

---

## 2. `Platform` Trait

Sync trait, `Send + Sync`. Grouped by purpose:

### 2.1 Environment

| Method | Purpose |
|---|---|
| `home_dir() -> String` | Current user's home. Used by `NavigateHome`, panel initialisation, and the `~` resolver. |
| `disk_space(path) -> Option<(free, total)>` | Volume capacity for the FS containing `path`. |
| `list_volumes() -> Vec<VolumeInfo>` | Mounted (and on Linux, also unmounted) volumes. Synchronous fallback path; the async `VolumeWatcher` (§4) is preferred. |
| `eject_volume(device_path, action) -> Result<()>` | macOS native removable-volume action on a blocking worker; default reports unsupported. |
| `supports_zsh_emacs_keymap() -> bool` | Opts macOS into the embedded-zsh Emacs-keymap workaround. Other platforms use the default `false`. |

### 2.2 Icons & thumbnails

| Method | Purpose |
|---|---|
| `find_icon(name) -> Option<(bytes, content_type)>` | Look up a freedesktop-style icon name. Default `None`. The free function `resolve_icon` falls back to embedded SVGs. |
| `get_thumbnail(path, size) -> Option<(bytes, content_type)>` | Native thumbnailer (Windows `IShellItemImageFactory`, macOS `QLThumbnailGenerator`). MUST be safe to call from any worker thread — `ThumbPool` calls it from a thread pool. |

`get_thumbnail_cancellable(path, size, &AtomicBool)` is the pool entry point.
The default checks the flag before/after `get_thumbnail`; macOS and Windows
native calls already in progress may finish. Linux overrides it with cooperative
read/decode/frame checkpoints. Cancellation releases request waiters immediately;
the pool keeps the worker marked `Cancelling` until it physically returns.

### 2.3 Launching applications

| Method | Purpose |
|---|---|
| `open_file(path)` | Fire-and-forget: open with the OS default association. |
| `open_in_editor(path) -> Result<path, error>` | Open in the user's default text editor. |
| `open_terminal(path)` | Launch a terminal emulator with `path` as cwd. |
| `reveal_in_file_manager(path, kind)` | Reveal in the host file manager (Files/Finder/Explorer). |
| `list_open_with_apps(path, kind) -> Result<Vec<ResolvedOpenWithApp>>` | Enumerate apps that can open `path`. |
| `launch_open_with(path, &ResolvedOpenWithApp) -> Result<()>` | Invoke a specific result from `list_open_with_apps`. Default impl handles the cross-platform `OpenWithLaunch` variants (`Default`, `Command`, `MacApplication`); platform-specific variants (`LinuxApp`) override. |

`ResolvedOpenWithApp` carries an `OpenWithAppOption` (id, label, detail,
is_default, source) plus an `OpenWithLaunch` enum that encodes how to invoke
it. The helper `append_supplemental_apps` merges per-platform lists while
de-duplicating by normalised (label, detail).

### 2.4 Host clipboard

| Method | Purpose |
|---|---|
| `read_external_file_clipboard() -> Result<Option<ExternalClipboardPayload>>` | Read a file-manager copy/move payload from the OS clipboard (Linux: `x-special/gnome-copied-files` etc.). |
| `write_external_file_clipboard(&payload) -> Result<()>` | Push a file-manager payload onto the OS clipboard. |
| `write_text_clipboard(text) -> Result<()>` | Plain text into the clipboard. |

`ExternalClipboardPayload { op: Copy | Move, source_paths: Vec<String> }`
crosses the FE/BE boundary only as the underlying file ops — the FE does not
see the payload directly.

---

## 3. Implementations

### 3.1 `LinuxPlatform`

Home resolution order:

| Step | Source | Condition |
|---|---|---|
| 1 | `$HOME` | Non-empty |
| 2 | `/proc/self/status` → `Uid:` → `/etc/passwd` | `$HOME` absent or empty |
| 3 | `/` | All of the above fail |

Other Linux specifics: freedesktop icon theme lookup, MIME/`xdg-mime` for
default apps and "open with" enumeration, `xdg-open`/`gio open` for fire-and-
forget launches, `xclip`/`wl-copy` (selected at runtime) for the clipboard,
`/proc/mounts` for the fallback volume list.

### 3.2 `MacOSPlatform`

Home from `$HOME` (which the OS always sets). Native APIs: `NSWorkspace` for
default-app launch, `QLThumbnailGenerator` for thumbnails, `LSCopyApplications`
for "open with", `NSPasteboard` for the clipboard, `open -R` for reveal.

The image viewer recognizes `.heic` and `.heif` on macOS and delegates decoding
to WKWebView (Safari 17 or newer), for both HTTP images and buffered Blob images.
The file server uses `image/heic` and `image/heif`; viewer prefetch applies the
image size cap to both formats. Linux and Windows use the hex preview by
default. The backend-owned `enableHeic2any` setting opts these platforms into
HEIC2ANY conversion in the WebView: local images are fetched through the file
server, other sources use backend-provided buffers, and the first converted
image is displayed as PNG. The decoder is bundled and loaded on demand; no
external service or installed codec is needed. Conversion does not preserve
metadata or provide animation playback. Image input caps apply; decoding errors
are shown in the preview. Superseded results are discarded and object URLs are
released. If HEIC2ANY's older decoder rejects the container (`ERR_LIBHEIF`),
the viewer lazily loads `heic-to` and retries locally with its newer decoder.
macOS keeps its native path even if the persisted setting is true.
Native thumbnails use a separate OS pipeline and do not determine viewer format
support.

### 3.3 `WindowsPlatform`

Home from `USERPROFILE`. `SHGetKnownFolderPath` for canonical folders,
`IShellItemImageFactory` for thumbnails, `AssocQueryString` for default apps
and the "Open with" list, `ShellExecuteEx` for launches, `OleSetClipboard` for
file clipboard payloads in `CF_HDROP` format, `SetClipboardData(CF_UNICODETEXT)`
for `write_text_clipboard` (Copy Paths bro).

Release builds set `windows_subsystem = "windows"` (`src-tauri/src/main.rs`),
so the process has no console. A console-subsystem child such as `git.exe`
must be spawned with `CREATE_NO_WINDOW` (`git::git_command`). Otherwise each
child allocates a console — on Windows 11, usually a Windows Terminal window
that flashes and closes. Startup scans run several `git` commands per
local panel. The window `focus` handler sends `RefreshGitStatus`, which runs
those commands again, so each flash steals focus and the popups repeat.

---

## 4. `VolumeWatcher` (async)

Separate async trait, also under `src/platform/mod.rs`. Provides
**enumeration, mounting, and change notification** without leaking
OS-specifics (udisks2 D-Bus, `/proc/mounts` polling, Windows
`WM_DEVICECHANGE`) into the WS runtime.

```rust
#[async_trait::async_trait]
pub trait VolumeWatcher: Send + Sync {
    async fn list_volumes(&self) -> Vec<VolumeInfo>;
    async fn mount(&self, device_path: &str) -> Result<String, String>;
    async fn eject(&self, device_path: &str, action: EjectAction) -> Result<(), String>;
    async fn subscribe(&self) -> mpsc::Receiver<()>;
}
```

`create_volume_watcher(platform)` picks the best available backend:

* Linux + udisks2 reachable → `Udisks2Watcher` (show mounted filesystems and
  unmounted user-removable media, mount on click, receive change signals over
  D-Bus). Unmounted internal/system partitions remain outside the source bar.
* Linux without udisks2 → `ProcMountsWatcher` (poll `/proc/mounts`, no
  mounting or ejecting).
* Other platforms → `FallbackVolumeWatcher` (wraps `Platform::list_volumes`;
  `mount` returns `Err`, eject delegates to `Platform` on a blocking worker;
  `subscribe` returns a channel that never fires, and
  callers fall back to periodic polling).

`VolumeInfo.eject` is present only for mounted removable local volumes. It
carries a backend-only physical drive ID, drive name, mounted sibling count,
and whole-drive capability. Linux derives these from UDisks2 Drive and Block
objects, unmounts through Filesystem.Unmount, then uses Drive.PowerOff where
supported or Drive.Eject for ejectable media. Flash cards and thumb drives
without power-off support finish after all their volumes are unmounted because
some readers advertise Ejectable without a working hardware eject command.
macOS checks `/Volumes` against the mount
table, excludes disk images, identifies external physical media with
`diskutil info -plist`, and uses `diskutil unmount` or `diskutil eject` on the
corresponding whole disk. Windows returns unsupported. No path is passed
through a shell and neither platform forces an unmount.

---

## 5. Wiring

```rust
// main.rs
let platform = platform::create_platform();              // Arc<dyn Platform>
let thumb_pool = ThumbPool::start(platform.clone(), 8);
let state = AppState::new_default(platform.clone(), thumb_pool, ...);
// later, inside the ws server task:
let volume_watcher = create_volume_watcher(platform).await;
```

The watcher immediately reconciles the synchronous startup inventory before
the first client snapshot. This gives startup and hot-plug updates identical
volume visibility and stable UDisks IDs. Existing panel paths are preserved and
rebound to the most specific mounted source when an initial fallback ID changes.

`AppState.platform: Arc<dyn Platform>` is the canonical handle. Intent
handlers reach it through `ctx.state.lock().await.platform` (or by cloning the
`Arc` out of the lock before any blocking call). `ThumbPool` holds its own
clone to call `get_thumbnail` off the async runtime.

---

## 6. PAL consumers

Non-exhaustive — grows as new intents are added.

| Site | PAL surface used |
|---|---|
| `state/mod.rs` initial state | `home_dir`, `list_volumes` |
| `runtime.rs` volumes domain | `disk_space`, `home_dir` (sources sort, `~` expansion) |
| `ws/intents/navigation.rs` | `home_dir` (NavigateHome), `open_file`, `open_terminal`, `reveal_in_file_manager`, `list_open_with_apps`, `launch_open_with` |
| `ws/intents/viewer.rs` | `open_in_editor` |
| `ws/intents/jobs.rs` | `read_external_file_clipboard`, `write_external_file_clipboard` (copy/cut to host clipboard) |
| `ws/intents/bro.rs` | `write_text_clipboard` |
| `thumb_pool.rs` | `get_thumbnail` (worker threads) |
| `platform::resolve_icon` | `find_icon` (icon-serving HTTP route) |
| `ws/server.rs` startup | `create_volume_watcher` |

---

## 7. `NavigateHome` intent (canonical example)

```json
{ "type": "Intent", "name": "NavigateHome", "scope": "<panelId>" }
```

**BE behaviour**

1. Resolve `scope` to a panel id.
2. Acquire `platform.home_dir()` (cheap — cached values are not used; the
   trait method is the canonical lookup).
3. Call `navigate_to(panel_id, home_path)` — identical to any other directory
   navigation: clears entries/selection, resets cursor, spawns a scan job,
   sends a `StatePatch`.

The FE fires this from the `~` button in the panel header (`tabIndex={-1}` so
focus stays in the panel).

---

## 8. Adding a new platform method

1. Add the method to the `Platform` trait with a no-op default
   (`Option::None`, `vec![]`, or `Err("not supported")`) so existing impls
   keep compiling.
2. Override in each `*Platform` impl where the host can provide it.
3. Call it from the consuming intent / subsystem.
4. Document it in §2 of this file.

If the new surface is async or needs ambient state (signals, subscriptions),
prefer a separate trait alongside `VolumeWatcher` rather than turning
`Platform` async.
