import { useState, useRef, useEffect } from "react";
import { useAppDispatch } from "../store";
import type { PendingInput } from "../store";
import type { WsClient } from "../ws/client";

interface InputModalProps {
  input: PendingInput;
  client: WsClient;
}

export function InputModal({ input, client }: InputModalProps) {
  const dispatch = useAppDispatch();
  const [value, setValue] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const close = () => dispatch({ type: "CLOSE_INPUT" });

  const submit = () => {
    const trimmed = value.trim();
    if (submitting || !trimmed) return;
    setSubmitting(true);
    client.sendIntent("ProvideInput", undefined, {
      requestId: input.requestId,
      value: trimmed,
    });
    close();
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter")  { e.preventDefault(); submit(); }
    if (e.key === "Escape") { e.preventDefault(); close(); }
  };

  return (
    <div className="modal-overlay" tabIndex={-1} onClick={close} onKeyDown={(e) => {
      if (e.key === "Tab") { e.preventDefault(); return; }
      if (e.key === "Enter") { e.preventDefault(); submit(); return; }
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
              ref={inputRef}
              className="modal__rename-input"
              type={input.kind === "password" ? "password" : "text"}
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={handleKeyDown}
              spellCheck={false}
              autoComplete="off"
            />
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
    </div>
  );
}
