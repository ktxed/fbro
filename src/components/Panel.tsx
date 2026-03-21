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

  // Keep focused panel's div focused so it receives keyboard events
  useEffect(() => {
    if (isFocused) {
      focusDivRef.current?.focus();
    }
  }, [isFocused]);

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
        client.sendIntent("MoveCursor", "focused-panel", { direction: "up" });
        break;
      case "ArrowDown":
        e.preventDefault();
        client.sendIntent("MoveCursor", "focused-panel", { direction: "down" });
        break;
      case "Enter":
        e.preventDefault();
        client.sendIntent("OpenEntry", "focused-panel");
        break;
      case "Backspace":
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
            className={`panel__row ${i === panel.cursor ? "panel__row--cursor" : ""}`}
            onMouseDown={() => {
              if (!isFocused) client.sendIntent("SwitchFocus", undefined, { to: panelId });
              client.sendIntent("SetCursor", "focused-panel", { index: i });
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
