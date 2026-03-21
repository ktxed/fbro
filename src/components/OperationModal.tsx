import { useState } from "react";
import type { WsClient } from "../ws/client";
import type { PendingOp, Job, ConflictPolicy } from "../ws/types";
import { useAppDispatch } from "../store";

interface OperationModalProps {
  pending: PendingOp | null;
  activeJob: Job | null;
  activeJobId: string | null;
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

export function OperationModal({ pending, activeJob, activeJobId, client }: OperationModalProps) {
  const dispatch = useAppDispatch();
  const [conflictPolicy, setConflictPolicy] = useState<ConflictPolicy>("ask");
  const [submitted, setSubmitted] = useState(false);

  const close = () => { setSubmitted(false); dispatch({ type: "CLOSE_OPERATION" }); };

  // Nothing to show
  if (!pending && !activeJob) return null;

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
      });
      if (background) {
        close();
      }
      // The reducer will transition to activeJobId when JobStarted arrives
    };

    return (
      <div className="modal-overlay" onClick={close}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
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
          </div>
          <div className="modal__actions">
            <button className="modal__btn modal__btn--primary" disabled={submitted} onClick={() => startOp(false)}>
              {submitted ? "Starting…" : label}
            </button>
            <button className="modal__btn" disabled={submitted} onClick={() => startOp(true)}>
              Background
            </button>
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
    const p = activeJob.progress;
    const label = opLabel(activeJob.type);

    const filePercent = p && p.currentFileBytesTotal > 0
      ? Math.round((p.currentFileBytesCopied / p.currentFileBytesTotal) * 100)
      : 0;
    const jobPercent = p && p.totalFiles > 0
      ? Math.round((p.completedFiles / p.totalFiles) * 100)
      : 0;

    return (
      <div className="modal-overlay" onClick={isTerminal ? close : undefined}>
        <div className="modal" onClick={(e) => e.stopPropagation()}>
          <div className="modal__header">{label} — {activeJob.state}</div>
          <div className="modal__body">
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
                {p.currentFile && (
                  <div className="modal__progress-section">
                    <span className="modal__progress-label">{p.currentFile}</span>
                    <div className="modal__progress-bar">
                      <div
                        className="modal__progress-fill"
                        style={{ width: `${filePercent}%` }}
                      />
                    </div>
                    <span className="modal__progress-text">
                      {filePercent}% ({formatBytes(p.currentFileBytesCopied)} / {formatBytes(p.currentFileBytesTotal)})
                    </span>
                  </div>
                )}

                {/* Job-wide progress (only if > 1 file) */}
                {p.totalFiles > 1 && (
                  <div className="modal__progress-section">
                    <span className="modal__progress-label">Overall</span>
                    <div className="modal__progress-bar">
                      <div
                        className="modal__progress-fill"
                        style={{ width: `${jobPercent}%` }}
                      />
                    </div>
                    <span className="modal__progress-text">
                      {jobPercent}% ({p.completedFiles} / {p.totalFiles} files)
                    </span>
                  </div>
                )}
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
              <button className="modal__btn" onClick={() =>
                client.sendIntent("CancelJob", undefined, { jobId: activeJobId })
              }>
                Cancel
              </button>
            )}
            {isTerminal && (
              <button className="modal__btn modal__btn--primary" onClick={close}>
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
