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
import { stepZoom, resetZoom } from "../utils/zoom";
import {
  IconPencil,
  IconEye,
  IconEdit,
  IconFilePlus,
  IconCopy,
  IconArrowsMove,
  IconFolderPlus,
  IconTrash,
  IconTerminal,
  IconLogout,
} from "@tabler/icons-react";

function isEditableTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false;
  if (target.classList.contains("panel__filter-input")) return false;
  return target.isContentEditable
    || target instanceof HTMLInputElement
    || target instanceof HTMLTextAreaElement
    || target instanceof HTMLSelectElement;
}

const TOOLBAR_ICONS: Record<(typeof TOOLBAR_COMMAND_IDS)[number], React.ReactNode> = {
  "file.rename": <IconPencil size={16} />,
  "file.view": <IconEye size={16} />,
  "file.edit": <IconEdit size={16} />,
  "file.touch": <IconFilePlus size={16} />,
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
  "file.touch": "New File",
  "file.copy": "Copy",
  "file.move": "Move",
  "file.mkdir": "MkDir",
  "file.delete": "Delete",
  "app.open_terminal": "Shell",
  "app.quit": "Quit",
};

export function Toolbar({ client, jobsViewOpen, onOpenJobsView }: { client: WsClient; jobsViewOpen: boolean; onOpenJobsView: () => void; }) {
  const state = useAppState();
  const dispatch = useAppDispatch();

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (modKey(e) && (e.key === "=" || e.key === "+")) { e.preventDefault(); stepZoom(1); return; }
      if (modKey(e) && e.key === "-") { e.preventDefault(); stepZoom(-1); return; }
      if (modKey(e) && e.key === "0") { e.preventDefault(); resetZoom(); return; }

      const blockingModalActive = state.pendingOp || state.activeJobId || state.pendingQuit || state.pendingRename || state.pendingInfo || state.pendingMkdir || state.pendingTouch || state.pendingNoEditor || state.pendingGoToPath || state.pendingPanelHistory || state.pendingDryRun || state.pendingContextMenu || state.pendingOpenWith || state.pendingBro || state.pendingInput || jobsViewOpen;
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
      const command = findMatchingCommand(state.commands, GLOBAL_SHORTCUT_COMMAND_IDS, e);
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
  }, [client, dispatch, jobsViewOpen, onOpenJobsView, state]);

  const focusedPanel = state.panels[state.focus];
  const isShadow = !!focusedPanel?.shadowSource;
  const shadowDisabled = new Set(["file.rename", "file.move", "file.delete", "file.mkdir", "file.touch", "app.open_terminal"]);

  return (
    <div className="toolbar">
      {TOOLBAR_COMMAND_IDS.map((commandId) => {
        const command = getCommand(state.commands, commandId);
        const key = formatPrimaryBinding(command);
        const isDisabled = isShadow && shadowDisabled.has(commandId);
        const label = isShadow && commandId === "file.copy" ? "Extract" : TOOLBAR_LABELS[commandId];
        const detail = command?.title ?? label;
        const icon = TOOLBAR_ICONS[commandId];
        return (
          <button
            key={commandId}
            className={`toolbar__btn${isDisabled ? " toolbar__btn--disabled" : ""}`}
            tabIndex={-1}
            disabled={isDisabled}
            title={key ? `${key} — ${detail}` : detail}
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
