import { createContext, useContext, useReducer, type Dispatch } from "react";
import type {
  PanelState,
  Source,
  Job,
  BEMessage,
  StateSnapshotMessage,
  StatePatchMessage,
} from "../ws/types";

// ─── State ───────────────────────────────────────────────────────────────────

export interface AppState {
  connected: boolean;
  sessionId: string | null;
  layout: { panels: string[] };
  panels: Record<string, PanelState>;
  sources: Record<string, Source>;
  focus: string;
  jobs: Job[];
}

const initialState: AppState = {
  connected: false,
  sessionId: null,
  layout: { panels: [] },
  panels: {},
  sources: {},
  focus: "",
  jobs: [],
};

// ─── Actions ─────────────────────────────────────────────────────────────────

export type AppAction =
  | { type: "CONNECTED" }
  | { type: "DISCONNECTED" }
  | { type: "BE_MESSAGE"; msg: BEMessage };

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
    next.jobs = domains.jobs;
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
        case "JobEvent":
          // minimal handling — update job state in list
          return state;
        case "Error":
          console.error("BE error:", msg.code, msg.message);
          return state;
        default:
          return state;
      }
    }
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
