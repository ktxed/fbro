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
  mounted: boolean;
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
  filterModeActive: boolean;
  cursor: number;
  selection: number[];
  selectedEntries: FileOpFile[];
  entries: VFSEntry[];
  version: number;
  calculatingSize: string[];
  totalCount?: number;
  totalSize?: number;
  windowStart?: number;
}

export interface LayoutDomain {
  panels: string[];
}

export interface Job {
  jobId: string;
  type: string;
  state: "Running" | "AwaitingDecision" | "Completed" | "Cancelled" | "Failed";
  paused: boolean;
  startedAt: number;
  sourcePath: string;
  destPath?: string;
  progress?: FileOpProgress;
  decision?: DecisionRequest;
  error?: string;
}

export interface DirSizeTask {
  panelId: string;
  entryName: string;
  fullPath: string;
  enqueuedAt: number;
  status: "Queued" | "Running";
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

export interface KeyBinding {
  key: string;
  shift?: boolean;
  ctrl?: boolean;
  alt?: boolean;
  meta?: boolean;
}

export interface InteractiveCommand {
  id: string;
  category: string;
  title: string;
  bindings: KeyBinding[];
  defaultBindings: KeyBinding[];
  intentName?: string;
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
  timestamp: number;
  protocolVersion: string;
  client: {
    kind: "gui" | "viewer";
    name: string;
    supports: string[];
  };
}

export interface IntentMessage {
  type: "Intent";
  timestamp: number;
  name: string;
  scope?: string | { from?: string; to?: string };
  args?: Record<string, unknown>;
  options?: Record<string, unknown>;
}

// ─── BE → Client messages ────────────────────────────────────────────────────

export interface WelcomeMessage {
  type: "Welcome";
  timestamp: number;
  protocolVersion: string;
  sessionId: string;
  fileServerPort: number;
}

export interface StateSnapshotMessage {
  type: "StateSnapshot";
  timestamp: number;
  domains: {
    layout?: LayoutDomain;
    panels?: Record<string, PanelState>;
    sources?: Source[];
    focus?: string;
    jobs?: Job[];
    dirSizeQueue?: { tasks: DirSizeTask[] };
    commands?: InteractiveCommand[];
  };
}

export interface StatePatchMessage {
  type: "StatePatch";
  timestamp: number;
  domain: "panels" | "layout" | "focus" | "selections" | "jobs" | "sources" | "dirSizeQueue";
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
    filter?: PanelState["filter"];
    filterModeActive?: boolean;
    selection?: number[];
    selectedEntries?: FileOpFile[];
    [key: string]: unknown;
  };
}

export interface JobStartedMessage {
  type: "JobStarted";
  timestamp: number;
  job: JobStart;
}

export interface JobEventMessage {
  type: "JobEvent";
  timestamp: number;
  jobId: string;
  event: "Progress" | "DecisionRequired" | "Completed" | "Failed" | "Cancelled" | "Paused" | "Resumed";
  progress?: FileOpProgress;
  decision?: DecisionRequest;
  error?: string;
}

export interface ErrorMessage {
  type: "Error";
  timestamp: number;
  code: string;
  message: string;
  recoverable: boolean;
  extra?: Record<string, unknown>;
}

export interface QuitConfirmRequiredMessage {
  type: "QuitConfirmRequired";
  timestamp: number;
  activeJobs: number;
}

export interface FileChunkMessage {
  type: "FileChunk";
  timestamp: number;
  path: string;
  offset: number;
  /** Hex-encoded bytes. */
  data: string;
  eof: boolean;
  fileSize?: number;
}

export interface RenameOkMessage {
  type: "RenameOk";
  timestamp: number;
  panelId: string;
}

export interface MkdirOkMessage {
  type: "MkdirOk";
  timestamp: number;
  panelId: string;
}

export interface ViewerFileChangedMessage {
  type: "ViewerFileChanged";
  timestamp: number;
  path: string;
  filename: string;
  sourceId: string;
  panelId: string;
  fileSize?: number;
  isLocal?: boolean;
}

export interface DryRunReadyMessage {
  type: "DryRunReady";
  timestamp: number;
  scriptPath: string;
  scriptDir: string;
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
  | ViewerFileChangedMessage
  | DryRunReadyMessage;

export type FEMessage = HelloMessage | IntentMessage;
