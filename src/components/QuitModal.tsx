import { useRef, useEffect } from "react";
import { useAppDispatch, useAppState } from "../store";
import type { WsClient } from "../ws/client";

interface QuitModalProps {
  client: WsClient;
}

export function QuitModal({ client }: QuitModalProps) {
  const dispatch = useAppDispatch();
  const state = useAppState();
  const overlayRef = useRef<HTMLDivElement>(null);

  const activeJobs = Object.values(state.jobs).filter(
    j => j.state === "Running" || j.state === "AwaitingDecision"
  ).length;

  useEffect(() => {
    overlayRef.current?.focus();
  }, []);

  const confirm = () => client.sendIntent("ConfirmQuit");
  const cancel = () => dispatch({ type: "CANCEL_QUIT" });

  return (
    <div
      className="modal-overlay"
      ref={overlayRef}
      tabIndex={-1}
      onKeyDown={(e) => {
        if (e.key === "Tab") e.preventDefault();
        if (e.key === "Escape") cancel();
        if (e.key === "Enter") confirm();
      }}
    >
      <div className="modal">
        <div className="modal__header">Quit</div>
        <div className="modal__body">
          {activeJobs > 0 ? (
            <p className="modal__summary">
              There {activeJobs === 1 ? "is" : "are"}{" "}
              <strong>{activeJobs}</strong> job{activeJobs === 1 ? "" : "s"} on-going.
              Are you sure you want to cancel {activeJobs === 1 ? "it" : "them"} and exit?
            </p>
          ) : (
            <p className="modal__summary">Are you sure you want to quit?</p>
          )}
          <div className="modal__actions">
            <button className="modal__btn modal__btn--primary" onClick={confirm}>
              Yes, quit
            </button>
            <button className="modal__btn" onClick={cancel}>
              Cancel
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
