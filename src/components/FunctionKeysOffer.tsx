import { useEffect, useRef, useState } from "react";
import { useAppState } from "../store";
import type { WsClient } from "../ws/client";
import { ModalLayer } from "./ModalStack";
import { ModalDialog } from "./ModalDialog";

/**
 * One-time macOS offer: the system sends F1, F2… without Fn, but the
 * button-bar commands still default to number keys. Either answer is
 * persisted by the backend; Escape only postpones it to the next launch.
 */
export function FunctionKeysOffer({ client }: { client: WsClient }) {
  const { connected, settings: { general } } = useAppState();
  const [postponed, setPostponed] = useState(false);
  const [submitting, setSubmitting] = useState<"function" | "numbers" | null>(null);
  const acceptRef = useRef<HTMLButtonElement>(null);

  const offered = general.systemFunctionKeysStandard === true
    && general.buttonBarKeys === "numbers"
    && !general.functionKeysOfferAnswered;

  // The answer lands as a settings patch; a lost connection re-enables the buttons.
  useEffect(() => { if (!connected) setSubmitting(null); }, [connected]);

  if (!offered || postponed || !connected) return null;

  const answer = (choice: "function" | "numbers") => {
    if (submitting) return;
    setSubmitting(choice);
    if (choice === "function") client.sendIntent("SetGeneralOption", undefined, { key: "buttonBarKeys", value: "function" });
    else client.sendIntent("SetGeneralOption", undefined, { key: "functionKeysOfferAnswered", value: true });
  };

  return (
    <ModalLayer
      modalId="function-keys-offer"
      initialFocusRef={acceptRef}
      onKeyDown={(e) => {
        if (e.defaultPrevented) return;
        if (e.key === "Escape") {
          e.preventDefault();
          setPostponed(true);
        }
      }}
    >
      <ModalDialog ariaLabel="Use function keys" testId="function-keys-offer">
        <div className="modal__header">Use function keys?</div>
        <div className="modal__body">
          <p className="modal__summary">
            This Mac sends F1, F2, … without holding Fn. fbro can use F1–F10 for View, Edit, Copy,
            Move and the other button bar commands instead of the number keys 1–0.
          </p>
          <p className="modal__summary">
            You can change this later in Settings → Shortcuts. Customized shortcuts keep their keys.
          </p>
        </div>
        <div className="modal__actions">
          <button
            ref={acceptRef}
            className="modal__btn modal__btn--primary"
            data-testid="function-keys-offer-accept"
            disabled={submitting !== null}
            onClick={() => answer("function")}
          >
            {submitting === "function" ? "Switching…" : "Use function keys"}
          </button>
          <button
            className="modal__btn"
            data-testid="function-keys-offer-decline"
            disabled={submitting !== null}
            onClick={() => answer("numbers")}
          >
            {submitting === "numbers" ? "Saving…" : "Keep number keys"}
          </button>
        </div>
      </ModalDialog>
    </ModalLayer>
  );
}
