import { useRef } from "react";
import { useAppDispatch, type PendingEditorConflict } from "../store";
import { flushEditor } from "../editor/sessions";
import { ModalLayer } from "./ModalStack";
import { ModalDialog } from "./ModalDialog";
import { MnemonicLabel } from "./MnemonicLabel";
import { isLetterKey, isNativeActivationTarget } from "./modalKeyboard";
import { runEditorAction } from "./PanelEditor";

interface Props {
  conflict: PendingEditorConflict;
}

/** The editor's file changed (or vanished) on disk while it had unsaved
 *  changes. Autosave stays paused until the user picks a version. */
export function EditorConflictModal({ conflict }: Props) {
  const dispatch = useAppDispatch();
  const primaryRef = useRef<HTMLButtonElement>(null);
  const missing = conflict.kind === "missing";
  const close = () => dispatch({ type: "CLOSE_EDITOR_CONFLICT" });
  const overwrite = () => {
    close();
    void flushEditor(conflict.sessionId, true);
  };
  const reload = () => {
    close();
    runEditorAction(conflict.sessionId, "reload");
  };
  const keepBoth = () => {
    close();
    runEditorAction(conflict.sessionId, "keepBoth");
  };

  return (
    <ModalLayer
      modalId="editor-conflict"
      initialFocusRef={primaryRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.defaultPrevented) return;
        if (event.key === "Escape") {
          event.preventDefault();
          close();
          return;
        }
        if (event.ctrlKey || event.metaKey || event.altKey) return;
        if (isLetterKey(event, missing ? "s" : "o")) { event.preventDefault(); overwrite(); }
        else if (!missing && isLetterKey(event, "r")) { event.preventDefault(); reload(); }
        else if (!missing && isLetterKey(event, "k")) { event.preventDefault(); keepBoth(); }
        else if (event.key === "Enter" && !isNativeActivationTarget(event.target)) { event.preventDefault(); overwrite(); }
      }}
    >
      <ModalDialog ariaLabel={missing ? "File is missing" : "File changed on disk"}>
        <div className="modal__header">{missing ? "File is missing" : "File changed on disk"}</div>
        <div className="modal__body">
          <p className="modal__summary">
            {missing
              ? <>The file <strong>{conflict.fileName}</strong> was deleted or renamed outside the editor. Your text is still in the editor.</>
              : <>The file <strong>{conflict.fileName}</strong> changed on disk while the editor had unsaved changes.</>}
          </p>
          <div className="modal__actions">
            <button ref={primaryRef} className="modal__btn modal__btn--primary" aria-keyshortcuts={missing ? "S" : "O"} data-testid="editor-conflict-overwrite" onClick={overwrite}>
              <MnemonicLabel text={missing ? "Save it again" : "Overwrite"} mnemonic={missing ? "s" : "o"} />
            </button>
            {!missing && (
              <button className="modal__btn" aria-keyshortcuts="R" data-testid="editor-conflict-reload" onClick={reload}>
                <MnemonicLabel text="Reload" mnemonic="r" />
              </button>
            )}
            {!missing && (
              <button className="modal__btn" aria-keyshortcuts="K" data-testid="editor-conflict-keep-both" onClick={keepBoth}>
                <MnemonicLabel text="Keep both" mnemonic="k" />
              </button>
            )}
            <button className="modal__btn" data-testid="editor-conflict-cancel" onClick={close}>
              Cancel
            </button>
          </div>
        </div>
      </ModalDialog>
    </ModalLayer>
  );
}
