# Plan: syntax highlighting in editor tabs

Status: implemented, awaiting validation · 2026-11-12 · branch `feat/editor-syntax-highlighting`

## Goal

Editor tabs highlight source code for common languages using CodeMirror's own
language support. Each file gets the language its name (or its `#!` line)
says, and no longer gets Markdown regardless of type. The editor also gains
bracket auto-close and code folding. Colours come from new theme tokens,
seeded from established themes.

## Decisions (agreed 2026-11-12)

| # | Decision |
|---|---|
| D1 | Languages come from `@codemirror/language-data` (lazy per language), plus a small alias table and shebang detection for its gaps. |
| D2 | Syntax colours: **One Dark** for `fbro-dark` and **GitHub Light** for `daylight`, as new `--syntax-*` tokens. |
| D3 | The F3 viewer, quick preview, and the comrak Markdown preview are **out of scope**. |
| D4 | `closeBrackets()` and `foldGutter()` + `foldKeymap` are added, because they are plain CM extensions. The one keymap risk is handled in §4. |

## Implementation notes (differences from the draft)

- **Bundled schemes** (`src-tauri/themes/*.toml`) each define the nine `--syntax-*`
  colours too, because `every_bundled_scheme_is_valid_complete_and_uniquely_named`
  expects every bundled scheme to carry the full token set. Solarized and IntelliJ
  use their own editor palettes, Matrix and Magenta use shades of their hue,
  Windows XP uses Visual Studio's classic colours, and CDE uses dark inks. Every value is
  adjusted in lightness to reach ≥ 4.5:1 against its `bg-panel`.
- `@codemirror/autocomplete` (for `closeBrackets`) and `@codemirror/legacy-modes`
  (diff) are direct dependencies, so their imports don't rely on transitive installs.
- The editor chunk shrank from 543 KB to 114 KB plus a shared CodeMirror view chunk
  (325 KB). Languages are separate lazy chunks; the main `index` chunk grew by ~2 KB.
- If a reload arrives while the first load is still waiting for its language,
  only the text is replaced (`pendingText` in `PanelEditor`).

## Non-goals

- Line numbers, autocompletion, linting, LSP.
- A per-file "change language" command (could follow later if needed).
- Highlighting in the viewer or preview (D3).
- Backend or protocol changes: language is picked from `panel.editor.fileName`,
  which the frontend already has. A session's path is fixed for its lifetime
  (no retargeting in `src-tauri/src/editor/` or `ws/intents/editor.rs`), so the
  language chosen at load never goes stale.

## 1. Dependency

- `npm i @codemirror/language-data` (6.5.x). This brings `@codemirror/legacy-modes`
  and the `lang-*` packages. Every language loads through a dynamic `import()`,
  so Vite splits each into its own chunk. Measured: about 4 KB gzip added to the
  editor chunk, about 124 lazy chunks / 1.8 MB on disk.
- Check `npm ls @codemirror/language @codemirror/state @codemirror/view` shows a
  single deduped copy of each, because a second copy breaks CM's `instanceof`
  checks. Pin with `overrides` if not.

## 2. Language detection: `src/editor/language.ts` (new, CM-only module)

Imported only by `codemirror.ts`, so it stays inside the lazy editor chunk.

```ts
export async function loadLanguage(fileName: string, firstLine: string): Promise<Extension>
```

Order:
1. **Markdown:** `.md`, `.markdown`, `.mdx` → `markdown({ codeLanguages: languages })`
   (fenced blocks highlight too, also lazily).
2. **Alias table** (file name → `language-data` name), for gaps the probe found:

   | Name / pattern | Language |
   |---|---|
   | `Makefile`, `GNUmakefile`, `*.mk` | none (plain). `legacy-modes` has no make mode, and Shell mis-colours `$(…)`/recipes. |
   | `.bashrc`, `.bash_profile`, `.zshrc`, `.profile`, `.envrc`, `*.zsh` | Shell |
   | `.env`, `.env.*` | Properties files |
   | `*.jsonc`, `*.json5`, `tsconfig*.json`, `.eslintrc` | JSON (comments are tolerated as error tokens; acceptable) |
   | `Cargo.lock`, `*.lock` that are TOML (`Cargo.lock`, `poetry.lock`, `uv.lock`) | TOML |
   | `.gitignore`, `.dockerignore`, `*.ignore` | none (plain) |
   | `*.diff`, `*.patch` | legacy `diff` mode via `StreamLanguage.define(diff)` |

3. `LanguageDescription.matchFilename(languages, fileName)`.
4. **Shebang**, only when 1–3 found nothing: `#!/usr/bin/env X` or `#!/…/X`,
   where X ∈ {sh, bash, zsh, dash, ksh} → Shell, python* → Python, node/deno/bun → JavaScript,
   ruby → Ruby, perl → Perl, lua → Lua. Look up by name via `LanguageDescription.matchLanguageName`.
5. Otherwise `[]`: plain text, no Markdown keymap.

