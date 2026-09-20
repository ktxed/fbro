import { useEffect, useState } from "react";
import { useAppDispatch } from "../store";
import type { PendingGoToPath } from "../store";
import type { WsClient } from "../ws/client";
import { useInputHistory } from "../hooks/useInputHistory";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { ModalLayer } from "./ModalStack";

interface GoToPathModalProps {
  goToPath: PendingGoToPath;
  client: WsClient;
}

export function GoToPathModal({ goToPath, client }: GoToPathModalProps) {
  const dispatch = useAppDispatch();
  const [value, setValue] = useState(goToPath.currentPath);
  const [submitting, setSubmitting] = useState(false);

  const history = useInputHistory("gotoPath", setValue);

  useEffect(() => {
    (history.inputRef as React.RefObject<HTMLInputElement>).current?.focus();
    (history.inputRef as React.RefObject<HTMLInputElement>).current?.select();
  }, []);

  useEffect(() => {
    if (goToPath.error) setSubmitting(false);
  }, [goToPath.error]);

  const close = () => dispatch({ type: "CLOSE_GO_TO_PATH" });

  const submit = () => {
    const path = value.trim();
    if (submitting || !path) return;
    setSubmitting(true);
    history.addToHistory(value);
    client.sendIntent("NavigatePanelTo", undefined, {
      panelId: goToPath.panelId,
      path,
    });
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    history.onKeyDown(e);
    if (e.defaultPrevented) return;
    if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      submit();
      return;
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    }
  };

  return (
    <ModalLayer modalId="go-to-path" onClick={close} onKeyDown={(e) => {
      if (e.defaultPrevented) return;
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
              ref={history.inputRef as React.RefObject<HTMLInputElement>}
              className="modal__rename-input"
              data-testid="goto-path-input"
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={handleKeyDown}
              spellCheck={false}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
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
    </ModalLayer>
  );
}
