# Built-in editor tab with autosave, and notes

**Goal:** Edit small text files in a CodeMirror editor tab opened next to the listing tab it came from. Everything saves automatically. Escape returns to the origin tab and leaves the editor tab open. Notes are plain `.md` files kept by a note store, reachable from the source bar; the first store is a scratch folder, and the note API is shaped so a different store (for example an Obsidian vault) can replace it later.

**Architecture:** Two layers with separate intents.

- **Editor layer (file-level).** `TabKind::Editor`: a panel in the same slot as its origin listing tab, inserted right after it, remembering that origin. The backend owns editor sessions (target, loaded-bytes hash, fingerprint, encoding details), every check, every write, and session persistence. The frontend holds only the editor state (text, undo history, selection), cached per session so it survives tab switches. It knows nothing about notes.
- **Notes layer.** A `NoteStore` trait in `src-tauri/src/notes/` with one implementation in v1, `FolderNoteStore` over `config_dir()/scratch`. Note intents (`CreateNote`, `OpenNotes`) talk only to the active store through the trait. A store hands back a local file to edit, and the editor layer opens it like any other file.

Because saves are automatic, there is no "unsaved changes" prompt anywhere, and editor tabs can be restored after a restart like listing tabs.

**Constraints:** Do not work on `development`. Before the first change, create and switch to a new branch `editor-tab-notes` from the current `development` (`git switch -c editor-tab-notes development`); all work happens there, and it is squashed when merged. Do not commit before user validation. Protocol, source, keyboard, tab, and modal changes update the matching `spex/` spec in the same change. Linux is the verified platform; macOS and Windows paths are written for all three but not verified unless a native run is performed. Prefer OSS components over custom editor formats.

**Supersedes:** `2026-11-01-snippets-quick-edit-tab.md`.

## How it works for the user

- **Open:** with the cursor on a file, press **Ctrl+Shift+F4** (macOS **Cmd+Alt+4**). An editor tab named after the file opens right after the current tab, in the same panel, and takes focus. If that file already has an editor tab, that tab is activated instead.
- **Edit:** changes save about one second after you stop typing, and also when you leave the tab, close it, or quit the app. **Ctrl/Cmd+S** saves immediately. The tab chip shows a small dot only while a save is pending, and a warning icon if saving is blocked.
- **Escape:** saves, then switches back to the origin tab (cursor on the file when that listing contains it). The editor tab stays in the tab bar; switch back with Shift+J/K, a click, or Ctrl+Shift+F4 on the same file. The undo history is kept.
- **Preview:** **Ctrl+F3** (macOS **Cmd+3**) in the editor toggles quick preview in the other panel. It shows the note rendered, refreshes after every autosave (about half a second after you stop typing while the preview is open), and keeps its scroll position. Focus stays in the editor. **F3** opens the full viewer on the file.
- **Close:** **Ctrl/Cmd+W** in the editor saves and closes the tab.
- **Changed on disk:** if the buffer is fully saved, the editor reloads the new version silently. If you have changes that aren't saved yet, autosave pauses and a bar at the top of the editor says so; **Ctrl/Cmd+S** opens a dialog with **Overwrite**, **Reload**, and **Keep both**.
- **Notes:** a **Scratch** entry in the source bar (named after the active note store) opens the notes folder in the active panel. It's an ordinary folder, so every file operation works there. **New Note** (command palette) creates `YYYY-MM-DD-HHMMSS.md` there and opens it in an editor tab next to the current listing tab. Escape returns to that listing with its cursor unchanged. Closing the tab of a new note you never typed in deletes the empty file.

## Precedents to follow

