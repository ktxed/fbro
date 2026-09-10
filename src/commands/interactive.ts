import type { Dispatch } from "react";
import { invoke } from "@tauri-apps/api/core";

import { isMac, isWindows } from "../utils/platform";
import type { AppAction, AppState } from "../store";
import { markHistoryNavPending } from "../store";
import { isGlobalSelectionSourceId, isSearchSourceId } from "../utils/sourceId";
import { contextualEntryIsSelected } from "../contextualOperations";
import type { WsClient } from "../ws/client";
import type {
  FileOpFile,
  FileOpType,
  InteractiveCommand,
  KeyBinding,
  PanelState,
  Source,
  VFSEntry,
} from "../ws/types";

export const TOOLBAR_COMMAND_IDS = [
  "file.rename",
  "file.view",
  "file.edit",
  "file.copy",
  "file.move",
  "file.mkdir",
  "file.delete",
  "app.open_terminal",
  "app.quit",
] as const;

export const COMMAND_PALETTE_COMMAND_ID = "app.command_palette" as const;

export const GLOBAL_SHORTCUT_COMMAND_IDS = [
  COMMAND_PALETTE_COMMAND_ID,
  "app.open_jobs",
  "app.search",
  "app.settings",
  "app.install_fbro_command",
  "panel.viewer.embed",
  "panel.navigate_source_root",
  "file.info",
  "file.compare",
  "file.set_permissions",
  "panel.toggle_hidden",
  "panel.toggle_view_mode",
  "file.copy_external",
  "file.cut_external",
  "file.paste_external",
  "file.paste_external_move",
  ...TOOLBAR_COMMAND_IDS,
] as const;

export const PANEL_SHORTCUT_COMMAND_IDS = [
  "panel.cursor.up",
  "panel.cursor.down",
  "panel.cursor.left",
  "panel.cursor.right",
  "panel.cursor.first",
  "panel.cursor.last",
  "panel.selection.extend_up",
  "panel.selection.extend_down",
  "panel.selection.extend_first",
  "panel.selection.extend_last",
  "panel.selection.extend_left",
  "panel.selection.extend_right",
  "panel.selection.toggle_or_size",
  "panel.selection.calculate_sizes",
  "panel.selection.select_all",
  "panel.selection.clear",
  "panel.selection.invert",
  "panel.cursor.page_up",
  "panel.cursor.page_down",
  "panel.open",
  "panel.open_entry",
  "panel.navigate_up",
  "panel.navigate_home",
  "panel.navigate_to_path",
  "panel.refresh",
  "panel.history.open",
  "panel.history.back",
  "panel.history.forward",
  "panel.pass_to_bro",
  "app.open_terminal",
  "panel.focus.switch",
  "panel.source.next",
  "panel.source.previous",
  "panel.filter.open",
  "panel.path.sync",
  "panel.context_menu",
  "panel.toggle_hidden",
  "panel.toggle_view_mode",
  "panel.sort.open",
  "panel.git.actions",
  "panel.bookmarks.open",
  "panel.bookmarks.browse",
  "panel.toggle_embedded_terminal",
  "panel.tab.new",
  "panel.tab.close",
  "panel.tab.next",
  "panel.tab.prev",
  "file.touch",
  "file.quick_edit",
  "file.compare",
] as const;

export const COMMAND_PALETTE_COMMAND_IDS = [
  ...new Set([
    ...GLOBAL_SHORTCUT_COMMAND_IDS,
    ...PANEL_SHORTCUT_COMMAND_IDS,
  ]),
].filter((commandId) => commandId !== COMMAND_PALETTE_COMMAND_ID);

const KEY_LABELS: Record<string, string> = {
  " ": "Space",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
};

type KeyboardLikeEvent = Pick<KeyboardEvent, "key" | "shiftKey" | "ctrlKey" | "altKey" | "metaKey" | "code">;

interface ExecuteCommandContext {
  state: AppState;
  dispatch: Dispatch<AppAction>;
  client: WsClient;
  panelId?: string;
  panel?: PanelState;
  sources?: Record<string, Source>;
  otherPanelId?: string;
  onOpenJobsView?: () => void;
  onOpenCommandPalette?: () => void;
  onOpenFilter?: (initialValue: string) => void;
  onOpenContextMenu?: () => void;
  getPanelPageSize?: () => number;
  /** Current grid column count (cards per row). Used in grid mode for
   *  Up/Down/PageUp/PageDown deltas. */
  getGridCols?: () => number;
  /** Visible card rows in the grid viewport. Used for PageUp/PageDown. */
  getGridPageRows?: () => number;
}

