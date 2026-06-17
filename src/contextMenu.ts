import { formatPrimaryBinding } from "./commands/interactive";
import type { InteractiveCommand } from "./ws/types";
import { isMac, isWindows } from "./utils/platform";

export type ContextMenuItem =
  | { id: string; kind: "separator" }
  | {
      id: string;
      kind: "command" | "clipboard" | "platform" | "openWith" | "bro" | "globalSelection";
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
  canAddToGlobalSelection?: boolean;
  canCopy?: boolean;
  canRename?: boolean;
  canDelete?: boolean;
  canCopyFullPath?: boolean;
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
      id: "platform.file_manager",
      kind: "platform",
      label: "Open in Finder",
      actionId: "file_manager",
    };
  }
  if (isWindows) {
    return {
      id: "platform.file_manager",
      kind: "platform",
      label: "Show in Explorer",
      actionId: "file_manager",
    };
  }
  return {
    id: "platform.file_manager",
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
  const openWithItem: ContextMenuItem | null = options.canOpenWith && (kind === "file" || kind === "directory")
    ? { id: "platform.open_with", kind: "openWith", label: "Open With..." }
    : null;
  const broItem: ContextMenuItem | null = options.canPassToBro && (kind === "file" || kind === "directory")
    ? { id: "platform.pass_to_bro", kind: "bro", label: "Pass to Bro..." }
    : null;
  const globalSelectionItem: ContextMenuItem | null = options.canAddToGlobalSelection && (kind === "file" || kind === "directory")
    ? { id: "selection.to_global", kind: "globalSelection", label: "To global selection" }
    : null;

  return [
    ...(options.canPlatform ? [platformItem()] : []),
    ...(broItem ? [broItem] : []),
    ...(globalSelectionItem ? [globalSelectionItem] : []),
    ...(openWithItem ? [openWithItem] : []),
    ...(options.canCopy ? [commandItem("file.copy", isShadow ? "Extract" : "Copy", commands)] : []),
    ...(options.canRename ? [commandItem("file.rename", "Rename", commands)] : []),
    ...(options.canDelete ? [commandItem("file.delete", "Delete", commands)] : []),
    { id: "separator.clipboard", kind: "separator" },
    { id: "clipboard.name", kind: "clipboard", label: "Copy name", clipboardText: name },
    ...(options.canCopyFullPath ? [{ id: "clipboard.fullPath", kind: "clipboard", label: "Copy full path", clipboardText: fullPath } as ContextMenuItem] : []),
  ];
}
