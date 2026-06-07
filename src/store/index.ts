import { createContext, useContext, useReducer, type Dispatch } from "react";
import { isSearchSourceId } from "../utils/sourceId";
import { disposeCachedTerminal } from "../components/PanelTerminal";
import type {
  PanelState,
  PanelSlot,
  SearchParams,
  SearchResult,
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
  ExternalClipboardReadyMessage,
  ExternalClipboardWrittenMessage,
  CacheStatusReadyMessage,
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

export interface PendingInput {
  panelId: string;
  requestId: string;
  kind: "password" | "text";
  prompt: string;
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
  status: "idle" | "running" | "completed" | "cancelled" | "failed";
  activeTab: "input" | "results" | "extra";
  // Used to correlate the StartSearch intent with the JobStarted message
  requestId: string | null;
  // The params used for this search — used to pre-fill the Input tab when
  // re-opening a job from the jobs bar.
  searchParams: SearchParams | null;
}

export type SettingsTab = "general" | "display" | "sharing";

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
  dirSizeQueue: DirSizeTask[];
  commands: Record<string, InteractiveCommand>;
  pendingOp: PendingOp | null;
  activeJobId: string | null;
  pendingQuit: { activeJobs: number } | null;
  pendingRename: PendingRename | null;
  pendingInfo: PendingInfo | null;
  pendingMkdir: PendingMkdir | null;
  pendingTouch: PendingTouch | null;
  pendingNoEditor: PendingNoEditor | null;
  pendingGoToPath: PendingGoToPath | null;
  pendingPanelHistory: PendingPanelHistory | null;
  pendingDryRun: PendingDryRun | null;
  pendingContextMenu: PendingContextMenu | null;
  pendingOpenWith: PendingOpenWith | null;
  pendingBro: PendingBro | null;
  pendingInput: PendingInput | null;
  pendingSearch: PendingSearch | null;
  pendingAttributes: PendingAttributes | null;
  pendingSettings: PendingSettings | null;
  commandPaletteOpen: boolean;
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
  transientHint: { message: string; token: number } | null;
  embeddedViewer: { sourceSlotId: string } | null;
  cacheStatus: { prefetchCache: PrefetchCacheStatus } | null;
  bookmarks: BookmarkEntry[];
  bookmarksOpen: { panelId: string } | null;
  maximizedTerminalPanelId: string | null;
  childProcesses: Record<number, ChildProcess>;
  fontSize: number;
  settings: SettingsState;
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
  dirSizeQueue: [],
  commands: {},
  pendingOp: null,
  activeJobId: null,
  pendingQuit: null,
  pendingRename: null,
  pendingInfo: null,
  pendingMkdir: null,
  pendingTouch: null,
  pendingNoEditor: null,
  pendingGoToPath: null,
  pendingPanelHistory: null,
  pendingDryRun: null,
  pendingContextMenu: null,
  pendingOpenWith: null,
  pendingBro: null,
  pendingInput: null,
  pendingSearch: null,
  pendingAttributes: null,
  pendingSettings: null,
  commandPaletteOpen: false,
  discoveredPeers: [],
  shareConfig: { enabled: false, allowIpv6: false, sharedSourceIds: [], sharedRoots: [], shareSuggestions: [], pairedPeers: [] },
  pendingPairing: null,
  remoteSourceStatus: {},

  transientHint: null,
  embeddedViewer: null,
  cacheStatus: null,
  bookmarks: [],
  bookmarksOpen: null,
  maximizedTerminalPanelId: null,
  childProcesses: {},
  fontSize: 13,
  settings: {
    general: { showHiddenDefault: false, rememberWindowState: true, preserveAttributes: true, autoResumeCopies: true, ludicrousMode: false, maxImageSizeMb: 512, savePanelViewMode: false, enableVideoThumbnails: true, thumbnailMaxImageSizeMb: 100, thumbnailVideoFrameSkip: 5, streamThumbnails: true, sessionPath: "", bookmarksPath: "", thumbnailCacheDir: "", thumbnailCacheItemCount: 0, thumbnailCacheSizeBytes: 0, persistPanelTabs: true, procTreeMode: false, colorScheme: "fbro-dark" },
  },
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
  | { type: "OPEN_TOUCH"; touch: PendingTouch }
  | { type: "CLOSE_TOUCH" }
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
  | { type: "OPEN_SEARCH"; search: PendingSearch }
  | { type: "CLOSE_SEARCH" }
  | { type: "SET_SEARCH_TAB"; tab: "input" | "results" | "extra" }
  | { type: "SET_SEARCH_REQUEST_ID"; requestId: string }
  | { type: "RESET_SEARCH_FOR_NEW_JOB"; requestId: string }
  | { type: "OPEN_COMMAND_PALETTE" }
  | { type: "CLOSE_COMMAND_PALETTE" }
  | { type: "CLOSE_TRANSIENT_HINT" }
  | { type: "SHOW_TRANSIENT_HINT"; message: string }
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
    next.slots = { ...domains.slots };
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
      panels[id] = { ...p, panelId: id, entries: p.entries ?? [], selection: p.selection ?? [], selectedEntries: p.selectedEntries ?? [], version: p.version ?? 0, calculatingSize: p.calculatingSize ?? [], totalCount: p.totalCount, totalSize: p.totalSize, windowStart: p.windowStart, filterModeActive: p.filterModeActive ?? false, showHidden: p.showHidden ?? false, viewMode: p.viewMode ?? "list", supportsThumbnails: p.supportsThumbnails ?? false };
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
    next.settings = domains.settings;
  }
  if (domains.discoveredPeers !== undefined) {
    next.discoveredPeers = domains.discoveredPeers;
  }
  if (domains.shareConfig !== undefined) {
    next.shareConfig = domains.shareConfig;
  }
  return next;
}

