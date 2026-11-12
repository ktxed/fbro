// CodeMirror 6 setup for built-in editor tabs. Imported lazily by
// `PanelEditor`, so CodeMirror stays out of the startup bundle.
import { EditorState, Prec, Transaction, type Extension } from "@codemirror/state";
import {
  EditorView,
  drawSelection,
  highlightActiveLine,
  highlightSpecialChars,
  keymap,
  placeholder,
  type KeyBinding,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import {
  HighlightStyle,
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
  syntaxHighlighting,
} from "@codemirror/language";
import { closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { tags } from "@lezer/highlight";

export interface EditorHandlers {
  /** Escape after CodeMirror's own layers (find panel) declined it. */
  onEscape: () => void;
  /** Mod-s. */
  onSave: () => void;
  /** Mod-w. */
  onClose: () => void;
  /** Mod-Shift-m: toggle filling the window. */
  onToggleMaximize: () => void;
  /** Every document change that came from the user. */
  onUserChange: () => void;
  /** The cursor moved or the text changed; read layout via `requestMeasure`. */
  onCursor: (view: EditorView) => void;
}

// Colours come from the app's CSS tokens so both built-in schemes and custom
// schemes apply without a second palette.
const theme = EditorView.theme({
  "&": {
    height: "100%",
    color: "var(--text)",
    backgroundColor: "var(--bg-panel)",
    fontSize: "inherit",
  },
  "&.cm-focused": { outline: "none" },
  ".cm-scroller": { fontFamily: "var(--font)", lineHeight: "1.5" },
  ".cm-content": { padding: "8px 0", caretColor: "var(--accent)" },
  ".cm-line": { padding: "0 12px" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)" },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "var(--bg-cursor-focused)",
  },
  ".cm-activeLine": { backgroundColor: "var(--bg-hover)" },
  ".cm-selectionMatch": { backgroundColor: "var(--bg-selected)" },
  ".cm-searchMatch": { backgroundColor: "var(--bg-selected)", outline: "1px solid var(--border)" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--bg-cursor-focused)" },
  ".cm-placeholder": { color: "var(--text-muted)" },
  ".cm-gutters": { backgroundColor: "var(--bg-panel)", color: "var(--text-muted)", border: "none" },
  ".cm-foldGutter .cm-gutterElement": { padding: "0 2px 0 6px", cursor: "pointer" },
  ".cm-foldGutter .cm-gutterElement:hover": { color: "var(--text)" },
  ".cm-foldPlaceholder": {
    backgroundColor: "var(--bg-elev)",
    border: "1px solid var(--border)",
    color: "var(--text-dim)",
    padding: "0 4px",
  },
  ".cm-panels": { backgroundColor: "var(--bg-elev)", color: "var(--text)", borderColor: "var(--border)" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border)" },
  // The find panel mirrors `.modal__btn` and the app's text inputs. CodeMirror's
  // base theme paints `.cm-button` with a light/dark gradient image, so the
  // `background` shorthand is needed to clear it.
  ".cm-panel.cm-search .cm-textfield": {
    appearance: "none",
    font: "inherit",
    color: "var(--text)",
    background: "var(--bg-input)",
    border: "1px solid var(--border)",
    borderRadius: "3px",
    padding: "3px 8px",
    outline: "none",
  },
  ".cm-panel.cm-search .cm-textfield:focus": { borderColor: "var(--border-focus)" },
  ".cm-panel.cm-search .cm-button": {
    appearance: "none",
    font: "inherit",
    color: "var(--text-dim)",
    background: "var(--bg-footer)",
    border: "1px solid var(--border)",
    borderRadius: "3px",
    padding: "3px 12px",
    cursor: "pointer",
  },
  ".cm-panel.cm-search .cm-button:hover, .cm-panel.cm-search .cm-button:focus-visible": {
    background: "var(--bg-focused)",
    color: "var(--text)",
    borderColor: "var(--border-focus)",
    outline: "none",
  },
  ".cm-panel.cm-search .cm-button:active": { background: "var(--bg-focused)" },
  ".cm-panel.cm-search label": { color: "var(--text-dim)", fontSize: "inherit", cursor: "pointer" },
  ".cm-panel.cm-search input[type=checkbox]": { accentColor: "var(--accent)", margin: "0 4px 0 0", verticalAlign: "middle" },
  ".cm-panel.cm-search [name=close]": {
    appearance: "none",
    font: "inherit",
    color: "var(--text-muted)",
    background: "transparent",
    border: "none",
    cursor: "pointer",
  },
  ".cm-panel.cm-search [name=close]:hover": { color: "var(--text)" },
});

// Markdown keeps the app's own tokens; code uses the `--syntax-*` tokens
// (seeded from One Dark and GitHub Light). Plain identifiers and punctuation
// stay `--text`.
const codeHighlight = HighlightStyle.define([
  { tag: tags.heading, fontWeight: "bold", color: "var(--accent)" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.strong, fontWeight: "bold" },
  { tag: tags.strikethrough, textDecoration: "line-through" },
  { tag: [tags.link, tags.url], color: "var(--text-link)", textDecoration: "underline" },
  { tag: [tags.monospace, tags.processingInstruction], color: "var(--text-info)" },
  { tag: tags.quote, color: "var(--text-muted)" },
  { tag: tags.list, color: "var(--accent)" },
  { tag: [tags.comment, tags.meta], color: "var(--syntax-comment)" },
  { tag: [tags.keyword, tags.modifier, tags.self, tags.operatorKeyword], color: "var(--syntax-keyword)" },
  { tag: [tags.string, tags.character, tags.inserted, tags.special(tags.string)], color: "var(--syntax-string)" },
  { tag: [tags.number, tags.bool, tags.atom, tags.null, tags.constant(tags.name), tags.standard(tags.name)], color: "var(--syntax-number)" },
  { tag: [tags.typeName, tags.className, tags.namespace, tags.annotation], color: "var(--syntax-type)" },
  {
    tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.macroName, tags.labelName],
    color: "var(--syntax-function)",
  },
  { tag: [tags.propertyName, tags.attributeName], color: "var(--syntax-property)" },
  { tag: [tags.tagName, tags.angleBracket], color: "var(--syntax-tag)" },
  { tag: [tags.operator, tags.escape, tags.regexp, tags.special(tags.name)], color: "var(--syntax-operator)" },
  { tag: [tags.invalid, tags.deleted], color: "var(--text-danger)" },
]);

