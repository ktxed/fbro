# Custom color schemes — plan

## Context

fbro ships two hard-coded schemes (`fbro-dark`, `daylight`). Their tokens live in
`src/styles.css` (`:root` and `[data-color-scheme="daylight"]`, ~60 tokens), the
metadata (terminal colours, mermaid theme, mode) in `src/theme/colorSchemes.ts`,
and the backend only accepts those two ids (`is_known_color_scheme`,
`resolve_active_color_scheme` in `src-tauri/src/state/settings.rs`). Users can't
make their own.

Goal: users can create custom schemes **from the current scheme as a template**
and edit them in a visual editor that shows each colour and where it is used.
The editor is a **Color Scheme** tab in Settings that appears **only when opened
from the Display tab**. Schemes are TOML files in `<config_dir>/themes/` (same
style as `bookmarks.toml` / `session.toml`), managed by the backend; dropping a
file into that folder imports it.

Branch: `feat/custom-color-schemes` (from `development`). Status: implemented, awaiting validation.

### Deviations from the original plan

- The catalog is projected as `general.colorSchemes` (not `settings.colorSchemes`)
  because the frontend merges settings patches one level deep under `general`.
- The color editor is a fixed pane under the token list (swatch picker + text
  field for the selected token) instead of an inline expansion under each row.
  This keeps the list a plain one-tab-stop listbox with no nested controls.
- Preview elements select their token on click (cycling through the tokens an
  element shows); hover only shows the token names as a tooltip.
- `--bg-tab-focused-inactive` is stored as a concrete color (`#13161f`) instead of
  the former `color-mix(...)`, so Duplicate can read it back; it differs from the
  old result by at most 1/255 per channel.
- `PanelTerminal` resolves the active scheme itself (`useColorScheme`) instead of
  taking a `colorScheme` prop.

## Theme file format

`<config_dir>/themes/<slug>.toml` (`crate::config::config_dir()`), id `custom:<slug>`:

```toml
version = 1
name = "Midnight"
description = "Based on fbro Dark"
mode = "dark"          # dark | light → native controls, mermaid, window bg
base = "fbro-dark"     # built-in that supplies any token missing below

[colors]               # keys = CSS token names without "--"
bg = "#0d0d0d"
bg-panel = "#111111"
bg-selected = "rgba(220, 170, 20, 0.30)"
# …

[terminal]
background = "#1e1e1e"
foreground = "#dddddd"
cursor = "#d6e8ff"
selection_background = "#2d228a"
```

- New schemes are written **complete**: every registry token copied from the template.
  A hand-written file can leave tokens out; those come from `base`.
- Validation is done by the backend: the colour keys must match `^[a-z][a-z0-9-]*$`. Values must be
  `#rgb[a]` / `#rrggbb[aa]` / `rgb()` / `rgba()`. Nothing else is allowed, so CSS
  can't be injected through the generated `<style>`. Unknown keys are kept when the file is saved.
- Files that fail to parse are listed with their error. You can't select them,
  and fbro never rewrites them.

## Backend

**New `src-tauri/src/themes.rs`** (follows the `bookmarks.rs` pattern: serde + `toml`, atomic tmp+rename):
- `themes_dir()`, `load_all() -> ThemeCatalog { items, errors }`, `save(&Theme)`,
  `delete(slug)`, `unique_slug(name)`, `validate_color(&str)`.
- Watcher on `themes/` with the existing `notify = "7"` dependency (debounced). On
  change it reloads the catalog and broadcasts it. That is how an import works: copy
  a `.toml` into the folder.
- Unit tests in the same file: parse/round-trip, injection rejected
  (`red;}` / `url(`), slug collisions, a partial file inheriting from base, a broken file reported.

**Settings integration** (`state/settings.rs`, `ws/intents/settings.rs`, `ws/appearance.rs`):
- `AppState` holds the `ThemeCatalog`. `is_known_color_scheme` also accepts valid
  `custom:*` ids. `resolve_active_color_scheme` returns `String`. A missing or invalid
  custom id falls back to its base, or to dark if that is unknown. The preference
  itself stays as set, so restoring the file restores the scheme.
- Follow-system still maps to the built-ins. Choosing a custom scheme for
  dark or light is a possible follow-up, not part of this plan.
