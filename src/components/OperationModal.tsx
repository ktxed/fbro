import { useState, useRef, useEffect } from "react";
import type { WsClient } from "../ws/client";
import type { PendingOp, Job, ConflictPolicy } from "../ws/types";
import { useAppDispatch } from "../store";
import type { PendingDryRun } from "../store";

interface OperationModalProps {
  pending: PendingOp | null;
  activeJob: Job | null;
  activeJobId: string | null;
  pendingDryRun: PendingDryRun | null;
  panelIds: string[];
  client: WsClient;
}

function opLabel(t: string) {
  switch (t) {
    case "copy": return "Copy";
    case "move": return "Move";
    case "delete": return "Delete";
    default: return t;
  }
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
}

function formatSpeed(bytesPerSec: number): string {
  if (bytesPerSec < 1024) return `${bytesPerSec.toFixed(0)} B/s`;
  if (bytesPerSec < 1024 * 1024) return `${(bytesPerSec / 1024).toFixed(1)} KiB/s`;
  if (bytesPerSec < 1024 * 1024 * 1024) return `${(bytesPerSec / (1024 * 1024)).toFixed(1)} MiB/s`;
  return `${(bytesPerSec / (1024 * 1024 * 1024)).toFixed(1)} GiB/s`;
}

function formatEta(remainingBytes: number, bytesPerSec: number): string {
  const secs = Math.round(remainingBytes / bytesPerSec);
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
}

const LARGE_FILE_THRESHOLD = 1024 * 1024; // 1 MiB — show speed only above this

function useTransferSpeed(currentFile: string | undefined, bytesCopied: number) {
  const sampleRef = useRef<{ file: string; bytes: number; time: number } | null>(null);
  const [speed, setSpeed] = useState<number | null>(null);

  useEffect(() => {
    if (!currentFile) { setSpeed(null); return; }

    const now = Date.now();
    const prev = sampleRef.current;

    if (!prev || prev.file !== currentFile) {
      sampleRef.current = { file: currentFile, bytes: bytesCopied, time: now };
      setSpeed(null);
      return;
    }

    const deltaBytes = bytesCopied - prev.bytes;
    const deltaSec = (now - prev.time) / 1000;

    if (deltaSec >= 0.5 && deltaBytes > 0) {
      setSpeed(deltaBytes / deltaSec);
      sampleRef.current = { file: currentFile, bytes: bytesCopied, time: now };
    }
  }, [bytesCopied, currentFile]);

  return speed;
}