- `spex/tabs.md` §3 and `src-tauri/src/ws/intents/tabs.rs:445` (`promote_terminal`) — building a sibling tab panel, inserting it after the source, and the patch order.
- `src/components/PanelTerminal.tsx` `TERM_CACHE` — module-level cache keyed by session id that survives tab switches.
- `src-tauri/src/ws/intents/terminal.rs` `quick_edit_target` — checks for local driver, regular file, listing tab, no shadow source.
- `src-tauri/src/terminal/mod.rs:58` — `file_fingerprint` / `fingerprint_changed`, the cheap first test before hashing.
- `src-tauri/src/watcher.rs` — per-panel directory watch (200 ms debounce) used to notice external changes.
- `src-tauri/src/ws/runtime.rs` persistence (`TabSession`) — terminal tabs are dropped today; editor tabs get their own variant.
- `panel.navigate_home` / `NavigateHome` — the shape of "navigate the active panel to a known folder".
- `src-tauri/src/bro/` providers — the shape of a trait with built-in implementations registered at startup.

## Corrections to earlier drafts

- `panel.sort.open` is **Shift+S**, not Ctrl+S; Ctrl/Cmd+S is unbound, so the editor can use it freely.
- `open_write_new` uses `create_new(true)` (`vfs/local.rs:296`) and refuses existing files; saves write a temp file and rename it.
- Size + modified time misses same-second, same-size edits; saves also compare a hash of the bytes last loaded or saved.
- A dedicated scratch source would give one path two source ids and lose features that check `source.kind == "filesystem"`; the notes folder stays on the local filesystem source.
- `TouchFile` (New File, Shift+F4) empties an existing file, and `TouchFileOk` has no `requestId` and clears `pendingTouch`; neither the editor nor notes use it.

## Backend — editor layer

- [ ] Read `spex/tabs.md`, `spex/sources.md`, `spex/vfs.md`, `spex/proto.md`, `spex/keyboard.md`, `spex/modal-stack.md`, `spex/modal-keyboard-audit.md` before touching those areas.
- [ ] **Tab kind:** add `TabKind::Editor` (`"editor"`) in `state/mod.rs`, and `Panel.editor: Option<EditorTab { session_id, file_path, origin_panel_id }>`. `source_id` / `path` hold the file's local source and parent folder. Editor panels never register a listing scan; they register a watcher on the parent folder to catch external changes.
- [ ] **Editor sessions:** `src-tauri/src/editor/` keeps an in-memory registry keyed by `session_id`: canonical path (symlinks resolved at open), hash of the last loaded or saved bytes, `FileFingerprint`, line ending (`lf`/`crlf`), BOM flag, trailing-newline flag, last saved revision.
- [ ] **Internal `open_editor_tab(origin_panel_id, source_id, local_path, created_note: Option<CreatedNote>)`**, shared by `OpenEditorTab` and the notes layer. It checks the file itself on disk — local driver, regular file, ≤ 2 MiB, UTF-8 — and never depends on the origin's listing. Refusals: `NOT_LOCAL`, `NOT_REGULAR_FILE`, `TOO_LARGE`, `NOT_UTF8`. If an editor tab for that canonical path already exists in any slot, activate it. Otherwise insert a new editor panel after the origin, activate it, and emit `panel-upsert` (create) + `slot` + `layout` + `focus` patches in that order.
- [ ] **Origin resolution:** the origin is always a listing tab. If the requesting panel is a listing tab, use it. If it is an editor tab, use that tab's own origin when it still exists. Otherwise use the nearest listing tab in the same slot (left first). If the slot has no listing tab, the editor tab has no origin and `ReturnToOriginTab` uses its fallback.
- [ ] **Intent `OpenEditorTab { panelId, path, filename }`** (scope: the listing panel). Look the filename up in that panel's listing as `quick_edit_target` does (path matches, entry exists, not `..`), then call `open_editor_tab`. Refusals reply `EditorOpenFailed { requestId, code, message }` and leave focus where it was.
- [ ] **Intent `LoadEditorSession { panelId }`** → `EditorSessionReady { requestId, panelId, sessionId, text, eol, bom }`. Called when an editor tab mounts without cached state (first open or after a restart). The text uses `\n`; the session keeps the original style.
- [ ] **Intent `SaveEditorFile { panelId, revision, text, force }`.** Saves for one session run one at a time on the blocking thread:
  1. Unless `force`, compare the current fingerprint, then the hash, with the session's. A mismatch replies `SaveEditorFailed { code: STALE }`; a missing file replies `MISSING`.
  2. Restore the session's line ending, BOM, and trailing-newline style.
  3. Default path: write `.<name>.fbro-tmp-<random>` in the same folder with `driver.open_write_new`, copy the target's permissions, flush and sync, then `driver.try_rename` over the target.
  4. In-place path (truncate and write via `open_write`) when renaming would change the file's identity: more than one hard link, or (Unix) a different owner or group than the process. This is the same rule as Vim's `backupcopy=auto`.
  5. Windows: retry the rename up to 3 times with a short backoff when another program holds the file open, then reply `IO`.
  6. On any failure remove the temp file. On success update the hash, fingerprint, and saved revision, and reply `SaveEditorOk { requestId, panelId, revision }`. The folder watcher refreshes listings.
