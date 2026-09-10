import { IconCopy } from "@tabler/icons-react";
import { useEffect, useMemo, useRef, useState } from "react";

import { useAppDispatch } from "../store";
import type { WsClient } from "../ws/client";
import { ResizableModal } from "./ResizableModal";
import { ModalLayer } from "./ModalStack";

function diagnosticText(info: Readonly<FbroBuildInfo>): string {
  const dirty = info.dirty === null ? "unknown" : info.dirty ? "dirty" : "clean";
  return [
    `fbro ${info.version}`,
    `Commit: ${info.commit}`,
    `Short commit: ${info.shortCommit}`,
    `Branch: ${info.branch ?? "detached or unavailable"}`,
    `Tag: ${info.tag ?? "none"}`,
    `Working tree: ${dirty}`,
    `Built at: ${info.builtAt}`,
    `Profile: ${info.profile}`,
  ].join("\n");
}

export function AboutModal({ client }: { client: WsClient }) {
  const dispatch = useAppDispatch();
  const closeRef = useRef<HTMLButtonElement>(null);
  const copyResetRef = useRef<number | null>(null);
  const [copied, setCopied] = useState(false);
  const info = __FBRO_BUILD_INFO__;
  const diagnostics = useMemo(() => diagnosticText(info), [info]);
  const dirtyLabel = info.dirty === null ? "Unknown" : info.dirty ? "Dirty" : "Clean";

  useEffect(() => {
    closeRef.current?.focus();
    return () => {
      if (copyResetRef.current !== null) window.clearTimeout(copyResetRef.current);
    };
  }, []);

  const close = () => dispatch({ type: "CLOSE_ABOUT" });
  const copyDiagnostics = () => {
    void navigator.clipboard.writeText(diagnostics).then(() => {
      setCopied(true);
      if (copyResetRef.current !== null) window.clearTimeout(copyResetRef.current);
      copyResetRef.current = window.setTimeout(() => setCopied(false), 1500);
    }).catch(() => setCopied(false));
  };

  return (
    <ModalLayer
      modalId="about"
      onClick={close}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          close();
        }
      }}
    >
      <ResizableModal
        modalId="about"
        client={client}
        className="about-modal"
        ariaModal
        ariaLabel="About fbro"
        testId="about-modal"
        defaultSize={{ width: 580, height: 440 }}
        minSize={{ width: 440, height: 340 }}
      >
        <div className="modal__header">About fbro</div>
        <div className="modal__body about-modal__body">
          <div className="about-modal__product">
            <strong>fbro</strong>
            <span>orthodox file manager</span>
          </div>
          <table className="modal__info-table about-modal__table">
            <tbody>
              <tr>
                <td className="modal__info-label">Version</td>
                <td className="modal__info-value" data-testid="about-version">{info.version}</td>
              </tr>
              <tr>
                <td className="modal__info-label">Commit</td>
                <td className="modal__info-value about-modal__hash" data-testid="about-commit">{info.commit}</td>
              </tr>
              <tr>
                <td className="modal__info-label">Short commit</td>
                <td className="modal__info-value" data-testid="about-short-commit">{info.shortCommit}</td>
              </tr>
              <tr>
                <td className="modal__info-label">Branch</td>
                <td className="modal__info-value" data-testid="about-branch">{info.branch ?? "Detached or unavailable"}</td>
              </tr>
              <tr>
                <td className="modal__info-label">Exact tag</td>
                <td className="modal__info-value" data-testid="about-tag">{info.tag ?? "—"}</td>
              </tr>
              <tr>
                <td className="modal__info-label">Working tree</td>
                <td
                  className={`modal__info-value about-modal__status about-modal__status--${info.dirty === null ? "unknown" : info.dirty ? "dirty" : "clean"}`}
                  data-testid="about-dirty"
                >
                  {dirtyLabel}
                </td>
              </tr>
              <tr>
                <td className="modal__info-label">Built at</td>
                <td className="modal__info-value" data-testid="about-built-at">{info.builtAt}</td>
              </tr>
              <tr>
                <td className="modal__info-label">Profile</td>
                <td className="modal__info-value" data-testid="about-profile">{info.profile}</td>
              </tr>
            </tbody>
          </table>
        </div>
        <div className="modal__actions">
          <button className="modal__btn about-modal__copy" type="button" onClick={copyDiagnostics}>
            <IconCopy size={14} aria-hidden />
            {copied ? "Copied" : "Copy build info"}
          </button>
          <button ref={closeRef} className="modal__btn modal__btn--primary" type="button" onClick={close}>
            Close
          </button>
        </div>
      </ResizableModal>
    </ModalLayer>
  );
}
