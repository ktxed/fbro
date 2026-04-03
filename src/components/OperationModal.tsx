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

function MnemonicLabel({ text, mnemonic }: { text: string; mnemonic: string }) {
  const idx = text.toLowerCase().indexOf(mnemonic.toLowerCase());
  if (idx < 0) return <>{text}</>;
  return (
    <>
      {text.slice(0, idx)}
      <span className="modal__mnemonic">{text[idx]}</span>
      {text.slice(idx + 1)}
    </>
  );
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
  const [applyToAll, setApplyToAll] = useState(false);
  const modalRef = useRef<HTMLDivElement>(null);
  const p = activeJob?.progress;
  const speed = useTransferSpeed(p?.currentFile, p?.currentFileBytesCopied ?? 0);

  const close = () => { setSubmitted(false); setDryMode(false); dispatch({ type: "CLOSE_OPERATION" }); };
  const closeDryRun = () => { setSubmitted(false); setDryMode(false); dispatch({ type: "CLOSE_DRY_RUN" }); };

  // Re-focus when the modal appears or changes phase so terminal/decision states
  // still receive keyboard handling after in-place transitions.
  const isVisible = !!(pending || activeJob || pendingDryRun);
  const focusToken = pendingDryRun
    ? `dry:${pendingDryRun.scriptPath}`
    : pending
      ? `pending:${pending.opType}:${pending.sourcePanel}:${pending.destPanel ?? ""}`
      : activeJob
        ? `job:${activeJobId ?? ""}:${activeJob.state}:${activeJob.decision?.destination ?? ""}`
        : "hidden";
  useEffect(() => {
    if (!isVisible) return;
    const timer = window.setTimeout(() => modalRef.current?.focus(), 0);
    return () => window.clearTimeout(timer);
  }, [focusToken, isVisible]);

  useEffect(() => {
    if (activeJob?.state !== "AwaitingDecision") {
      setApplyToAll(false);
    }
  }, [activeJob?.state, activeJob?.decision?.destination]);

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
    const canStartInBackground = !dryMode;
    const conflictPolicies: ConflictPolicy[] = ["ask", "overwrite", "skip"];

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

    const cycleConflictPolicy = () => {
      if (isDelete || submitted) return;
      const index = conflictPolicies.indexOf(conflictPolicy);
      const next = conflictPolicies[(index + 1 + conflictPolicies.length) % conflictPolicies.length];
      setConflictPolicy(next);
    };

    const handlePendingKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (e.key === "Tab") {
        e.preventDefault();
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        close();
        return;
      }
      if (e.key === "Enter") {
        e.preventDefault();
        startOp(false);
        return;
      }
      if ((e.key === "d" || e.key === "D") && !submitted) {
        e.preventDefault();
        setDryMode((current) => !current);
        return;
      }
      if ((e.key === "p" || e.key === "P") && canStartInBackground && !submitted) {
        e.preventDefault();
        setStartPaused((current) => !current);
        return;
      }
      if ((e.key === "a" || e.key === "A")) {
        e.preventDefault();
        cycleConflictPolicy();
        return;
      }
      if (canStartInBackground && (e.key === "b" || e.key === "B")) {
        e.preventDefault();
        startOp(true);
      }
    };

    return (
      <div className="modal-overlay" ref={modalRef} tabIndex={-1} onClick={close} onKeyDown={handlePendingKeyDown}>
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
                <label><MnemonicLabel text="Action if file exists:" mnemonic="a" /></label>
                <select
                  value={conflictPolicy}
                  onChange={(e) => setConflictPolicy(e.target.value as ConflictPolicy)}
                >
                  <option value="ask">Ask</option>
                  <option value="overwrite">Overwrite</option>
                  <option value="skip">Skip</option>
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
                {" "}<MnemonicLabel text="Dry mode (generate bash script instead)" mnemonic="d" />
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
                  {" "}<MnemonicLabel text="Start paused when sent to background" mnemonic="p" />
                </label>
              </div>
            )}
          </div>
          <div className="modal__actions">
            <button className="modal__btn modal__btn--primary" disabled={submitted} onClick={() => startOp(false)}>
              {submitted ? "Starting…" : label}
            </button>
            {canStartInBackground && (
              <button className="modal__btn" disabled={submitted} onClick={() => startOp(true)}>
                <MnemonicLabel text="Background" mnemonic="b" />
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
    const isProgress = !isTerminal && !isDecision;
    const label = opLabel(activeJob.type);
    const decisionOptions = activeJob.decision?.options ?? [];
    const primaryDecision = decisionOptions.includes("overwrite")
      ? "overwrite"
      : decisionOptions[0];
    const skipDecision = decisionOptions.find((opt) => opt === "skip");

    const filePercent = p && p.currentFileBytesTotal > 0
      ? Math.round((p.currentFileBytesCopied / p.currentFileBytesTotal) * 100)
      : 0;
    const jobPercent = p && p.totalFiles > 0
      ? Math.round((p.completedFiles / p.totalFiles) * 100)
      : 0;

    const submitDecision = (decision: string) => {
      if (!activeJobId) return;
      client.sendIntent("ProvideJobDecision", undefined, {
        jobId: activeJobId,
        decision,
        applyToAll: applyToAll && decision !== "cancel",
      });
      setApplyToAll(false);
    };

    const cancelJob = () => {
      if (!activeJobId) return;
      client.sendIntent("CancelJob", undefined, { jobId: activeJobId });
      setApplyToAll(false);
    };

    const resumeJob = () => {
      if (!activeJobId) return;
      client.sendIntent("ResumeJob", undefined, { jobId: activeJobId });
    };

    const pauseJob = () => {
      if (!activeJobId) return;
      client.sendIntent("PauseJob", undefined, { jobId: activeJobId });
    };

    const handleActiveJobKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (e.key === "Tab") {
        e.preventDefault();
        return;
      }
      if (isDecision) {
        if ((e.key === "a" || e.key === "A") && activeJob.decision?.supportsApplyToAll) {
          e.preventDefault();
          setApplyToAll((current) => !current);
          return;
        }
        if (e.key === "Escape") {
          e.preventDefault();
          cancelJob();
          return;
        }
        if (e.key === "Enter" && primaryDecision) {
          e.preventDefault();
          submitDecision(primaryDecision);
          return;
        }
        if ((e.key === "s" || e.key === "S") && skipDecision) {
          e.preventDefault();
          submitDecision(skipDecision);
          return;
        }
      }
      if (isProgress) {
        if (e.key === "Enter") {
          e.preventDefault();
          close();
          return;
        }
        if (e.key === "Escape") {
          e.preventDefault();
          cancelJob();
          return;
        }
        if ((e.key === "r" || e.key === "R") && activeJob.paused) {
          e.preventDefault();
          resumeJob();
          return;
        }
      }
      if (!isTerminal) return;
      if (e.key === "Escape" || e.key === "Enter") {
        e.preventDefault();
        close();
      }
    };

    return (
      <div className="modal-overlay" ref={modalRef} tabIndex={-1} onClick={isTerminal ? close : undefined} onKeyDown={handleActiveJobKeyDown}>
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
                {activeJob.decision.supportsApplyToAll && (
                  <div className="modal__option">
                    <label>
                      <input
                        type="checkbox"
                        checked={applyToAll}
                        onChange={(e) => setApplyToAll(e.target.checked)}
                      />
                      {" "}<MnemonicLabel text="Apply this choice to all conflicts" mnemonic="a" />
                    </label>
                  </div>
                )}
                <div className="modal__actions">
                  {decisionOptions.map((opt) => (
                    <button
                      key={opt}
                      className={`modal__btn${opt === primaryDecision ? " modal__btn--primary" : ""}`}
                      onClick={() => submitDecision(opt)}
                    >
                      {opt === primaryDecision
                        ? opt.charAt(0).toUpperCase() + opt.slice(1)
                        : opt === skipDecision
                          ? <MnemonicLabel text={opt.charAt(0).toUpperCase() + opt.slice(1)} mnemonic="s" />
                          : opt.charAt(0).toUpperCase() + opt.slice(1)}
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
            {isProgress && (
              <>
                <button className="modal__btn modal__btn--primary" onClick={close}>
                  Background
                </button>
                {activeJob.paused ? (
                  <button className="modal__btn" onClick={resumeJob}>
                    <MnemonicLabel text="Resume" mnemonic="r" />
                  </button>
                ) : (
                  <button className="modal__btn" onClick={pauseJob}>
                    Pause
                  </button>
                )}
                <button className="modal__btn" onClick={cancelJob}>
                  Cancel
                </button>
              </>
            )}
            {isDecision && (
              <button className="modal__btn" onClick={cancelJob}>
                Cancel
              </button>
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
