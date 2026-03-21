import { useEffect, useRef } from "react";
import type { PanelState, VFSEntry } from "../ws/types";
import type { WsClient } from "../ws/client";

interface PanelProps {
  panelId: string;
  panel: PanelState;
  isFocused: boolean;
  client: WsClient;
}

function entryIcon(entry: VFSEntry): string {
  switch (entry.kind) {
    case "directory": return "▶";
    case "symlink":   return "↗";
    case "file":      return " ";
    default:          return "?";
  }
}

function formatSize(entry: VFSEntry): string {
  if (entry.kind === "directory") return "<DIR>";
  if (entry.size === undefined) return "";
  const s = entry.size;
  if (s < 1024) return `${s}`;
  if (s < 1024 * 1024) return `${(s / 1024).toFixed(1)}K`;
  if (s < 1024 * 1024 * 1024) return `${(s / (1024 * 1024)).toFixed(1)}M`;
  return `${(s / (1024 * 1024 * 1024)).toFixed(1)}G`;
}

export function Panel({ panelId, panel, isFocused, client }: PanelProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const focusDivRef = useRef<HTMLDivElement>(null);
  const rightDrag = useRef({ active: false, moved: false, startRow: -1, mode: "select" as "select" | "deselect", visited: new Set<number>() });

  // Keep focused panel's div focused so it receives keyboard events
  useEffect(() => {
    if (isFocused) {
      focusDivRef.current?.focus();
    }
  }, [isFocused]);

  // End right-drag on mouse up anywhere
  useEffect(() => {
    const onMouseUp = (e: MouseEvent) => {
      if (e.button === 2) rightDrag.current.active = false;
    };
    window.addEventListener("mouseup", onMouseUp);
    return () => window.removeEventListener("mouseup", onMouseUp);
  }, []);

  // Scroll cursor into view
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const row = list.children[panel.cursor] as HTMLElement | undefined;
    row?.scrollIntoView({ block: "nearest" });
  }, [panel.cursor]);

  function handleKeyDown(e: React.KeyboardEvent) {
    switch (e.key) {
      case "ArrowUp":
        e.preventDefault();
        if (e.metaKey) {
          client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "first" });
        } else if (e.shiftKey) {
          client.sendIntent("MoveCursorWithSelect", "focused-panel", { direction: "up" });
        } else {
          client.sendIntent("MoveCursor", "focused-panel", { direction: "up" });
        }
        break;
      case "ArrowDown":
        e.preventDefault();
        if (e.metaKey) {
          client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "last" });
        } else if (e.shiftKey) {
          client.sendIntent("MoveCursorWithSelect", "focused-panel", { direction: "down" });
        } else {
          client.sendIntent("MoveCursor", "focused-panel", { direction: "down" });
        }
        break;
      case " ":
        e.preventDefault();
        client.sendIntent("ToggleSelection", "focused-panel");
        break;
      case "Home":
        e.preventDefault();
        client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "first" });
        break;
      case "End":
        e.preventDefault();
        client.sendIntent("MoveCursorToEdge", "focused-panel", { edge: "last" });
        break;
      case "Enter":
      case "ArrowRight":
        e.preventDefault();
        client.sendIntent("OpenEntry", "focused-panel");
        break;
      case "Backspace":
      case "ArrowLeft":
        e.preventDefault();
        client.sendIntent("NavigateUp", "focused-panel");
        break;
      case "Tab":
        e.preventDefault();
        client.sendIntent("SwitchFocus", undefined, {
          to: panelId === "left" ? "right" : "left",
        });
        break;
    }
  }

  const statusText =
    panel.status === "loading"
      ? "Loading…"
      : `${panel.entries.length} item${panel.entries.length !== 1 ? "s" : ""}`;

  return (
    <div
      className={`panel ${isFocused ? "panel--focused" : ""}`}
      ref={focusDivRef}
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onFocus={() => {
        if (!isFocused) {
          client.sendIntent("SwitchFocus", undefined, { to: panelId });
        }
      }}
    >
      <div className="panel__header">
        <span className="panel__path">{panel.path}</span>
      </div>

      <div className="panel__list" ref={listRef}>
        {panel.entries.map((entry, i) => (
          <div
            key={entry.name}
            className={[
              "panel__row",
              i === panel.cursor ? "panel__row--cursor" : "",
              panel.selection.includes(i) ? "panel__row--selected" : "",
            ].filter(Boolean).join(" ")}
            onMouseDown={(e) => {
              if (e.button === 2) {
                const mode = panel.selection.includes(i) ? "deselect" : "select";
                rightDrag.current = { active: true, moved: false, startRow: i, mode, visited: new Set([i]) };
                return;
              }
              if (!isFocused) client.sendIntent("SwitchFocus", undefined, { to: panelId });
              client.sendIntent("SetCursor", "focused-panel", { index: i });
            }}
            onMouseEnter={() => {
              const drag = rightDrag.current;
              if (!drag.active || drag.visited.has(i)) return;
              drag.visited.add(i);
              const intent = drag.mode === "deselect" ? "DeselectAtIndex" : "SelectAtIndex";
              if (!drag.moved) {
                drag.moved = true;
                client.sendIntent(intent, "focused-panel", { index: drag.startRow });
              }
              client.sendIntent(intent, "focused-panel", { index: i });
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              if (!rightDrag.current.moved) {
                // Pure right-click (no drag) → toggle
                client.sendIntent("ToggleSelectionAtIndex", "focused-panel", { index: i });
              }
            }}
            onDoubleClick={() => {
              if (entry.kind === "directory") client.sendIntent("OpenEntry", "focused-panel");
            }}
          >
            <span className="panel__icon">{entryIcon(entry)}</span>
            <span className="panel__name">{entry.name}</span>
            <span className="panel__size">{formatSize(entry)}</span>
          </div>
        ))}
        {panel.status === "loading" && panel.entries.length === 0 && (
          <div className="panel__loading">Loading…</div>
        )}
      </div>

      <div className="panel__footer">{statusText}</div>
    </div>
  );
}
