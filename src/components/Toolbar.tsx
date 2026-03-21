import { useEffect } from "react";
import { useAppState, useAppDispatch } from "../store";
import type { FileOpType, PendingOp, FileOpFile } from "../ws/types";

const ACTIONS = [
  { key: "F3",  label: "View",   icon: "👁️"  },
  { key: "F4",  label: "Edit",   icon: "✏️"  },
  { key: "F5",  label: "Copy",   icon: "📋"  },
  { key: "F6",  label: "Move",   icon: "✂️"  },
  { key: "F7",  label: "MkDir",  icon: "📁"  },
  { key: "F8",  label: "Delete", icon: "🗑️" },
  { key: "F9",  label: "Shell",  icon: "🖥️" },
  { key: "F10", label: "Quit",   icon: "🚪"  },
] as const;

const FILE_OP_KEYS: Record<string, FileOpType> = {
  F5: "copy",
  F6: "move",
  F8: "delete",
};

export function Toolbar() {
  const state = useAppState();
  const dispatch = useAppDispatch();

  const openFileOp = (opType: FileOpType) => {
    const focusedPanelId = state.focus;
    const panel = state.panels[focusedPanelId];
    if (!panel) return;

    // Files: selection or cursor item
    const indices = panel.selection.length > 0
      ? panel.selection
      : [panel.cursor];

    const files: FileOpFile[] = indices
      .map((idx) => panel.entries[idx])
      .filter(Boolean)
      .map((e) => ({ name: e.name, size: e.size, kind: e.kind }));

    if (files.length === 0) return;

    // Destination panel: the "other" panel (for 2 panels)
    const panelIds = state.layout.panels.length > 0 ? state.layout.panels : Object.keys(state.panels);
    const destPanelId = opType !== "delete"
      ? panelIds.find((id) => id !== focusedPanelId) ?? undefined
      : undefined;
    const destPath = destPanelId ? state.panels[destPanelId]?.path : undefined;

    const op: PendingOp = {
      opType,
      files,
      sourcePanel: focusedPanelId,
      destPanel: destPanelId,
      destPath,
      conflictPolicy: "ask",
    };

    dispatch({ type: "OPEN_OPERATION", op });
  };

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      const opType = FILE_OP_KEYS[e.key];
      if (opType) {
        e.preventDefault();
        openFileOp(opType);
        return;
      }
      if (ACTIONS.some(a => a.key === e.key)) {
        e.preventDefault();
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  });

  return (
    <div className="toolbar">
      {ACTIONS.map(({ key, label, icon }) => (
        <button
          key={key}
          className="toolbar__btn"
          tabIndex={-1}
          title={`${key} — ${label}`}
          onClick={() => {
            const opType = FILE_OP_KEYS[key];
            if (opType) openFileOp(opType);
          }}
        >
          <span className="toolbar__key">{key}</span>
          <span className="toolbar__icon">{icon}</span>
          <span className="toolbar__label">{label}</span>
        </button>
      ))}
    </div>
  );
}
