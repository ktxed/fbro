# Findings: syntax highlighting in editor tabs

**Date:** 2026-11-11 · **Branch:** `feat/editor-syntax-highlighting` · **Status:** decided 2026-11-12, see `spex/plans/2026-11-12-editor-syntax-highlighting.md`

**Question:** can CodeMirror editor tabs highlight common languages, and what does it cost?

**Short answer:** yes. `@codemirror/language-data` is the official, battle-tested
way to do it. Most of the work is the colour palette (about 10 new theme tokens
× 2 schemes), not the wiring.

## 1. Current state

- `src/editor/codemirror.ts:137` applies `markdown()` to **every** file. A `.py` or `.sh`
  file is parsed as Markdown, so `# comment` shows as a bold accent-coloured heading
  and `*ptr` / `_name_` turn italic. This is a bug today, separate from the feature.
- `markdownHighlight` (`codemirror.ts:106`) only styles Markdown tags. `comment`/`meta`
  already map to `--text-muted` and `monospace` to `--text-info`. Code tags
  (keyword, string, number, type, function, property, operator) are unstyled.
- `createEditorState(doc, handlers)` is synchronous. `PanelEditor.tsx:200` calls it when
  `EditorSessionReady` arrives; `filePath` is already a prop there, so the
  language can be picked from the file name without a protocol change.
- The `EditorState` is cached per session (`src/editor/sessions.ts`), so a language
  chosen at creation survives tab switches. Not verified: whether a session's
  file name can change while it is open. If it can, the language would go stale.
- Backend limit: UTF-8, ≤ 2 MiB (`src-tauri/src/editor/mod.rs:19`). Lezer parses
  incrementally under a time budget, so 2 MiB is well within comfortable range.
- The current editor chunk `dist/assets/codemirror-*.js` is **543 KB** (minified,
  not gzipped). It is already large because `lang-markdown` statically pulls in
  `lang-html` → `lang-css` + `lang-javascript` (for HTML embedded in Markdown).
- Nothing else in the app highlights code: no shiki/highlight.js/prism in
  `package.json`, no syntect/tree-sitter in `Cargo.toml`. The F3 viewer shows plain
  text; the Markdown preview is rendered by `comrak` in the backend, with
  unhighlighted fenced code.

## 2. Options (OSS first, per AGENTS.md)

| Option | Fit | Verdict |
|---|---|---|
| **`@codemirror/language-data`** (descriptions + lazy `load()` per language) | Native CM6; one import; Lezer grammars for the major languages, legacy stream modes for the rest | **Recommended** |
| Hand-pick `@codemirror/lang-*` packages | Native, smaller list, but we'd maintain the extension map ourselves | Fallback if we want a curated set |
| Shiki / TextMate grammars (`codemirror-shiki`-style bridges) | VS Code-quality grammars, but Oniguruma WASM, non-incremental, third-party glue | No: heavy, fragile in webkitgtk |
| Tree-sitter WASM in the webview | Excellent grammars, but needs custom CM glue and WASM per language | No: custom work |
| Backend highlighting (syntect / tree-sitter in Rust) | Would fit "backend authoritative" and could serve the viewer too | No for the editor: tokens must update on every keystroke locally; could be reconsidered for the **viewer** |

## 3. Measurements (scratch bundle, esbuild, `--splitting --minify`)

`@codemirror/language-data@6.5.2`, entry = `languages` table + `load()`:

| | Size |
|---|---|
| Descriptions table (always loaded with the editor chunk) | 16 KB min / **4 KB gzip** |
| All languages, split | 124 chunks, **1.8 MB** total on disk |
| Largest chunks | 259 KB (shared legacy/lezer core), 104 KB, 98 KB, 87 KB |
| Languages listed | 143 |

So startup and editor-open costs stay the same, apart from a few KB. Each language
adds one or two chunk loads the first time a file of that type opens, from local
disk in Tauri. The app bundle grows by about 1.8 MB.

## 4. Coverage probe (`LanguageDescription.matchFilename`)

**Matched:** Rust, Python (+ `BUILD`, `.bzl`), TypeScript (`.ts/.mts/.cts`), TSX,
JavaScript (`.js/.mjs/.cjs`), JSON, YAML, TOML, Go, C/C++ (`.h` → C), Shell
(`.sh/.bash/.ksh`, `PKGBUILD`), Dockerfile, CMakeLists.txt, Kotlin, Swift,
PowerShell, Lua, Ruby (+ Gemfile/Rakefile), SQL, `.ini`/`.properties`.

**Not matched (would be plain text):** `Makefile`, `.bashrc`/`.zshrc`/`.profile`,
`justfile`, `.env`, `.gitignore`, `Cargo.lock`, `.jsonc`, extension-less shebang
scripts. The registry has no Nix, Zig, Svelte, Elixir, HCL/Terraform, or Diff entries.

