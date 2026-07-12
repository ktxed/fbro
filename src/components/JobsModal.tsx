import { useEffect, useRef, useState, type ReactNode } from "react";
import { useAppDispatch, useAppState } from "../store";
import type { ChildProcess, DirSizeTask, FileOpProgress, Job, PrefetchCacheStatus } from "../ws/types";
import type { WsClient } from "../ws/client";
import { IconEye, IconPlayerPlay, IconPlayerPause, IconX } from "@tabler/icons-react";
import { cancelJobWithPartialPrompt } from "../utils/jobCancel";
import { ResizableModal } from "./ResizableModal";

const FILE_LIST_PAGE_SIZE = 25;

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
}

function fileSizeLabel(size?: number) {
  return size === undefined || size === null ? "—" : formatBytes(size);
}

function jobShortId(jobId: string) {
  return jobId.replace(/^job-/, "#");
}

function isTerminalJob(job: Job) {
  return job.state === "Completed" || job.state === "Cancelled" || job.state === "Failed";
}

function completedFileCount(job: Job) {
  return job.progress?.completedFiles || job.progress?.totalFiles || job.files.length;
}

function listedFileCount(job: Job) {
  return job.files.length || job.progress?.totalFiles || completedFileCount(job);
}

function completedBytes(job: Job) {
  return job.progress?.totalBytesCopied || job.progress?.totalBytesTotal || job.files.reduce((sum, file) => sum + (file.size ?? 0), 0);
}

function SummaryChip({
  label,
  value,
  onClick,
  active = false,
}: {
  label: string;
  value: string;
  onClick?: () => void;
  active?: boolean;
}) {
  const content = (
    <>
      <span className="jobs-modal__summary-label">{label}</span>
      <span className="jobs-modal__summary-value">{value}</span>
    </>
  );
  if (onClick) {
    return (
      <span className={`jobs-modal__summary-chip jobs-modal__summary-chip--expandable${active ? " jobs-modal__summary-chip--active" : ""}`}>
        {content}
        <button
          className="jobs-modal__summary-chip-action"
          onClick={onClick}
          title={active ? "Collapse File List" : "Expand File List"}
          aria-label={active ? "Collapse File List" : "Expand File List"}
        >
          {active ? "-" : "+"}
        </button>
      </span>
    );
  }
  return (
    <span className="jobs-modal__summary-chip">
      {content}
    </span>
  );
}

function JobSummary({
  job,
  canExpandFiles,
  filesExpanded,
  onToggleFiles,
}: {
  job: Job;
  canExpandFiles: boolean;
  filesExpanded: boolean;
  onToggleFiles?: () => void;
}) {
  const fileChip = canExpandFiles ? (
    <SummaryChip
      label="Files"
      value={String(listedFileCount(job))}
      onClick={onToggleFiles}
      active={filesExpanded}
    />
  ) : null;
  if (!isTerminalJob(job)) {
    return fileChip ? <div className="jobs-modal__summary">{fileChip}</div> : null;
  }

  if (job.kind === "sync-bro" && job.syncResult) {
    const differing = job.syncResult.differing.length
      + job.syncResult.localOnly.length
      + job.syncResult.remoteOnly.length
      + job.syncResult.unverified.length;
    const errors = job.syncResult.localErrors.length + job.syncResult.remoteErrors.length;
    return (
      <div className="jobs-modal__summary">
        {fileChip}
        <SummaryChip label="Matched" value={String(job.syncResult.matched)} />
        <SummaryChip label="Differences" value={String(differing)} />
        <SummaryChip label="Errors" value={String(errors)} />
      </div>
    );
  }

  if (job.kind === "fedup-bro" && job.fedupResult) {
    return (
      <div className="jobs-modal__summary">
        {fileChip}
        <SummaryChip label="Groups" value={String(job.fedupResult.duplicateGroups)} />
        <SummaryChip label="Duplicates" value={String(job.fedupResult.duplicateFiles)} />
        <SummaryChip label="Reclaimable" value={formatBytes(job.fedupResult.reclaimableBytes)} />
        <SummaryChip label="Errors" value={String(job.fedupResult.errorTotal)} />
      </div>
    );
  }

  if (job.kind === "outgoing-transfer") {
    return (
      <div className="jobs-modal__summary">
        {canExpandFiles ? fileChip : <SummaryChip label="Files" value={String(completedFileCount(job))} />}
        <SummaryChip label="Size" value={formatBytes(completedBytes(job))} />
      </div>
    );
  }

  if (job.kind === "file-op" || job.kind === "fp-bro" || job.kind === "remote-manifest") {
    return (
      <div className="jobs-modal__summary">
        {canExpandFiles ? fileChip : <SummaryChip label="Files" value={String(completedFileCount(job))} />}
        <SummaryChip label="Size" value={formatBytes(completedBytes(job))} />
      </div>
    );
  }

  if (fileChip) {
    return <div className="jobs-modal__summary">{fileChip}</div>;
  }

  return null;
}

