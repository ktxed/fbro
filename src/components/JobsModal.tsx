import { useEffect, useRef, useState } from "react";
import { useAppState } from "../store";
import type { DirSizeTask, FileOpProgress, Job, PrefetchCacheStatus } from "../ws/types";
import type { WsClient } from "../ws/client";
import { IconPlayerPlay, IconPlayerPause, IconX } from "@tabler/icons-react";

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
}

function ProgressBar({ p }: { p: FileOpProgress }) {
  const totalPct = p.totalBytesTotal > 0
    ? Math.round((p.totalBytesCopied / p.totalBytesTotal) * 100)
    : p.overallPercent ?? (
      p.totalFiles > 0
        ? Math.round((p.completedFiles / p.totalFiles) * 100)
        : 0
    );
  const currentLabel = p.currentFile?.split("/").pop() || p.statusText || "⏳";

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
      <div className="jobs-modal__current-file" title={p.currentFile ?? p.statusText}>
        {currentLabel}
      </div>
    </div>
  );
}

function JobRow({ job, client }: { job: Job; client: WsClient }) {
  const isActive = job.state === "Queued" || job.state === "Running" || job.state === "AwaitingDecision";
  const typeLabel = job.type.charAt(0).toUpperCase() + job.type.slice(1);

  const stateLabel: Record<string, string> = {
    Queued: "Queued",
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
            {job.supportsPause && job.paused ? (
              <button
                className="jobs-modal__cancel"
                title="Resume job"
                onClick={() => client.sendIntent("ResumeJob", undefined, { jobId: job.jobId })}
              >
                <IconPlayerPlay size={12} /> Resume
              </button>
            ) : job.supportsPause ? (
              <button
                className="jobs-modal__cancel"
                title="Pause job"
                onClick={() => client.sendIntent("PauseJob", undefined, { jobId: job.jobId })}
              >
                <IconPlayerPause size={12} /> Pause
              </button>
            ) : null}
              <button
                className="jobs-modal__cancel"
                title="Force cancel"
                onClick={() => client.sendIntent("CancelJob", undefined, { jobId: job.jobId })}
              >
                <IconX size={12} /> Cancel
              </button>
          </>
        )}
      </div>

      <div className="jobs-modal__paths">
        {job.kind === "fp-bro" ? (
          <>
            <span className="jobs-modal__path" title={job.sourcePath}>
              Inputs: {job.sourcePath}
            </span>
            {job.resultPath && (
              <span className="jobs-modal__path" title={job.resultPath}>
                Result: {job.resultPath}
              </span>
            )}
          </>
        ) : (
          <>
            <span className="jobs-modal__path" title={job.sourcePath}>
              From: {job.sourcePath}
            </span>
            {job.destPath && (
              <span className="jobs-modal__path" title={job.destPath}>
                To: {job.destPath}
              </span>
            )}
          </>
        )}
      </div>

      {job.progress && isActive && <ProgressBar p={job.progress} />}

      {job.error && (
        <div className="jobs-modal__error">{job.error}</div>
      )}
    </div>
  );
}

function DirSizeTaskRow({ task, client }: { task: DirSizeTask; client: WsClient }) {
  return (
    <div className="jobs-modal__row jobs-modal__row--active">
      <div className="jobs-modal__row-header">
        <span className="jobs-modal__type">Folder size</span>
        <span className="jobs-modal__timestamp">{new Date(task.enqueuedAt).toLocaleTimeString()}</span>
        <span className={`jobs-modal__state jobs-modal__state--${task.status.toLowerCase()}`}>
          {task.status}
        </span>
        <button
          className="jobs-modal__cancel"
          title="Cancel folder size calculation"
          onClick={() => client.sendIntent("CancelSizeCalculation", task.panelId, { name: task.entryName })}
        >
          <IconX size={12} /> Cancel
        </button>
      </div>

      <div className="jobs-modal__paths">
        <span className="jobs-modal__path" title={task.fullPath}>
          Folder: {task.fullPath}
        </span>
        <span className="jobs-modal__path">
          Panel: {task.panelId}
        </span>
      </div>
    </div>
  );
}

