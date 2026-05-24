# Project context

`fbro` is an n-panel "orthodox" file manager.

The `spex` folder contains the project's specification:

- `spec.md` - overview
- `proto.md` - client-server communication between frontend and backend
- `vfs.md` - virtual file system driver abstraction

## Technologies

fbro is developed with Tauri:

- Backend: Rust
- Frontend: React/TypeScript
- Frontend and backend communicate over WebSocket
