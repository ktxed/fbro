import { useEffect, useRef } from "react";
import { HashBroProgressBar } from "./HashBroProgressBar";
import { IconCheck, IconClock, IconLoader2, IconPlayerStop, IconX } from "@tabler/icons-react";
import type { HashCheckBroFile, Job } from "../ws/types";
import type { WsClient } from "../ws/client";
import { ModalLayer } from "./ModalStack";
import { ResizableModal } from "./ResizableModal";
import { isNativeActivationTarget } from "./modalKeyboard";

const statusLabel: Record<HashCheckBroFile["status"], string> = {
  pending: "Pending", checking: "Checking…", matched: "Matches",
  mismatched: "Mismatch", error: "Error", cancelled: "Cancelled",
};


export function HashCheckBroView({ job, client, onClose }: { job: Job; client: WsClient; onClose: () => void }) {
  const currentRow = useRef<HTMLDivElement>(null);
  const result = job.hashCheckResult;
  const checkingPath = result?.items.find((item) => item.status === "checking")?.hashPath;
  const running = job.state === "Running" || job.state === "Queued";
  useEffect(() => {
    currentRow.current?.scrollIntoView({ block: "nearest" });
  }, [checkingPath]);
  const progress = job.progress;
  const filePercent = progress?.currentFileBytesTotal
    ? (progress.currentFileBytesCopied / progress.currentFileBytesTotal) * 100
    : progress?.currentFilePercent ?? 0;
  const overallPercent = progress?.totalBytesTotal
    ? (progress.totalBytesCopied / progress.totalBytesTotal) * 100
    : progress?.totalFiles ? (progress.completedFiles / progress.totalFiles) * 100 : 0;
  const count = (status: HashCheckBroFile["status"]) => result?.items.filter((item) => item.status === status).length ?? 0;

  return (
    <ModalLayer modalId="operation:hash-check-bro" onKeyDown={(e) => {
      if (e.defaultPrevented) return;
      if (e.key === "Escape" || (e.key === "Enter" && !isNativeActivationTarget(e.target))) {
        e.preventDefault();
        onClose();
      }
    }}>
      <ResizableModal modalId="operation:hash-check-bro" client={client} ariaLabel="Check hashes" testId="hash-check-dialog"
        defaultSize={{ width: 640, height: 460 }} minSize={{ width: 480, height: 320 }} className="modal--fixed modal--hash-check">
        <div className="modal__header">Check hashes — {job.state}</div>
        <div className="modal__body">
          <p className="hash-check__folder" title={job.sourcePath}>{job.sourcePath}</p>
          {result && <p className="hash-check__summary" data-testid="hash-check-summary">
            {count("matched")} matches · {count("mismatched")} mismatches · {count("error")} errors · {result.ignoredCount} ignored
          </p>}
          {running && <>
            {checkingPath && <HashBroProgressBar label="Current file" percent={filePercent} />}
            <HashBroProgressBar label="Overall" percent={overallPercent} />
          </>}
          {!result && <p>Preparing checks…</p>}
          {result?.items.length === 0 && <p data-testid="hash-check-empty">No matching file pairs.</p>}
          <div className="hash-check__rows" data-testid="hash-check-rows" tabIndex={0} aria-label="Hash check results">
            {result?.items.map((item) => {
              const Icon = item.status === "matched" ? IconCheck
                : item.status === "mismatched" || item.status === "error" ? IconX
                : item.status === "checking" ? IconLoader2
                : item.status === "cancelled" ? IconPlayerStop : IconClock;
              return (
                <div key={item.hashPath} ref={item.status === "checking" ? currentRow : undefined}
                  className={`hash-check__row hash-check__row--${item.status}`} data-testid="hash-check-row" data-status={item.status}>
                  <div className="hash-check__row-line">
                    <Icon size={16} aria-hidden />
                    <span className="hash-check__name" title={item.counterpartPath}>{item.counterpartPath.split(/[/\\]/).pop()}</span>
                    <span className="hash-check__status">{statusLabel[item.status]}</span>
                  </div>
                  {item.error && <p className="hash-check__detail">{item.error}</p>}
                  {item.status === "mismatched" && <p className="hash-check__detail">
                    Expected: {item.expectedHash}<br />Actual: {item.actualHash}
                  </p>}
                </div>
              );
            })}
          </div>
          {job.error && <p className="modal__rename-error" role="alert">{job.error}</p>}
          {job.hashCheckReportError && <p className="modal__rename-error" role="alert">{job.hashCheckReportError}</p>}
          {job.reportSavedPath && <p className="hash-check__saved" data-testid="hash-check-saved">Saved: {job.reportSavedPath}</p>}
          {running && <p className="hash-check__hint">Closing keeps the check running. Reopen it or cancel it through Jobs.</p>}
        </div>
        <div className="modal__actions">
          <button className="modal__btn modal__btn--primary" data-modal-initial-focus data-command-id="job.close" onClick={onClose}>Close</button>
        </div>
      </ResizableModal>
    </ModalLayer>
  );
}
