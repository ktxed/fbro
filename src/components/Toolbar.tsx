import { useEffect } from "react";
import { useAppState, useAppDispatch } from "../store";
import type { WsClient } from "../ws/client";
import type { FileOpType, PendingOp, FileOpFile } from "../ws/types";
import { modKey } from "../utils/platform";

const ACTIONS = [
  { key: "F2",  label: "Rename", icon: "✏️"  },
  { key: "F3",  label: "View",   icon: "👁️"  },
  { key: "F4",  label: "Edit",   icon: "🖊️"  },
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

/** Resolve an absolute index into the panel's local entries array (handles windowed mode). */
function entryAt(panel: { entries: { name: string; kind: string; size?: number; mtime?: string; permissions?: string }[]; cursor: number; windowStart?: number }, absIndex: number) {
  const localIdx = absIndex - (panel.windowStart ?? 0);
  return panel.entries[localIdx];
}

function openInfo(state: ReturnType<typeof useAppState>, dispatch: ReturnType<typeof useAppDispatch>) {
  const panel = state.panels[state.focus];
  if (!panel) return;
  const entry = entryAt(panel, panel.cursor);
  if (!entry) return;
  const dirPath = panel.path.replace(/\/+$/, "");
  dispatch({
    type: "OPEN_INFO",
    info: {
      name: entry.name,
      kind: entry.kind,
      fullPath: entry.name === ".." ? dirPath : `${dirPath}/${entry.name}`,
      dirPath,
      size: entry.size,
      mtime: entry.mtime,
      permissions: entry.permissions,
    },
  });
}

function openRename(state: ReturnType<typeof useAppState>, dispatch: ReturnType<typeof useAppDispatch>) {
  const panel = state.panels[state.focus];
  if (!panel) return;
  const entry = entryAt(panel, panel.cursor);
  if (!entry || entry.name === "..") return;
  const dirPath = panel.path.replace(/\/+$/, "");
  dispatch({
    type: "OPEN_RENAME",
    rename: {
      sourcePanel: state.focus,
      sourcePath: `${dirPath}/${entry.name}`,
      dirPath,
      filename: entry.name,
    },
  });
}

export function Toolbar({ client }: { client: WsClient }) {
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
      .map((idx) => entryAt(panel, idx))
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
      // Don't handle keys while a modal is active
      if (state.pendingOp || state.activeJobId || state.pendingQuit || state.pendingRename || state.pendingInfo || state.pendingMkdir || state.pendingNoEditor) return;

      if ((e.key === "i" || e.key === "I") && !modKey(e)) {
        e.preventDefault();
        openInfo(state, dispatch);
        return;
      }
      if (e.key === "F2") {
        e.preventDefault();
        openRename(state, dispatch);
        return;
      }
        if (e.key === "F3") {
        e.preventDefault();
        const panel = state.panels[state.focus];
        if (panel) {
          const entry = entryAt(panel, panel.cursor);
          if (entry && entry.kind !== "directory") {
            client.sendIntent("ViewFile", undefined, {
              panelId: state.focus,
              path: panel.path,
              sourceId: panel.sourceId,
              filename: entry.name,
            });
          }
        }
        return;
      }
      if (e.key === "F4") {
        e.preventDefault();
        const panel = state.panels[state.focus];
        if (panel) {
          const entry = entryAt(panel, panel.cursor);
          if (entry && entry.kind !== "directory") {
            client.sendIntent("EditFile", undefined, {
              path: panel.path,
              filename: entry.name,
            });
          }
        }
        return;
      }
      if (e.key === "F7") {
        e.preventDefault();
        const panel = state.panels[state.focus];
        if (panel) {
          dispatch({
            type: "OPEN_MKDIR",
            mkdir: { sourcePanel: state.focus, dirPath: panel.path },
          });
        }
        return;
      }
      if (e.key === "F10") {
        e.preventDefault();
        client.sendIntent("Quit");
        return;
      }
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
            if (key === "F2") { openRename(state, dispatch); return; }
            if (key === "F3") {
              const panel = state.panels[state.focus];
              if (panel) {
                const entry = entryAt(panel, panel.cursor);
                if (entry && entry.kind !== "directory") {
                  client.sendIntent("ViewFile", undefined, {
                    panelId: state.focus,
                    path: panel.path,
                    sourceId: panel.sourceId,
                    filename: entry.name,
                  });
                }
              }
              return;
            }
            if (key === "F4") {
              const panel = state.panels[state.focus];
              if (panel) {
                const entry = entryAt(panel, panel.cursor);
                if (entry && entry.kind !== "directory") {
                  client.sendIntent("EditFile", undefined, {
                    path: panel.path,
                    filename: entry.name,
                  });
                }
              }
              return;
            }
            if (key === "F7") {
              const panel = state.panels[state.focus];
              if (panel) {
                dispatch({
                  type: "OPEN_MKDIR",
                  mkdir: { sourcePanel: state.focus, dirPath: panel.path },
                });
              }
              return;
            }
            if (key === "F9") { client.sendIntent("OpenTerminal", state.focus); return; }
            if (key === "F10") { client.sendIntent("Quit"); return; }
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