Most of these gaps take a few lines to close: a small alias table (`.bashrc` → Shell,
`.jsonc`/`Cargo.lock` → JSON/TOML, `Makefile`), plus a first-line `#!` check
(`#!/bin/bash`, `#!/usr/bin/env python3`, …). `@codemirror/legacy-modes` ships modes
for some missing ones (e.g. `diff`, `nginx`) that we could wrap with `StreamLanguage`.

## 5. The costs, worst first

1. **Palette / theme tokens.** About 10 new `--syntax-*` tokens (keyword, string, number,
   comment, type, function, property/attribute, operator/punctuation, constant/atom,
   tag, invalid). Each needs an `fbro-dark` and a `daylight` value in `src/styles.css`,
   plus an entry in `src/theme/tokens.ts` (new group, e.g. `"syntax"` / "Code").
   Existing custom schemes inherit missing tokens from their `base`
   (`spex/settings.md` §custom schemes), so they don't break. `ColorSchemePreview.tsx`
   should probably show a code sample so users can see what they're editing.
2. **Loading is asynchronous.** `LanguageDescription.load()` returns a Promise. Options:
   - (a) await the language before `createEditorState`. Simple; the delay only
     happens on the first open of each language.
   - (b) create the state with plain text and swap the language in through a
     `Compartment` once it loads. Instant text, then a flash of colour.

   Recommend (a). It's local disk, and the editor already awaits `loadCodeMirror()`.
3. **Per-language behaviour changes.** HTML/XML/JSX add auto-close tags as you
   type; Markdown's Enter continues lists (already active today on every file,
   which is wrong for code). None of these use Mod-s/Mod-w/Mod-Shift-m/Escape/F3,
   so the editor's key ownership (`spex/keyboard.md`) doesn't change. To check:
   whether any `lang-*` package adds a keymap that shadows Tab/indent behaviour.
4. **Bundle growth.** About 1.8 MB on disk across about 124 lazy chunks. This makes
   the AppImage and installers bigger but doesn't slow startup.
5. **Legacy stream modes** (Shell, TOML, Dockerfile, INI, …) re-tokenise more
   coarsely and colour less precisely than Lezer grammars. Fine at 2 MiB.
6. **Consistency.** The editor would highlight code but the F3 viewer and the
   Markdown preview's fenced blocks wouldn't. That is a separate decision (see open
   questions).

## 6. Free wins

- `markdown({ codeLanguages: languages })` highlights fenced code blocks inside `.md`
  notes using the same lazy loading.
- Fixes the "everything is Markdown" bug in §1.
- Optional, cheap: `closeBrackets()` and `foldGutter()` from the same family. These
  are user-visible behaviour changes, though, so they're out of scope unless asked for.

## 7. Implementation sketch

- `package.json`: add `@codemirror/language-data` (brings `legacy-modes` + lang packages).
- `src/editor/codemirror.ts`:
  - `export async function languageFor(fileName, firstLine)`: alias table →
    `LanguageDescription.matchFilename(languages, name)` → shebang check →
    `await desc.load()`; returns `LanguageSupport | null`.
  - `createEditorState(doc, handlers, language)`: use `language ?? []` instead of the
    unconditional `markdown()`; `.md` files get `markdown({ codeLanguages: languages })`.
  - Replace `markdownHighlight` with one `HighlightStyle` covering Markdown and code
    tags, all `var(--syntax-*)` / existing tokens.
- `src/components/PanelEditor.tsx:200`: `await cm.languageFor(fileName, firstLine)` before
  `createEditorState`. Guard against `disposed` after the await.
- `src/styles.css` + `src/theme/tokens.ts`: new tokens for both schemes; possibly a code
  sample in `ColorSchemePreview.tsx`.
- Docs: `site/src/content/docs/viewing/editor.mdx` (a line on highlighting and which
  files get it); `site/.../customize/color-schemes.mdx` if a new token group appears.
  The `editor-tab` screenshot scene could use a code file.
- Tests: extend `tests/e2e/smoke/cases/editor-tab.mjs`. Open a `.rs`/`.py` file and
  assert tokens carry highlight classes (`.cm-line span[class]`); open a `.py` file
  and assert `# x` is **not** styled as a heading. `tsc` via `npm run build`.

## 8. Open questions for the user

1. **Language set:** everything in `language-data` (+1.8 MB, zero upkeep), or a curated
   list of ~20 `lang-*` packages (smaller, we maintain the extension map)?
2. **Palette source:** design our own `--syntax-*` colours, or derive them from an
   established scheme (e.g. One Dark for `fbro-dark`, GitHub Light for `daylight`)?
3. **Viewer parity:** should the F3 viewer / quick preview highlight too? That is a
   bigger change: the viewer is a different component and may handle large files
   where a CodeMirror read-only view or backend highlighting would be needed.
4. **Extras:** bracket auto-close / code folding, yes or no?

## 9. Platforms

Coverage and sizes were measured on Linux. Nothing here is platform-specific
(pure JS in the webview); WebView2 and WKWebView only need the usual check that
token colours come through `var(--…)`.
