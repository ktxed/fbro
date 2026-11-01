import { useRef } from "react";
import { useAppDispatch } from "../store";
import type { WsClient } from "../ws/client";
import { ModalLayer } from "./ModalStack";
import { ModalDialog } from "./ModalDialog";

interface Props {
  client: WsClient;
  failed: number;
}

/** Quitting could not save every editor tab (conflict, missing file, or a
 *  failed write). Staying is the default. */
export function EditorQuitModal({ client, failed }: Props) {
  const dispatch = useAppDispatch();
  const stayRef = useRef<HTMLButtonElement>(null);
  const stay = () => dispatch({ type: "CLOSE_EDITOR_QUIT" });
  const closeAnyway = () => {
    dispatch({ type: "CLOSE_EDITOR_QUIT" });
    client.sendIntent("Quit", undefined, { editorsFlushed: true });
  };
  const files = failed === 1 ? "1 file" : `${failed} files`;

  return (
    <ModalLayer
      modalId="editor-quit"
      initialFocusRef={stayRef}
      tabIndex={-1}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          stay();
        }
      }}
    >
      <ModalDialog ariaLabel="Unsaved editor tabs">
        <div className="modal__header">Unsaved editor tabs</div>
        <div className="modal__body">
          <p className="modal__summary">
            {files} couldn't be saved. Closing now loses those changes.
          </p>
          <div className="modal__actions">
            <button className="modal__btn modal__btn--primary" data-testid="editor-quit-close" onClick={closeAnyway}>
              Close anyway
            </button>
            <button ref={stayRef} className="modal__btn" data-testid="editor-quit-stay" onClick={stay}>
              Stay
            </button>
          </div>
        </div>
      </ModalDialog>
    </ModalLayer>
  );
}
