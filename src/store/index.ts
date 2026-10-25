import { createContext, useContext, useReducer, type Dispatch } from "react";
import { isSearchSourceId } from "../utils/sourceId";
import { applySlotUpdate } from "./slots";
import { disposeCachedTerminal } from "../components/PanelTerminal";
import { applyNotificationPatch, applyNotificationSnapshot, createNotificationState, expireNotificationToast, type NotificationState } from "../notifications/state";
import type { NotificationRequest, NotificationRequestAction } from "../notifications/service";
import type {
  PanelState,
  PanelSlot,
  SearchParams,
  SearchResult,
  Source,
  Job,
  JobQueue,
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
  FedupDeleteDryRunReadyMessage,
  OpenWithAppItem,
  OpenWithOptionsFailedMessage,
  OpenWithOptionsReadyMessage,
  BroInputItem,
  BroItem,
  BroCatalogItem,
  BroCatalogFailedMessage,
  BroCatalogReadyMessage,
  BroOptionsFailedMessage,
  BroOptionsReadyMessage,
  GitChange,
  GitLogEntry,
  GitLogReadyMessage,
  GitCommitShowReadyMessage,
  GitActionFinishedMessage,
  ExternalClipboardReadyMessage,
  CacheStatusReadyMessage,
  ThumbnailQueueReadyMessage,
  PrefetchCacheStatus,
  BookmarkEntry,
  ChildProcess,
  GeneralSettings,
  SettingsState,
  TerminalClosedMessage,
  DiscoveredPeer,
  ShareConfig,
  PendingPairing,
  RemoteSourceStatus,
  GlobalSelectionSummary,
  NotificationsPatch,
  NotificationInput,
} from "../ws/types";

// ─── State ───────────────────────────────────────────────────────────────────

const MAX_HISTORY = 50;

export interface HistoryEntry { path: string; sourceId: string; }
export interface PanelHistoryState { entries: HistoryEntry[]; cursor: number; }

// Side-channel flag: panels whose next navigation StatePatch must NOT push a
// new history entry (because the navigation was triggered by back/forward/jump).
const pendingHistoryNavPanels = new Set<string>();
export function markHistoryNavPending(panelId: string) {
  pendingHistoryNavPanels.add(panelId);
}

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
  targetKind?: "file" | "directory" | "other";
  linkTarget?: string;
  fullPath: string;
  dirPath: string;
  size?: number;
  mtime?: string;
  btime?: string; // birth time — file creation; not available on all platforms/filesystems
  permissions?: string;
  /** Same as VFSEntry.iconName — drives the Type-row icon in Info modal. */
  iconName?: string;
}

export interface PendingMkdir {
  sourcePanel: string;
  dirPath: string;
  error?: string;
}

export interface PendingGitInit {
  panelId: string;
  sourceId: string;
  targetPath: string;
  requestId: string;
  submitting: boolean;
  error?: string;
}

export interface PendingTouch {
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
  manifestPath?: string;
  manifestName?: string;
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
  isShadow?: boolean;
  targetScope: "entry" | "selection";
  targetCount: number;
}

export type EjectAction = "unmount-volume" | "eject-drive";

export interface PendingEject {
  sourceId: string | null;
  action: EjectAction | null;
  requestId: string | null;
  error: string | null;
}

export interface PendingSourceMenu {
  sourceId: string;
  anchorLeft: number;
  anchorRight: number;
  anchorTop: number;
  anchorBottom: number;
}

