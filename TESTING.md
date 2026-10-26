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
npm run test:notifications
npm run test:selection-mask
npm run test:tabs
npm run test:modal-keyboard
node --test tests/check-version.test.mjs
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo check --manifest-path tools/lfs-spy/Cargo.toml --locked
```

`npm run build` runs TypeScript checking and the production frontend build.
There is no separate frontend lint command. The Node tests cover build metadata
and release-version consistency, notification history/toast projection,
selection-mask prompt defaults, stale tab-update rejection, and cyclic keyboard
navigation (including empty-bookmark navigation). Rust tests
cover backend behavior and adapters;
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
| `FBRO_E2E_TABS_ONLY=1` | Mouse tab reordering, overflow scrolling, terminal ownership, cancellation, active-tab/focus preservation, and tab click/close behavior |
| `FBRO_E2E_BOOKMARKS_ONLY=1` | Current-folder initial focus, cyclic Up/Down navigation, Home/End, keyboard creation/removal, focus, and list sizing |
| `FBRO_E2E_BUTTON_BAR_ONLY=1` | Persistent button bar visibility, Settings and command palette, hidden-bar shortcuts, focus, both themes, and invalid values |
| `FBRO_E2E_PANEL_SELECTION_ONLY=1` | Space selection, folder sizes, Shift+Space targeting, mask selection, and keyboard ownership in list/grid views |
| `FBRO_E2E_SELECTION_MASK_ONLY=1` | Select/Deselect Files masks, defaults, keyboard/focus ownership, and full windowed-list matching |
| `FBRO_E2E_NOTIFICATIONS_ONLY=1` | Notification publication, FIFO toasts, history, clear, keyboard focus, and reload |
| `FBRO_E2E_JOURNAL_SORT_ONLY=1` | Journal sort order, remembered preferences, panels/tabs, reload, root isolation, and reset |
| `FBRO_E2E_THUMBNAILS_ONLY=1` | Thumbnail settings, Jobs queue, and viewport behavior |
| `FBRO_E2E_VIEWER_ONLY=1` | Viewer and preview regressions |
| `FBRO_E2E_DIFF_ONLY=1` | New-file diff Up/Down scrolling in split and unified views |
| `FBRO_E2E_GIT_ONLY=1` | Repository initialization and Git operations |
| `FBRO_E2E_HASH_CHECK_ONLY=1` | Check hashes |
| `FBRO_E2E_HASH_GENERATE_ONLY=1` | Generate hashes, followed by Check hashes |
| `FBRO_E2E_SEARCH_ACTIVITY_ONLY=1` | Search progress, background jobs, and cancellation |
| `FBRO_E2E_SEARCH_ACTIONS_ONLY=1` | Search-result actions, duplicate cursor/selection preservation on reconnect, and focus |
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
| Diff viewer | `diff.mjs` | Up/Down scroll an entirely new file in split and unified views; split panes stay synchronized |
| Local files and UI | `local.mjs` | Create, copy, overwrite, rename, delete, sort, filtering, empty-folder suggestions, settings, bookmarks, Bros, clipboard feedback, grid navigation, and symlink routes |
| Bottom button bar | `button-bar.mjs` | Keyboard toggle in Settings → Display, palette recovery while hidden, backend-confirmed state shared between clients, frontend reload with the bar hidden, reclaimed layout space in both themes, file shortcuts and zoom while hidden, modal guards, opener restoration, and invalid-setting rejection. Rust settings tests cover older-file defaults and persistence round-trips. |
| File icons | `file-icons.mjs` (called by the local flow) | Exact extension keys, case normalization, and successfully loaded panel icons for HTML, PNG, JPEG, JSON, and TOML. Backend capability gates the cache control; on macOS keyboard clearing increments the icon revision and preserves button focus. Disk reuse, corruption recovery, and clearing are covered by `platform::macos::icon_cache::tests`. Native macOS PNG lookup is also covered by `platform::tests::native_extension_icons_are_png`; visually check both color schemes on macOS. |
| Panel selection and sizes | `panel-selection.mjs` (called by the local flow) | Space selects folders/files in list and grid views, calculates unknown sizes, reuses known/zero sizes, preserves cursor/focus, ignores held-key repeats, and respects modal/input/native controls; Shift+Space sizes selected/all folders without changing selection; Copy targets a selected folder despite a file cursor |
| Panel tabs | `tabs.mjs` | Real mouse reordering in both directions and themes, backend authority before drop, inactive-tab and other-panel focus preservation, sub-threshold clicks, Escape/outside/pointer-cancel/blur/modal/concurrent-close cancellation, focused terminal identity/PTY preservation, overflowing edge auto-scroll, frontend reload, close button, middle click and empty-strip double click. Rust tests cover invalid/stale/cross-slot/no-op requests, slot versions, and listing session order/active-index serialization with terminal exclusion. |
| Journal sorting | `journal-sort.mjs` (also called by the local flow; focused with `FBRO_E2E_JOURNAL_SORT_ONLY=1`) | Actual name/size/date ordering in both directions and rendered header/row agreement; remembered order after re-entry, keyboard refresh and frontend reload; sharing across panels and listing tabs; filesystem `/` preference isolation; disabled panel-local sorting without cache overwrite; re-enable restoration; reset removes Journal preferences and new audit events keep the current order; ordinary sorting adds no audit events. Rust tests cover millisecond date ordering and backend/session restoration. |
| Focus and modals | `focus-handoff.mjs`, `modal-focus.mjs`, and checks throughout the suite | Initial focus, exact opener restoration, covered layers ignoring keys, one submission per activation, nested Escape, backend rejection/removal, outside clicks, and preserving focus during toasts or toolbar actions |
| Notifications | `notifications.mjs` | Frontend clipboard publication without duplicates, timestamps/severity, FIFO burst delivery, keyboard history navigation, Clear focus handoff, clearing without cancelling delivery, overflow arrivals preserving the browsed row, both schemes, reload without replay, and OS notification capability/keyboard setting controls with backend confirmation, invalid-setting rejection, backend-only OS publication, and opener restoration |
| Thumbnails | `thumbnails.mjs` (also called by the local flow) | Platform capability controls, Jobs tab navigation, queue refresh preserving focus, visible-card loading, off-screen cooldown, viewport promotion, selection fill, and abandoning old requests |
| Viewers | `viewer.mjs` | Large and delayed images, J-next/K-previous rapid/reverse navigation, invalid-image recovery, text and binary content, horizontal arrow scrolling in small/large text with find-input cursor ownership, Enter/Shift+Enter find navigation from input/content with wrapping, native buttons, history priority, empty results and focus restoration, PDF, Markdown/HTML images, Mermaid, zoom, audio/video controls, preview keyboard ownership, and opener restoration |
| HEIC conversion | `heic.mjs` | Backend-confirmed opt-in settings, startup settings delivery, errors, optional real-file decoding, and live disabling |
| Quick Edit | `quick-edit.mjs` | Explicit target wins over a different cursor file without modifying it; keyboard invocation sends the displayed directory/filename; edit/save, terminal closure, listing focus restoration, journal entries, and editing a newly created empty file |
| Global selections | `global-selection.mjs` | Creation, reload persistence, adding through menus and long right-clicks, keyboard selection, focus, and removing references without deleting files |
| Selection masks | `selection-mask.mjs` (also called by the local flow) | +/− dialogs in list/grid views and both themes, Shift/numpad symbols, prefill and overtyping, additive/subtractive file matching, unrelated directory selection, input history including covered-history ownership, single submit, Cancel/Escape, held keys, command-palette handoff, filter typing, font zoom, opener restoration, and full-list matching/prefill in Mode B |
| Git | `git-init.mjs`, `git.mjs` | Initialization, branch creation/filtering/checkout, Local/Remote grouping, tracking checkout/reuse and conflicting-name entry, checkout rejection, validation and busy states, stage/unstage/rollback, changes/log navigation (J down, K up), active-log-row and inline-action Tab/Shift+Tab order without scrolling through inactive rows, expanded messages/files, history pagination with Space, journal entries, modal guards, and focus |
| Fedup | `fedup.mjs` | Global-selection input, default ignores, case-insensitive includes, and custom excludes |
| Hashes | `hash-check.mjs`, `hash-generate.mjs` | Discovery, stale targets, options, matching/malformed sidecars, skip/overwrite, reports/toasts, progress, background/reopen, cancellation, and preserving existing sidecars |
| Archives | `archive.mjs` | Enter a ZIP, copy a file out, and verify its content |
| Search | `search.mjs` | Activity without hits, inspected counts, cancellation, duplicate-name hints and viewer paths, toolbar/shortcut Copy with cursor and selection targets, repeated-filename exclusion/suffixes via inline and bottom actions, background copies, per-dialog defaults, both themes, Jump to location, Open With, duplicate cursor/selection preservation on reconnect, stale/deleted targets, modal keys, and date/size filters |
| Remote | `remote.mjs`, `job-files.mjs`, `remote-journal.mjs` | Folder suggestions and typed shares, validation/history, unsharing, pairing, read-only browsing, viewer full-image/partial-range outgoing job idle completion and resume while connected, job file keyboard expansion/navigation across pages, Tab/Shift+Tab through active-file Copy and Jump before adjacent jobs, copying real paths, revealing files and focus return in both schemes, copying, thumbnails, integrity, cancellation on both peers, a fresh transfer after cancellation, incoming/outgoing connection and explicit disconnection notification history, and durable Journal VFS payloads on both peers: correlated pairing/connection/job lifecycles, pairing revocation on both peers, every viewer idle interval under a stable operation UUID, 28-file backend path snapshots, actual bytes, cancellation, Sync Bro and serving manifest outcomes, secret-field exclusion, and retained records after clearing notification history with OS delivery disabled |

Several UI flows check both `fbro-dark` and `daylight`. Modal tests sample common
focus and cancellation paths; they do not cover every operation/conflict state.
Tab dragging uses native WebDriver mouse actions. Its middle-click close check
dispatches `auxclick` directly because WebKitWebDriver on Wayland reports a
middle-button release as button 0 and leaves the middle button held.
Search viewer fixtures are isolated from files created by earlier flows.
Created-time search checks depend on filesystem birth-time support.
Hash progress/cancellation tests use sparse 8 GiB files, so their apparent size
does not require 8 GiB of allocated disk space.

## Focused Rust tests

Notification state projection and toast queue tests: `npm run test:notifications`
(Node.js 22.19 or newer). Backend service, channel routing, retention, intent
validation, and producer ownership: `cargo test --manifest-path src-tauri/Cargo.toml notifications`.

OS notification checks cover the 30-second boundary, execution rather than queue
time, cancellation/failure classification, aggregate hash mismatches, duplicate
lifecycle events, peer/session correlation (including reverse-direction connect
and disconnect while the opposite direction stays open), incoming pairing expiry,
native activity handoffs, and disabled or failed delivery. Fake native delivery keeps
these policy checks independent of an actual desktop notification server.
The frontend projection tests include `os` records without frontend native delivery
or toast/snapshot replay. The Rust settings test
`os_notifications_default_enabled_for_legacy_settings_and_preserve_opt_out`
covers legacy defaults, persistence round-trips, and incorrect JSON value types.

The Linux native adapter has an ignored fixture using a private session bus:

```bash
dbus-run-session -- env FBRO_NOTIFICATION_TEST_BUS=1 cargo test --manifest-path src-tauri/Cargo.toml --locked linux_notifications_private_bus_roundtrip_and_missing_service -- --ignored --nocapture
```

It checks app identity/icon, escaped peer/file text, normal urgency, default
expiry, absence of native actions, and unavailable-service errors. More than 128
notifications without native closure signals verify that bounded handle retention
continues accepting deliveries. This fixture does not contact the user's desktop
notification service. Fake worker tests exercise activity epochs and suppression
of pending deliveries. The worker queue and retained native handles are each
bounded at 128; native service calls have a three-second timeout.

### Native OS notification validation

Automated policy and Linux fixture tests do not verify real desktop presentation.
Before release, manually validate Linux on Wayland and X11 with a notification
daemon, including an installed package/AppImage identity and icon:

1. Background fbro and finish a copy/search/hash-check job that runs at least 30
   seconds. Confirm one native alert and its retained history entry. Repeat a
   short operation and confirm no native completion alert.
2. Focus the main window, a detached viewer, and a detached diff in turn. Finish
   eligible work in each case and verify no native alert or focus stealing.
   Switch between fbro windows while completion occurs; return from another
   application before a pending send and check it is suppressed.
3. Toggle **Show background OS notifications** using the keyboard. Confirm
   disabling leaves history and in-app feedback intact, survives restart, and
   does not replay old alerts when reenabled.
4. Check incoming pairing, both directions of peer connections, transfers,
   failures, and hash mismatches. Pairing alerts must omit the code and offer no
   approval button. Remote end wording must not imply receiver verification.
5. Exercise Do Not Disturb and an unavailable notification service. Operations
   and history must remain correct; failed native delivery must not recursively
   create another notification.

macOS adapter and policy coverage runs on a native macOS host with the same Rust
`notifications` filter. It verifies packaged identity requirements, plain-text
Unicode payloads, distinct immediate request IDs, informational content, actual
authorization status classification, owned native error conversion, foreground
and enabled permission preparation, and denial/error suppression. Worker tests
hold authorization IPC open while focus or the preference changes and verify
that only a subsequent fresh event reaches native submission.

Build the macOS application with `npm run tauri -- build --debug --bundles app`.
Use `src-tauri/target/debug/bundle/macos/fbro.app` for native acceptance; bare
`tauri dev` and cargo test binaries deliberately expose no native OS delivery
capability. They still expose the macOS notification settings link. In a bare
development run verify the Notifications section remains visible, the delivery
checkbox is disabled with a development-build explanation in the info tooltip, and Tab/Enter reaches
and activates the installed app's system settings button. The
`macos_notifications_development_keeps_settings_available_without_native_delivery`
unit test verifies this capability separation on the bare test executable.
`npm run test:notifications:macos` briefly launches that bundle with an isolated
configuration, verifies the real backend's native capability and disabled
preference, then closes its own process. Before launch it verifies the complete
bundle signature, signing identifier `dev.fbro.app`, and bound Info.plist. A
linker-only executable signature must fail this check. It leaves a temporary log and neither
requests permission nor sends alerts. `FBRO_APP_BINARY` may select another
packaged executable. This check needs a logged-in macOS desktop session.
Run `npm run test:notifications:macos -- --request-permission` to additionally
exercise real authorization with the test app foregrounded. This opt-in mode
waits up to two minutes for Allow/Deny, fails on a native API error, and records
the native completion. The test activates its exact PID through AppKit so an
existing fbro instance cannot leave the test window in the background and skip
the foreground permission hook. A native error accompanied by a saved denied
authorization state is an ordinary denial, not a service failure.
macOS remembers the answer for fbro; it will not prompt
again after a decision. The test never answers or resets system permission.
`npm run test:notifications:macos -- --open-settings` exercises the explicit
backend action and correlated reply, opening the system notification pane for
fbro. It does not change notification permission or the app's delivery preference.

In addition to steps 1–5 above, validate:

1. On first enabled foreground launch, macOS requests alert permission for
   **fbro**. Grant permission and check System Settings → Notifications → fbro.
   Verify bundle identifier `dev.fbro.app`, application name, and bundled icon.
2. Deny permission on a fresh authorization state. History and toast/log feedback
   continue, without native alerts, repeated prompts, or stale delivery after a
   later grant in System Settings. Restart and confirm denial is respected.
3. Start with the app preference disabled. No permission prompt should appear.
   Enable it by keyboard while foregrounded; a first-time authorization request
   should appear. Enabling from a background observer must defer the request
   until a native window gains focus.
   In General → Notifications, Tab to **About OS notifications** and verify the
   tooltip includes the macOS permission notice (and development limitation for
   a bare executable). Check hover and Enter/Space as well. Escape must dismiss
   only the tooltip and retain info-button focus; Tab dismisses it while moving
   to the next enabled control. The help must not occupy section space when closed.
   From the delivery checkbox press Tab to **Open notification settings**, then
   Enter or Space. System Settings should open Notifications → fbro without
   changing either permission or the fbro preference. Return to fbro and verify
   the settings modal and button focus are retained. Repeat in both color schemes;
   rapid activation must send one request, and launch failure must appear inline.
4. Turn macOS notifications off and on in System Settings while the app runs.
   Only newly eligible events deliver. Check banners/Notification Center and
   Focus/Do Not Disturb; no sound, badge, approval action, or foreground banner
   is requested. System settings may retain entries without displaying banners.

Real macOS permission UI and desktop presentation remain manual acceptance
checks; unit payloads and Linux fixtures do not verify them.

Windows adapter coverage runs on a native Windows host with
`cargo test --manifest-path src-tauri/Cargo.toml --locked windows_notifications`.
It verifies installed identity: a matching Start Menu target and `dev.fbro.app`
pass, while `target/debug`, `target/release`, a dependency test executable, a
different link target, a mismatched id, and the PowerShell AppUserModelID fail.
It also checks that `tauri.conf.json` `identifier` equals the adapter constant,
that payloads stay plain, silent, short, and actionless, that the authorization
predicate allows a missing value or `1`, denies an explicit `0`, and errors on
any other number, that the settings URI is exactly `ms-settings:notifications`,
and that shortcut candidates use Tauri's per-user and per-machine Start Menu
layouts (`fbro\fbro.lnk` and `fbro.lnk`). The development capability test
requires the test executable to stay inside Cargo's build tree:
`supports_os_notifications` is false and `supports_os_notification_settings`
stays true, so the test never initializes WinRT. None of these tests post a toast.

`tauri dev` and other build-tree binaries deliberately expose no native OS
delivery. They still expose the Windows notification settings link. In a
development run, verify that the Notifications section remains visible, the
delivery checkbox is disabled, and the info tooltip says the installed fbro
application is required. Tab reaches **Open notification settings**; Enter or
Space opens Settings → System → Notifications and does not change a Windows
toggle or the fbro preference.

Build and install an NSIS or MSI package (`npm run tauri -- build`) before
toast acceptance. The installer stamps `dev.fbro.app` on the Start Menu
shortcut; the adapter does not register one itself. In addition to steps 1–5
above, validate on Windows 10 and Windows 11:

1. Confirm the Start Menu shortcut (`fbro\fbro.lnk` or `fbro.lnk`, per-user or
   per-machine) has `System.AppUserModel.ID` `dev.fbro.app` and targets the
   installed executable. The toast name and icon come from that shortcut.
2. Background the installed app and finish an eligible job. Confirm one silent
   short toast and its retained history entry. A click may focus the app.
   There is no button, protocol argument, or extra action.
3. In General → Notifications, Tab to **About OS notifications** and verify
   the tooltip includes the Windows banner notice. For an unpackaged
   executable it also includes the installed-application limitation. Escape
   dismisses only the tooltip and keeps focus on the info button. Tab to
   **Open notification settings**, then Enter or Space. Windows Settings
   should open the Notifications page without changing a system toggle or the
   fbro preference. Repeat in both color schemes.
4. Turn fbro's banner off and on under Settings → System → Notifications →
   fbro, and turn the global toast toggle off and on, while the app runs.
   Only newly eligible events deliver. A missing per-app value allows
   delivery. An explicit disable discards that event and leaves history intact.
5. Check Focus assist. It may hide banners. fbro does not override it. If the
   toast shows a blank line under the title, record it: `notify-rust` 4.18
   emits an empty subtitle element, and switching the submission path is
   deferred until that is visible on an installed package.

Real Windows toast presentation, the shortcut icon, Focus assist, and installed
MSI/NSIS behavior remain manual acceptance checks. They have not been run for
this adapter. Unit tests do not post a toast.

On the development macOS host, packaged startup/signature and real saved-denial
handling have been verified. Info-tooltip focus opening and Escape dismissal
(retaining the Settings modal and info-button focus) were verified in both
themes. The native notification-settings notice, Tab/Enter
activation, actual Notifications → fbro destination, focus return to the button,
and Escape dismissal were verified in both fbro-dark and daylight using the
native accessibility helper and an isolated app configuration.
Grant/banner/Do Not Disturb acceptance remains
pending. The denial regression test verifies that `UNErrorDomain:1` with actual
denied status does not re-arm permission requests, while the same native error
with undetermined status remains diagnosable and retryable.

Append a filter to the Rust test command:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --locked patch_tests
```

| Filter | Coverage |
| --- | --- |
| `patch_tests` | Delayed scan patches preserving newer cursor/selection/version, queued entries/completion/errors, and independent window updates |
| `live_listing_` | Live cursor/selection and scroll-anchor reconciliation, including exact duplicate search paths and removed results |
| `deletion_recovery` | Delayed deletion events from previous locations, recreated directories, recovery from a genuinely deleted current folder, and newer navigation winning before recovery mutates state |
| `ws::intents::panel::tests` | Cursor/range selection, Space folder selection and size reuse, running-size preservation, full-list size targets beyond render windows, supported drivers, and stale archive completion guards |
| `ws::intents::panel::tests::selection_masks*` | Whole-name wildcard alternatives, literal punctuation, Unicode/case/newlines, extensionless *.*, file and symlink eligibility, selection merge/subtraction, stale epochs, and matches beyond the render window |
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
| `ws::intents::terminal::tests` | Explicit Quick Edit targets independent of cursor/selection/render window, required arguments, changed locations, regular-file restrictions, and spaces/Unicode in paths |
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
