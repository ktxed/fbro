import { useState, useEffect } from "react";
import { useAppDispatch } from "../store";
import type { PendingMkdir } from "../store";
import type { WsClient } from "../ws/client";
import { useInputHistory } from "../hooks/useInputHistory";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { ModalLayer } from "./ModalStack";

interface MkDirModalProps {
  mkdir: PendingMkdir;
  client: WsClient;
}

export function MkDirModal({ mkdir, client }: MkDirModalProps) {
  const dispatch = useAppDispatch();
  const [value, setValue] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const history = useInputHistory("mkdir", setValue);

  useEffect(() => {
    (history.inputRef as React.RefObject<HTMLInputElement>).current?.focus();
  }, []);

  useEffect(() => {
    if (mkdir.error) setSubmitting(false);
  }, [mkdir.error]);

  const close = () => dispatch({ type: "CLOSE_MKDIR" });

  const submit = () => {
    const name = value.trim();
    if (submitting || !name) return;
    setSubmitting(true);
    history.addToHistory(value);
    client.sendIntent("MakeDir", undefined, {
      sourcePanel: mkdir.sourcePanel,
      dirPath: mkdir.dirPath,
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
    <ModalLayer modalId="mkdir" onClick={close} onKeyDown={(e) => {
      if (e.key === "Tab") { e.preventDefault(); return; }
      if (e.key === "Enter") { e.preventDefault(); submit(); return; }
      if (e.key === "Escape") { e.preventDefault(); close(); }
    }}>
      <div className="modal" role="dialog" aria-label="Create Directory" data-testid="mkdir-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">Create Directory</div>
        <div className="modal__body">
          <p className="modal__rename-from" title={mkdir.dirPath}>
            In: {mkdir.dirPath}
          </p>
          <div className="modal__rename-row">
            <label className="modal__rename-label">Name:</label>
            <input
              ref={history.inputRef as React.RefObject<HTMLInputElement>}
              className="modal__rename-input"
              aria-label="Name"
              data-testid="mkdir-name-input"
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
          {mkdir.error && (
            <p className="modal__rename-error">{mkdir.error}</p>
          )}
        </div>
        <div className="modal__actions">
          <button
            className="modal__btn modal__btn--primary"
            data-command-id="file.mkdir.confirm"
            disabled={submitting || !value.trim()}
            onClick={submit}
          >
            {submitting ? "Creating…" : "Create"}
          </button>
          <button className="modal__btn" data-command-id="file.mkdir.cancel" onClick={close}>
            Cancel
          </button>
        </div>
      </div>
    </ModalLayer>
  );
}