function applyPatch(state: AppState, msg: StatePatchMessage): AppState {
  if (msg.domain === "panels" && msg.entityId) {
    const panelId = msg.entityId;
    const panel = state.panels[panelId];
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
        calculatingSize: [],
        shadowSource: p.shadowSource,
        terminalSessionId: p.terminalSessionId,
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
    if (msg.patch.selection !== undefined) {
      preGuardPanel = { ...preGuardPanel, selection: msg.patch.selection as number[] };
    }
    if (msg.patch.selectedEntries !== undefined) {
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

    const locationChanged = updated.path !== panel.path
      || (msg.patch.sourceId !== undefined && updated.sourceId !== panel.sourceId);

    if (locationChanged) {
      // Bookmarks, history, and procs are navigation/virtual tools, not real filesystem
      // locations — entering or exiting them must not create a history entry.
      if (updated.sourceId === "bookmarks" || panel.sourceId === "bookmarks"
       || updated.sourceId === "history"  || panel.sourceId === "history"
       || updated.sourceId === "procs"    || panel.sourceId === "procs") {
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
      if (patch.remove) {
        nextSlots = { ...nextSlots };
        delete nextSlots[slotId];
      } else {
        const existing = nextSlots[slotId];
        const merged: PanelSlot = {
          slotId,
          tabPanelIds: patch.tabPanelIds ?? existing?.tabPanelIds ?? [],
          activePanelId:
            patch.activePanelId ?? existing?.activePanelId ?? slotId,
        };
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
    let next = fontSize !== undefined ? { ...state, fontSize } : state;
    if (general !== undefined) {
      next = { ...next, settings: { ...next.settings, general: { ...next.settings.general, ...general } } };
    }
    return next;
  }

  if (msg.domain === "discoveredPeers") {
    const peers = (msg.patch.peers as DiscoveredPeer[]) ?? [];
    return { ...state, discoveredPeers: peers };
  }

  if (msg.domain === "shareConfig") {
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
  const job = state.jobs[msg.jobId];
  if (!job) return state;
  const files = msg.files ?? job.files;

  let updated: Job;

  switch (msg.event) {
    case "Queued":
      updated = { ...job, files, state: "Queued", paused: false, decision: undefined };
      break;
    case "Progress":
      if (job.kind === "search" && msg.searchProgress) {
        // Accumulate search results
        const sp = msg.searchProgress;
        updated = {
          ...job,
          files,
          state: "Running",
          paused: false,
          searchResults: [...(job.searchResults ?? []), ...sp.newResults],
          searchScanned: sp.scanned,
          searchMatched: sp.matched,
          searchCurrentPath: sp.currentPath,
        };
      } else {
        updated = { ...job, files, state: "Running", paused: false, progress: msg.progress ?? job.progress, decision: undefined };
      }
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
              : job.kind === "fp-bro" || job.kind === "sync-bro"
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

  // Sync search progress/status into pendingSearch if it belongs to this search job
  if (job.kind === "search" && state.pendingSearch?.jobId === msg.jobId) {
    if (msg.event === "Progress" && msg.searchProgress) {
      const sp = msg.searchProgress;
      next = {
        ...next,
        pendingSearch: {
          ...state.pendingSearch,
          results: [...state.pendingSearch.results, ...sp.newResults],
          scanned: sp.scanned,
          matched: sp.matched,
          currentPath: sp.currentPath,
        },
      };
    } else if (msg.event === "Completed") {
      next = {
        ...next,
        pendingSearch: { ...state.pendingSearch, status: "completed" },
      };
    } else if (msg.event === "Cancelled") {
      next = {
        ...next,
        pendingSearch: { ...state.pendingSearch, status: "cancelled" },
      };
    } else if (msg.event === "Failed") {
      next = {
        ...next,
        pendingSearch: { ...state.pendingSearch, status: "failed" },
      };
    }
  }

  return next;
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
        case "SyncBroResult": {
          const job = state.jobs[msg.jobId];
          if (!job) return state;
          return {
            ...state,
            jobs: { ...state.jobs, [msg.jobId]: { ...job, syncResult: msg.result } },
          };
        }
        case "SyncReportSaved": {
          const job = state.jobs[msg.jobId];
          if (!job) return state;
          return {
            ...state,
            jobs: { ...state.jobs, [msg.jobId]: { ...job, reportSavedPath: msg.path } },
            transientHint: { message: `Saved sync report to ${msg.path}`, token: msg.timestamp },
          };
        }
        case "Error":
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
            return { ...state, pendingTouch: null, transientHint: { message: msg.message, token: msg.timestamp } };
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
          if (msg.code.startsWith("PASTE_EXTERNAL_")) {
            return {
              ...state,
              transientHint: {
                message: msg.message,
                token: msg.timestamp,
              },
            };
          }
          if (msg.code.startsWith("WRITE_EXTERNAL_")) {
            return {
              ...state,
              transientHint: {
                message: msg.message,
                token: msg.timestamp,
              },
            };
          }
          if (msg.code === "START_FILE_OP_FAILED") {
            return {
              ...state,
              pendingOp: null,
              transientHint: {
                message: msg.message,
                token: msg.timestamp,
              },
            };
          }
          if (msg.code === "PANEL_RECOVERED_HOME") {
            return {
              ...state,
              transientHint: {
                message: msg.message,
                token: msg.timestamp,
              },
            };
          }
          if (msg.code === "BRO_LAUNCH_FAILED") {
            return {
              ...state,
              transientHint: {
                message: msg.message,
                token: msg.timestamp,
              },
            };
          }
          // Surface remote / sharing errors as toasts. Without this they
          // disappear silently and the user can't tell why Connect or
          // Pair didn't do anything.
          if (msg.code.startsWith("remote.") || msg.code.startsWith("sharing.")) {
            return {
              ...state,
              transientHint: {
                message: msg.message,
                token: msg.timestamp,
              },
            };
          }
          console.error("BE error:", msg.code, msg.message);
          return state;
        case "RenameOk":
          return { ...state, pendingRename: null };
        case "MkdirOk":
          return { ...state, pendingMkdir: null };
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
            },
            pendingContextMenu: null,
            transientHint: null,
          };
        }
        case "ExternalClipboardWritten": {
          const m = msg as ExternalClipboardWrittenMessage;
          const verb = m.opType === "move" ? "Cut" : "Copied";
          const noun = m.itemCount === 1 ? "item" : "items";
          return {
            ...state,
            transientHint: {
              message: `${verb} ${m.itemCount} ${noun} to the system clipboard.`,
              token: m.timestamp,
            },
          };
        }
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
        case "CacheStatusReady":
          return { ...state, cacheStatus: { prefetchCache: (msg as CacheStatusReadyMessage).prefetchCache } };
        case "TerminalClosed": {
          // Belt-and-suspenders: BE also broadcasts a StatePatch clearing terminalSessionId,
          // but handle it here too in case the patch arrives out of order or is missed.
          const closedMsg = msg as TerminalClosedMessage;
          disposeCachedTerminal(closedMsg.sessionId);
          const panelId = closedMsg.panelId;
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
            pendingPairing: {
              handshakeId: msg.handshakeId,
              code: msg.code,
              name: msg.name,
              os: msg.os,
              pubkeyAlgo: msg.pubkeyAlgo,
              expiresAt: msg.expiresAt,
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
    case "OPEN_TOUCH":
      return { ...state, pendingTouch: action.touch };
    case "CLOSE_TOUCH":
      return { ...state, pendingTouch: null };
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
    case "CLOSE_TRANSIENT_HINT":
      return { ...state, transientHint: null };
    case "SHOW_TRANSIENT_HINT":
      return {
        ...state,
        transientHint: {
          message: action.message,
          token: Date.now(),
        },
      };
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
