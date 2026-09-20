import { useState, useEffect } from "react";
import { useAppDispatch } from "../store";
import type { PendingInput } from "../store";
import type { WsClient } from "../ws/client";
import { useInputHistory } from "../hooks/useInputHistory";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { ModalLayer } from "./ModalStack";

interface InputModalProps {
  input: PendingInput;
  client: WsClient;
}

export function InputModal({ input, client }: InputModalProps) {
  const dispatch = useAppDispatch();
  const [value, setValue] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const history = useInputHistory("input", setValue);

  useEffect(() => {
    (history.inputRef as React.RefObject<HTMLInputElement>).current?.focus();
  }, []);

  const close = () => dispatch({ type: "CLOSE_INPUT" });

  const submit = () => {
    const trimmed = value.trim();
    if (submitting || !trimmed) return;
    setSubmitting(true);
    if (input.kind !== "password") history.addToHistory(value);
    client.sendIntent("ProvideInput", undefined, {
      requestId: input.requestId,
      value: trimmed,
    });
    close();
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    if (input.kind !== "password") history.onKeyDown(e);
    if (e.defaultPrevented) return;
    if (e.key === "Enter")  { e.preventDefault(); e.stopPropagation(); submit(); }
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); }
  };

  return (
    <ModalLayer modalId="input" onClick={close} onKeyDown={(e) => {
      if (e.defaultPrevented) return;
      if (e.key === "Escape") { e.preventDefault(); close(); }
    }}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">{input.prompt}</div>
        <div className="modal__body">
          <div className="modal__rename-row">
            <label className="modal__rename-label">
              {input.kind === "password" ? "Password:" : "Value:"}
            </label>
            <input
              ref={history.inputRef as React.RefObject<HTMLInputElement>}
              className="modal__rename-input"
              type={input.kind === "password" ? "password" : "text"}
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={handleKeyDown}
              spellCheck={false}
              autoComplete="off"
            />
            {history.isOpen && input.kind !== "password" && (
              <InputHistoryDropdown
                anchorEl={history.inputRef.current}
                items={history.items}
                activeIndex={history.activeIndex}
                onSelect={history.selectItem}
                onClose={history.close}
              />
            )}
          </div>
        </div>
        <div className="modal__actions">
          <button
            className="modal__btn modal__btn--primary"
            disabled={submitting || !value.trim()}
            onClick={submit}
          >
            {submitting ? "Submitting..." : "OK"}
          </button>
          <button className="modal__btn" onClick={close}>
            Cancel
          </button>
        </div>
      </div>
    </ModalLayer>
  );
}
