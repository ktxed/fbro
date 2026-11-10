# Plan: tab navigation and command palette from the editor tab

Status: implemented, awaiting validation · 2026-11-11 (D1 resolved as (a))

## Goal

Let the user switch panel tabs, and open the command palette, while focus is in
a built-in editor tab (CodeMirror). Today the editor swallows every key except
a short pass-through list (`spex/keyboard.md` §3a), and the tab keys are
Shift+J / Shift+K, which have to type text in an editor. So the only way out
of an editor tab is Escape (back to the origin tab) or the mouse.

## Current state (verified in `src-tauri/src/ws/commands.rs`)

| Command | Binding today |
|---|---|
| `panel.tab.next` | Shift+J |
| `panel.tab.prev` | Shift+K |
| `panel.source.next` | **Ctrl+Tab** |
| `panel.source.previous` | **Shift+Tab**, Ctrl+Shift+Tab |
| `app.command_palette` | Ctrl+Shift+P (macOS Cmd+Shift+P) |

Note: the request listed Shift+Tab as the *current* Next Source binding. In the
registry, Shift+Tab is a Previous Source binding. `spex/keyboard.md` §4 ("Shift+Tab
retains the Next Source binding") disagrees with the registry and is stale.
See decision D1.

## Target bindings

| Command | New binding (all platforms) | From the editor tab |
|---|---|---|
| `panel.tab.next` | Ctrl+Tab (Shift+J dropped) | yes: released by the editor |
| `panel.tab.prev` | Ctrl+Shift+Tab (Shift+K dropped) | yes: released by the editor |
| `panel.source.next` | Shift+Tab | no (Shift+Tab un-indents in CodeMirror) |
| `panel.source.previous` | none (palette and source bar only) | no |
| `app.command_palette` | unchanged: Ctrl+Shift+P / Cmd+Shift+P | yes: released by the editor |

Ctrl+Tab is a literal Ctrl on macOS too, not `mod_binding`: Cmd+Tab is the OS
app switcher, and Ctrl+Tab is the conventional macOS "next tab" key as well
(Safari, Terminal, VS Code).

Previous Source loses its key on purpose: with only a few sources, cycling
forward is enough. Multi-stroke chords (for example Ctrl+S → Right/Left) are a
later, separate change. `KeyBindingSchema.strokes` is already a `Vec`, but
`bindingMatches` and the pass-through helpers only look at `strokes[0]`, so
chords need a matcher with pending-prefix state and are out of scope here.

### Decision D1: what Shift+Tab does

The request says "keep Shift+Tab" on Next Source. Since Shift+Tab is
Previous Source today, there are two readings:

- **(a) Recommended: Shift+Tab becomes Next Source.** This matches the
  intent of keeping a forward-cycling key, and it matches the stale spec
  sentence that users may already have read. Cost: Shift+Tab usually means
  "backwards", and Shift+Tab now goes the other way.
- **(b) Shift+Tab stays Previous Source, and Next Source loses its key.**
  This keeps direction semantics, but there would be no forward source key.

The rest of this plan assumes (a).

## Why these keys are safe in CodeMirror

`src/editor/codemirror.ts` installs `searchKeymap` (with F3 removed),
`defaultKeymap`, `historyKeymap`, `indentWithTab`, and the app's own
`editorKeys`. None of them bind Ctrl-Tab, Ctrl-Shift-Tab, or Mod-Shift-p.
`indentWithTab` binds only Tab and Shift-Tab, so Shift+Tab correctly stays
with the editor. Keys CodeMirror doesn't handle reach `PanelEditor`'s root
`onKeyDown`, where the existing `passThrough()` helper accepts them: each
binding carries Ctrl, so it counts as "special".

Ctrl+Tab already reaches the page on WebKitGTK and WebView2, because it is
Next Source today. That still needs a check on macOS WKWebView.

## Changes

### 1. Registry: `src-tauri/src/ws/commands.rs`

- `panel.tab.next`: `keybinding_with_modifiers("Tab", false, true, false, false)`.
- `panel.tab.prev`: `keybinding_with_modifiers("Tab", true, true, false, false)`.
- `panel.source.next`: `keybinding_with_modifiers("Tab", true, false, false, false)`.
- `panel.source.previous`: `vec![]`. Add a comment pointing to this plan or
  future chords.
- Add a comment near the tab commands: the bindings carry Ctrl so that
  `PanelEditor` can release them, and Shift+J/K would type text.
- Tests (inline `mod tests`):
  - `tab_cycle_bindings_are_ctrl_tab`: for all three platforms, next = Ctrl+Tab
    and prev = Ctrl+Shift+Tab with `meta == false`, and no other command shares
    either stroke.
  - `source_next_is_shift_tab_and_previous_is_unbound`.
  - Search for any test that asserts the old source or tab bindings, and update it.
- Regenerate the site snapshot:
  `FBRO_UPDATE_SITE_DATA=1 cargo test --manifest-path src-tauri/Cargo.toml site_command_data_is_current`.

User keybinding overrides: check whether settings persist per-command binding
overrides (`spex/settings.md`). If so, a user who rebound these ids keeps their
override, and nothing needs migrating. Confirm this, and note the result in the
commit body.

### 2. Editor pass-through: `src/components/PanelEditor.tsx`

In `onKeyDown`, add branches after the existing ones, each one calling
`event.preventDefault()`:

- `panel.tab.next` / `panel.tab.prev`: run them through `executeCommand` (or the
  same `panel.tab.switch` intent that `interactive.ts` builds), with this
  editor's `panelId` as the focused panel. No explicit flush is needed:
  switching away from the tab already autosaves (§3a). Check this against the
  autosave triggers in `src/editor/sessions.ts`, and flush first if the
  switch-away path isn't covered for keyboard switches.
- `app.command_palette`: `dispatch({ type: "OPEN_COMMAND_PALETTE" })`. The
  modal stack captures the CodeMirror content element as the opener and returns
  focus to it on close. That is the existing `ModalStack` behaviour; do not add
  a focus callback.

Prefer a small table (`[commandId, action]`) over a longer `else if` chain once
there are six entries. The `passThrough()` rule ("modifier or F-key only")
stays unchanged.

Focus after a switch: going to a listing tab must focus the listing, and going
to another editor tab must focus its CodeMirror once ready. Both are existing
tab-activation paths (`Panel` activation and §3a "Focus"). Verify them; don't
add new focus code.

### 3. Other surfaces that see Tab

| Site | Effect of the new keys | Action |
|---|---|---|
| `ModalStack.tsx:165` focus trap | Ctrl+Tab inside a modal advances focus | none; modals own keys |
| `PanelTabList.tsx:280` | Ctrl+Tab closes the list (`key === "Tab"`) | acceptable; optional: ignore when `ctrlKey` |
| `EmbeddedViewerPanel.tsx:166` | Ctrl+Tab treated as Tab traversal | none in this change |
| `Panel.tsx` input handler (~1856) | Ctrl+Tab in the filter/path input now matches a panel command and bubbles | decide whether `panel.tab.*` should `focusPanel()` first, like `panel.source.*`; recommended: no, since the tab switch moves focus anyway |
| Empty-folder suggestions (§4) | Shift+Tab = Next Source, as the spec already says | spec now correct |
| `PanelTerminal.tsx` promoted terminal tab | still traps Ctrl+Tab, and the shell gets it | out of scope; follow-up: release `panel.tab.*` via `releasedChord` the same way |

### 4. Specs (same commit)

- `spex/keyboard.md` §3a: add rows for `panel.tab.next`/`prev` (Ctrl+Tab /
  Ctrl+Shift+Tab: switch tab, autosave on leave) and `app.command_palette`.
  Change "Shift+S, Shift+J/K, and letter commands type text" to drop J/K.
- `spex/keyboard.md` §3 (end of the Git/diff paragraph): "Shift+J/Shift+K
  select the next/previous panel tab" → Ctrl+Tab / Ctrl+Shift+Tab.
- `spex/keyboard.md` §4: the Shift+Tab sentence becomes accurate. Re-read it.
- `spex/tabs.md` §command table (lines ~372–373): new bindings.
- `spex/sources.md`, if it names source keys: Next Source = Shift+Tab, Previous
  Source unbound.

### 5. User guide (`site/`)

Shortcuts render from `commands.json`, so most pages update on their own after
the regeneration. Text changes:

- `using/navigation.mdx:42` and `using/sources.mdx:10` pair next/previous
  source. `<Shortcut id="panel.source.previous" />` would render "no default".
  Reword to mention only the next-source key, and say that Previous Source is
  in the command palette.
- `using/tabs.mdx`: add one sentence saying tab switching works from inside an
  editor tab.
- The editor-tab guide page, if there is one: list the released keys (tab
  switching, palette).
- The cheat sheet (`CheatSheetModal.tsx:49`) row "Next / previous source"
  shows only one key now. Check that it renders cleanly with an unbound command
  (`bindings.length > 0` branch at ~180).

### 6. Smoke tests

Extend `tests/e2e/smoke/cases/editor-tab.mjs`:

1. With the editor focused, type text, then press Ctrl+Tab. The next tab becomes
   active and the text is saved on disk.
2. Ctrl+Shift+Tab returns to the editor tab, and CodeMirror has focus.
3. Ctrl+Shift+P opens the palette. Escape closes it, and focus is back in
   CodeMirror (the opener). The editor text is unchanged, with no stray "P".
4. Shift+J in the editor types "J" (regression guard for the dropped binding).

Add Ctrl+Tab / Ctrl+Shift+Tab tab cycling and Shift+Tab next-source to
`keyboard-shortcuts.mjs` if it covers panel keys. Update the coverage list in
`TESTING.md`.

## Verification

- `cargo test --manifest-path src-tauri/Cargo.toml commands`
- `cargo test --manifest-path src-tauri/Cargo.toml site_command_data_is_current`
- `npm run build`
- Smoke: `editor-tab`, `keyboard-shortcuts` (host run, see `TESTING.md`)
- Manual on macOS (WKWebView): Ctrl+Tab reaches the page and isn't consumed by
  the window's tab-bar handling. On Windows (WebView2): Ctrl+Tab /
  Ctrl+Shift+Tab aren't treated as browser accelerators. Neither is covered by
  the Linux fixtures, so report them as unverified until checked.

## Implementation notes

- WebKitGTK reports Ctrl+Shift+Tab as `key: "Unidentified"`, `code: "Tab"`
  (GDK's ISO_Left_Tab), so the old Ctrl+Shift+Tab → Previous Source binding
  never fired on Linux. `canonicalEventKey` (`src/commands/bindings.ts`) now
  maps the `Tab` code to `Tab`, in both dispatch and shortcut recording.
- The editor runs released commands through `executeCommand` via an
  `onRunCommand` prop from `Panel`, so tab cycling stays in `interactive.ts`.
- The cheat sheet row became "Next source" (one key).
- User binding overrides live per command in `command_bindings`; they are
  untouched, so nothing needs migrating.

## Out of scope / follow-ups

- Multi-stroke chords (prefix state machine in `bindings.ts`, cheat sheet and
  `Keys` rendering for multiple strokes), then restore a Previous Source key.
- Releasing `panel.tab.*` from promoted terminal tabs.
- `panel.tab.list` from the editor: still blocked by CodeMirror's
  Ctrl/Cmd+Shift+L (§3a).
- An optional macOS alias Cmd+Shift+] / Cmd+Shift+[ for tab cycling.
