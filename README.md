# fbro

<p align="center">
  <img src="logo256.png" alt="fbro" width="96" height="96" />
</p>

**fbro** is a dual-pane orthodox file manager for power users.

Built with [Tauri](https://tauri.app/) (Rust backend + React frontend). The backend owns state; the UI talks over WebSocket via intents and jobs.

> Status: early (`0.1.0`). Developed primarily on Linux; other platforms may work with gaps.

## Features

- Dual panels, tabs, F-key style operations, command palette
- Interruptible copy/move/delete with conflict handling and job queue
- Archives as folders (zip/7z), search results as a panel source
- **Bros** — processors for pack/extract, duplicates (Fedup), local↔remote integrity sync
- Git status and actions in the panel
- Embedded terminal + quick edit
- Viewer (text/hex/markdown/mermaid/media) and file compare
- Peer remote: pair two fbro instances, browse and transfer over SFTP

## Development

Requirements: [Node.js](https://nodejs.org/), [Rust](https://rustup.rs/), and [Tauri 2 system deps](https://v2.tauri.app/start/prerequisites/) for your OS.

```bash
npm install
npm run tauri dev
```

```bash
npm run tauri build
```

Smoke tests: see [TESTING.md](TESTING.md).

## Design docs

Architecture and protocol live under [`spex/`](spex/):

| Doc | Topic |
|-----|--------|
| [spec.md](spex/spec.md) | Overview |
| [proto.md](spex/proto.md) | FE ↔ BE protocol |
| [vfs.md](spex/vfs.md) | Virtual file system |
| [bro.md](spex/bro.md) | Bro processors |
| [remote-fbro.md](spex/remote-fbro.md) | Peer remote |

## License

License TBD.