export interface ContextActionTarget {
  panelId: string;
  dirPath: string;
  fullPath: string;
  name: string;
  kind: string;
  size?: number;
  scope?: "entry" | "selection";
  entries?: FileOpFile[];
}

function normalizeKey(key: string): string {
  return key.length === 1 ? key.toLowerCase() : key;
}

function keyLabel(key: string): string {
  if (KEY_LABELS[key]) return KEY_LABELS[key];
  if (key.length === 1) return key === " " ? "Space" : key.toUpperCase();
  return key;
}

function entryAt(panel: Pick<PanelState, "entries" | "windowStart">, absIndex: number) {
  const localIdx = absIndex - (panel.windowStart ?? 0);
  return panel.entries[localIdx];
}

function getFocusedPanel(state: AppState, panelOverride?: PanelState, panelIdOverride?: string) {
  if (panelOverride) {
    return {
      panelId: panelIdOverride ?? panelOverride.panelId,
      panel: panelOverride,
    };
  }

  const panelId = panelIdOverride ?? state.focus;
  const panel = state.panels[panelId];
  return panel ? { panelId, panel } : null;
}

function getOtherPanelId(state: AppState, currentPanelId: string) {
  const panelIds = state.layout.panels.length > 0 ? state.layout.panels : Object.keys(state.panels);
  return panelIds.find((id) => id !== currentPanelId);
}

function currentEntry(panel: PanelState) {
  return entryAt(panel, panel.cursor);
}

function openInfo(dispatch: Dispatch<AppAction>, panel: PanelState, state: AppState) {
  const entry = currentEntry(panel);
  if (!entry) return false;

  let dirPath = panel.path.replace(/[/\\]+$/, "");
  let fullPath = entry.name === ".." ? dirPath : `${dirPath}/${entry.name}`;

  // When the panel is on a search results source, resolve the real filesystem
  // paths from the job's accumulated results.
  if (panel.sourceId && isSearchSourceId(panel.sourceId)) {
    const jobId = panel.sourceId.replace(/^search:/, "");
    const job = state.jobs[jobId];
    const result = job?.searchResults?.find((r) => r.name === entry.name);
    if (result) {
      fullPath = result.fullPath;
      dirPath = result.parentPath;
    }
  }

  dispatch({
    type: "OPEN_INFO",
    info: {
      name: entry.name,
      kind: entry.kind,
      fullPath,
      dirPath,
      size: entry.size,
      mtime: entry.mtime,
      btime: entry.btime,
      permissions: entry.permissions,
      iconName: entry.iconName,
    },
  });
  return true;
}

function openAttributes(dispatch: Dispatch<AppAction>, panel: PanelState, state: AppState) {
  const indices = panel.selection.length > 0 ? panel.selection : [panel.cursor];
  const entries = indices
    .map((i) => entryAt(panel, i))
    .filter((e) => !!e && e.name !== "..") as VFSEntry[];

  if (entries.length === 0) return false;

  let dirPath = panel.path.replace(/[/\\]+$/, "");
  const paths: string[] = [];
  const names: string[] = [];

  for (const entry of entries) {
    let fullPath = `${dirPath}/${entry.name}`;
    if (panel.sourceId && isSearchSourceId(panel.sourceId)) {
      const jobId = panel.sourceId.replace(/^search:/, "");
      const job = state.jobs[jobId];
      const result = job?.searchResults?.find((r) => r.name === entry.name);
      if (result) {
        fullPath = result.fullPath;
      }
    }
    paths.push(fullPath);
    names.push(entry.name);
  }

  dispatch({
    type: "OPEN_ATTRIBUTES",
    attributes: {
      panelId: panel.panelId,
      paths,
      names,
      currentMode: entries[0].permissions ?? "rwxrwxrwx",
    },
  });
  return true;
}

function openRename(dispatch: Dispatch<AppAction>, panel: PanelState) {
  const entry = currentEntry(panel);
  if (!entry || entry.name === "..") return false;

  const dirPath = panel.path.replace(/[/\\]+$/, "");
  return openRenameForTarget(dispatch, {
    panelId: panel.panelId,
    dirPath,
    fullPath: `${dirPath}/${entry.name}`,
    name: entry.name,
  });
}

function openRenameForTarget(dispatch: Dispatch<AppAction>, target: Pick<ContextActionTarget, "panelId" | "dirPath" | "fullPath" | "name">) {
  dispatch({
    type: "OPEN_RENAME",
    rename: {
      sourcePanel: target.panelId,
      sourcePath: target.fullPath,
      dirPath: target.dirPath,
      filename: target.name,
    },
  });
  return true;
}

