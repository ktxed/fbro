import { useEffect } from "react";
import { useAppState, useAppDispatch } from "../store";
import type { WsClient } from "../ws/client";
import {
  bindingMatches,
  COMMAND_PALETTE_COMMAND_ID,
  executeCommand,
  findMatchingCommand,
  formatPrimaryBinding,
  getCommand,
  GLOBAL_SHORTCUT_COMMAND_IDS,
  TOOLBAR_COMMAND_IDS,
} from "../commands/interactive";
import { modKey } from "../utils/platform";
import { isArchiveSourceId, isGlobalSelectionSourceId } from "../utils/sourceId";
import { applyZoom, nextZoomSize, DEFAULT_FONT_SIZE } from "../utils/zoom";
import {
  IconPencil,
  IconEye,
  IconEdit,
  IconCopy,
  IconArrowsMove,
  IconFolderPlus,
  IconTrash,
  IconTerminal,
  IconLogout,
} from "@tabler/icons-react";
import { useModalStack } from "./ModalStack";

function isEditableTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false;
  if (target.classList.contains("panel__filter-input")) return false;
  // Custom segmented date/time control: its segments are spans, not inputs,
  // but it consumes digit/arrow keys for editing — treat it like a text field.
  if (target.closest(".dt-input")) return true;
  return target.isContentEditable
    || target instanceof HTMLInputElement
    || target instanceof HTMLTextAreaElement
    || target instanceof HTMLSelectElement;
}

const TOOLBAR_ICONS: Record<(typeof TOOLBAR_COMMAND_IDS)[number], React.ReactNode> = {
  "file.rename": <IconPencil size={16} />,
  "file.view": <IconEye size={16} />,
  "file.edit": <IconEdit size={16} />,
  "file.copy": <IconCopy size={16} />,
  "file.move": <IconArrowsMove size={16} />,
  "file.mkdir": <IconFolderPlus size={16} />,
  "file.delete": <IconTrash size={16} />,
  "app.open_terminal": <IconTerminal size={16} />,
  "app.quit": <IconLogout size={16} />,
};

const TOOLBAR_LABELS: Record<(typeof TOOLBAR_COMMAND_IDS)[number], string> = {
  "file.rename": "Rename",
  "file.view": "View",
  "file.edit": "Edit",
  "file.copy": "Copy",
  "file.move": "Move",
  "file.mkdir": "MkDir",
  "file.delete": "Delete",
  "app.open_terminal": "Shell",
  "app.quit": "Quit",
};

export function Toolbar({ client, onOpenJobsView }: { client: WsClient; onOpenJobsView: () => void; }) {
  const state = useAppState();
  const dispatch = useAppDispatch();
  const { hasActiveModal } = useModalStack();

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (modKey(e) && (e.key === "=" || e.key === "+")) { e.preventDefault(); const s = nextZoomSize(state.fontSize, 1);  applyZoom(s); client.sendIntent("SetFontSize", undefined, { value: s }); return; }
      if (modKey(e) && e.key === "-")                    { e.preventDefault(); const s = nextZoomSize(state.fontSize, -1); applyZoom(s); client.sendIntent("SetFontSize", undefined, { value: s }); return; }
      if (modKey(e) && e.key === "0")                    { e.preventDefault(); applyZoom(DEFAULT_FONT_SIZE); client.sendIntent("SetFontSize", undefined, { value: DEFAULT_FONT_SIZE }); return; }

      const blockingModalActive = hasActiveModal || !!state.pendingContextMenu || !!state.pendingSourceMenu || !!state.bookmarksOpen;
      const paletteCommand = getCommand(state.commands, COMMAND_PALETTE_COMMAND_ID);
      if (!blockingModalActive && paletteCommand && paletteCommand.bindings.some((binding) => bindingMatches(e, binding))) {
        e.preventDefault();
        executeCommand(paletteCommand.id, {
          state,
          dispatch,
          client,
          onOpenJobsView,
          onOpenCommandPalette: () => dispatch({ type: "OPEN_COMMAND_PALETTE" }),
          onOpenContextMenu: () => dispatch({ type: "CLOSE_COMMAND_PALETTE" }),
        });
        return;
      }

      // Don't handle keys while a modal is active
      if (blockingModalActive || state.commandPaletteOpen) return;
      if (isEditableTarget(e.target)) return;
      const focusedMode = state.panels[state.focus]?.viewMode ?? "list";
      const command = findMatchingCommand(state.commands, GLOBAL_SHORTCUT_COMMAND_IDS, e, focusedMode);
      if (!command) return;
      e.preventDefault();
      executeCommand(command.id, {
        state,
        dispatch,
        client,
        onOpenJobsView,
        onOpenCommandPalette: () => dispatch({ type: "OPEN_COMMAND_PALETTE" }),
        onOpenContextMenu: () => dispatch({ type: "CLOSE_COMMAND_PALETTE" }),
      });
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [client, dispatch, hasActiveModal, onOpenJobsView, state]);

  const focusedPanel = state.panels[state.focus];
  const isArchive = !!focusedPanel?.shadowSource && isArchiveSourceId(focusedPanel.sourceId);
  const sourceSupports = focusedPanel
    ? (state.sources[focusedPanel.sourceId]?.supports ?? [])
    : [];
  const COMMAND_CAP: Partial<Record<string, string>> = {
    "file.rename":       "Rename",
    "file.copy":         "CopySelection",
    "file.edit":         "Edit",
    "file.move":         "MoveSelection",
    "file.mkdir":        "NewFolder",
    "file.delete":       "DeleteSelection",
    "app.open_terminal": "OpenShell",
  };

  return (
    <div className="toolbar">
      {TOOLBAR_COMMAND_IDS.map((commandId) => {
        const command = getCommand(state.commands, commandId);
        const key = formatPrimaryBinding(command);
        const cap = COMMAND_CAP[commandId];
        const globalSelectionRoot = !!focusedPanel && isGlobalSelectionSourceId(focusedPanel.sourceId) && focusedPanel.path === "";
        const globalSelectionAny = !!focusedPanel && isGlobalSelectionSourceId(focusedPanel.sourceId);
        const globalBlocked =
          (globalSelectionAny && (commandId === "file.view" || commandId === "file.edit" || commandId === "file.copy" || commandId === "file.move" || commandId === "file.mkdir" || commandId === "app.open_terminal"))
          || (globalSelectionAny && !globalSelectionRoot && commandId === "file.rename");
        const isDisabled = globalBlocked || (cap !== undefined && !sourceSupports.includes(cap));
        const label = isArchive && commandId === "file.copy" ? "Extract" : TOOLBAR_LABELS[commandId];
        const detail = command?.title ?? label;
        const icon = TOOLBAR_ICONS[commandId];
        return (
          <button
            key={commandId}
            className={`toolbar__btn${isDisabled ? " toolbar__btn--disabled" : ""}`}
            data-command-id={commandId}
            tabIndex={-1}
            disabled={isDisabled}
            title={key ? `${key} — ${detail}` : detail}
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => {
              if (isDisabled) return;
              executeCommand(commandId, {
                state,
                dispatch,
                client,
                onOpenJobsView,
                onOpenCommandPalette: () => dispatch({ type: "OPEN_COMMAND_PALETTE" }),
                onOpenContextMenu: () => dispatch({ type: "CLOSE_COMMAND_PALETTE" }),
              });
            }}
          >
            <span className="toolbar__key">{key}</span>
            <span className="toolbar__icon">{icon}</span>
            <span className="toolbar__label">{label}</span>
          </button>
        );
      })}
    </div>
  );
}
