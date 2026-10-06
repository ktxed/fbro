import { useEffect, useRef } from "react";
import { IconCheck, IconClock, IconLoader2, IconPlayerStop, IconPlayerSkipForward, IconX } from "@tabler/icons-react";
import type { HashGenerateBroFile, Job } from "../ws/types";
import type { WsClient } from "../ws/client";
import { HashBroProgressBar } from "./HashBroProgressBar";
import { ModalLayer } from "./ModalStack";
import { ResizableModal } from "./ResizableModal";
import { isNativeActivationTarget } from "./modalKeyboard";

const statusLabel: Record<HashGenerateBroFile["status"], string> = {
  pending: "Pending", hashing: "Hashing…", created: "Created", replaced: "Replaced",
  skipped: "Skipped — already exists", error: "Error", cancelled: "Cancelled",
};

export function HashGenerateBroView({ job, client, onClose }: { job: Job; client: WsClient; onClose: () => void }) {
  const currentRow = useRef<HTMLDivElement>(null);
  const result = job.hashGenerateResult;
  const hashingPath = result?.items.find((item) => item.status === "hashing")?.hashPath;
  const running = job.state === "Running" || job.state === "Queued";
  useEffect(() => { currentRow.current?.scrollIntoView({ block: "nearest" }); }, [hashingPath]);
  const progress = job.progress;
  const filePercent = progress?.currentFileBytesTotal
    ? (progress.currentFileBytesCopied / progress.currentFileBytesTotal) * 100 : progress?.currentFilePercent ?? 0;
  const overallPercent = progress?.totalBytesTotal
    ? (progress.totalBytesCopied / progress.totalBytesTotal) * 100
    : progress?.totalFiles ? (progress.completedFiles / progress.totalFiles) * 100 : 0;
  const count = (status: HashGenerateBroFile["status"]) => result?.items.filter((item) => item.status === status).length ?? 0;
  return (
    <ModalLayer modalId="operation:hash-generate-bro" onKeyDown={(e) => {
      if (e.defaultPrevented) return;
      if (e.key === "Escape" || (e.key === "Enter" && !isNativeActivationTarget(e.target))) {
        e.preventDefault(); onClose();
      }
    }}>
      <ResizableModal modalId="operation:hash-generate-bro" client={client} ariaLabel="Generate hashes" testId="hash-generate-dialog"
        defaultSize={{ width: 640, height: 460 }} minSize={{ width: 480, height: 320 }} className="modal--fixed modal--hash-check">
        <div className="modal__header">Generate hashes — {job.state}</div>
        <div className="modal__body">
          <p className="hash-check__folder" title={job.sourcePath}>{job.sourcePath}</p>
          {result && <p className="hash-check__summary" data-testid="hash-generate-summary">
            {count("created")} created · {count("replaced")} replaced · {count("skipped")} skipped · {count("error")} errors · {result.ignoredCount} ignored
          </p>}
          {running && <>
            {hashingPath && <HashBroProgressBar label="Current file" percent={filePercent} />}
            <HashBroProgressBar label="Overall" percent={overallPercent} />
          </>}
          {!result && <p>Preparing hashes…</p>}
          {result?.items.length === 0 && <p>No eligible files.</p>}
          <div className="hash-check__rows" tabIndex={0} aria-label="Hash generation results">
            {result?.items.map((item) => {
              const Icon = item.status === "created" || item.status === "replaced" ? IconCheck
                : item.status === "error" ? IconX : item.status === "hashing" ? IconLoader2
                : item.status === "cancelled" ? IconPlayerStop : item.status === "skipped" ? IconPlayerSkipForward : IconClock;
              const colorStatus = item.status === "created" || item.status === "replaced" ? "matched"
                : item.status === "hashing" ? "checking" : item.status;
              return (
                <div key={item.hashPath} ref={item.status === "hashing" ? currentRow : undefined}
                  className={`hash-check__row hash-check__row--${colorStatus}`} data-testid="hash-generate-row" data-status={item.status}>
                  <div className="hash-check__row-line">
                    <Icon size={16} aria-hidden />
                    <span className="hash-check__name" title={item.sourcePath}>{item.sourcePath.split(/[/\\]/).pop()}</span>
                    <span className="hash-check__status">{statusLabel[item.status]}</span>
                  </div>
                  {item.error && <p className="hash-check__detail">{item.error}</p>}
                </div>
              );
            })}
          </div>
          {job.error && <p className="modal__rename-error" role="alert">{job.error}</p>}
          {running && <p className="hash-check__hint">Closing keeps generation running. Reopen it or cancel it through Jobs.</p>}
        </div>
        <div className="modal__actions">
          <button className="modal__btn modal__btn--primary" data-modal-initial-focus data-command-id="job.close" onClick={onClose}>Close</button>
        </div>
      </ResizableModal>
    </ModalLayer>
  );
}
