import { useState, useEffect } from "react";
import { useAppDispatch } from "../store";
import type { PendingTouch } from "../store";
import type { WsClient } from "../ws/client";
import { useInputHistory } from "../hooks/useInputHistory";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { ModalLayer } from "./ModalStack";

interface TouchModalProps {
  touch: PendingTouch;
  client: WsClient;
}

export function TouchModal({ touch, client }: TouchModalProps) {
  const dispatch = useAppDispatch();
  const [value, setValue] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const history = useInputHistory("touch", setValue);

  useEffect(() => {
    (history.inputRef as React.RefObject<HTMLInputElement>).current?.focus();
  }, []);

  useEffect(() => {
    if (touch.error) setSubmitting(false);
  }, [touch.error]);

  const close = () => dispatch({ type: "CLOSE_TOUCH" });

  const submit = () => {
    const name = value.trim();
    if (submitting || !name) return;
    setSubmitting(true);
    history.addToHistory(value);
    client.sendIntent("TouchFile", undefined, {
      sourcePanel: touch.sourcePanel,
      dirPath: touch.dirPath,
      name,
    });
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    history.onKeyDown(e);
    if (e.defaultPrevented) return;
    if (e.key === "Enter")  { e.preventDefault(); submit(); }
    if (e.key === "Escape") { e.preventDefault(); close(); }
  };

  return (
    <ModalLayer modalId="touch" onClick={close} onKeyDown={(e) => {
      if (e.key === "Tab") { e.preventDefault(); return; }
      if (e.key === "Enter") { e.preventDefault(); submit(); return; }
      if (e.key === "Escape") { e.preventDefault(); close(); }
    }}>
      <div className="modal" role="dialog" aria-label="New File" data-testid="touch-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">New File</div>
        <div className="modal__body">
          <p className="modal__rename-from" title={touch.dirPath}>
            In: {touch.dirPath}
          </p>
          <div className="modal__rename-row">
            <label className="modal__rename-label">Name:</label>
            <input
              ref={history.inputRef as React.RefObject<HTMLInputElement>}
              className="modal__rename-input"
              aria-label="Name"
              data-testid="touch-name-input"
              value={value}
              onChange={(e) => setValue(e.target.value)}
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
          {touch.error && (
            <p className="modal__rename-error">{touch.error}</p>
          )}
        </div>
        <div className="modal__actions">
          <button
            className="modal__btn modal__btn--primary"
            data-command-id="file.touch.confirm"
            disabled={submitting || !value.trim()}
            onClick={submit}
          >
            {submitting ? "Creating…" : "Create"}
          </button>
          <button className="modal__btn" data-command-id="file.touch.cancel" onClick={close}>
            Cancel
          </button>
        </div>
      </div>
    </ModalLayer>
  );
}