function CacheEntryRow({ label, status, client, cacheKey }: {
  label: string;
  status: PrefetchCacheStatus | null;
  client: WsClient;
  cacheKey: string;
}) {
  const usedPct = status && status.maxEntries > 0
    ? Math.round((status.entryCount / status.maxEntries) * 100)
    : 0;

  return (
    <div className="jobs-modal__cache-item">
      <div className="jobs-modal__cache-item-header">
        <span className="jobs-modal__cache-name">{label}</span>
        <button
          className="jobs-modal__cache-clear"
          disabled={!status || status.entryCount === 0}
          onClick={() => client.sendIntent("ClearCache", undefined, { cache: cacheKey })}
        >
          Clear
        </button>
      </div>
      {status ? (
        <>
          <div className="jobs-modal__cache-stats">
            <span>{status.entryCount} / {status.maxEntries} entries</span>
            <span>{formatBytes(status.totalBytes)}</span>
          </div>
          <div className="jobs-modal__bar">
            <div className="jobs-modal__bar-fill" style={{ width: `${usedPct}%` }} />
          </div>
        </>
      ) : (
        <div className="jobs-modal__cache-loading">Loading…</div>
      )}
    </div>
  );
}

export function JobsModal({ onClose, client }: { onClose: () => void; client: WsClient }) {
  const state = useAppState();
  const overlayRef = useRef<HTMLDivElement>(null);
  const [activeTab, setActiveTab] = useState<"jobs" | "cache">("jobs");

  const byNewest = (a: Job, b: Job) => b.startedAt - a.startedAt;
  const jobs = Object.values(state.jobs).sort(byNewest);
  const fileJobs = jobs.filter((job) => job.kind === "file-op");
  const broJobs = jobs.filter((job) => job.kind === "fp-bro");
  const activeFileJobs = fileJobs.filter(
    (job) => job.state === "Queued" || job.state === "Running" || job.state === "AwaitingDecision"
  );
  const finishedFileJobs = fileJobs.filter(
    (job) => job.state !== "Queued" && job.state !== "Running" && job.state !== "AwaitingDecision"
  );
  const dirSizeTasks = [...state.dirSizeQueue].sort((a, b) => a.enqueuedAt - b.enqueuedAt);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape" || e.key === "j") { e.preventDefault(); onClose(); }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  useEffect(() => {
    if (activeTab === "cache") {
      client.sendIntent("QueryCacheStatus", undefined, {});
    }
  }, [activeTab, client]);

  const onOverlayClick = (e: React.MouseEvent) => {
    if (e.target === overlayRef.current) onClose();
  };

  return (
    <div className="modal-overlay" ref={overlayRef} onClick={onOverlayClick}>
      <div className="modal jobs-modal" role="dialog" aria-label="Jobs">
        <div className="modal__header">Jobs</div>

        <div className="jobs-modal__tabs">
          <button
            className={`jobs-modal__tab${activeTab === "jobs" ? " jobs-modal__tab--active" : ""}`}
            onClick={() => setActiveTab("jobs")}
          >
            Jobs
          </button>
          <button
            className={`jobs-modal__tab${activeTab === "cache" ? " jobs-modal__tab--active" : ""}`}
            onClick={() => setActiveTab("cache")}
          >
            Cache
          </button>
        </div>

        <div className="modal__body jobs-modal__body">
          {activeTab === "jobs" && (
            <>
              {fileJobs.length === 0 && broJobs.length === 0 && dirSizeTasks.length === 0 && (
                <div className="jobs-modal__empty">No jobs.</div>
              )}

              {activeFileJobs.length > 0 && (
                <section>
                  <div className="jobs-modal__section-title">Active File Jobs</div>
                  {activeFileJobs.map(j => <JobRow key={j.jobId} job={j} client={client} />)}
                </section>
              )}

              {broJobs.length > 0 && (
                <section>
                  <div className="jobs-modal__section-title">Bro Jobs</div>
                  {broJobs.map(j => <JobRow key={j.jobId} job={j} client={client} />)}
                </section>
              )}

              {dirSizeTasks.length > 0 && (
                <section>
                  <div className="jobs-modal__section-title">Folder Size Queue</div>
                  {dirSizeTasks.map(task => (
                    <DirSizeTaskRow
                      key={`${task.panelId}:${task.entryName}:${task.enqueuedAt}`}
                      task={task}
                      client={client}
                    />
                  ))}
                </section>
              )}

              {finishedFileJobs.length > 0 && (
                <section>
                  <div className="jobs-modal__section-title">Finished File Jobs</div>
                  {finishedFileJobs.map(j => <JobRow key={j.jobId} job={j} client={client} />)}
                </section>
              )}
            </>
          )}

          {activeTab === "cache" && (
            <div className="jobs-modal__cache-body">
              <CacheEntryRow
                label="Viewer prefetch"
                status={state.cacheStatus?.prefetchCache ?? null}
                client={client}
                cacheKey="prefetch"
              />
            </div>
          )}
        </div>

        <div className="modal__actions">
          <button className="modal__btn" onClick={onClose}>Close <kbd>j</kbd></button>
        </div>
      </div>
    </div>
  );
}