function openFileOp(state: AppState, dispatch: Dispatch<AppAction>, panel: PanelState, opType: FileOpType) {
  const files: FileOpFile[] = panel.selection.length > 0
    ? panel.selectedEntries
    : (() => {
        const entry = currentEntry(panel);
        return entry ? [{ name: entry.name, size: entry.size, kind: entry.kind }] : [];
      })();

  return openFileOpForEntries(state, dispatch, panel.panelId, files, opType);
}

function broSuggestedOutputDir(state: AppState, panelId: string) {
  const otherPanelId = getOtherPanelId(state, panelId);
  return otherPanelId ? state.panels[otherPanelId]?.path : undefined;
}

function openPassToBro(
  state: AppState,
  dispatch: Dispatch<AppAction>,
  client: WsClient,
  panel: PanelState,
  explicitPath?: string,
) {
  const requestId = globalThis.crypto?.randomUUID?.() ?? `bro-${Date.now()}-${Math.random().toString(16).slice(2)}`;
  const suggestedOutputDir = broSuggestedOutputDir(state, panel.panelId);
  dispatch({
    type: "OPEN_BRO",
    bro: {
      panelId: panel.panelId,
      dirPath: panel.path.replace(/[/\\]+$/, ""),
      requestId,
      explicitPath,
      status: "loading",
      inputs: [],
      bros: [],
    },
  });
  client.sendIntent("ListBros", panel.panelId, {
    requestId,
    targetPath: explicitPath,
    suggestedOutputDir,
  });
  return true;
}

function openFileOpForEntries(
  state: AppState,
  dispatch: Dispatch<AppAction>,
  sourcePanel: string,
  files: FileOpFile[],
  opType: FileOpType,
) {
  if (files.length === 0) return false;

  const destPanelId = opType !== "delete"
    ? getOtherPanelId(state, sourcePanel) ?? undefined
    : undefined;
  const destPath = destPanelId ? state.panels[destPanelId]?.path : undefined;

  dispatch({
    type: "OPEN_OPERATION",
    op: {
      opType,
      files,
      sourcePanel,
      destPanel: destPanelId,
      destPath,
      conflictPolicy: "ask",
    },
  });
  return true;
}

export function executeTargetedCommand(
  commandId: string,
  context: Pick<ExecuteCommandContext, "state" | "dispatch" | "client">,
  target: ContextActionTarget,
) {
  const file: FileOpFile = {
    name: target.name,
    size: target.size,
    kind: target.kind,
    sourcePath: target.fullPath,
  };
  const files = target.entries?.length ? target.entries : [file];
  const explicitPath = target.scope === "selection" ? undefined : target.fullPath;

  switch (commandId) {
    case "panel.pass_to_bro":
      return context.state.panels[target.panelId]
        ? openPassToBro(context.state, context.dispatch, context.client, context.state.panels[target.panelId], explicitPath)
        : false;
    case "file.compare":
      context.client.sendIntent("CompareFiles", target.panelId, {
        panelId: target.panelId,
        ...(explicitPath ? { targetPath: explicitPath } : {}),
      });
      return true;
    case "file.copy":
      return openFileOpForEntries(context.state, context.dispatch, target.panelId, files, "copy");
    case "file.move":
      return openFileOpForEntries(context.state, context.dispatch, target.panelId, files, "move");
    case "file.delete":
      return openFileOpForEntries(context.state, context.dispatch, target.panelId, files, "delete");
    case "file.rename":
      return openRenameForTarget(context.dispatch, target);
    default:
      return false;
  }
}

function openViewerIntent(client: WsClient, panel: PanelState, intentName: "ViewFile" | "EditFile") {
  const entry = currentEntry(panel);
  if (!entry || entry.kind === "directory") return false;

  if (intentName === "ViewFile") {
    client.sendIntent("ViewFile", undefined, {
      panelId: panel.panelId,
      path: panel.path,
      sourceId: panel.sourceId,
      filename: entry.name,
    });
  } else {
    client.sendIntent("EditFile", undefined, {
      path: panel.path,
      filename: entry.name,
    });
  }
  return true;
}

function toggleSelectionOrSize(client: WsClient, panel: PanelState) {
  const cursorEntry = currentEntry(panel);
  const calcSet = new Set(panel.calculatingSize ?? []);

  if (cursorEntry?.kind === "directory" && panel.selection.length === 0) {
    if (calcSet.has(cursorEntry.name)) {
      client.sendIntent("CancelSizeCalculation", "focused-panel", { name: cursorEntry.name });
    } else {
      client.sendIntent("CalculateSize", "focused-panel", { name: cursorEntry.name });
    }
  } else {
    client.sendIntent("ToggleSelection", "focused-panel");
  }
  return true;
}

