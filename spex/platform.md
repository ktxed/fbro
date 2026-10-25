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
| `icon_cache_dir() -> Option<PathBuf>` / `icon_cache_revision() -> u64` / `clear_icon_cache() -> Result<u64, String>` | macOS persistent native-icon cache location and invalidation revision. Other platforms return no directory and reject clearing. Clear runs on a blocking worker. |
| `find_file_type_icon(extension) -> Option<(bytes, content_type)>` | Native document icon by extension, without the dot. Default `None`; macOS uses `UTType` and `NSWorkspace.iconForContentType:`. |
| `find_icon(name) -> Option<(bytes, content_type)>` | Look up a freedesktop-style icon name. Default `None`. The free function `resolve_icon` falls back to embedded SVGs. |
| `supports_managed_thumbnails() -> bool` | Default `false`; Linux returns `true`. Exposed as a read-only settings capability to show fbro's generation/cache controls and Jobs thumbnail view only where applicable. |
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

### 2.5 OS notifications

The PAL advertises `supports_os_notifications() -> bool`, defaulting to false.
`send_os_notification(&self, message: &str) -> Result<(), String>` has an
unsupported default, so platform rollout does not change shared notification
policy or force incomplete adapters.
Consumers use the backend notification service rather than calling this surface
directly.

`prepare_os_notifications()` defaults to a no-op. The service calls it on the UI
thread only while enabled and a native fbro window is focused, at startup, focus
gain, or preference enablement. It must return immediately and handle permission
prompts asynchronously. `os_notifications_authorized() -> Result<bool, String>`
defaults to `Ok(true)`; the delivery worker calls it outside locks and rechecks
activity, preference generation, and expiry afterward before native submission.

`supports_os_notification_settings() -> bool` defaults to false and gates the
settings notice/action through a backend capability. macOS opts in for both
packaged and bare development executables: opening the installed fbro app's
settings is independent of the calling process's native delivery identity.
`open_os_notification_settings() -> Result<(), String>` defaults to an
unsupported error and runs on a blocking worker after an explicit user action,
outside shared locks. macOS invokes `/usr/bin/open` with a fixed app-specific
notification-pane URL (`?id=dev.fbro.app`), using the System Settings extension
identifier on macOS 13+ and the legacy System Preferences pane on older releases.
The URI pattern follows [Chromium's `OpenSystemSettingsPane(kNotifications)`](https://chromium.googlesource.com/chromium/src/+/HEAD/base/mac/mac_util.mm);
there is no shell interpolation, arbitrary URL argument, or permission change.

Linux is the first supported adapter. It uses `notify-rust` with the session
freedesktop service, application name and desktop-entry identity `fbro`, the
application icon, escaped body text, normal urgency, and desktop-controlled
expiry. Native submission runs on the notification worker after shared locks are
released, with a three-second native service timeout. The adapter retains the
newest 128 native handles and evicts the oldest on overflow, preserving recent
notification connections without depending on closure signals for capacity.
No `notify-send` executable or custom D-Bus protocol is required. Missing session
services return a delivery error without changing operation state or history.

Application activity is tracked centrally from Tauri native focus/destruction
events across main, viewer, and diff windows. A 150 ms handoff grace period
prevents notifications between fbro windows. The worker rechecks activity,
enablement, and notification expiry before submission; no frontend-derived focus
intent or preference is needed. See [notifications.md](notifications.md).

macOS uses `objc2-user-notifications` with `UNUserNotificationCenter`, rather than
the deprecated `NSUserNotification` backend. Capability requires an `.app` main
bundle with identifier `dev.fbro.app`; bare binaries return unsupported without
calling Notification Center, whose initialization can throw for missing identity.
Packaging signs the complete app bundle with ad hoc identity `-` by default,
binding Info.plist and resources to the app identifier. A linker-signed executable
alone is insufficient for Notification Center authorization. Release builds may
override this with an Apple signing identity; ad hoc signing is not notarization.
Authorization requests ask only for alerts, once per process from a foreground
UI hook; errors allow a later foreground retry and denial is logged with the
System Settings path. An authorization error triggers an asynchronous settings
read: a saved denied status is terminal for the process even if the native
completion included an error (macOS can return `UNErrorDomain:1` for denial).
Other states retain status/error diagnostics and allow a later foreground retry.
Delivery queries actual authorization (authorized or
provisional), then sends immediate requests with unique UUIDs, title `fbro`, and
plain body text. Both lookup and submission wait at most three seconds on the
worker. Completion blocks convert borrowed native errors to owned Rust text;
late callbacks are safe after timeout. The bundle supplies name/icon. No sound,
badge, custom action, focus change, or Do Not Disturb override is requested.

Windows retains the unsupported defaults. Packaged-application validation and
the remaining platform rollout are tracked in
[the notification roadmap](plans/os-notifications.md).

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

Panel file icon keys retain lowercase extensions as URL-safe UTF-8 hex
(`file-type-<hex>`). `resolve_icon` first tries native extension lookup, then
uses the existing freedesktop mapping and embedded fallback (including the HTML
icon). macOS provides native document and folder icons as PNG, using
`NSBitmapImageRep` and ImageIO on the file-server worker. A bounded, process-local
cache holds up to 256 lookup results, including misses. Successful icons are also
stored atomically as hashed PNG filenames under `$HOME/Library/Caches/fbro/icons/v1`.
Disk hits are reused across launches; unavailable/truncated entries regenerate.
Cache write failures log and still return the native image. Misses remain memory-only.
Lookup and clear share a lock, preventing pre-clear lookups from restoring stale
entries. Clearing removes disk and memory entries and increments the backend
revision used in panel HTTP URLs to bypass WebView caching. Display settings
expose the clear action only when the backend reports a cache directory. The panel displays a generic folder/file icon until the requested
image loads, and retains that fallback if loading fails. Extension lookup also works for remote and archive entries; it
does not inspect content, custom per-file icons, or application bundles.

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

### 3.4 Embedded terminals

Embedded PTY resize operations run on one serialized worker per session,
coalescing pending sizes. The WebSocket loop only queues geometry; it must
continue draining terminal output while the OS resizes the PTY. In particular,
Windows ConPTY resizing or shutdown can wait for output consumption. Session
removal releases the registry lock before process termination or PTY disposal,
and explicit close performs that blocking teardown on a worker thread.

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
