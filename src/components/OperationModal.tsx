import { useState, useRef, useEffect } from "react";
import type { WsClient } from "../ws/client";
import type { PendingOp, Job, ConflictPolicy, VerificationPolicy, FileOpFile, SyncBroResult, FedupBroResult } from "../ws/types";
import { useAppDispatch, useAppState } from "../store";
import type { AppState, PendingDryRun } from "../store";
import { isArchiveSourceId, isGlobalSelectionSourceId } from "../utils/sourceId";
import { IconAlertTriangle } from "@tabler/icons-react";
import { ResizableModal } from "./ResizableModal";
import { ModalLayer } from "./ModalStack";
import { isEditableTarget, isLetterKey, isNativeActivationTarget } from "./modalKeyboard";
import { useCopyToClipboard } from "../hooks/useCopyToClipboard";
import { CopyButton } from "./CopyButton";
import { MnemonicLabel } from "./MnemonicLabel";

interface OperationModalProps {
  pending: PendingOp | null;
  activeJob: Job | null;
  activeJobId: string | null;
  pendingDryRun: PendingDryRun | null;
  panelIds: string[];
  client: WsClient;
}

function SyncBroResultsView({ result }: { result: SyncBroResult }) {
  const errorCount = result.localErrors.length + result.remoteErrors.length;
  const counts: { label: string; value: number; cc?: string }[] = [
    { label: "Matched", value: result.matched, cc: "ok" },
    { label: "Differing", value: result.differing.length, cc: result.differing.length ? "warn" : undefined },
    { label: "Local-only", value: result.localOnly.length, cc: result.localOnly.length ? "warn" : undefined },
    { label: "Remote-only", value: result.remoteOnly.length, cc: result.remoteOnly.length ? "warn" : undefined },
    { label: "Unverified", value: result.unverified.length, cc: result.unverified.length ? "warn" : undefined },
    { label: "Errors", value: errorCount, cc: errorCount ? "err" : undefined },
  ];
  return (
    <div className="modal__sync-results">
      <div className="modal__sync-counts">
        {counts.map((c) => (
          <div key={c.label} className={`modal__sync-count${c.cc ? ` modal__sync-count--${c.cc}` : ""}`}>
            <span className="modal__sync-count-value">{c.value}</span>
            <span className="modal__sync-count-label">{c.label}</span>
          </div>
        ))}
      </div>
      <div className="modal__sync-lists">
        {result.differing.length > 0 && (
          <section className="modal__sync-section">
            <h4>Differing</h4>
            {result.differing.map((d) => (
              <div key={d.path} className="modal__sync-row" title={d.path}>
                <span className="modal__sync-path">{d.path}</span>
                <span className="modal__sync-detail">
                  local {formatBytes(d.localSize)} · remote {formatBytes(d.remoteSize)}
                </span>
              </div>
            ))}
          </section>
        )}
        {result.localOnly.length > 0 && (
          <section className="modal__sync-section">
            <h4>Local-only</h4>
            {result.localOnly.map((p) => (
              <div key={p} className="modal__sync-row" title={p}><span className="modal__sync-path">{p}</span></div>
            ))}
          </section>
        )}
        {result.remoteOnly.length > 0 && (
          <section className="modal__sync-section">
            <h4>Remote-only</h4>
            {result.remoteOnly.map((p) => (
              <div key={p} className="modal__sync-row" title={p}><span className="modal__sync-path">{p}</span></div>
            ))}
          </section>
        )}
        {result.unverified.length > 0 && (
          <section className="modal__sync-section">
            <h4>Unverified</h4>
            {result.unverified.map((p) => (
              <div key={p} className="modal__sync-row" title={p}><span className="modal__sync-path">{p}</span></div>
            ))}
          </section>
        )}
        {(result.localErrors.length > 0 || result.remoteErrors.length > 0) && (
          <section className="modal__sync-section">
            <h4>Hash errors</h4>
            {[...result.localErrors, ...result.remoteErrors].map((e) => (
              <div key={e.path} className="modal__sync-row" title={`${e.path}: ${e.message}`}>
                <span className="modal__sync-path">{e.path}</span>
                <span className="modal__sync-detail modal__sync-detail--err">{e.message}</span>
              </div>
            ))}
          </section>
        )}
      </div>
    </div>
  );
}

