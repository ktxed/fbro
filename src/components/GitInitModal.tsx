import { useRef, useState } from "react";
import { useAppDispatch, type PendingGitInit } from "../store";
import type { WsClient } from "../ws/client";
import { useInputHistory } from "../hooks/useInputHistory";
import { InputHistoryDropdown } from "./InputHistoryDropdown";
import { ModalLayer } from "./ModalStack";
import { ModalDialog } from "./ModalDialog";
import { isNativeActivationTarget } from "./modalKeyboard";

export function GitInitModal({ gitInit, client }: { gitInit: PendingGitInit; client: WsClient }) {
  const dispatch = useAppDispatch();
  const [branch, setBranch] = useState("main");
  const history = useInputHistory("git-init-branch", setBranch);
  // Guard within the same render too: repeated Enter/click events must not
  // submit again before the store's submitting update is rendered.
  const submittedRequest = useRef<string | null>(null);
  if (!gitInit.submitting) submittedRequest.current = null;
  const close = () => dispatch({ type: "CLOSE_GIT_INIT" });
  const submit = () => {
    if (gitInit.submitting || submittedRequest.current || !branch.trim()) return;
    const requestId = globalThis.crypto?.randomUUID?.() ?? `git-init-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    submittedRequest.current = requestId;
    dispatch({ type: "SUBMIT_GIT_INIT", requestId });
    history.addToHistory(branch);
    client.sendIntent("InitGitRepository", undefined, {
      panelId: gitInit.panelId,
      sourceId: gitInit.sourceId,
      targetPath: gitInit.targetPath,
      branch: branch.trim(),
      requestId,
    });
  };
  const onKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    if (e.defaultPrevented) return;
    if (e.key === "Escape") {
      e.preventDefault();
      close();
    } else if (e.key === "Enter" && !isNativeActivationTarget(e.target)) {
      e.preventDefault();
      submit();
    }
  };
  return (
    <ModalLayer modalId="git-init" initialFocusRef={history.inputRef} onClick={close} onKeyDown={onKeyDown}>
      <ModalDialog ariaLabel="Initialize Git repository" testId="git-init-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="modal__header">Initialize Git repository</div>
        <div className="modal__body">
          <p className="modal__rename-from" title={gitInit.targetPath} data-testid="git-init-target">In: {gitInit.targetPath}</p>
          <div className="modal__rename-row">
            <label className="modal__rename-label" htmlFor="git-init-branch">Initial branch:</label>
            <input
              id="git-init-branch"
              ref={history.inputRef as React.RefObject<HTMLInputElement>}
              className="modal__rename-input"
              data-testid="git-init-branch-input"
              value={branch}
              disabled={gitInit.submitting}
              onChange={(e) => setBranch(e.target.value)}
              onKeyDown={(e) => {
                history.onKeyDown(e);
                if (e.defaultPrevented) return;
                if (e.key === "Enter") { e.preventDefault(); e.stopPropagation(); submit(); }
              }}
              spellCheck={false}
              autoComplete="off"
            />
            {history.isOpen && (
              <InputHistoryDropdown anchorEl={history.inputRef.current} items={history.items}
                activeIndex={history.activeIndex} onSelect={history.selectItem} onClose={history.close} />
            )}
          </div>
          {gitInit.error && <p className="modal__rename-error" role="alert">{gitInit.error}</p>}
        </div>
        <div className="modal__actions">
          <button className="modal__btn modal__btn--primary" data-command-id="git.init.confirm"
            disabled={gitInit.submitting || !branch.trim()} onClick={submit}>
            {gitInit.submitting ? "Initializing…" : "Initialize"}
          </button>
          <button className="modal__btn" data-command-id="git.init.cancel" onClick={close}>
            {gitInit.submitting ? "Close" : "Cancel"}
          </button>
        </div>
      </ModalDialog>
    </ModalLayer>
  );
}