- [ ] **External changes:** when the watcher reports a change in an editor tab's parent folder, compare the fingerprint and hash. If they changed, send `EditorFileChanged { panelId, sessionId, state: "modified" | "missing" }`. The frontend decides: reload silently when fully saved, otherwise pause autosave and show the conflict bar.
- [ ] **Intent `ReloadEditorSession { panelId }`** re-reads the file and replies `EditorSessionReady`. **Keep both** = save the local text as `<name> (conflict YYYY-MM-DD HHMMSS).<ext>` next to the file (via `open_write_new`), then reload.
- [ ] **Intent `ReturnToOriginTab { panelId }`:** activate `origin_panel_id` if it is still in the slot. The origin keeps its cursor; if its listing contains the edited file, the cursor moves to that entry, otherwise it stays where it was. If the origin is gone (or there never was one), open a new listing tab of the file's folder after the editor tab with the cursor on the file. Emit `slot` + `layout` + `focus`.
- [ ] **Closing:** `panel.tab.close` on an editor tab drops its session. On the slot's only tab it turns the panel into a listing of the file's folder instead of being a no-op.
- [ ] **Empty new notes are discarded on close:** an editor tab opened by `CreateNote` carries `created_note: Some(CreatedNote { store_id, note_id })`. When that tab closes (Ctrl/Cmd+W, middle-click, ×), after the final flush, if no save ever succeeded in this session and the file on disk is still 0 bytes with an unchanged fingerprint, the editor layer calls `notes::discard_empty_note(created_note)`, which calls the store. Escape never discards, because the tab stays open. App quit doesn't discard; the restored tab keeps the marker.
- [ ] **Quick preview follows editor tabs:** `ResolveViewerFile` (`ws/intents/viewer.rs`) resolves an editor-tab source panel to its file instead of a cursor entry. `Panel.editor` carries `saved_version: u64`, bumped on every successful save, external reload, and Keep both, and sent in the editor panel's patch so the preview knows to refresh. No new viewer intent.
- [ ] **Persistence:** add `TabSession::Editor { source_id, rel_path, origin_index, created_note: Option<{ store_id, note_id }> }` in `persistence.rs` and `runtime.rs`. On restore, create the editor panel without reading the file; the first `LoadEditorSession` reads it. A missing or unreadable file restores as a listing of its folder plus a toast. `persist_panel_tabs = false` keeps whichever tab is active, editor included.
- [ ] **Not an `AppJob`:** saves have the same shape as the `touch_file` handler (`ws/intents/jobs.rs:2564`) — `spawn_blocking` around driver calls, a journal entry, a reply — but are separate code in `ws/intents/editor.rs`, and every reply carries a `requestId`. A save is a single small write with nothing to report, cancel, pause, or skip, and an `AppJob` would put a job-bar entry and notification on screen every second. Saves for one session are serialized by the editor module.

## Backend — notes layer