// CodeMirror's Fold All / Unfold All are Ctrl-Alt-[ / ]. On Windows, AltGr
// arrives as Ctrl+Alt, and on German, Polish, … layouts AltGr types `[`; the
// exact key name matches before CodeMirror's AltGr guard, so typing `[` would
// fold the whole file. Fold and unfold at the cursor stay.
const foldKeys = foldKeymap.filter((binding) => !binding.key?.startsWith("Ctrl-Alt-"));

/** F3 belongs to the viewer in fbro; Mod-g still finds the next match. */
const searchKeys = searchKeymap.filter((binding) => binding.key !== "F3");

function editorKeys(handlers: EditorHandlers): KeyBinding[] {
  return [
    { key: "Mod-s", preventDefault: true, run: () => { handlers.onSave(); return true; } },
    { key: "Mod-w", preventDefault: true, run: () => { handlers.onClose(); return true; } },
    { key: "Mod-Shift-m", preventDefault: true, run: () => { handlers.onToggleMaximize(); return true; } },
    { key: "Escape", run: () => { handlers.onEscape(); return true; } },
  ];
}

/** `language` comes from `loadLanguage` (`[]` for plain text). */
export function createEditorState(doc: string, handlers: EditorHandlers, language: Extension): EditorState {
  const extensions: Extension[] = [
    highlightSpecialChars(),
    history(),
    drawSelection(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    indentOnInput(),
    bracketMatching(),
    EditorView.lineWrapping,
    closeBrackets(),
    foldGutter(),
    language,
    syntaxHighlighting(codeHighlight),
    search({ top: true }),
    placeholder("Empty"),
    theme,
    keymap.of([...closeBracketsKeymap, ...searchKeys, ...foldKeys, ...defaultKeymap, ...historyKeymap, indentWithTab]),
    // Lowest priority: the find panel's own Escape handling runs first.
    Prec.lowest(keymap.of(editorKeys(handlers))),
    EditorView.updateListener.of((update) => {
      if (update.selectionSet || update.docChanged || update.focusChanged) handlers.onCursor(update.view);
      if (!update.docChanged) return;
      const fromUser = update.transactions.some((tr) => tr.docChanged && !tr.annotation(Transaction.remote));
      if (fromUser) handlers.onUserChange();
    }),
  ];
  return EditorState.create({ doc, extensions });
}

export function createEditorView(parent: HTMLElement, state: EditorState): EditorView {
  return new EditorView({ parent, state });
}

/** Replace the whole document with text from disk, keeping it undoable but not
 *  counting it as an unsaved user change. */
export function replaceFromDisk(view: EditorView, text: string): void {
  const current = view.state.doc.toString();
  if (current === text) return;
  const anchor = Math.min(view.state.selection.main.anchor, text.length);
  view.dispatch({
    changes: { from: 0, to: view.state.doc.length, insert: text },
    selection: { anchor },
    annotations: Transaction.remote.of(true),
  });
}

export { loadLanguage } from "./language";
export type { EditorState, EditorView };
