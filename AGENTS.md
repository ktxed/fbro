# fbro — agent guide

`fbro` is a keyboard-first, 2-panel "orthodox" file manager built with Tauri:
a Rust backend (`src-tauri/`) and a React/TypeScript frontend (`src/`) talking
over WebSocket.

- **The backend is authoritative.** It owns state, selection resolution,
  settings, and every file operation. The frontend renders snapshots and
  `StatePatch`es and sends intents — no optimistic state, no frontend-derived
  selections, no user preferences in `localStorage`.
- **Keyboard first.** Every feature must be fully usable without a mouse.
- **Linux is primary; macOS and Windows are supported.** Anything touching the
  platform must consider all three (tool discovery, WebView2 styling, Cmd vs Ctrl).

The rules below are terse; the linked spec holds the detail. Read the relevant
spec before changing an area.

## Spec map (`spex/`)

| Spec | Scope — read before… |
|---|---|
| `spec.md` | Architecture overview, backend-authoritative model |
| `proto.md` | WebSocket protocol (FMP v0): intents, jobs, state patches — adding messages |
| `vfs.md` | `VfsDriver` abstraction — adding or changing a driver |
| `sources.md` | How locations become sources, lifecycles, tab mapping |
| `platform.md` | Platform Abstraction Layer — any OS-specific behaviour |
| `keyboard.md` | Input ownership and priority — any key handling |
| `modal-stack.md` | `ModalLayer` invariants — adding or changing a dialog |
| `modal-keyboard-audit.md` | Known modal keyboard pitfalls (Enter, double submit, focus) |
| `settings.md` | Typed backend settings tree — adding a setting |
| `tabs.md` | Tabbed panel mode |
| `search.md` | Backend search jobs and result sources |
| `bro.md` | "Pass to Bro" processors — adding a bro |
| `remote-fbro.md` | Pairing and remote browse/transfer |
| `thumbnails.md` | Grid-view thumbnail pipeline |
| `virtual-panel.md` | Rendering huge directories |
| `inline-operations.md` | Inline choices toolbar |

`spex/plans/` holds plans and backlog, not specs. A change to protocol, source,
keyboard, or modal behaviour **updates the matching spec in the same commit**.

## Commands

- Dev: `npm run tauri dev` · with local-FS spy: `npm run spy` + `tools/lfs-spy`
- Frontend type check + build: `npm run build` (`tsc` is the only frontend check)
- Rust tests: `cargo test --manifest-path src-tauri/Cargo.toml [filter]`
- E2E smoke (fast host run, see `TESTING.md`):
  `npm run tauri -- build --no-bundle && FBRO_E2E_COMPOSE_FILES=docker-compose.e2e.host.yml GDK_BACKEND=wayland npm run e2e:smoke:local`
- Build-info: `npm run test:build-info`

## Frontend rules

**Feedback**
- Every copy-to-clipboard button is a `<CopyButton text what />`
  (`src/components/CopyButton.tsx`); other copy actions (keyboard shortcuts,
  menu items) call `useCopyToClipboard()` (`src/hooks/`). Both toast
  "Copied <what>" — `what` is a lowercase noun: "full path", "commit hash", "3 names".
  Never call `navigator.clipboard` directly.
- Backend-side copies (files, raw git blobs) reply with a `*ClipboardWritten`
  message; the reducer turns it into a toast. Add one for any new backend copy.
- Other one-shot confirmations: `dispatch({ type: "SHOW_TRANSIENT_HINT", message })`.
  The separate viewer/diff windows have no store, so no toasts there.

**Modals** — `spex/modal-stack.md` §5
- Every dialog is `<ModalLayer modalId>` wrapping `ModalDialog` or
  `ResizableModal` (for content-heavy dialogs; it persists its size).
  No `window` key listeners; use `useModalStack().hasActiveModal`, never a new
  "is X open" boolean.
- Initial focus via `initialFocusRef` / `data-modal-initial-focus`, not a
  `useEffect` calling `.focus()`. Effects may only move focus in response to a
  later state change (a new job state, data finishing loading).
- Enter: check `e.defaultPrevented` first, guard default actions with
  `isNativeActivationTarget` (`modalKeyboard.ts`), never submit twice. While
  submitting, disable the primary button and label it "Verb…".
- Escape dismisses nested state first (find bar, confirmation). Escape on a
  running job sends it to the background, never cancels it.
- `.modal__actions`: primary button first, then Cancel.
- Lists/tablists: one Tab stop, arrows + Home/End via `rovingIndexForKey`.

**Focus**
- After a modal, viewer, or inline row action (e.g. copy) completes, focus
  returns to the row or panel it started from (`CopyButton`'s `onCopied`
  is the hook for inline rows).
- Every component that takes focus must capture its opener first and hand
  focus back on every close/removal path, including cancellation, completion,
  and backend rejection. Restore the exact available opener; otherwise fall
  back to the active listing. Preserve focus deliberately taken by a successor
  or an outside click. Reuse `ModalStack` / `useFocusReturn` and the shared
  removal refs; do not add competing focus-restoration callbacks.
- Informational toasts and toolbar/job-bar mouse actions preserve existing
  focus. Fix ownership and lifecycle handoffs; never add polling or hover-based
  focus recovery loops.
- Keep the active item visible: `scrollIntoView({ block: "nearest" })`.

**Keyboard** — `spex/keyboard.md`
- Commands and default bindings are defined in the backend
  (`src-tauri/src/ws/commands.rs`, `mod_binding` for Cmd/Ctrl). Wire the
  frontend in `src/commands/interactive.ts` (id lists + `executeCommand`), and
  in `src/contextualOperations.ts` / `src/contextMenu.ts` for file operations.
