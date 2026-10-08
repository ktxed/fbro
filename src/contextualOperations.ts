import type { AppState } from "./store";
import { isLikelyBinaryPath } from "./utils/binaryPath";
import { isGlobalSelectionSourceId, isSearchSourceId } from "./utils/sourceId";
import type { FileOpFile, PanelState, VFSEntry } from "./ws/types";

export type ContextualOperationId =
  | "platform.reveal"
  | "platform.open-with"
  | "panel.browse-archive"
  | "panel.jump-to-location"
  | "panel.pass-to-bro"
  | "file.compare"
  | "git.view-diff"
  | "git.init"
  | "git.stage"
  | "git.unstage"
  | "git.rollback"
  | "selection.to-global"
  | "file.preview"
  | "file.copy"
  | "file.move"
  | "file.rename"
  | "file.delete"
  | "clipboard.names"
  | "clipboard.paths";

export interface OperationCardinality {
  min: number;
  max?: number;
}

export interface ContextualEntry extends FileOpFile {
  fullPath: string;
  entry?: VFSEntry;
}

export interface ContextualTarget {
  panelId: string;
  dirPath: string;
  scope: "entry" | "selection";
  entries: ContextualEntry[];
  primary: ContextualEntry;
  count: number;
}

export const OPERATION_CARDINALITY: Record<ContextualOperationId, OperationCardinality> = {
  "platform.reveal": { min: 1, max: 1 },
  "platform.open-with": { min: 1, max: 1 },
  "panel.browse-archive": { min: 1, max: 1 },
  "panel.jump-to-location": { min: 1, max: 1 },
  "panel.pass-to-bro": { min: 1 },
  "file.compare": { min: 1, max: 2 },
  "git.view-diff": { min: 1, max: 1 },
  "git.init": { min: 1, max: 1 },
  "git.stage": { min: 1, max: 1 },
  "git.unstage": { min: 1, max: 1 },
  "git.rollback": { min: 1, max: 1 },
  "selection.to-global": { min: 1 },
  "file.preview": { min: 1, max: 1 },
  "file.copy": { min: 1 },
  "file.move": { min: 1 },
  "file.rename": { min: 1, max: 1 },
  "file.delete": { min: 1 },
  "clipboard.names": { min: 1 },
  "clipboard.paths": { min: 1 },
};

function entryAt(panel: PanelState, absIndex: number): VFSEntry | undefined {
  return panel.entries[absIndex - (panel.windowStart ?? 0)];
}

function fullPathFor(dirPath: string, entry: Pick<FileOpFile, "name" | "sourcePath">): string {
  return entry.sourcePath ?? `${dirPath}/${entry.name}`;
}

function sameContextualEntry(a: Pick<FileOpFile, "name" | "kind" | "sourcePath">, b: Pick<FileOpFile, "name" | "kind" | "sourcePath">): boolean {
  if (a.sourcePath && b.sourcePath) {
    return a.sourcePath === b.sourcePath && a.kind === b.kind;
  }
  return a.name === b.name && a.kind === b.kind;
}

export function contextualEntryIsSelected(panel: PanelState, absIndex: number, renderedEntry?: VFSEntry): boolean {
  const entry = renderedEntry ?? entryAt(panel, absIndex);
  return !!entry && panel.selectedEntries.some((selected) => sameContextualEntry(selected, entry));
}

export function resolveContextualTarget(panel: PanelState, absIndex: number, renderedEntry?: VFSEntry): ContextualTarget | null {
  const primaryEntry = renderedEntry ?? entryAt(panel, absIndex);
  if (!primaryEntry || primaryEntry.name === "..") return null;
  const dirPath = panel.path.replace(/[/\\]+$/, "");
  const primary: ContextualEntry = {
    name: primaryEntry.name,
    kind: primaryEntry.kind,
    size: primaryEntry.size,
    sourcePath: primaryEntry.sourcePath,
    fullPath: fullPathFor(dirPath, primaryEntry),
    entry: primaryEntry,
  };
  // Selection indices are owned by the backend listing order and can differ
  // from rendered indices while a sorted view is settling. Entry identity is
  // the stable way to decide whether the contextual row belongs to selection.
  const targetsSelection = contextualEntryIsSelected(panel, absIndex, primaryEntry);
  if (!targetsSelection) {
    return { panelId: panel.panelId, dirPath, scope: "entry", entries: [primary], primary, count: 1 };
  }
  const entries = panel.selectedEntries.map((entry) => ({
    ...entry,
    fullPath: fullPathFor(dirPath, entry),
    ...(sameContextualEntry(entry, primary) ? { entry: primaryEntry } : {}),
  }));
  return {
    panelId: panel.panelId,
    dirPath,
    scope: "selection",
    entries,
    primary,
    count: entries.length,
  };
}

