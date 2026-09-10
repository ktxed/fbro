import { useRef, useEffect } from "react";
import { useAppDispatch } from "../store";
import type { PendingNoEditor } from "../store";
import { IconCopy } from "@tabler/icons-react";
import { ModalLayer } from "./ModalStack";

interface NoEditorModalProps {
  noEditor: PendingNoEditor;
}

export function NoEditorModal({ noEditor }: NoEditorModalProps) {
  const dispatch = useAppDispatch();
  const overlayRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    overlayRef.current?.focus();
  }, []);

  const close = () => dispatch({ type: "CLOSE_NO_EDITOR" });

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Escape" || e.key === "Enter") {
      e.preventDefault();
      close();
    }
  };

  return (
    <ModalLayer modalId="no-editor" ref={overlayRef} onClick={close} onKeyDown={handleKeyDown}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">file: {noEditor.filename}</div>
        <div className="modal__body">
          <div className="modal__no-editor-path">
            <code className="modal__no-editor-code">{noEditor.fullPath}</code>
            <button
              className="modal__job-param-copy"
              onClick={() => navigator.clipboard.writeText(noEditor.fullPath)}
              title="Copy path"
            >
              <IconCopy size={14} />
            </button>
          </div>
          <p className="modal__no-editor-msg">No editor available</p>
        </div>
        <div className="modal__actions">
          <button className="modal__btn modal__btn--primary" onClick={close}>OK</button>
        </div>
      </div>
    </ModalLayer>
  );
}
