import { createContext, useContext, useReducer, type Dispatch } from "react";
import type {
  PanelState,
  Source,
  Job,
  DirSizeTask,
  BEMessage,
  InteractiveCommand,
  StateSnapshotMessage,
  StatePatchMessage,
  JobStartedMessage,
  JobEventMessage,
  PendingOp,
  QuitConfirmRequiredMessage,
  VFSEntry,
  DryRunReadyMessage,
  OpenWithAppItem,
  OpenWithOptionsFailedMessage,
  OpenWithOptionsReadyMessage,
  BroInputItem,
  BroItem,
  BroOptionsFailedMessage,
  BroOptionsReadyMessage,
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

export interface PendingGoToPath {
  panelId: string;
  currentPath: string;
  error?: string;
}

export interface PendingPanelHistory {
  panelId: string;
  currentPath: string;
  error?: string;
}

export interface PendingDryRun {
  scriptPath: string;
  scriptDir: string;
  scriptName: string;
  sourcePanel: string;
}

export interface PendingContextMenu {
  panelId: string;
  absIndex: number;
  name: string;
  kind: string;
  size?: number;
  dirPath: string;
  fullPath: string;
  anchorLeft: number;
  anchorRight: number;
  anchorTop: number;
  anchorBottom: number;
}

export interface PendingOpenWith {
  panelId: string;
  absIndex: number;
  name: string;
  kind: string;
  dirPath: string;
  fullPath: string;
  requestId: string;
  status: "loading" | "ready" | "error";
  apps: OpenWithAppItem[];
  error?: string;
}

export interface PendingBro {
  panelId: string;
  dirPath: string;
  requestId: string;
  explicitPath?: string;
  status: "loading" | "ready" | "error";
  inputs: BroInputItem[];
  bros: BroItem[];
  error?: string;
}

export interface AppState {
  connected: boolean;
  sessionId: string | null;
  fileServerPort: number;
  layout: { panels: string[] };
  panels: Record<string, PanelState>;
  panelHistory: Record<string, string[]>;
  sources: Record<string, Source>;
  focus: string;
  jobs: Record<string, Job>;
  dirSizeQueue: DirSizeTask[];
  commands: Record<string, InteractiveCommand>;
  pendingOp: PendingOp | null;
  activeJobId: string | null;
  pendingQuit: { activeJobs: number } | null;
  pendingRename: PendingRename | null;
  pendingInfo: PendingInfo | null;
  pendingMkdir: PendingMkdir | null;
  pendingNoEditor: PendingNoEditor | null;
  pendingGoToPath: PendingGoToPath | null;
  pendingPanelHistory: PendingPanelHistory | null;
  pendingDryRun: PendingDryRun | null;
  pendingContextMenu: PendingContextMenu | null;
  pendingOpenWith: PendingOpenWith | null;
  pendingBro: PendingBro | null;
  commandPaletteOpen: boolean;
}

const initialState: AppState = {
  connected: false,
  sessionId: null,
  fileServerPort: 0,
  layout: { panels: [] },
  panels: {},
  panelHistory: {},
  sources: {},
  focus: "",
  jobs: {},
  dirSizeQueue: [],
  commands: {},
  pendingOp: null,
  activeJobId: null,
  pendingQuit: null,
  pendingRename: null,
  pendingInfo: null,
  pendingMkdir: null,
  pendingNoEditor: null,
  pendingGoToPath: null,
  pendingPanelHistory: null,
  pendingDryRun: null,
  pendingContextMenu: null,
  pendingOpenWith: null,
  pendingBro: null,
  commandPaletteOpen: false,
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
  | { type: "CLOSE_NO_EDITOR" }
  | { type: "OPEN_GO_TO_PATH"; goToPath: PendingGoToPath }
  | { type: "CLOSE_GO_TO_PATH" }
  | { type: "OPEN_PANEL_HISTORY"; panelHistory: PendingPanelHistory }
  | { type: "CLOSE_PANEL_HISTORY" }
  | { type: "CLOSE_DRY_RUN" }
  | { type: "OPEN_CONTEXT_MENU"; menu: PendingContextMenu }
  | { type: "CLOSE_CONTEXT_MENU" }
  | { type: "OPEN_OPEN_WITH"; openWith: PendingOpenWith }
  | { type: "CLOSE_OPEN_WITH" }
  | { type: "OPEN_BRO"; bro: PendingBro }
  | { type: "CLOSE_BRO" }
  | { type: "OPEN_COMMAND_PALETTE" }
  | { type: "CLOSE_COMMAND_PALETTE" };

function pushPanelHistoryEntry(
  panelHistory: Record<string, string[]>,
  panelId: string,
  previousPath: string,
  nextPath: string,
) {
  if (!previousPath || previousPath === nextPath) return panelHistory;

  const current = panelHistory[panelId] ?? [];
  const next = [previousPath, ...current.filter((path) => path !== previousPath)].slice(0, 20);
  return { ...panelHistory, [panelId]: next };
}

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
      panels[id] = { ...p, panelId: id, entries: p.entries ?? [], selection: p.selection ?? [], selectedEntries: p.selectedEntries ?? [], version: p.version ?? 0, calculatingSize: p.calculatingSize ?? [], totalCount: p.totalCount, totalSize: p.totalSize, windowStart: p.windowStart, filterModeActive: p.filterModeActive ?? false };
    }
    next.panels = panels;
    next.panelHistory = Object.fromEntries(
      Object.keys(panels).map((panelId) => [panelId, state.panelHistory[panelId] ?? []]),
    );
  }
  if (domains.jobs) {
    const jobs: Record<string, Job> = {};
    for (const j of domains.jobs) jobs[j.jobId] = j;
    next.jobs = jobs;
  }
  if (domains.dirSizeQueue) {
    next.dirSizeQueue = domains.dirSizeQueue.tasks ?? [];
  }
  if (domains.commands) {
    const commands: Record<string, InteractiveCommand> = {};
    for (const command of domains.commands) commands[command.id] = command;
    next.commands = commands;
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
    if (msg.patch.selectedEntries !== undefined) {
      preGuardPanel = { ...preGuardPanel, selectedEntries: msg.patch.selectedEntries as PanelState["selectedEntries"] };
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
    if (msg.patch.filter !== undefined) {
      updated = { ...updated, filter: msg.patch.filter as PanelState["filter"] };
    }
    if (msg.patch.filterModeActive !== undefined) {
      updated = { ...updated, filterModeActive: msg.patch.filterModeActive as boolean };
    }
    if (msg.patch.selection !== undefined) {
      updated = { ...updated, selection: msg.patch.selection };
    }
    if (msg.patch.selectedEntries !== undefined) {
      updated = { ...updated, selectedEntries: msg.patch.selectedEntries as PanelState["selectedEntries"] };
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
    if (msg.patch.totalSize !== undefined) {
      updated = { ...updated, totalSize: msg.patch.totalSize === null ? undefined : msg.patch.totalSize as number };
    }
    if (msg.patch.windowStart !== undefined) {
      updated = { ...updated, windowStart: msg.patch.windowStart === null ? undefined : msg.patch.windowStart as number };
    }
    if (msg.patch.windowEntries !== undefined) {
      updated = { ...updated, entries: msg.patch.windowEntries as VFSEntry[] };
    }

    const nextHistory = updated.path !== panel.path
      ? pushPanelHistoryEntry(state.panelHistory, panelId, panel.path, updated.path)
      : state.panelHistory;

    return { ...state, panels: { ...state.panels, [panelId]: updated }, panelHistory: nextHistory };
  }

  if (msg.domain === "focus") {
    return { ...state, focus: msg.patch.focus as string };
  }

  if (msg.domain === "sources") {
    const sources: Record<string, Source> = {};
    for (const s of msg.patch.sources as Source[]) sources[s.id] = s;
    return { ...state, sources };
  }

  if (msg.domain === "dirSizeQueue") {
    return { ...state, dirSizeQueue: (msg.patch.tasks as DirSizeTask[]) ?? [] };
  }

  return state;
}

function applyJobStarted(state: AppState, msg: JobStartedMessage): AppState {
  const { job } = msg;
  const newJob: Job = {
    jobId: job.jobId,
    type: job.opType,
    state: "Running",
    paused: false,
    startedAt: msg.timestamp,
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
      updated = { ...job, state: "Running", paused: false, progress: msg.progress ?? job.progress, decision: undefined };
      break;
    case "DecisionRequired":
      updated = { ...job, state: "AwaitingDecision", decision: msg.decision };
      break;
    case "Paused":
      updated = { ...job, paused: true };
      break;
    case "Resumed":
      updated = { ...job, paused: false };
      break;
    case "Completed":
      updated = { ...job, state: "Completed", paused: false, decision: undefined };
      break;
    case "Failed":
      updated = { ...job, state: "Failed", paused: false, error: msg.error, decision: undefined };
      break;
    case "Cancelled":
      updated = { ...job, state: "Cancelled", paused: false, decision: undefined };
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
          if (msg.code === "NAVIGATE_FAILED") {
            const panelId = msg.extra?.panelId as string | undefined;
            if (!panelId) return state;
            let next = state;
            const pendingGoToPath = state.pendingGoToPath;
            if (pendingGoToPath?.panelId === panelId) {
              next = {
                ...next,
                pendingGoToPath: {
                  ...pendingGoToPath,
                  error: msg.message,
                },
              };
            }
            const pendingPanelHistory = state.pendingPanelHistory;
            if (pendingPanelHistory?.panelId === panelId) {
              next = {
                ...next,
                pendingPanelHistory: {
                  ...pendingPanelHistory,
                  error: msg.message,
                },
              };
            }
            return next;
          }
          console.error("BE error:", msg.code, msg.message);
          return state;
        case "RenameOk":
          return { ...state, pendingRename: null };
        case "MkdirOk":
          return { ...state, pendingMkdir: null };
        case "NavigateOk":
          return {
            ...state,
            pendingGoToPath: state.pendingGoToPath?.panelId === msg.panelId ? null : state.pendingGoToPath,
            pendingPanelHistory: state.pendingPanelHistory?.panelId === msg.panelId ? null : state.pendingPanelHistory,
          };
        case "DryRunReady": {
          const m = msg as DryRunReadyMessage;
          const scriptName = m.scriptPath.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? m.scriptPath;
          return {
            ...state,
            pendingOp: null,
            pendingDryRun: {
              scriptPath: m.scriptPath,
              scriptDir: m.scriptDir,
              scriptName,
              sourcePanel: state.pendingOp?.sourcePanel ?? "",
            },
          };
        }
        case "OpenWithOptionsReady":
          if (!state.pendingOpenWith || state.pendingOpenWith.requestId !== (msg as OpenWithOptionsReadyMessage).requestId) {
            return state;
          }
          return {
            ...state,
            pendingOpenWith: {
              ...state.pendingOpenWith,
              status: "ready",
              apps: (msg as OpenWithOptionsReadyMessage).apps,
              error: undefined,
            },
          };
        case "OpenWithOptionsFailed":
          if (!state.pendingOpenWith || state.pendingOpenWith.requestId !== (msg as OpenWithOptionsFailedMessage).requestId) {
            return state;
          }
          return {
            ...state,
            pendingOpenWith: {
              ...state.pendingOpenWith,
              status: "error",
              error: (msg as OpenWithOptionsFailedMessage).message,
              apps: [],
            },
          };
        case "BroOptionsReady":
          if (!state.pendingBro || state.pendingBro.requestId !== (msg as BroOptionsReadyMessage).requestId) {
            return state;
          }
          return {
            ...state,
            pendingBro: {
              ...state.pendingBro,
              status: "ready",
              inputs: (msg as BroOptionsReadyMessage).inputs,
              bros: (msg as BroOptionsReadyMessage).bros,
              error: undefined,
            },
          };
        case "BroOptionsFailed":
          if (!state.pendingBro || state.pendingBro.requestId !== (msg as BroOptionsFailedMessage).requestId) {
            return state;
          }
          return {
            ...state,
            pendingBro: {
              ...state.pendingBro,
              status: "error",
              error: (msg as BroOptionsFailedMessage).message,
              inputs: [],
              bros: [],
            },
          };
        case "QuitConfirmRequired":
          return applyQuitConfirmRequired(state, msg);
        default:
          return state;
      }
    }
    case "OPEN_OPERATION":
      return { ...state, pendingOp: action.op, pendingContextMenu: null };
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
    case "OPEN_GO_TO_PATH":
      return { ...state, pendingGoToPath: action.goToPath };
    case "CLOSE_GO_TO_PATH":
      return { ...state, pendingGoToPath: null };
    case "OPEN_PANEL_HISTORY":
      return { ...state, pendingPanelHistory: action.panelHistory };
    case "CLOSE_PANEL_HISTORY":
      return { ...state, pendingPanelHistory: null };
    case "CLOSE_DRY_RUN":
      return { ...state, pendingDryRun: null };
    case "OPEN_CONTEXT_MENU":
      return { ...state, pendingContextMenu: action.menu };
    case "CLOSE_CONTEXT_MENU":
      return { ...state, pendingContextMenu: null };
    case "OPEN_OPEN_WITH":
      return { ...state, pendingContextMenu: null, pendingOpenWith: action.openWith };
    case "CLOSE_OPEN_WITH":
      return { ...state, pendingOpenWith: null };
    case "OPEN_BRO":
      return { ...state, pendingContextMenu: null, pendingBro: action.bro };
    case "CLOSE_BRO":
      return { ...state, pendingBro: null };
    case "OPEN_COMMAND_PALETTE":
      return { ...state, commandPaletteOpen: true };
    case "CLOSE_COMMAND_PALETTE":
      return { ...state, commandPaletteOpen: false };
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
