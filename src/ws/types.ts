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
  gitStatus?: string;
  gitDetails?: {
    changedCount: number;
    samplePaths: string[];
  };
}

export interface GitChange {
  path: string;
  indexStatus: string;
  worktreeStatus: string;
}

export interface GitRepoStatus {
  root: string;
  branch: string;
  detached: boolean;
  clean: boolean;
  stagedCount: number;
  modifiedCount: number;
  untrackedCount: number;
  conflictedCount: number;
  upstream?: string;
  aheadCount?: number;
  behindCount?: number;
  branches: string[];
  changes: GitChange[];
}

export interface Source {
  id: string;
  name: string;
  kind: "filesystem" | "virtual" | "remote";
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
  iconName?: string;
  /** Set on remote sources — peer that owns it. Drives the volume-bar
   *  label suffix and the read-only badge tooltip. */
  peer?: SourcePeer;
}

export interface SourcePeer {
  name: string;
  fingerprint: string;
  os?: string;
}

export interface GlobalSelectionSummary {
  id: string;
  name: string;
  itemCount: number;
  createdAt: string;
}

// ─── Remote-fbro: discovery / pairing / sharing ──────────────────────────────

export interface DiscoveredPeer {
  id: string;
  name: string;
  host: string;
  port: number;
  os?: string;
  pubkeyFp?: string;
  paired: boolean;
  /** Unix millis of last `ServiceResolved`. The FE dims rows whose
   *  lastSeen is older than a short threshold. */
  lastSeen: number;
}

export interface PairedPeer {
  fingerprint: string;
  name: string;
  os?: string;
  pairedAt: number;
  lastSeen?: number;
  lastHost?: string;
  lastSftpPort?: number;
  lastPairPort?: number;
  /** Unix seconds when the last endpoint was learned from mDNS or connect. */
  lastEndpointSeenAt?: number;
}

export interface ShareConfig {
  enabled: boolean;
  allowIpv6: boolean;
  sharedSourceIds: string[];
  sharedRoots: SharedRoot[];
  shareSuggestions: SharedRoot[];
  pairedPeers: PairedPeer[];
  /** Our own Ed25519 fingerprint. Surfaced so the user can compare it
   *  against what a peer sees during pairing. */
  fingerprint?: string;
}

export interface SharedRoot {
  id: string;
  name: string;
  path: string;
  sourceId: string;
  origin: "source" | "home" | "bookmark";
  iconName?: string;
}

export interface PendingPairing {
  handshakeId: string;
  code: string;
  name: string;
  os?: string;
  pubkeyAlgo: string;
  expiresAt: number;
}

export interface ShadowSource {
  parentSourceId: string;
  parentPath: string;
  archivePath: string;
  cursorHint?: string;
}

export type TabKind = "listing" | "terminal";

export interface PanelSlot {
  slotId: string;
  tabPanelIds: string[];
  activePanelId: string;
}

export interface PanelState {
  panelId: string;
  /** Slot this panel belongs to. Panels sharing a slot are sibling tabs. */
  slotId: string;
  /** Tab kind — "listing" for folder view, "terminal" for promoted PTY tab. */
  tabKind: TabKind;
  /** User-supplied tab title; when absent, derived from path/cwd. */
  tabTitle?: string;
  path: string;
  status: "loading" | "ready" | "error";
  sourceId: string;
  sort: { by: string; order: "asc" | "desc" };
  filter: null | { mode: string; value: string };
  filterModeActive: boolean;
  showHidden: boolean;
  viewMode: "list" | "grid";
  /** True if the underlying source supports OS-native or fbro-managed
   *  thumbnails (host-path-backed: local FS, search results). When false,
   *  grid mode is rejected and the toggle command is a no-op. */
  supportsThumbnails: boolean;
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
  /** When set, this panel is hosting an embedded terminal split with this PTY session id. */
  terminalSessionId?: number;
  git?: GitRepoStatus;
}

export interface LayoutDomain {
  /** Active panel id per slot (slot order). Kept for back-compat. */
  panels: string[];
  /** Ordered slot ids — drives the visible layout left-to-right. */
  slots: string[];
}

export interface SearchParams {
  paths: string[];
  depth: string;
  fileMask?: string | null;
  maskMode: string;
  contentQuery?: string | null;
  sizeOp?: "interval" | "gt" | "lt" | "eq";
  sizeValue?: number;
  sizeMax?: number;
  sizeUnit?: "B" | "KiB" | "MiB" | "GiB";
  timeOp?: "interval" | "before" | "after" | "relative";
  timeValue?: string;
  timeTo?: string;
}

