import { useState, useRef, useEffect } from "react";
import { useAppDispatch } from "../store";
import type { PendingMkdir } from "../store";
import type { WsClient } from "../ws/client";

interface MkDirModalProps {
  mkdir: PendingMkdir;
  client: WsClient;
}

export function MkDirModal({ mkdir, client }: MkDirModalProps) {
  const dispatch = useAppDispatch();
  const [value, setValue] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    if (mkdir.error) setSubmitting(false);
  }, [mkdir.error]);

  const close = () => dispatch({ type: "CLOSE_MKDIR" });

  const submit = () => {
    const name = value.trim();
    if (submitting || !name) return;
    setSubmitting(true);
    client.sendIntent("MakeDir", undefined, {
      sourcePanel: mkdir.sourcePanel,
      dirPath: mkdir.dirPath,
      name,
    });
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter")  { e.preventDefault(); submit(); }
    if (e.key === "Escape") { e.preventDefault(); close(); }
  };

  return (
    <div className="modal-overlay" tabIndex={-1} onClick={close} onKeyDown={(e) => { if (e.key === "Tab") e.preventDefault(); }}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">Create Directory</div>
        <div className="modal__body">
          <p className="modal__rename-from" title={mkdir.dirPath}>
            In: {mkdir.dirPath}
          </p>
          <div className="modal__rename-row">
            <label className="modal__rename-label">Name:</label>
            <input
              ref={inputRef}
              className="modal__rename-input"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={handleKeyDown}
              spellCheck={false}
              autoComplete="off"
            />
          </div>
          {mkdir.error && (
            <p className="modal__rename-error">{mkdir.error}</p>
          )}
        </div>
        <div className="modal__actions">
          <button
            className="modal__btn modal__btn--primary"
            disabled={submitting || !value.trim()}
            onClick={submit}
          >
            {submitting ? "Creating…" : "Create"}
          </button>
          <button className="modal__btn" onClick={close}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
