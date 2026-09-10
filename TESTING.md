# Testing

This project currently has a WebDriver smoke test for the Tauri app in
`tests/e2e/smoke/index.mjs`.

## Requirements

Install the normal project dependencies first:

```bash
npm install
```

The smoke test also requires:

- Docker with Compose support, accessible by the current user.
- `tauri-driver` in `PATH`.
- `WebKitWebDriver` in `PATH`.
- System runtime dependencies for building and running a Tauri/WebKitGTK app.
- `zip` for the archive scenario.

On Linux/Wayland, use the system `WebKitWebDriver` plus `tauri-driver`; the
local WebDriver-controlled app windows can be run with `GDK_BACKEND=wayland`.
The Dockerized backend app instances still run under `xvfb-run`.

## Smoke Tests

Storage-policy regressions are covered by backend tests:

```bash
cargo test --manifest-path src-tauri/Cargo.toml storage
cargo test --manifest-path src-tauri/Cargo.toml viewer_prefetch
cargo test --manifest-path src-tauri/Cargo.toml thumb_pool
```

Linux fixtures cover internal NVMe/SATA, rotational disks, USB/MMC readers,
removable PCI devices, remote mounts, partitions, mapped/mixed devices, unresolved
members, hidden nested mounts, symlink targets, and device replacement. Instrumented
viewer and thumbnail tests verify rejected work does not call its provider and
foreground thumbnails still work. Windows storage-descriptor fixtures also run on
Linux, but do not exercise Win32 device queries. Tests inject registries only in test code;
there is no runtime eligibility override.

On native macOS and Windows, additionally verify source `storageClass` and
`prefetchEligible` on internal SSDs, external SSDs/card readers, network volumes,
and multi-device volumes. Unknown devices must stay disabled. Verify unplug/remount
updates within the discovery interval and does not reuse old viewer cache entries.
Linux fixture tests do not replace these native platform checks.

The smoke test starts two fbro backend/app instances named `fbro-a` and
`fbro-b`, starts two local `tauri-driver` processes, drives the UI through
WebDriver, and observes backend state over WebSocket.

Run the default smoke test:

```bash
npm run e2e:smoke
```

This uses `docker-compose.e2e.yml`. That image builds the app inside Docker,
which is reproducible but slower.

For faster local iteration, build the app once on the host and run the host
runtime image:

```bash
npm run tauri -- build --no-bundle
FBRO_E2E_COMPOSE_FILES=docker-compose.e2e.host.yml GDK_BACKEND=wayland npm run e2e:smoke:local
```

The host-runtime compose file builds only a runtime image and bind-mounts
`src-tauri/target/release/fbro` into both containers. Both `fbro-a` and
`fbro-b` use the same image.

For backend/debug iteration, build the debug binary and use the debug local
script:

```bash
GDK_BACKEND=wayland npm run e2e:smoke:local_debug
```

This uses `docker-compose.e2e.host.yml` plus `docker-compose.e2e.debug.yml`,
bind-mounts `src-tauri/target/debug/fbro` into both containers, and launches
the frontend-only app windows from the same debug binary. The script builds
that binary with `tauri build --debug --no-bundle` first; a plain `cargo build`
is not enough for this harness because it creates a dev-mode binary that tries
to load `http://localhost:5173`.

### Hybrid Host/Docker Layout

The local smoke mode is hybrid: WebDriver and the frontend-only app windows run
on the host, while the two backend-capable fbro instances run in Docker. The
host app windows connect to the Dockerized instances over WebSocket.

```mermaid
flowchart LR
  subgraph Host["Host"]
    Smoke["tests/e2e/smoke/index.mjs"]
    TD_A["tauri-driver :4444"]
    TD_B["tauri-driver :4445"]
    WK_A["WebKitWebDriver :4446"]
    WK_B["WebKitWebDriver :4447"]
    FE_A["frontend-only fbro-a window"]
    FE_B["frontend-only fbro-b window"]
    Bin["src-tauri/target/release/fbro\nor target/debug/fbro"]
    Artifacts["artifacts/e2e"]
  end

  subgraph Docker["Docker Compose"]
    Runtime["fbro-e2e-host runtime image"]
    A["container fbro-a\nWS :17878 -> :7878\nfileserver :17881"]
    B["container fbro-b\nWS :27878 -> :7878\nfileserver :27881"]
    WorkA["/work/a + /state/a"]
    WorkB["/work/b + /state/b"]
  end

  Smoke --> TD_A
  Smoke --> TD_B
  TD_A --> WK_A --> FE_A
  TD_B --> WK_B --> FE_B

  FE_A -->|"ws://127.0.0.1:17878"| A
  FE_B -->|"ws://127.0.0.1:27878"| B
  Smoke -->|"observer WebSocket"| A
  Smoke -->|"observer WebSocket"| B

  Bin -->|"bind-mounted read-only"| A
  Bin -->|"bind-mounted read-only"| B
  Runtime --> A
  Runtime --> B
  Artifacts -->|"bind mount"| WorkA
  Artifacts -->|"bind mount"| WorkB
  A --- WorkA
  B --- WorkB
  B <-->|"pairing + remote SFTP"| A
```

