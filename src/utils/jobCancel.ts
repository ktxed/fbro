import type { WsClient } from "../ws/client";
import type { Job } from "../ws/types";
import type { AppState } from "../store";

export function isRemoteCopyJob(state: AppState, job: Job): boolean {
  const sourcePanel = state.panels[job.sourcePanel];
  const source = sourcePanel ? state.sources[sourcePanel.sourceId] : undefined;
  return job.kind === "file-op" && job.type === "copy" && source?.kind === "remote";
}

export function cancelJobWithPartialPrompt(client: WsClient, state: AppState, job: Job) {
  if (!isRemoteCopyJob(state, job)) {
    client.sendIntent("CancelJob", undefined, { jobId: job.jobId });
    return;
  }

  if (!window.confirm("Cancel this remote copy?")) return;
  const keepPartialFiles = window.confirm("Keep partially copied files so the transfer can be resumed later?");
  client.sendIntent("CancelJob", undefined, { jobId: job.jobId, keepPartialFiles });
}
