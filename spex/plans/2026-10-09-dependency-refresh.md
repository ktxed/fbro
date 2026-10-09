# Dependency refresh implementation plan

**Goal:** Refresh frontend and backend dependencies and verify the existing application behavior.

**Architecture:** Keep the existing React/Tauri stack, backend authority, and platform adapters. Update compatible release lines; migrate Vite and russh to supported patched versions. Defer unrelated breaking migrations to keep this maintenance pass reviewable.

**Constraints:** Work on `development`. Do not commit before user validation. Preserve the configured WebView browser targets. Keep sources, keyboard, and modal behavior stable; document the scan-patch replay correction discovered during verification.

## Frontend

- [x] Refresh direct compatible versions in `package.json` and regenerate `package-lock.json` with npm.
- [x] Upgrade Vite and its React plugin together, checking the upstream migration guide and `vite.config.ts`.
- [x] Run `npm run build`, `npm run test:build-info`, and `npm audit`; record remaining advisories and major-version deferrals.

## Backend

- [x] Refresh `src-tauri/Cargo.lock` and the helper lockfile `tools/lfs-spy/Cargo.lock` within existing release constraints.
- [x] Upgrade russh to a patched release, adapting source APIs only as required after reading the relevant specs.
- [x] Run Rust tests and check the helper build; inspect dependency security results.

## Integration

- [x] Build the Tauri debug application and run the existing Linux smoke suite.
- [x] Review manifest/lockfile changes and document verification, limitations, and deferred migrations in this plan and `TESTING.md`.
- [x] Report results for user validation, leaving all changes uncommitted.

## Decisions and verification

- Vite 8.3.4 uses Oxc for minification; retain the existing explicit browser targets.
- React/ReactDOM 19.3.0, Tauri JS API 2.12.2 and CLI 2.12.1, and compatible frontend packages are refreshed. WebdriverIO 10.0.2 replaces vulnerable test-tool dependencies; migrate `executeAsync` to an async `execute` function and make the Bro/PDF readiness selectors unique. Explicitly select the first duplicate search row and isolate the search viewer fixtures from earlier local-flow files. Use WebdriverIO's documented `strictSelectors: false` compatibility setting to preserve v9 first-match behavior in existing collection readiness checks, including Git's multi-file commit actions.
- Pin PDF.js exactly to **5.5.207**. The previous 5.7.284 is in the affected range of [GHSA-hq66-cqwq-w95j](https://github.com/advisories/GHSA-hq66-cqwq-w95j), introduced in 5.6.83. PDF.js 6 raises the legacy browser floor to Chrome 125/Safari 18 and removes polyfills. The unaffected 5.5 release retains legacy library and worker polyfills without custom replacements.
- Node.js minimum is 22.19.0 (WebdriverIO 10); the Docker builder copies the official Node 24 Bookworm distribution instead of using Debian's older Node packages.
- russh 0.64.1 uses explicit session-channel acceptance and `PublicKeyOrCertificate`; preserve paired raw Ed25519 pinning and reject certificates. Disable unused RSA support. Two unit tests cover matching/mismatching keys and rejecting a certificate for the paired key.
- `npm ci`, `npm ls --depth=0`, frontend build, build-info test, Tauri debug build, 362 Rust tests, and helper `cargo check --locked` pass.
- Full-smoke hash generation exposed delayed atomic scan patches replaying older selections and decreasing backend panel versions. Guard stale replay while retaining uncommitted streamed entries/completion/errors and independently versioned windows. Four regression tests pass; the selection test reproduced the bug before the fix. Focused Linux hash-generation/check smoke passes. Update `proto.md` with the replay invariant.
- The final rebuilt application's focused native Linux/Wayland remote smoke passes, including pairing, browsing, transfer completion, cancellation on both peers, retry after cancellation, and focus restoration.
- The complete native Linux/Wayland smoke suite passes on the final debug binary, including local operations, viewers, Git, hashing, archives, Search, and remote operations. An earlier full run passed local flows but timed out starting the second frontend window; focused remote and the final complete rerun both passed. Do not attribute that transient startup timeout to a dependency without further evidence.
- npm audit: **26 findings → 2 low findings** (`mermaid`/`katex`, [GHSA-238p-pmpm-9mq7](https://github.com/advisories/GHSA-238p-pmpm-9mq7)). No moderate/high/critical findings. Mermaid 12 also retains the affected KaTeX dependency, so retain Mermaid 11.17.2. Do not force npm's proposed downgrade to Mermaid 10.
- RustSec audit: **6 vulnerabilities → 0** in the backend and 0 in the helper. Backend warnings remain for `glib` 0.18.5 (RUSTSEC-2024-0429), `instant` (RUSTSEC-2024-0384), and `proc-macro-error` (RUSTSEC-2024-0370). Tauri/WebKitGTK requires the glib 0.18 ecosystem; a direct glib upgrade cannot replace its dependency tree.
- Defer unrelated breaking migrations: TypeScript 7, xterm 6 and addon-fit 0.11, Tooltip 6, Mermaid 12, and incompatible Rust release lines (database, archive, discovery, system-info, crypto, WebSocket, TOML, diff, and Markdown libraries). Current compatible release lines are refreshed in the locks.
- Repository-wide `cargo fmt --check` reports extensive pre-existing formatting differences; avoid reformatting unrelated source.
- Native macOS/Windows and the full Docker source-build image require separate verification.
