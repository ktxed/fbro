import type { Dispatch } from "react";

import { isMac } from "../utils/platform";
import type { AppAction, AppState } from "../store";
import type { WsClient } from "../ws/client";
import type {
  FileOpFile,
  FileOpType,
  InteractiveCommand,
  KeyBinding,
  PanelState,
  Source,
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
  "panel.navigate_source_root",
  "file.info",
  "file.paste_external",
  "file.paste_external_move",
  ...TOOLBAR_COMMAND_IDS,
] as const;

export const PANEL_SHORTCUT_COMMAND_IDS = [
  "panel.cursor.up",
  "panel.cursor.down",
  "panel.cursor.first",
  "panel.cursor.last",
  "panel.selection.extend_up",
  "panel.selection.extend_down",
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
  "panel.history.open",
  "panel.pass_to_bro",
  "app.open_terminal",
  "panel.focus.switch",
  "panel.source.next",
  "panel.source.previous",
  "panel.filter.open",
  "panel.path.sync",
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
  getPanelPageSize?: () => number;
}

export interface ContextActionTarget {
  panelId: string;
  dirPath: string;
  fullPath: string;
  name: string;
  kind: string;
  size?: number;
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

function openInfo(dispatch: Dispatch<AppAction>, panel: PanelState) {
  const entry = currentEntry(panel);
  if (!entry) return false;

  const dirPath = panel.path.replace(/[/\\]+$/, "");
  dispatch({
    type: "OPEN_INFO",
    info: {
      name: entry.name,
      kind: entry.kind,
      fullPath: entry.name === ".." ? dirPath : `${dirPath}/${entry.name}`,
      dirPath,
      size: entry.size,
      mtime: entry.mtime,
      btime: entry.btime,
      permissions: entry.permissions,
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
  };

  switch (commandId) {
    case "panel.pass_to_bro":
      return context.state.panels[target.panelId]
        ? openPassToBro(context.state, context.dispatch, context.client, context.state.panels[target.panelId], target.fullPath)
        : false;
    case "file.copy":
      return openFileOpForEntries(context.state, context.dispatch, target.panelId, [file], "copy");
    case "file.move":
      return openFileOpForEntries(context.state, context.dispatch, target.panelId, [file], "move");
    case "file.delete":
      return openFileOpForEntries(context.state, context.dispatch, target.panelId, [file], "delete");
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

function openFocusedEntry(client: WsClient, panel: PanelState) {
  const entry = currentEntry(panel);
  if (entry && entry.kind !== "directory") {
    client.sendIntent("OpenFile", "focused-panel");
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

export function bindingMatches(event: KeyboardLikeEvent, binding: KeyBinding) {
  const normalizedEventKey = normalizeKey(event.key);
  const normalizedBindingKey = normalizeKey(binding.key);
  const matchesPhysicalLetterKey =
    normalizedBindingKey.length === 1
    && /^[a-z]$/i.test(normalizedBindingKey)
    && event.code === `Key${normalizedBindingKey.toUpperCase()}`;
  const isShiftProducedSymbol =
    normalizedEventKey === normalizedBindingKey
    && normalizedBindingKey.length === 1
    && normalizedBindingKey.trim().length > 0
    && /[^a-z0-9]/i.test(normalizedBindingKey)
    && !binding.shift
    && event.shiftKey;

  return (normalizedEventKey === normalizedBindingKey || matchesPhysicalLetterKey)
    && (isShiftProducedSymbol || !!binding.shift === event.shiftKey)
    && !!binding.ctrl === event.ctrlKey
    && !!binding.alt === event.altKey
    && !!binding.meta === event.metaKey;
}

export function findMatchingCommand(
  commands: Record<string, InteractiveCommand>,
  commandIds: readonly string[],
  event: KeyboardLikeEvent,
) {
  return commandIds
    .map((commandId) => commands[commandId])
    .find((command) => command && command.bindings.some((binding) => bindingMatches(event, binding)));
}

export function executeCommand(commandId: string, context: ExecuteCommandContext) {
  const { state, dispatch, client } = context;
  const focused = getFocusedPanel(state, context.panel, context.panelId);

  switch (commandId) {
    case COMMAND_PALETTE_COMMAND_ID:
      context.onOpenCommandPalette?.();
      return !!context.onOpenCommandPalette;
    case "app.open_jobs":
      context.onOpenJobsView?.();
      return !!context.onOpenJobsView;
    case "panel.navigate_source_root":
      client.sendIntent("NavigateSourceRoot", focused?.panelId ?? state.focus);
      return true;
    case "panel.pass_to_bro":
      return focused ? openPassToBro(state, dispatch, client, focused.panel) : false;
    case "file.info":
      return focused ? openInfo(dispatch, focused.panel) : false;
    case "file.rename":
      return focused ? openRename(dispatch, focused.panel) : false;
    case "file.view":
      return focused ? openViewerIntent(client, focused.panel, "ViewFile") : false;
    case "file.edit":
      return focused ? openViewerIntent(client, focused.panel, "EditFile") : false;
    case "file.copy":
      return focused ? openFileOp(state, dispatch, focused.panel, "copy") : false;
    case "file.move":
      return focused ? openFileOp(state, dispatch, focused.panel, "move") : false;
    case "file.paste_external":
      if (!focused) return false;
      client.sendIntent("PasteExternalClipboard", undefined, { panelId: focused.panelId });
      return true;
    case "file.paste_external_move":
      if (!focused) return false;
      client.sendIntent("PasteExternalClipboard", undefined, {
        panelId: focused.panelId,
        preferredOp: "move",
      });
      return true;
    case "file.mkdir":
      if (!focused) return false;
      dispatch({
        type: "OPEN_MKDIR",
        mkdir: { sourcePanel: focused.panelId, dirPath: focused.panel.path },
      });
      return true;
    case "file.delete":
      return focused ? openFileOp(state, dispatch, focused.panel, "delete") : false;
    case "app.open_terminal":
      client.sendIntent("OpenTerminal", focused?.panelId ?? state.focus);
      return true;
    case "app.quit":
      client.sendIntent("Quit");
      return true;
    case "panel.cursor.up":
      client.sendIntent("MoveCursor", "focused-panel", { direction: "up" });
      return true;
    case "panel.cursor.down":
      client.sendIntent("MoveCursor", "focused-panel", { direction: "down" });
      return true;
    case "panel.cursor.first":
      client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "first" });
      return true;
    case "panel.cursor.last":
      client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "last" });
      return true;
    case "panel.selection.extend_up":
      client.sendIntent("MoveCursorWithSelect", "focused-panel", { direction: "up" });
      return true;
    case "panel.selection.extend_down":
      client.sendIntent("MoveCursorWithSelect", "focused-panel", { direction: "down" });
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
      const pageSize = context.getPanelPageSize?.() ?? 1;
      const delta = commandId === "panel.cursor.page_down" ? pageSize : -pageSize;
      client.sendIntent("MoveCursorBy", "focused-panel", { delta });
      return true;
    }
    case "panel.open":
      return focused ? openFocusedEntry(client, focused.panel) : false;
    case "panel.open_entry":
      client.sendIntent("OpenEntry", "focused-panel");
      return true;
    case "panel.navigate_up":
      client.sendIntent("NavigateUp", "focused-panel");
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
    case "panel.history.open":
      if (!focused) return false;
      dispatch({
        type: "OPEN_PANEL_HISTORY",
        panelHistory: {
          panelId: focused.panelId,
          currentPath: focused.panel.path,
        },
      });
      return true;
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
    default:
      return false;
  }
}