export function cardinalityAllows(operationId: ContextualOperationId, count: number): boolean {
  const { min, max } = OPERATION_CARDINALITY[operationId];
  return count >= min && (max === undefined || count <= max);
}

function selectedFiles(panel?: PanelState): FileOpFile[] {
  if (!panel) return [];
  return panel.selectedEntries.filter((entry) => entry.kind === "file" && entry.name !== "..");
}

export function canCompareTarget(state: AppState, target: ContextualTarget): boolean {
  if (!cardinalityAllows("file.compare", target.count) || target.entries.some((entry) => entry.kind !== "file")) return false;
  if (target.count === 2) return true;
  const panel = state.panels[target.panelId];
  if (!panel) return false;
  const samePanelSelected = selectedFiles(panel);
  if (samePanelSelected.length === 1 && samePanelSelected[0].name !== target.primary.name) {
    return true;
  }
  const panelIds = state.layout.panels.length > 0 ? state.layout.panels : Object.keys(state.panels);
  const otherPanel = state.panels[panelIds.find((id) => id !== target.panelId) ?? ""];
  if (!otherPanel) return false;
  const otherSelected = selectedFiles(otherPanel);
  if (otherSelected.length !== 1) return false;
  const sameLocation = otherPanel.sourceId === panel.sourceId && otherPanel.path === panel.path;
  return !sameLocation || otherSelected[0].name !== target.primary.name;
}

export function contextualOperationAvailable(
  operationId: ContextualOperationId,
  state: AppState,
  target: ContextualTarget,
): boolean {
  if (!cardinalityAllows(operationId, target.count)) return false;
  const panel = state.panels[target.panelId];
  const source = panel ? state.sources[panel.sourceId] : undefined;
  if (!panel || !source) return false;
  const supports = source.supports ?? [];
  const isGlobal = isGlobalSelectionSourceId(panel.sourceId);
  const isGlobalRoot = isGlobal && panel.path === "";
  const isGlobalInside = isGlobal && panel.path !== "";
  const isSearchResult = isSearchSourceId(panel.sourceId) && !!target.primary.sourcePath;
  const isLocalFilesystem = source.kind === "filesystem"
    && !panel.shadowSource
    && !source.shadow
    && source.mounted;

  switch (operationId) {
    case "platform.reveal":
      return isLocalFilesystem;
    case "platform.open-with":
      return isLocalFilesystem || isSearchResult;
    case "panel.jump-to-location":
      return isSearchResult && target.primary.kind === "file";
    case "panel.browse-archive":
      return target.primary.entry?.browsableArchive === true;
    case "panel.pass-to-bro":
      return isLocalFilesystem || isGlobalRoot || canCompareTarget(state, target);
    case "file.compare":
      return canCompareTarget(state, target);
    case "git.view-diff":
      return isLocalFilesystem
        && target.primary.kind === "file"
        && !!target.primary.entry?.gitStatus
        && !isLikelyBinaryPath(target.primary.fullPath || target.primary.name);
    case "git.init":
      return isLocalFilesystem && !source.capabilities.readOnly && panel.tabKind === "listing"
        && target.primary.kind === "directory";
    case "git.stage":
    case "git.unstage":
    case "git.rollback":
      return isLocalFilesystem && target.primary.kind === "file" && !!target.primary.entry?.gitStatus;
    case "selection.to-global":
      return isLocalFilesystem;
    case "file.preview":
      return target.primary.kind === "file" && panel.supportsThumbnails;
    case "file.copy":
      return !!panel.shadowSource || supports.includes("CopySelection");
    case "file.move":
      return !panel.shadowSource && supports.includes("MoveSelection");
    case "file.rename":
      return (isLocalFilesystem && supports.includes("Rename")) || isGlobalRoot;
    case "file.delete":
      return (!panel.shadowSource && supports.includes("DeleteSelection")) || isGlobal;
    case "clipboard.names":
      return true;
    case "clipboard.paths":
      return isLocalFilesystem || isGlobalInside;
  }
}
