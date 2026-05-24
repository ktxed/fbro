# Hybrid E2E Host Setup Follow-Up Plan

## Context

The hybrid E2E implementation is in place: Docker runs the isolated fbro
backend instances, while the host runs WebDriverIO, `tauri-driver`, and local
frontend-only Tauri windows.

The project-level Node dependencies now install and resolve locally:

- `webdriverio`
- `ws`

`tauri-driver` is installed at:

- `/home/ktx/.cargo/bin/tauri-driver`

The local smoke runner now preflights required host dependencies before it
starts Docker or writes artifacts.

## Remaining Host Blockers

### 1. Docker daemon access

Current failure:

```text
Docker is installed, but this user cannot access the Docker daemon
```

The user running the E2E command must be able to access
`/var/run/docker.sock`.

Likely fixes:

```bash
sudo usermod -aG docker "$USER"
newgrp docker
```

or run the smoke command from a shell/session that already has Docker daemon
access.

Verification:

```bash
docker version --format '{{.Server.Version}}'
```

### 2. Native WebKit WebDriver

`WebKitWebDriver` is not on `PATH`.

Install the distro package that provides it. On Debian/Ubuntu-like systems:

```bash
sudo apt-get update
sudo apt-get install -y webkit2gtk-driver
```

Verification:

```bash
command -v WebKitWebDriver
```

### 3. Artifact directory ownership

`artifacts/e2e` currently exists as `nobody:nogroup`, so the normal user
cannot create new subdirectories there.

Fix ownership:

```bash
sudo chown -R "$USER:$USER" artifacts
```

Verification:

```bash
mkdir -p artifacts/e2e/logs
```

## Final Validation

After the host blockers are fixed, run:

```bash
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
node --check tests/e2e/smoke.mjs
npm run e2e:smoke:local
```

Expected smoke behavior:

- Docker starts `fbro-a` and `fbro-b`.
- Backends listen on host WebSocket ports `17878` and `27878`.
- Local `tauri-driver` processes listen on separate ports.
- Local frontend-only Tauri windows connect to the dockerized backends via
  `FBRO_WS_URL`.
- Smoke scenarios cover local file operations, archive copy-out, search, and
  remote pairing/copy between the dockerized backends.

## Notes

The runner intentionally checks Docker, `tauri-driver`, `WebKitWebDriver`, and
the local fbro binary before creating artifact directories or starting Docker.
This keeps failures early and explicit when the host is not ready.
