import { useState } from "react";
import { useAppState, useAppDispatch } from "../store";
import type { WsClient } from "../ws/client";
import { IconChevronLeft, IconChevronRight, IconListDetails, IconX } from "@tabler/icons-react";
import { cancelJobWithPartialPrompt } from "../utils/jobCancel";

const VISIBLE = 5;

function jobShortId(jobId: string) {
  return jobId.replace(/^job-/, "#");
}

export function JobsBar({ client, onOpenJobsView }: { client: WsClient; onOpenJobsView: () => void }) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const [scrollIndex, setScrollIndex] = useState(0);
  const queuedJobIds = new Set(state.jobQueue.jobIds);
  const queueActiveJob = state.jobQueue.activeJobId ? state.jobs[state.jobQueue.activeJobId] : undefined;
  const queueTotal = state.jobQueue.completedCount + state.jobQueue.jobIds.length;
  const queueCurrent = queueTotal > 0 ? state.jobQueue.completedCount + 1 : 0;

  const activeJobs = Object.values(state.jobs).filter(
    (j) =>
      !queuedJobIds.has(j.jobId) &&
      (j.state === "Queued" ||
      j.state === "Running" ||
      j.state === "AwaitingDecision" ||
      (j.kind === "search" &&
        (j.state === "Completed" || j.state === "Cancelled" || j.state === "Failed")))
  );

  if (activeJobs.length === 0 && queueTotal === 0) return null;

  const clamped = Math.min(scrollIndex, Math.max(0, activeJobs.length - VISIBLE));
  const visible = activeJobs.slice(clamped, clamped + VISIBLE);
  const canLeft = clamped > 0;
  const canRight = clamped + VISIBLE < activeJobs.length;

  const onWheel = (e: React.WheelEvent) => {
    if (activeJobs.length <= VISIBLE) return;
    e.preventDefault();
    const delta = e.deltaY !== 0 ? e.deltaY : e.deltaX;
    setScrollIndex(Math.max(0, Math.min(activeJobs.length - VISIBLE, clamped + (delta > 0 ? 1 : -1))));
  };

  return (
    <div className="jobs-bar" onWheel={onWheel}>
      <button
        className="jobs-bar__arrow"
        disabled={!canLeft}
        onClick={() => setScrollIndex(clamped - 1)}
      >
        <IconChevronLeft size={14} />
      </button>

      {queueTotal > 0 && (
        <button
          className="jobs-bar__job jobs-bar__job--queue"
          onClick={() => {
            if (queueActiveJob) dispatch({ type: "SET_ACTIVE_JOB", jobId: queueActiveJob.jobId });
          }}
          title="Open current queued job"
        >
          <span className="jobs-bar__job-label">
            Queue{queueActiveJob ? `: ${queueActiveJob.type.charAt(0).toUpperCase() + queueActiveJob.type.slice(1)}` : ""}
          </span>
          <span className="jobs-bar__job-pct">{queueCurrent} / {queueTotal}</span>
          <span
            className="jobs-bar__job-close"
            role="button"
            title="Abort current queued job"
            onClick={(e) => {
              e.stopPropagation();
              if (queueActiveJob) cancelJobWithPartialPrompt(client, state, queueActiveJob);
            }}
          >
            <IconX size={12} />
          </span>
          <span
            className="jobs-bar__job-close jobs-bar__job-open"
            role="button"
            title="Manage queue"
            onClick={(e) => {
              e.stopPropagation();
              onOpenJobsView();
            }}
          >
            <IconListDetails size={12} />
          </span>
        </button>
      )}

      {visible.map((job) => {
        const isActive = job.jobId === state.activeJobId;
        const p = job.progress;
        const pct = job.state === "Queued"
          ? null
          : p && p.totalFiles > 0
          ? Math.round((p.completedFiles / p.totalFiles) * 100)
          : null;
        const typeLabel = job.kind === "search"
          ? "Search"
          : job.kind === "outgoing-transfer"
          ? `Outgoing ${jobShortId(job.jobId)}`
          : job.kind === "remote-manifest"
          ? "Remote Sync"
          : job.kind === "sync-bro"
          ? "Sync Bro"
          : job.kind === "fedup-bro"
          ? "Fedup Bro"
          : job.type.charAt(0).toUpperCase() + job.type.slice(1);
        const searchPct = job.kind === "search" && job.searchMatched !== undefined
          ? `${job.searchMatched} hits`
          : null;

        return (
          <button
            key={job.jobId}
            className={`jobs-bar__job${isActive ? " jobs-bar__job--active" : ""}`}
            onClick={() => dispatch({ type: "SET_ACTIVE_JOB", jobId: job.jobId })}
          >
            <span className="jobs-bar__job-label">{typeLabel}</span>
            {job.state === "Queued" && <span className="jobs-bar__job-pct">Queued</span>}
            {searchPct !== null && <span className="jobs-bar__job-pct">{searchPct}</span>}
            {pct !== null && searchPct === null && <span className="jobs-bar__job-pct">{pct}%</span>}
            <span
              className="jobs-bar__job-close"
              role="button"
              title={job.kind === "search" && job.state !== "Running" && job.state !== "Queued" ? "Discard results" : "Cancel job"}
              onClick={(e) => {
                e.stopPropagation();
                if (job.kind === "search" && job.state !== "Running" && job.state !== "Queued") {
                  client.sendIntent("RemoveSearchSource", undefined, { sourceId: job.searchSourceId });
                } else {
                  cancelJobWithPartialPrompt(client, state, job);
                }
              }}
              >
                <IconX size={12} />
              </span>
          </button>
        );
      })}

      <button
        className="jobs-bar__arrow"
        disabled={!canRight}
        onClick={() => setScrollIndex(clamped + 1)}
      >
        <IconChevronRight size={14} />
      </button>
    </div>
  );
}
