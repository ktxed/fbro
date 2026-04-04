import { formatPrimaryBinding } from "./commands/interactive";
import type { InteractiveCommand } from "./ws/types";
import { isMac, isWindows } from "./utils/platform";

export type ContextMenuItem =
  | { id: string; kind: "separator" }
  | {
      id: string;
      kind: "command" | "clipboard" | "platform";
      label: string;
      shortcut?: string;
      commandId?: string;
      clipboardText?: string;
      actionId?: string;
    };

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
): ContextMenuItem[] {
  return [
    platformItem(),
    commandItem("file.copy", "Copy", commands),
    commandItem("file.rename", "Rename", commands),
    commandItem("file.delete", "Delete", commands),
    { id: "separator.clipboard", kind: "separator" },
    { id: "clipboard.name", kind: "clipboard", label: "Copy name", clipboardText: name },
    { id: "clipboard.fullPath", kind: "clipboard", label: "Copy full path", clipboardText: fullPath },
  ];
}