export interface SearchResult {
  name: string;
  kind: string;
  size?: number;
  mtime?: string;
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
  kind: "file-op" | "fp-bro" | "search" | "outgoing-transfer" | "sync-bro" | "fedup-bro" | "remote-manifest";
  type: string;
  state: "Queued" | "Running" | "AwaitingDecision" | "Completed" | "Cancelled" | "Failed";
  paused: boolean;
  supportsPause: boolean;
  startedAt: number;
  sourcePanel: string;
  sourcePath: string;
  destPanel?: string;
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
  // Sync Bro-specific fields
  syncResult?: SyncBroResult;
  reportSavedPath?: string;
  // Fedup Bro-specific fields
  fedupResult?: FedupBroResult;
}

export interface SyncBroDiff {
  path: string;
  localSize: number;
  remoteSize: number;
  localHash: string;
  remoteHash: string;
}

export interface SyncBroError {
  path: string;
  message: string;
}

export interface SyncBroResult {
  matched: number;
  differing: SyncBroDiff[];
  localOnly: string[];
  remoteOnly: string[];
  unverified: string[];
  localErrors: SyncBroError[];
  remoteErrors: SyncBroError[];
}

export interface FedupBroFile {
  path: string;
  size: number;
  mtime?: number;
  hash: string;
}

export interface FedupBroGroup {
  hash: string;
  size: number;
  keeper: FedupBroFile;
  duplicates: FedupBroFile[];
}

export interface FedupBroError {
  path: string;
  message: string;
}

