import { useEffect, useRef, useState } from "react";
import { useAppDispatch } from "../store";
import type { PendingGoToPath } from "../store";
import type { WsClient } from "../ws/client";

interface GoToPathModalProps {
  goToPath: PendingGoToPath;
  client: WsClient;
}

export function GoToPathModal({ goToPath, client }: GoToPathModalProps) {
  const dispatch = useAppDispatch();
  const [value, setValue] = useState(goToPath.currentPath);
  const [submitting, setSubmitting] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  useEffect(() => {
    if (goToPath.error) setSubmitting(false);
  }, [goToPath.error]);

  const close = () => dispatch({ type: "CLOSE_GO_TO_PATH" });

  const submit = () => {
    const path = value.trim();
    if (submitting || !path) return;
    setSubmitting(true);
    client.sendIntent("NavigatePanelTo", undefined, {
      panelId: goToPath.panelId,
      path,
    });
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      submit();
    }
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    }
  };

  return (
    <div className="modal-overlay" tabIndex={-1} onClick={close} onKeyDown={(e) => {
      if (e.key === "Tab") { e.preventDefault(); return; }
      if (e.key === "Enter") { e.preventDefault(); submit(); return; }
      if (e.key === "Escape") { e.preventDefault(); close(); }
    }}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">Go To Path</div>
        <div className="modal__body">
          <p className="modal__rename-from" title={goToPath.currentPath}>
            Current: {goToPath.currentPath}
          </p>
          <div className="modal__rename-row">
            <label className="modal__rename-label">Path:</label>
            <input
              ref={inputRef}
              className="modal__rename-input"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={handleKeyDown}
              spellCheck={false}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
            />
          </div>
          {goToPath.error && (
            <p className="modal__rename-error">{goToPath.error}</p>
          )}
        </div>
        <div className="modal__actions">
          <button
            className="modal__btn modal__btn--primary"
            disabled={submitting || !value.trim()}
            onClick={submit}
          >
            {submitting ? "Navigating…" : "Go To"}
          </button>
          <button className="modal__btn" onClick={close}>
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