Useful environment variables:

- `FBRO_E2E_VIEWER_ONLY=1`: run only the image viewer regression flow. It covers
  delayed loads, more than 21 high-resolution JPEGs, reverse/rapid navigation,
  and recovery from an invalid image. Timings and process-memory samples are
  written to `artifacts/e2e/viewer-timings.json`; they include WebDriver and a
  local forwarding server's overhead and are not a standalone rendering benchmark.
- `FBRO_E2E_COMPOSE_FILES`: compose file list used by local smoke runs.
- `FBRO_APP_BINARY`: host app binary used by the frontend-only WebDriver
  windows. The debug script sets this to `src-tauri/target/debug/fbro`.
- `FBRO_E2E_ARTIFACTS`: artifact output directory. Defaults to `artifacts/e2e`.
- `FBRO_E2E_KEEP_DOCKER=1`: keep Docker containers after the run for debugging.
- `FBRO_A_WEBDRIVER_PORT`, `FBRO_B_WEBDRIVER_PORT`: tauri-driver HTTP ports.
- `FBRO_A_NATIVE_WEBDRIVER_PORT`, `FBRO_B_NATIVE_WEBDRIVER_PORT`: native
  WebKitWebDriver ports used under tauri-driver.

By default the smoke harness runs Docker cleanup in `finally`:

```bash
docker compose ... down --remove-orphans
```

Viewer diagnostics use the existing backend stderr log: `[viewer navigation]`
reports navigation preparation time, `[viewer http]` distinguishes cache hits
from disk reads, `[viewer prefetch]` reports scheduling/read/eviction activity
and allocated cache bytes, and `[thumbnail]` reports queue waits and generation
times. The prefetch cache is limited to 21 entries and 256 MiB, with two readers;
foreground loads do not wait for a prefetch slot.

Run the deterministic prefetch regression tests with:

```bash
cargo test --manifest-path src-tauri/Cargo.toml viewer_prefetch
```

## What The Smoke Test Covers

The smoke test exercises these flows:

- Local file operations on `fbro-a`:
  - Create directories and files.
  - Copy from left to right panel.
  - Rename and verify renamed content.
  - Delete and verify removal.
  - Sort entries by name, size, and date in both directions.
- Archive browsing: create a zip file, enter it from the UI, copy a file out of
  the archive, and verify extracted content.
- Search: search for `renamed.txt`, mount the search results source, and verify
  the result appears in the panel.
- Datetime search: verify the Created/Modified time filter toggle works correctly.
  Creates a file with mtime set to two years ago and btime≈now, then searches
  with a "Between" interval covering current year ±1.  The modified filter must
  exclude the stale-mtime file (mtime outside the interval); the created filter
  must include it if the filesystem reports birth time.
- File size search: verify the size filter works for both "Bigger than" and
  "In range" operators.  Uses the sort-test files with known byte sizes: 500 B,
  100 B, and 10 B.  Asserts that only the correct file matches each filter and
  that all other files are excluded from results.
- Remote sharing: enable sharing on both instances, pair `fbro-b` to `fbro-a`,
  mount `fbro-a`'s shared `/work/a` source on `fbro-b`, verify the remote source
  is read-only, copy `remote.txt` from remote `fbro-a` into local `/work/b/dst`,
  and verify the copied payload.

## Artifacts

The smoke test does not currently emit a formal test report such as JUnit or
HTML. It writes diagnostics under `artifacts/e2e`:

- `final-tree.txt`: final filesystem tree for both instances.
- `fbro-a.log`, `fbro-b.log`: container app logs.
- `logs/tauri-driver-a.log`, `logs/tauri-driver-b.log`: local driver logs.
- `transcripts/fbro-a.json`, `transcripts/fbro-b.json`: WebSocket observer
  transcripts.
- `screenshots/fbro-a-failure.png`, `screenshots/fbro-b-failure.png`: failure
  screenshots when a browser session exists.

The authoritative pass/fail signal is the process exit code.
