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

function isEditableTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable
    || target instanceof HTMLInputElement
    || target instanceof HTMLTextAreaElement
    || target instanceof HTMLSelectElement;
}

const TOOLBAR_ICONS: Record<(typeof TOOLBAR_COMMAND_IDS)[number], string> = {
  "file.rename": "✏️",
  "file.view": "👁️",
  "file.edit": "🖊️",
  "file.copy": "📋",
  "file.move": "✂️",
  "file.mkdir": "📁",
  "file.delete": "🗑️",
  "app.open_terminal": "🖥️",
  "app.quit": "🚪",
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

export function Toolbar({ client, jobsViewOpen, onOpenJobsView }: { client: WsClient; jobsViewOpen: boolean; onOpenJobsView: () => void; }) {
  const state = useAppState();
  const dispatch = useAppDispatch();

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const blockingModalActive = state.pendingOp || state.activeJobId || state.pendingQuit || state.pendingRename || state.pendingInfo || state.pendingMkdir || state.pendingNoEditor || state.pendingDryRun || jobsViewOpen;
      const paletteCommand = getCommand(state.commands, COMMAND_PALETTE_COMMAND_ID);
      if (!blockingModalActive && paletteCommand && paletteCommand.bindings.some((binding) => bindingMatches(e, binding))) {
        e.preventDefault();
        executeCommand(paletteCommand.id, {
          state,
          dispatch,
          client,
          onOpenJobsView,
          onOpenCommandPalette: () => dispatch({ type: "OPEN_COMMAND_PALETTE" }),
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
      });
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [client, dispatch, jobsViewOpen, onOpenJobsView, state]);

  return (
    <div className="toolbar">
      {TOOLBAR_COMMAND_IDS.map((commandId) => {
        const command = getCommand(state.commands, commandId);
        const key = formatPrimaryBinding(command);
        const label = TOOLBAR_LABELS[commandId];
        const detail = command?.title ?? label;
        const icon = TOOLBAR_ICONS[commandId];
        return (
          <button
            key={commandId}
            className="toolbar__btn"
            tabIndex={-1}
            title={key ? `${key} — ${detail}` : detail}
            onClick={() => {
              executeCommand(commandId, {
                state,
                dispatch,
                client,
                onOpenJobsView,
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