function toggleSelectedDirectorySizes(client: WsClient, panel: PanelState) {
  const calcSet = new Set(panel.calculatingSize ?? []);
  const isWindowed = panel.totalCount !== undefined && panel.totalCount > panel.entries.length;
  const windowStart = panel.windowStart ?? 0;
  const entriesToCalc = panel.selection.length > 0
    ? panel.selection.map((index) => (
        isWindowed
          ? (index >= windowStart && index < windowStart + panel.entries.length
              ? panel.entries[index - windowStart]
              : undefined)
          : panel.entries[index]
      ))
    : panel.entries;

  for (const entry of entriesToCalc) {
    if (!entry || entry.kind !== "directory") continue;
    if (calcSet.has(entry.name)) {
      client.sendIntent("CancelSizeCalculation", "focused-panel", { name: entry.name });
    } else {
      client.sendIntent("CalculateSize", "focused-panel", { name: entry.name });
    }
  }
  return true;
}

function openFocusedEntry(client: WsClient, panel: PanelState, sources: Record<string, Source>) {
  const entry = currentEntry(panel);
  if (entry && entry.kind !== "directory") {
    // Archive files are navigable — treat Enter the same as navigating into a directory.
    const lname = entry.name.toLowerCase();
    if (sources[panel.sourceId]?.kind === "remote" || panel.sourceId === "journal") {
      client.sendIntent("ViewFile", undefined, {
        panelId: panel.panelId,
        path: panel.path,
        sourceId: panel.sourceId,
        filename: entry.name,
      });
    } else if (entry.kind === "file" && (lname.endsWith(".zip") || lname.endsWith(".7z")) && !panel.shadowSource) {
      client.sendIntent("OpenEntry", "focused-panel");
    } else {
      client.sendIntent("OpenFile", "focused-panel");
    }
  } else {
    client.sendIntent("OpenEntry", "focused-panel");
  }
  return true;
}

function navigateSource(client: WsClient, panelId: string, panel: PanelState, sources: Record<string, Source>, direction: "next" | "previous") {
  const sourceList = Object.values(sources);
  if (sourceList.length <= 1) return false;

  const current = sourceList.findIndex((source) => source.id === panel.sourceId);
  if (current === -1) return false;

  const nextIndex = direction === "previous"
    ? (current - 1 + sourceList.length) % sourceList.length
    : (current + 1) % sourceList.length;
  client.sendIntent("NavigateToSource", panelId, { sourceId: sourceList[nextIndex].id });
  return true;
}

export function getCommand(commands: Record<string, InteractiveCommand>, commandId: string) {
  return commands[commandId];
}

export function formatBinding(binding: KeyBinding) {
  const parts: string[] = [];
  if (binding.meta) parts.push(isMac ? "Cmd" : "Meta");
  if (binding.ctrl) parts.push("Ctrl");
  if (binding.alt) parts.push("Alt");
  if (binding.shift) parts.push("Shift");
  parts.push(keyLabel(binding.key));
  return parts.join("+");
}

export function formatPrimaryBinding(command?: InteractiveCommand) {
  return command?.bindings[0] ? formatBinding(command.bindings[0]) : "";
}

export function bindingMatches(event: KeyboardLikeEvent, binding: KeyBinding, mode?: "list" | "grid") {
  if (binding.modes && binding.modes.length > 0) {
    if (!mode || !binding.modes.includes(mode)) return false;
  }
  const normalizedEventKey = normalizeKey(event.key);
  const normalizedBindingKey = normalizeKey(binding.key);
  const matchesPhysicalLetterKey =
    normalizedBindingKey.length === 1
    && /^[a-z]$/i.test(normalizedBindingKey)
    && event.code === `Key${normalizedBindingKey.toUpperCase()}`;
  const matchesPhysicalDigitKey =
    normalizedBindingKey.length === 1
    && /^[0-9]$/.test(normalizedBindingKey)
    && event.code === `Digit${normalizedBindingKey}`;
  const isShiftProducedSymbol =
    normalizedEventKey === normalizedBindingKey
    && normalizedBindingKey.length === 1
    && normalizedBindingKey.trim().length > 0
    && /[^a-z0-9]/i.test(normalizedBindingKey)
    && !binding.shift
    && event.shiftKey;

  return (normalizedEventKey === normalizedBindingKey || matchesPhysicalLetterKey || matchesPhysicalDigitKey)
    && (isShiftProducedSymbol || !!binding.shift === event.shiftKey)
    && !!binding.ctrl === event.ctrlKey
    && !!binding.alt === event.altKey
    && !!binding.meta === event.metaKey;
}