function FedupBroResultsView({ jobId, result, client }: { jobId: string; result: FedupBroResult; client: WsClient }) {
  const [expandedGroupKey, setExpandedGroupKey] = useState<string | null>(null);
  const counts: { label: string; value: string; cc?: string }[] = [
    { label: "Groups", value: String(result.duplicateGroups), cc: result.duplicateGroups ? "warn" : "ok" },
    { label: "Duplicate files", value: String(result.duplicateFiles), cc: result.duplicateFiles ? "warn" : "ok" },
    { label: "Reclaimable", value: formatBytes(result.reclaimableBytes), cc: result.reclaimableBytes ? "warn" : undefined },
    { label: "Scanned", value: String(result.scannedFiles) },
    { label: "Errors", value: String(result.errorTotal), cc: result.errorTotal ? "err" : undefined },
  ];
  const requestPage = (groupOffset: number, errorOffset: number) => {
    client.sendIntent("GetFedupResultPage", undefined, {
      jobId,
      groupOffset,
      groupLimit: result.groupLimit,
      errorOffset,
      errorLimit: result.errorLimit,
    });
  };
  const groupStart = result.groupTotal === 0 ? 0 : result.groupOffset + 1;
  const groupEnd = Math.min(result.groupOffset + result.groups.length, result.groupTotal);
  const errorStart = result.errorTotal === 0 ? 0 : result.errorOffset + 1;
  const errorEnd = Math.min(result.errorOffset + result.errors.length, result.errorTotal);
  const canPrevGroups = result.groupOffset > 0;
  const canNextGroups = result.groupOffset + result.groupLimit < result.groupTotal;
  const canPrevErrors = result.errorOffset > 0;
  const canNextErrors = result.errorOffset + result.errorLimit < result.errorTotal;
  return (
    <div className="modal__sync-results modal__sync-results--fedup">
      <div className="modal__sync-counts">
        {counts.map((c) => (
          <div key={c.label} className={`modal__sync-count${c.cc ? ` modal__sync-count--${c.cc}` : ""}`}>
            <span className="modal__sync-count-value">{c.value}</span>
            <span className="modal__sync-count-label">{c.label}</span>
          </div>
        ))}
      </div>
      <div className="modal__sync-lists">
        {result.groupTotal > 0 && (
          <section className="modal__sync-section">
            <h4>Duplicate groups {groupStart}-{groupEnd} of {result.groupTotal}</h4>
            {result.groups.map((group) => {
              const groupKey = `${group.hash}:${group.keeper.path}`;
              const expanded = expandedGroupKey === groupKey;
              return (
                <div key={groupKey} className="modal__sync-row modal__sync-row--stacked" title={group.keeper.path}>
                  <div className="modal__sync-row-main">
                    <span className="modal__sync-path">Keep: {group.keeper.path}</span>
                    <span className="modal__sync-detail">
                      {group.duplicates.length} duplicate{group.duplicates.length === 1 ? "" : "s"} · {formatBytes(group.size)} each
                    </span>
                    <button
                      className="modal__btn modal__btn--compact"
                      onClick={() => setExpandedGroupKey(expanded ? null : groupKey)}
                    >
                      {expanded ? "Collapse list" : "Expand list"}
                    </button>
                  </div>
                  {expanded && (
                    <div className="modal__fedup-file-list">
                      <div className="modal__fedup-file-row">
                        <span className="modal__fedup-file-role">Keep</span>
                        <CopyButton text={group.keeper.path} what="path" title="Copy path" />
                        <span className="modal__fedup-file-path" title={group.keeper.path}>{group.keeper.path}</span>
                      </div>
                      {group.duplicates.map((file) => (
                        <div key={file.path} className="modal__fedup-file-row">
                          <span className="modal__fedup-file-role">Candidate</span>
                          <CopyButton text={file.path} what="path" title="Copy path" />
                          <span className="modal__fedup-file-path" title={file.path}>{file.path}</span>
                        </div>
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
            {(canPrevGroups || canNextGroups) && (
              <div className="modal__actions modal__actions--inline">
                <button
                  className="modal__btn"
                  disabled={!canPrevGroups}
                  onClick={() => requestPage(Math.max(0, result.groupOffset - result.groupLimit), result.errorOffset)}
                >
                  Previous groups
                </button>
                <button
                  className="modal__btn"
                  disabled={!canNextGroups}
                  onClick={() => requestPage(result.groupOffset + result.groupLimit, result.errorOffset)}
                >
                  Next groups
                </button>
              </div>
            )}
          </section>
        )}
        {result.errorTotal > 0 && (
          <section className="modal__sync-section">
            <h4>Scan errors {errorStart}-{errorEnd} of {result.errorTotal}</h4>
            {result.errors.map((e) => (
              <div key={`${e.path}:${e.message}`} className="modal__sync-row" title={`${e.path}: ${e.message}`}>
                <span className="modal__sync-path">{e.path}</span>
                <span className="modal__sync-detail modal__sync-detail--err">{e.message}</span>
              </div>
            ))}
            {(canPrevErrors || canNextErrors) && (
              <div className="modal__actions modal__actions--inline">
                <button
                  className="modal__btn"
                  disabled={!canPrevErrors}
                  onClick={() => requestPage(result.groupOffset, Math.max(0, result.errorOffset - result.errorLimit))}
                >
                  Previous errors
                </button>
                <button
                  className="modal__btn"
                  disabled={!canNextErrors}
                  onClick={() => requestPage(result.groupOffset, result.errorOffset + result.errorLimit)}
                >
                  Next errors
                </button>
              </div>
            )}
          </section>
        )}
      </div>
    </div>
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

function supportsCopyDestName(pending: PendingOp) {
  if (pending.opType !== "copy" || pending.files.length !== 1) return false;
  const kind = pending.files[0].kind;
  return kind === "file" || kind === "directory";
}

function supportsQueuedLocalCopy(state: AppState, pending: PendingOp, dryMode: boolean) {
  if (dryMode || pending.opType !== "copy" || !pending.destPanel) return false;
  const sourcePanel = state.panels[pending.sourcePanel];
  const destPanel = state.panels[pending.destPanel];
  if (!sourcePanel || !destPanel) return false;
  const source = state.sources[sourcePanel.sourceId];
  const destination = state.sources[destPanel.sourceId];
  return !!source
    && !!destination
    && source.kind === "filesystem"
    && destination.kind === "filesystem"
    && !source.shadow
    && !destination.shadow
    && !sourcePanel.shadowSource
    && !destPanel.shadowSource;
}

function destinationPanelShowsTargetPath(
  pending: PendingOp,
  destPanelState: AppState["panels"][string] | undefined,
) {
  return !!destPanelState && (!pending.destPath || pending.destPath === destPanelState.path);
}

type CopyDestNameIssueKind = "empty" | "separator" | "existing-destination" | "same-panel-name";

interface CopyDestNameIssue {
  kind: CopyDestNameIssueKind;
  message: string;
}

function copyDestNameIssue(
  pending: PendingOp,
  destName: string,
  destPanelState: AppState["panels"][string] | undefined,
): CopyDestNameIssue | null {
  if (!supportsCopyDestName(pending)) return null;
  if (!destName) return { kind: "empty", message: "Destination name is required." };
  if (/[\\/]/.test(destName)) {
    return { kind: "separator", message: "Destination name cannot contain path separators." };
  }
  if (
    pending.destPanel !== pending.sourcePanel
    && destPanelState
    && destinationPanelShowsTargetPath(pending, destPanelState)
    && destPanelState.entries.some((entry) => entry.name === destName)
  ) {
    return { kind: "existing-destination", message: "Destination already contains an item with this name." };
  }
  if (
    pending.destPanel === pending.sourcePanel
    && !pending.files[0].sourcePath
    && destName === pending.files[0].name
  ) {
    return { kind: "same-panel-name", message: "Change the destination name when copying into the same panel." };
  }
  return null;
}

function isCopyDestNameConflictIssue(issue: CopyDestNameIssue | null) {
  return issue?.kind === "existing-destination" || issue?.kind === "same-panel-name";
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

function shouldDefaultSaveDigestFileForPending(state: AppState, pending: PendingOp) {
  if (pending.opType !== "copy") return false;
  const sourcePanelState = state.panels[pending.sourcePanel];
  const source = sourcePanelState ? state.sources[sourcePanelState.sourceId] : undefined;
  return source?.kind === "remote";
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
  const copyToClipboard = useCopyToClipboard();
  const [conflictPolicy, setConflictPolicy] = useState<ConflictPolicy>("ask");
  const [verificationPolicy, setVerificationPolicy] = useState<VerificationPolicy>("fail");
  const [saveDigestFileState, setSaveDigestFileState] = useState({ pendingKey: "none", value: false });
  const [submitted, setSubmitted] = useState(false);
  const [dryMode, setDryMode] = useState(false);
  const [startPaused, setStartPaused] = useState(true);
  const [destName, setDestName] = useState("");
  const [applyToAll, setApplyToAll] = useState(false);
  const [confirmingCancel, setConfirmingCancel] = useState(false);
  const modalRef = useRef<HTMLDivElement>(null);
  const destNameInputRef = useRef<HTMLInputElement>(null);
  const selectedDestNameConflictRef = useRef<string | null>(null);
  // The modal stays mounted between operations, so a repeated identical copy
  // must not inherit the previous operation's "Starting…" state.
  const submittedPendingRef = useRef<PendingOp | null>(null);
  const p = activeJob?.progress;
  const isTransferTerminal = activeJob?.state === "Completed" || activeJob?.state === "Cancelled" || activeJob?.state === "Failed";
  const { speed, samples: speedSamples } = useJobTransferSpeed(activeJobId, p?.totalBytesCopied ?? 0, isTransferTerminal);
  const pendingKey = pendingSessionKey(pending);
  const pendingDestPanelState = pending?.destPanel ? state.panels[pending.destPanel] : undefined;
  const pendingDestNameIssue = pending && supportsCopyDestName(pending)
    ? copyDestNameIssue(pending, destName, pendingDestPanelState)
    : null;
  const pendingDestNameConflictKey = pending && isCopyDestNameConflictIssue(pendingDestNameIssue)
    ? `${pendingKey}:${pendingDestNameIssue?.kind}:${destName}`
    : null;
  const defaultSaveDigestFile = pending ? shouldDefaultSaveDigestFileForPending(state, pending) : false;
  const saveDigestFile = saveDigestFileState.pendingKey === pendingKey
    ? saveDigestFileState.value
    : defaultSaveDigestFile;

  const close = () => {
    submittedPendingRef.current = null;
    setSubmitted(false);
    setDryMode(false);
    dispatch({ type: "CLOSE_OPERATION" });
  };
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
    submittedPendingRef.current = null;
    setSubmitted(false);
    setDryMode(false);
    setConflictPolicy(shouldDefaultResumeForPending(state, pending) ? "resume" : pending.conflictPolicy);
    setDestName(supportsCopyDestName(pending) ? pending.files[0].name : "");
  }, [pending]);

  useEffect(() => {
    if (!pendingDestNameConflictKey) {
      selectedDestNameConflictRef.current = null;
      return;
    }
    if (selectedDestNameConflictRef.current === pendingDestNameConflictKey) return;
    selectedDestNameConflictRef.current = pendingDestNameConflictKey;
    const timer = window.setTimeout(() => {
      destNameInputRef.current?.focus();
      destNameInputRef.current?.select();
    }, 0);
    return () => window.clearTimeout(timer);
  }, [pendingDestNameConflictKey]);

  // Nothing to show
  if (!pendingDryRun && !pending && !activeJob) return null;

  // ── Dry run result ──────────────────────────────────────────────────────
  if (pendingDryRun) {
    const otherPanel = panelIds.find(id => id !== pendingDryRun.sourcePanel) ?? panelIds[0] ?? "";
    const title = pendingDryRun.manifestPath ? "Fedup Delete Dry-Run Generated" : "Dry Run Script Generated";
    return (
      <ModalLayer modalId="operation:dry-run" ref={modalRef} onKeyDown={(e) => { if (e.key === "Escape") closeDryRun(); }}>
        <ResizableModal
          modalId="operation:dry-run"
          client={client}
          className="modal--fixed"
          ariaLabel={title}
          testId="dry-run-dialog"
          defaultSize={{ width: 560, height: 380 }}
          minSize={{ width: 420, height: 280 }}
        >
          <div className="modal__header">{title}</div>
          <div className="modal__body">
            <p className="modal__summary">
              {pendingDryRun.manifestPath ? "Script and manifest saved:" : "Script saved to your home folder:"}
            </p>
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
              <CopyButton
                text={pendingDryRun.scriptPath}
                what="script path"
                title="Copy path"
              />
              {pendingDryRun.manifestPath && pendingDryRun.manifestName && (
                <>
                  <span className="modal__job-param-label">Manifest:</span>
                  <button
                    className="modal__dry-run-name"
                    title={pendingDryRun.manifestPath}
                    onClick={() => {
                      if (otherPanel) {
                        client.sendIntent("NavigatePanelTo", undefined, {
                          panelId: otherPanel,
                          path: pendingDryRun.scriptDir,
                          cursorHint: pendingDryRun.manifestName,
                        });
                      }
                      closeDryRun();
                    }}
                  >{pendingDryRun.manifestName}</button>
                  <CopyButton
                    text={pendingDryRun.manifestPath!}
                    what="manifest path"
                    title="Copy path"
                  />
                </>
              )}
            </div>
          </div>
          <div className="modal__actions">
            <button className="modal__btn modal__btn--primary" data-command-id="dry-run.close" onClick={closeDryRun}>Close</button>
          </div>
        </ResizableModal>
      </ModalLayer>
    );
  }

  // ── Phase 1: Confirmation ───────────────────────────────────────────────
  if (pending) {
    const fileCount = pending.files.length;
    const isDelete = pending.opType === "delete";
    const sourcePanelState = state.panels[pending.sourcePanel];
    const isArchiveSource = !!sourcePanelState?.shadowSource && isArchiveSourceId(sourcePanelState.sourceId);
    const isGlobalSelectionDelete = isDelete
      && !!sourcePanelState
      && isGlobalSelectionSourceId(sourcePanelState.sourceId);
    const label = isArchiveSource && pending.opType === "copy" ? "Extract" : opLabel(pending.opType);
    const canStartInBackground = !dryMode;
    const canQueue = supportsQueuedLocalCopy(state, pending, dryMode);
    const canSuggestResume = canSuggestResumeForPending(state, pending);
    const conflictPolicies: ConflictPolicy[] = canSuggestResume
      ? ["ask", "resume", "overwrite", "skip"]
      : ["ask", "overwrite", "skip"];
    const selectedConflictPolicy = conflictPolicies.includes(conflictPolicy) ? conflictPolicy : "ask";
    const showDestNameInput = supportsCopyDestName(pending);
    const destNameWarning = showDestNameInput ? pendingDestNameIssue?.message ?? null : null;
    const isSubmitted = submitted && submittedPendingRef.current === pending;
    const canSubmitPending = !isSubmitted && !destNameWarning;

    const startOp = (background: boolean, enqueue = false) => {
      if (!canSubmitPending) return;
      submittedPendingRef.current = pending;
      setSubmitted(true);
      if (isGlobalSelectionDelete && sourcePanelState) {
        if (sourcePanelState.path) {
          client.sendIntent("RemoveGlobalSelectionItems", undefined, {
            panelId: pending.sourcePanel,
            selectionName: sourcePanelState.path,
            paths: pending.files.map((file) => file.sourcePath ?? file.name),
          });
        } else {
          client.sendIntent("RemoveGlobalSelections", undefined, {
            names: pending.files.map((file) => file.name),
          });
        }
        close();
        return;
      }
      client.sendIntent("StartFileOp", undefined, {
        opType: pending.opType,
        files: pending.files,
        sourcePanel: pending.sourcePanel,
        destPanel: pending.destPanel,
        destPath: pending.destPath,
        ...(pending.sourceId ? { sourceId: pending.sourceId } : {}),
        ...(showDestNameInput && destName !== pending.files[0].name ? { destName } : {}),
        conflictPolicy: selectedConflictPolicy,
        verificationPolicy,
        saveDigestFile,
        background: background || enqueue,
        ...(enqueue ? { enqueue: true } : {}),
        ...(background && startPaused ? { startPaused: true } : {}),
        ...(dryMode ? { dryMode: true } : {}),
      });
      if (background || enqueue) {
        close();
      }
      // The reducer will transition to activeJobId when JobStarted arrives
    };

    const cycleConflictPolicy = () => {
      if (isDelete || isSubmitted) return;
      const index = conflictPolicies.indexOf(selectedConflictPolicy);
      const next = conflictPolicies[(index + 1 + conflictPolicies.length) % conflictPolicies.length];
      setConflictPolicy(next);
    };

    const handlePendingKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (e.defaultPrevented) return;
      if (e.key === "Escape") {
        e.preventDefault();
        close();
        return;
      }
      if (isNativeActivationTarget(e.target) && !isEditableTarget(e.target)) return;
      if (isEditableTarget(e.target) && e.key !== "Enter") return;
      if (e.key === "Enter") {
        e.preventDefault();
        startOp(false);
        return;
      }
      if (isLetterKey(e, "d") && !isSubmitted) {
        e.preventDefault();
        setDryMode((current) => !current);
        return;
      }
      if (isLetterKey(e, "p") && canStartInBackground && !isSubmitted) {
        e.preventDefault();
        setStartPaused((current) => !current);
        return;
      }
      if (isLetterKey(e, "a")) {
        e.preventDefault();
        cycleConflictPolicy();
        return;
      }
      if (canStartInBackground && isLetterKey(e, "b")) {
        e.preventDefault();
        startOp(true);
      }
    };

    return (
      <ModalLayer modalId={`operation:${pending.opType}`} ref={modalRef} onClick={close} onKeyDown={handlePendingKeyDown}>
        <ResizableModal
          modalId={`operation:${pending.opType}`}
          client={client}
          className="modal--fixed"
          ariaLabel={label}
          testId="operation-dialog"
          defaultSize={{ width: 560, height: 380 }}
          minSize={{ width: 420, height: 280 }}
        >
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
                      data-testid="copy-save-digest-file-input"
                      checked={saveDigestFile}
                      onChange={(e) => setSaveDigestFileState({ pendingKey, value: e.target.checked })}
                    />
                    {" "}Save BLAKE3 digest file
                  </label>
                </div>
                {showDestNameInput && (
                  <>
                    <div className="modal__option modal__option--field">
                      <label htmlFor="copy-destination-name">Destination name:</label>
                      <input
                        ref={destNameInputRef}
                        id="copy-destination-name"
                        className="modal__rename-input modal__copy-name-input"
                        data-testid="copy-destination-name-input"
                        value={destName}
                        onChange={(e) => setDestName(e.target.value)}
                      />
                    </div>
                    {destNameWarning && (
                      <div className="modal__warning" role="alert">
                        <IconAlertTriangle size={15} />
                        <span>{destNameWarning}</span>
                      </div>
                    )}
                  </>
                )}
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
            <button className="modal__btn modal__btn--primary" data-command-id={`file.${pending.opType}.confirm`} disabled={!canSubmitPending} onClick={() => startOp(false)}>
              {isSubmitted ? "Starting…" : label}
            </button>
            {canStartInBackground && (
              <button className="modal__btn" data-command-id={`file.${pending.opType}.background`} disabled={!canSubmitPending} onClick={() => startOp(true)}>
                <MnemonicLabel text="Background" mnemonic="b" />
              </button>
            )}
            {canQueue && (
              <button className="modal__btn" data-command-id={`file.${pending.opType}.queue`} disabled={!canSubmitPending} onClick={() => startOp(false, true)}>
                Queue
              </button>
            )}
            <button className="modal__btn" data-command-id={`file.${pending.opType}.cancel`} onClick={close}>
              Cancel
            </button>
          </div>
        </ResizableModal>
      </ModalLayer>
    );
  }

  // ── Phase 2 & 3: Progress / Decision ─────────────────────────────────────
  if (activeJob) {
    const isTerminal = activeJob.state === "Completed" || activeJob.state === "Cancelled" || activeJob.state === "Failed";
    const isDecision = activeJob.state === "AwaitingDecision";
    const isQueued = activeJob.state === "Queued";
    const isProgress = activeJob.state === "Running";
    const isBroResult = activeJob.kind === "fp-bro" || activeJob.kind === "sync-bro" || activeJob.kind === "fedup-bro";
    const isSyncBro = activeJob.kind === "sync-bro";
    const isFedupBro = activeJob.kind === "fedup-bro";
    const syncResult = activeJob.syncResult;
    const fedupResult = activeJob.fedupResult;
    // For Sync Bro the report file is written only on demand; the result path
    // exists only once the user has saved it. fp-bro produces a file directly.
    const effectiveResultPath = isSyncBro || isFedupBro ? activeJob.reportSavedPath : activeJob.resultPath;
    const requiresExplicitClose = activeJob.kind === "sync-bro" || activeJob.kind === "fedup-bro";
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
    const showSkipCurrentFile = activeJob.kind === "file-op"
      && activeJob.type === "copy"
      && !activeSourcePanel?.shadowSource;
    const canSkipCurrentFile = showSkipCurrentFile
      && isProgress
      && !activeJob.paused
      && !!p?.currentFile;
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

    const skipCurrentFile = () => {
      if (!activeJobId || !canSkipCurrentFile) return;
      client.sendIntent("SkipCurrentFile", undefined, { jobId: activeJobId });
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

    const closeActiveJob = () => {
      client.sendIntent("RefreshPanels");
      close();
    };

    const showResult = () => {
      if (!effectiveResultPath) return;
      const { parent, name } = splitParentAndName(effectiveResultPath);
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

    const saveSyncReport = () => {
      if (!syncResult) return;
      client.sendIntent("SaveSyncReport", undefined, {
        jobId: activeJob.jobId,
        result: syncResult,
      });
    };

    const saveFedupReport = () => {
      if (!fedupResult) return;
      client.sendIntent("SaveFedupReport", undefined, {
        jobId: activeJob.jobId,
      });
    };

    const generateFedupDeleteDryRun = () => {
      if (!fedupResult) return;
      client.sendIntent("GenerateFedupDeleteDryRun", undefined, {
        jobId: activeJob.jobId,
      });
    };

    const handleActiveJobKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
      if (e.defaultPrevented) return;
      if (e.key === "Escape") {
        e.preventDefault();
        if (confirmingCancel) setConfirmingCancel(false);
        else close();
        return;
      }
      if (isNativeActivationTarget(e.target)) return;
      if (isDecision) {
        if (isLetterKey(e, "a") && activeJob.decision?.supportsApplyToAll) {
          e.preventDefault();
          setApplyToAll((current) => !current);
          return;
        }
        if (e.key === "Enter" && primaryDecision) {
          e.preventDefault();
          submitDecision(primaryDecision);
          return;
        }
        if (isLetterKey(e, "s") && skipDecision) {
          e.preventDefault();
          submitDecision(skipDecision);
          return;
        }
      }
      if (isQueued || isProgress) {
        if (confirmingCancel) {
          if (e.key === "Enter") {
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
        if (isLetterKey(e, "r") && activeJob.supportsPause && activeJob.paused) {
          e.preventDefault();
          resumeJob();
          return;
        }
      }
      if (e.key === "Enter") {
        e.preventDefault();
        if (isTerminal) {
          if (requiresExplicitClose) {
            closeActiveJob();
          } else {
            close();
          }
        }
      }
    };

    return (
      <ModalLayer modalId={`operation:${activeJob.kind}`} ref={modalRef} onClick={isTerminal && !requiresExplicitClose ? close : undefined} onKeyDown={handleActiveJobKeyDown}>
        <ResizableModal
          modalId={isSyncBro || isFedupBro ? `operation:${activeJob.kind}:result` : `operation:${activeJob.kind}`}
          client={client}
          className={`modal--fixed${isSyncBro || isFedupBro ? " modal--sync" : ""}`}
          ariaLabel={`${label} ${activeJob.state}`}
          testId="job-dialog"
          defaultSize={isSyncBro || isFedupBro ? { width: 640, height: 460 } : { width: 560, height: 380 }}
          minSize={isSyncBro || isFedupBro ? { width: 520, height: 340 } : { width: 420, height: 280 }}
        >
          <div className="modal__header">{label} — {activeJob.state}</div>
          <div className="modal__body">
            <div className="modal__job-params">
              {isOutgoing ? (
                <>
                  <span className="modal__job-param-label">From:</span>
                  <span className="modal__job-param-value" title={activeJob.sourcePath}>{activeJob.sourcePath}</span>
                  <span className="modal__job-param-label">To:</span>
                  <span className="modal__job-param-value">Destination selected on the receiving fbro</span>
                </>
              ) : isBroResult ? (
                <>
                  <span className="modal__job-param-label">Inputs:</span>
                  <span className="modal__job-param-value" title={activeJob.sourcePath}>{activeJob.sourcePath}</span>
                  <CopyButton text={activeJob.sourcePath} what="source path" title="Copy path" />
                  {activeJob.resultPath && (<>
                    <span className="modal__job-param-label">Result:</span>
                    <span className="modal__job-param-value" title={activeJob.resultPath}>{activeJob.resultPath}</span>
                    <CopyButton text={activeJob.resultPath!} what="result path" title="Copy path" />
                  </>)}
                </>
              ) : (
                <>
                  <span className="modal__job-param-label">From:</span>
                  <span className="modal__job-param-value" title={activeJob.sourcePath}>{activeJob.sourcePath}</span>
                  <CopyButton text={activeJob.sourcePath} what="source path" title="Copy path" />
                  {activeJob.destPath && (<>
                    <span className="modal__job-param-label">To:</span>
                    <span className="modal__job-param-value" title={activeJob.destPath}>{activeJob.destPath}</span>
                    <CopyButton text={activeJob.destPath!} what="destination path" title="Copy path" />
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

                {/* File currently being hashed (the status label shows tree-level
                    progress). Two lines are reserved so the layout stays steady. */}
                {isSyncBro && isProgress && (
                  <div className="modal__sync-current">
                    <span className="modal__sync-current-label">Hashing:</span>
                    <span className="modal__sync-current-path" title={p.currentFile ?? undefined}>
                      {p.currentFile ?? "…"}
                    </span>
                    {p.currentFile && (
                      <CopyButton
                        text={p.currentFile!}
                        what="path"
                        title="Copy path"
                      />
                    )}
                  </div>
                )}

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
                {activeJob.state === "Completed" && (isOutgoing ? "Outgoing transfer completed." : isFedupBro ? "Fedup Bro report completed." : isBroResult ? "Bro completed successfully." : "Operation completed successfully.")}
                  {activeJob.state === "Cancelled" && (isOutgoing ? "Outgoing transfer was cancelled." : isBroResult ? "Bro job was cancelled." : "Operation was cancelled.")}
                  {activeJob.state === "Failed" && (isOutgoing ? `Outgoing transfer failed: ${activeJob.error ?? "unknown error"}` : isBroResult ? `Bro failed: ${activeJob.error ?? "unknown error"}` : `Operation failed: ${activeJob.error ?? "unknown error"}`)}
                </p>
                {isSyncBro && activeJob.state === "Completed" && syncResult && (
                  <SyncBroResultsView result={syncResult} />
                )}
                {isFedupBro && activeJob.state === "Completed" && fedupResult && (
                  <FedupBroResultsView jobId={activeJob.jobId} result={fedupResult} client={client} />
                )}
                {isBroResult && activeJob.state === "Completed" && effectiveResultPath && (
                  <div className="modal__job-params">
                    <span className="modal__job-param-label">{isSyncBro || isFedupBro ? "Saved:" : "Output:"}</span>
                    <button
                      className="modal__dry-run-name"
                      title={effectiveResultPath}
                      onClick={showResult}
                    >{splitParentAndName(effectiveResultPath).name}</button>
                    <CopyButton
                      text={effectiveResultPath}
                      what="result path"
                      title="Copy path"
                    />
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
                {showSkipCurrentFile && (
                  <button className="modal__btn" data-command-id="job.skip-current-file" onClick={skipCurrentFile} disabled={!canSkipCurrentFile}>
                    Skip
                  </button>
                )}
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
                {isSyncBro && activeJob.state === "Completed" && syncResult && !activeJob.reportSavedPath && (
                  <button className="modal__btn" data-command-id="job.save-report" onClick={saveSyncReport}>
                    Save report…
                  </button>
                )}
                {isFedupBro && activeJob.state === "Completed" && fedupResult && !activeJob.reportSavedPath && (
                  <button className="modal__btn" data-command-id="job.save-report" onClick={saveFedupReport}>
                    Save report…
                  </button>
                )}
                {isFedupBro && activeJob.state === "Completed" && fedupResult && fedupResult.duplicateFiles > 0 && (
                  <button className="modal__btn" data-command-id="job.fedup-delete-dry-run" onClick={generateFedupDeleteDryRun}>
                    Generate delete dry-run
                  </button>
                )}
                {isBroResult && activeJob.state === "Completed" && effectiveResultPath && (
                  <button className="modal__btn" data-command-id="job.show-result" onClick={showResult}>
                    Show Result
                  </button>
                )}
                {isBroResult && effectiveResultPath && (
                  <button className="modal__btn" data-command-id="job.copy-path" onClick={() => copyToClipboard(effectiveResultPath, "result path")}>
                    Copy Path
                  </button>
                )}
                <button className="modal__btn modal__btn--primary" data-command-id="job.close" onClick={closeActiveJob}>
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
        </ResizableModal>
      </ModalLayer>
    );
  }

  return null;
}
