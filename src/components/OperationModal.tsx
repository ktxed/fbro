import { useState, useRef, useEffect } from "react";
import type { WsClient } from "../ws/client";
import type { PendingOp, Job, ConflictPolicy, VerificationPolicy, FileOpFile } from "../ws/types";
import { useAppDispatch, useAppState } from "../store";
import type { AppState, PendingDryRun } from "../store";
import { isArchiveSourceId } from "../utils/sourceId";
import { IconCopy } from "@tabler/icons-react";

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
    case "outgoing": return "Outgoing transfer";
    default: return t;
  }
}

function decisionLabel(decision: string) {
  switch (decision) {
    case "copyRemote": return "Copy Remote Files";
    case "report": return "Generate Report";
    default: return decision.charAt(0).toUpperCase() + decision.slice(1);
  }
}

function jobShortId(jobId: string) {
  return jobId.replace(/^job-/, "#");
}

function splitParentAndName(fullPath: string): { parent: string; name: string } {
  const normalized = fullPath.replace(/\\/g, "/");
  const lastSlash = normalized.lastIndexOf("/");
  if (lastSlash <= 0) {
    return { parent: "/", name: normalized.slice(lastSlash + 1) || normalized };
  }
  return {
    parent: normalized.slice(0, lastSlash) || "/",
    name: normalized.slice(lastSlash + 1),
  };
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

const SPEED_DISPLAY_THRESHOLD = 1024 * 1024; // 1 MiB — show speed only above this
const LARGE_CANCEL_THRESHOLD = 1024 * 1024 * 1024; // 1 GiB
const SPEED_SAMPLE_MIN_INTERVAL_MS = 500;
const SPEED_SAMPLE_MAX_COUNT = 120;
const SPEED_GRAPH_WIDTH = 280;
const SPEED_GRAPH_HEIGHT = 56;
const SPEED_GRAPH_PADDING = 4;
const SPEED_GRAPH_RIGHT = SPEED_GRAPH_WIDTH - SPEED_GRAPH_PADDING;
const SPEED_GRAPH_BOTTOM = SPEED_GRAPH_HEIGHT - SPEED_GRAPH_PADDING;

function useJobTransferSpeed(jobId: string | null, bytesCopied: number, terminal: boolean) {
  const sampleRef = useRef<{ jobId: string; bytes: number; time: number; speed: number | null } | null>(null);
  const [speed, setSpeed] = useState<number | null>(null);
  const [samples, setSamples] = useState<number[]>([]);

  useEffect(() => {
    if (!jobId || terminal) {
      sampleRef.current = null;
      setSpeed(null);
      setSamples([]);
      return;
    }

    const now = Date.now();
    const prev = sampleRef.current;

    if (!prev || prev.jobId !== jobId || bytesCopied < prev.bytes) {
      sampleRef.current = { jobId, bytes: bytesCopied, time: now, speed: null };
      setSpeed(null);
      setSamples([]);
      return;
    }

    const deltaBytes = bytesCopied - prev.bytes;
    const deltaMs = now - prev.time;
    const deltaSec = deltaMs / 1000;

    if (deltaMs >= SPEED_SAMPLE_MIN_INTERVAL_MS && deltaBytes > 0) {
      const instantSpeed = deltaBytes / deltaSec;
      const smoothedSpeed = prev.speed == null
        ? instantSpeed
        : (prev.speed * 0.7) + (instantSpeed * 0.3);
      setSpeed(smoothedSpeed);
      setSamples((current) => [...current.slice(-(SPEED_SAMPLE_MAX_COUNT - 1)), smoothedSpeed]);
      sampleRef.current = { jobId, bytes: bytesCopied, time: now, speed: smoothedSpeed };
    }
  }, [bytesCopied, jobId, terminal]);

  return { speed, samples };
}

function movingAverage3(samples: number[]) {
  return samples.map((_, index) => {
    const start = Math.max(0, index - 2);
    const window = samples.slice(start, index + 1);
    return window.reduce((sum, value) => sum + value, 0) / window.length;
  });
}

function speedGraphData(samples: number[]) {
  if (samples.length < 2) return null;
  const averaged = movingAverage3(samples);
  let minSpeed = Math.min(...averaged);
  let maxSpeed = Math.max(...averaged);
  if (maxSpeed <= 0) return null;

  const span = maxSpeed - minSpeed;
  const minUsefulSpan = Math.max(maxSpeed * 0.12, 1024);
  if (span < minUsefulSpan) {
    const center = averaged[averaged.length - 1] || maxSpeed;
    minSpeed = Math.max(0, center - (minUsefulSpan / 2));
    maxSpeed = center + (minUsefulSpan / 2);
  }

  const domain = Math.max(maxSpeed - minSpeed, 1);
  const usableWidth = SPEED_GRAPH_RIGHT - SPEED_GRAPH_PADDING;
  const usableHeight = SPEED_GRAPH_HEIGHT - (SPEED_GRAPH_PADDING * 2);
  const points = averaged.map((sample, index) => {
    const x = SPEED_GRAPH_PADDING + (index / (samples.length - 1)) * usableWidth;
    const normalized = Math.max(0, Math.min(1, (sample - minSpeed) / domain));
    const y = SPEED_GRAPH_PADDING + usableHeight - (normalized * usableHeight);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  }).join(" ");
  return {
    points,
    areaPoints: `${SPEED_GRAPH_PADDING},${SPEED_GRAPH_BOTTOM} ${points} ${SPEED_GRAPH_RIGHT},${SPEED_GRAPH_BOTTOM}`,
    minSpeed,
    midSpeed: minSpeed + (domain / 2),
    maxSpeed,
  };
}

function hasPartialDestinationFile(destPanelState: AppState["panels"][string] | undefined, file: FileOpFile) {
  if (!destPanelState || file.kind === "directory" || file.size === undefined || file.size === null) {
    return false;
  }
  const destEntry = destPanelState.entries.find((entry) => entry.name === file.name);
  return destEntry?.kind === "file"
    && typeof destEntry.size === "number"
    && destEntry.size > 0
    && destEntry.size < file.size;
}

function canSuggestResumeForPending(state: AppState, pending: PendingOp) {
  const destPanelState = pending.destPanel ? state.panels[pending.destPanel] : undefined;
  return pending.opType !== "delete"
    && !!destPanelState
    && (!pending.destPath || pending.destPath === destPanelState.path)
    && pending.files.some((file) => hasPartialDestinationFile(destPanelState, file));
}

function shouldDefaultResumeForPending(state: AppState, pending: PendingOp) {
  if (pending.opType !== "copy" || pending.files.length !== 1) return false;
  const sourcePanelState = state.panels[pending.sourcePanel];
  const destPanelState = pending.destPanel ? state.panels[pending.destPanel] : undefined;
  const source = sourcePanelState ? state.sources[sourcePanelState.sourceId] : undefined;
  const dest = destPanelState ? state.sources[destPanelState.sourceId] : undefined;
  return source?.kind === "remote"
    && dest?.kind === "filesystem"
    && (!pending.destPath || pending.destPath === destPanelState?.path)
    && hasPartialDestinationFile(destPanelState, pending.files[0]);
}

function pendingSessionKey(pending: PendingOp | null) {
  if (!pending) return "none";
  const filesKey = pending.files
    .map((file) => `${file.name}:${file.kind}:${file.size ?? ""}:${file.sourcePath ?? ""}`)
    .join("|");
  return `${pending.opType}:${pending.sourcePanel}:${pending.destPanel ?? ""}:${pending.destPath ?? ""}:${filesKey}`;
}

export function OperationModal({ pending, activeJob, activeJobId, pendingDryRun, panelIds, client }: OperationModalProps) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const [conflictPolicy, setConflictPolicy] = useState<ConflictPolicy>("ask");
  const [verificationPolicy, setVerificationPolicy] = useState<VerificationPolicy>("fail");
  const [saveDigestFile, setSaveDigestFile] = useState(true);
  const [submitted, setSubmitted] = useState(false);
  const [dryMode, setDryMode] = useState(false);
  const [startPaused, setStartPaused] = useState(true);
  const [applyToAll, setApplyToAll] = useState(false);
  const [confirmingCancel, setConfirmingCancel] = useState(false);
  const modalRef = useRef<HTMLDivElement>(null);
  const p = activeJob?.progress;
  const isTransferTerminal = activeJob?.state === "Completed" || activeJob?.state === "Cancelled" || activeJob?.state === "Failed";
  const { speed, samples: speedSamples } = useJobTransferSpeed(activeJobId, p?.totalBytesCopied ?? 0, isTransferTerminal);
  const pendingKey = pendingSessionKey(pending);

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

  useEffect(() => {
    setConfirmingCancel(false);
  }, [activeJobId, activeJob?.state]);

  useEffect(() => {
    if (!pending) return;
    setConflictPolicy(shouldDefaultResumeForPending(state, pending) ? "resume" : pending.conflictPolicy);
  }, [pendingKey]);

  // Nothing to show
  if (!pendingDryRun && !pending && !activeJob) return null;

  // ── Dry run result ──────────────────────────────────────────────────────
  if (pendingDryRun) {
    const otherPanel = panelIds.find(id => id !== pendingDryRun.sourcePanel) ?? panelIds[0] ?? "";
    return (
      <div className="modal-overlay" ref={modalRef} tabIndex={-1} onKeyDown={(e) => { if (e.key === "Escape") closeDryRun(); if (e.key === "Tab") e.preventDefault(); }}>
        <div className="modal modal--fixed" role="dialog" aria-label="Dry Run Script Generated" data-testid="dry-run-dialog" onClick={(e) => e.stopPropagation()}>
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
              ><IconCopy size={14} /></button>
            </div>
          </div>
          <div className="modal__actions">
            <button className="modal__btn modal__btn--primary" data-command-id="dry-run.close" onClick={closeDryRun}>Close</button>
          </div>
        </div>
      </div>
    );
  }

  // ── Phase 1: Confirmation ───────────────────────────────────────────────
  if (pending) {
    const fileCount = pending.files.length;
    const isDelete = pending.opType === "delete";
    const sourcePanelState = state.panels[pending.sourcePanel];
    const isArchiveSource = !!sourcePanelState?.shadowSource && isArchiveSourceId(sourcePanelState.sourceId);
    const label = isArchiveSource && pending.opType === "copy" ? "Extract" : opLabel(pending.opType);
    const canStartInBackground = !dryMode;
    const canSuggestResume = canSuggestResumeForPending(state, pending);
    const conflictPolicies: ConflictPolicy[] = canSuggestResume
      ? ["ask", "resume", "overwrite", "skip"]
      : ["ask", "overwrite", "skip"];
    const selectedConflictPolicy = conflictPolicies.includes(conflictPolicy) ? conflictPolicy : "ask";

    const startOp = (background: boolean) => {
      if (submitted) return;
      setSubmitted(true);
      client.sendIntent("StartFileOp", undefined, {
        opType: pending.opType,
        files: pending.files,
        sourcePanel: pending.sourcePanel,
        destPanel: pending.destPanel,
        destPath: pending.destPath,
        conflictPolicy: selectedConflictPolicy,
        verificationPolicy,
        saveDigestFile,
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
      const index = conflictPolicies.indexOf(selectedConflictPolicy);
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
        <div className="modal modal--fixed" role="dialog" aria-label={label} data-testid="operation-dialog" onClick={(e) => e.stopPropagation()}>
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
              <>
                <div className="modal__option">
                  <label><MnemonicLabel text="Action if file exists:" mnemonic="a" /></label>
                  <select
                    value={selectedConflictPolicy}
                    onChange={(e) => setConflictPolicy(e.target.value as ConflictPolicy)}
                  >
                    <option value="ask">Ask</option>
                    {canSuggestResume && <option value="resume">Resume partial files</option>}
                    <option value="overwrite">Overwrite</option>
                    <option value="skip">Skip</option>
                  </select>
                </div>
                <div className="modal__option">
                  <label>Action if verification fails:</label>
                  <select
                    value={verificationPolicy}
                    onChange={(e) => setVerificationPolicy(e.target.value as VerificationPolicy)}
                  >
                    <option value="fail">Fail</option>
                    <option value="prompt">Ask</option>
                  </select>
                </div>
                <div className="modal__option">
                  <label>
                    <input
                      type="checkbox"
                      checked={saveDigestFile}
                      onChange={(e) => setSaveDigestFile(e.target.checked)}
                    />
                    {" "}Save BLAKE3 digest file
                  </label>
                </div>
              </>
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
            <button className="modal__btn modal__btn--primary" data-command-id={`file.${pending.opType}.confirm`} disabled={submitted} onClick={() => startOp(false)}>
              {submitted ? "Starting…" : label}
            </button>
            {canStartInBackground && (
              <button className="modal__btn" data-command-id={`file.${pending.opType}.background`} disabled={submitted} onClick={() => startOp(true)}>
                <MnemonicLabel text="Background" mnemonic="b" />
              </button>
            )}
            <button className="modal__btn" data-command-id={`file.${pending.opType}.cancel`} onClick={close}>
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
    const isQueued = activeJob.state === "Queued";
    const isProgress = activeJob.state === "Running";
    const isBroResult = activeJob.kind === "fp-bro" || activeJob.kind === "sync-bro";
    const isOutgoing = activeJob.kind === "outgoing-transfer";
    const activeSourcePanel = state.panels[activeJob.sourcePanel];
    const activeSource = activeSourcePanel ? state.sources[activeSourcePanel.sourceId] : undefined;
    const isRemoteCopy = activeJob.kind === "file-op"
      && activeJob.type === "copy"
      && activeSource?.kind === "remote";
    const label = isOutgoing && activeJobId
      ? `Outgoing ${jobShortId(activeJobId)}`
      : opLabel(activeJob.type);
    const decisionOptions = activeJob.decision?.options ?? [];
    const isVerificationDecision = activeJob.decision?.kind === "verification";
    const primaryDecision = decisionOptions.includes("resume")
      ? "resume"
      : decisionOptions.includes("retry")
      ? "retry"
      : decisionOptions.includes("overwrite")
      ? "overwrite"
      : decisionOptions[0];
    const skipDecision = decisionOptions.find((opt) => opt === "skip");

    const filePercent = p && p.currentFileBytesTotal > 0
      ? Math.round((p.currentFileBytesCopied / p.currentFileBytesTotal) * 100)
      : p?.currentFilePercent ?? 0;
    const jobPercent = p && p.totalBytesTotal > 0
      ? Math.round((p.totalBytesCopied / p.totalBytesTotal) * 100)
      : p?.overallPercent ?? (
        p && p.totalFiles > 0
          ? Math.round((p.completedFiles / p.totalFiles) * 100)
          : 0
      );
    const currentProgressLabel = p?.statusText ?? p?.currentFile ?? "Waiting";
    const totalFileCount = p && p.totalFiles > 0 ? p.totalFiles : activeJob.files.length;
    const showOverallProgress = totalFileCount > 1;
    const shouldConfirmCancel = (isQueued || isProgress)
      && (isRemoteCopy || (p?.currentFileBytesTotal ?? 0) > LARGE_CANCEL_THRESHOLD);
    const graph = speedGraphData(speedSamples);
    const showSpeedGraph = !!graph
      && speed !== null
      && (p?.totalBytesTotal ?? 0) >= SPEED_DISPLAY_THRESHOLD;

    const submitDecision = (decision: string) => {
      if (!activeJobId) return;
      client.sendIntent("ProvideJobDecision", undefined, {
        jobId: activeJobId,
        decision,
        applyToAll: applyToAll && decision !== "cancel",
      });
      setApplyToAll(false);
    };

    const cancelJob = (keepPartialFiles = false) => {
      if (!activeJobId) return;
      client.sendIntent("CancelJob", undefined, { jobId: activeJobId, keepPartialFiles });
      setApplyToAll(false);
      setConfirmingCancel(false);
    };

    const requestCancelJob = () => {
      if (shouldConfirmCancel) {
        setConfirmingCancel(true);
        return;
      }
      cancelJob();
    };

    const resumeJob = () => {
      if (!activeJobId) return;
      client.sendIntent("ResumeJob", undefined, { jobId: activeJobId });
    };

    const pauseJob = () => {
      if (!activeJobId) return;
      client.sendIntent("PauseJob", undefined, { jobId: activeJobId });
    };

    const showResult = () => {
      if (!activeJob.resultPath) return;
      const { parent, name } = splitParentAndName(activeJob.resultPath);
      const targetPanel = activeJob.resultPanelId ?? panelIds[0];
      if (targetPanel) {
        client.sendIntent("NavigatePanelTo", undefined, {
          panelId: targetPanel,
          path: parent,
          cursorHint: name,
        });
      }
      close();
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
      if (isQueued || isProgress) {
        if (confirmingCancel) {
          if (e.key === "Escape" || e.key === "Enter") {
            e.preventDefault();
            setConfirmingCancel(false);
            return;
          }
          return;
        }
        if (e.key === "Enter") {
          e.preventDefault();
          close();
          return;
        }
        if (e.key === "Escape") {
          e.preventDefault();
          requestCancelJob();
          return;
        }
        if ((e.key === "r" || e.key === "R") && activeJob.supportsPause && activeJob.paused) {
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
        <div className="modal modal--fixed" role="dialog" aria-label={`${label} ${activeJob.state}`} data-testid="job-dialog" onClick={(e) => e.stopPropagation()}>
          <div className="modal__header">{label} — {activeJob.state}</div>
          <div className="modal__body">
            <div className="modal__job-params">
              {isOutgoing ? (
                <>
                  <span className="modal__job-param-label">From:</span>
                  <span className="modal__job-param-value" title={activeJob.sourcePath}>{activeJob.sourcePath}</span>
                  <span className="modal__job-param-label">To:</span>
                  <span className="modal__job-param-value">Remote fbro destination unavailable</span>
                </>
              ) : isBroResult ? (
                <>
                  <span className="modal__job-param-label">Inputs:</span>
                  <span className="modal__job-param-value" title={activeJob.sourcePath}>{activeJob.sourcePath}</span>
                  <button className="modal__job-param-copy" onClick={() => navigator.clipboard.writeText(activeJob.sourcePath)} title="Copy path"><IconCopy size={14} /></button>
                  {activeJob.resultPath && (<>
                    <span className="modal__job-param-label">Result:</span>
                    <span className="modal__job-param-value" title={activeJob.resultPath}>{activeJob.resultPath}</span>
                    <button className="modal__job-param-copy" onClick={() => navigator.clipboard.writeText(activeJob.resultPath!)} title="Copy path"><IconCopy size={14} /></button>
                  </>)}
                </>
              ) : (
                <>
                  <span className="modal__job-param-label">From:</span>
                  <span className="modal__job-param-value" title={activeJob.sourcePath}>{activeJob.sourcePath}</span>
                  <button className="modal__job-param-copy" onClick={() => navigator.clipboard.writeText(activeJob.sourcePath)} title="Copy path"><IconCopy size={14} /></button>
                  {activeJob.destPath && (<>
                    <span className="modal__job-param-label">To:</span>
                    <span className="modal__job-param-value" title={activeJob.destPath}>{activeJob.destPath}</span>
                    <button className="modal__job-param-copy" onClick={() => navigator.clipboard.writeText(activeJob.destPath!)} title="Copy path"><IconCopy size={14} /></button>
                  </>)}
                </>
              )}
            </div>
            {showSpeedGraph && (
              <div className="modal__speed-graph" aria-label="Transfer speed graph">
                <div className="modal__speed-graph-frame">
                  <svg
                    className="modal__speed-graph-svg"
                    viewBox={`0 0 ${SPEED_GRAPH_WIDTH} ${SPEED_GRAPH_HEIGHT}`}
                    preserveAspectRatio="none"
                    aria-hidden="true"
                  >
                    <line
                      className="modal__speed-graph-grid"
                      x1={SPEED_GRAPH_PADDING}
                      y1={SPEED_GRAPH_PADDING}
                      x2={SPEED_GRAPH_RIGHT}
                      y2={SPEED_GRAPH_PADDING}
                    />
                    <line
                      className="modal__speed-graph-grid"
                      x1={SPEED_GRAPH_PADDING}
                      y1={SPEED_GRAPH_HEIGHT / 2}
                      x2={SPEED_GRAPH_RIGHT}
                      y2={SPEED_GRAPH_HEIGHT / 2}
                    />
                    <line
                      className="modal__speed-graph-grid"
                      x1={SPEED_GRAPH_PADDING}
                      y1={SPEED_GRAPH_BOTTOM}
                      x2={SPEED_GRAPH_RIGHT}
                      y2={SPEED_GRAPH_BOTTOM}
                    />
                    <line
                      className="modal__speed-graph-axis"
                      x1={SPEED_GRAPH_RIGHT}
                      y1={SPEED_GRAPH_PADDING}
                      x2={SPEED_GRAPH_RIGHT}
                      y2={SPEED_GRAPH_BOTTOM}
                    />
                    <polygon className="modal__speed-graph-fill" points={graph.areaPoints} />
                    <polyline className="modal__speed-graph-line" points={graph.points} />
                  </svg>
                  <div className="modal__speed-graph-scale-panel" aria-hidden="true">
                    <span>{formatSpeed(graph.maxSpeed)}</span>
                    <span>{formatSpeed(graph.midSpeed)}</span>
                    <span>{formatSpeed(graph.minSpeed)}</span>
                  </div>
                </div>
                <div className="modal__speed-graph-meta">
                  <span className="modal__progress-speed">{formatSpeed(speed)}</span>
                  {p && p.totalBytesTotal > p.totalBytesCopied && (
                    <span>ETA {formatEta(p.totalBytesTotal - p.totalBytesCopied, speed)}</span>
                  )}
                </div>
              </div>
            )}
            {isOutgoing && (
              <p className="modal__summary">
                The external destination is controlled by the remote fbro and is not available here.
              </p>
            )}
            {confirmingCancel && (
              <div className="modal__cancel-confirm" role="alertdialog" aria-label="Confirm cancel transfer">
                <p className="modal__cancel-title">Cancel this transfer?</p>
                {isRemoteCopy ? (
                  <p className="modal__summary">
                    This is a remote copy. Choose whether partially copied destination files should be kept for resuming later or deleted.
                  </p>
                ) : (
                  <p className="modal__summary">
                    {p?.currentFile ? <strong>{p.currentFile}</strong> : "The current file"} is larger than 1 GiB.
                    Cancelling will stop the active job.
                  </p>
                )}
                {p && p.currentFileBytesTotal > 0 && (
                  <p className="modal__cancel-detail">
                    {formatBytes(p.currentFileBytesCopied)} of {formatBytes(p.currentFileBytesTotal)} copied for the current file.
                  </p>
                )}
              </div>
            )}
            {/* Decision required */}
            {isDecision && activeJob.decision && (
              <div className="modal__decision">
                <p>{isVerificationDecision ? "Transfer verification failed:" : "File already exists:"}</p>
                <p className="modal__conflict-path">{activeJob.decision.destination}</p>
                {activeJob.decision.message && (
                  <p className="modal__summary">{activeJob.decision.message}</p>
                )}
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
                      data-command-id={`job.decision.${opt}`}
                      onClick={() => submitDecision(opt)}
                    >
                      {opt === primaryDecision
                        ? decisionLabel(opt)
                        : opt === skipDecision
                          ? <MnemonicLabel text={decisionLabel(opt)} mnemonic="s" />
                          : decisionLabel(opt)}
                    </button>
                  ))}
                </div>
              </div>
            )}

            {/* Progress */}
            {isQueued && (
              <p className="modal__terminal">
                This bro job is queued and will start after the current file-producing bro finishes.
              </p>
            )}

            {!isDecision && !isQueued && p && (
              <div className="modal__progress">
                {/* Current file progress */}
                <div className="modal__progress-section">
                  <span className="modal__progress-label">{currentProgressLabel}</span>
                  <div className="modal__progress-bar">
                    <div
                      className="modal__progress-fill"
                      style={{ width: `${filePercent}%` }}
                    />
                  </div>
                  <span className="modal__progress-text">
                    {p.currentFileBytesTotal > 0 ? (
                      <>
                        {filePercent}% ({formatBytes(p.currentFileBytesCopied)} / {formatBytes(p.currentFileBytesTotal)})
                      </>
                    ) : p.currentFilePercent !== undefined ? `${filePercent}%` : currentProgressLabel}
                  </span>
                </div>

                {/* Job-wide progress */}
                {showOverallProgress && (
                  <div className="modal__progress-section">
                    <span className="modal__progress-label">Overall</span>
                    <div className="modal__progress-bar">
                      <div
                        className="modal__progress-fill"
                        style={{ width: `${jobPercent}%` }}
                      />
                    </div>
                    <span className="modal__progress-text">
                      {p.totalBytesTotal > 0
                        ? `${formatBytes(p.totalBytesCopied)} / ${formatBytes(p.totalBytesTotal)}`
                        : p.overallPercent !== undefined
                          ? `${jobPercent}%`
                          : `${p.completedFiles + (p.currentFile ? 1 : 0)} / ${p.totalFiles} files`}
                    </span>
                  </div>
                )}
              </div>
            )}

            {isOutgoing && activeJob.files.length > 0 && (
              <div className="modal__transfer-files">
                {activeJob.files.map((file) => {
                  const path = file.sourcePath ?? file.name;
                  const isCurrent = p?.currentFile === path;
                  return (
                    <div
                      key={path}
                      className={`modal__transfer-file${isCurrent ? " modal__transfer-file--current" : ""}`}
                      title={path}
                    >
                      <span>{file.name}</span>
                      {file.size !== undefined && <span>{formatBytes(file.size)}</span>}
                    </div>
                  );
                })}
              </div>
            )}

            {/* Terminal state */}
            {isTerminal && (
              <>
                <p className="modal__terminal">
                  {activeJob.state === "Completed" && (isOutgoing ? "Outgoing transfer completed." : isBroResult ? "Bro completed successfully." : "Operation completed successfully.")}
                  {activeJob.state === "Cancelled" && (isOutgoing ? "Outgoing transfer was cancelled." : isBroResult ? "Bro job was cancelled." : "Operation was cancelled.")}
                  {activeJob.state === "Failed" && (isOutgoing ? `Outgoing transfer failed: ${activeJob.error ?? "unknown error"}` : isBroResult ? `Bro failed: ${activeJob.error ?? "unknown error"}` : `Operation failed: ${activeJob.error ?? "unknown error"}`)}
                </p>
                {isBroResult && activeJob.state === "Completed" && activeJob.resultPath && (
                  <div className="modal__job-params">
                    <span className="modal__job-param-label">Output:</span>
                    <button
                      className="modal__dry-run-name"
                      title={activeJob.resultPath}
                      onClick={showResult}
                    >{splitParentAndName(activeJob.resultPath).name}</button>
                    <button
                      className="modal__job-param-copy"
                      onClick={() => navigator.clipboard.writeText(activeJob.resultPath!)}
                      title="Copy path"
                    ><IconCopy size={14} /></button>
                  </div>
                )}
              </>
            )}
          </div>
          <div className="modal__actions">
            {(isQueued || isProgress) && !confirmingCancel && (
              <>
                <button className="modal__btn modal__btn--primary" data-command-id="job.background" onClick={close}>
                  Background
                </button>
                {activeJob.supportsPause && activeJob.paused ? (
                  <button className="modal__btn" data-command-id="job.resume" onClick={resumeJob}>
                    <MnemonicLabel text="Resume" mnemonic="r" />
                  </button>
                ) : activeJob.supportsPause ? (
                  <button className="modal__btn" data-command-id="job.pause" onClick={pauseJob}>
                    Pause
                  </button>
                ) : null}
                <button className="modal__btn" data-command-id="job.cancel" onClick={requestCancelJob}>
                  Cancel
                </button>
              </>
            )}
            {isDecision && (
              <button className="modal__btn" data-command-id="job.cancel" onClick={() => cancelJob()}>
                Cancel
              </button>
            )}
            {isTerminal && (
              <>
                {isBroResult && activeJob.state === "Completed" && activeJob.resultPath && (
                  <button className="modal__btn" data-command-id="job.show-result" onClick={showResult}>
                    Show Result
                  </button>
                )}
                {isBroResult && activeJob.resultPath && (
                  <button className="modal__btn" data-command-id="job.copy-path" onClick={() => navigator.clipboard.writeText(activeJob.resultPath!)}>
                    Copy Path
                  </button>
                )}
                <button className="modal__btn modal__btn--primary" data-command-id="job.close" onClick={() => {
                  client.sendIntent("RefreshPanels");
                  close();
                }}>
                  Close
                </button>
              </>
            )}
            {confirmingCancel && (
              <>
                <button className="modal__btn modal__btn--primary" data-command-id="job.keep-running" onClick={() => setConfirmingCancel(false)}>
                  Keep Running
                </button>
                {isRemoteCopy ? (
                  <>
                    <button className="modal__btn" data-command-id="job.cancel.delete-partials" onClick={() => cancelJob(false)}>
                      Delete Partials
                    </button>
                    <button className="modal__btn" data-command-id="job.cancel.keep-partials" onClick={() => cancelJob(true)}>
                      Keep Partials
                    </button>
                  </>
                ) : (
                  <button className="modal__btn" data-command-id="job.cancel.confirm" onClick={() => cancelJob()}>
                    Cancel Transfer
                  </button>
                )}
              </>
            )}
          </div>
        </div>
      </div>
    );
  }

  return null;
}
