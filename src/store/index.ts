import { createContext, useContext, useReducer, type Dispatch } from "react";
import type {
  PanelState,
  Source,
  Job,
  BEMessage,
  StateSnapshotMessage,
  StatePatchMessage,
  JobStartedMessage,
  JobEventMessage,
  PendingOp,
  QuitConfirmRequiredMessage,
} from "../ws/types";

// ─── State ───────────────────────────────────────────────────────────────────

export interface AppState {
  connected: boolean;
  sessionId: string | null;
  layout: { panels: string[] };
  panels: Record<string, PanelState>;
  sources: Record<string, Source>;
  focus: string;
  jobs: Record<string, Job>;
  pendingOp: PendingOp | null;
  activeJobId: string | null;
  pendingQuit: { activeJobs: number } | null;
}

const initialState: AppState = {
  connected: false,
  sessionId: null,
  layout: { panels: [] },
  panels: {},
  sources: {},
  focus: "",
  jobs: {},
  pendingOp: null,
  activeJobId: null,
  pendingQuit: null,
};

// ─── Actions ─────────────────────────────────────────────────────────────────

export type AppAction =
  | { type: "CONNECTED" }
  | { type: "DISCONNECTED" }
  | { type: "BE_MESSAGE"; msg: BEMessage }
  | { type: "OPEN_OPERATION"; op: PendingOp }
  | { type: "CLOSE_OPERATION" }
  | { type: "SET_ACTIVE_JOB"; jobId: string }
  | { type: "CANCEL_QUIT" };

// ─── Reducer ─────────────────────────────────────────────────────────────────

function applySnapshot(state: AppState, msg: StateSnapshotMessage): AppState {
  const { domains } = msg;
  let next = { ...state };

  if (domains.layout) {
    next.layout = { panels: domains.layout.panels };
  }
  if (domains.focus !== undefined) {
    next.focus = domains.focus;
  }
  if (domains.sources) {
    const sources: Record<string, Source> = {};
    for (const s of domains.sources) sources[s.id] = s;
    next.sources = sources;
  }
  if (domains.panels) {
    const panels: Record<string, PanelState> = {};
    for (const [id, p] of Object.entries(domains.panels)) {
      panels[id] = { ...p, panelId: id, entries: p.entries ?? [], selection: p.selection ?? [], version: p.version ?? 0 };
    }
    next.panels = panels;
  }
  if (domains.jobs) {
    const jobs: Record<string, Job> = {};
    for (const j of domains.jobs) jobs[j.jobId] = j;
    next.jobs = jobs;
  }
  return next;
}

function applyPatch(state: AppState, msg: StatePatchMessage): AppState {
  if (msg.domain === "panels" && msg.entityId) {
    const panelId = msg.entityId;
    const panel = state.panels[panelId];
    if (!panel) return state;

    // Version guard — if version goes backwards skip (shouldn't happen in v0)
    if (msg.version <= panel.version && msg.version !== 0) return state;

    let updated = { ...panel, version: msg.version };

    if (msg.patch.appendEntries) {
      updated = { ...updated, entries: [...updated.entries, ...msg.patch.appendEntries] };
    }
    if (msg.patch.status !== undefined) {
      updated = { ...updated, status: msg.patch.status };
    }
    if (msg.patch.cursor !== undefined) {
      updated = { ...updated, cursor: msg.patch.cursor };
    }
    if (msg.patch.path !== undefined) {
      updated = { ...updated, path: msg.patch.path };
    }
    if (msg.patch.entries !== undefined) {
      updated = { ...updated, entries: msg.patch.entries };
    }
    if (msg.patch.selection !== undefined) {
      updated = { ...updated, selection: msg.patch.selection };
    }

    return { ...state, panels: { ...state.panels, [panelId]: updated } };
  }

  if (msg.domain === "focus") {
    return { ...state, focus: msg.patch.focus as string };
  }

  return state;
}

function applyJobStarted(state: AppState, msg: JobStartedMessage): AppState {
  const { job } = msg;
  const newJob: Job = {
    jobId: job.jobId,
    type: job.opType,
    state: "Running",
    sourcePath: job.sourcePath,
    destPath: job.destPath,
    progress: {
      totalFiles: job.totalFiles,
      completedFiles: 0,
      currentFileBytesTotal: 0,
      currentFileBytesCopied: 0,
      totalBytesTotal: job.totalBytesTotal,
      totalBytesCopied: 0,
    },
  };
  return {
    ...state,
    jobs: { ...state.jobs, [job.jobId]: newJob },
    activeJobId: job.background ? state.activeJobId : (state.activeJobId ?? job.jobId),
    pendingOp: null,
  };
}

function applyQuitConfirmRequired(state: AppState, msg: QuitConfirmRequiredMessage): AppState {
  return { ...state, pendingQuit: { activeJobs: msg.activeJobs } };
}

function applyJobEvent(state: AppState, msg: JobEventMessage): AppState {
  const job = state.jobs[msg.jobId];
  if (!job) return state;

  let updated: Job;

  switch (msg.event) {
    case "Progress":
      updated = { ...job, state: "Running", progress: msg.progress ?? job.progress, decision: undefined };
      break;
    case "DecisionRequired":
      updated = { ...job, state: "AwaitingDecision", decision: msg.decision };
      break;
    case "Completed":
      updated = { ...job, state: "Completed", decision: undefined };
      break;
    case "Failed":
      updated = { ...job, state: "Failed", error: msg.error, decision: undefined };
      break;
    case "Cancelled":
      updated = { ...job, state: "Cancelled", decision: undefined };
      break;
    default:
      return state;
  }

  return { ...state, jobs: { ...state.jobs, [msg.jobId]: updated } };
}

function reducer(state: AppState, action: AppAction): AppState {
  switch (action.type) {
    case "CONNECTED":
      return { ...state, connected: true };
    case "DISCONNECTED":
      return { ...state, connected: false, sessionId: null };
    case "BE_MESSAGE": {
      const msg = action.msg;
      switch (msg.type) {
        case "Welcome":
          return { ...state, sessionId: msg.sessionId };
        case "StateSnapshot":
          return applySnapshot(state, msg);
        case "StatePatch":
          return applyPatch(state, msg);
        case "JobStarted":
          return applyJobStarted(state, msg);
        case "JobEvent":
          return applyJobEvent(state, msg);
        case "Error":
          console.error("BE error:", msg.code, msg.message);
          return state;
        case "QuitConfirmRequired":
          return applyQuitConfirmRequired(state, msg);
        default:
          return state;
      }
    }
    case "OPEN_OPERATION":
      return { ...state, pendingOp: action.op };
    case "CLOSE_OPERATION":
      return { ...state, pendingOp: null, activeJobId: null };
    case "SET_ACTIVE_JOB":
      return { ...state, activeJobId: action.jobId, pendingOp: null };
    case "CANCEL_QUIT":
      return { ...state, pendingQuit: null };
  }
}

// ─── Context ─────────────────────────────────────────────────────────────────

export const AppContext = createContext<{
  state: AppState;
  dispatch: Dispatch<AppAction>;
}>({ state: initialState, dispatch: () => {} });

export function useAppState() {
  return useContext(AppContext).state;
}

export function useAppDispatch() {
  return useContext(AppContext).dispatch;
}

export function useAppReducer() {
  return useReducer(reducer, initialState);
}