- Match registry commands with `bindingMatches`; match local letter shortcuts
  (modals, viewers) with `isLetterKey` / `shortcutLetter` from `modalKeyboard.ts`.
  Both fall back to physical `event.code`, so macOS Option and non-Latin
  layouts work. Never compare `e.key` to a letter by hand.
- Show registry shortcuts with `formatPrimaryBinding`, never a hard-coded label.
  Platform modifier: `modKey()` from `src/utils/platform.ts`.
- Local shortcut hints: `<MnemonicLabel text mnemonic />` underlines the key
  when it is a letter of the label; otherwise append a `<kbd>` key cap. Either
  way, set `aria-keyshortcuts` on the control when you can.

**Styling and components**
- Colours via `var(--…)` tokens in `src/styles.css`; check both `fbro-dark` and
  `daylight`. Literal colours belong only in the `:root` /
  `[data-color-scheme]` blocks; a deliberately fixed colour (video letterbox,
  PDF paper) carries a comment. New tokens need a daylight value. Content rendered through `FloatingPortal` sets its own
  `data-color-scheme` (`resolveColorScheme`).
- Custom-styled controls need `appearance: none; font: inherit` and explicit
  colours, or Windows WebView2 paints native chrome.
- Icons: `@tabler/icons-react` only; `aria-hidden` next to a text label;
  icon-only buttons need `title` + `aria-label`.
- Controls with a fixed position render after variable-width badges, so they don't shift.
- Number inputs: `onWheel={stepNumberInputOnWheel}`. Prompt text inputs:
  `useInputHistory`. Ellipsis is "…", never "...".
- Interactive elements driven by smoke tests get `data-testid` or `data-command-id`.

## Backend rules

**Protocol** — `spex/proto.md`
- New intent: arm in `ws/intents/mod.rs::dispatch` → domain `handle()` → parse
  with `arg_str`/`arg_bool`/`arg_u64`/…; add mutating ones to `should_record_command`.
- New `ServerMessage` variant: there is no codegen. Mirror it by hand in
  `src/ws/types.ts`, add it to the `BEMessage` union, and handle it in
  `src/store/index.ts`. Serde renames must match TS field names exactly.
- Naming: intents PascalCase VerbNoun; replies `*Ok` / `*Ready` / `*Failed` /
  `*Result`, correlated by `requestId`; error codes SCREAMING_SNAKE.
- `StatePatch` versions increase per domain/entity; related patches go out
  atomically and in order.

**VFS and jobs** — `spex/vfs.md`
- User-visible file operations on a source go through `VfsDriver`. Drivers are
  pure adapters: no UI, no prompts, typed `VfsError` (`NotSupported` for
  unsupported ops). Async-backed drivers own a runtime and `block_on` (see `vfs/sftp.rs`).
- Filesystem mutations happen only in jobs. Long work reports through
  `ProgressReporter` (throttled) and honours cancel / pause / skip.

**Async**
- `VfsDriver` is synchronous: driver calls, sync I/O, and heavy CPU in async
  code go through `tokio::task::spawn_blocking`.
- Hold the `SharedState` lock briefly: clone the `Arc`s or senders you need,
  then release it before any `.await` or blocking call.

**Platform** — `spex/platform.md` §8
- OS code lives in per-OS files (`platform/`, `storage/`, …) behind a trait
  method with a no-op default. The frontend never queries the platform directly.
- External tools are probed per OS (Windows: PATH, Scoop, Choco, WinGet).

**Bros** — `spex/bro.md`
- New provider: copy `bro/providers/tar.rs`, register in `builtin_providers()`,
  add pre-bro scoring and a catalog entry, add tests, update `bro.md`.
  Bro ids are `native:<kebab-name>` or `builtin:<kebab-name>`.
- Media players get a playlist file (`LaunchArg::TargetPlaylistFile`), never
  expanded paths (ARG_MAX).

**Style**
- Errors: `VfsError` in drivers, `Result<_, String>` with context
  (`map_err(|e| format!("…: {e}"))`) elsewhere.
- Logging: `eprintln!("[subsystem] …")`; there is no logging crate.

## Testing

- New Rust logic gets unit tests: inline `#[cfg(test)] mod tests` or a sibling
  `tests.rs`, with small fake drivers and temp dirs cleaned by a `Drop` guard.
- User-visible flows get a smoke case in `tests/e2e/smoke/cases/`, especially
  modal, keyboard, and focus behaviour: the covered layer is inert, only the top
  layer handles keys, and focus is restored on close.
- Linux fixtures don't replace native macOS/Windows checks. Say which
  platforms were not verified. Keep the coverage list in `TESTING.md` current.

## Workflow

- **Never commit before the user has validated the change.** Implement, report,
  then wait for an explicit request to commit.
- Work on `development`; feature branches get squashed.
- Conventional Commits with a scope: `viewer`, `git`, `panel`, `tabs`,
  `search`, `terminal`, `ui`, `appimage`, …. The body explains cause and fix.
- **Prefer battle-tested OSS over custom protocols, formats, or components.**
  List the existing options first and justify any custom work (e.g. remote
  fbro became SFTP over system ssh rather than a custom RPC).
- Releases follow [RELEASING.md](RELEASING.md).

## Known debt

None tracked right now. If you knowingly leave code that breaks a rule above,
list it here (file + rule) so it isn't copied as precedent.
