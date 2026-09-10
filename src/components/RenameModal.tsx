import { useState, useEffect } from "react";
import { useAppDispatch } from "../store";
import type { PendingRename } from "../store";
import type { WsClient } from "../ws/client";
import { useInputHistory } from "../hooks/useInputHistory";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { ModalLayer } from "./ModalStack";

interface RenameModalProps {
  rename: PendingRename;
  client: WsClient;
}

export function RenameModal({ rename, client }: RenameModalProps) {
  const dispatch = useAppDispatch();
  const [value, setValue] = useState(rename.filename);
  const [localError, setLocalError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const history = useInputHistory("rename", setValue);

  useEffect(() => {
    (history.inputRef as React.RefObject<HTMLInputElement>).current?.focus();
    (history.inputRef as React.RefObject<HTMLInputElement>).current?.select();
  }, []);

  useEffect(() => {
    if (rename.error) setSubmitting(false);
  }, [rename.error]);

  const close = () => dispatch({ type: "CLOSE_RENAME" });

  const submit = () => {
    const dest = value.trim();
    if (submitting || !dest || dest === rename.filename) { close(); return; }
    if (dest.includes("/") || dest.includes("\\")) {
      setLocalError("Rename cannot change directories — use Move for that.");
      return;
    }
    setLocalError(null);
    setSubmitting(true);
    history.addToHistory(value);
    client.sendIntent("RenameFile", undefined, {
      sourcePanel: rename.sourcePanel,
      sourcePath: rename.sourcePath,
      destName: dest,
    });
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    history.onKeyDown(e);
    if (e.defaultPrevented) return;
    if (e.key === "Enter")  { e.preventDefault(); submit(); }
    if (e.key === "Escape") { e.preventDefault(); close(); }
  };

  return (
    <ModalLayer modalId="rename" onClick={close} onKeyDown={(e) => {
      if (e.key === "Tab") { e.preventDefault(); return; }
      if (e.key === "Enter") { e.preventDefault(); submit(); return; }
      if (e.key === "Escape") { e.preventDefault(); close(); }
    }}>
      <div className="modal" role="dialog" aria-label="Rename" data-testid="rename-dialog" onClick={(e) => e.stopPropagation()}>
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
              ref={history.inputRef as React.RefObject<HTMLInputElement>}
              className="modal__rename-input"
              aria-label="To"
              data-testid="rename-to-input"
              value={value}
              onChange={(e) => { setValue(e.target.value); setLocalError(null); }}
              onKeyDown={handleKeyDown}
              spellCheck={false}
              autoComplete="off"
            />
            {history.isOpen && (
              <InputHistoryDropdown
                anchorEl={history.inputRef.current}
                items={history.items}
                activeIndex={history.activeIndex}
                onSelect={history.selectItem}
                onClose={history.close}
              />
            )}
          </div>
          {(localError ?? rename.error) && (
            <p className="modal__rename-error">{localError ?? rename.error}</p>
          )}
        </div>
        <div className="modal__actions">
          <button
            className="modal__btn modal__btn--primary"
            data-command-id="file.rename.confirm"
            disabled={submitting}
            onClick={submit}
          >
            {submitting ? "Renaming…" : "Rename"}
          </button>
          <button className="modal__btn" data-command-id="file.rename.cancel" onClick={close}>
            Cancel
          </button>
        </div>
      </div>
    </ModalLayer>
  );
}
