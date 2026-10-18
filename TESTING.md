# Testing

Run commands from the repository root. Use unit tests for backend logic and
WebDriver smoke tests for application behavior, keyboard handling, and focus.

## Build and unit tests

Install Node.js 22.19+, current stable Rust, and the Tauri system dependencies
listed in [README.md](README.md#development), then install the locked packages:

```bash
npm ci
```

The standard checks are:

```bash
npm run build
npm run test:build-info
node --test tests/check-version.test.mjs
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo check --manifest-path tools/lfs-spy/Cargo.toml --locked
```

`npm run build` runs TypeScript checking and the production frontend build.
There is no separate frontend lint command. The Node tests cover build metadata
and release-version consistency. Rust tests cover backend behavior and adapters;
the last command checks the optional local-filesystem spy helper.

For dependency changes, also run `npm audit`. With `cargo-audit` installed,
audit both Rust lockfiles:

```bash
cargo audit --file src-tauri/Cargo.lock
cargo audit --file tools/lfs-spy/Cargo.lock
```

## Linux smoke tests

The smoke runner is [tests/e2e/smoke/index.mjs](tests/e2e/smoke/index.mjs).
It drives native frontend windows with WebDriver and checks backend state over
WebSocket. Two backend app instances, `fbro-a` and `fbro-b`, run in Docker under
Xvfb. Their files and settings live under `artifacts/e2e`.

### Requirements

In addition to the normal build dependencies, the host needs:

- Docker with Compose, accessible by the current user.
- `tauri-driver` and `WebKitWebDriver` in `PATH`.
- A graphical session for the native app windows.
- `git` and `zip` for the Git and archive fixtures.

The commands below use Wayland. On an X11 session, omit `GDK_BACKEND=wayland`.
Run one smoke suite at a time: the default ports and Compose services are shared.
The containers reset their fixture directories and settings at startup.

### Recommended: debug build

```bash
GDK_BACKEND=wayland npm run e2e:smoke:local_debug
```

This builds the app with `tauri build --debug --no-bundle`, then uses
[docker-compose.e2e.host.yml](docker-compose.e2e.host.yml) and
[docker-compose.e2e.debug.yml](docker-compose.e2e.debug.yml). The host frontend
windows and Docker backends use the same debug binary.

Use the Tauri build command for smoke binaries. A plain `cargo build` produces
a development app that expects the Vite server at `http://localhost:5173`.
Finish building before starting the tests; concurrent builds can replace the
frontend assets while Tauri is embedding them.

### Host release build

```bash
npm run tauri -- build --no-bundle
FBRO_APP_BINARY=src-tauri/target/release/fbro \
  FBRO_E2E_COMPOSE_FILES=docker-compose.e2e.host.yml \
  GDK_BACKEND=wayland npm run e2e:smoke:local
```

This builds only the Docker runtime image and mounts the host release binary
into both containers. Setting `FBRO_APP_BINARY` explicitly keeps the frontend
and backend on the intended build.

### Build the backend app in Docker

```bash
npm run tauri -- build --no-bundle
FBRO_APP_BINARY=src-tauri/target/release/fbro GDK_BACKEND=wayland npm run e2e:smoke
```

The default [docker-compose.e2e.yml](docker-compose.e2e.yml) builds the backend
app inside Docker. The first command builds the binary for the native frontend
windows. This mode is slower than the host-runtime modes.

### Run a focused flow

Set one of these flags before the debug command. For example:

```bash
FBRO_E2E_REMOTE_ONLY=1 GDK_BACKEND=wayland npm run e2e:smoke:local_debug
```

| Flag | Runs |
| --- | --- |
| `FBRO_E2E_FOCUS_ONLY=1` | Focus handoffs and modal ownership |
| `FBRO_E2E_PANEL_SELECTION_ONLY=1` | Space selection, folder sizes, Shift+Space targeting, and keyboard ownership in list/grid views |
| `FBRO_E2E_NOTIFICATIONS_ONLY=1` | Notification publication, FIFO toasts, history, clear, keyboard focus, and reload |
| `FBRO_E2E_THUMBNAILS_ONLY=1` | Thumbnail settings, Jobs queue, and viewport behavior |
| `FBRO_E2E_VIEWER_ONLY=1` | Viewer and preview regressions |
| `FBRO_E2E_GIT_ONLY=1` | Repository initialization and Git operations |
| `FBRO_E2E_HASH_CHECK_ONLY=1` | Check hashes |
| `FBRO_E2E_HASH_GENERATE_ONLY=1` | Generate hashes, followed by Check hashes |
| `FBRO_E2E_SEARCH_ACTIVITY_ONLY=1` | Search progress, background jobs, and cancellation |
| `FBRO_E2E_SEARCH_ACTIONS_ONLY=1` | Search-result actions and focus |
| `FBRO_E2E_REMOTE_ONLY=1` | Sharing, pairing, browsing, transfers, and cancellation |

The viewer flow also accepts `--viewer-only`:

```bash
GDK_BACKEND=wayland npm run e2e:smoke:local_debug -- --viewer-only
```

Set `FBRO_HEIC_FIXTURE=/absolute/path/to/image.heic` on a full run to check
successful optional HEIC conversion. Without it, the HEIC conversion flow
checks settings, invalid-file errors, and switching an open viewer back to Hex.

## Smoke coverage

The full suite runs every flow below. Case files are in
[tests/e2e/smoke/cases](tests/e2e/smoke/cases/); those assertions are the detailed
coverage reference.

| Area | Case files | Main checks |
| --- | --- | --- |
| Local files and UI | `local.mjs` | Create, copy, overwrite, rename, delete, sort, filtering, empty-folder suggestions, settings, bookmarks, Bros, clipboard feedback, grid navigation, and symlink routes |
| File icons | `file-icons.mjs` (called by the local flow) | Exact extension keys, case normalization, and successfully loaded panel icons for HTML, PNG, JPEG, JSON, and TOML. Backend capability gates the cache control; on macOS keyboard clearing increments the icon revision and preserves button focus. Disk reuse, corruption recovery, and clearing are covered by `platform::macos::icon_cache::tests`. Native macOS PNG lookup is also covered by `platform::tests::native_extension_icons_are_png`; visually check both color schemes on macOS. |
| Panel selection and sizes | `panel-selection.mjs` (called by the local flow) | Space selects folders/files in list and grid views, calculates unknown sizes, reuses known/zero sizes, preserves cursor/focus, ignores held-key repeats, and respects modal/input/native controls; Shift+Space sizes selected/all folders without changing selection; Copy targets a selected folder despite a file cursor |
| Focus and modals | `focus-handoff.mjs`, `modal-focus.mjs`, and checks throughout the suite | Initial focus, exact opener restoration, covered layers ignoring keys, one submission per activation, nested Escape, backend rejection/removal, outside clicks, and preserving focus during toasts or toolbar actions |
| Notifications | `notifications.mjs` | Frontend clipboard publication without duplicates, timestamps/severity, FIFO burst delivery, keyboard history navigation, Clear focus handoff, clearing without cancelling delivery, overflow arrivals preserving the browsed row, both schemes, and reload without replay |
| Thumbnails | `thumbnails.mjs` (also called by the local flow) | Platform capability controls, Jobs tab navigation, queue refresh preserving focus, visible-card loading, off-screen cooldown, viewport promotion, selection fill, and abandoning old requests |
| Viewers | `viewer.mjs` | Large and delayed images, J-next/K-previous rapid/reverse navigation, invalid-image recovery, text and binary content, horizontal arrow scrolling in small/large text with find-input cursor ownership, PDF, Markdown/HTML images, Mermaid, zoom, audio/video controls, preview keyboard ownership, and opener restoration |
| HEIC conversion | `heic.mjs` | Backend-confirmed opt-in settings, startup settings delivery, errors, optional real-file decoding, and live disabling |
| Quick Edit | `quick-edit.mjs` | Edit/save in the embedded terminal, terminal closure, journal entries, and editing a newly created empty file |
| Global selections | `global-selection.mjs` | Creation, reload persistence, adding through menus and long right-clicks, keyboard selection, focus, and removing references without deleting files |
| Git | `git-init.mjs`, `git.mjs` | Initialization, branch creation/filtering/checkout, validation and busy states, stage/unstage/rollback, changes/log navigation (J down, K up), journal entries, modal guards, and focus |
| Fedup | `fedup.mjs` | Global-selection input, default ignores, case-insensitive includes, and custom excludes |
| Hashes | `hash-check.mjs`, `hash-generate.mjs` | Discovery, stale targets, options, matching/malformed sidecars, skip/overwrite, reports/toasts, progress, background/reopen, cancellation, and preserving existing sidecars |
| Archives | `archive.mjs` | Enter a ZIP, copy a file out, and verify its content |
| Search | `search.mjs` | Activity without hits, inspected counts, cancellation, duplicate-name hints and viewer paths, toolbar/shortcut Copy with cursor and selection targets, repeated-filename exclusion/suffixes via inline and bottom actions, background copies, per-dialog defaults, both themes, Jump to location, Open With, stale/deleted targets, modal keys, and date/size filters |
| Remote | `remote.mjs` | Folder suggestions and typed shares, validation/history, unsharing, pairing, read-only browsing, copying, thumbnails, integrity, cancellation on both peers, and a fresh transfer after cancellation |

Several UI flows check both `fbro-dark` and `daylight`. Modal tests sample common
focus and cancellation paths; they do not cover every operation/conflict state.
Search viewer fixtures are isolated from files created by earlier flows.
Created-time search checks depend on filesystem birth-time support.
Hash progress/cancellation tests use sparse 8 GiB files, so their apparent size
does not require 8 GiB of allocated disk space.

## Focused Rust tests

Notification state projection and toast queue tests: `npm run test:notifications`
(Node.js 22.19 or newer). Backend service, channel routing, retention, intent
validation, and producer ownership: `cargo test --manifest-path src-tauri/Cargo.toml notifications`.

Append a filter to the Rust test command:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --locked patch_tests
```

| Filter | Coverage |
| --- | --- |
| `patch_tests` | Delayed scan patches preserving newer cursor/selection/version, queued entries/completion/errors, and independent window updates |
| `ws::intents::panel::tests` | Cursor/range selection, Space folder selection and size reuse, running-size preservation, full-list size targets beyond render windows, supported drivers, and stale archive completion guards |
| `remote::` | Share validation/persistence, SFTP root resolution, transfer isolation, cancellation, and late lifecycle events |
| `vfs::sftp::tests` | SFTP reads/cancellation and paired Ed25519 host-key pinning, including rejection of mismatched keys and certificates |
| `jobs::file_ops::tests` | File-operation and remote-transfer cleanup/cancellation |
| `jobs::search::tests` | Activity/counts, estimates, roots/depth, large directory backlogs, permission errors, FIFO avoidance, and cancellation |
| `search_navigation_tests` | Exact source/path resolution, result ordering, and missing results |
| `copy_duplicates`, `search_copy_duplicates`, `copy_duplicate_args_tests` | Repeated search filenames, exact source identity, exclusion/suffixes, collisions, folder passthrough, defaults, actual copy contents, and matching dry-run destinations |
| `git::` | Repository operations, target/driver validation, native-path mapping, and replies |
| `jobs::check_hashes` | Parsing, streaming, I/O errors, report collisions, symlinks, and cancellation at report commit |
| `jobs::generate_hashes` | Selection, filename handling, checker compatibility, destination races, I/O failures, cleanup, and per-file commit cancellation |
| `terminal::tests` | Nonblocking resize/close, pending resize coalescing, and session registry release |
| `storage` | Device classification and prefetch eligibility |
| `viewer_prefetch` | Scheduling, cache limits, storage policy, and cancellation |
| `thumb_pool` | Deduplication, visibility priority, cooldown/wakeup, concurrency, stale epochs, and navigation cleanup |
| `managed_thumbnails_are_linux_only` | Managed-thumbnail capability on the compiled host platform |
| `heic_preview_is_opt_in` | Older settings defaulting to off and enabled-setting serialization |

## Failure diagnostics

A zero exit code means the smoke run passed. The runner does not produce JUnit
or HTML reports. By default, diagnostics are written under `artifacts/e2e`:

| Artifact | Contains |
| --- | --- |
| `final-tree.txt` | Final fixture filesystem trees for both instances |
| `fbro-a.log`, `fbro-b.log` | Container app stderr/stdout |
| `logs/tauri-driver-a.log`, `logs/tauri-driver-b.log` | Host driver/app logs |
| `transcripts/fbro-a.json`, `transcripts/fbro-b.json` | Backend WebSocket messages observed during the run |
| `screenshots/` | Failure captures and screenshots from individual cases |
| `viewer-timings.json` | Viewer timings and process-memory samples |

Viewer timings include WebDriver and forwarding-server overhead; use them to
investigate regressions rather than as standalone rendering benchmarks.
Backend log prefixes identify viewer navigation, HTTP reads/cache hits, prefetch,
and thumbnail work: `[viewer navigation]`, `[viewer http]`, `[viewer prefetch]`,
and `[thumbnail]`.

Useful overrides:

| Variable | Purpose |
| --- | --- |
| `FBRO_E2E_KEEP_DOCKER=1` | Keep backend containers after the run for inspection |
| `FBRO_E2E_ARTIFACTS` | Change the runner's diagnostic output directory |
| `FBRO_APP_BINARY` | Select the host app binary; the debug script sets it automatically |
| `FBRO_E2E_COMPOSE_FILES` | Select Compose files; separate multiple files with `:` on Linux |
| `FBRO_A_WEBDRIVER_PORT`, `FBRO_B_WEBDRIVER_PORT` | Change host tauri-driver ports (defaults: 4444, 4445) |
| `FBRO_A_NATIVE_WEBDRIVER_PORT`, `FBRO_B_NATIVE_WEBDRIVER_PORT` | Change WebKitWebDriver ports (defaults: 4446, 4447) |

The Compose files mount `artifacts/e2e` explicitly. If changing the artifact
location, adjust the Compose mounts and fixture roots as well. The runner's
configuration is in [config.mjs](tests/e2e/smoke/config.mjs).

The runner stops drivers after each run. It also removes the Compose services
unless `FBRO_E2E_KEEP_DOCKER=1` is set. After an interrupted debug run,
clean up retained containers with:

```bash
docker compose -f docker-compose.e2e.host.yml -f docker-compose.e2e.debug.yml down --remove-orphans
```

## Checks that need native hardware or another OS

The automated smoke environment exercises Linux. Linux fixtures, including
Windows storage-descriptor fixtures, do not validate native macOS or Windows
APIs. Check the affected platforms when changing these areas:

- **Remote discovery:** On macOS, enable remote access alongside Bonjour and
  confirm the local hostname does not acquire numeric suffixes. Verify LAN
  discovery and pairing with a Linux peer, including with VPN and Apple P2P
  interfaces present. Linux backend tests do not verify this native interaction
  or recovery from the reported network outage.
- **Keyboard, focus, and styling:** Cmd/Ctrl shortcuts, modal stacking, exact
  opener restoration, both themes, and WebView controls. The smoke harness checks
  viewer-launch intents but cannot verify the backend's separate native viewer
  window or its OS focus handoff.
- **Media:** Native audio output and codec support; the MP3 smoke case mocks
  playback. Check HEIC/HEIF decoding in both F3 and embedded preview, including
  macOS native decoding and optional conversion on Linux/Windows.
- **Terminals:** Real editor resize and close on Linux/macOS and Windows ConPTY.
  Quick Edit smoke uses the fake-nano fixture.
- **Files and storage:** Native symlinks, path/ACL behavior, thumbnail providers,
  internal/external SSDs, card readers, network and multi-device volumes. Check
  `storageClass`/`prefetchEligible`, keep unknown devices ineligible, and confirm
  unplug/remount updates without reusing stale viewer cache entries.
- **Eject:** Real removable devices on Linux with UDisks2 and on macOS. Test
  single- and multi-volume drives, active file jobs, and externally busy devices.
  Volume-only unmount must preserve siblings; whole-drive eject must remove them
  and recover open tabs to Home. Docker covers only the picker and its empty state.

## Keeping this guide current

Update the coverage and focused-run tables when adding or changing test flows.
Keep dated run results, test counts, dependency advisories, and platform results
in the relevant PR or implementation plan. The
[dependency refresh record](spex/plans/2026-10-09-dependency-refresh.md) contains
that change's validation results and remaining audit warnings.
