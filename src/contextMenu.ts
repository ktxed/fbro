import { formatPrimaryBinding } from "./commands/interactive";
import type { InteractiveCommand } from "./ws/types";
import { isMac, isWindows } from "./utils/platform";

export type ContextMenuItem =
  | { id: string; kind: "separator" }
  | {
      id: string;
      kind: "command" | "clipboard" | "platform" | "openWith" | "bro" | "globalSelection" | "gitDiff";
      label: string;
      shortcut?: string;
      commandId?: string;
      clipboardText?: string;
      actionId?: string;
    };

export interface ContextMenuOptions {
  isShadow?: boolean;
  canPlatform?: boolean;
  canOpenWith?: boolean;
  canPassToBro?: boolean;
  canCompare?: boolean;
  canGitDiff?: boolean;
  canGitInit?: boolean;
  canAddToGlobalSelection?: boolean;
  canCopy?: boolean;
  canMove?: boolean;
  canRename?: boolean;
  canDelete?: boolean;
  canCopyFullPath?: boolean;
  targetCount?: number;
  clipboardNames?: string;
  clipboardPaths?: string;
}

function commandItem(
  commandId: string,
  fallbackLabel: string,
  commands: Record<string, InteractiveCommand>,
): ContextMenuItem {
  const command = commands[commandId];
  return {
    id: commandId,
    kind: "command",
    label: command?.title ?? fallbackLabel,
    shortcut: formatPrimaryBinding(command),
    commandId,
  };
}

function platformItem(): ContextMenuItem {
  if (isMac) {
    return {
      id: "platform.reveal",
      kind: "platform",
      label: "Open in Finder",
      actionId: "file_manager",
    };
  }
  if (isWindows) {
    return {
      id: "platform.reveal",
      kind: "platform",
      label: "Show in Explorer",
      actionId: "file_manager",
    };
  }
  return {
    id: "platform.reveal",
    kind: "platform",
    label: "Open containing folder",
    actionId: "file_manager",
  };
}

export function buildContextMenuItems(
  commands: Record<string, InteractiveCommand>,
  fullPath: string,
  name: string,
  kind: string,
  options: ContextMenuOptions = {},
): ContextMenuItem[] {
  const isShadow = !!options.isShadow;
  const isMultiple = (options.targetCount ?? 1) > 1;
  const openWithItem: ContextMenuItem | null = options.canOpenWith && (kind === "file" || kind === "directory")
    ? { id: "platform.open-with", kind: "openWith", label: "Open With…" }
    : null;
  const broItem: ContextMenuItem | null = options.canPassToBro && (kind === "file" || kind === "directory")
    ? { id: "panel.pass-to-bro", kind: "bro", label: "Pass to Bro…" }
    : null;
  const compareItem: ContextMenuItem | null = options.canCompare && kind === "file"
    ? commandItem("file.compare", "Compare", commands)
    : null;
  const gitDiffItem: ContextMenuItem | null = options.canGitDiff && kind === "file"
    ? { id: "git.view-diff", kind: "gitDiff", label: "View Git diff" }
    : null;
  const globalSelectionItem: ContextMenuItem | null = options.canAddToGlobalSelection && (kind === "file" || kind === "directory")
    ? { id: "selection.to-global", kind: "globalSelection", label: "To global selection" }
    : null;

  return [
    ...(options.canPlatform ? [platformItem()] : []),
    ...(compareItem ? [compareItem] : []),
    ...(gitDiffItem ? [gitDiffItem] : []),
    ...(options.canGitInit && kind === "directory" ? [commandItem("git.init", "Initialize Git repository…", commands)] : []),
    ...(broItem ? [broItem] : []),
    ...(globalSelectionItem ? [globalSelectionItem] : []),
    ...(openWithItem ? [openWithItem] : []),
    ...(options.canCopy ? [commandItem("file.copy", isShadow ? "Extract" : "Copy", commands)] : []),
    ...(options.canMove ? [commandItem("file.move", "Move", commands)] : []),
    ...(options.canRename ? [commandItem("file.rename", "Rename", commands)] : []),
    ...(options.canDelete ? [commandItem("file.delete", "Delete", commands)] : []),
    { id: "separator.clipboard", kind: "separator" },
    { id: "clipboard.names", kind: "clipboard", label: isMultiple ? "Copy names" : "Copy name", clipboardText: options.clipboardNames ?? name },
    ...(options.canCopyFullPath ? [{ id: "clipboard.paths", kind: "clipboard", label: isMultiple ? "Copy full paths" : "Copy full path", clipboardText: options.clipboardPaths ?? fullPath } as ContextMenuItem] : []),
  ];
}