export function findMatchingCommand(
  commands: Record<string, InteractiveCommand>,
  commandIds: readonly string[],
  event: KeyboardLikeEvent,
  mode?: "list" | "grid",
) {
  return commandIds
    .map((commandId) => commands[commandId])
    .find((command) => command && command.bindings.some((binding) => bindingMatches(event, binding, mode)));
}

export function executeCommand(commandId: string, context: ExecuteCommandContext) {
  const { state, dispatch, client } = context;
  const focused = getFocusedPanel(state, context.panel, context.panelId);
  const sourceCapability = (cap: string): boolean =>
    state.sources[focused?.panel.sourceId ?? ""]?.supports.includes(cap) ?? false;

  switch (commandId) {
    case COMMAND_PALETTE_COMMAND_ID:
      context.onOpenCommandPalette?.();
      return !!context.onOpenCommandPalette;
    case "app.open_jobs":
      context.onOpenJobsView?.();
      return !!context.onOpenJobsView;
    case "app.settings":
      dispatch({ type: "OPEN_SETTINGS", tab: "general" });
      return true;
    case "app.install_fbro_command":
      void invoke<{ path: string; command: string }>("install_fbro_command")
        .then((result) => {
          dispatch({
            type: "SHOW_TRANSIENT_HINT",
            message: `Installed ${result.command} at ${result.path}`,
          });
        })
        .catch((error) => {
          dispatch({
            type: "SHOW_TRANSIENT_HINT",
            message: error instanceof Error ? error.message : String(error),
          });
        });
      return true;
    case "panel.viewer.embed":
      dispatch({ type: "TOGGLE_EMBEDDED_VIEWER" });
      return true;
    case "app.search":
      if (!focused) return false;
      dispatch({
        type: "OPEN_SEARCH",
        search: {
          sourcePanelPath: focused.panel.path,
          jobId: null,
          searchSourceId: null,
          searchParams: null,
          results: [],
          scanned: 0,
          matched: 0,
          currentPath: "",
          status: "idle",
          activeTab: "input",
          requestId: null,
        },
      });
      return true;
    case "panel.navigate_source_root":
      client.sendIntent("NavigateSourceRoot", focused?.panelId ?? state.focus);
      return true;
    case "panel.refresh":
      if (!focused) return false;
      client.sendIntent("Refresh", focused.panelId);
      return true;
    case "panel.pass_to_bro":
      if (focused && isGlobalSelectionSourceId(focused.panel.sourceId) && focused.panel.path !== "") return false;
      return focused ? openPassToBro(state, dispatch, client, focused.panel) : false;
    case "file.compare":
      if (!focused) return false;
      client.sendIntent("CompareFiles", focused.panelId, { panelId: focused.panelId });
      return true;
    case "file.info":
      return focused ? openInfo(dispatch, focused.panel, state) : false;
    case "file.rename":
      if (!focused || !sourceCapability("Rename")) return false;
      if (isGlobalSelectionSourceId(focused.panel.sourceId) && focused.panel.path !== "") return false;
      return openRename(dispatch, focused.panel);
    case "file.view":
      if (focused && isGlobalSelectionSourceId(focused.panel.sourceId)) return false;
      return focused ? openViewerIntent(client, focused.panel, "ViewFile") : false;
    case "file.edit":
      if (!focused || !sourceCapability("Edit")) return false;
      if (isGlobalSelectionSourceId(focused.panel.sourceId)) return false;
      return openViewerIntent(client, focused.panel, "EditFile");
    case "file.quick_edit": {
      if (isWindows || !focused || !sourceCapability("Edit") || !sourceCapability("OpenShell")) return false;
      const entry = currentEntry(focused.panel);
      if (!entry || entry.kind !== "file" || focused.panel.terminalSessionId !== undefined || focused.panel.shadowSource) return false;
      client.sendIntent("QuickEditFile", focused.panelId);
      return true;
    }
    case "file.copy":
      if (!focused || !sourceCapability("CopySelection")) return false;
      return openFileOp(state, dispatch, focused.panel, "copy");
    case "file.copy_external":
      if (!focused || focused.panel.shadowSource) return false;
      client.sendIntent("WriteExternalClipboard", undefined, {
        panelId: focused.panelId,
        opType: "copy",
      });
      return true;
    case "file.move":
      if (!focused || !sourceCapability("MoveSelection")) return false;
      return openFileOp(state, dispatch, focused.panel, "move");
    case "file.cut_external":
      if (!focused || focused.panel.shadowSource) return false;
      client.sendIntent("WriteExternalClipboard", undefined, {
        panelId: focused.panelId,
        opType: "move",
      });
      return true;
    case "file.paste_external":
      if (!focused || focused.panel.shadowSource) return false;
      client.sendIntent("PasteExternalClipboard", undefined, { panelId: focused.panelId });
      return true;
    case "file.paste_external_move":
      if (!focused || focused.panel.shadowSource) return false;
      client.sendIntent("PasteExternalClipboard", undefined, {
        panelId: focused.panelId,
        preferredOp: "move",
      });
      return true;
    case "file.touch":
      if (!focused || !sourceCapability("Touch")) return false;
      dispatch({
        type: "OPEN_TOUCH",
        touch: { sourcePanel: focused.panelId, dirPath: focused.panel.path },
      });
      return true;
    case "file.set_permissions":
      if (!focused) return false;
      if (focused.panel.shadowSource) return false;
      const attrsSource = state.sources[focused.panel.sourceId];
      if (!attrsSource || attrsSource.kind !== "filesystem") return false;
      return openAttributes(dispatch, focused.panel, state);
    case "file.mkdir":
      if (!focused || !sourceCapability("NewFolder")) return false;
      dispatch({
        type: "OPEN_MKDIR",
        mkdir: { sourcePanel: focused.panelId, dirPath: focused.panel.path },
      });
      return true;
    case "file.delete":
      if (!focused || !sourceCapability("DeleteSelection")) return false;
      return openFileOp(state, dispatch, focused.panel, "delete");
    case "app.open_terminal":
      if (!focused || !sourceCapability("OpenShell")) return false;
      client.sendIntent("OpenTerminal", focused.panelId);
      return true;
    case "app.quit":
      client.sendIntent("Quit");
      return true;
    case "panel.cursor.up": {
      if (focused?.panel.viewMode === "grid") {
        const cols = context.getGridCols?.() ?? 1;
        client.sendIntent("MoveCursorBy", "focused-panel", { delta: -cols });
      } else {
        client.sendIntent("MoveCursor", "focused-panel", { direction: "up" });
      }
      return true;
    }
    case "panel.cursor.down": {
      if (focused?.panel.viewMode === "grid") {
        const cols = context.getGridCols?.() ?? 1;
        client.sendIntent("MoveCursorBy", "focused-panel", { delta: cols });
      } else {
        client.sendIntent("MoveCursor", "focused-panel", { direction: "down" });
      }
      return true;
    }
    case "panel.cursor.left":
      if (focused && focused.panel.viewMode === "grid" && focused.panel.cursor === 0) {
        client.sendIntent("NavigateUp", "focused-panel", {
          sourceId: focused.panel.sourceId,
          path: focused.panel.path,
          panelEpoch: focused.panel.panelEpoch,
        });
      } else {
        client.sendIntent("MoveCursorBy", "focused-panel", { delta: -1 });
      }
      return true;
    case "panel.cursor.right":
      client.sendIntent("MoveCursorBy", "focused-panel", { delta: 1 });
      return true;
    case "panel.cursor.first":
      client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "first" });
      return true;
    case "panel.cursor.last":
      client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "last" });
      return true;
    case "panel.selection.extend_up": {
      if (focused?.panel.viewMode === "grid") {
        const cols = context.getGridCols?.() ?? 1;
        client.sendIntent("MoveCursorWithSelect", "focused-panel", { delta: -cols });
      } else {
        client.sendIntent("MoveCursorWithSelect", "focused-panel", { direction: "up" });
      }
      return true;
    }
    case "panel.selection.extend_down": {
      if (focused?.panel.viewMode === "grid") {
        const cols = context.getGridCols?.() ?? 1;
        client.sendIntent("MoveCursorWithSelect", "focused-panel", { delta: cols });
      } else {
        client.sendIntent("MoveCursorWithSelect", "focused-panel", { direction: "down" });
      }
      return true;
    }
    case "panel.selection.extend_first":
      client.sendIntent("MoveCursorWithSelect", "focused-panel", { edge: "first" });
      return true;
    case "panel.selection.extend_last":
      client.sendIntent("MoveCursorWithSelect", "focused-panel", { edge: "last" });
      return true;
    case "panel.selection.extend_left":
      client.sendIntent("MoveCursorWithSelect", "focused-panel", { delta: -1 });
      return true;
    case "panel.selection.extend_right":
      client.sendIntent("MoveCursorWithSelect", "focused-panel", { delta: 1 });
      return true;
    case "panel.selection.toggle_or_size":
      return focused ? toggleSelectionOrSize(client, focused.panel) : false;
    case "panel.selection.calculate_sizes":
      return focused ? toggleSelectedDirectorySizes(client, focused.panel) : false;
    case "panel.selection.select_all":
      client.sendIntent("SelectAll", "focused-panel");
      return true;
    case "panel.selection.clear":
      client.sendIntent("ClearSelection", "focused-panel");
      return true;
    case "panel.selection.invert":
      client.sendIntent("InvertSelection", "focused-panel");
      return true;
    case "panel.cursor.page_up":
    case "panel.cursor.page_down": {
      let pageSize: number;
      if (focused?.panel.viewMode === "grid") {
        const cols = context.getGridCols?.() ?? 1;
        const rows = context.getGridPageRows?.() ?? 1;
        pageSize = Math.max(1, cols * rows);
      } else {
        pageSize = context.getPanelPageSize?.() ?? 1;
      }
      const delta = commandId === "panel.cursor.page_down" ? pageSize : -pageSize;
      client.sendIntent("MoveCursorBy", "focused-panel", { delta });
      return true;
    }
    case "panel.open":
      return focused ? openFocusedEntry(client, focused.panel, state.sources) : false;
    case "panel.open_entry":
      client.sendIntent("OpenEntry", "focused-panel");
      return true;
    case "panel.navigate_up":
      if (!focused) return false;
      client.sendIntent("NavigateUp", "focused-panel", {
        sourceId: focused.panel.sourceId,
        path: focused.panel.path,
        panelEpoch: focused.panel.panelEpoch,
      });
      return true;
    case "panel.navigate_home":
      client.sendIntent("NavigateHome", "focused-panel");
      return true;
    case "panel.navigate_to_path":
      if (!focused) return false;
      dispatch({
        type: "OPEN_GO_TO_PATH",
        goToPath: {
          panelId: focused.panelId,
          currentPath: focused.panel.path,
        },
      });
      return true;
    case "panel.history.open": {
      if (!focused) return false;
      const hist = state.panelHistory[focused.panelId];
      const entries: string[] = [];
      const seen = new Set<string>();
      const rawEntries = hist ? [...hist.entries].reverse() : [];
      for (const e of rawEntries) {
        if (
          !seen.has(e.path) &&
          e.path &&
          e.sourceId !== "bookmarks" &&
          e.sourceId !== "history" &&
          !isSearchSourceId(e.sourceId) &&
          !e.sourceId.startsWith("zip:") &&
          !e.sourceId.startsWith("7z:")
        ) {
          seen.add(e.path);
          entries.push(e.path);
        }
      }
      client.sendIntent("EnterHistorySource", focused.panelId, { entries });
      return true;
    }
    case "panel.history.back": {
      if (!focused) return false;
      const hb = state.panelHistory[focused.panelId];
      if (!hb || hb.cursor <= 0) return false;
      const targetB = hb.entries[hb.cursor - 1];
      dispatch({ type: "NAV_HISTORY_BACK", panelId: focused.panelId });
      markHistoryNavPending(focused.panelId);
      client.sendIntent("NavigateToHistoryEntry", undefined, {
        panelId: focused.panelId, path: targetB.path, sourceId: targetB.sourceId,
      });
      return true;
    }
    case "panel.history.forward": {
      if (!focused) return false;
      const hf = state.panelHistory[focused.panelId];
      if (!hf || hf.cursor >= hf.entries.length - 1) return false;
      const targetF = hf.entries[hf.cursor + 1];
      dispatch({ type: "NAV_HISTORY_FORWARD", panelId: focused.panelId });
      markHistoryNavPending(focused.panelId);
      client.sendIntent("NavigateToHistoryEntry", undefined, {
        panelId: focused.panelId, path: targetF.path, sourceId: targetF.sourceId,
      });
      return true;
    }
    case "panel.focus.switch": {
      if (!focused) return false;
      const target = getOtherPanelId(state, focused.panelId);
      if (!target) return false;
      client.sendIntent("SwitchFocus", undefined, { to: target });
      return true;
    }
    case "panel.source.next":
      return focused ? navigateSource(client, focused.panelId, focused.panel, context.sources ?? state.sources, "next") : false;
    case "panel.source.previous":
      return focused ? navigateSource(client, focused.panelId, focused.panel, context.sources ?? state.sources, "previous") : false;
    case "panel.filter.open":
      if (!focused || !context.onOpenFilter) return false;
      context.onOpenFilter(focused.panel.filter?.value ?? "");
      return true;
    case "panel.path.sync": {
      if (!focused) return false;
      const otherPanelId = context.otherPanelId ?? getOtherPanelId(state, focused.panelId);
      if (!otherPanelId) return false;
      client.sendIntent("NavigatePanelTo", undefined, {
        panelId: otherPanelId,
        path: focused.panel.path,
      });
      return true;
    }
    case "panel.context_menu": {
      if (!focused) return false;
      const panelElement = document.querySelector<HTMLElement>(`[data-panel-id="${focused.panelId}"]`);
      const row = panelElement?.querySelector<HTMLElement>(`[data-abs-index="${focused.panel.cursor}"]`);
      const rect = row?.getBoundingClientRect();
      if (!rect) return false;
      const entry = currentEntry(focused.panel);
      if (!entry || entry.name === "..") return false;
      const dirPath = focused.panel.path.replace(/[/\\]+$/, "");
      const fullPath = entry.sourcePath ?? `${dirPath}/${entry.name}`;
      const targetsSelection = contextualEntryIsSelected(focused.panel, focused.panel.cursor);
      dispatch({
        type: "OPEN_CONTEXT_MENU",
        menu: {
          panelId: focused.panelId,
          absIndex: focused.panel.cursor,
          name: entry.name,
          kind: entry.kind,
          size: entry.size,
          dirPath,
          fullPath,
          anchorLeft: rect.left,
          anchorRight: rect.right,
          anchorTop: rect.top,
          anchorBottom: rect.bottom,
          isShadow: !!focused.panel.shadowSource,
          targetScope: targetsSelection ? "selection" : "entry",
          targetCount: targetsSelection ? focused.panel.selectedEntries.length : 1,
        },
      });
      const panelDiv = document.querySelector<HTMLElement>(`[data-panel-id="${focused.panelId}"]`);
      panelDiv?.focus();
      return true;
    }
    case "panel.toggle_hidden": {
      if (!focused) return false;
      client.sendIntent("ToggleHidden", focused.panelId, {});
      return true;
    }
    case "panel.toggle_view_mode": {
      if (!focused) return false;
      const focusedPanel = state.panels[focused.panelId];
      if (!focusedPanel) return false;
      const next = focusedPanel.viewMode === "grid" ? "list" : "grid";
      if (next === "grid" && !focusedPanel.supportsThumbnails) {
        dispatch({
          type: "SHOW_TRANSIENT_HINT",
          message: "Grid view is not available for this source",
        });
        return true;
      }
      client.sendIntent("SetPanelViewMode", focused.panelId, { mode: next });
      return true;
    }
    case "panel.sort.open": {
      if (!focused) return false;
      dispatch({ type: "OPEN_SORT_SELECTOR", panelId: focused.panelId });
      return true;
    }
    case "panel.bookmarks.open": {
      if (!focused) return false;
      dispatch({ type: "OPEN_BOOKMARKS", panelId: focused.panelId });
      return true;
    }
    case "panel.bookmarks.browse": {
      if (!focused) return false;
      client.sendIntent("EnterBookmarkSource", focused.panelId);
      return true;
    }
    case "panel.tab.new": {
      if (!focused) return false;
      client.sendIntent("panel.tab.new", undefined, {
        slotId: focused.panel.slotId,
        source: "clone",
      });
      return true;
    }
    case "panel.tab.close": {
      if (!focused) return false;
      const slot = state.slots[focused.panel.slotId];
      if (!slot || slot.tabPanelIds.length <= 1) return false;
      client.sendIntent("panel.tab.close", undefined, { panelId: focused.panelId });
      return true;
    }
    case "panel.tab.next":
    case "panel.tab.prev": {
      if (!focused) return false;
      const slot = state.slots[focused.panel.slotId];
      if (!slot || slot.tabPanelIds.length <= 1) return false;
      const idx = slot.tabPanelIds.indexOf(focused.panelId);
      if (idx < 0) return false;
      const dir = commandId === "panel.tab.next" ? 1 : -1;
      const nextIdx = (idx + dir + slot.tabPanelIds.length) % slot.tabPanelIds.length;
      const nextId = slot.tabPanelIds[nextIdx];
      client.sendIntent("panel.tab.switch", undefined, {
        slotId: slot.slotId,
        panelId: nextId,
      });
      return true;
    }
    case "panel.toggle_embedded_terminal": {
      if (!focused || !sourceCapability("OpenShell")) return false;
      const focusedPanel = state.panels[focused.panelId];
      if (!focusedPanel) return false;
      if (focusedPanel.terminalSessionId !== undefined) {
        client.sendIntent("ClosePanelTerminal", focused.panelId);
      } else {
        client.sendIntent("OpenPanelTerminal", focused.panelId, { cols: 80, rows: 24 });
      }
      return true;
    }
    default:
      return false;
  }
}