`desc.load()` failures (a missing chunk) are logged with
`console.warn("[editor] …")` and fall back to plain text. The editor must never
fail to open because of highlighting.

The alias table and shebang parser are pure functions with no CM imports, so the
Markdown/alias/shebang decision is a plain string → language-name mapping that is
easy to test (§8).

## 3. Editor wiring

### `src/editor/codemirror.ts`
- `createEditorState(doc, handlers, language: Extension)`: replace the unconditional
  `markdown()` with `language`.
- Add `closeBrackets()`, `foldGutter()`, `codeFolding()` (comes with the gutter), and
  `keymap.of([...closeBracketsKeymap, ...foldKeys])`, ahead of `defaultKeymap`
  (`closeBracketsKeymap` must win Backspace).
- Replace `markdownHighlight` with one `codeHighlight` `HighlightStyle` (§5) that keeps
  the current Markdown look and adds code tags.
- Theme additions: `.cm-gutters` (background `var(--bg-panel)`, no border, colour
  `var(--text-muted)`), `.cm-foldGutter .cm-gutterElement:hover` (`var(--text)`),
  `.cm-foldPlaceholder` (`var(--bg-elev)`, `var(--border)`, `var(--text-dim)`). Literal
  colours stay out of the theme.
- Re-export `loadLanguage`.

### `src/components/PanelEditor.tsx` (≈ line 200)
On `EditorSessionReady` with no view yet:
```ts
const language = await cm.loadLanguage(fileName, firstLineOf(message.text));
if (disposed) return;
mount(cm, cm.createEditorState(message.text, handlersFor(sessionId), language));
```
- `fileName` is read through the existing `latest` ref pattern, so the mount effect's
  dependency list (`client, panelId, sessionId`) doesn't change.
- `replaceFromDisk` and the cached state are untouched: the language lives in the
  cached `EditorState`.
- Session restore after restart goes through the same `LoadEditorSession` path, so
  it gets the language too.

## 4. Keyboard

New keys while the editor has focus (CM-local; not registry commands):

| Key (Linux/Windows) | macOS | Action |
|---|---|---|
| Ctrl+Shift+[ | Cmd+Alt+[ | Fold block at cursor |
| Ctrl+Shift+] | Cmd+Alt+] | Unfold |
| Backspace between an empty pair | same | Delete both brackets |

**Dropped on purpose:** CM's `foldAll` / `unfoldAll` (Ctrl+Alt+[ / ]). On Windows,
AltGr arrives as Ctrl+Alt, and on German, Polish, Czech, … layouts AltGr+8 types
`[`. CM's keymap matches the exact `Ctrl-Alt-[` name *before* its AltGr guard
(`@codemirror/view` `runHandlers`: the guard only protects the base-key fallback),
so typing `[` would fold the whole file. `foldKeys = foldKeymap.filter(b => !b.key?.startsWith("Ctrl-Alt-"))`.
This means Fold All / Unfold All aren't offered at all, since the gutter folds one
block at a time. Revisit if asked for.

Conflicts checked:
- `panel.history.back/forward` are bare `[` / `]`. The editor owns letters and
  punctuation, so they never reach the listing from the editor. No change.
- Released pass-through keys (`RELEASED_COMMAND_IDS`: Ctrl+Tab, Ctrl+Shift+Tab,
  palette) and `file.view` (F3 / Cmd+3), `panel.viewer.embed` (Ctrl+F3 / Cmd+3 variant),
  `notes.new` (Ctrl+Shift+F4 / Cmd+Alt+4) don't use `[` / `]`. Verified in `ws/commands.rs`.
