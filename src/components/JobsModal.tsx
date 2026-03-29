import { useEffect, useRef } from "react";
import { useAppState } from "../store";
import type { Job, FileOpProgress } from "../ws/types";
import type { WsClient } from "../ws/client";

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
}

function ProgressBar({ p }: { p: FileOpProgress }) {
  const totalPct = p.totalBytesTotal > 0
    ? Math.round((p.totalBytesCopied / p.totalBytesTotal) * 100)
    : p.totalFiles > 0
    ? Math.round((p.completedFiles / p.totalFiles) * 100)
    : 0;

  return (
    <div className="jobs-modal__progress">
      <div className="jobs-modal__progress-row">
        <span>Files: {p.completedFiles} / {p.totalFiles}</span>
        {p.totalBytesTotal > 0 && (
          <span>{formatBytes(p.totalBytesCopied)} / {formatBytes(p.totalBytesTotal)}</span>
        )}
        <span>{totalPct}%</span>
      </div>
      <div className="jobs-modal__bar">
        <div className="jobs-modal__bar-fill" style={{ width: `${totalPct}%` }} />
      </div>
      <div className="jobs-modal__current-file" title={p.currentFile}>
        {p.currentFile?.split("/").pop() || "⏳"}
      </div>
    </div>
  );
}

function JobRow({ job, client }: { job: Job; client: WsClient }) {
  const isActive = job.state === "Running" || job.state === "AwaitingDecision";
  const typeLabel = job.type.charAt(0).toUpperCase() + job.type.slice(1);

  const stateLabel: Record<string, string> = {
    Running: "Running",
    AwaitingDecision: "Waiting",
    Completed: "Done",
    Cancelled: "Cancelled",
    Failed: "Failed",
  };

  return (
    <div className={`jobs-modal__row${isActive ? " jobs-modal__row--active" : ""}`}>
      <div className="jobs-modal__row-header">
        <span className="jobs-modal__type">{typeLabel}</span>
        <span className="jobs-modal__timestamp">{new Date(job.startedAt).toLocaleTimeString()}</span>
        <span className={`jobs-modal__state jobs-modal__state--${job.state.toLowerCase()}${job.paused ? " jobs-modal__state--paused" : ""}`}>
          {job.paused ? "Paused" : (stateLabel[job.state] ?? job.state)}
        </span>
        {isActive && (
          <>
            {job.paused ? (
              <button
                className="jobs-modal__cancel"
                title="Resume job"
                onClick={() => client.sendIntent("ResumeJob", undefined, { jobId: job.jobId })}
              >
                ▶ Resume
              </button>
            ) : (
              <button
                className="jobs-modal__cancel"
                title="Pause job"
                onClick={() => client.sendIntent("PauseJob", undefined, { jobId: job.jobId })}
              >
                ⏸ Pause
              </button>
            )}
            <button
              className="jobs-modal__cancel"
              title="Force cancel"
              onClick={() => client.sendIntent("CancelJob", undefined, { jobId: job.jobId })}
            >
              ✕ Cancel
            </button>
          </>
        )}
      </div>

      <div className="jobs-modal__paths">
        <span className="jobs-modal__path" title={job.sourcePath}>
          From: {job.sourcePath}
        </span>
        {job.destPath && (
          <span className="jobs-modal__path" title={job.destPath}>
            To: {job.destPath}
          </span>
        )}
      </div>

      {job.progress && isActive && <ProgressBar p={job.progress} />}

      {job.error && (
        <div className="jobs-modal__error">{job.error}</div>
      )}
    </div>
  );
}

export function JobsModal({ onClose, client }: { onClose: () => void; client: WsClient }) {
  const state = useAppState();
  const overlayRef = useRef<HTMLDivElement>(null);

  const byNewest = (a: Job, b: Job) => b.startedAt - a.startedAt;
  const jobs = Object.values(state.jobs).sort(byNewest);
  const active = jobs.filter(j => j.state === "Running" || j.state === "AwaitingDecision");
  const finished = jobs.filter(j => j.state !== "Running" && j.state !== "AwaitingDecision");

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape" || e.key === "j") { e.preventDefault(); onClose(); }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  const onOverlayClick = (e: React.MouseEvent) => {
    if (e.target === overlayRef.current) onClose();
  };

  return (
    <div className="modal-overlay" ref={overlayRef} onClick={onOverlayClick}>
      <div className="modal jobs-modal" role="dialog" aria-label="Jobs">
        <div className="modal__header">
          <span>Jobs</span>
          <button className="jobs-modal__close" onClick={onClose}>✕</button>
        </div>

        <div className="modal__body jobs-modal__body">
          {jobs.length === 0 && (
            <div className="jobs-modal__empty">No jobs.</div>
          )}

          {active.length > 0 && (
            <section>
              <div className="jobs-modal__section-title">Active</div>
              {active.map(j => <JobRow key={j.jobId} job={j} client={client} />)}
            </section>
          )}

          {finished.length > 0 && (
            <section>
              <div className="jobs-modal__section-title">Finished</div>
              {finished.map(j => <JobRow key={j.jobId} job={j} client={client} />)}
            </section>
          )}
        </div>

        <div className="modal__actions">
          <button className="modal__btn" onClick={onClose}>Close <kbd>j</kbd></button>
        </div>
      </div>
    </div>
  );
}
