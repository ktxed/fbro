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
  VFSEntry,
} from "../ws/types";

// ─── State ───────────────────────────────────────────────────────────────────

export interface PendingRename {
  sourcePanel: string;
  sourcePath: string; // full path of the file being renamed
  dirPath: string;    // directory containing the file
  filename: string;   // current filename
  error?: string;     // set when BE reports RENAME_FAILED
}

export interface PendingInfo {
  name: string;
  kind: string;
  fullPath: string;
  dirPath: string;
  size?: number;
  mtime?: string;
  btime?: string; // birth time — file creation; not available on all platforms/filesystems
  permissions?: string;
}

export interface PendingMkdir {
  sourcePanel: string;
  dirPath: string;
  error?: string;
}

export interface PendingNoEditor {
  filename: string;
  fullPath: string;
}

export interface AppState {
  connected: boolean;
  sessionId: string | null;
  fileServerPort: number;
  layout: { panels: string[] };
  panels: Record<string, PanelState>;
  sources: Record<string, Source>;
  focus: string;
  jobs: Record<string, Job>;
  pendingOp: PendingOp | null;
  activeJobId: string | null;
  pendingQuit: { activeJobs: number } | null;
  pendingRename: PendingRename | null;
  pendingInfo: PendingInfo | null;
  pendingMkdir: PendingMkdir | null;
  pendingNoEditor: PendingNoEditor | null;
}

const initialState: AppState = {
  connected: false,
  sessionId: null,
  fileServerPort: 0,
  layout: { panels: [] },
  panels: {},
  sources: {},
  focus: "",
  jobs: {},
  pendingOp: null,
  activeJobId: null,
  pendingQuit: null,
  pendingRename: null,
  pendingInfo: null,
  pendingMkdir: null,
  pendingNoEditor: null,
};

// ─── Actions ─────────────────────────────────────────────────────────────────

export type AppAction =
  | { type: "CONNECTED" }
  | { type: "DISCONNECTED" }
  | { type: "BE_MESSAGE"; msg: BEMessage }
  | { type: "OPEN_OPERATION"; op: PendingOp }
  | { type: "CLOSE_OPERATION" }
  | { type: "SET_ACTIVE_JOB"; jobId: string }
  | { type: "CANCEL_QUIT" }
  | { type: "OPEN_RENAME"; rename: PendingRename }
  | { type: "CLOSE_RENAME" }
  | { type: "OPEN_INFO"; info: PendingInfo }
  | { type: "CLOSE_INFO" }
  | { type: "OPEN_MKDIR"; mkdir: PendingMkdir }
  | { type: "CLOSE_MKDIR" }
  | { type: "OPEN_NO_EDITOR"; noEditor: PendingNoEditor }
  | { type: "CLOSE_NO_EDITOR" };

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
      panels[id] = { ...p, panelId: id, entries: p.entries ?? [], selection: p.selection ?? [], version: p.version ?? 0, calculatingSize: p.calculatingSize ?? [], totalCount: p.totalCount, windowStart: p.windowStart };
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

    // Some fields are applied before the version guard because a dirsize try_send
    // can fire between the BE's state-lock release and its tx.send, putting a
    // higher-versioned dirsize patch in the channel ahead of the cursor/selection
    // patch.  These fields are safe to apply out-of-order: they carry no stale
    // structural data and WebSocket ordering still prevents truly stale values
    // from a previous navigation session.
    let preGuardPanel = panel;
    if (msg.patch.updateEntrySize !== undefined) {
      const { name, size } = msg.patch.updateEntrySize as { name: string; size: number };
      preGuardPanel = { ...preGuardPanel, entries: preGuardPanel.entries.map(e => e.name === name ? { ...e, size } : e) };
    }
    if (msg.patch.cursor !== undefined) {
      preGuardPanel = { ...preGuardPanel, cursor: msg.patch.cursor as number };
    }
    if (msg.patch.selection !== undefined) {
      preGuardPanel = { ...preGuardPanel, selection: msg.patch.selection as number[] };
    }
    if (msg.patch.calculatingSize !== undefined) {
      preGuardPanel = { ...preGuardPanel, calculatingSize: msg.patch.calculatingSize as string[] };
    }
    if (preGuardPanel !== panel) {
      state = { ...state, panels: { ...state.panels, [panelId]: preGuardPanel } };
    }

    // Version guard — drop structurally stale patches (old scan data arriving
    // after a newer scan has already produced results).
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
    if (msg.patch.sort !== undefined) {
      updated = { ...updated, sort: msg.patch.sort };
    }
    if (msg.patch.selection !== undefined) {
      updated = { ...updated, selection: msg.patch.selection };
    }
    if (msg.patch.sourceId !== undefined) {
      updated = { ...updated, sourceId: msg.patch.sourceId as string };
    }
    if (msg.patch.calculatingSize !== undefined) {
      updated = { ...updated, calculatingSize: msg.patch.calculatingSize as string[] };
    }
    if (msg.patch.updateEntrySize !== undefined) {
      const { name, size } = msg.patch.updateEntrySize as { name: string; size: number };
      updated = { ...updated, entries: updated.entries.map(e => e.name === name ? { ...e, size } : e) };
    }
    if (msg.patch.totalCount !== undefined) {
      updated = { ...updated, totalCount: msg.patch.totalCount === null ? undefined : msg.patch.totalCount as number };
    }
    if (msg.patch.windowStart !== undefined) {
      updated = { ...updated, windowStart: msg.patch.windowStart === null ? undefined : msg.patch.windowStart as number };
    }
    if (msg.patch.windowEntries !== undefined) {
      updated = { ...updated, entries: msg.patch.windowEntries as VFSEntry[] };
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
          return { ...state, sessionId: msg.sessionId, fileServerPort: msg.fileServerPort };
        case "StateSnapshot":
          return applySnapshot(state, msg);
        case "StatePatch":
          return applyPatch(state, msg);
        case "JobStarted":
          return applyJobStarted(state, msg);
        case "JobEvent":
          return applyJobEvent(state, msg);
        case "Error":
          if (msg.code === "RENAME_FAILED" && state.pendingRename) {
            return { ...state, pendingRename: { ...state.pendingRename, error: msg.message } };
          }
          if (msg.code === "MKDIR_FAILED" && state.pendingMkdir) {
            return { ...state, pendingMkdir: { ...state.pendingMkdir, error: msg.message } };
          }
          if (msg.code === "NO_EDITOR" && msg.extra?.path) {
            const path = msg.extra.path as string;
            const filename = path.replace(/\\/g, "/").split("/").filter(Boolean).pop() || path;
            return { ...state, pendingNoEditor: { filename, fullPath: path } };
          }
          console.error("BE error:", msg.code, msg.message);
          return state;
        case "RenameOk":
          return { ...state, pendingRename: null };
        case "MkdirOk":
          return { ...state, pendingMkdir: null };
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
    case "OPEN_RENAME":
      return { ...state, pendingRename: action.rename };
    case "CLOSE_RENAME":
      return { ...state, pendingRename: null };
    case "OPEN_INFO":
      return { ...state, pendingInfo: action.info };
    case "CLOSE_INFO":
      return { ...state, pendingInfo: null };
    case "OPEN_MKDIR":
      return { ...state, pendingMkdir: action.mkdir };
    case "CLOSE_MKDIR":
      return { ...state, pendingMkdir: null };
    case "OPEN_NO_EDITOR":
      return { ...state, pendingNoEditor: action.noEditor };
    case "CLOSE_NO_EDITOR":
      return { ...state, pendingNoEditor: null };
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