- On a non-US layout, Ctrl+Shift+[ may not be typeable as such. CM matches by
  `event.key` with a `keyCode` base-name fallback, which is the same story as the
  existing Mod-g/Mod-f. Accept it, and note it in the guide.

`spex/keyboard.md` §3a (editor key ownership) gets the two fold keys and the
foldAll exclusion with its reason.

## 5. Colours

### Tokens (`src/styles.css`, both scheme blocks; `src/theme/tokens.ts`, new group)

New `TokenGroupId` `"syntax"`, label **"Code"**, placed after `"terminal"`.

| Token | Covers (`@lezer/highlight` tags) | fbro-dark (One Dark) | daylight (GitHub Light) |
|---|---|---|---|
| `--syntax-keyword` | keyword, controlKeyword, moduleKeyword, operatorKeyword, modifier, self | `#c678dd` | `#cf222e` |
| `--syntax-string` | string, character, inserted, special(string) | `#98c379` | `#0a3069` |
| `--syntax-number` | number, bool, atom, null, constant(name), standard(name) | `#d19a66` | `#0550ae` |
| `--syntax-type` | typeName, className, namespace, annotation | `#e5c07b` | `#953800` |
| `--syntax-function` | function(variableName), function(propertyName), macroName, labelName | `#61afef` | `#8250df` |
| `--syntax-property` | propertyName, attributeName, definition(propertyName) | `#e06c75` | `#0550ae` |
| `--syntax-tag` | tagName, angleBracket | `#e06c75` | `#116329` |
| `--syntax-operator` | operator, punctuation-ish: escape, regexp, url, special(name) | `#56b6c2` | `#cf222e` |
| `--syntax-comment` | comment, lineComment, blockComment, docComment, meta | `#7f848e` | `#6e7781` |

Kept on existing tokens, so editing notes looks the same as today: heading →
`--accent`, link/url → `--text-link`, monospace → `--text-info`, quote →
`--text-muted`, list → `--accent`, invalid → `--text-danger`, deleted →
`--text-danger`.

Plain identifiers, `variableName`, and punctuation keep `--text`, so dense code
doesn't turn into confetti.

The values are seeds. Check them by eye on `--bg-panel` / `--bg-panel-focused` in both
schemes (One Dark's comment grey `#5c6370` is too dim on `#111`, hence `#7f848e`),
and aim for WCAG AA 4.5:1 on body-size text.

Custom schemes need no migration: missing tokens cascade from `base` through
`data-color-scheme-base` (`spex/settings.md`).

### Colour scheme editor preview
`src/components/ColorSchemePreview.tsx`: add a small code sample (5–6 lines, for
example a Rust function with a comment, a string, a number, a type, and a call).
Its spans list their token in `data-tokens`, like the other preview parts, so
clicking one selects it and the selected token gets `data-token-active`.

## 6. Docs

- `site/src/content/docs/viewing/editor.mdx`:
  - "Editor tabs": one paragraph. Code is highlighted based on the file name or
    the `#!` line; Markdown notes highlight fenced code blocks; brackets and quotes
    close automatically.
  - Keys table: fold / unfold rows (hard-coded `<kbd>`, like the other CM-local rows).
- `site/src/content/docs/customize/color-schemes.mdx`: mention the **Code** group if the
  page lists groups.
- Screenshot: leave `editor-tab` (Markdown note + preview) as it is. Optionally add a code
  scene later. Not required.
- `spex/keyboard.md` §3a as in §4. No change to `proto.md` (no protocol change).
- AGENTS.md spec map: add a row for `spex/findings/` ("investigations behind plans"),
  if we keep that folder.

## 7. Implementation order

1. [ ] Add dependency; confirm deduped CM packages; `npm run build`.
2. [ ] `language.ts`: alias table, shebang parser, `loadLanguage`.
3. [ ] `codemirror.ts`: language param, `codeHighlight`, closeBrackets, fold gutter + filtered keymap, gutter theme.
4. [ ] `PanelEditor.tsx`: await language before mount.
5. [ ] Tokens in `styles.css` (both schemes) + `tokens.ts`; tune contrast by eye in both schemes.
6. [ ] `ColorSchemePreview.tsx` code sample.
7. [ ] Smoke test (§8), `TESTING.md` coverage list.
8. [ ] Guide + `keyboard.md`.
9. [ ] Check `vite build` output: language chunks present, editor chunk growth ≈ 16 KB min.

## 8. Tests

- **Frontend logic:** `tsc` is the only frontend check (no unit runner), so keep the
  alias/shebang mapping small and obvious. It's exercised by the smoke case below.
- **Smoke (`tests/e2e/smoke/cases/editor-tab.mjs`, extend):**
  - Open `sample.rs` in an editor tab → wait for `.cm-content span[class^="ͼ"]`, i.e. a
    highlighted token, and for `fn` to carry a class different from a plain identifier.
  - Open `script.py` whose first line is `# comment` → that line is **not** bold
    (it used to be a Markdown heading).
  - Open an extension-less file starting with `#!/bin/sh` → tokens highlighted.
  - Type `(` → the document contains `()`, cursor between them; Backspace removes both.
  - Ctrl+Shift+[ on a Rust block folds it (`.cm-foldPlaceholder` appears), Ctrl+Shift+] unfolds.
  - Escape still returns to the origin tab and focus lands on the listing row
    (regression for keymap order).
- **Rust:** none needed (no backend change). Run `cargo test` only if the site command
  snapshot is touched, which it shouldn't be.

## 9. Risks

| Risk | Mitigation |
|---|---|
| AltGr `[` triggers Fold All on Windows | Ctrl-Alt fold bindings removed (§4) |
| Duplicate `@codemirror/*` copies after install | `npm ls` check, `overrides` |
| Chunk load fails (corrupt install) | Fallback to plain text + `console.warn` |
| Auto-closed quotes in Markdown prose (`'`) | CM skips closing after a word character (`don't`); accept |
| Legacy-mode quality (Shell, TOML) | Acceptable. These are coarse but correct-ish |
| Bundle +1.8 MB | Accepted (D1); lazy, so no startup cost |

## 10. Platforms

Linux is verified by smoke. macOS (Cmd+Alt+[ fold keys, WKWebView chunk loading
via `tauri://`) and Windows (WebView2, AltGr layouts) need manual checks: open a
`.rs` file, fold/unfold, and type `[` on a German layout on Windows.