- `window_background()` uses the custom scheme's `bg` (and its base if `bg` is missing).
- Snapshot and patches gain `settings.colorSchemes { dir, items, errors }`. It also
  goes into the viewer lightweight patch, so viewer and diff windows (which have no
  store) can render the active custom scheme.

**Intents** (`ws/intents/mod.rs::dispatch` → new `ws/intents/themes.rs`, mutating
ones go in `should_record_command`):
- `CreateColorScheme { requestId, name, base, mode, colors, terminal }` → `ColorSchemeSaved { requestId, id }`
- `UpdateColorScheme { requestId, id, name?, description?, mode?, colors, terminal }` → `ColorSchemeSaved`
- `DeleteColorScheme { requestId, id }`. If it was the manual scheme, `colorScheme` is reset to its base.
- Failures reply `ColorSchemeFailed { requestId, code, message }` (`BAD_COLOR_SCHEME`, `THEME_IO`).
- Writes go through the existing `settings_mutation` gate, like `apply_system_color_scheme`.
- The new `ServerMessage`s are mirrored by hand in `src/ws/types.ts` and `src/store/index.ts`.

## Frontend

**Token cascade groundwork** (`src/styles.css`):
- Turn the 3 daylight-only structural rules (`.panel` / `.panel__tab` backgrounds, lines
  ~320–326, ~582–592) into tokens (e.g. `--bg-panel-unfocused`, `--bg-tab`,
  `--bg-tab-inactive-focused`). Both schemes get values, and the current look stays pixel-identical.
- Change the daylight selector to
  `[data-color-scheme="daylight"], [data-color-scheme-base="daylight"]`. A custom
  scheme then renders `data-color-scheme="custom:x" data-color-scheme-base="<base>"`.
  Its missing tokens come from the base through the normal cascade, with no JS merge.
- The remaining translucent overlay/shadow literals stay as they are (out of scope; listed in the plan doc).

**Token registry** — new `src/theme/tokens.ts`: `{ name, label, group, description,
preview }` for every editable colour token, grouped as Surfaces · Panel rows & cursor ·
File types · Borders & accent · Status & feedback · Toasts · Terminal. The editor,
the template capture, and the preview hooks all read it. Non-colour tokens (font, sizes,
`--shadow-modal`) are excluded.

**Rendering custom schemes:**
- `src/theme/colorSchemes.ts`: `resolveColorScheme(id, catalog?)` returns built-ins plus
  customs (terminal and mode come from the file, mermaid from mode). Add a `useColorScheme()`
  hook. The existing callers (`App.tsx`, `BookmarksPopover`, `SourceContextMenu`,
  `GitBranchPicker`, `InputHistoryDropdown`, `PanelTerminal`, `Panel`, `FileViewer`,
  `DiffViewerApp`, `EmbeddedViewerPanel`) switch to it or pass the catalog, and each also
  sets `data-color-scheme-base`.
- New `CustomSchemeStyles` component (mounted in `App.tsx`, `FileViewer`, `DiffViewerApp`) that injects
  `<style data-fbro-custom-schemes>` with `[data-color-scheme="custom:x"] { --token: value; }`.
  It is appended after styles.css, so its rules win at equal specificity.
- The Display tab Scheme select lists the built-ins, then the custom schemes.

**Settings tab visibility** (`store/index.ts`, `SettingsModal.tsx`):
- `SettingsTab` gains `"colorScheme"`. `PendingSettings` gains `showColorSchemeEditor`.
- A **Customize…** button in Display → Color Scheme dispatches
  `OPEN_SETTINGS { tab: "colorScheme", showColorSchemeEditor: true }`.
  `TABS` is filtered, so the tab appears directly under Display only then. It stays
  while Settings is open and disappears on `CLOSE_SETTINGS`. `app.settings` and the palette never show it.

**Editor** — new `src/components/SettingsColorSchemeTab.tsx` and `ColorSchemePreview.tsx`:
- **Header:** scheme picker (built-ins and customs), name and description, mode, and the actions
  **Duplicate…** (primary for built-ins), **Delete**, **Revert**, **Open themes folder**
  (navigates the active panel to `colorSchemes.dir`), **Use this scheme**.
- **Template:** Duplicate reads the computed values of every registry token from the
  currently rendered scheme (`getComputedStyle`) plus its terminal colours. That snapshot
  is the template the user asked for, and it is sent as `CreateColorScheme`.
