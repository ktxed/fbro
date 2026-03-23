import { useState, useEffect, useRef } from "react";
import type { PanelState, VFSEntry } from "../ws/types";
import type { WsClient } from "../ws/client";

interface PanelProps {
  panelId: string;
  panel: PanelState;
  isFocused: boolean;
  modalActive: boolean;
  client: WsClient;
}

function fallbackIcon(entry: VFSEntry): string {
  switch (entry.kind) {
    case "directory": return "▶";
    case "symlink":   return "↗";
    case "file":      return " ";
    default:          return "?";
  }
}

function EntryIcon({ entry }: { entry: VFSEntry }) {
  const [failed, setFailed] = useState(false);
  const iconName = entry.iconName;
  return (
    <span className="panel__col-icon">
      {iconName && !failed ? (
        <img
          src={`icon://localhost/${iconName}`}
          alt=""
          draggable={false}
          onError={() => setFailed(true)}
        />
      ) : (
        <span>{fallbackIcon(entry)}</span>
      )}
    </span>
  );
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

function formatMtime(mtime: string | undefined): string {
  if (!mtime) return "";
  const secs = parseInt(mtime, 10);
  if (isNaN(secs)) return "";
  const d = new Date(secs * 1000);
  const p = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function Panel({ panelId, panel, isFocused, modalActive, client }: PanelProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const focusDivRef = useRef<HTMLDivElement>(null);
  const rightDrag = useRef({ active: false, everMoved: false, startRow: -1, mode: "select" as "select" | "deselect", preSelection: [] as number[] });

  // Keep focused panel's div focused so it receives keyboard events.
  // Also re-focus when a modal closes (modalActive transitions to false).
  useEffect(() => {
    if (isFocused && !modalActive) {
      focusDivRef.current?.focus();
    }
  }, [isFocused, modalActive]);

  // End right-drag on mouse up anywhere; if a drag occurred, send final SetSelection
  useEffect(() => {
    const onMouseUp = (e: MouseEvent) => {
      if (e.button === 2) {
        rightDrag.current.active = false;
      }
    };
    window.addEventListener("mouseup", onMouseUp);
    return () => window.removeEventListener("mouseup", onMouseUp);
  }, []);

  // Scroll cursor into view within panel__list.
  // Uses getBoundingClientRect so the row position is always relative to the
  // list viewport, regardless of which element is the CSS offsetParent.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;
    const row = list.children[panel.cursor] as HTMLElement | undefined;
    if (!row) return;
    const listRect = list.getBoundingClientRect();
    const rowRect = row.getBoundingClientRect();
    const rowTop = rowRect.top - listRect.top + list.scrollTop;
    const rowBottom = rowTop + row.offsetHeight;
    if (rowTop < list.scrollTop) {
      list.scrollTop = rowTop;
    } else if (rowBottom > list.scrollTop + list.clientHeight) {
      list.scrollTop = rowBottom - list.clientHeight;
    }
  }, [panel.cursor]);

  function handleKeyDown(e: React.KeyboardEvent) {
    if (modalActive) {
      e.preventDefault();
      return;
    }
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
      case "PageUp":
      case "PageDown": {
        e.preventDefault();
        const list = listRef.current;
        if (!list) break;
        const firstRow = list.children[0] as HTMLElement | undefined;
        const rowHeight = firstRow ? firstRow.getBoundingClientRect().height : 1;
        const pageSize = Math.max(1, Math.floor(list.clientHeight / rowHeight));
        const delta = e.key === "PageDown" ? pageSize : -pageSize;
        client.sendIntent("MoveCursorBy", "focused-panel", { delta });
        break;
      }
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
        if (!isFocused && !modalActive) {
          client.sendIntent("SwitchFocus", undefined, { to: panelId });
        }
      }}
    >
      <div className="panel__header">
        <span className="panel__path">{panel.path}</span>
        <button
          className="panel__home-btn"
          tabIndex={-1}
          title="Navigate to home folder"
          onClick={() => client.sendIntent("NavigateHome", panelId)}
        >~</button>
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
                rightDrag.current = { active: true, everMoved: false, startRow: i, mode, preSelection: [...panel.selection] };
                return;
              }
              if (!isFocused) client.sendIntent("SwitchFocus", undefined, { to: panelId });
              client.sendIntent("SetCursor", "focused-panel", { index: i });
            }}
            onMouseEnter={() => {
              const drag = rightDrag.current;
              if (!drag.active) return;
              drag.everMoved = true;
              // Compute the full range [startRow..currentRow] and apply mode to preSelection
              const lo = Math.min(drag.startRow, i);
              const hi = Math.max(drag.startRow, i);
              const rangeSet = new Set<number>();
              for (let r = lo; r <= hi; r++) rangeSet.add(r);
              let indices: number[];
              if (drag.mode === "deselect") {
                indices = drag.preSelection.filter(idx => !rangeSet.has(idx));
              } else {
                const merged = new Set(drag.preSelection);
                rangeSet.forEach(r => merged.add(r));
                indices = [...merged];
              }
              client.sendIntent("SetSelection", "focused-panel", { indices });
            }}
            onContextMenu={(e) => {
              e.preventDefault();
              if (!rightDrag.current.everMoved) {
                // Pure right-click (no drag) → toggle
                client.sendIntent("ToggleSelectionAtIndex", "focused-panel", { index: i });
              }
            }}
            onDoubleClick={() => {
              if (entry.kind === "directory") client.sendIntent("OpenEntry", "focused-panel");
            }}
          >
            <EntryIcon entry={entry} />
            <span className="panel__name">{entry.name}</span>
            <span className="panel__size">{formatSize(entry)}</span>
            <span className="panel__mtime">{formatMtime(entry.mtime)}</span>
            <span className="panel__perms">{entry.permissions ?? ""}</span>
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
