// ─── Domain types ────────────────────────────────────────────────────────────

export interface VFSEntry {
  name: string;
  kind: "file" | "directory" | "symlink" | "other";
  size?: number;
  mtime?: string;
}

export interface Source {
  id: string;
  name: string;
  kind: "filesystem" | "virtual";
  supports: string[];
  capabilities: {
    readOnly: boolean;
    supportsStreaming: boolean;
    supportsJobs: boolean;
  };
}

export interface PanelState {
  panelId: string;
  path: string;
  status: "loading" | "ready" | "error";
  sourceId: string;
  sort: { by: string; order: "asc" | "desc" };
  filter: null | { mode: string; value: string };
  cursor: number;
  selection: number[];
  entries: VFSEntry[];
  version: number;
}

export interface LayoutDomain {
  panels: string[];
}

export interface Job {
  jobId: string;
  type: string;
  state: "Created" | "Running" | "AwaitingDecision" | "Completed" | "Cancelled" | "Failed";
  progress?: { current: number; total: number };
}

export interface DecisionRequest {
  kind: string;
  source?: string;
  destination?: string;
  options: string[];
  supportsApplyToAll: boolean;
}

// ─── Client → BE messages ────────────────────────────────────────────────────

export interface HelloMessage {
  type: "Hello";
  timestamp: string;
  protocolVersion: string;
  client: {
    kind: "gui";
    name: string;
    supports: string[];
  };
}

export interface IntentMessage {
  type: "Intent";
  timestamp: string;
  name: string;
  scope?: string | { from?: string; to?: string };
  args?: Record<string, unknown>;
  options?: Record<string, unknown>;
}

// ─── BE → Client messages ────────────────────────────────────────────────────

export interface WelcomeMessage {
  type: "Welcome";
  timestamp: string;
  protocolVersion: string;
  sessionId: string;
}

export interface StateSnapshotMessage {
  type: "StateSnapshot";
  timestamp: string;
  domains: {
    layout?: LayoutDomain;
    panels?: Record<string, PanelState>;
    sources?: Source[];
    focus?: string;
    jobs?: Job[];
  };
}

export interface StatePatchMessage {
  type: "StatePatch";
  timestamp: string;
  domain: "panels" | "layout" | "focus" | "selections" | "jobs";
  entityId?: string;
  version: number;
  patch: {
    appendEntries?: VFSEntry[];
    status?: PanelState["status"];
    cursor?: number;
    path?: string;
    entries?: VFSEntry[];
    selection?: number[];
    [key: string]: unknown;
  };
}

export interface JobEventMessage {
  type: "JobEvent";
  timestamp: string;
  jobId: string;
  event: "Progress" | "DecisionRequired" | "Completed" | "Failed" | "Cancelled";
  progress?: { current: number; total: number };
  decision?: DecisionRequest;
}

export interface ErrorMessage {
  type: "Error";
  timestamp: string;
  code: string;
  message: string;
  recoverable: boolean;
}

// ─── Union types ─────────────────────────────────────────────────────────────

export type BEMessage =
  | WelcomeMessage
  | StateSnapshotMessage
  | StatePatchMessage
  | JobEventMessage
  | ErrorMessage;

export type FEMessage = HelloMessage | IntentMessage;