export interface PendingGlobalSelectionAdd {
  panelId: string;
  targetPath?: string;
  targetName: string;
  targetCount: number;
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

export interface BroCatalogState {
  requestId: string | null;
  status: "idle" | "loading" | "ready" | "error";
  bros: BroCatalogItem[];
  error?: string;
}

export interface GitCommitShowCache {
  status: "loading" | "ready" | "error";
  requestId: string;
  message: string;
  changes: GitChange[];
  error?: string;
}

export interface PendingGitLog {
  panelId: string;
  requestId: string;
  status: "loading" | "ready" | "error";
  commits: GitLogEntry[];
  hasMore: boolean;
  error?: string;
  /** Accordion: hash of the expanded commit, or null when collapsed. */
  expandedHash: string | null;
  /** Lazy-loaded file lists keyed by full commit hash. */
  commitShows: Record<string, GitCommitShowCache>;
}

export interface PendingInput {
  panelId: string;
  requestId: string;
  kind: "password" | "text";
  prompt: string;
}

export interface PendingSelectionMask {
  panelId: string;
  sourceId: string;
  path: string;
  mode: "select" | "deselect";
  initialMask: string | null;
}

export interface PendingSearch {
  sourcePanelPath: string;
  // Set once the search job starts
  jobId: string | null;
  searchSourceId: string | null;
  results: SearchResult[];
  scanned: number;
  matched: number;
  currentPath: string;
  estimatedPercent?: number;
  status: "idle" | "running" | "completed" | "cancelled" | "failed";
  activeTab: "input" | "results" | "extra";
  // Used to correlate the StartSearch intent with the JobStarted message
  requestId: string | null;
  // The params used for this search — used to pre-fill the Input tab when
  // re-opening a job from the jobs bar.
  searchParams: SearchParams | null;
}

export type SettingsTab = "general" | "display" | "bros" | "sharing";

export interface PendingAttributes {
  panelId: string;
  paths: string[];
  names: string[];
  currentMode: string;   // e.g. "rwxr-xr-x"
  error?: string;
}

export interface PendingSettings {
  activeTab: SettingsTab;
}

export interface AppState {
  connected: boolean;
  sessionId: string | null;
  fileServerPort: number;
  layout: { panels: string[]; slots: string[] };
  panels: Record<string, PanelState>;
  slots: Record<string, PanelSlot>;
  panelHistory: Record<string, PanelHistoryState>;
  sources: Record<string, Source>;
  focus: string;
  jobs: Record<string, Job>;
  jobQueue: JobQueue;
  dirSizeQueue: DirSizeTask[];
  commands: Record<string, InteractiveCommand>;
  pendingOp: PendingOp | null;
  activeJobId: string | null;
  pendingQuit: { activeJobs: number } | null;
  pendingRename: PendingRename | null;
  pendingInfo: PendingInfo | null;
  pendingMkdir: PendingMkdir | null;
  pendingGitInit: PendingGitInit | null;
  pendingTouch: PendingTouch | null;
  pendingSelectionMask: PendingSelectionMask | null;
  pendingNoEditor: PendingNoEditor | null;
  pendingGoToPath: PendingGoToPath | null;
  pendingPanelHistory: PendingPanelHistory | null;
  pendingDryRun: PendingDryRun | null;
  pendingContextMenu: PendingContextMenu | null;
  pendingSourceMenu: PendingSourceMenu | null;
  pendingEject: PendingEject | null;
  pendingGlobalSelectionAdd: PendingGlobalSelectionAdd | null;
  pendingOpenWith: PendingOpenWith | null;
  pendingBro: PendingBro | null;
  broCatalog: BroCatalogState;
  /** In-flight / loaded git log for the repository modal Log tab. */
  pendingGitLog: PendingGitLog | null;
  gitActionResult: GitActionFinishedMessage | null;
  pendingInput: PendingInput | null;
  pendingSearch: PendingSearch | null;
  pendingAttributes: PendingAttributes | null;
  pendingSettings: PendingSettings | null;
  sharedFolderInput: { path: string; requestId: string | null; error?: string };
  commandPaletteOpen: boolean;
  aboutOpen: boolean;
  activeSortSelectorPanelId: string | null;
  /** mDNS-discovered fbro peers on the LAN. Updated via
   *  `StatePatch { domain: "discoveredPeers" }`. */
  discoveredPeers: DiscoveredPeer[];
  /** Sharing toggle, shared sources, paired-peer list. Updated via
   *  `StatePatch { domain: "shareConfig" }`. */
  shareConfig: ShareConfig;
  /** Active pairing handshake awaiting user confirmation. Set on
   *  `PairingRequested`, cleared on `PairingResolved`. */
  pendingPairing: PendingPairing | null;
  /** Live connection status per mounted remote source. Updated by
   *  `RemoteSourceStatus` events from the BE's SFTP supervisor.
   *  Missing keys are implicitly "connected" / unknown. */
  remoteSourceStatus: Record<string, RemoteSourceStatus>;
  notifications: NotificationState;
  notificationRequests: NotificationRequest[];
  notificationConnectionReady: boolean;
  notificationPublishError: string | null;
  embeddedViewer: { sourceSlotId: string } | null;
  cacheStatus: { prefetchCache: PrefetchCacheStatus } | null;
  thumbnailQueue: ThumbnailQueueReadyMessage | null;
  bookmarks: BookmarkEntry[];
  bookmarksOpen: { panelId: string } | null;
  maximizedTerminalPanelId: string | null;
  childProcesses: Record<number, ChildProcess>;
  globalSelections: GlobalSelectionSummary[];
  fontSize: number;
  settings: SettingsState;
}

const MAXIMIZED_TERMINAL_STORAGE_KEY = "fbro.maximizedTerminalPanelId";

function readMaximizedTerminalPanelId(): string | null {
  if (typeof window === "undefined") return null;
  try {
    return window.sessionStorage.getItem(MAXIMIZED_TERMINAL_STORAGE_KEY);
  } catch {
    return null;
  }
}

function storeMaximizedTerminalPanelId(panelId: string | null) {
  if (typeof window === "undefined") return;
  try {
    if (panelId === null) window.sessionStorage.removeItem(MAXIMIZED_TERMINAL_STORAGE_KEY);
    else window.sessionStorage.setItem(MAXIMIZED_TERMINAL_STORAGE_KEY, panelId);
  } catch {
    // Session storage can be unavailable in restricted WebViews.
  }
}

const initialState: AppState = {
  connected: false,
  sessionId: null,
  fileServerPort: 0,
  layout: { panels: [], slots: [] },
  panels: {},
  slots: {},
  panelHistory: {},
  sources: {},
  focus: "",
  jobs: {},
  jobQueue: { jobIds: [], completedCount: 0 },
  dirSizeQueue: [],
  commands: {},
  pendingOp: null,
  activeJobId: null,
  pendingQuit: null,
  pendingRename: null,
  pendingInfo: null,
  pendingMkdir: null,
  pendingGitInit: null,
  pendingTouch: null,
  pendingSelectionMask: null,
  pendingNoEditor: null,
  pendingGoToPath: null,
  pendingPanelHistory: null,
  pendingDryRun: null,
  pendingContextMenu: null,
  pendingSourceMenu: null,
  pendingEject: null,
  pendingGlobalSelectionAdd: null,
  pendingOpenWith: null,
  pendingBro: null,
  broCatalog: { requestId: null, status: "idle", bros: [] },
  pendingGitLog: null,
  gitActionResult: null,
  pendingInput: null,
  pendingSearch: null,
  pendingAttributes: null,
  pendingSettings: null,
  commandPaletteOpen: false,
  aboutOpen: false,
  activeSortSelectorPanelId: null,
  discoveredPeers: [],
  shareConfig: { enabled: false, allowIpv6: false, sharedSourceIds: [], sharedRoots: [], shareSuggestions: [], pairedPeers: [] },
  sharedFolderInput: { path: "", requestId: null },
  pendingPairing: null,
  remoteSourceStatus: {},

  notifications: createNotificationState(),
  notificationRequests: [],
  notificationConnectionReady: false,
  notificationPublishError: null,
  embeddedViewer: null,
  cacheStatus: null,
  thumbnailQueue: null,
  bookmarks: [],
  bookmarksOpen: null,
  maximizedTerminalPanelId: readMaximizedTerminalPanelId(),
  childProcesses: {},
  globalSelections: [],
  fontSize: 13,
  settings: {
    general: { showHiddenDefault: false, showButtonBar: true, osNotificationsEnabled: true, supportsOsNotifications: false, supportsOsNotificationSettings: false, rememberWindowState: true, preserveAttributes: true, autoResumeCopies: true, ludicrousMode: false, enableHeic2any: false, maxImageSizeMb: 512, savePanelViewMode: false, rememberSortPerLocation: false, locationSettingsCount: 0, iconCacheDir: "", iconCacheRevision: 0, supportsManagedThumbnails: false, enableVideoThumbnails: true, thumbnailMaxImageSizeMb: 100, thumbnailVideoFrameSkip: 5, streamThumbnails: true, sessionPath: "", bookmarksPath: "", thumbnailCacheDir: "", thumbnailCacheItemCount: 0, thumbnailCacheSizeBytes: 0, persistPanelTabs: true, procTreeMode: false, colorScheme: "fbro-dark", sevenzipExtractor: "auto", sevenzipCustomPath: "", sevenzipResolvedPath: "", fedupDefaultIgnorePatterns: ".git\n.idea\n.vscode\n.venv\nnode_modules\n__MACOSX\n._*\n.DS_Store\nartifacts/e2e\ndist/assets\ntarget/debug\ntarget/release\ntarget/bundle\n.m2", compareMaxFileSizeMb: 10, compareDifferMode: "internal", compareExternalCommand: "", quickTerminalEditor: "nano", zshEmacsKeymap: false },
    modalSizes: {},
  },
};

// ─── Actions ─────────────────────────────────────────────────────────────────

export type AppAction =
  | { type: "CONNECTED" }
  | { type: "DISCONNECTED" }
  | { type: "BE_MESSAGE"; msg: BEMessage; notificationRequestId: string }
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
  | { type: "OPEN_GIT_INIT"; gitInit: PendingGitInit }
  | { type: "SUBMIT_GIT_INIT"; requestId: string }
  | { type: "CLOSE_GIT_INIT" }
  | { type: "OPEN_TOUCH"; touch: PendingTouch }
  | { type: "CLOSE_TOUCH" }
  | { type: "OPEN_SELECTION_MASK"; selectionMask: PendingSelectionMask }
  | { type: "CLOSE_SELECTION_MASK" }
  | { type: "OPEN_NO_EDITOR"; noEditor: PendingNoEditor }
  | { type: "CLOSE_NO_EDITOR" }
  | { type: "OPEN_GO_TO_PATH"; goToPath: PendingGoToPath }
  | { type: "CLOSE_GO_TO_PATH" }
  | { type: "OPEN_PANEL_HISTORY"; panelHistory: PendingPanelHistory }
  | { type: "CLOSE_PANEL_HISTORY" }
  | { type: "CLOSE_DRY_RUN" }
  | { type: "OPEN_CONTEXT_MENU"; menu: PendingContextMenu }
  | { type: "CLOSE_CONTEXT_MENU" }
  | { type: "OPEN_SOURCE_MENU"; menu: PendingSourceMenu }
  | { type: "CLOSE_SOURCE_MENU" }
  | { type: "OPEN_EJECT"; eject: PendingEject }
  | { type: "CLOSE_EJECT" }
  | { type: "SUBMIT_EJECT"; requestId: string; sourceId: string; action: EjectAction }
  | { type: "SET_EJECT_SELECTION"; sourceId: string; action: EjectAction }
  | { type: "OPEN_GLOBAL_SELECTION_ADD"; add: PendingGlobalSelectionAdd }
  | { type: "CLOSE_GLOBAL_SELECTION_ADD" }
  | { type: "OPEN_OPEN_WITH"; openWith: PendingOpenWith }
  | { type: "CLOSE_OPEN_WITH" }
  | { type: "OPEN_BRO"; bro: PendingBro }
  | { type: "CLOSE_BRO" }
  | { type: "REQUEST_BRO_CATALOG"; requestId: string }
  | { type: "REQUEST_GIT_LOG"; panelId: string; requestId: string; append: boolean }
  | { type: "REQUEST_GIT_COMMIT_SHOW"; panelId: string; requestId: string; hash: string }
  | { type: "SET_GIT_LOG_EXPANDED"; hash: string | null }
  | { type: "CLEAR_GIT_LOG" }
  | { type: "OPEN_SEARCH"; search: PendingSearch }
  | { type: "CLOSE_SEARCH" }
  | { type: "SET_SEARCH_TAB"; tab: "input" | "results" | "extra" }
  | { type: "SET_SEARCH_REQUEST_ID"; requestId: string }
  | { type: "RESET_SEARCH_FOR_NEW_JOB"; requestId: string }
  | { type: "OPEN_COMMAND_PALETTE" }
  | { type: "CLOSE_COMMAND_PALETTE" }
  | { type: "OPEN_ABOUT" }
  | { type: "CLOSE_ABOUT" }
  | { type: "OPEN_SORT_SELECTOR"; panelId: string }
  | { type: "CLOSE_SORT_SELECTOR" }
  | NotificationRequestAction
  | { type: "EXPIRE_NOTIFICATION_TOAST"; id: string }
  | { type: "CLOSE_INPUT" }
  | { type: "TOGGLE_EMBEDDED_VIEWER" }
  | { type: "NAV_HISTORY_BACK";    panelId: string }
  | { type: "NAV_HISTORY_FORWARD"; panelId: string }
  | { type: "NAV_HISTORY_JUMP";    panelId: string; entryIndex: number }
  | { type: "OPEN_BOOKMARKS"; panelId: string }
  | { type: "CLOSE_BOOKMARKS" }
  | { type: "OPEN_ATTRIBUTES"; attributes: PendingAttributes }
  | { type: "CLOSE_ATTRIBUTES" }
  | { type: "OPEN_SETTINGS"; tab: SettingsTab }
  | { type: "CLOSE_SETTINGS" }
  | { type: "SET_SHARED_FOLDER_INPUT"; path: string }
  | { type: "SUBMIT_SHARED_FOLDER"; requestId: string }
  | { type: "SET_PAIRING_DECISION"; handshakeId: string; accept: boolean }
  | { type: "SET_MAXIMIZED_TERMINAL"; panelId: string | null };

// ─── Reducer ─────────────────────────────────────────────────────────────────

function applySnapshot(state: AppState, msg: StateSnapshotMessage): AppState {
  const { domains } = msg;
  let next = { ...state };

  if (domains.layout) {
    next.layout = {
      panels: domains.layout.panels,
      slots: domains.layout.slots ?? [],
    };
  }
  if (domains.slots) {
    next.slots = Object.fromEntries(Object.entries(domains.slots).map(([id, slot]) => [
      id, applySlotUpdate(state.slots[id], id, slot.version, slot)!,
    ]));
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
      panels[id] = { ...p, panelId: id, entries: p.entries ?? [], selection: p.selection ?? [], selectedEntries: p.selectedEntries ?? [], version: p.version ?? 0, panelEpoch: p.panelEpoch ?? 0, calculatingSize: p.calculatingSize ?? [], totalCount: p.totalCount, totalSize: p.totalSize, windowStart: p.windowStart, filterModeActive: p.filterModeActive ?? false, showHidden: p.showHidden ?? false, viewMode: p.viewMode ?? "list", supportsThumbnails: p.supportsThumbnails ?? false };
    }
    next.panels = panels;
    next.panelHistory = Object.fromEntries(
      Object.keys(panels).map((panelId) => {
        const existing = state.panelHistory[panelId];
        if (existing) return [panelId, existing];
        const p = panels[panelId];
        return [panelId, { entries: [{ path: p.path, sourceId: p.sourceId }], cursor: 0 }];
      }),
    );
  }
  if (domains.jobs) {
    const jobs: Record<string, Job> = {};
    for (const j of domains.jobs) jobs[j.jobId] = j;
    next.jobs = jobs;
  }
  if (domains.jobQueue) {
    next.jobQueue = {
      activeJobId: domains.jobQueue.activeJobId,
      jobIds: domains.jobQueue.jobIds ?? [],
      completedCount: domains.jobQueue.completedCount ?? 0,
    };
  }
  if (domains.dirSizeQueue) {
    next.dirSizeQueue = domains.dirSizeQueue.tasks ?? [];
  }
  if (domains.commands) {
    const commands: Record<string, InteractiveCommand> = {};
    for (const command of domains.commands) commands[command.id] = command;
    next.commands = commands;
  }
  if (domains.bookmarks) {
    next.bookmarks = domains.bookmarks;
  }
  if (domains.fontSize !== undefined) {
    next.fontSize = domains.fontSize;
  }
  if (domains.settings !== undefined) {
    next.settings = {
      ...domains.settings,
      modalSizes: domains.settings.modalSizes ?? {},
    };
  }
  if (domains.discoveredPeers !== undefined) {
    next.discoveredPeers = domains.discoveredPeers;
  }
  if (domains.shareConfig !== undefined) {
    next.shareConfig = domains.shareConfig;
  }
  if (domains.globalSelections !== undefined) {
    next.globalSelections = domains.globalSelections;
  }
  if (domains.notifications !== undefined) {
    next.notifications = applyNotificationSnapshot(state.notifications, domains.notifications);
  }
  if (next.maximizedTerminalPanelId
      && next.panels[next.maximizedTerminalPanelId]?.terminalSessionId === undefined) {
    storeMaximizedTerminalPanelId(null);
    next.maximizedTerminalPanelId = null;
  }
  return next;
}