- [ ] **`NoteStore` trait** in `src-tauri/src/notes/mod.rs`. Synchronous, called through `spawn_blocking`, typed `NoteError`:
  ```rust
  pub(crate) trait NoteStore: Send + Sync {
      /// Stable id, also the pinned-location id ("scratch").
      fn id(&self) -> &str;
      /// Label for the source bar ("Scratch").
      fn display_name(&self) -> &str;
      /// Local folder a panel opens to browse the notes; created if missing.
      fn browse_location(&self) -> Result<PathBuf, NoteError>;
      /// Create an empty note and return its id and the local file to edit.
      fn create_note(&self, request: &NewNoteRequest) -> Result<NoteRef, NoteError>;
      /// Remove a note this store created that was never written to.
      /// The caller has already checked it is empty and unchanged.
      fn discard_empty_note(&self, note_id: &str) -> Result<(), NoteError>;
  }
  pub(crate) struct NewNoteRequest { pub title: Option<String> }
  pub(crate) struct NoteRef { pub note_id: String, pub local_path: PathBuf }
  ```
  `NoteError` maps to `NOTES_UNAVAILABLE` (store folder can't be created or reached) and `IO`.
- [ ] **`FolderNoteStore`** in `notes/folder.rs`: root `config_dir()/scratch`, id `"scratch"`, name `"Scratch"`. `create_note` makes `YYYY-MM-DD-HHMMSS.md` (or a sanitized `title.md`) with exclusive create, retrying with `-2`, `-3`, … on `AlreadyExists`. `note_id` is the path relative to the root. `discard_empty_note` re-checks the file is 0 bytes and removes it; it refuses ids that resolve outside the root. No other module calls `config_dir()/scratch` directly.
- [ ] **Registration:** `AppState.note_store: Arc<dyn NoteStore>`, set at startup to `FolderNoteStore`. No setting in v1; choosing a store becomes a setting when a second implementation exists.
- [ ] **Intent `CreateNote { panelId, title? }`** in `ws/intents/notes.rs` → `NoteCreated { requestId, panelId, storeId, noteId, editorPanelId }` or `NoteCreateFailed { requestId, code, message }`. It calls `store.create_note`, resolves the local source containing `local_path`, then calls the editor layer's `open_editor_tab` with `created_note` set and the origin resolved as above. Patches go out first, then `NoteCreated`. If the editor refuses the file, it discards the new empty note and replies `NoteCreateFailed` with the editor's code. Journal kind `"note-create"` with `storeId` and `noteId`; a discard is journaled as `"note-discard"`.
- [ ] **Intent `OpenNotes { panelId }`** → `NavigateOk`, or `OpenNotesFailed { requestId, code, message }`. It calls `store.browse_location` and navigates the panel there on the local source that contains it, like `NavigateHome`.
- [ ] **Source bar:** add `pinnedLocations` to the snapshot next to `sources`, built from the store: `[{ id: store.id(), name: store.display_name(), iconName: "notes", action: "OpenNotes" }]`. It is not a `Source`; activating it sends `OpenNotes`. Document in `spex/sources.md` that the source bar can also show pinned locations, and that they hand off to the real source.
- [ ] **Separate from New File:** note and editor code never sends `TouchFile`, never uses `TouchFileOk` / `pendingTouch` / `TouchModal`, and doesn't call `touch_file`. Frontend state lives under `editorTabs` / `pendingNote` in the store.

## Dispatch and protocol

- [ ] Arm the editor intents (`OpenEditorTab`, `LoadEditorSession`, `SaveEditorFile`, `ReloadEditorSession`, `ReturnToOriginTab`) and note intents (`CreateNote`, `OpenNotes`) in `ws/intents/mod.rs::dispatch`, routed to `editor.rs` and `notes.rs`; parse with `arg_str` / `arg_bool` / `arg_u64`.
- [ ] Add `OpenEditorTab` and `CreateNote` to `should_record_command` (`ReturnToOriginTab` and `OpenNotes` are navigation, not mutations). Do **not** journal every autosave: record one `"edit"` journal entry per editor session at its first successful save (mirroring `record_quick_edit_intent` / `record_quick_edit_result`).
- [ ] Mirror `EditorOpenFailed`, `EditorSessionReady`, `SaveEditorOk`, `SaveEditorFailed`, `EditorFileChanged`, `NoteCreated`, `NoteCreateFailed`, `OpenNotesFailed`, the `editor` panel field, `tabKind: "editor"`, and `pinnedLocations` by hand in `src/ws/types.ts`; add the messages to the `BEMessage` union and handle them in `src/store/index.ts`. Editor text never enters the store.
- [ ] Document both intent groups, replies, error codes (`NOT_LOCAL`, `NOT_REGULAR_FILE`, `TOO_LARGE`, `NOT_UTF8`, `STALE`, `MISSING`, `IO`, `NOTES_UNAVAILABLE`), and patch order in `spex/proto.md` and `spex/tabs.md` §4. Note payloads use `storeId` / `noteId`, never a store-specific field.

## Frontend

- [ ] Add CodeMirror 6 (`@codemirror/view`, `@codemirror/state`, `@codemirror/commands`, `@codemirror/language`, `@codemirror/lang-markdown`, `@codemirror/search`) and lazy-load the editor chunk.
- [ ] `PanelEditor` component rendered by `Panel.tsx` when `tabKind === "editor"`, filling the panel like a solo terminal tab.
- [ ] `EDITOR_CACHE` at module level keyed by `sessionId` holds the CodeMirror `EditorState` (text, undo history, selection, scroll). Unmounting on tab switch stores it; remounting restores it; tab close or session drop deletes it.
- [ ] **Autosave controller** per session:
  - Every change bumps a revision. Save 1 s after the last change (400 ms while quick preview is showing this editor's file); at most one save in flight; changes made during a save trigger another save after it.
  - Flush immediately on Ctrl/Cmd+S, Escape, tab switch away, tab close, and window blur.
  - Mark the buffer clean only when `SaveEditorOk.revision` equals the current revision.
  - On `STALE`/`MISSING` or an `EditorFileChanged` with unsaved changes, pause autosave and show the conflict bar. On `IO`, show the error in the bar and retry on the next change.
- [ ] **Conflict bar** (top of the editor, non-modal, does not take focus): "Changed on disk — autosave paused. Press Ctrl/Cmd+S to resolve." Its buttons and Ctrl/Cmd+S open a `ModalLayer` dialog with **Overwrite** (resend with `force: true`), **Reload**, **Keep both**, then Cancel. Use `initialFocusRef`. Focus returns to the editor afterwards.
- [ ] **Tab chip** in `PanelTabBar.tsx`: editor icon from `@tabler/icons-react` (`aria-hidden`), file name as the label, path in the tooltip. Pending-save dot and warning icon render after variable-width badges.
- [ ] **App quit:** in the main window's close-requested handler, flush every editor session (wait up to ~2 s). If any save fails, show a `ModalLayer` dialog: "N files couldn't be saved — Close anyway / Stay".
- [ ] **Theme:** style CodeMirror from `var(--…)` tokens. Any new tokens get `fbro-dark` and `daylight` values and entries in `src/theme/tokens.ts`. Custom-styled controls get `appearance: none; font: inherit`.
- [ ] **Source bar:** render `pinnedLocations` after the sources with the same keyboard behaviour as source entries, using the backend-provided name and icon; activating one sends its `action`. Add `data-testid` to the entry.
- [ ] **Preview via quick preview:** when the quick preview's source slot is showing an editor tab, `EmbeddedViewerPanel` resolves through `ResolveViewerFile` as usual and adds the editor's `saved_version` to its refresh key (`EmbeddedViewerPanel.tsx:71`), so it reloads after each save. When a reload resolves to the same file, keep the preview's scroll position (as a fraction of content height) instead of jumping to the top; Mermaid and image state may re-render. Focus stays in the editor. Switching back to the origin listing makes the preview follow the listing cursor again, as today. No live (unsaved-text) rendering and no second markdown renderer.

## Keyboard and commands

- [ ] Add an "Editor tabs" section to `spex/keyboard.md` and `spex/tabs.md` §6. While the editor has focus it receives all keys except:
  - **Escape** — after CodeMirror's own layers (find panel, completion): flush and `ReturnToOriginTab`.
  - **Ctrl/Cmd+S** — save now, or open the conflict dialog when autosave is paused.
  - **Ctrl/Cmd+W** — flush and close the tab.
  - **F3** — flush and open the viewer on the file.
  - **Ctrl+F3** (macOS **Cmd+3**, the `panel.viewer.embed` binding) — flush and toggle quick preview in the other panel; focus stays in the editor.
  - **Tab** indents (CodeMirror `indentWithTab`); Escape is the documented way out of the editor, which is CodeMirror's own accessibility guidance for Tab-capturing editors.
  - Panel-scoped bindings such as Shift+J/K and letter commands type text inside the editor.
- [ ] Make the capture-phase key listeners (`ModalStack.tsx:181`, `useInputHistory.ts:98`, `InputHistoryDropdown.tsx:64`, `ContextMenu.tsx:315`, `useTabReorder.ts:109`) ignore events from inside the editor unless their own layer is open. While a modal is open over the editor, the editor is inert.
- [ ] Commands in `src-tauri/src/ws/commands.rs`:
  - `file.edit_builtin` "Edit in Editor Tab": **Ctrl+Shift+F4** on Linux/Windows, **Cmd+Alt+4** on macOS (Cmd+Shift+4 is the macOS screenshot shortcut and never reaches the app). Shift+F4 is `file.touch` and Ctrl+F4 is `file.quick_edit`; neither changes.
  - `notes.new` "New Note" → `CreateNote`: no default binding.
  - `notes.open` "Open Notes" → `OpenNotes`: no default binding (the source-bar entry is the main way in).
- [ ] Wire them in `src/commands/interactive.ts`, and add `file.edit_builtin` to `src/contextualOperations.ts` / `src/contextMenu.ts` for regular files.
- [ ] Show bindings with `formatPrimaryBinding`; set `aria-keyshortcuts` on the conflict-bar button and the dialog buttons.
- [ ] PTY Quick Edit (`file.quick_edit`) and external Edit (`file.edit`) are unchanged.

## Tests and docs

- [ ] **Rust unit tests** (temp directory cleaned by a `Drop` guard):
  - open refusals: `TOO_LARGE`, `NOT_UTF8`, `NOT_REGULAR_FILE`, `NOT_LOCAL`
  - save via temp + rename, permissions kept; in-place save for a hard-linked file; save through a symlink writes the target and keeps the link
  - no temp file left after a failed rename
  - `STALE` on an external same-size, same-second edit (hash check); `force` overwrite; `MISSING` after delete; Keep both writes the conflict copy
  - CRLF, BOM, and trailing-newline round-trip
  - one editor tab per canonical path; `ReturnToOriginTab` with the origin present and closed; closing the only tab turns it into a listing
  - persistence: editor tab saved and restored; missing file restores as a listing
  - `ResolveViewerFile` on an editor-tab panel resolves to its file; `saved_version` increases on save, external reload, and Keep both
  - `open_editor_tab` opens a file outside the origin's listing; origin resolution from a listing, an editor tab, and a terminal tab; `ReturnToOriginTab` leaves the cursor alone when the origin doesn't list the file
  - empty-note discard: removed on close when never saved and still empty; kept when typed in, when another program wrote to it, after Escape, and for tabs not created by `CreateNote`
  - `FolderNoteStore`: lazy folder creation, timestamp names, `-2` suffix on collision, never overwrites; `discard_empty_note` refuses non-empty files and ids outside the root
  - note intents with a **fake `NoteStore`** pointing at a temp folder: `CreateNote` and `OpenNotes` use only the trait (no scratch-path knowledge), and a store error becomes `NOTES_UNAVAILABLE`
- [ ] **Smoke case** in `tests/e2e/smoke/cases/`:
  - Ctrl+Shift+F4 opens an editor tab after the origin; typing autosaves (content on disk after the debounce)
  - Escape returns to the origin tab with the cursor on the file; the editor tab is still present and its undo history survives switching back
  - Shift+S and letter keys type text inside the editor instead of running panel commands
  - external edit while clean → silent reload; external edit with unsaved changes → conflict bar → Ctrl+S → Reload
  - Ctrl+W closes the tab and returns focus to a listing; covered panels ignore keys while the dialog is open
  - Scratch source-bar entry opens the notes folder; New Note opens an editor tab after the current listing tab, Escape returns to it with the cursor unchanged; New Note then Ctrl/Cmd+W without typing leaves no file behind
  - Ctrl+F3 in the editor opens quick preview of the note in the other panel with focus still in the editor; typing updates the preview after the autosave; a long note keeps its preview scroll position across updates
  - New Note never sends `TouchFile` or produces `TouchFileOk`; Shift+F4 New File behaves exactly as before
- [ ] Update `spex/tabs.md` (tab kind, lifecycle, persistence, keybindings), `spex/keyboard.md` (editor tabs, and §3 quick preview following an editor tab), `spex/spec.md` "Shared viewer content", `spex/proto.md`, `spex/sources.md` (pinned locations), `TESTING.md` coverage list, and add the entry to `spex/plans/feature-ideas.md`. Add a short "Notes" section to `spex/spec.md` describing the `NoteStore` boundary and what a new store must provide.
- [ ] Run `npm run build` and `cargo test --manifest-path src-tauri/Cargo.toml`; run the Linux smoke suite and state explicitly that macOS and Windows were not verified.
- [ ] Report results for user validation and leave the change uncommitted on `editor-tab-notes`.

## Decisions

- **Editor tab in the same slot:** Escape returning to the origin tab and the editor staying open both require a real tab. Opening in the same slot keeps the other panel free as a copy target. One editor tab per file avoids two buffers fighting over one file.
- **Autosave (1 s debounce plus flushes):** no unsaved-changes prompts, editor tabs can be restored after restart, and Escape can always leave without asking. Ctrl/Cmd+S stays as "save now" for habit and as the way into conflict resolution.
- **Conflicts never overwrite silently:** a clean buffer follows the disk; a buffer with unsaved changes pauses autosave until you choose Overwrite, Reload, or Keep both.
- **Atomic save with identity-preserving fallback:** temp file + rename protects against half-written files; in-place writes for hard-linked or other-owner files, as Vim does, avoid breaking links or ownership. Symlinks are resolved at open so the link itself survives.
- **Notes behind a `NoteStore` trait, with their own intents:** the protocol speaks in `storeId` / `noteId`, and only `FolderNoteStore` knows about the scratch folder, so the backing mechanism can change without touching the protocol, the frontend, or the editor. One implementation in v1; the trait stays small (browse location, create note) until a second store needs more.
- **Future Obsidian store (not in v1):** an Obsidian vault is a folder of Markdown files plus `.obsidian/` config, so an `ObsidianVaultStore` fits the same trait. Its `browse_location` is the vault folder (or its configured new-note folder), and `create_note` follows the vault's new-note location, naming, and templates. The built-in editor edits the returned file as usual. Opening a note in the Obsidian app itself (`obsidian://open?vault=…&file=…`) would be an extra note action, not a change to the editor. Today's scratch notes are already plain `.md` files, so a user can open the scratch folder as a vault right away.
- **Assumption:** every store returns a local file to edit. A store that isn't file-backed (a database or remote API) would need note-level load/save intents; that is out of scope until such a store is wanted.
- **No scratch source:** the folder lives on the local filesystem source, so git, sharing, global selections, search, hashing, and PTY Quick Edit all work unchanged. The source bar reaches it through a pinned location that hands off to that source.
- **No notes setting in v1:** a configurable path or store choice needs a rule for existing notes; it comes with the second store.
- **Any small UTF-8 local file:** the backend checks size, encoding, driver, and file type, so limiting the editor to notes would be policy rather than safety.
- **macOS binding:** Cmd+Alt+4, because macOS takes Cmd+Shift+4 for screenshots before the app sees it.
- **Editor library:** CodeMirror 6 over Monaco (needs web workers, which are fragile in webkitgtk, WKWebView, and WebView2; much larger bundle), Ace (heavier, weaker accessibility), ProseMirror/Lexical (rich text, not markdown source), and a plain `textarea` (no highlighting, search, or undo history worth keeping).
- **Preview through quick preview, fed by autosave:** because the file on disk trails the editor by about half a second, the existing quick preview gives a near-live rendered view with the existing renderer, image policy, and Mermaid support, at the cost of a small backend resolution change and a refresh key. A true as-you-type preview needs `FileViewer`'s markdown renderer to accept unsaved text and a rule for resolving its image links; that stays out of v1.

## Implementation notes (2026-11-02)

Implemented on `editor-tab-notes`. Where the code differs from the checklist above:

- **No fingerprint, hash only.** Change detection compares a blake3 hash of the file (≤ 2 MiB, cheap) with the bytes last loaded or saved. Size + mtime cannot decide either way: a same-second, same-size edit keeps them and a bare `touch` changes them. `DiskState` carries no fingerprint.
- **No trailing-newline flag.** The final newline is exactly what the buffer contains; only the line ending (LF/CRLF) and the UTF-8 BOM are restored on save.
- **Editor sessions live on the panel** (`Panel.editor: EditorTab`), not in a separate registry; a per-tab `io_lock` serializes loads, saves, and external-change checks.
- **Editor panels are scanned like a hidden listing of their folder**, which gives them the existing watcher, reconnect rescans, and recovery. New tabs opened by `spawn_panel_scan` register their folder watch explicitly, because `run_scan` itself does not (`panel.tab.new` has the same gap; left unchanged).
- **Capture-phase listeners needed no change:** each already acts only when its own layer is open or its input is focused.
- **F3 / Ctrl+F3 from the editor** go through the registry bindings, but only bindings with a modifier or an F-key pass through. On macOS `file.view` is plain `3`, which types, so the separate viewer is reached from the editor only through the preview's Open viewer button there.
- **Keep both** is its own intent, `KeepBothEditorVersions`; the conflict dialog uses mnemonics O / R / K (S for a deleted file).
- **Quit flush:** `EditorFlushRequested` → `Quit { editorsFlushed: true }`; a second quit request within 3 s skips the flush so a stuck frontend can't block quitting.
- **Note-intent tests:** there is no intent-level harness (`IntentContext` needs a Tauri `AppHandle`), so store independence is checked by a test asserting the note intents contain nothing store-specific, plus the smoke flow.
- **CodeMirror chunk** is ~188 kB gzipped and lazy-loaded; `@codemirror/lang-markdown` pulls in the HTML/CSS/JS grammars for embedded HTML.
- **Key change (2026-11-02):** Ctrl+Shift+F4 / Cmd+Alt+4 is **New Note** (`notes.new`), as originally intended for notes; it also works inside an editor tab. "Edit in Editor Tab" (`file.edit_builtin`) has no default key: context menu and command palette. New File stays on Shift+F4. Earlier sections of this plan that put the key on `file.edit_builtin` are superseded.
- **Preview refresh (2026-11-02):** one refresh per save, done in place via `FileViewer.refresh()` instead of remounting (which had caused several full repaints per keystroke: the saved-version bump plus the hidden listing's rescans, each remounting the viewer).
- **Cursor-following preview (2026-11-02):** comrak `render.sourcepos` tags rendered elements with source lines; the editor publishes its cursor line and viewport position, and quick preview scrolls the matching block to the same height (smoke-tested with Ctrl+End / Ctrl+Home on a 40-section note).
- **Configurable notes folder (2026-11-02):** `notesFolder` general setting (empty = default `config_dir()/scratch`, absolute or `~/…`), Settings → General → Notes. Changing it swaps the store without moving notes; `discard_empty_note` now checks the expected path. Supersedes the "No notes setting in v1" decision.
- **Full screen (2026-11-02):** Ctrl/Cmd+Shift+M or the header button, mirroring terminal maximize; Escape restores first; z-index 90 keeps dialogs above it.
- **F4 on notes (2026-11-02):** `EditFile` carries `panelId`; files inside the notes folder (`NoteStore::contains`) open in an editor tab, everything else (and refused files) in the external editor.