- **Built-ins are read-only.** Their inputs are disabled, with a hint pointing to Duplicate.
- **Token list (left):** grouped rows with a swatch, label, description ("context"), and a
  text input for hex/rgba. Enter or a click on the swatch expands an inline
  **react-colorful** `RgbaStringColorPicker` with alpha. It is small and widely used, and it handles
  keyboard input on its sliders. A native `<input type=color>` can't do alpha, and many tokens are rgba.
  The picker opens inline, not in a portal or nested modal. There is a filter box,
  and the list is one Tab stop with roving arrows/Home/End (`rovingIndexForKey`).
- **Live preview (right):** a mini fbro rendered in a container with the draft vars set
  inline: two panels with header and footer, rows for dir, file, exec, archive, media, link,
  dotfile, selected, focused and unfocused cursor, plus a button bar, a modal, a toast, a status
  line, and a terminal chip. Elements carry `data-token`. Hovering or clicking one selects its
  token row, and the selected row outlines the preview elements that use it. This is how the
  editor shows each colour in context.
- **Apply model (matches the settings spec's live-apply rule):** the draft updates the preview
  immediately. Saves are debounced (~300 ms) as `UpdateColorScheme`. When the edited
  scheme is active, the whole app updates from the backend patch. **Revert** restores the
  values from when the tab was opened. Delete asks for inline confirmation.
- **Escape:** first closes the open picker or the delete confirmation, then closes Settings (modal-stack §5).
  Focus returns to the token row, and after closing Settings, to the opener.
- Styling uses tokens only. New CSS is checked in both built-ins and in one custom scheme.

## Docs

- `spex/settings.md`: new "Custom color schemes" section covering the file format, folder, tab
  visibility rule, apply model, fallback, and the `colorScheme` key accepting `custom:*`.
- `spex/proto.md`: the new intents and messages.
- `AGENTS.md` styling rule: a new colour token also needs a `src/theme/tokens.ts` entry.
- `TESTING.md` coverage list.

## Critical files

`src-tauri/src/themes.rs` (new) · `src-tauri/src/state/settings.rs` ·
`src-tauri/src/ws/intents/{mod,settings,themes}.rs` · `src-tauri/src/ws/appearance.rs` ·
`src/styles.css` · `src/theme/{colorSchemes,tokens}.ts` · `src/components/{SettingsModal,
SettingsDisplayTab,SettingsColorSchemeTab,ColorSchemePreview,CustomSchemeStyles}.tsx` ·
`src/store/index.ts` · `src/ws/types.ts` · `package.json` (+react-colorful)

Code to reuse: `config::config_dir`, the atomic write in `bookmarks.rs::save`, `settings_mutation`
gate and broadcast in `ws/appearance.rs`, `rovingIndexForKey` / `isNativeActivationTarget`
(`modalKeyboard.ts`), `ModalLayer` / `useFocusReturn`, `useNotifications`,
the existing `color-scheme.mjs` smoke case.

## Verification

1. `cargo test --manifest-path src-tauri/Cargo.toml themes settings`. Covers the new unit tests and the
   existing color-scheme tests, updated for `String` resolution.
2. `npm run build` (tsc).
3. Manual `npm run tauri dev`:
   - The Display tab looks identical in both built-ins, including the tokenized panel/tab backgrounds.
   - Customize… shows the tab. Duplicate creates `~/.config/fbro/themes/<slug>.toml` with every token.
   - Editing a token updates the preview immediately and, once saved, the whole app, plus open viewer,
     diff, and terminal windows and the portals (bookmarks, context menu).
   - Deleting the active scheme falls back to its base.
   - Dropping a hand-written partial `.toml` into the folder makes it appear. A broken one shows its error.
   - Closing Settings and reopening it with Ctrl+, does not show the tab.
4. Smoke: new `tests/e2e/smoke/cases/color-scheme-editor.mjs`. It checks that the tab is hidden by default,
   appears through Customize…, Duplicate writes the file, a token edit changes the computed `--bg` on the
   root, Escape closes the picker before Settings, focus is restored, and the tab is gone after close.
   Run with the host E2E command from AGENTS.md.
5. macOS and Windows are not verified locally. Check the WebView2 `appearance: none` styling on
   the new inputs and the picker.
