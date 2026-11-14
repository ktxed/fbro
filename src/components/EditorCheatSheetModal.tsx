import { useRef } from "react";

import { useAppDispatch, useAppState } from "../store";
import { isMac } from "../utils/platform";
import type { WsClient } from "../ws/client";
import type { KeyBinding } from "../ws/types";
import { CheatSheetSections, type CheatSheetSection } from "./CheatSheetModal";
import { ModalLayer } from "./ModalStack";
import { ResizableModal } from "./ResizableModal";

interface Chord {
  mod?: boolean;
  shift?: boolean;
  alt?: boolean;
  /** A literal Control key, also on macOS. */
  ctrl?: boolean;
}

/** `mod` is Ctrl, or Cmd on macOS. */
function chord(key: string, { mod, shift, alt, ctrl }: Chord = {}): KeyBinding {
  return {
    strokes: [{ key, shift: !!shift, ctrl: !!ctrl || (!!mod && !isMac), alt: !!alt, meta: !!mod && isMac }],
    modes: [],
  };
}

// Editor-local shortcuts are not registry commands (`spex/keyboard.md` §3a), so
// they are listed here; the registry rows below read the live bindings. The
// CodeMirror rows mirror `defaultKeymap` / `searchKeymap` as `codemirror.ts`
// installs them (F3, Select All Occurrences and Fold All removed).
const SECTIONS: CheatSheetSection[] = [
  {
    title: "View",
    rows: [
      { label: "Wrap long lines", fixed: [chord("z", { alt: true })] },
      { label: "Line numbers", fixed: [chord("n", { alt: true })] },
      { label: "Show whitespace", fixed: [chord("w", { alt: true })] },
      { label: "Full screen / restore", fixed: [chord("m", { mod: true, shift: true })] },
      { label: "This sheet", fixed: [chord("F1")] },
    ],
  },
  {
    title: "File and tabs",
    rows: [
      { label: "Save now", fixed: [chord("s", { mod: true })] },
      { label: "Close tab", fixed: [chord("w", { mod: true })] },
      { label: "Back to the previous tab", fixed: [chord("Escape")] },
      { label: "Open in viewer", commandIds: ["file.view"] },
      { label: "Quick preview", commandIds: ["panel.viewer.embed"] },
      { label: "New note", commandIds: ["notes.new"] },
      { label: "Next / previous tab", commandIds: ["panel.tab.next", "panel.tab.prev"] },
      { label: "Command palette", commandIds: ["app.command_palette"] },
    ],
  },
  {
    title: "Find",
    rows: [
      { label: "Find and replace", fixed: [chord("f", { mod: true })] },
      { label: "Next / previous match", fixed: [chord("g", { mod: true }), chord("g", { mod: true, shift: true })] },
      { label: "Go to line", fixed: [chord("g", { mod: true, alt: true })] },
      { label: "Select next occurrence", fixed: [chord("d", { mod: true })] },
    ],
  },
  {
    title: "Editing",
    rows: [
      { label: "Undo / redo", fixed: [chord("z", { mod: true }), isMac ? chord("z", { mod: true, shift: true }) : chord("y", { mod: true })] },
      { label: "Indent / un-indent", fixed: [chord("Tab"), chord("Tab", { shift: true })] },
      { label: "Toggle comment", fixed: [chord("/", { mod: true })] },
      { label: "Move line up / down", fixed: [chord("ArrowUp", { alt: true }), chord("ArrowDown", { alt: true })] },
      { label: "Copy line up / down", fixed: [chord("ArrowUp", { alt: true, shift: true }), chord("ArrowDown", { alt: true, shift: true })] },
      { label: "Delete line", fixed: [chord("k", { mod: true, shift: true })] },
      { label: "Insert blank line below", fixed: [chord("Enter", { mod: true })] },
      { label: "Select line", fixed: [isMac ? chord("l", { ctrl: true }) : chord("l", { alt: true })] },
      { label: "Grow selection to syntax", fixed: [chord("i", { mod: true })] },
      { label: "Add cursor above / below", fixed: [chord("ArrowUp", { mod: true, alt: true }), chord("ArrowDown", { mod: true, alt: true })] },
    ],
  },
  {
    title: "Folding",
    rows: [
      { label: "Fold / unfold at cursor", fixed: [chord("[", { mod: true, alt: isMac, shift: !isMac }), chord("]", { mod: true, alt: isMac, shift: !isMac })] },
      { label: "Jump to matching bracket", fixed: [chord("\\", { mod: true, shift: true })] },
    ],
  },
];

/** The built-in editor's shortcut sheet: F1 inside an editor tab. A modal like
 *  the app sheet, so the modal stack returns focus to the editor on close. */
export function EditorCheatSheetModal({ client }: { client: WsClient }) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const bodyRef = useRef<HTMLDivElement>(null);

  const close = () => dispatch({ type: "CLOSE_EDITOR_CHEAT_SHEET" });
  const openSettings = () => {
    close();
    dispatch({ type: "OPEN_SETTINGS", tab: "display" });
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.defaultPrevented) return;
    // Escape or F1, the key that opened it, closes.
    if (event.key === "Escape" || event.key === "F1") {
      event.preventDefault();
      close();
    }
  };

  return (
    <ModalLayer modalId="editor-cheat-sheet" initialFocusRef={bodyRef} onClick={close} onKeyDown={onKeyDown}>
      <ResizableModal
        modalId="editor-cheat-sheet"
        client={client}
        className="cheat-sheet"
        ariaModal
        ariaLabel="Editor Shortcuts"
        testId="editor-cheat-sheet-modal"
        defaultSize={{ width: 760, height: 560 }}
        minSize={{ width: 420, height: 320 }}
      >
        <div className="modal__header">Editor Shortcuts</div>
        <div
          ref={bodyRef}
          className="modal__body cheat-sheet__body"
          tabIndex={0}
          aria-label="Editor shortcuts"
          data-testid="editor-cheat-sheet-body"
        >
          <CheatSheetSections sections={SECTIONS} commands={state.commands} paletteBinding={state.commands["app.command_palette"]?.bindings[0]} />
        </div>
        <div className="modal__actions">
          <button className="modal__btn modal__btn--primary" type="button" onClick={close} data-testid="editor-cheat-sheet-close">
            Close
          </button>
          <button className="modal__btn" type="button" onClick={openSettings} data-testid="editor-cheat-sheet-settings">
            Editor settings…
          </button>
        </div>
      </ResizableModal>
    </ModalLayer>
  );
}
