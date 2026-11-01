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
import { HighlightStyle, bracketMatching, indentOnInput, syntaxHighlighting } from "@codemirror/language";
import { markdown } from "@codemirror/lang-markdown";
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
  ".cm-panels": { backgroundColor: "var(--bg-elev)", color: "var(--text)", borderColor: "var(--border)" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--border)" },
  ".cm-panel.cm-search input, .cm-panel.cm-search button": {
    appearance: "none",
    font: "inherit",
    color: "var(--text)",
    backgroundColor: "var(--bg-input)",
    border: "1px solid var(--border)",
    borderRadius: "3px",
  },
  ".cm-panel.cm-search label": { color: "var(--text)" },
  ".cm-panel.cm-search [name=close]": { color: "var(--text-muted)", backgroundColor: "transparent", border: "none" },
});

const markdownHighlight = HighlightStyle.define([
  { tag: tags.heading, fontWeight: "bold", color: "var(--accent)" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.strong, fontWeight: "bold" },
  { tag: tags.strikethrough, textDecoration: "line-through" },
  { tag: [tags.link, tags.url], color: "var(--text-link)", textDecoration: "underline" },
  { tag: [tags.monospace, tags.processingInstruction], color: "var(--text-info)" },
  { tag: [tags.quote, tags.comment, tags.meta], color: "var(--text-muted)" },
  { tag: tags.list, color: "var(--accent)" },
]);

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

export function createEditorState(doc: string, handlers: EditorHandlers): EditorState {
  const extensions: Extension[] = [
    highlightSpecialChars(),
    history(),
    drawSelection(),
    highlightActiveLine(),
    highlightSelectionMatches(),
    indentOnInput(),
    bracketMatching(),
    EditorView.lineWrapping,
    markdown(),
    syntaxHighlighting(markdownHighlight),
    search({ top: true }),
    placeholder("Empty"),
    theme,
    keymap.of([...searchKeys, ...defaultKeymap, ...historyKeymap, indentWithTab]),
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

export type { EditorState, EditorView };
