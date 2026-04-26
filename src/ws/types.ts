// ─── Domain types ────────────────────────────────────────────────────────────

export interface VFSEntry {
  name: string;
  kind: "file" | "directory" | "symlink" | "other";
  size?: number;
  mtime?: string;
  btime?: string; // birth time — file creation; not available on all platforms/filesystems
  permissions?: string;
  iconName?: string;
  /** Color class hint — a short CSS class name (e.g. "cc-dir", "cc-archive"). */
  cc?: string;
  /** Real absolute path when the entry's logical name differs from its filesystem
   *  path (e.g. search-result entries). Absent for all normal entries. */
  sourcePath?: string;
}

export interface Source {
  id: string;
  name: string;
  kind: "filesystem" | "virtual" | "archive";
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
  shadow?: boolean;
}

export interface ShadowSource {
  parentSourceId: string;
  parentPath: string;
  archivePath: string;
  cursorHint?: string;
}

export interface PanelState {
  panelId: string;
  path: string;
  status: "loading" | "ready" | "error";
  sourceId: string;
  sort: { by: string; order: "asc" | "desc" };
  filter: null | { mode: string; value: string };
  filterModeActive: boolean;
  showHidden: boolean;
  cursor: number;
  selection: number[];
  selectedEntries: FileOpFile[];
  entries: VFSEntry[];
  version: number;
  calculatingSize: string[];
  totalCount?: number;
  totalSize?: number;
  windowStart?: number;
  /** Monotonic counter for window-slice patches. Independent from `version` so
   *  fast scroll-driven RequestWindow responses can't be dropped by the
   *  structural-version guard (see store reducer). */
  windowVersion?: number;
  shadowSource?: ShadowSource;
}

export interface LayoutDomain {
  panels: string[];
}

export interface SearchParams {
  paths: string[];
  depth: string;
  fileMask?: string | null;
  maskMode: string;
  contentQuery?: string | null;
}

export interface SearchResult {
  name: string;
  kind: string;
  size?: number;
  parentPath: string;
  fullPath: string;
  sourceId: string;
}

export interface SearchProgress {
  scanned: number;
  matched: number;
  currentPath: string;
  newResults: SearchResult[];
}

export interface Job {
  jobId: string;
  kind: "file-op" | "fp-bro" | "search";
  type: string;
  state: "Queued" | "Running" | "AwaitingDecision" | "Completed" | "Cancelled" | "Failed";
  paused: boolean;
  supportsPause: boolean;
  startedAt: number;
  sourcePath: string;
  destPath?: string;
  resultPath?: string;
  resultPanelId?: string;
  files: FileOpFile[];
  progress?: FileOpProgress;
  decision?: DecisionRequest;
  error?: string;
  // Search-specific fields
  searchSourceId?: string;
  searchParams?: SearchParams;
  searchResults?: SearchResult[];
  searchScanned?: number;
  searchMatched?: number;
  searchCurrentPath?: string;
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
  statusText?: string;
  currentFileBytesTotal: number;
  currentFileBytesCopied: number;
  totalBytesTotal: number;
  totalBytesCopied: number;
  currentFilePercent?: number;
  overallPercent?: number;
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
  sourcePath?: string;
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

export interface OpenWithAppItem {
  id: string;
  label: string;
  detail?: string;
  isDefault: boolean;
  source: "system" | "supplement";
}

export interface BroInputItem {
  path: string;
  name: string;
  kind: string;
}

export interface BroOutputItem {
  label: string;
  defaultValue?: string;
}

export interface BroParamItem {
  id: string;
  label: string;
  kind: "int" | "password";
  required: boolean;
  defaultValue?: string;
  minValue?: number;
  maxValue?: number;
}

export interface BroPreItem {
  score: number;
  reason: string;
}

export interface BroItem {
  id: string;
  name: string;
  version: string;
  source: string;
  detail?: string;
  protocol: "fnf" | "fp";
  pre: BroPreItem;
  output?: BroOutputItem;
  params: BroParamItem[];
}

export interface JobStart {
  jobId: string;
  kind: "file-op" | "fp-bro" | "search";
  opType: string;
  files: FileOpFile[];
  sourcePanel: string;
  sourcePath: string;
  destPanel?: string;
  destPath?: string;
  initialState: Job["state"];
  totalFiles: number;
  totalBytesTotal: number;
  background: boolean;
  supportsPause: boolean;
  resultPath?: string;
  resultPanelId?: string;
  requestId?: string;
  searchSourceId?: string;
  searchParams?: SearchParams;
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

export interface NavigateOkMessage {
  type: "NavigateOk";
  timestamp: number;
  panelId: string;
}

export interface ExternalClipboardReadyMessage {
  type: "ExternalClipboardReady";
  timestamp: number;
  panelId: string;
  opType: Extract<FileOpType, "copy" | "move">;
  files: FileOpFile[];
  destPath: string;
}

export interface ExternalClipboardWrittenMessage {
  type: "ExternalClipboardWritten";
  timestamp: number;
  opType: Extract<FileOpType, "copy" | "move">;
  itemCount: number;
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
  event: "Queued" | "Progress" | "DecisionRequired" | "Completed" | "Failed" | "Cancelled" | "Paused" | "Resumed";
  progress?: FileOpProgress;
  searchProgress?: SearchProgress;
  decision?: DecisionRequest;
  error?: string;
  resultPath?: string;
  resultPanelId?: string;
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

export interface OpenWithOptionsReadyMessage {
  type: "OpenWithOptionsReady";
  timestamp: number;
  requestId: string;
  apps: OpenWithAppItem[];
}

export interface OpenWithOptionsFailedMessage {
  type: "OpenWithOptionsFailed";
  timestamp: number;
  requestId: string;
  message: string;
}

export interface BroOptionsReadyMessage {
  type: "BroOptionsReady";
  timestamp: number;
  requestId: string;
  inputs: BroInputItem[];
  bros: BroItem[];
}

export interface BroOptionsFailedMessage {
  type: "BroOptionsFailed";
  timestamp: number;
  requestId: string;
  message: string;
}

export interface InputRequiredMessage {
  type: "InputRequired";
  timestamp: number;
  panelId: string;
  requestId: string;
  kind: "password" | "text";
  prompt: string;
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
  | NavigateOkMessage
  | ExternalClipboardReadyMessage
  | ExternalClipboardWrittenMessage
  | ViewerFileChangedMessage
  | DryRunReadyMessage
  | OpenWithOptionsReadyMessage
  | OpenWithOptionsFailedMessage
  | BroOptionsReadyMessage
  | BroOptionsFailedMessage
  | InputRequiredMessage;

export type FEMessage = HelloMessage | IntentMessage;
