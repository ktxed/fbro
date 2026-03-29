// ─── Domain types ────────────────────────────────────────────────────────────

export interface VFSEntry {
  name: string;
  kind: "file" | "directory" | "symlink" | "other";
  size?: number;
  mtime?: string;
  btime?: string; // birth time — file creation; not available on all platforms/filesystems
  permissions?: string;
  iconName?: string;
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
  rootPath: string;
  freeBytes?: number;
  totalBytes?: number;
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
  calculatingSize: string[];
  totalCount?: number;
  windowStart?: number;
}

export interface LayoutDomain {
  panels: string[];
}

export interface Job {
  jobId: string;
  type: string;
  state: "Running" | "AwaitingDecision" | "Completed" | "Cancelled" | "Failed";
  sourcePath: string;
  destPath?: string;
  progress?: FileOpProgress;
  decision?: DecisionRequest;
  error?: string;
}

export interface FileOpProgress {
  totalFiles: number;
  completedFiles: number;
  currentFile?: string;
  currentFileBytesTotal: number;
  currentFileBytesCopied: number;
  totalBytesTotal: number;
  totalBytesCopied: number;
}

export interface DecisionRequest {
  source: string;
  destination: string;
  options: string[];
  supportsApplyToAll: boolean;
}

export interface FileOpFile {
  name: string;
  size?: number;
  kind: string;
}

export interface JobStart {
  jobId: string;
  opType: string;
  files: FileOpFile[];
  sourcePanel: string;
  sourcePath: string;
  destPanel?: string;
  destPath?: string;
  totalFiles: number;
  totalBytesTotal: number;
  background: boolean;
}

export type FileOpType = "copy" | "move" | "delete";
export type ConflictPolicy = "ask" | "skip" | "overwrite";

export interface PendingOp {
  opType: FileOpType;
  files: FileOpFile[];
  sourcePanel: string;
  destPanel?: string;
  destPath?: string;
  conflictPolicy: ConflictPolicy;
}

// ─── Client → BE messages ────────────────────────────────────────────────────

export interface HelloMessage {
  type: "Hello";
  timestamp: string;
  protocolVersion: string;
  client: {
    kind: "gui" | "viewer";
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
  fileServerPort: number;
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
    sort?: PanelState["sort"];
    cursor?: number;
    path?: string;
    sourceId?: string;
    entries?: VFSEntry[];
    selection?: number[];
    [key: string]: unknown;
  };
}

export interface JobStartedMessage {
  type: "JobStarted";
  timestamp: string;
  job: JobStart;
}

export interface JobEventMessage {
  type: "JobEvent";
  timestamp: string;
  jobId: string;
  event: "Progress" | "DecisionRequired" | "Completed" | "Failed" | "Cancelled";
  progress?: FileOpProgress;
  decision?: DecisionRequest;
  error?: string;
}

export interface ErrorMessage {
  type: "Error";
  timestamp: string;
  code: string;
  message: string;
  recoverable: boolean;
  extra?: Record<string, unknown>;
}

export interface QuitConfirmRequiredMessage {
  type: "QuitConfirmRequired";
  timestamp: string;
  activeJobs: number;
}

export interface FileChunkMessage {
  type: "FileChunk";
  timestamp: string;
  path: string;
  offset: number;
  /** Hex-encoded bytes. */
  data: string;
  eof: boolean;
  fileSize?: number;
}

export interface RenameOkMessage {
  type: "RenameOk";
  timestamp: string;
  panelId: string;
}

export interface MkdirOkMessage {
  type: "MkdirOk";
  timestamp: string;
  panelId: string;
}

export interface ViewerFileChangedMessage {
  type: "ViewerFileChanged";
  timestamp: string;
  path: string;
  filename: string;
  sourceId: string;
  panelId: string;
  fileSize?: number;
  isLocal?: boolean;
}

// ─── Union types ─────────────────────────────────────────────────────────────

export type BEMessage =
  | WelcomeMessage
  | StateSnapshotMessage
  | StatePatchMessage
  | JobStartedMessage
  | JobEventMessage
  | ErrorMessage
  | QuitConfirmRequiredMessage
  | FileChunkMessage
  | RenameOkMessage
  | MkdirOkMessage
  | ViewerFileChangedMessage;

export type FEMessage = HelloMessage | IntentMessage;