export interface FedupBroResult {
  duplicateGroups: number;
  duplicateFiles: number;
  reclaimableBytes: number;
  scannedFiles: number;
  scannedBytes: number;
  groups: FedupBroGroup[];
  errors: FedupBroError[];
  groupOffset: number;
  groupLimit: number;
  groupTotal: number;
  errorOffset: number;
  errorLimit: number;
  errorTotal: number;
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
  kind?: "conflict" | "verification" | "bro-output" | string;
  source: string;
  destination: string;
  message?: string;
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
  /** When non-empty, the binding only matches when the focused panel's
   *  viewMode is in this list. Absent/empty = applies in all modes. */
  modes?: ("list" | "grid")[];
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
  kind: "file-op" | "fp-bro" | "search" | "outgoing-transfer" | "sync-bro" | "fedup-bro" | "remote-manifest";
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
export type ConflictPolicy = "ask" | "skip" | "overwrite" | "resume";
export type VerificationPolicy = "fail" | "prompt";

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

export interface SyncBroResultMessage {
  type: "SyncBroResult";
  timestamp: number;
  jobId: string;
  result: SyncBroResult;
}

export interface FedupBroResultMessage {
  type: "FedupBroResult";
  timestamp: number;
  jobId: string;
  result: FedupBroResult;
}

export interface FedupBroResultPageMessage {
  type: "FedupBroResultPage";
  timestamp: number;
  jobId: string;
  result: FedupBroResult;
}

export interface FedupReportSavedMessage {
  type: "FedupReportSaved";
  timestamp: number;
  jobId: string;
  path: string;
}

export interface FedupDeleteDryRunReadyMessage {
  type: "FedupDeleteDryRunReady";
  timestamp: number;
  jobId: string;
  scriptPath: string;
  manifestPath: string;
  scriptDir: string;
}

export interface SyncReportSavedMessage {
  type: "SyncReportSaved";
  timestamp: number;
  jobId: string;
  path: string;
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

export interface BookmarkEntry {
  name: string;
  path: string;
}

export interface GeneralSettings {
  showHiddenDefault: boolean;
  rememberWindowState: boolean;
  preserveAttributes: boolean;
  autoResumeCopies: boolean;
  ludicrousMode: boolean;
  maxImageSizeMb: number;
  savePanelViewMode: boolean;
  enableVideoThumbnails: boolean;
  thumbnailMaxImageSizeMb: number;
  thumbnailVideoFrameSkip: number;
  streamThumbnails: boolean;
  sessionPath: string;
  bookmarksPath: string;
  /** Empty string on platforms that don't use the fbro-managed thumbnailer. */
  thumbnailCacheDir: string;
  thumbnailCacheItemCount: number;
  thumbnailCacheSizeBytes: number;
  persistPanelTabs: boolean;
  procTreeMode: boolean;
  colorScheme: string;
  /** "auto" | "custom" | "internal" */
  sevenzipExtractor: string;
  sevenzipCustomPath: string;
  /** Absolute path the backend resolved given the mode + custom path; empty if it'll fall back to the in-process extractor. */
  sevenzipResolvedPath: string;
  /** Newline-separated wildcard names Fedup Bro ignores when default ignores are enabled. */
  fedupDefaultIgnorePatterns: string;
}

export interface ModalSizePreference {
  width: number;
  height: number;
}

export interface SettingsState {
  general: GeneralSettings;
  modalSizes: Record<string, ModalSizePreference>;
}

export interface ChildProcess {
  pid: number;
  name: string;
  label: string;
  started_at: number;
}

export interface StateSnapshotMessage {
  type: "StateSnapshot";
  timestamp: number;
  domains: {
    layout?: LayoutDomain;
    panels?: Record<string, PanelState>;
    slots?: Record<string, PanelSlot>;
    sources?: Source[];
    focus?: string;
    jobs?: Job[];
    dirSizeQueue?: { tasks: DirSizeTask[] };
    commands?: InteractiveCommand[];
    bookmarks?: BookmarkEntry[];
    fontSize?: number;
    settings?: SettingsState;
    discoveredPeers?: DiscoveredPeer[];
    shareConfig?: ShareConfig;
    globalSelections?: GlobalSelectionSummary[];
  };
}

export interface StatePatchMessage {
  type: "StatePatch";
  timestamp: number;
  domain: "panels" | "layout" | "slots" | "focus" | "selections" | "jobs" | "sources" | "dirSizeQueue" | "bookmarks" | "procs" | "settings" | "discoveredPeers" | "shareConfig" | "globalSelections";
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
  event: "Queued" | "Progress" | "DecisionRequired" | "Completed" | "Failed" | "Cancelled" | "Paused" | "Resumed" | "FileSkipped";
  files?: FileOpFile[];
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

export interface TouchFileOkMessage {
  type: "TouchFileOk";
  timestamp: number;
  panelId: string;
}

export interface SetPermissionsOkMessage {
  type: "SetPermissionsOk";
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
  isBinary?: boolean;
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

export interface PrefetchCacheStatus {
  entryCount: number;
  totalBytes: number;
  maxEntries: number;
}

export interface CacheStatusReadyMessage {
  type: "CacheStatusReady";
  timestamp: number;
  prefetchCache: PrefetchCacheStatus;
}

export interface TerminalSessionOpenedMessage {
  type: "TerminalSessionOpened";
  timestamp: number;
  panelId: string;
  sessionId: number;
}

export interface TerminalClosedMessage {
  type: "TerminalClosed";
  timestamp: number;
  panelId: string;
  sessionId: number;
}

// ─── Remote / pairing events ─────────────────────────────────────────────────

export interface PairingRequestedMessage {
  type: "PairingRequested";
  timestamp: number;
  handshakeId: string;
  /** Zero-padded 6-digit string. */
  code: string;
  /** Self-reported machine name of the other side. */
  name: string;
  os?: string;
  pubkeyAlgo: string;
  /** Unix millis when the handshake auto-aborts. */
  expiresAt: number;
}

export interface PairingResolvedMessage {
  type: "PairingResolved";
  timestamp: number;
  handshakeId: string;
  outcome: "paired" | "rejected" | "timeout";
  fingerprint?: string;
}

export interface RemoteSessionStartedMessage {
  type: "RemoteSessionStarted";
  timestamp: number;
  fingerprint: string;
  sourceId: string;
}

export interface RemoteSessionEndedMessage {
  type: "RemoteSessionEnded";
  timestamp: number;
  fingerprint: string;
  sourceId: string;
}

export type RemoteSourceStatusValue =
  | "connected"
  | "reconnecting"
  | "disconnected"
  | "dead";

export interface RemoteSourceStatusMessage {
  type: "RemoteSourceStatus";
  timestamp: number;
  sourceId: string;
  status: RemoteSourceStatusValue;
  message?: string;
  /** Unix millis of the next scheduled reconnect attempt. */
  nextRetryAt?: number;
}

/** Per-source status as stored in the FE state, indexed by sourceId. */
export interface RemoteSourceStatus {
  status: RemoteSourceStatusValue;
  message?: string;
  nextRetryAt?: number;
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
  | TouchFileOkMessage
  | SetPermissionsOkMessage
  | NavigateOkMessage
  | SyncBroResultMessage
  | FedupBroResultMessage
  | FedupBroResultPageMessage
  | FedupReportSavedMessage
  | FedupDeleteDryRunReadyMessage
  | SyncReportSavedMessage
  | ExternalClipboardReadyMessage
  | ExternalClipboardWrittenMessage
  | ViewerFileChangedMessage
  | DryRunReadyMessage
  | OpenWithOptionsReadyMessage
  | OpenWithOptionsFailedMessage
  | BroOptionsReadyMessage
  | BroOptionsFailedMessage
  | InputRequiredMessage
  | CacheStatusReadyMessage
  | TerminalSessionOpenedMessage
  | TerminalClosedMessage
  | PairingRequestedMessage
  | PairingResolvedMessage
  | RemoteSessionStartedMessage
  | RemoteSessionEndedMessage
  | RemoteSourceStatusMessage;

export type FEMessage = HelloMessage | IntentMessage;