function applyPatch(state: AppState, msg: StatePatchMessage): AppState {
  if (msg.domain === "notifications") {
    return { ...state, notifications: applyNotificationPatch(state.notifications, msg.version, msg.patch as NotificationsPatch) };
  }
  if (msg.domain === "panels" && msg.entityId) {
    const panelId = msg.entityId;
    let panel = state.panels[panelId];
    if (!panel) {
      // Unknown panel id — accept it iff this patch is a "create" payload
      // carrying enough fields to seed a PanelState. The BE emits one of
      // these when a tab is opened so the FE can render the new panel
      // without paying for a full StateSnapshot.
      const p = msg.patch as Partial<PanelState> & { create?: boolean };
      if (!p.create || !p.slotId || !p.sourceId || p.path === undefined) return state;
      const fresh: PanelState = {
        panelId,
        slotId: p.slotId,
        tabKind: p.tabKind ?? "listing",
        tabTitle: p.tabTitle,
        path: p.path,
        status: p.status ?? "loading",
        sourceId: p.sourceId,
        sort: p.sort ?? { by: "name", order: "asc" },
        filter: null,
        filterModeActive: false,
        showHidden: p.showHidden ?? false,
        viewMode: p.viewMode ?? "list",
        supportsThumbnails: p.supportsThumbnails ?? false,
        cursor: 0,
        selection: [],
        selectedEntries: [],
        entries: [],
        version: msg.version ?? 0,
        panelEpoch: p.panelEpoch ?? 0,
        calculatingSize: [],
        shadowSource: p.shadowSource,
        terminalSessionId: p.terminalSessionId,
        git: p.git,
      };
      return {
        ...state,
        panels: { ...state.panels, [panelId]: fresh },
        panelHistory: {
          ...state.panelHistory,
          [panelId]: { entries: [{ path: fresh.path, sourceId: fresh.sourceId }], cursor: 0 },
        },
      };
    }

    const incomingEpoch = typeof msg.patch.panelEpoch === "number" ? msg.patch.panelEpoch : undefined;
    const locationPatch = msg.patch.path !== undefined || msg.patch.sourceId !== undefined;
    const currentEpoch = panel.panelEpoch ?? 0;
    const epochAdvances = incomingEpoch !== undefined && incomingEpoch > currentEpoch;
    if (incomingEpoch !== undefined) {
      if ((!locationPatch && incomingEpoch !== currentEpoch)
       || (locationPatch && incomingEpoch < currentEpoch)) {
        return state;
      }
    }

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
    // Only pre-guard cursor for cursor-only patches (MoveCursor/SetCursor).
    // Scan patches that include entries alongside cursor must not bypass the
    // version guard — a quiet_rescan completing concurrently with a SetCursor
    // can have the same version number, and its stale cursor value must not
    // silently overwrite what the user just clicked.
    const patchHasEntries =
      msg.patch.entries !== undefined ||
      msg.patch.appendEntries !== undefined ||
      msg.patch.windowEntries !== undefined;
    if (msg.patch.cursor !== undefined && !patchHasEntries) {
      preGuardPanel = { ...preGuardPanel, cursor: msg.patch.cursor as number };
    }
    if (msg.patch.selection !== undefined && !patchHasEntries) {
      preGuardPanel = { ...preGuardPanel, selection: msg.patch.selection as number[] };
    }
    if (msg.patch.selectedEntries !== undefined && !patchHasEntries) {
      preGuardPanel = { ...preGuardPanel, selectedEntries: msg.patch.selectedEntries as PanelState["selectedEntries"] };
    }
    if (msg.patch.calculatingSize !== undefined) {
      preGuardPanel = { ...preGuardPanel, calculatingSize: msg.patch.calculatingSize as string[] };
    }
    // Window-slice fields (windowEntries / windowStart / totalCount) ride on
    // their own monotonic `windowVersion`, decoupled from the structural
    // version. They must be applied pre-guard so a fast scroll cannot be
    // dropped by the structural-version guard during interleaved patches
    // (e.g. cursor/dirsize/selection updates between two send_window responses).
    if (msg.patch.windowVersion !== undefined && msg.patch.windowEntries !== undefined) {
      const incoming = msg.patch.windowVersion as number;
      const current = preGuardPanel.windowVersion ?? 0;
      if (incoming > current) {
        preGuardPanel = {
          ...preGuardPanel,
          entries: msg.patch.windowEntries as VFSEntry[],
          windowStart: msg.patch.windowStart === null || msg.patch.windowStart === undefined
            ? preGuardPanel.windowStart
            : msg.patch.windowStart as number,
          totalCount: msg.patch.totalCount === null || msg.patch.totalCount === undefined
            ? preGuardPanel.totalCount
            : msg.patch.totalCount as number,
          windowVersion: incoming,
        };
      }
    }
    if (preGuardPanel !== panel) {
      state = { ...state, panels: { ...state.panels, [panelId]: preGuardPanel } };
      panel = preGuardPanel;
    }

    // Version guard — drop structurally stale patches (old scan data arriving
    // after a newer scan has already produced results).
    if (msg.version <= panel.version && msg.version !== 0 && !epochAdvances) return state;

    let updated = {
      ...panel,
      version: epochAdvances ? msg.version : Math.max(panel.version, msg.version),
    };

    if (incomingEpoch !== undefined) {
      updated = { ...updated, panelEpoch: incomingEpoch };
    }
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
    if (msg.patch.showHidden !== undefined) {
      updated = { ...updated, showHidden: msg.patch.showHidden as boolean };
    }
    if (msg.patch.viewMode !== undefined) {
      updated = { ...updated, viewMode: msg.patch.viewMode as PanelState["viewMode"] };
    }
    if (msg.patch.supportsThumbnails !== undefined) {
      updated = { ...updated, supportsThumbnails: msg.patch.supportsThumbnails as boolean };
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
    // Window-slice fields are gated by their own monotonic windowVersion (see
    // pre-guard above). For patches that ARE structurally fresh (passed the
    // version guard) but whose windowVersion is stale relative to the slice
    // already in state, do NOT overwrite. windowStart=null (sent by rescan to
    // reset state) is a structural reset and is allowed through unconditionally.
    if (msg.patch.windowStart === null) {
      updated = { ...updated, windowStart: undefined };
    }
    if (msg.patch.windowEntries !== undefined && msg.patch.windowEntries !== null) {
      const incoming = msg.patch.windowVersion as number | undefined;
      const current = updated.windowVersion ?? 0;
      if (incoming === undefined || incoming > current) {
        updated = {
          ...updated,
          entries: msg.patch.windowEntries as VFSEntry[],
          windowStart: msg.patch.windowStart === null || msg.patch.windowStart === undefined
            ? updated.windowStart
            : msg.patch.windowStart as number,
          windowVersion: incoming ?? current,
        };
      }
    } else if (msg.patch.windowStart !== undefined && msg.patch.windowStart !== null) {
      // windowStart-only patches (rare; current BE always co-emits with entries)
      updated = { ...updated, windowStart: msg.patch.windowStart as number };
    }
    if (msg.patch.shadowSource !== undefined) {
      updated = { ...updated, shadowSource: msg.patch.shadowSource === null ? undefined : msg.patch.shadowSource as PanelState["shadowSource"] };
    }
    if (msg.patch.terminalSessionId !== undefined) {
      updated = { ...updated, terminalSessionId: msg.patch.terminalSessionId === null ? undefined : msg.patch.terminalSessionId as number };
    }
    if (msg.patch.git !== undefined) {
      updated = { ...updated, git: msg.patch.git === null ? undefined : msg.patch.git as PanelState["git"] };
    }
    if (msg.patch.viewportAnchor !== undefined && msg.patch.viewportAnchor !== null) {
      const viewportAnchor = msg.patch.viewportAnchor as NonNullable<PanelState["viewportAnchor"]>;
      const zebraPhase = ((updated.zebraPhase ?? 0) + viewportAnchor.delta) % 2;
      updated = {
        ...updated,
        viewportAnchor,
        zebraPhase: zebraPhase < 0 ? zebraPhase + 2 : zebraPhase,
      };
    } else if (msg.patch.viewportAnchor === null
        || (msg.patch.entries !== undefined && msg.patch.status === "loading")) {
      updated = { ...updated, zebraPhase: 0 };
    }

    const locationChanged = updated.path !== panel.path
      || (msg.patch.sourceId !== undefined && updated.sourceId !== panel.sourceId);

    if (locationChanged) {
      // Bookmarks, history, procs, and global selections are navigation/virtual tools, not real filesystem
      // locations — entering or exiting them must not create a history entry.
      if (updated.sourceId === "bookmarks" || panel.sourceId === "bookmarks"
       || updated.sourceId === "history"  || panel.sourceId === "history"
       || updated.sourceId === "procs"    || panel.sourceId === "procs"
       || updated.sourceId === "global-selections" || panel.sourceId === "global-selections") {
        return { ...state, panels: { ...state.panels, [panelId]: updated } };
      }
      if (pendingHistoryNavPanels.has(panelId)) {
        pendingHistoryNavPanels.delete(panelId);
        // cursor already moved by the dispatch; just commit the panel state
        return { ...state, panels: { ...state.panels, [panelId]: updated } };
      }
      const newEntry: HistoryEntry = { path: updated.path, sourceId: updated.sourceId };
      const hist: PanelHistoryState = state.panelHistory[panelId]
        ?? { entries: [{ path: panel.path, sourceId: panel.sourceId }], cursor: 0 };
      const nextFwd = hist.entries[hist.cursor + 1];
      let newHist: PanelHistoryState;
      if (nextFwd?.path === newEntry.path && nextFwd?.sourceId === newEntry.sourceId) {
        // Matches next-forward entry → advance cursor, preserve forward history
        newHist = { ...hist, cursor: hist.cursor + 1 };
      } else if (newEntry.path !== hist.entries[hist.cursor]?.path
              || newEntry.sourceId !== hist.entries[hist.cursor]?.sourceId) {
        // Distinct new location → truncate forward history, push new entry
        const base = [...hist.entries.slice(0, hist.cursor + 1), newEntry];
        const capped = base.length > MAX_HISTORY ? base.slice(1) : base;
        newHist = { entries: capped, cursor: capped.length - 1 };
      } else {
        newHist = hist; // same as current position — no change
      }
      return {
        ...state,
        panels: { ...state.panels, [panelId]: updated },
        panelHistory: { ...state.panelHistory, [panelId]: newHist },
      };
    }

    return { ...state, panels: { ...state.panels, [panelId]: updated } };
  }

  if (msg.domain === "focus") {
    return { ...state, focus: msg.patch.focus as string };
  }

  if (msg.domain === "layout") {
    const patch = msg.patch as { panels?: string[]; slots?: string[] };
    return {
      ...state,
      layout: {
        panels: patch.panels ?? state.layout.panels,
        slots: patch.slots ?? state.layout.slots,
      },
    };
  }

  if (msg.domain === "slots") {
    const slotId = msg.entityId;
    const patch = msg.patch as Partial<PanelSlot> & {
      remove?: boolean;
      layoutSlots?: string[];
    };
    let nextSlots = state.slots;
    let nextLayout = state.layout;

    if (slotId) {
      const existing = nextSlots[slotId];
      if (existing && msg.version <= existing.version) return state;
      if (patch.remove) {
        nextSlots = { ...nextSlots };
        delete nextSlots[slotId];
      } else {
        const merged = applySlotUpdate(existing, slotId, msg.version, patch)!;
        nextSlots = { ...nextSlots, [slotId]: merged };
      }
    }

    if (patch.layoutSlots) {
      nextLayout = { ...nextLayout, slots: patch.layoutSlots };
    }
    return { ...state, slots: nextSlots, layout: nextLayout };
  }

  if (msg.domain === "sources") {
    const newSources: Record<string, Source> = {};
    for (const s of msg.patch.sources as Source[]) newSources[s.id] = s;

    // Clean up history entries referencing removed search sources.
    const removedSearchIds = Object.keys(state.sources)
      .filter(id => isSearchSourceId(id) && !newSources[id]);

    if (removedSearchIds.length > 0) {
      let ph = state.panelHistory;
      for (const rid of removedSearchIds) {
        for (const [pid, hist] of Object.entries(ph)) {
          const filtered = hist.entries.filter(e => e.sourceId !== rid);
          if (filtered.length === hist.entries.length) continue;
          const removedBefore = hist.entries.slice(0, hist.cursor).filter(e => e.sourceId === rid).length;
          const cursorGone = hist.entries[hist.cursor]?.sourceId === rid;
          const rawCursor = cursorGone
            ? Math.max(0, hist.cursor - removedBefore - 1)
            : hist.cursor - removedBefore;
          const safeEntries = filtered.length > 0 ? filtered : [hist.entries[hist.cursor]];
          ph = { ...ph, [pid]: { entries: safeEntries, cursor: Math.min(rawCursor, safeEntries.length - 1) } };
        }
      }
      return { ...state, sources: newSources, panelHistory: ph };
    }

    return { ...state, sources: newSources };
  }

  if (msg.domain === "dirSizeQueue") {
    return { ...state, dirSizeQueue: (msg.patch.tasks as DirSizeTask[]) ?? [] };
  }

  if (msg.domain === "bookmarks") {
    return { ...state, bookmarks: (msg.patch.bookmarks as BookmarkEntry[]) ?? [] };
  }

  if (msg.domain === "settings") {
    const fontSize = msg.patch.fontSize as number | undefined;
    const general = msg.patch.general as GeneralSettings | undefined;
    const modalSizes = msg.patch.modalSizes as SettingsState["modalSizes"] | undefined;
    let next = fontSize !== undefined ? { ...state, fontSize } : state;
    if (general !== undefined) {
      next = { ...next, settings: { ...next.settings, general: { ...next.settings.general, ...general } } };
    }
    if (modalSizes !== undefined) {
      next = { ...next, settings: { ...next.settings, modalSizes } };
    }
    return next;
  }

  if (msg.domain === "discoveredPeers") {
    const peers = (msg.patch.peers as DiscoveredPeer[]) ?? [];
    return { ...state, discoveredPeers: peers };
  }

  if (msg.domain === "shareConfig") {
    if (msg.version < (state.shareConfig.version ?? 0)) return state;
    const enabled = msg.patch.enabled as boolean | undefined;
    const allowIpv6 = msg.patch.allowIpv6 as boolean | undefined;
    const sharedSourceIds = msg.patch.sharedSourceIds as string[] | undefined;
    const sharedRoots = msg.patch.sharedRoots as ShareConfig["sharedRoots"] | undefined;
    const shareSuggestions = msg.patch.shareSuggestions as ShareConfig["shareSuggestions"] | undefined;
    const pairedPeers = msg.patch.pairedPeers as ShareConfig["pairedPeers"] | undefined;
    const fingerprint = msg.patch.fingerprint as string | undefined;
    return {
      ...state,
      shareConfig: {
        version: msg.version,
        enabled: enabled ?? state.shareConfig.enabled,
        allowIpv6: allowIpv6 ?? state.shareConfig.allowIpv6,
        sharedSourceIds: sharedSourceIds ?? state.shareConfig.sharedSourceIds,
        sharedRoots: sharedRoots ?? state.shareConfig.sharedRoots,
        shareSuggestions: shareSuggestions ?? state.shareConfig.shareSuggestions,
        pairedPeers: pairedPeers ?? state.shareConfig.pairedPeers,
        fingerprint: fingerprint ?? state.shareConfig.fingerprint,
      },
    };
  }

  if (msg.domain === "globalSelections") {
    return {
      ...state,
      globalSelections: (msg.patch.globalSelections as GlobalSelectionSummary[]) ?? [],
    };
  }

  if (msg.domain === "procs") {
    const list = (msg.patch.procs as ChildProcess[]) ?? [];
    const map: Record<number, ChildProcess> = {};
    for (const p of list) map[p.pid] = p;
    return { ...state, childProcesses: map };
  }

  if (msg.domain === "jobs" && msg.patch.removeJobId) {
    const { [msg.patch.removeJobId as string]: _removed, ...remainingJobs } = state.jobs;
    return { ...state, jobs: remainingJobs };
  }

  if (msg.domain === "jobQueue") {
    return {
      ...state,
      jobQueue: {
        activeJobId: msg.patch.activeJobId as string | undefined,
        jobIds: (msg.patch.jobIds as string[]) ?? [],
        completedCount: (msg.patch.completedCount as number) ?? 0,
      },
    };
  }

  return state;
}

function applyJobStarted(state: AppState, msg: JobStartedMessage): AppState {
  const { job } = msg;
  const newJob: Job = {
    jobId: job.jobId,
    kind: job.kind,
    type: job.opType,
    state: job.initialState,
    paused: false,
    supportsPause: job.supportsPause,
    startedAt: msg.timestamp,
    sourcePanel: job.sourcePanel,
    sourcePath: job.sourcePath,
    destPanel: job.destPanel,
    destPath: job.destPath,
    resultPath: job.resultPath,
    resultPanelId: job.resultPanelId,
    files: job.files,
    searchSourceId: job.searchSourceId,
    searchParams: job.searchParams,
    searchResults: job.kind === "search" ? [] : undefined,
    searchScanned: job.kind === "search" ? 0 : undefined,
    searchMatched: job.kind === "search" ? 0 : undefined,
    progress: {
      totalFiles: job.totalFiles,
      completedFiles: 0,
      statusText: undefined,
      currentFileBytesTotal: 0,
      currentFileBytesCopied: 0,
      totalBytesTotal: job.totalBytesTotal,
      totalBytesCopied: 0,
      currentFilePercent: undefined,
      overallPercent: undefined,
    },
  };

  let next = {
    ...state,
    jobs: { ...state.jobs, [job.jobId]: newJob },
    activeJobId: job.background || job.kind === "search" ? state.activeJobId : (state.activeJobId ?? job.jobId),
    pendingOp: null,
  };

  // Correlate with pending search if requestId matches
  if (job.kind === "search" && job.requestId && state.pendingSearch?.requestId === job.requestId) {
    next = {
      ...next,
      pendingSearch: {
        ...state.pendingSearch!,
        jobId: job.jobId,
        searchSourceId: job.searchSourceId ?? null,
        searchParams: job.searchParams ?? null,
        status: "running",
        activeTab: "results",
      },
    };
  }

  return next;
}

function applyQuitConfirmRequired(state: AppState, msg: QuitConfirmRequiredMessage): AppState {
  return { ...state, pendingQuit: { activeJobs: msg.activeJobs } };
}

function applyJobEvent(state: AppState, msg: JobEventMessage): AppState {
  const previousJob = state.jobs[msg.jobId];
  if (!previousJob) return state;
  if (previousJob.state === "Cancelled" && msg.event !== "Cancelled") return state;
  const sp = previousJob.kind === "search" ? msg.searchProgress : undefined;
  // Terminal events can carry the final search snapshot too. Empty activity
  // updates retain the results array instead of copying it every 200 ms.
  const job = sp ? {
    ...previousJob,
    searchResults: sp.newResults.length > 0
      ? [...(previousJob.searchResults ?? []), ...sp.newResults]
      : previousJob.searchResults,
    searchScanned: sp.scanned,
    searchMatched: sp.matched,
    searchCurrentPath: sp.currentPath,
    searchEstimatedPercent: sp.estimatedPercent,
  } : previousJob;
  const files = msg.files ?? job.files;

  let updated: Job;

  switch (msg.event) {
    case "Queued":
      updated = { ...job, files, state: "Queued", paused: false, decision: undefined };
      break;
    case "Progress":
      updated = { ...job, files, state: "Running", paused: false, progress: msg.progress ?? job.progress, decision: undefined };
      break;
    case "DecisionRequired":
      updated = { ...job, files, state: "AwaitingDecision", decision: msg.decision };
      break;
    case "Paused":
      updated = { ...job, files, paused: true };
      break;
    case "Resumed":
      updated = { ...job, files, paused: false };
      break;
    case "FileSkipped":
      updated = {
        ...job,
        files,
        state: "Running",
        paused: false,
        decision: undefined,
      };
      break;
    case "Completed":
      const completedProgress = job.progress
        ? {
            ...job.progress,
            completedFiles: job.progress.totalFiles || job.progress.completedFiles,
            currentFile: undefined,
            statusText: job.kind === "outgoing-transfer"
              ? "Outgoing transfer completed"
              : job.kind === "remote-manifest"
                ? "Remote Sync hashing completed"
              : job.kind === "hash-check-bro"
                ? "Hash check completed"
              : job.kind === "hash-generate-bro"
                ? "Hash generation completed"
              : job.kind === "fp-bro" || job.kind === "sync-bro" || job.kind === "fedup-bro"
                ? "Bro completed successfully"
                : "Operation completed successfully",
            currentFileBytesCopied: job.progress.currentFileBytesTotal || job.progress.currentFileBytesCopied,
            totalBytesCopied: job.progress.totalBytesTotal || job.progress.totalBytesCopied,
            currentFilePercent: 100,
            overallPercent: 100,
          }
        : job.progress;
      updated = {
        ...job,
        files,
        state: "Completed",
        paused: false,
        decision: undefined,
        progress: completedProgress,
        resultPath: msg.resultPath ?? job.resultPath,
        resultPanelId: msg.resultPanelId ?? job.resultPanelId,
      };
      break;
    case "Failed":
      updated = {
        ...job,
        files,
        state: "Failed",
        paused: false,
        error: msg.error,
        decision: undefined,
        resultPath: msg.resultPath ?? job.resultPath,
        resultPanelId: msg.resultPanelId ?? job.resultPanelId,
      };
      break;
    case "Cancelled":
      updated = {
        ...job,
        files,
        state: "Cancelled",
        paused: false,
        decision: undefined,
        resultPath: msg.resultPath ?? job.resultPath,
        resultPanelId: msg.resultPanelId ?? job.resultPanelId,
      };
      break;
    default:
      return state;
  }

  let next = { ...state, jobs: { ...state.jobs, [msg.jobId]: updated } };
  // Reuse the cached job snapshot when this search is open, including the
  // final estimate and counters on terminal events.
  if (job.kind === "search" && state.pendingSearch?.jobId === msg.jobId) {
    const status = msg.event === "Completed" ? "completed"
      : msg.event === "Cancelled" ? "cancelled"
      : msg.event === "Failed" ? "failed"
      : state.pendingSearch.status;
    next = {
      ...next,
      pendingSearch: {
        ...state.pendingSearch,
        ...(sp ? {
          results: updated.searchResults ?? [],
          scanned: sp.scanned,
          matched: sp.matched,
          currentPath: sp.currentPath,
          estimatedPercent: sp.estimatedPercent,
        } : {}),
        status,
      },
    };
  }

  return next;
}

function requestNotification(state: AppState, requestId: string, input: NotificationInput): AppState {
  return { ...state, notificationRequests: [...state.notificationRequests, { requestId, input }] };
}

function reducer(state: AppState, action: AppAction): AppState {
  switch (action.type) {
    case "CONNECTED":
      return { ...state, connected: true, notificationConnectionReady: false };
    case "DISCONNECTED":
      return {
        ...state, connected: false, sessionId: null, notificationConnectionReady: false,
        sharedFolderInput: state.sharedFolderInput.requestId
          ? { ...state.sharedFolderInput, requestId: null, error: "Connection lost. Check shared folders after reconnecting, then retry if needed." }
          : state.sharedFolderInput,
      };
    case "BE_MESSAGE": {
      const msg = action.msg;
      switch (msg.type) {
        case "ViewerFileReady":
          // Request-local reply; the preview session owns this ephemeral target.
          return state;
        case "Welcome":
          return { ...state, sessionId: msg.sessionId, fileServerPort: msg.fileServerPort,
            slots: Object.fromEntries(Object.entries(state.slots).map(([id, slot]) => [id, { ...slot, version: -1 }])),
            notificationConnectionReady: true, notifications: { ...state.notifications, version: -1 } };
        case "PublishNotificationOk":
        case "PublishNotificationFailed":
          return { ...state,
            notificationRequests: state.notificationRequests.filter((request) => request.requestId !== msg.requestId),
            notificationPublishError: msg.type === "PublishNotificationFailed" ? msg.message : state.notificationPublishError,
          };
        case "ClearNotificationsOk":
        case "ClearNotificationsFailed":
          return state;
        case "StateSnapshot":
          return applySnapshot(state, msg);
        case "StatePatch":
          return applyPatch(state, msg);
        case "JobStarted":
          return applyJobStarted(state, msg);
        case "JobEvent":
          return applyJobEvent(state, msg);
        case "SyncBroResult": {
          const job = state.jobs[msg.jobId];
          if (!job) return state;
          return {
            ...state,
            jobs: { ...state.jobs, [msg.jobId]: { ...job, syncResult: msg.result } },
          };
        }
        case "HashGenerateBroResult": {
          const job = state.jobs[msg.jobId];
          if (!job) return state;
          return { ...state, jobs: { ...state.jobs, [msg.jobId]: { ...job, hashGenerateResult: msg.result } } };
        }
        case "HashGenerateBroFileResult": {
          const job = state.jobs[msg.jobId];
          if (!job?.hashGenerateResult) return state;
          const result = { ...job.hashGenerateResult, items: job.hashGenerateResult.items.map((item) => item.hashPath === msg.item.hashPath ? msg.item : item) };
          return { ...state, jobs: { ...state.jobs, [msg.jobId]: { ...job, hashGenerateResult: result } } };
        }
        case "HashCheckBroResult": {
          const job = state.jobs[msg.jobId];
          if (!job) return state;
          return { ...state, jobs: { ...state.jobs, [msg.jobId]: { ...job, hashCheckResult: msg.result } } };
        }
        case "HashCheckBroFileResult": {
          const job = state.jobs[msg.jobId];
          if (!job?.hashCheckResult) return state;
          const result = { ...job.hashCheckResult, items: job.hashCheckResult.items.map((item) => item.hashPath === msg.item.hashPath ? msg.item : item) };
          return { ...state, jobs: { ...state.jobs, [msg.jobId]: { ...job, hashCheckResult: result } } };
        }
        case "HashCheckReportSaved": {
          const job = state.jobs[msg.jobId];
          if (!job) return state;
          return {
            ...state,
            jobs: { ...state.jobs, [msg.jobId]: { ...job, reportSavedPath: msg.path, hashCheckReportError: undefined } },
          };
        }
        case "HashCheckReportFailed": {
          const job = state.jobs[msg.jobId];
          if (!job) return state;
          return {
            ...state,
            jobs: { ...state.jobs, [msg.jobId]: { ...job, hashCheckReportError: msg.message } },
          };
        }
        case "FedupBroResult":
        case "FedupBroResultPage": {
          const job = state.jobs[msg.jobId];
          if (!job) return state;
          return {
            ...state,
            jobs: { ...state.jobs, [msg.jobId]: { ...job, fedupResult: msg.result } },
          };
        }
        case "FedupReportSaved": {
          const job = state.jobs[msg.jobId];
          if (!job) return state;
          return {
            ...state,
            jobs: { ...state.jobs, [msg.jobId]: { ...job, reportSavedPath: msg.path } },
          };
        }
        case "SyncReportSaved": {
          const job = state.jobs[msg.jobId];
          if (!job) return state;
          return {
            ...state,
            jobs: { ...state.jobs, [msg.jobId]: { ...job, reportSavedPath: msg.path } },
          };
        }
        case "Error":
          if (
            (msg.code === "GIT_LOG_FAILED" || msg.code === "GIT_UNAVAILABLE")
            && state.pendingGitLog
            && state.pendingGitLog.status === "loading"
          ) {
            return {
              ...state,
              pendingGitLog: {
                ...state.pendingGitLog,
                status: "error",
                error: msg.message,
              },
            };
          }
          if (
            (msg.code === "GIT_COMMIT_SHOW_FAILED" || msg.code === "GIT_UNAVAILABLE")
            && state.pendingGitLog
          ) {
            const shows = state.pendingGitLog.commitShows;
            // Accordion: at most one expand in flight; mark that loading entry failed.
            const match = Object.entries(shows).find(([, entry]) => entry.status === "loading");
            if (match) {
              const [hash, entry] = match;
              return {
                ...state,
                pendingGitLog: {
                  ...state.pendingGitLog,
                  commitShows: {
                    ...shows,
                    [hash]: { ...entry, status: "error", error: msg.message },
                  },
                },
              };
            }
          }
          if (msg.code === "RENAME_FAILED" && state.pendingRename) {
            return { ...state, pendingRename: { ...state.pendingRename, error: msg.message } };
          }
          if (msg.code === "MKDIR_FAILED" && state.pendingMkdir) {
            return { ...state, pendingMkdir: { ...state.pendingMkdir, error: msg.message } };
          }
          if (msg.code === "TOUCH_FAILED" && state.pendingTouch) {
            return { ...state, pendingTouch: { ...state.pendingTouch, error: msg.message } };
          }
          if (msg.code === "SET_PERMISSIONS_FAILED" && state.pendingAttributes) {
            return { ...state, pendingAttributes: { ...state.pendingAttributes, error: msg.message } };
          }
          if (msg.code === "TOUCH_NOT_SUPPORTED") {
            return { ...state, pendingTouch: null };
          }
          if (msg.code === "NO_EDITOR" && msg.extra?.path) {
            const path = msg.extra.path as string;
            const filename = path.replace(/\\/g, "/").split("/").filter(Boolean).pop() || path;
            return { ...state, pendingNoEditor: { filename, fullPath: path } };
          }
          if (msg.code === "NAVIGATE_FAILED") {
            const panelId = msg.extra?.panelId as string | undefined;
            if (!panelId) return requestNotification(state, action.notificationRequestId, { message: msg.message, level: "error" });
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
            return next === state
              ? requestNotification(state, action.notificationRequestId, { message: msg.message, level: "error" })
              : next;
          }
          if (msg.code === "START_FILE_OP_FAILED") {
            return { ...state, pendingOp: null };
          }
          // Notifications for unconditional backend feedback arrive separately
          // through the authoritative notifications domain.
          return state;
        case "AddSharedFolderOk":
          return {
            ...state,
            sharedFolderInput: state.sharedFolderInput.requestId === msg.requestId
              ? { path: "", requestId: null } : state.sharedFolderInput,
          };
        case "AddSharedFolderFailed": {
          if (state.sharedFolderInput.requestId !== msg.requestId) return state;
          const next = {
            ...state,
            sharedFolderInput: { ...state.sharedFolderInput, requestId: null, error: msg.message },
          };
          return state.pendingSettings?.activeTab === "sharing" ? next
            : requestNotification(next, action.notificationRequestId, { message: msg.message, level: "error" });
        }
        case "RenameOk":
          return { ...state, pendingRename: null };
        case "MkdirOk":
          return { ...state, pendingMkdir: null };
        case "InitGitRepositoryOk":
          return {
            ...state,
            pendingGitInit: state.pendingGitInit?.requestId === msg.requestId
              ? null : state.pendingGitInit,
          };
        case "InitGitRepositoryFailed":
          if (state.pendingGitInit?.requestId === msg.requestId) {
            return { ...state, pendingGitInit: { ...state.pendingGitInit, submitting: false, error: msg.message } };
          }
          return requestNotification(state, action.notificationRequestId, { message: msg.message, level: "error" });
        case "TouchFileOk":
          return { ...state, pendingTouch: null };
        case "SetPermissionsOk":
          return { ...state, pendingAttributes: null };
        case "NavigateOk":
          return {
            ...state,
            pendingGoToPath: state.pendingGoToPath?.panelId === msg.panelId ? null : state.pendingGoToPath,
            pendingPanelHistory: state.pendingPanelHistory?.panelId === msg.panelId ? null : state.pendingPanelHistory,
          };
        case "EjectSourceOk":
          return {
            ...state,
            pendingEject: state.pendingEject?.requestId === msg.requestId ? null : state.pendingEject,
          };
        case "EjectSourceFailed":
          if (state.pendingEject?.requestId !== msg.requestId) {
            return requestNotification(state, action.notificationRequestId, { message: msg.message, level: "error" });
          }
          return {
            ...state,
            pendingEject: { ...state.pendingEject, requestId: null, error: msg.message },
          };
        case "ExternalClipboardReady": {
          const m = msg as ExternalClipboardReadyMessage;
          return {
            ...state,
            pendingOp: {
              opType: m.opType,
              files: m.files,
              sourcePanel: m.panelId,
              destPanel: m.panelId,
              destPath: m.destPath,
              conflictPolicy: "ask",
              ...(m.sourceId ? { sourceId: m.sourceId } : {}),
            },
            pendingContextMenu: null,
          };
        }
        case "ExternalClipboardWritten":
        case "FileClipboardWritten":
        case "TextClipboardWritten":
          return state;
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
        case "FedupDeleteDryRunReady": {
          const m = msg as FedupDeleteDryRunReadyMessage;
          const job = state.jobs[m.jobId];
          const scriptName = m.scriptPath.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? m.scriptPath;
          const manifestName = m.manifestPath.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? m.manifestPath;
          return {
            ...state,
            pendingDryRun: {
              scriptPath: m.scriptPath,
              scriptDir: m.scriptDir,
              scriptName,
              manifestPath: m.manifestPath,
              manifestName,
              sourcePanel: job?.sourcePanel ?? "",
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
          const normalizedBros = (msg as BroOptionsReadyMessage).bros.map((bro) => ({
            ...bro,
            pre: bro.pre ?? { score: 0, reason: "Available for this selection" },
            params: bro.params ?? [],
          }));
          return {
            ...state,
            pendingBro: {
              ...state.pendingBro,
              status: "ready",
              inputs: (msg as BroOptionsReadyMessage).inputs,
              bros: normalizedBros,
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
        case "BroCatalogReady": {
          const ready = msg as BroCatalogReadyMessage;
          if (state.broCatalog.requestId !== ready.requestId) return state;
          return {
            ...state,
            broCatalog: {
              requestId: ready.requestId,
              status: "ready",
              bros: ready.bros,
              error: undefined,
            },
          };
        }
        case "BroCatalogFailed": {
          const failed = msg as BroCatalogFailedMessage;
          if (state.broCatalog.requestId !== failed.requestId) return state;
          return {
            ...state,
            broCatalog: {
              requestId: failed.requestId,
              status: "error",
              bros: [],
              error: failed.message,
            },
          };
        }
        case "GitLogReady": {
          const ready = msg as GitLogReadyMessage;
          if (!state.pendingGitLog || state.pendingGitLog.requestId !== ready.requestId) {
            return state;
          }
          // Append when loading more (we already had commits); replace on fresh fetch.
          const wasLoadMore = state.pendingGitLog.commits.length > 0 && state.pendingGitLog.status === "loading";
          return {
            ...state,
            pendingGitLog: {
              ...state.pendingGitLog,
              status: "ready",
              commits: wasLoadMore
                ? [...state.pendingGitLog.commits, ...ready.commits]
                : ready.commits,
              hasMore: ready.hasMore,
              error: undefined,
              // Fresh (non-append) log invalidates expand cache.
              expandedHash: wasLoadMore ? state.pendingGitLog.expandedHash : null,
              commitShows: wasLoadMore ? state.pendingGitLog.commitShows : {},
            },
          };
        }
        case "GitCommitShowReady": {
          const ready = msg as GitCommitShowReadyMessage;
          if (!state.pendingGitLog || state.pendingGitLog.panelId !== ready.panelId) {
            return state;
          }
          const existing = state.pendingGitLog.commitShows[ready.hash];
          if (existing && existing.requestId !== ready.requestId) {
            return state;
          }
          return {
            ...state,
            pendingGitLog: {
              ...state.pendingGitLog,
              commitShows: {
                ...state.pendingGitLog.commitShows,
                [ready.hash]: {
                  status: "ready",
                  requestId: ready.requestId,
                  message: ready.message,
                  changes: ready.changes,
                  error: undefined,
                },
              },
            },
          };
        }
        case "GitActionFinished":
          return { ...state, gitActionResult: msg as GitActionFinishedMessage };
        case "InputRequired": {
          const m = msg as import("../ws/types").InputRequiredMessage;
          return {
            ...state,
            pendingInput: {
              panelId: m.panelId,
              requestId: m.requestId,
              kind: m.kind,
              prompt: m.prompt,
            },
          };
        }
        case "QuitConfirmRequired":
          return applyQuitConfirmRequired(state, msg);
        case "ClearIconCacheOk":
        case "ClearIconCacheFailed":
        case "OpenOsNotificationSettingsOk":
        case "OpenOsNotificationSettingsFailed":
          return state; // Settings consumes the correlated operation reply.
        case "CacheStatusReady":
          return { ...state, cacheStatus: { prefetchCache: (msg as CacheStatusReadyMessage).prefetchCache } };
        case "ThumbnailQueueReady":
          return { ...state, thumbnailQueue: msg as ThumbnailQueueReadyMessage };
        case "TerminalClosed": {
          // Belt-and-suspenders: BE also broadcasts a StatePatch clearing terminalSessionId,
          // but handle it here too in case the patch arrives out of order or is missed.
          const closedMsg = msg as TerminalClosedMessage;
          disposeCachedTerminal(closedMsg.sessionId);
          const panelId = closedMsg.panelId;
          if (panelId && state.maximizedTerminalPanelId === panelId) {
            storeMaximizedTerminalPanelId(null);
          }
          if (!panelId || !state.panels[panelId]) return state;
          const panel = state.panels[panelId];
          if (panel.terminalSessionId === undefined && state.maximizedTerminalPanelId !== panelId) return state;
          const next: AppState = panel.terminalSessionId !== undefined
            ? { ...state, panels: { ...state.panels, [panelId]: { ...panel, terminalSessionId: undefined } } }
            : state;
          return state.maximizedTerminalPanelId === panelId
            ? { ...next, maximizedTerminalPanelId: null }
            : next;
        }
        case "PairingRequested":
          return {
            ...state,
            pendingSettings: { activeTab: "sharing" },
            pendingPairing: {
              handshakeId: msg.handshakeId,
              code: msg.code,
              name: msg.name,
              os: msg.os,
              pubkeyAlgo: msg.pubkeyAlgo,
              peerFingerprint: msg.peerFingerprint,
              expiresAt: msg.expiresAt,
              localDecision: null,
            },
          };
        case "PairingResolved":
          // Only close the modal if it's the same handshake — a stale
          // resolution for an earlier handshake mustn't dismiss a fresh one.
          if (state.pendingPairing?.handshakeId !== msg.handshakeId) return state;
          return { ...state, pendingPairing: null };
        case "RemoteSessionStarted":
        case "RemoteSessionEnded":
          // No store mutation today — surface in a future "active connections"
          // indicator. Drop intentionally to keep the reducer total.
          return state;
        case "RemoteSourceStatus": {
          // Drop the entry when the source is healthy — that way the
          // FE's "is it reconnecting?" check is a single map lookup
          // returning undefined for the common case.
          if (msg.status === "connected") {
            if (!state.remoteSourceStatus[msg.sourceId]) return state;
            const next = { ...state.remoteSourceStatus };
            delete next[msg.sourceId];
            return { ...state, remoteSourceStatus: next };
          }
          return {
            ...state,
            remoteSourceStatus: {
              ...state.remoteSourceStatus,
              [msg.sourceId]: {
                status: msg.status,
                message: msg.message,
                nextRetryAt: msg.nextRetryAt,
              },
            },
          };
        }
        default:
          return state;
      }
    }
    case "OPEN_OPERATION":
      return { ...state, pendingOp: action.op, pendingContextMenu: null };
    case "CLOSE_OPERATION":
      return { ...state, pendingOp: null, activeJobId: null };
    case "SET_ACTIVE_JOB": {
      const activatedJob = state.jobs[action.jobId];
      // Search jobs open the SearchModal instead of the OperationModal
      if (activatedJob?.kind === "search") {
        return {
          ...state,
          pendingSearch: {
            sourcePanelPath: activatedJob.sourcePath,
            jobId: action.jobId,
            searchSourceId: activatedJob.searchSourceId ?? null,
            searchParams: activatedJob.searchParams ?? null,
            results: activatedJob.searchResults ?? [],
            scanned: activatedJob.searchScanned ?? 0,
            matched: activatedJob.searchMatched ?? 0,
            currentPath: activatedJob.searchCurrentPath ?? "",
            estimatedPercent: activatedJob.searchEstimatedPercent,
            status: activatedJob.state === "Running" ? "running"
              : activatedJob.state === "Completed" ? "completed"
              : activatedJob.state === "Cancelled" ? "cancelled"
              : activatedJob.state === "Failed" ? "failed"
              : "idle",
            activeTab: "results",
            requestId: null,
          },
        };
      }
      return { ...state, activeJobId: action.jobId, pendingOp: null };
    }
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
    case "OPEN_GIT_INIT":
      return { ...state, pendingGitInit: action.gitInit };
    case "SUBMIT_GIT_INIT":
      return state.pendingGitInit
        ? { ...state, pendingGitInit: { ...state.pendingGitInit, requestId: action.requestId, submitting: true, error: undefined } }
        : state;
    case "CLOSE_GIT_INIT":
      return { ...state, pendingGitInit: null };
    case "OPEN_TOUCH":
      return { ...state, pendingTouch: action.touch };
    case "CLOSE_TOUCH":
      return { ...state, pendingTouch: null };
    case "OPEN_SELECTION_MASK":
      return { ...state, pendingSelectionMask: action.selectionMask };
    case "CLOSE_SELECTION_MASK":
      return { ...state, pendingSelectionMask: null };
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
    case "OPEN_ATTRIBUTES":
      return { ...state, pendingAttributes: action.attributes, pendingContextMenu: null };
    case "CLOSE_ATTRIBUTES":
      return { ...state, pendingAttributes: null };
    case "OPEN_SETTINGS":
      return { ...state, pendingSettings: { activeTab: action.tab }, pendingContextMenu: null };
    case "CLOSE_SETTINGS":
      return { ...state, pendingSettings: null };
    case "SET_SHARED_FOLDER_INPUT":
      if (state.sharedFolderInput.requestId) return state;
      return { ...state, sharedFolderInput: { path: action.path, requestId: null } };
    case "SUBMIT_SHARED_FOLDER":
      if (state.sharedFolderInput.requestId) return state;
      return { ...state, sharedFolderInput: { ...state.sharedFolderInput, requestId: action.requestId, error: undefined } };
    case "SET_PAIRING_DECISION":
      if (state.pendingPairing?.handshakeId !== action.handshakeId) return state;
      return {
        ...state,
        pendingPairing: { ...state.pendingPairing, localDecision: action.accept },
      };
    case "SET_MAXIMIZED_TERMINAL":
      storeMaximizedTerminalPanelId(action.panelId);
      return { ...state, maximizedTerminalPanelId: action.panelId };
    case "OPEN_CONTEXT_MENU":
      return { ...state, pendingContextMenu: action.menu };
    case "CLOSE_CONTEXT_MENU":
      return { ...state, pendingContextMenu: null };
    case "OPEN_SOURCE_MENU":
      return { ...state, pendingSourceMenu: action.menu, pendingContextMenu: null };
    case "CLOSE_SOURCE_MENU":
      return { ...state, pendingSourceMenu: null };
    case "OPEN_EJECT":
      return { ...state, pendingEject: action.eject, pendingSourceMenu: null, commandPaletteOpen: false };
    case "CLOSE_EJECT":
      return { ...state, pendingEject: null };
    case "SUBMIT_EJECT":
      if (!state.pendingEject) return state;
      return { ...state, pendingEject: { ...state.pendingEject, sourceId: action.sourceId, action: action.action, requestId: action.requestId, error: null } };
    case "SET_EJECT_SELECTION":
      if (!state.pendingEject || state.pendingEject.requestId) return state;
      return { ...state, pendingEject: { ...state.pendingEject, sourceId: action.sourceId, action: action.action, error: null } };
    case "OPEN_GLOBAL_SELECTION_ADD":
      return { ...state, pendingContextMenu: null, pendingGlobalSelectionAdd: action.add };
    case "CLOSE_GLOBAL_SELECTION_ADD":
      return { ...state, pendingGlobalSelectionAdd: null };
    case "OPEN_OPEN_WITH":
      return { ...state, pendingContextMenu: null, pendingOpenWith: action.openWith };
    case "CLOSE_OPEN_WITH":
      return { ...state, pendingOpenWith: null };
    case "OPEN_BRO":
      return { ...state, pendingContextMenu: null, pendingBro: action.bro };
    case "CLOSE_BRO":
      return { ...state, pendingBro: null };
    case "REQUEST_BRO_CATALOG":
      return {
        ...state,
        broCatalog: {
          requestId: action.requestId,
          status: "loading",
          bros: [],
          error: undefined,
        },
      };
    case "REQUEST_GIT_LOG": {
      const samePanel = state.pendingGitLog?.panelId === action.panelId;
      const keep = action.append && samePanel ? state.pendingGitLog!.commits : [];
      return {
        ...state,
        pendingGitLog: {
          panelId: action.panelId,
          requestId: action.requestId,
          status: "loading",
          commits: keep,
          hasMore: action.append ? (state.pendingGitLog?.hasMore ?? false) : false,
          error: undefined,
          expandedHash: action.append && samePanel ? state.pendingGitLog!.expandedHash : null,
          commitShows: action.append && samePanel ? state.pendingGitLog!.commitShows : {},
        },
      };
    }
    case "REQUEST_GIT_COMMIT_SHOW": {
      if (!state.pendingGitLog || state.pendingGitLog.panelId !== action.panelId) {
        return state;
      }
      const prev = state.pendingGitLog.commitShows[action.hash];
      // Keep ready cache; only (re)fetch when missing or previously errored.
      if (prev?.status === "ready" || prev?.status === "loading") {
        return {
          ...state,
          pendingGitLog: {
            ...state.pendingGitLog,
            expandedHash: action.hash,
          },
        };
      }
      return {
        ...state,
        pendingGitLog: {
          ...state.pendingGitLog,
          expandedHash: action.hash,
          commitShows: {
            ...state.pendingGitLog.commitShows,
            [action.hash]: {
              status: "loading",
              requestId: action.requestId,
              message: "",
              changes: [],
              error: undefined,
            },
          },
        },
      };
    }
    case "SET_GIT_LOG_EXPANDED": {
      if (!state.pendingGitLog) return state;
      return {
        ...state,
        pendingGitLog: {
          ...state.pendingGitLog,
          expandedHash: action.hash,
        },
      };
    }
    case "CLEAR_GIT_LOG":
      return { ...state, pendingGitLog: null };
    case "OPEN_COMMAND_PALETTE":
      return { ...state, commandPaletteOpen: true };
    case "CLOSE_COMMAND_PALETTE":
      return { ...state, commandPaletteOpen: false };
    case "OPEN_ABOUT":
      return { ...state, aboutOpen: true, commandPaletteOpen: false };
    case "CLOSE_ABOUT":
      return { ...state, aboutOpen: false };
    case "OPEN_SORT_SELECTOR":
      return { ...state, activeSortSelectorPanelId: action.panelId };
    case "CLOSE_SORT_SELECTOR":
      return { ...state, activeSortSelectorPanelId: null };
    case "REQUEST_NOTIFICATION":
      return { ...state, notificationRequests: [...state.notificationRequests, action.request], notificationPublishError: null };
    case "EXPIRE_NOTIFICATION_TOAST":
      return { ...state, notifications: expireNotificationToast(state.notifications, action.id) };
    case "CLOSE_INPUT":
      return { ...state, pendingInput: null };
    case "TOGGLE_EMBEDDED_VIEWER": {
      if (state.embeddedViewer) return { ...state, embeddedViewer: null };
      const focusedSlotId = state.panels[state.focus]?.slotId;
      if (!focusedSlotId) return state;
      return { ...state, embeddedViewer: { sourceSlotId: focusedSlotId } };
    }
    case "NAV_HISTORY_BACK": {
      const h = state.panelHistory[action.panelId];
      if (!h || h.cursor <= 0) return state;
      return { ...state, panelHistory: { ...state.panelHistory, [action.panelId]: { ...h, cursor: h.cursor - 1 } } };
    }
    case "NAV_HISTORY_FORWARD": {
      const h = state.panelHistory[action.panelId];
      if (!h || h.cursor >= h.entries.length - 1) return state;
      return { ...state, panelHistory: { ...state.panelHistory, [action.panelId]: { ...h, cursor: h.cursor + 1 } } };
    }
    case "NAV_HISTORY_JUMP": {
      const h = state.panelHistory[action.panelId];
      if (!h) return state;
      const idx = Math.max(0, Math.min(action.entryIndex, h.entries.length - 1));
      return { ...state, panelHistory: { ...state.panelHistory, [action.panelId]: { ...h, cursor: idx } } };
    }
    case "OPEN_BOOKMARKS":
      return { ...state, bookmarksOpen: { panelId: action.panelId }, pendingContextMenu: null };
    case "CLOSE_BOOKMARKS":
      return { ...state, bookmarksOpen: null };
    case "OPEN_SEARCH":
      return { ...state, pendingSearch: action.search };
    case "CLOSE_SEARCH":
      return { ...state, pendingSearch: null };
    case "SET_SEARCH_TAB":
      if (!state.pendingSearch) return state;
      return { ...state, pendingSearch: { ...state.pendingSearch, activeTab: action.tab } };
    case "SET_SEARCH_REQUEST_ID":
      if (!state.pendingSearch) return state;
      return { ...state, pendingSearch: { ...state.pendingSearch, requestId: action.requestId } };
    case "RESET_SEARCH_FOR_NEW_JOB":
      if (!state.pendingSearch) return state;
      return {
        ...state,
        pendingSearch: {
          ...state.pendingSearch,
          jobId: null,
          searchSourceId: null,
          searchParams: null,
          results: [],
          scanned: 0,
          matched: 0,
          currentPath: "",
          estimatedPercent: undefined,
          status: "idle",
          requestId: action.requestId,
        },
      };
    default:
      return state;
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