export function OperationModal({ pending, activeJob, activeJobId, pendingDryRun, panelIds, client }: OperationModalProps) {
  const dispatch = useAppDispatch();
  const [conflictPolicy, setConflictPolicy] = useState<ConflictPolicy>("ask");
  const [submitted, setSubmitted] = useState(false);
  const [dryMode, setDryMode] = useState(false);
  const [startPaused, setStartPaused] = useState(true);
  const modalRef = useRef<HTMLDivElement>(null);
  const p = activeJob?.progress;
  const speed = useTransferSpeed(p?.currentFile, p?.currentFileBytesCopied ?? 0);

  const close = () => { setSubmitted(false); setDryMode(false); dispatch({ type: "CLOSE_OPERATION" }); };
  const closeDryRun = () => { setSubmitted(false); setDryMode(false); dispatch({ type: "CLOSE_DRY_RUN" }); };

  // Auto-focus the modal when it appears
  const isVisible = !!(pending || activeJob || pendingDryRun);
  useEffect(() => {
    if (isVisible) modalRef.current?.focus();
  }, [isVisible]);

  // Nothing to show
  if (!pendingDryRun && !pending && !activeJob) return null;

  // ── Dry run result ──────────────────────────────────────────────────────
  if (pendingDryRun) {
    const otherPanel = panelIds.find(id => id !== pendingDryRun.sourcePanel) ?? panelIds[0] ?? "";
    return (
      <div className="modal-overlay" ref={modalRef} tabIndex={-1} onKeyDown={(e) => { if (e.key === "Escape") closeDryRun(); if (e.key === "Tab") e.preventDefault(); }}>
        <div className="modal modal--fixed" onClick={(e) => e.stopPropagation()}>
          <div className="modal__header">Dry Run Script Generated</div>
          <div className="modal__body">
            <p className="modal__summary">Script saved to your home folder:</p>
            <div className="modal__job-params">
              <span className="modal__job-param-label">Script:</span>
              <button
                className="modal__dry-run-name"
                title={pendingDryRun.scriptPath}
                onClick={() => {
                  if (otherPanel) {
                    client.sendIntent("NavigatePanelTo", undefined, {
                      panelId: otherPanel,
                      path: pendingDryRun.scriptDir,
                      cursorHint: pendingDryRun.scriptName,
                    });
                  }
                  closeDryRun();
                }}
              >{pendingDryRun.scriptName}</button>
              <button
                className="modal__job-param-copy"
                onClick={() => navigator.clipboard.writeText(pendingDryRun.scriptPath)}
                title="Copy path"
              >⧉</button>
            </div>
          </div>
          <div className="modal__actions">
            <button className="modal__btn modal__btn--primary" onClick={closeDryRun}>Close</button>
          </div>
        </div>
      </div>
    );
  }

  // ── Phase 1: Confirmation ───────────────────────────────────────────────
  if (pending) {
    const fileCount = pending.files.length;
    const isDelete = pending.opType === "delete";
    const label = opLabel(pending.opType);

    const startOp = (background: boolean) => {
      if (submitted) return;
      setSubmitted(true);
      client.sendIntent("StartFileOp", undefined, {
        opType: pending.opType,
        sourcePanel: pending.sourcePanel,
        destPanel: pending.destPanel,
        conflictPolicy,
        background,
        ...(background && startPaused ? { startPaused: true } : {}),
        ...(dryMode ? { dryMode: true } : {}),
      });
      if (background) {
        close();
      }
      // The reducer will transition to activeJobId when JobStarted arrives
    };

    return (
      <div className="modal-overlay" ref={modalRef} tabIndex={-1} onClick={close} onKeyDown={(e) => { if (e.key === "Tab") e.preventDefault(); }}>
        <div className="modal modal--fixed" onClick={(e) => e.stopPropagation()}>
          <div className="modal__header">{label}</div>
          <div className="modal__body">
            <p className="modal__summary">
              {label} {fileCount === 1 ? (
                <strong>{pending.files[0].name}</strong>
              ) : (
                <>{fileCount} items</>
              )}
              {!isDelete && pending.destPath && (
                <> to <strong>{pending.destPath}</strong></>
              )}
            </p>

            {fileCount > 1 && (
              <ul className="modal__file-list">
                {pending.files.slice(0, 10).map((f) => (
                  <li key={f.name}>{f.name}</li>
                ))}
                {fileCount > 10 && <li>… and {fileCount - 10} more</li>}
              </ul>
            )}

            {!isDelete && (
              <div className="modal__option">
                <label>If file exists:</label>
                <select
                  value={conflictPolicy}
                  onChange={(e) => setConflictPolicy(e.target.value as ConflictPolicy)}
                >
                  <option value="ask">Ask</option>
                  <option value="skip">Skip</option>
                  <option value="overwrite">Overwrite</option>
                </select>
              </div>
            )}
            <div className="modal__option">
              <label>
                <input
                  type="checkbox"
                  checked={dryMode}
                  onChange={(e) => setDryMode(e.target.checked)}
                />
                {" "}Dry mode (generate bash script instead)
              </label>
            </div>
            {!dryMode && (
              <div className="modal__option">
                <label>
                  <input
                    type="checkbox"
                    checked={startPaused}
                    onChange={(e) => setStartPaused(e.target.checked)}
                  />
                  {" "}Start paused when sent to background
                </label>
              </div>
            )}
          </div>
          <div className="modal__actions">
            <button className="modal__btn modal__btn--primary" disabled={submitted} onClick={() => startOp(false)}>
              {submitted ? "Starting…" : label}
            </button>
            {!dryMode && (
              <button className="modal__btn" disabled={submitted} onClick={() => startOp(true)}>
                Background
              </button>
            )}
            <button className="modal__btn" onClick={close}>
              Cancel
            </button>
          </div>
        </div>
      </div>
    );
  }

  // ── Phase 2 & 3: Progress / Decision ─────────────────────────────────────
  if (activeJob) {
    const isTerminal = activeJob.state === "Completed" || activeJob.state === "Cancelled" || activeJob.state === "Failed";
    const isDecision = activeJob.state === "AwaitingDecision";
    const label = opLabel(activeJob.type);

    const filePercent = p && p.currentFileBytesTotal > 0
      ? Math.round((p.currentFileBytesCopied / p.currentFileBytesTotal) * 100)
      : 0;
    const jobPercent = p && p.totalFiles > 0
      ? Math.round((p.completedFiles / p.totalFiles) * 100)
      : 0;

    return (
      <div className="modal-overlay" ref={modalRef} tabIndex={-1} onClick={isTerminal ? close : undefined} onKeyDown={(e) => { if (e.key === "Tab") e.preventDefault(); }}>
        <div className="modal modal--fixed" onClick={(e) => e.stopPropagation()}>
          <div className="modal__header">{label} — {activeJob.state}</div>
          <div className="modal__body">
            <div className="modal__job-params">
              <span className="modal__job-param-label">From:</span>
              <span className="modal__job-param-value" title={activeJob.sourcePath}>{activeJob.sourcePath}</span>
              <button className="modal__job-param-copy" onClick={() => navigator.clipboard.writeText(activeJob.sourcePath)} title="Copy path">⧉</button>
              {activeJob.destPath && (<>
                <span className="modal__job-param-label">To:</span>
                <span className="modal__job-param-value" title={activeJob.destPath}>{activeJob.destPath}</span>
                <button className="modal__job-param-copy" onClick={() => navigator.clipboard.writeText(activeJob.destPath!)} title="Copy path">⧉</button>
              </>)}
            </div>
            {/* Decision required */}
            {isDecision && activeJob.decision && (
              <div className="modal__decision">
                <p>File already exists:</p>
                <p className="modal__conflict-path">{activeJob.decision.destination}</p>
                <div className="modal__actions">
                  {activeJob.decision.options.map((opt) => (
                    <button
                      key={opt}
                      className="modal__btn"
                      onClick={() =>
                        client.sendIntent("ProvideJobDecision", undefined, {
                          jobId: activeJobId,
                          decision: opt,
                        })
                      }
                    >
                      {opt.charAt(0).toUpperCase() + opt.slice(1)}
                    </button>
                  ))}
                </div>
              </div>
            )}

            {/* Progress */}
            {!isDecision && p && (
              <div className="modal__progress">
                {/* Current file progress */}
                <div className="modal__progress-section">
                  <span className="modal__progress-label">{p.currentFile ?? "⏳"}</span>
                  <div className="modal__progress-bar">
                    <div
                      className="modal__progress-fill"
                      style={{ width: `${filePercent}%` }}
                    />
                  </div>
                  <span className="modal__progress-text">
                    {p.currentFile ? (
                      <>
                        {filePercent}% ({formatBytes(p.currentFileBytesCopied)} / {formatBytes(p.currentFileBytesTotal)})
                        {speed !== null && p.currentFileBytesTotal >= LARGE_FILE_THRESHOLD && (
                          <span className="modal__progress-speed">
                            {" — "}{formatSpeed(speed)}
                            {p.currentFileBytesTotal > p.currentFileBytesCopied && (
                              <> · ETA {formatEta(p.currentFileBytesTotal - p.currentFileBytesCopied, speed)}</>
                            )}
                          </span>
                        )}
                      </>
                    ) : "⏳"}
                  </span>
                </div>

                {/* Job-wide progress */}
                <div className="modal__progress-section">
                  <span className="modal__progress-label">Overall</span>
                  <div className="modal__progress-bar">
                    <div
                      className="modal__progress-fill"
                      style={{ width: `${jobPercent}%` }}
                    />
                  </div>
                  <span className="modal__progress-text">
                    {p.completedFiles + (p.currentFile ? 1 : 0)} / {p.totalFiles} files
                    {p.totalBytesTotal > 0 && (
                      <> · {formatBytes(p.totalBytesCopied)} / {formatBytes(p.totalBytesTotal)}</>
                    )}
                  </span>
                </div>
              </div>
            )}

            {/* Terminal state */}
            {isTerminal && (
              <p className="modal__terminal">
                {activeJob.state === "Completed" && "Operation completed successfully."}
                {activeJob.state === "Cancelled" && "Operation was cancelled."}
                {activeJob.state === "Failed" && `Operation failed: ${activeJob.error ?? "unknown error"}`}
              </p>
            )}
          </div>
          <div className="modal__actions">
            {!isTerminal && !isDecision && (
              <>
                <button className="modal__btn" onClick={close}>
                  Background
                </button>
                {activeJob.paused ? (
                  <button className="modal__btn" onClick={() =>
                    client.sendIntent("ResumeJob", undefined, { jobId: activeJobId })
                  }>
                    Resume
                  </button>
                ) : (
                  <button className="modal__btn" onClick={() =>
                    client.sendIntent("PauseJob", undefined, { jobId: activeJobId })
                  }>
                    Pause
                  </button>
                )}
                <button className="modal__btn" onClick={() =>
                  client.sendIntent("CancelJob", undefined, { jobId: activeJobId })
                }>
                  Cancel
                </button>
              </>
            )}
            {isTerminal && (
              <button className="modal__btn modal__btn--primary" onClick={() => {
                client.sendIntent("RefreshPanels");
                close();
              }}>
                Close
              </button>
            )}
          </div>
        </div>
      </div>
    );
  }

  return null;
}
