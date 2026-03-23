import { useState, useRef, useEffect } from "react";
import { useAppDispatch } from "../store";
import type { PendingRename } from "../store";
import type { WsClient } from "../ws/client";

interface RenameModalProps {
  rename: PendingRename;
  client: WsClient;
}

export function RenameModal({ rename, client }: RenameModalProps) {
  const dispatch = useAppDispatch();
  const [value, setValue] = useState(rename.filename);
  const [localError, setLocalError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  // When a new error arrives from the store, allow re-submission
  useEffect(() => {
    if (rename.error) setSubmitting(false);
  }, [rename.error]);

  const close = () => dispatch({ type: "CLOSE_RENAME" });

  const submit = () => {
    const dest = value.trim();
    if (submitting || !dest || dest === rename.filename) { close(); return; }
    if (dest.includes("/") || dest.includes("\\")) {
      setLocalError("Rename cannot change directories — use Move (F6) for that.");
      return;
    }
    setLocalError(null);
    setSubmitting(true);
    client.sendIntent("RenameFile", undefined, {
      sourcePanel: rename.sourcePanel,
      sourcePath: rename.sourcePath,
      destName: dest,
    });
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter")  { e.preventDefault(); submit(); }
    if (e.key === "Escape") { e.preventDefault(); close(); }
  };

  return (
    <div className="modal-overlay" tabIndex={-1} onClick={close} onKeyDown={(e) => { if (e.key === "Tab") e.preventDefault(); }}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">Rename</div>
        <div className="modal__body">
          <p className="modal__summary">
            Rename <strong>{rename.filename}</strong>
          </p>
          <p className="modal__rename-from" title={rename.dirPath}>
            From: {rename.dirPath}
          </p>
          <div className="modal__rename-row">
            <label className="modal__rename-label">To:</label>
            <input
              ref={inputRef}
              className="modal__rename-input"
              value={value}
              onChange={(e) => { setValue(e.target.value); setLocalError(null); }}
              onKeyDown={handleKeyDown}
              spellCheck={false}
              autoComplete="off"
            />
          </div>
          {(localError ?? rename.error) && (
            <p className="modal__rename-error">{localError ?? rename.error}</p>
          )}
        </div>
        <div className="modal__actions">
          <button
            className="modal__btn modal__btn--primary"
            disabled={submitting}
            onClick={submit}
          >
            {submitting ? "Renaming…" : "Rename"}
          </button>
          <button className="modal__btn" onClick={close}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