function ProgressBar({ p }: { p: FileOpProgress }) {
  const totalPct = p.totalBytesTotal > 0
    ? Math.round((p.totalBytesCopied / p.totalBytesTotal) * 100)
    : p.overallPercent ?? (
      p.totalFiles > 0
        ? Math.round((p.completedFiles / p.totalFiles) * 100)
        : 0
    );
  const currentLabel = p.statusText || p.currentFile?.split("/").pop() || "Waiting";

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
      <div className="jobs-modal__current-file" title={p.statusText ?? p.currentFile}>
        {currentLabel}
      </div>
    </div>
  );
}

function JobRow({
  job,
  client,
  onClose,
  prefix,
  queueActions,
  suppressActiveActions = false,
  filesExpanded = false,
  filePage = 0,
  onToggleFiles,
  onFilePageChange,
}: {
  job: Job;
  client: WsClient;
  onClose: () => void;
  prefix?: ReactNode;
  queueActions?: ReactNode;
  suppressActiveActions?: boolean;
  filesExpanded?: boolean;
  filePage?: number;
  onToggleFiles?: () => void;
  onFilePageChange?: (page: number) => void;
}) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const isActive = job.state === "Queued" || job.state === "Running" || job.state === "AwaitingDecision";
  const isTerminal = isTerminalJob(job);
  const typeLabel = job.kind === "outgoing-transfer"
    ? `Outgoing ${jobShortId(job.jobId)}`
    : job.kind === "remote-manifest"
    ? "Remote Sync Hashing"
    : job.kind === "sync-bro"
    ? "Sync Bro"
    : job.kind === "fedup-bro"
    ? "Fedup Bro"
    : job.type.charAt(0).toUpperCase() + job.type.slice(1);

  const stateLabel: Record<string, string> = {
    Queued: "Queued",
    Running: "Running",
    AwaitingDecision: "Waiting",
    Completed: "Done",
    Cancelled: "Cancelled",
    Failed: "Failed",
  };
  const canExpandFiles = job.files.length > 0 && !!onToggleFiles && !!onFilePageChange;
  const maxFilePage = Math.max(0, Math.ceil(job.files.length / FILE_LIST_PAGE_SIZE) - 1);
  const clampedFilePage = Math.min(filePage, maxFilePage);
  const fileStart = clampedFilePage * FILE_LIST_PAGE_SIZE;
  const fileEnd = Math.min(fileStart + FILE_LIST_PAGE_SIZE, job.files.length);
  const visibleFiles = filesExpanded ? job.files.slice(fileStart, fileEnd) : [];

  return (
    <div className={`jobs-modal__row${isActive ? " jobs-modal__row--active" : ""}`}>
      <div className="jobs-modal__row-header">
        {prefix}
        <span className="jobs-modal__type">{typeLabel}</span>
        <span className="jobs-modal__timestamp">{new Date(job.startedAt).toLocaleTimeString()}</span>
        <span className={`jobs-modal__state jobs-modal__state--${job.state.toLowerCase()}${job.paused ? " jobs-modal__state--paused" : ""}`}>
          {job.paused ? "Paused" : (stateLabel[job.state] ?? job.state)}
        </span>
        {!suppressActiveActions && isActive && (
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
                onClick={() => cancelJobWithPartialPrompt(client, state, job)}
              >
                <IconX size={12} /> Cancel
              </button>
          </>
        )}
        {queueActions}
        {isTerminal && (
          <button
            className="jobs-modal__action"
            title="Show job details"
            onClick={() => {
              dispatch({ type: "SET_ACTIVE_JOB", jobId: job.jobId });
              onClose();
            }}
          >
            <IconEye size={12} /> Details
          </button>
        )}
      </div>

      <div className="jobs-modal__paths">
        {job.kind === "outgoing-transfer" ? (
          <>
            <span className="jobs-modal__path" title={job.sourcePath}>
              Local shared files: {job.sourcePath}
            </span>
            <span className="jobs-modal__path">
              External destination is on the remote fbro.
            </span>
          </>
        ) : job.kind === "remote-manifest" ? (
          <>
            <span className="jobs-modal__path" title={job.sourcePath}>
              Remote hashing: {job.sourcePath}
            </span>
          </>
        ) : job.kind === "sync-bro" ? (
          <>
            <span className="jobs-modal__path" title={job.sourcePath}>
              Local: {job.sourcePath}
            </span>
            {job.destPath && (
              <span className="jobs-modal__path" title={job.destPath}>
                Remote: {job.destPath}
              </span>
            )}
          </>
        ) : job.kind === "fedup-bro" ? (
          <>
            <span className="jobs-modal__path" title={job.sourcePath}>
              Inputs: {job.sourcePath}
            </span>
          </>
        ) : job.kind === "fp-bro" ? (
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

      <JobSummary
        job={job}
        canExpandFiles={canExpandFiles}
        filesExpanded={filesExpanded}
        onToggleFiles={onToggleFiles}
      />

      {job.error && (
        <div className="jobs-modal__error">{job.error}</div>
      )}

      {canExpandFiles && filesExpanded && (
        <div className="jobs-modal__file-list">
          <div className="jobs-modal__file-list-header">
            <span>Files {fileStart + 1}-{fileEnd} of {job.files.length}</span>
            <div className="jobs-modal__file-list-pager">
              <button
                disabled={clampedFilePage === 0}
                onClick={() => onFilePageChange(clampedFilePage - 1)}
              >
                Previous
              </button>
              <button
                disabled={clampedFilePage >= maxFilePage}
                onClick={() => onFilePageChange(clampedFilePage + 1)}
              >
                Next
              </button>
            </div>
          </div>
          <div className="jobs-modal__file-list-body">
            {visibleFiles.map((file, index) => {
              const path = file.sourcePath ?? file.name;
              return (
                <div key={`${path}:${fileStart + index}`} className="jobs-modal__file-row" title={path}>
                  <span className="jobs-modal__file-name">{file.name}</span>
                  <span className="jobs-modal__file-kind">{file.kind}</span>
                  <span className="jobs-modal__file-size">{fileSizeLabel(file.size)}</span>
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}

function QueueJobRow({
  job,
  order,
  active,
  client,
  onClose,
  fileListProps,
}: {
  job: Job;
  order: number;
  active: boolean;
  client: WsClient;
  onClose: () => void;
  fileListProps: Pick<
    Parameters<typeof JobRow>[0],
    "filesExpanded" | "filePage" | "onToggleFiles" | "onFilePageChange"
  >;
}) {
  const queueActions = active ? null : (
    <div className="jobs-modal__queue-actions">
      <button title="Move up" onClick={() => client.sendIntent("ReorderQueuedJob", undefined, { jobId: job.jobId, action: "up" })}>Up</button>
      <button title="Move down" onClick={() => client.sendIntent("ReorderQueuedJob", undefined, { jobId: job.jobId, action: "down" })}>Down</button>
      <button title="Promote to next in line" onClick={() => client.sendIntent("ReorderQueuedJob", undefined, { jobId: job.jobId, action: "next" })}>Next</button>
      <button title="Move to last in line" onClick={() => client.sendIntent("ReorderQueuedJob", undefined, { jobId: job.jobId, action: "last" })}>Last</button>
      <button title="Delete queued job" onClick={() => client.sendIntent("DeleteQueuedJob", undefined, { jobId: job.jobId })}>
        <IconX size={12} />
      </button>
    </div>
  );
  return (
    <JobRow
      job={job}
      client={client}
      onClose={onClose}
      prefix={<span className="jobs-modal__queue-order">{order}</span>}
      queueActions={queueActions}
      suppressActiveActions={!active}
      {...fileListProps}
    />
  );
}

function actionLabel(action?: string) {
  switch (action) {
    case "spawning_worker":
      return "Starting worker";
    case "worker_spawned":
      return "Worker running";
    case "read_dir":
      return "Reading directory";
    case "scan_entries":
      return "Scanning entries";
    case "read_dir_error":
      return "Directory read failed";
    case "timeout_waiting_worker_exit":
      return "Timed out, waiting for worker";
    case "cancel_waiting_worker_exit":
      return "Cancelled, waiting for worker";
    case "complete":
      return "Finishing";
    default:
      return "Starting";
  }
}

function DirSizeTaskRow({ task, client }: { task: DirSizeTask; client: WsClient }) {
  const nowMs = Date.now();
  const startedAt = task.startedAt ?? task.enqueuedAt;
  const elapsedSeconds = Math.max(0, Math.floor((nowMs - startedAt) / 1000));
  const lastProgressAt = task.lastProgressAt ?? task.startedAt ?? task.enqueuedAt;
  const idleSeconds = Math.max(0, Math.floor((nowMs - lastProgressAt) / 1000));
  const elapsedLabel = elapsedSeconds < 60
    ? `${elapsedSeconds}s`
    : `${Math.floor(elapsedSeconds / 60)}m ${elapsedSeconds % 60}s`;
  const idleLabel = task.status === "Running" && idleSeconds >= 10 ? `No progress for ${idleSeconds}s` : null;
  const currentPath = task.status === "Running" ? task.currentPath : null;
  const currentAction = task.status === "Queued"
    ? "Waiting for a folder-size slot"
    : actionLabel(task.currentAction);

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
        <span className="jobs-modal__path" title={currentPath ?? task.fullPath}>
          Current: {currentAction}{currentPath ? `: ${currentPath}` : ""}
        </span>
        <span className="jobs-modal__path">
          Panel: {task.panelId}
        </span>
      </div>
      <div className="jobs-modal__summary">
        <span className="jobs-modal__summary-chip">
          <span className="jobs-modal__summary-label">Elapsed</span>
          <span className="jobs-modal__summary-value">{elapsedLabel}</span>
        </span>
        <span className="jobs-modal__summary-chip">
          <span className="jobs-modal__summary-label">Folders</span>
          <span className="jobs-modal__summary-value">{task.dirsScanned.toLocaleString()}</span>
        </span>
        <span className="jobs-modal__summary-chip">
          <span className="jobs-modal__summary-label">Files</span>
          <span className="jobs-modal__summary-value">{task.filesScanned.toLocaleString()}</span>
        </span>
        <span className="jobs-modal__summary-chip">
          <span className="jobs-modal__summary-label">Size</span>
          <span className="jobs-modal__summary-value">{formatBytes(task.bytesScanned)}</span>
        </span>
        {task.errors > 0 && (
          <span className="jobs-modal__summary-chip">
            <span className="jobs-modal__summary-label">Errors</span>
            <span className="jobs-modal__summary-value">{task.errors.toLocaleString()}</span>
          </span>
        )}
        {idleLabel && <span className="jobs-modal__error">{idleLabel}</span>}
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

function ProcessRow({ proc, client }: { proc: ChildProcess; client: WsClient }) {
  const elapsed = Math.floor((Date.now() / 1000) - proc.started_at);
  const timeLabel = elapsed < 60
    ? `${elapsed}s ago`
    : elapsed < 3600
    ? `${Math.floor(elapsed / 60)}m ago`
    : `${Math.floor(elapsed / 3600)}h ago`;

  return (
    <div className="jobs-modal__row">
      <div className="jobs-modal__row-header">
        <span className="jobs-modal__type">{proc.name}</span>
        <span className="jobs-modal__time">{timeLabel}</span>
        <button
          className="jobs-modal__action"
          title="Kill process"
          onClick={() => client.sendIntent("KillProcess", undefined, { pid: proc.pid })}
        >
          <IconX size={14} />
        </button>
      </div>
      <div className="jobs-modal__paths">
        <span className="jobs-modal__path" title={proc.label}>{proc.label}</span>
        <span className="jobs-modal__pid">PID {proc.pid}</span>
      </div>
    </div>
  );
}

export function JobsModal({ onClose, client }: { onClose: () => void; client: WsClient }) {
  const state = useAppState();
  const overlayRef = useRef<HTMLDivElement>(null);
  const [activeTab, setActiveTab] = useState<"jobs" | "procs" | "cache">("jobs");
  const [expandedFileJobs, setExpandedFileJobs] = useState<Record<string, boolean>>({});
  const [filePages, setFilePages] = useState<Record<string, number>>({});
  const [, setClockTick] = useState(0);

  const byNewest = (a: Job, b: Job) => b.startedAt - a.startedAt;
  const jobs = Object.values(state.jobs).sort(byNewest);
  const queuedJobIds = new Set(state.jobQueue.jobIds);
  const queuedJobs = state.jobQueue.jobIds
    .map((jobId) => state.jobs[jobId])
    .filter((job): job is Job => !!job);
  const fileJobs = jobs.filter((job) => job.kind === "file-op");
  const outgoingJobs = jobs.filter((job) => job.kind === "outgoing-transfer");
  const broJobs = jobs.filter((job) => job.kind === "fp-bro" || job.kind === "sync-bro" || job.kind === "fedup-bro" || job.kind === "remote-manifest");
  const activeFileJobs = fileJobs.filter(
    (job) => !queuedJobIds.has(job.jobId)
      && (job.state === "Queued" || job.state === "Running" || job.state === "AwaitingDecision")
  );
  const finishedFileJobs = fileJobs.filter(
    (job) => job.state !== "Queued" && job.state !== "Running" && job.state !== "AwaitingDecision"
  );
  const dirSizeTasks = [...state.dirSizeQueue].sort((a, b) => a.enqueuedAt - b.enqueuedAt);
  const childProcs = Object.values(state.childProcesses).sort((a, b) => b.started_at - a.started_at);

  useEffect(() => {
    if (dirSizeTasks.length === 0) return;
    const timer = window.setInterval(() => setClockTick((value) => value + 1), 1000);
    return () => window.clearInterval(timer);
  }, [dirSizeTasks.length]);

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
  const fileListProps = (job: Job) => ({
    filesExpanded: !!expandedFileJobs[job.jobId],
    filePage: filePages[job.jobId] ?? 0,
    onToggleFiles: () => {
      setExpandedFileJobs((current) => ({ ...current, [job.jobId]: !current[job.jobId] }));
      setFilePages((current) => ({ ...current, [job.jobId]: 0 }));
    },
    onFilePageChange: (page: number) => {
      setFilePages((current) => ({ ...current, [job.jobId]: Math.max(0, page) }));
    },
  });

  return (
    <div className="modal-overlay" ref={overlayRef} onClick={onOverlayClick}>
      <ResizableModal
        modalId="jobs"
        client={client}
        className="jobs-modal"
        ariaLabel="Jobs"
        defaultSize={{ width: 640, height: 520 }}
        minSize={{ width: 520, height: 340 }}
      >
        <div className="modal__header">Jobs</div>

        <div className="jobs-modal__tabs">
          <button
            className={`jobs-modal__tab${activeTab === "jobs" ? " jobs-modal__tab--active" : ""}`}
            onClick={() => setActiveTab("jobs")}
          >
            Jobs
          </button>
          <button
            className={`jobs-modal__tab${activeTab === "procs" ? " jobs-modal__tab--active" : ""}`}
            onClick={() => setActiveTab("procs")}
          >
            Processes{childProcs.length > 0 ? ` (${childProcs.length})` : ""}
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
              {fileJobs.length === 0 && outgoingJobs.length === 0 && broJobs.length === 0 && dirSizeTasks.length === 0 && queuedJobs.length === 0 && (
                <div className="jobs-modal__empty">No jobs.</div>
              )}

              {queuedJobs.length > 0 && (
                <section>
                  <div className="jobs-modal__section-title">
                    Copy Queue {state.jobQueue.completedCount + 1} / {state.jobQueue.completedCount + queuedJobs.length}
                  </div>
                  {queuedJobs.map((job, index) => (
                    <QueueJobRow
                      key={job.jobId}
                      job={job}
                      order={state.jobQueue.completedCount + index + 1}
                      active={job.jobId === state.jobQueue.activeJobId}
                      client={client}
                      onClose={onClose}
                      fileListProps={fileListProps(job)}
                    />
                  ))}
                </section>
              )}

              {activeFileJobs.length > 0 && (
                <section>
                  <div className="jobs-modal__section-title">Active File Jobs</div>
                  {activeFileJobs.map(j => <JobRow key={j.jobId} job={j} client={client} onClose={onClose} {...fileListProps(j)} />)}
                </section>
              )}

              {broJobs.length > 0 && (
                <section>
                  <div className="jobs-modal__section-title">Bro Jobs</div>
                  {broJobs.map(j => <JobRow key={j.jobId} job={j} client={client} onClose={onClose} {...fileListProps(j)} />)}
                </section>
              )}

              {outgoingJobs.length > 0 && (
                <section>
                  <div className="jobs-modal__section-title">Outgoing Transfers</div>
                  {outgoingJobs.map(j => <JobRow key={j.jobId} job={j} client={client} onClose={onClose} {...fileListProps(j)} />)}
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
                  {finishedFileJobs.map(j => <JobRow key={j.jobId} job={j} client={client} onClose={onClose} {...fileListProps(j)} />)}
                </section>
              )}
            </>
          )}

          {activeTab === "procs" && (
            <>
              {childProcs.length === 0 ? (
                <div className="jobs-modal__empty">No running processes.</div>
              ) : (
                <section>
                  {childProcs.map(p => <ProcessRow key={p.pid} proc={p} client={client} />)}
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
      </ResizableModal>
    </div>
  );
}
